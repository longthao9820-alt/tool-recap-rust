#![cfg_attr(windows, windows_subsystem = "windows")]

use std::{process::Command, sync::Arc};

use eframe::egui;
use serde_json::json;
use tool_recap_rust::{paths::AppPaths, ui::RecapApp, voicestudio::bundled_runtime_pin};

fn main() -> eframe::Result {
    let paths = AppPaths::discover();
    if std::env::args().any(|arg| arg == "--self-check") {
        let ok = self_check(&paths);
        std::process::exit(if ok { 0 } else { 1 });
    }
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/tool-recap.png"))
        .expect("embedded app icon must be valid");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Tool Recap Rust")
            .with_inner_size([1280.0, 840.0])
            .with_min_inner_size([900.0, 680.0])
            .with_icon(Arc::new(icon)),
        centered: true,
        renderer: eframe::Renderer::Glow,
        persistence_path: Some(paths.data.join("window-state.ron")),
        ..Default::default()
    };
    eframe::run_native(
        "Tool Recap Rust",
        options,
        Box::new(|cc| Ok(Box::new(RecapApp::new(cc)))),
    )
}

fn self_check(paths: &AppPaths) -> bool {
    let ffmpeg = paths.ffmpeg.is_file();
    let ffprobe = paths.ffprobe.is_file();
    let ffplay = paths.ffplay.is_file();
    let voice = paths.voicestudio_backend.is_file();
    let nvenc_compiled = if ffmpeg {
        Command::new(&paths.ffmpeg).args(["-hide_banner", "-encoders"]).output()
            .map(|out| String::from_utf8_lossy(&out.stdout).contains("h264_nvenc") || String::from_utf8_lossy(&out.stderr).contains("h264_nvenc"))
            .unwrap_or(false)
    } else { false };
    let pin = bundled_runtime_pin(paths);
    let ok = ffmpeg && ffprobe && ffplay && voice && nvenc_compiled && pin.is_some();
    println!("{}", serde_json::to_string_pretty(&json!({
        "ok": ok,
        "root": paths.root,
        "ffmpeg": ffmpeg,
        "ffprobe": ffprobe,
        "ffplay": ffplay,
        "h264_nvenc_compiled": nvenc_compiled,
        "voicestudio_backend": voice,
        "voicestudio_pin": pin,
        "note": "This package self-check validates bundled dependencies. Actual RTX hardware/driver capability is tested in the application before Start."
    })).unwrap());
    ok
}
