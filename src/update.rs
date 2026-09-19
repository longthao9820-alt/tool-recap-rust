use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{media::CancellationToken, paths::AppPaths};

const LATEST_RELEASE: &str = "https://api.github.com/repos/longthao9820-alt/tool-recap-rust/releases/latest";

#[derive(Debug, Clone)]
pub struct AvailableUpdate {
    pub version: Version,
    pub tag: String,
    pub notes: String,
    pub zip_url: String,
    pub checksum_url: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    body: Option<String>,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

pub fn check_for_update() -> Result<Option<AvailableUpdate>> {
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    let client = Client::builder().timeout(Duration::from_secs(20)).user_agent("tool-recap-rust-updater").build()?;
    let response = client.get(LATEST_RELEASE).send().context("check GitHub Releases")?;
    if response.status().as_u16() == 404 { return Ok(None); }
    let status = response.status();
    let release: GithubRelease = response.json().context("parse GitHub release")?;
    if !status.is_success() { bail!("GitHub release check failed: {status}"); }
    let tag = release.tag_name.trim_start_matches('v');
    let version = Version::parse(tag).with_context(|| format!("parse release version {}", release.tag_name))?;
    if version <= current { return Ok(None); }
    let zip = release.assets.iter().find(|asset| asset.name == "tool-recap-rust-windows-portable.zip")
        .ok_or_else(|| anyhow!("Release {} has no portable Windows ZIP.", release.tag_name))?;
    let checksum = release.assets.iter().find(|asset| asset.name == "tool-recap-rust-windows-portable.zip.sha256")
        .ok_or_else(|| anyhow!("Release {} has no SHA-256 checksum.", release.tag_name))?;
    Ok(Some(AvailableUpdate {
        version,
        tag: release.tag_name,
        notes: release.body.unwrap_or_default(),
        zip_url: zip.browser_download_url.clone(),
        checksum_url: checksum.browser_download_url.clone(),
    }))
}

pub fn download_and_launch_update<F>(
    paths: &AppPaths,
    update: &AvailableUpdate,
    cancel: &CancellationToken,
    mut progress: F,
) -> Result<()>
where F: FnMut(f32, &str) {
    let updates = paths.data.join("updates").join(update.version.to_string());
    if updates.exists() { fs::remove_dir_all(&updates).ok(); }
    fs::create_dir_all(&updates)?;
    let archive = updates.join("portable.zip");
    let client = Client::builder().timeout(Duration::from_secs(60 * 30)).user_agent("tool-recap-rust-updater").build()?;
    let mut response = client.get(&update.zip_url).send().context("download update")?;
    if !response.status().is_success() { bail!("Update download failed: {}", response.status()); }
    let total = response.content_length().unwrap_or(0);
    let mut file = fs::File::create(&archive)?;
    let mut downloaded = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        cancel.check()?;
        let read = response.read(&mut buffer)?;
        if read == 0 { break; }
        file.write_all(&buffer[..read])?;
        downloaded += read as u64;
        let fraction = if total > 0 { downloaded as f32 / total as f32 } else { 0.25 };
        progress(fraction * 0.7, "Downloading application update");
    }
    file.flush()?;

    progress(0.72, "Verifying update checksum");
    let checksum_text = client.get(&update.checksum_url).send()?.error_for_status()?.text()?;
    let expected = checksum_text.split_whitespace().next().ok_or_else(|| anyhow!("Invalid checksum file"))?.to_ascii_lowercase();
    let actual = sha256_file(&archive)?;
    if expected != actual { bail!("Update checksum mismatch. Existing installation was not changed."); }

    progress(0.78, "Extracting update staging folder");
    let staging = updates.join("staging");
    fs::create_dir_all(&staging)?;
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", "Expand-Archive", "-LiteralPath"])
        .arg(&archive)
        .arg("-DestinationPath").arg(&staging)
        .arg("-Force")
        .stdout(Stdio::null()).stderr(Stdio::piped()).status().context("extract update ZIP")?;
    if !status.success() { bail!("Could not extract the update package."); }
    for required in ["tool-recap-rust.exe", "tool-recap-updater.exe"] {
        if !staging.join(required).is_file() { bail!("Update package is missing {required}."); }
    }

    progress(0.92, "Preparing safe updater handoff");
    let bundled_updater = paths.root.join("tool-recap-updater.exe");
    if !bundled_updater.is_file() { bail!("Portable updater executable is missing."); }
    let runner = updates.join("update-runner.exe");
    fs::copy(&bundled_updater, &runner)?;
    let exe_name = std::env::current_exe()?.file_name().and_then(|s| s.to_str()).unwrap_or("tool-recap-rust.exe").to_owned();
    Command::new(runner)
        .arg("--apply").arg(&staging)
        .arg("--target").arg(&paths.root)
        .arg("--pid").arg(std::process::id().to_string())
        .arg("--exe").arg(exe_name)
        .arg("--version").arg(update.version.to_string())
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().context("launch updater")?;
    progress(1.0, "Updater ready; restarting application");
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 { break; }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn portable_runtime_versions(paths: &AppPaths) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let pin = paths.runtime.join("voicestudio").join("version.json");
    if pin.is_file() { out.push(("VoiceStudio pin".into(), pin)); }
    let ffmpeg = paths.runtime.join("ffmpeg").join("VERSION.txt");
    if ffmpeg.is_file() { out.push(("FFmpeg pin".into(), ffmpeg)); }
    out
}
