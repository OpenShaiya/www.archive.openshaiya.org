use anyhow::anyhow;
use ini::Ini;
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use serde::{Deserialize, Serialize};
use sqlite::{Connection, State};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use strum_macros::{Display, IntoStaticStr};
use uuid::Uuid;
use zip_extensions::zip_create_from_directory;

pub const AWS_S3_BUCKET: &str = "archive.openshaiya.org";

pub const GSCONFIG_TEMPLATE: &str = include_str!("../gsconfig.template.cfg");

pub const VERSION_TEMPLATE: &str = include_str!("../version.template.ini");

#[derive(Clone, Copy, PartialEq, Eq, Display, IntoStaticStr, Deserialize, Serialize)]
#[strum(serialize_all = "lowercase")]
#[serde(rename_all = "snake_case")]
pub enum Distribution {
    Us,
    De,
    Es,
    Pt,
    Ga,
}

struct ClientFile {
    path: String,
    key: String,
}

pub async fn build_client(
    conn: &Connection,
    dir: &Path,
    src: &Path,
    dist: Distribution,
    patch: u16,
    address: Option<String>,
) -> anyhow::Result<PathBuf> {
    let dest = create_temp_dir(dir, dist, patch)?;

    // Retrieve the relevant files and populate the directory.
    let collected_files = collect_dist_files(conn, dist, patch).await?;
    populate_client_directory(collected_files, src, &dest, dist, patch).await?;

    // Create the archive files.
    let fs_header_path = dest.join("data.sah");
    let fs_data_path = dest.join("data.saf");
    let data_path = dest.join("data");
    let mut fs_header_file = fs::File::create(&fs_header_path)?;

    // Build the archive, deleting the source file.
    let fs = libclient::fs::Filesystem::from_path(&data_path)?;
    let data_buf = fs.build_with_destination(&mut fs_header_file)?;

    // Delete the `data` folder and write the data file.
    fs::remove_dir_all(&data_path)?;
    fs::write(&fs_data_path, &data_buf)?;
    drop(data_buf);

    // Write the config files.
    let gsconfig = GSCONFIG_TEMPLATE.replace(
        "{address}",
        &address.unwrap_or_else(|| "127.0.0.1".to_string()),
    );
    let version = VERSION_TEMPLATE.replace("{patch}", &patch.to_string());
    fs::write(dest.join("gsconfig.cfg"), &gsconfig)?;
    fs::write(dest.join("version.ini"), &version)?;

    // Read the config.ini file;
    let config_path = dest.join("config.ini");
    let mut config = Ini::load_from_file(&config_path)?;

    // Set the user id, and TEST_IP=ENGLISH (this forces international clients to use gsconfig ip)
    config
        .with_section(Some("LOGIN"))
        .set("ID", "openshaiya")
        .set("TEST_IP", "ENGLISH");

    // Set the user id to save by default
    config
        .with_section(Some("INTERFACE"))
        .set("LOGIN_ID_SAVE", "TRUE");

    // Turn full-screen off my default, to avoid messing with users resolution unintentionally.
    config
        .with_section(Some("VIDEO"))
        .set("FULLSCREEN", "FALSE");

    config.write_to_file(&config_path)?;

    // Zip the file and remove the source folder.
    let zipped = zip(&dest, dir, &object_name(dist, patch)).await?;
    fs::remove_dir_all(&dest)?;
    Ok(zipped)
}

/// Get the formatted name of a distribution for a given patch number.
///
/// # Arguments
/// * `dist`    - The distribution.
/// * `patch`   - The patch.
pub fn object_name(dist: Distribution, patch: u16) -> String {
    format!("shaiya-{}-ps{:04}", dist, patch)
}

/// Creates a temporary directory, for storing the client files into. This will eventually
/// be built into a zip archive and then deleted.
///
/// # Arguments
/// * `dist`    - The client distribution.
/// * `patch`   - The requested patch number.
fn create_temp_dir(dir: &Path, dist: Distribution, patch: u16) -> anyhow::Result<PathBuf> {
    let dest = dir.join(format!("{}-{}", &object_name(dist, patch), Uuid::new_v4()));
    fs::create_dir_all(&dest)?;
    tracing::info!(?dest, "created temporary directory for client files");
    Ok(dest)
}

async fn collect_dist_files(
    conn: &Connection,
    dist: Distribution,
    patch: u16,
) -> anyhow::Result<Vec<ClientFile>> {
    let mut files = Vec::with_capacity(65535);
    let mut statement = conn.prepare(include_str!("../queries/files_for_dist.sql"))?;
    statement.bind::<&str>(1, dist.into())?;
    statement.bind::<i64>(2, patch as i64)?;

    while let State::Row = statement.next()? {
        let path = statement.read::<String>(0)?;
        let key = statement.read::<String>(1)?;
        files.push(ClientFile { path, key });
    }

    Ok(files)
}

/// Populates a client directory with the files for a specified path.
///
/// # Arguments
/// * `conn`    - The database connection.
/// * `s3       - The AWS s3 client.
/// * `dest`    - The directory to write the files to.
/// * `dist`    - The client distribution.
/// * `patch`   - The requested patch.
async fn populate_client_directory(
    files: Vec<ClientFile>,
    src: &Path,
    dest: &Path,
    dist: Distribution,
    patch: u16,
) -> anyhow::Result<()> {
    files
        .par_iter()
        .map(|file| {
            let ClientFile { path, key } = &file;
            let path = dest.join(&path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }

            let src_path = src.join(&key);
            let data = fs::read(&src_path)?;

            let mut dst = fs::File::create(&path)?;
            dst.write_all(&data)?;
            tracing::info!(?path, %key, %dist, patch, "wrote file");
            Ok(())
        })
        .collect::<anyhow::Result<()>>()
}

async fn zip(src: &Path, dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let archive_file = dir.join(format!("{}.zip", name));
    zip_create_from_directory(&archive_file, &src.to_path_buf())?;
    Ok(archive_file)
}

/// Normalizes a patch number for a specified distribution. If `patch` does not exist for a
/// distribution, it gets the next lowest available patch number.
///
/// # Arguments
/// * `conn`    - The connection to the database.
/// * `dist`    - The client distribution.
/// * `patch`   - The patch to search for.
pub fn normalize_patch(conn: &Connection, dist: Distribution, patch: u16) -> anyhow::Result<u16> {
    let mut statement = conn.prepare(include_str!("../queries/normalize_patch.sql"))?;
    statement.bind::<&str>(1, dist.into())?;
    statement.bind::<i64>(2, patch as i64)?;

    if let State::Done = statement.next()? {
        return Err(anyhow!("couldn't find patch for dist `{}`", dist));
    }
    Ok(statement.read::<i64>(0)? as u16)
}
