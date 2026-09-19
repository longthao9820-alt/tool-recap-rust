#![cfg_attr(windows, windows_subsystem = "windows")]

use std::{process::Command, sync::Arc};

use base64::Engine as _;
use eframe::egui;
use serde_json::json;
use tool_recap_rust::{paths::AppPaths, ui::RecapApp, voicestudio::bundled_runtime_pin};

const APP_ICON_PNG: &str = "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";

fn main() -> eframe::Result {
    let paths = AppPaths::discover();
    if std::env::args().any(|arg| arg == "--self-check") {
        let ok = self_check(&paths);
        std::process::exit(if ok { 0 } else { 1 });
    }
    let icon_bytes = base64::engine::general_purpose::STANDARD.decode(APP_ICON_PNG).expect("embedded app icon");
    let icon = eframe::icon_data::from_png_bytes(&icon_bytes).expect("embedded app icon must be valid");
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
        "note": "Bundled dependency check only; actual RTX hardware/driver capability is tested before Start."
    })).unwrap());
    ok
}
