#![feature(io_error_more)]
#![feature(is_some_with)]

use anyhow::anyhow;
use clap::Parser;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use regex::Regex;
use std::ffi::OsStr;
use std::fs;
use std::io::BufReader;
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

    // Create the output directory.
    fs::create_dir_all(&args.inflate_dir)?;

    // Collect all of the patch files in the input directory.
    let patches = fs::read_dir(&args.patch_dir)?
        .filter_map(Result::ok)
        .filter(|d| d.metadata().is_ok_and(|m| m.is_file()))
        .filter(|d| d.path().extension() == Some(OsStr::from_bytes(b"patch")))
        .map(|d| d.path())
        .collect::<Vec<_>>();

    // Iterate over each patch and inflate it.
    patches.par_iter().for_each(|path| {
        inflate_patch(path, &args.inflate_dir).expect("failed to inflate patch");
    });
    Ok(())
}

fn inflate_patch(path: &Path, out: &Path) -> anyhow::Result<()> {
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
    let out_dir = out.join(&patch_name);
    fs::create_dir_all(&out_dir)?;

    // Extract the contents of the patch, to the destination
    zip_extract::extract(BufReader::new(&file), &out_dir, true).map_err(|e| anyhow!(e))?;

    // If the patch contains an archive filesystem, we'll extract it.
    let header_file = out_dir.join("update.sah");
    let data_file = out_dir.join("update.saf");
    if header_file.is_file() {
        let fs = libclient::fs::Filesystem::from_archive(&header_file, &data_file)?;
        fs.extract(&out_dir.join("data"))?;

        // Delete the archive files.
        fs::remove_file(&header_file)?;
        fs::remove_file(&data_file)?;
    }
    Ok(())
}
