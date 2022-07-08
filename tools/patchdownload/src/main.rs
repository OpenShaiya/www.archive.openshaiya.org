use anyhow::anyhow;
use clap::Parser;
use configparser::ini::Ini;
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::StatusCode;
use std::cmp::min;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter, IntoStaticStr};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// The directory to download the patch to.
    #[clap(short, long, value_parser)]
    patch_dir: PathBuf,
}

/// The valid client distributions.
#[derive(Display, Debug, EnumIter, IntoStaticStr)]
#[strum(ascii_case_insensitive)]
#[strum(serialize_all = "lowercase")]
enum Distribution {
    Us,
    De,
    Pt,
    Es,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    std::fs::create_dir_all(&args.patch_dir)?;
    for dist in Distribution::iter() {
        if let Err(e) = download_os_patches(&dist, &args.patch_dir).await {
            println!("failed to download {} patches: {}", dist, e);
        };
    }
    Ok(())
}

/// Download the patches for a distribution, from AeriaGame's servers.
///
/// # Arguments
/// * `dist`    - The distribution to download.
/// * `dir`     - The path to download the patches to.
async fn download_os_patches(dist: &Distribution, dir: &Path) -> anyhow::Result<()> {
    // Download the patch version info.
    let version_resp = reqwest::get(&format!(
        "http://shaiya-{}.patch.aeriagames.com/Shaiya/UpdateVersion.ini",
        dist
    ))
    .await?
    .text()
    .await?;

    // Parse the version info.
    let mut version_config = Ini::new();
    let _ = version_config.read(version_resp);

    // Get the highest patch number.
    let backup = version_config.get("Version", "SaveBackUp");
    let patch = version_config.get("Version", "PatchFileVersion");
    let latest_patch = if let Some(backup) = backup {
        u16::from_str(&backup)?
    } else if let Some(patch) = patch {
        u16::from_str(&patch)?
    } else {
        return Err(anyhow!("failed to get a valid patch number"));
    };

    // Create the distribution directory
    let dist_dir = dir.join(format!("shaiya-{}", dist));
    std::fs::create_dir_all(&dist_dir)?;

    // Download the patches
    for patch_number in 0..=latest_patch {
        let url = format!(
            "http://shaiya-{}.patch.aeriagames.com/Shaiya/patch/ps{:04}.patch",
            dist, patch_number
        );
        let resp = reqwest::get(&url).await?;
        if resp.status() != StatusCode::OK {
            println!("skipping patch {:04} - file doesn't exist", patch_number);
            continue;
        }
        let content_length = resp
            .content_length()
            .ok_or_else(|| anyhow!("failed to get content length from {}", url))?;

        // Progress bar setup
        let pb = ProgressBar::new(content_length);
        pb.set_style(ProgressStyle::default_bar()
            .template("{msg}\n{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
            .progress_chars("#>-"));
        pb.set_message(format!("Downloading {}", url));

        // Download the file in chunks
        let filepath = dist_dir.join(format!("ps{:04}.patch", patch_number));
        let mut file = std::fs::File::create(&filepath)?;
        let mut downloaded: u64 = 0;
        let mut stream = resp.bytes_stream();
        while let Some(item) = stream.next().await {
            let chunk = item.map_err(|_| anyhow!("error while downloading file"))?;
            file.write_all(&chunk)?;
            let new = min(downloaded + (chunk.len() as u64), content_length);
            downloaded = new;
            pb.set_position(new);
        }
    }
    Ok(())
}
