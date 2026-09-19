#![cfg_attr(windows, windows_subsystem = "windows")]

use std::{
    collections::HashSet,
    env,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};

fn main() {
    if let Err(error) = run() {
        let log = env::temp_dir().join("tool-recap-update-error.log");
        let _ = fs::write(log, format!("{error:#}"));
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let staged = arg_path(&args, "--apply")?;
    let target = arg_path(&args, "--target")?;
    let pid: u32 = arg_value(&args, "--pid")?.parse()?;
    let exe = arg_value(&args, "--exe")?;
    let version = arg_value(&args, "--version").unwrap_or_else(|_| "unknown".into());
    if !staged.is_dir() || !target.is_dir() { bail!("Invalid update paths."); }

    wait_for_exit(pid, Duration::from_secs(120))?;
    let backup = target.join("data").join("update-backups").join(format!("{}-{}", version, unix_seconds()));
    fs::create_dir_all(&backup)?;

    let staged_files = collect_files(&staged)?;
    let mut backed_up = HashSet::new();
    let mut installed = Vec::new();
    for source in &staged_files {
        let rel = source.strip_prefix(&staged)?;
        if rel.components().next().map(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case("data")).unwrap_or(false) {
            continue;
        }
        let destination = target.join(rel);
        if destination.exists() {
            let backup_file = backup.join(rel);
            if let Some(parent) = backup_file.parent() { fs::create_dir_all(parent)?; }
            fs::copy(&destination, &backup_file).with_context(|| format!("backup {}", destination.display()))?;
            backed_up.insert(rel.to_path_buf());
        }
        if let Some(parent) = destination.parent() { fs::create_dir_all(parent)?; }
        let temp = destination.with_extension("update-partial");
        if let Err(error) = fs::copy(source, &temp).and_then(|_| {
            if destination.exists() { fs::remove_file(&destination)?; }
            fs::rename(&temp, &destination)
        }) {
            let _ = rollback(&target, &backup, &installed, &backed_up);
            return Err(error).context("apply update");
        }
        installed.push(rel.to_path_buf());
    }

    let executable = target.join(exe);
    if !executable.is_file() {
        rollback(&target, &backup, &installed, &backed_up)?;
        bail!("Updated application executable is missing; rolled back.");
    }
    Command::new(executable).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().context("restart updated application")?;
    Ok(())
}

fn rollback(target: &Path, backup: &Path, installed: &[PathBuf], backed_up: &HashSet<PathBuf>) -> Result<()> {
    for rel in installed.iter().rev() {
        let destination = target.join(rel);
        if backed_up.contains(rel) {
            let source = backup.join(rel);
            if source.is_file() {
                let _ = fs::copy(source, destination);
            }
        } else {
            let _ = fs::remove_file(destination);
        }
    }
    Ok(())
}

fn collect_files(root: &Path) -> Result<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() { walk(&path, out)?; } else if path.is_file() { out.push(path); }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn wait_for_exit(pid: u32, timeout: Duration) -> Result<()> {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if !pid_running(pid) { return Ok(()); }
        thread::sleep(Duration::from_millis(250));
    }
    bail!("Timed out waiting for application process to exit.")
}

fn pid_running(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let filter = format!("PID eq {pid}");
        return Command::new("tasklist").args(["/FI", &filter, "/FO", "CSV", "/NH"])
            .output().map(|out| String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())).unwrap_or(false);
    }
    #[cfg(not(windows))]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

fn arg_value(args: &[String], key: &str) -> Result<String> {
    let pos = args.iter().position(|value| value == key).ok_or_else(|| anyhow!("Missing {key}"))?;
    args.get(pos + 1).cloned().ok_or_else(|| anyhow!("Missing value for {key}"))
}

fn arg_path(args: &[String], key: &str) -> Result<PathBuf> { Ok(PathBuf::from(arg_value(args, key)?)) }
fn unix_seconds() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
