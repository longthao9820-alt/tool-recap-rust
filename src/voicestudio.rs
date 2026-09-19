use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::{Client, multipart};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    config::Settings,
    media::CancellationToken,
    model::TranscriptSegment,
    paths::AppPaths,
};

const SERVER_START_TIMEOUT: Duration = Duration::from_secs(300);
const MODEL_INSTALL_TIMEOUT: Duration = Duration::from_secs(60 * 60 * 3);

#[derive(Debug, Clone, Deserialize)]
pub struct VoiceChoice {
    pub voice_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub description: String,
}

impl VoiceChoice {
    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.clone()
        } else if !self.description.trim().is_empty() {
            format!("{} — {}", self.voice_id, self.description)
        } else {
            self.voice_id.clone()
        }
    }
}

#[derive(Clone)]
pub struct VoiceStudioManager {
    paths: AppPaths,
    process: Arc<Mutex<Option<Child>>>,
}

impl VoiceStudioManager {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths, process: Arc::new(Mutex::new(None)) }
    }

    pub fn ensure_running(&self, settings: &Settings, cancel: &CancellationToken) -> Result<()> {
        if self.health(settings) { return Ok(()); }
        if !self.paths.voicestudio_backend.is_file() {
            bail!(
                "VoiceStudio runtime is missing at {}. Use a verified portable build or run the runtime packaging script.",
                self.paths.voicestudio_backend.display()
            );
        }

        {
            let mut guard = self.process.lock().map_err(|_| anyhow!("VoiceStudio process lock poisoned"))?;
            let needs_start = guard.as_mut().map(|child| child.try_wait().ok().flatten().is_some()).unwrap_or(true);
            if needs_start {
                let data = self.paths.data.join("voicestudio");
                let cache = self.paths.data.join("models").join("huggingface");
                fs::create_dir_all(&data)?;
                fs::create_dir_all(&cache)?;
                let mut command = Command::new(&self.paths.voicestudio_backend);
                command
                    .env("OMNIVOICE_PORT", port_from_url(&settings.voicestudio_url).to_string())
                    .env("OMNIVOICE_DATA_DIR", &data)
                    .env("HF_HOME", &cache)
                    .env("HF_HUB_CACHE", cache.join("hub"))
                    .env("OMNIVOICE_DISABLE_ANALYTICS", "1")
                    .env("OMNIVOICE_DESKTOP_CONTAINED", "1")
                    .env("PYTHONUNBUFFERED", "1")
                    .env("PYTHONUTF8", "1")
                    .env("FOR_DISABLE_CONSOLE_CTRL_HANDLER", "1")
                    .env("TORCHDYNAMO_DISABLE", "1")
                    .env("HF_HUB_DISABLE_SYMLINKS", "1")
                    .env("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")
                    .env_remove("PYTHONHOME")
                    .env_remove("PYTHONPATH")
                    .stdin(Stdio::piped())
                    .stdout(log_file(&self.paths.data.join("logs/voicestudio.out.log"))?)
                    .stderr(log_file(&self.paths.data.join("logs/voicestudio.err.log"))?);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    command.creation_flags(0x08000000);
                }
                *guard = Some(command.spawn().context("start bundled VoiceStudio backend")?);
            }
        }

        let started = Instant::now();
        while started.elapsed() < SERVER_START_TIMEOUT {
            cancel.check()?;
            if self.health(settings) { return Ok(()); }
            {
                let mut guard = self.process.lock().map_err(|_| anyhow!("VoiceStudio process lock poisoned"))?;
                if let Some(child) = guard.as_mut() {
                    if let Some(status) = child.try_wait()? {
                        bail!("VoiceStudio exited during startup with {status}. See data/logs/voicestudio.err.log.");
                    }
                }
            }
            thread::sleep(Duration::from_millis(750));
        }
        bail!("VoiceStudio did not become ready within {} seconds.", SERVER_START_TIMEOUT.as_secs())
    }

    pub fn health(&self, settings: &Settings) -> bool {
        let client = short_client();
        let base = settings.voicestudio_url.trim_end_matches('/');
        client.get(format!("{base}/system/info"))
            .send()
            .map(|response| response.status().is_success())
            .unwrap_or_else(|_| {
                short_client().get(format!("{base}/health"))
                    .send().map(|response| response.status().is_success()).unwrap_or(false)
            })
    }

    pub fn voices(&self, settings: &Settings, cancel: &CancellationToken) -> Result<Vec<VoiceChoice>> {
        self.ensure_running(settings, cancel)?;
        let url = format!("{}/v1/audio/voices", settings.voicestudio_url.trim_end_matches('/'));
        let response = short_client().get(url).send().context("query VoiceStudio voices")?;
        let status = response.status();
        let value: Value = response.json().context("parse VoiceStudio voices")?;
        if !status.is_success() { bail!("VoiceStudio voice list failed ({status}): {value}"); }
        let mut voices = Vec::new();
        for item in value.get("voices").and_then(Value::as_array).into_iter().flatten() {
            if let Ok(voice) = serde_json::from_value::<VoiceChoice>(item.clone()) {
                voices.push(voice);
            }
        }
        if voices.is_empty() {
            voices.push(VoiceChoice { voice_id: "default".into(), name: "VoiceStudio Default".into(), language: "en".into(), description: String::new() });
        }
        Ok(voices)
    }

    pub fn ensure_model<F>(&self, settings: &Settings, repo_id: &str, cancel: &CancellationToken, mut progress: F) -> Result<()>
    where F: FnMut(f32, &str) {
        if repo_id.trim().is_empty() { return Ok(()); }
        self.ensure_running(settings, cancel)?;
        if self.model_installed(settings, repo_id).unwrap_or(false) { return Ok(()); }

        progress(0.0, &format!("Downloading VoiceStudio model {repo_id}"));
        let url = format!("{}/models/install", settings.voicestudio_url.trim_end_matches('/'));
        let response = long_client().post(url).json(&json!({"repo_id": repo_id})).send().context("start VoiceStudio model install")?;
        let status = response.status();
        let body: Value = response.json().unwrap_or(Value::Null);
        if !status.is_success() { bail!("VoiceStudio model install failed ({status}): {body}"); }

        let started = Instant::now();
        let mut pulse = 0.02f32;
        while started.elapsed() < MODEL_INSTALL_TIMEOUT {
            cancel.check()?;
            if self.model_installed(settings, repo_id).unwrap_or(false) {
                progress(1.0, &format!("VoiceStudio model ready: {repo_id}"));
                return Ok(());
            }
            if let Ok(Some((fraction, message))) = self.model_progress(settings, repo_id) {
                progress(fraction.clamp(0.01, 0.98), &message);
            } else {
                pulse = (pulse + 0.015).min(0.92);
                progress(pulse, &format!("Downloading {repo_id}…"));
            }
            thread::sleep(Duration::from_secs(1));
        }
        bail!("Timed out while downloading VoiceStudio model {repo_id}.")
    }

    pub fn preview<F>(&self, settings: &Settings, text: &str, output: &Path, cancel: &CancellationToken, progress: F) -> Result<()>
    where F: FnMut(f32, &str) {
        self.ensure_model(settings, &settings.tts_model_repo, cancel, progress)?;
        self.synthesize(settings, text, output, cancel)
    }

    pub fn synthesize(&self, settings: &Settings, text: &str, output: &Path, cancel: &CancellationToken) -> Result<()> {
        cancel.check()?;
        if let Some(parent) = output.parent() { fs::create_dir_all(parent)?; }
        let url = format!("{}/v1/audio/speech", settings.voicestudio_url.trim_end_matches('/'));
        let payload = json!({
            "model": settings.voice_model,
            "input": text,
            "voice": if settings.voice_id.trim().is_empty() { "default" } else { settings.voice_id.as_str() },
            "response_format": "wav",
            "speed": 1.0,
            "language": settings.voice_language,
            "instruct": settings.voice_style,
        });

        for attempt in 0..5 {
            cancel.check()?;
            let response = long_client().post(&url).json(&payload).send().context("VoiceStudio speech request")?;
            let status = response.status();
            if status.is_success() {
                let bytes = response.bytes().context("read VoiceStudio speech")?;
                if bytes.len() < 44 { bail!("VoiceStudio returned an invalid WAV."); }
                fs::write(output, &bytes)?;
                return Ok(());
            }
            let retry_after = response.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or((attempt + 1) as u64 * 2);
            let body = response.text().unwrap_or_default();
            if (status.as_u16() == 429 || status.as_u16() == 503) && attempt < 4 {
                thread::sleep(Duration::from_secs(retry_after.min(30)));
                continue;
            }
            bail!("VoiceStudio speech failed ({status}): {}", body.chars().take(1000).collect::<String>());
        }
        bail!("VoiceStudio speech retries exhausted.")
    }

    pub fn transcribe<F>(&self, settings: &Settings, audio: &Path, cancel: &CancellationToken, progress: F) -> Result<Vec<TranscriptSegment>>
    where F: FnMut(f32, &str) {
        self.ensure_model(settings, &settings.asr_model_repo, cancel, progress)?;
        self.select_asr_model(settings, &settings.asr_model_repo)?;
        cancel.check()?;
        let url = format!("{}/v1/audio/transcriptions", settings.voicestudio_url.trim_end_matches('/'));
        let form = multipart::Form::new()
            .file("file", audio).context("attach extracted audio")?
            .text("model", "faster-whisper")
            .text("response_format", "verbose_json");
        let response = long_client().post(url).multipart(form).send().context("VoiceStudio transcription request")?;
        let status = response.status();
        let value: Value = response.json().context("parse VoiceStudio transcription")?;
        if !status.is_success() { bail!("VoiceStudio transcription failed ({status}): {value}"); }
        let segments = value.get("segments").and_then(Value::as_array).ok_or_else(|| anyhow!("VoiceStudio verbose transcript contains no segments"))?;
        let mut out = Vec::new();
        for segment in segments {
            let text = segment.get("text").and_then(Value::as_str).unwrap_or_default().trim();
            if text.is_empty() { continue; }
            let start = segment.get("start").and_then(Value::as_f64).unwrap_or(0.0);
            let end = segment.get("end").and_then(Value::as_f64).unwrap_or(start);
            if end > start { out.push(TranscriptSegment { start, end, text: text.to_owned() }); }
        }
        if out.is_empty() { bail!("VoiceStudio produced an empty transcript."); }
        Ok(out)
    }

    fn select_asr_model(&self, settings: &Settings, repo_id: &str) -> Result<()> {
        let url = format!("{}/engines/select", settings.voicestudio_url.trim_end_matches('/'));
        let response = short_client().post(url).json(&json!({
            "family": "asr",
            "backend_id": "faster-whisper",
            "model_id": repo_id
        })).send().context("select VoiceStudio ASR model")?;
        let status = response.status();
        let body: Value = response.json().unwrap_or(Value::Null);
        if !status.is_success() {
            bail!("VoiceStudio ASR selection failed ({status}): {body}");
        }
        Ok(())
    }

    fn model_installed(&self, settings: &Settings, repo_id: &str) -> Result<bool> {
        let url = format!("{}/models", settings.voicestudio_url.trim_end_matches('/'));
        let response = short_client().get(url).send()?;
        if !response.status().is_success() { return Ok(false); }
        let value: Value = response.json()?;
        Ok(find_model(&value, repo_id).map(model_is_installed).unwrap_or(false))
    }

    fn model_progress(&self, settings: &Settings, repo_id: &str) -> Result<Option<(f32, String)>> {
        let status_url = format!("{}/models/install/status", settings.voicestudio_url.trim_end_matches('/'));
        let response = short_client().get(status_url).send()?;
        if response.status().is_success() {
            let value: Value = response.json()?;
            if let Some(jobs) = value.get("jobs").and_then(Value::as_array) {
                if let Some(job) = jobs.iter().find(|job| job.get("repo_id").and_then(Value::as_str) == Some(repo_id)) {
                    let state = job.get("state").and_then(Value::as_str).unwrap_or("downloading");
                    if state == "failed" {
                        let message = job.get("error").or_else(|| job.get("message")).and_then(Value::as_str).unwrap_or("model download failed");
                        bail!("VoiceStudio model install failed for {repo_id}: {message}");
                    }
                    if let Some(fraction) = job_fraction(job) {
                        return Ok(Some((fraction, format!("Downloading {repo_id} — {:.0}%", fraction * 100.0))));
                    }
                    return Ok(Some((0.02, format!("Downloading {repo_id}…"))));
                }
            }
        }
        let url = format!("{}/models", settings.voicestudio_url.trim_end_matches('/'));
        let value: Value = short_client().get(url).send()?.json()?;
        let Some(model) = find_model(&value, repo_id) else { return Ok(None); };
        Ok(job_fraction(model).map(|fraction| {
            (fraction, format!("Downloading {repo_id} — {:.0}%", fraction * 100.0))
        }))
    }

    pub fn shutdown(&self) {
        if let Ok(mut guard) = self.process.lock() {
            if let Some(mut child) = guard.take() {
                #[cfg(windows)]
                {
                    let _ = Command::new("taskkill").args(["/PID", &child.id().to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
                }
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn job_fraction(value: &Value) -> Option<f32> {
    for key in ["progress", "download_progress", "fraction"] {
        if let Some(mut fraction) = value.get(key).and_then(Value::as_f64) {
            if fraction > 1.0 { fraction /= 100.0; }
            return Some(fraction.clamp(0.0, 1.0) as f32);
        }
    }
    let have = ["downloaded_bytes", "bytes_done", "size_on_disk"]
        .into_iter().find_map(|key| value.get(key).and_then(Value::as_u64)).unwrap_or(0);
    let total = ["total_bytes", "bytes_total", "size_bytes", "download_size_bytes"]
        .into_iter().find_map(|key| value.get(key).and_then(Value::as_u64)).unwrap_or(0);
    (total > 0).then(|| (have as f64 / total as f64).clamp(0.0, 1.0) as f32)
}

fn find_model<'a>(value: &'a Value, repo_id: &str) -> Option<&'a Value> {
    match value {
        Value::Array(items) => items.iter().find_map(|item| find_model(item, repo_id)),
        Value::Object(map) => {
            if map.get("repo_id").and_then(Value::as_str) == Some(repo_id) { return Some(value); }
            map.values().find_map(|item| find_model(item, repo_id))
        }
        _ => None,
    }
}

fn model_is_installed(value: &Value) -> bool {
    ["installed", "downloaded", "cached", "is_cached", "ready"]
        .into_iter().any(|key| value.get(key).and_then(Value::as_bool).unwrap_or(false))
        || value.get("status").and_then(Value::as_str).map(|s| matches!(s, "installed" | "ready" | "downloaded")).unwrap_or(false)
}

fn port_from_url(url: &str) -> u16 {
    url.rsplit(':').next().and_then(|part| part.trim_end_matches('/').parse().ok()).unwrap_or(3900)
}

fn short_client() -> Client {
    Client::builder().connect_timeout(Duration::from_secs(2)).timeout(Duration::from_secs(10)).build().expect("reqwest client")
}

fn long_client() -> Client {
    Client::builder().connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(60 * 30)).build().expect("reqwest client")
}

fn log_file(path: &Path) -> Result<Stdio> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    Ok(Stdio::from(fs::OpenOptions::new().create(true).append(true).open(path)?))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimePin {
    pub upstream: String,
    pub version: String,
    pub source_ref: String,
    pub api_contract: String,
}

pub fn bundled_runtime_pin(paths: &AppPaths) -> Option<RuntimePin> {
    let path: PathBuf = paths.runtime.join("voicestudio").join("version.json");
    fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str(&raw).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursive_model_lookup_handles_wrapped_catalog() {
        let value = json!({"models":[{"repo_id":"k2-fsa/OmniVoice","downloaded":true}]});
        let model = find_model(&value, "k2-fsa/OmniVoice").unwrap();
        assert!(model_is_installed(model));
    }
}
