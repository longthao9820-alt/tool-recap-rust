use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::Sender,
    thread,
};

use anyhow::{Context, Result, bail};

use crate::{
    analyzer,
    config::Settings,
    media::{self, CancellationToken, SegmentRender},
    model::{EpisodeStage, QueueItem, RecapPlan},
    paths::AppPaths,
    voicestudio::VoiceStudioManager,
};

#[derive(Debug, Clone)]
pub enum PipelineEvent {
    Episode {
        index: usize,
        stage: EpisodeStage,
        progress: f32,
        status: String,
        output: Option<PathBuf>,
    },
    SystemStatus(String),
    BatchFinished {
        completed: usize,
        failed: usize,
        stopped: bool,
    },
}

#[derive(Debug, Clone)]
struct PreparedSegment {
    voice: PathBuf,
    subtitle: PathBuf,
    voice_duration: f64,
}

pub fn spawn_batch(
    items: Vec<QueueItem>,
    settings: Settings,
    paths: AppPaths,
    voicestudio: VoiceStudioManager,
    cancel: CancellationToken,
    sender: Sender<PipelineEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut completed = 0usize;
        let mut failed = 0usize;
        let gpu = media::detect_nvidia(&paths, &cancel);
        if !gpu.available {
            let message = format!("NVIDIA RTX rendering is unavailable: {}", gpu.reason);
            let _ = sender.send(PipelineEvent::SystemStatus(message.clone()));
            if !items.is_empty() {
                let _ = sender.send(PipelineEvent::Episode { index: 0, stage: EpisodeStage::Failed, progress: 0.0, status: message, output: None });
                failed = 1;
            }
            let _ = sender.send(PipelineEvent::BatchFinished { completed, failed, stopped: false });
            return;
        }
        let _ = sender.send(PipelineEvent::SystemStatus(format!(
            "GPU ready: {}{} • H.264 NVENC • NVDEC {}",
            gpu.gpu_name,
            if gpu.driver_version.is_empty() { String::new() } else { format!(" / driver {}", gpu.driver_version) },
            if gpu.nvdec_codecs.is_empty() { "not available".into() } else { gpu.nvdec_codecs.join(", ") }
        )));

        for (index, item) in items.iter().enumerate() {
            if cancel.is_cancelled() {
                let _ = sender.send(PipelineEvent::BatchFinished { completed, failed, stopped: true });
                return;
            }
            match process_episode(index, item, &settings, &paths, &voicestudio, &gpu, &cancel, &sender) {
                Ok(output) => {
                    completed += 1;
                    let _ = sender.send(PipelineEvent::Episode {
                        index,
                        stage: EpisodeStage::Completed,
                        progress: 1.0,
                        status: "Completed".into(),
                        output: Some(output),
                    });
                }
                Err(err) if cancel.is_cancelled() || err.to_string().contains("Stopped by user") => {
                    let _ = sender.send(PipelineEvent::Episode {
                        index,
                        stage: EpisodeStage::Stopped,
                        progress: item.progress,
                        status: "Stopped".into(),
                        output: None,
                    });
                    let _ = sender.send(PipelineEvent::BatchFinished { completed, failed, stopped: true });
                    return;
                }
                Err(err) => {
                    failed += 1;
                    let message = compact_error(&err);
                    let _ = sender.send(PipelineEvent::Episode {
                        index,
                        stage: EpisodeStage::Failed,
                        progress: item.progress,
                        status: message,
                        output: None,
                    });
                    let _ = sender.send(PipelineEvent::BatchFinished { completed, failed, stopped: false });
                    return;
                }
            }
        }
        let _ = sender.send(PipelineEvent::BatchFinished { completed, failed, stopped: false });
    })
}

