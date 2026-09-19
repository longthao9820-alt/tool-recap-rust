use std::{fs, path::{Path, PathBuf}};

use anyhow::{Context, Result, bail};

use crate::model::VIDEO_EXTENSIONS;

pub fn is_supported_video(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|s| s.to_str())
            .map(|ext| VIDEO_EXTENSIONS.iter().any(|allowed| ext.eq_ignore_ascii_case(allowed)))
            .unwrap_or(false)
}

pub fn enumerate_input(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        if is_supported_video(path) {
            return Ok(vec![path.to_path_buf()]);
        }
        bail!("The selected file is not a supported video.");
    }
    if !path.is_dir() {
        bail!("The selected input does not exist.");
    }

    let mut videos = Vec::new();
    for entry in fs::read_dir(path).with_context(|| format!("read folder {}", path.display()))? {
        let entry = entry?;
        let item = entry.path();
        // Deliberately do not recurse. Folder mode is direct children only.
        if is_supported_video(&item) {
            videos.push(item);
        }
    }
    videos.sort_by(|a, b| natural_key(a).cmp(&natural_key(b)));
    if videos.is_empty() {
        bail!("No supported videos were found directly inside the selected folder.");
    }
    Ok(videos)
}

fn natural_key(path: &Path) -> Vec<NaturalPart> {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or_default().to_lowercase();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut numeric = None;
    for ch in name.chars() {
        let is_num = ch.is_ascii_digit();
        if let Some(was_num) = numeric {
            if was_num != is_num {
                push_part(&mut parts, &mut current, was_num);
            }
        }
        numeric = Some(is_num);
        current.push(ch);
    }
    if let Some(is_num) = numeric {
        push_part(&mut parts, &mut current, is_num);
    }
    parts
}

fn push_part(out: &mut Vec<NaturalPart>, current: &mut String, numeric: bool) {
    if current.is_empty() { return; }
    if numeric {
        out.push(NaturalPart::Number(current.parse().unwrap_or(u64::MAX)));
    } else {
        out.push(NaturalPart::Text(std::mem::take(current)));
        return;
    }
    current.clear();
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
enum NaturalPart {
    Text(String),
    Number(u64),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn folder_scan_is_non_recursive_and_ignores_non_video() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("ep10.mp4"), b"").unwrap();
        fs::write(dir.path().join("ep2.mkv"), b"").unwrap();
        fs::write(dir.path().join("notes.txt"), b"").unwrap();
        fs::create_dir(dir.path().join("season2")).unwrap();
        fs::write(dir.path().join("season2").join("ep3.mp4"), b"").unwrap();

        let names: Vec<_> = enumerate_input(dir.path()).unwrap().into_iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["ep2.mkv", "ep10.mp4"]);
    }
}
