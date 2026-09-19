use std::{
    ffi::OsStr,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::{model::MediaInfo, paths::AppPaths};

#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) { self.0.store(true, Ordering::SeqCst); }
    pub fn reset(&self) { self.0.store(false, Ordering::SeqCst); }
    pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::SeqCst) }
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() { bail!("Stopped by user") } else { Ok(()) }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GpuCapabilities {
    pub available: bool,
    pub gpu_name: String,
    pub driver_version: String,
    pub nvenc_h264: bool,
    pub nvdec_codecs: Vec<String>,
    pub reason: String,
}

pub fn probe_media(paths: &AppPaths, source: &Path, cancel: &CancellationToken) -> Result<MediaInfo> {
    ensure_file(&paths.ffprobe, "FFprobe")?;
    let args = [
        "-v", "error", "-select_streams", "v:0",
        "-show_entries", "stream=codec_name,width,height:format=duration",
        "-of", "json",
    ];
    let mut cmd = Command::new(&paths.ffprobe);
    cmd.args(args).arg(source);
    let output = run_capture(cmd, cancel)?;
    if !output.status.success() {
        bail!("FFprobe failed: {}", stderr_text(&output));
    }
    let json: Value = serde_json::from_slice(&output.stdout).context("parse FFprobe output")?;
    let stream = json.get("streams").and_then(Value::as_array).and_then(|s| s.first())
        .ok_or_else(|| anyhow!("No video stream was found in {}", source.display()))?;
    let duration = json.pointer("/format/duration").and_then(Value::as_str)
        .and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    if duration <= 0.0 { bail!("Source video duration is invalid."); }
    Ok(MediaInfo {
        duration_ms: (duration * 1000.0).round() as u64,
        width: stream.get("width").and_then(Value::as_u64).unwrap_or(0) as u32,
        height: stream.get("height").and_then(Value::as_u64).unwrap_or(0) as u32,
        video_codec: stream.get("codec_name").and_then(Value::as_str).unwrap_or("unknown").to_owned(),
    })
}

pub fn extract_audio(paths: &AppPaths, source: &Path, output: &Path, cancel: &CancellationToken) -> Result<()> {
    ensure_file(&paths.ffmpeg, "FFmpeg")?;
    if let Some(parent) = output.parent() { fs::create_dir_all(parent)?; }
    let mut cmd = Command::new(&paths.ffmpeg);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(source)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(output);
    let result = run_capture(cmd, cancel)?;
    if !result.status.success() || !output.is_file() {
        bail!("Could not extract audio: {}", stderr_text(&result));
    }
    Ok(())
}

pub fn audio_duration_seconds(paths: &AppPaths, audio: &Path, cancel: &CancellationToken) -> Result<f64> {
    let mut cmd = Command::new(&paths.ffprobe);
    cmd.args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(audio);
    let result = run_capture(cmd, cancel)?;
    if !result.status.success() { bail!("FFprobe audio failed: {}", stderr_text(&result)); }
    let value = String::from_utf8_lossy(&result.stdout).trim().parse::<f64>()?;
    Ok(value)
}