fn process_episode(
    index: usize,
    item: &QueueItem,
    settings: &Settings,
    paths: &AppPaths,
    voicestudio: &VoiceStudioManager,
    gpu: &media::GpuCapabilities,
    cancel: &CancellationToken,
    sender: &Sender<PipelineEvent>,
) -> Result<PathBuf> {
    let safe_id = safe_filename(&item.name);
    let job_root = paths.data.join("jobs").join(&safe_id);
    if job_root.exists() { let _ = fs::remove_dir_all(&job_root); }
    fs::create_dir_all(&job_root)?;

    emit(sender, index, EpisodeStage::Analyzing, 0.01, "Inspecting source video");
    let media_info = media::probe_media(paths, &item.source, cancel)?;
    cancel.check()?;

    emit(sender, index, EpisodeStage::Analyzing, 0.06, "Extracting audio for VoiceStudio transcription");
    let extracted_audio = job_root.join("source.wav");
    media::extract_audio(paths, &item.source, &extracted_audio, cancel)?;

    emit(sender, index, EpisodeStage::Analyzing, 0.10, "Preparing VoiceStudio speech recognition");
    let transcript = voicestudio.transcribe(settings, &extracted_audio, cancel, |model_progress, message| {
        emit(sender, index, EpisodeStage::Analyzing, 0.10 + model_progress * 0.18, message);
    })?;
    cancel.check()?;

    emit(sender, index, EpisodeStage::Analyzing, 0.31, "Analyzing transcript and building recap plan");
    let source_name = item.source.file_name().and_then(|s| s.to_str()).unwrap_or(&item.name);
    let plan = analyzer::analyze(settings, source_name, &media_info, &transcript, cancel)?;
    persist_plan(&job_root, &plan)?;
    emit(sender, index, EpisodeStage::Analyzing, 0.40, &format!("Analysis complete • {} recap segments", plan.segments.len()));

    emit(sender, index, EpisodeStage::Preparing, 0.42, "Preparing VoiceStudio narration model");
    voicestudio.ensure_model(settings, &settings.tts_model_repo, cancel, |model_progress, message| {
        emit(sender, index, EpisodeStage::Preparing, 0.42 + model_progress * 0.08, message);
    })?;

    let voice_dir = job_root.join("voice");
    let subtitle_dir = job_root.join("subtitles");
    fs::create_dir_all(&voice_dir)?;
    fs::create_dir_all(&subtitle_dir)?;
    let mut prepared = Vec::with_capacity(plan.segments.len());
    for (segment_index, segment) in plan.segments.iter().enumerate() {
        cancel.check()?;
        let fraction = segment_index as f32 / plan.segments.len().max(1) as f32;
        emit(sender, index, EpisodeStage::Preparing, 0.50 + fraction * 0.20, &format!("Generating narration {}/{}", segment_index + 1, plan.segments.len()));
        let voice = voice_dir.join(format!("voice-{segment_index:03}.wav"));
        voicestudio.synthesize(settings, &segment.narration, &voice, cancel)?;
        let voice_duration = media::audio_duration_seconds(paths, &voice, cancel)?;
        if voice_duration <= 0.05 { bail!("VoiceStudio produced empty narration for segment {}.", segment_index + 1); }
        let subtitle = subtitle_dir.join(format!("segment-{segment_index:03}.srt"));
        fs::write(&subtitle, one_cue_srt(&segment.narration, voice_duration))?;
        prepared.push(PreparedSegment { voice, subtitle, voice_duration });
    }
    emit(sender, index, EpisodeStage::Preparing, 0.70, "Narration and render inputs are ready");

    let render_dir = job_root.join("rendered");
    fs::create_dir_all(&render_dir)?;
    let mut rendered = Vec::with_capacity(plan.segments.len());
    let mut final_srt_rows = Vec::new();
    let mut timeline = 0.0f64;
    for (segment_index, (segment, prep)) in plan.segments.iter().zip(prepared.iter()).enumerate() {
        cancel.check()?;
        let fraction = segment_index as f32 / plan.segments.len().max(1) as f32;
        emit(sender, index, EpisodeStage::Rendering, 0.72 + fraction * 0.25, &format!("Rendering segment {}/{} with NVENC", segment_index + 1, plan.segments.len()));
        let output = render_dir.join(format!("segment-{segment_index:03}.mp4"));
        media::render_segment(paths, gpu, SegmentRender {
            source: &item.source,
            voice: &prep.voice,
            output: &output,
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
            voice_duration: prep.voice_duration,
            source_codec: &media_info.video_codec,
            subtitle: settings.burn_subtitles.then_some(prep.subtitle.as_path()),
        }, cancel)?;
        rendered.push(output);
        final_srt_rows.push((timeline, timeline + prep.voice_duration, segment.narration.clone()));
        timeline += prep.voice_duration;
    }

    emit(sender, index, EpisodeStage::Rendering, 0.98, "Finalizing episode");
    let output_root = item.source.parent().unwrap_or(Path::new(".")).join(safe_component(&settings.output_subdirectory));
    fs::create_dir_all(&output_root)?;
    let source_stem = item.source.file_stem().and_then(|s| s.to_str()).unwrap_or(&item.name);
    let title = if plan.title.trim().is_empty() { source_stem.to_owned() } else { format!("{source_stem} - {}", plan.title.trim()) };
    let output_name = format!("{}.mp4", safe_filename(&title));
    let final_output = output_root.join(output_name);
    media::concat_segments(paths, &rendered, &job_root.join("concat.txt"), &final_output, cancel)?;
    let final_srt = final_output.with_extension("narration.srt");
    fs::write(final_srt, multi_cue_srt(&final_srt_rows))?;
    let _ = fs::remove_file(extracted_audio);
    Ok(final_output)
}

