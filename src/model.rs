use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "mov", "avi", "webm", "m4v", "ts"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EpisodeStage {
    Waiting,
    Analyzing,
    Preparing,
    Rendering,
    Completed,
    Stopped,
    Failed,
}

impl EpisodeStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting",
            Self::Analyzing => "Analyzing",
            Self::Preparing => "Preparing voice/video",
            Self::Rendering => "Rendering",
            Self::Completed => "Completed",
            Self::Stopped => "Stopped",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct QueueItem {
    pub source: PathBuf,
    pub name: String,
    pub stage: EpisodeStage,
    pub progress: f32,
    pub status: String,
    pub output: Option<PathBuf>,
}

impl QueueItem {
    pub fn new(source: PathBuf) -> Self {
        let name = source.file_stem().and_then(|s| s.to_str()).unwrap_or("Episode").to_owned();
        Self {
            source,
            name,
            stage: EpisodeStage::Waiting,
            progress: 0.0,
            status: "Ready".to_owned(),
            output: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecapPlan {
    pub title: String,
    pub segments: Vec<RecapSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecapSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub narration: String,
}

impl RecapPlan {
    pub fn validate(&self, source_duration_ms: u64) -> Result<(), String> {
        if self.title.trim().is_empty() {
            return Err("Analysis returned an empty title.".into());
        }
        if self.segments.is_empty() {
            return Err("Analysis returned no recap segments.".into());
        }
        let mut previous_end = 0;
        for (index, segment) in self.segments.iter().enumerate() {
            if segment.narration.trim().is_empty() {
                return Err(format!("Segment {} has empty narration.", index + 1));
            }
            if segment.start_ms >= segment.end_ms {
                return Err(format!("Segment {} has an invalid time range.", index + 1));
            }
            if segment.end_ms > source_duration_ms.saturating_add(250) {
                return Err(format!("Segment {} ends outside the source video.", index + 1));
            }
            if index > 0 && segment.start_ms < previous_end {
                return Err(format!("Segment {} overlaps the previous segment.", index + 1));
            }
            previous_end = segment.end_ms;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub video_codec: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_rejects_overlap_and_out_of_bounds() {
        let plan = RecapPlan {
            title: "x".into(),
            segments: vec![
                RecapSegment { start_ms: 0, end_ms: 5000, narration: "one".into() },
                RecapSegment { start_ms: 4000, end_ms: 6000, narration: "two".into() },
            ],
        };
        assert!(plan.validate(10_000).unwrap_err().contains("overlaps"));
    }
}
