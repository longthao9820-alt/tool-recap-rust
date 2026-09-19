use std::{env, path::PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
    pub data: PathBuf,
    pub runtime: PathBuf,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub ffplay: PathBuf,
    pub voicestudio_backend: PathBuf,
    pub settings: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Self {
        let exe = env::current_exe().unwrap_or_else(|_| PathBuf::from("tool-recap-rust.exe"));
        let exe_dir = exe.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let root = if exe_dir.join("runtime").exists() || exe_dir.join("data").exists() {
            exe_dir
        } else {
            env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };
        let data = root.join("data");
        let runtime = root.join("runtime");
        let ffmpeg_dir = runtime.join("ffmpeg").join("bin");
        let voicestudio_backend = runtime
            .join("voicestudio")
            .join("backend")
            .join(if cfg!(windows) { "omnivoice-backend.exe" } else { "omnivoice-backend" });
        Self {
            settings: data.join("settings.json"),
            ffmpeg: ffmpeg_dir.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" }),
            ffprobe: ffmpeg_dir.join(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" }),
            ffplay: ffmpeg_dir.join(if cfg!(windows) { "ffplay.exe" } else { "ffplay" }),
            root,
            data,
            runtime,
            voicestudio_backend,
        }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for path in [
            self.data.clone(),
            self.data.join("cache"),
            self.data.join("jobs"),
            self.data.join("models"),
            self.data.join("updates"),
            self.data.join("logs"),
        ] {
            std::fs::create_dir_all(path)?;
        }
        Ok(())
    }
}