fn persist_plan(job_root: &Path, plan: &RecapPlan) -> Result<()> {
    fs::write(job_root.join("recap-plan.json"), serde_json::to_vec_pretty(plan)?).context("persist recap plan")
}

fn emit(sender: &Sender<PipelineEvent>, index: usize, stage: EpisodeStage, progress: f32, status: &str) {
    let _ = sender.send(PipelineEvent::Episode {
        index,
        stage,
        progress: progress.clamp(0.0, 1.0),
        status: status.to_owned(),
        output: None,
    });
}

fn safe_component(value: &str) -> String {
    let cleaned = safe_filename(value);
    if cleaned.is_empty() { "recaps_da_render".into() } else { cleaned }
}

fn safe_filename(value: &str) -> String {
    let invalid = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let mut out = value.chars()
        .map(|ch| if invalid.contains(&ch) || ch.is_control() { '_' } else { ch })
        .collect::<String>();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    out = out.trim_matches([' ', '.']).to_owned();
    if out.len() > 120 { out.truncate(120); }
    if out.is_empty() { "recap".into() } else { out }
}

fn one_cue_srt(text: &str, duration: f64) -> String {
    format!("1\n00:00:00,000 --> {}\n{}\n", srt_time(duration), text.trim())
}

fn multi_cue_srt(rows: &[(f64, f64, String)]) -> String {
    rows.iter().enumerate().map(|(index, (start, end, text))| {
        format!("{}\n{} --> {}\n{}\n", index + 1, srt_time(*start), srt_time(*end), text.trim())
    }).collect::<Vec<_>>().join("\n")
}

fn srt_time(seconds: f64) -> String {
    let ms = (seconds.max(0.0) * 1000.0).round() as u64;
    let hours = ms / 3_600_000;
    let minutes = (ms % 3_600_000) / 60_000;
    let secs = (ms % 60_000) / 1000;
    let millis = ms % 1000;
    format!("{hours:02}:{minutes:02}:{secs:02},{millis:03}")
}

fn compact_error(error: &anyhow::Error) -> String {
    let text = format!("{error:#}");
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames_are_windows_safe() {
        assert_eq!(safe_filename("A:B / C?"), "A_B _ C_");
    }

    #[test]
    fn srt_time_formats_milliseconds() {
        assert_eq!(srt_time(61.234), "00:01:01,234");
    }
}
