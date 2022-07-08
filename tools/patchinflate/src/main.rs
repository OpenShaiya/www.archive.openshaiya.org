#![feature(io_error_more)]
#![feature(is_some_with)]

use anyhow::anyhow;
use clap::Parser;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use regex::Regex;
use std::ffi::OsStr;
use std::fs;
use std::io::{BufReader, Write};
use std::os::unix::prelude::OsStrExt;
use std::path::{Path, PathBuf};
use zip::{DateTime, ZipArchive};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// The directory containing the patch files.
    #[clap(short, long, value_parser)]
    patch_dir: PathBuf,

    /// The directory to extract the patch files to.
    #[clap(short, long, value_parser)]
    inflate_dir: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // If the `patch_dir` is not a valid directory, we should return early.
    if let Ok(metadata) = fs::metadata(&args.patch_dir) {
        if !metadata.is_dir() {
            return Err(anyhow!(std::io::ErrorKind::NotADirectory));
        }
    }

    // Create the output directories.
    let patch_dir = args.inflate_dir.join("patches");
    let client_dir = args.inflate_dir.join("clients");
    fs::create_dir_all(&patch_dir)?;
    fs::create_dir_all(&client_dir)?;

    // Collect all of the patch files in the input directory.
    let patches = fs::read_dir(&args.patch_dir)?
        .filter_map(Result::ok)
        .filter(|d| d.metadata().is_ok_and(|m| m.is_file()))
        .filter(|d| d.path().extension() == Some(OsStr::from_bytes(b"patch")))
        .map(|d| d.path())
        .collect::<Vec<_>>();

    // Iterate over each patch and inflate it.
    patches.par_iter().for_each(|path| {
        inflate_patch(path, &patch_dir, &client_dir).expect("failed to inflate patch");
    });
    Ok(())
}

fn inflate_patch(path: &Path, patch_dir: &Path, client_dir: &Path) -> anyhow::Result<()> {
    let re = Regex::new(r"(ps\d{4})")?;
    let captures = re.captures(path.to_str().unwrap()).unwrap();

    let patch = captures.get(1).unwrap().as_str();
    let file = fs::File::open(path)?;

    // Parse the patch file as a zip archive.
    let reader = BufReader::new(&file);
    let mut zip = ZipArchive::new(reader)?;

    // Find the most recent date within the archive.
    let mut date = DateTime::default();
    (0..zip.len()).for_each(|idx| {
        if let Ok(file) = zip.by_index(idx) {
            if file.last_modified().to_time().unwrap() > date.to_time().unwrap() {
                date = file.last_modified();
            }
        }
    });

    // Include the most recent date in the patch name.
    let patch_name = format!("{}-{}-{}-{}", patch, date.day(), date.month(), date.year());

    // Create the output directory.
    let patch_out_dir = patch_dir.join(&patch_name);
    fs::create_dir_all(&patch_out_dir)?;

    // Extract the contents of the patch, to the destination
    zip_extract::extract(BufReader::new(&file), &patch_out_dir, true).map_err(|e| anyhow!(e))?;

    // If the patch contains an archive filesystem, we'll extract it.
    let header_file = patch_out_dir.join("update.sah");
    let data_file = patch_out_dir.join("update.saf");
    if header_file.is_file() {
        let fs = libclient::fs::Filesystem::from_archive(&header_file, &data_file)?;
        fs.extract(&patch_out_dir.join("data"))?;

        // Delete the archive files.
        fs::remove_file(&header_file)?;
        fs::remove_file(&data_file)?;
    }

    // If the patch contains a game client, we'll create a copy in the `client_dir`
    let client_file = patch_out_dir.join("game.exe");
    if client_file.is_file() {
        let client_buf = fs::read(&client_file)?;
        let mut client = fs::File::create(&client_dir.join(format!("{}-game.exe", patch_name)))?;
        client.write_all(&client_buf)?;
    }
    Ok(())
}