pub fn detect_nvidia(paths: &AppPaths, cancel: &CancellationToken) -> GpuCapabilities {
    if !paths.ffmpeg.is_file() {
        return GpuCapabilities { reason: "Bundled FFmpeg is missing.".into(), ..Default::default() };
    }
    let encoders = command_text(&paths.ffmpeg, &["-hide_banner", "-encoders"], cancel).unwrap_or_default();
    let decoders = command_text(&paths.ffmpeg, &["-hide_banner", "-decoders"], cancel).unwrap_or_default();
    let nvenc_h264 = encoders.contains("h264_nvenc");
    let mut gpu_name = "NVIDIA GPU".to_owned();
    let mut driver_version = String::new();
    if let Ok(text) = command_text(Path::new("nvidia-smi"), &["--query-gpu=name,driver_version", "--format=csv,noheader"], cancel) {
        if let Some(line) = text.lines().next() {
            let mut fields = line.split(',').map(str::trim);
            gpu_name = fields.next().unwrap_or("NVIDIA GPU").to_owned();
            driver_version = fields.next().unwrap_or("").to_owned();
        }
    }
    let nvdec_codecs = ["h264_cuvid", "hevc_cuvid", "av1_cuvid", "vp9_cuvid", "mpeg2_cuvid"]
        .into_iter().filter(|codec| decoders.contains(codec)).map(str::to_owned).collect::<Vec<_>>();
    if !nvenc_h264 {
        return GpuCapabilities { gpu_name, driver_version, nvenc_h264, nvdec_codecs, reason: "Bundled FFmpeg does not expose h264_nvenc.".into(), available: false };
    }

    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut cmd = Command::new(&paths.ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=320x180:d=0.1:r=25", "-frames:v", "2", "-c:v", "h264_nvenc", "-f", "null", null]);
    match run_capture(cmd, cancel) {
        Ok(output) if output.status.success() => GpuCapabilities { available: true, gpu_name, driver_version, nvenc_h264, nvdec_codecs, reason: "H.264 NVENC hardware test passed.".into() },
        Ok(output) => GpuCapabilities { gpu_name, driver_version, nvenc_h264, nvdec_codecs, reason: format!("NVENC is present but the hardware test failed: {}", stderr_text(&output)), available: false },
        Err(err) => GpuCapabilities { gpu_name, driver_version, nvenc_h264, nvdec_codecs, reason: format!("NVENC test failed: {err}"), available: false },
    }
}

pub struct SegmentRender<'a> {
    pub source: &'a Path,
    pub voice: &'a Path,
    pub output: &'a Path,
    pub start_ms: u64,
    pub end_ms: u64,
    pub voice_duration: f64,
    pub source_codec: &'a str,
    pub subtitle: Option<&'a Path>,
}

pub fn render_segment(
    paths: &AppPaths,
    gpu: &GpuCapabilities,
    spec: SegmentRender<'_>,
    cancel: &CancellationToken,
) -> Result<()> {
    cancel.check()?;
    ensure_file(&paths.ffmpeg, "FFmpeg")?;
    if !gpu.available { bail!("NVIDIA H.264 NVENC is required: {}", gpu.reason); }
    if let Some(parent) = spec.output.parent() { fs::create_dir_all(parent)?; }

    let source_seconds = (spec.end_ms.saturating_sub(spec.start_ms)) as f64 / 1000.0;
    if source_seconds <= 0.0 || spec.voice_duration <= 0.0 { bail!("Invalid segment timing."); }
    let speed = (source_seconds / spec.voice_duration).clamp(0.75, 1.15);
    let stretched = source_seconds / speed;
    let pad = (spec.voice_duration - stretched).max(0.0);
    let decode = nvdec_decoder(spec.source_codec).filter(|name| gpu.nvdec_codecs.iter().any(|item| item == name));

    let result = run_segment_command(paths, &spec, speed, pad, decode, cancel)?;
    if result.status.success() && spec.output.is_file() { return Ok(()); }

    if decode.is_some() {
        let retry = run_segment_command(paths, &spec, speed, pad, None, cancel)?;
        if retry.status.success() && spec.output.is_file() { return Ok(()); }
        bail!("FFmpeg render failed after NVDEC fallback: {}", stderr_text(&retry));
    }
    bail!("FFmpeg render failed: {}", stderr_text(&result));
}

fn run_segment_command(
    paths: &AppPaths,
    spec: &SegmentRender<'_>,
    speed: f64,
    pad: f64,
    decoder: Option<&str>,
    cancel: &CancellationToken,
) -> Result<Output> {
    let start = spec.start_ms as f64 / 1000.0;
    let duration = (spec.end_ms - spec.start_ms) as f64 / 1000.0;
    let mut cmd = Command::new(&paths.ffmpeg);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error"]);
    if let Some(decoder) = decoder {
        cmd.args(["-hwaccel", "cuda", "-hwaccel_output_format", "cuda", "-c:v", decoder]);
    }
    cmd.args(["-ss", &format!("{start:.3}"), "-t", &format!("{duration:.3}"), "-i"])
        .arg(spec.source)
        .arg("-i").arg(spec.voice);

    let mut filter = String::new();
    if decoder.is_some() { filter.push_str("hwdownload,format=nv12,"); }
    filter.push_str(&format!("setpts=PTS/{speed:.6},trim=duration={:.3}", spec.voice_duration));
    if pad > 0.005 { filter.push_str(&format!(",tpad=stop_mode=clone:stop_duration={pad:.3}")); }
    if let Some(srt) = spec.subtitle {
        let escaped = ffmpeg_filter_path(srt);
        filter.push_str(&format!(",subtitles='{escaped}'"));
    }
    filter.push_str(",format=yuv420p");

    cmd.args(["-map", "0:v:0", "-map", "1:a:0", "-vf", &filter,
        "-c:v", "h264_nvenc", "-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", "19", "-b:v", "0",
        "-c:a", "aac", "-b:a", "192k", "-ar", "48000", "-movflags", "+faststart", "-shortest"])
        .arg(spec.output);
    run_capture(cmd, cancel)
}

