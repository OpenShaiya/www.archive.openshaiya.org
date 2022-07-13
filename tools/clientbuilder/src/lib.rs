use anyhow::anyhow;
use chrono::NaiveDateTime;
use flate2::write::GzEncoder;
use flate2::Compression;
use ini::Ini;
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use serde::{Deserialize, Serialize};
use sqlite::{Connection, State};
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use strum_macros::{Display, IntoStaticStr};
use tar::{Builder, EntryType, Header};
use uuid::Uuid;

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
    uncompressed_size: i64,
    epoch: u64,
}

pub async fn build_client<'a>(
    conn: &Connection,
    dir: &Path,
    src: &Path,
    dist: Distribution,
    patch: u16,
    address: Option<String>,
) -> anyhow::Result<PathBuf> {
    let dest = create_temp_dir(dir, dist, patch)?;

    // Retrieve the relevant files and populate the directory.
    // TODO: This really shouldn't even be a step (for the `data` directory). We should be able
    // to just skip this entirely and serialize directly to the data.saf file. That can be an optimisation
    // for the future, however.
    let collected_files = collect_dist_files(conn, dist, patch).await?;
    populate_client_directory(&collected_files, src, &dest, dist, patch).await?;

    // Get the most recent timestamp
    let most_recent_timestamp = collected_files.iter().map(|f| f.epoch).max().unwrap();

    // Create are gzipped tarball for the file data.
    let tar_gz = dest.join("game.tar.gz");
    let tar_gz_file = File::create(&tar_gz)?;
    let gzip = GzEncoder::new(tar_gz_file, Compression::fast());
    let mut tar = Builder::new(gzip);

    // Create the archive files.
    let fs_header_path = dest.join("data.sah");
    let data_path = dest.join("data");
    let mut fs_header_file = File::create(&fs_header_path)?;

    let total_uncompressed_size: usize = collected_files
        .par_iter()
        .map(|f| f.uncompressed_size as usize)
        .sum();

    // Build the data file in memory, and then copy it to the file stream.
    let mut data_buf: Vec<u8> = Vec::with_capacity(total_uncompressed_size);
    let fs = libclient::fs::Filesystem::from_path(&data_path)?;
    fs.build_with_destination(&mut fs_header_file, &mut data_buf)?;
    compress_file(
        &mut tar,
        "data.saf",
        &data_buf,
        data_buf.len(),
        most_recent_timestamp,
    )?;

    // Delete the data directory.
    tracing::info!(?data_path, "deleting data path to reclaim disk space...");
    fs::remove_dir_all(&data_path)?;

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

    // Collect all of the files in the root destination directory, and add them to the archive.
    tracing::info!("adding misc files to archive...");
    fs::read_dir(&dest)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|e| e.is_file() && e.to_str().unwrap() != tar_gz.to_str().unwrap())
        .for_each(|path| {
            let filename = path.file_name().expect("no file").to_str().unwrap();
            tracing::info!(filename, "appending file");

            // Read the file data and write it to the archive
            let buf = fs::read(&path).expect("failed to read file data");
            compress_file(&mut tar, filename, &buf, buf.len(), most_recent_timestamp)
                .expect("failed to add file to archive");
        });
    tar.finish()?;
    Ok(tar_gz)
}

fn compress_file<D: Write>(
    archive: &mut Builder<D>,
    name: &str,
    data: &[u8],
    data_len: usize,
    timestamp: u64,
) -> anyhow::Result<()> {
    let mut header = Header::new_gnu();
    header.set_size(data_len as u64);
    header.set_path(name)?;
    header.set_mtime(timestamp);
    header.set_entry_type(EntryType::Regular);
    header.set_mode(777);
    header.set_cksum();
    archive.append(&header, data)?;
    Ok(())
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
        let uncompressed_size = statement.read::<i64>(2)?;
        let date = statement.read::<String>(3)?;

        let date = NaiveDateTime::parse_from_str(&date, "%Y-%m-%d %H:%M:%S")?;

        files.push(ClientFile {
            path,
            key,
            uncompressed_size,
            epoch: date.timestamp() as u64,
        });
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
    files: &[ClientFile],
    src: &Path,
    dest: &Path,
    dist: Distribution,
    patch: u16,
) -> anyhow::Result<()> {
    files
        .par_iter()
        .map(|file| {
            let ClientFile { path, key, .. } = &file;
            let path = dest.join(&path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }

            let src_path = src.join(&key);
            let data = fs::read(&src_path)?;

            let mut dst = fs::File::create(&path)?;
            dst.write_all(&data)?;
            tracing::trace!(?path, %key, %dist, patch, "wrote file");
            Ok(())
        })
        .collect::<anyhow::Result<()>>()
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