pub fn concat_segments(paths: &AppPaths, segments: &[PathBuf], list_path: &Path, output: &Path, cancel: &CancellationToken) -> Result<()> {
    if segments.is_empty() { bail!("No rendered segments to concatenate."); }
    let mut list = String::new();
    for segment in segments {
        list.push_str("file '");
        list.push_str(&segment.to_string_lossy().replace('\\', "/").replace('\'', "'\\''"));
        list.push_str("'\n");
    }
    fs::write(list_path, list)?;
    let mut cmd = Command::new(&paths.ffmpeg);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"])
        .arg(list_path).args(["-c", "copy", "-movflags", "+faststart"]).arg(output);
    let result = run_capture(cmd, cancel)?;
    if !result.status.success() || !output.is_file() { bail!("Final concat failed: {}", stderr_text(&result)); }
    Ok(())
}

pub fn play_audio(paths: &AppPaths, path: &Path) -> Result<()> {
    ensure_file(&paths.ffplay, "FFplay")?;
    Command::new(&paths.ffplay)
        .args(["-nodisp", "-autoexit", "-loglevel", "quiet"])
        .arg(path)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().context("start voice preview")?;
    Ok(())
}

pub fn run_capture(mut command: Command, cancel: &CancellationToken) -> Result<Output> {
    cancel.check()?;
    command.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().context("start child process")?;
    loop {
        if cancel.is_cancelled() {
            kill_tree(child.id());
            let _ = child.kill();
            let _ = child.wait();
            bail!("Stopped by user");
        }
        if child.try_wait()?.is_some() {
            return child.wait_with_output().context("collect child process output");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

fn command_text(program: &Path, args: &[&str], cancel: &CancellationToken) -> Result<String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    let output = run_capture(cmd, cancel)?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr))
}

fn nvdec_decoder(codec: &str) -> Option<&'static str> {
    match codec.to_ascii_lowercase().as_str() {
        "h264" => Some("h264_cuvid"),
        "hevc" | "h265" => Some("hevc_cuvid"),
        "av1" => Some("av1_cuvid"),
        "vp9" => Some("vp9_cuvid"),
        "mpeg2video" => Some("mpeg2_cuvid"),
        _ => None,
    }
}

fn ffmpeg_filter_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:")
        .replace('\'', "\\'")
}

fn stderr_text(output: &Output) -> String {
    let text = String::from_utf8_lossy(&output.stderr);
    text.trim().chars().take(1200).collect()
}

fn ensure_file(path: &Path, label: &str) -> Result<()> {
    if path.is_file() { Ok(()) } else { bail!("{label} is missing: {}", path.display()) }
}

pub fn read_response_limited<R: Read>(mut reader: R, limit: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    reader.by_ref().take(limit as u64).read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_codecs_to_nvdec() {
        assert_eq!(nvdec_decoder("h264"), Some("h264_cuvid"));
        assert_eq!(nvdec_decoder("hevc"), Some("hevc_cuvid"));
        assert_eq!(nvdec_decoder("prores"), None);
    }

    #[test]
    fn subtitle_path_is_filter_safe_on_windows() {
        let escaped = ffmpeg_filter_path(Path::new(r"C:\Video Recap\caption.srt"));
        assert!(escaped.contains("C\\:"));
        assert!(escaped.contains("Video Recap"));
    }
}
