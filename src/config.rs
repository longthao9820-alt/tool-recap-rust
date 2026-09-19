use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub api_endpoint: String,
    pub api_key: String,
    pub api_model: String,
    pub analysis_language: String,
    pub voice_id: String,
    pub voice_model: String,
    pub voice_language: String,
    pub voice_style: String,
    pub voicestudio_url: String,
    pub tts_model_repo: String,
    pub asr_model_repo: String,
    pub output_subdirectory: String,
    pub burn_subtitles: bool,
    pub notify_complete: bool,
    pub check_updates_on_start: bool,
    pub last_input_directory: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_endpoint: "http://127.0.0.1:20128/v1".to_owned(),
            api_key: String::new(),
            api_model: "gpt-5.6-sol".to_owned(),
            analysis_language: "English".to_owned(),
            voice_id: "default".to_owned(),
            voice_model: "omnivoice".to_owned(),
            voice_language: "en".to_owned(),
            voice_style: "clear documentary narration".to_owned(),
            voicestudio_url: "http://127.0.0.1:3900".to_owned(),
            tts_model_repo: "k2-fsa/OmniVoice".to_owned(),
            asr_model_repo: "deepdml/faster-whisper-large-v3-turbo-ct2".to_owned(),
            output_subdirectory: "recaps_da_render".to_owned(),
            burn_subtitles: false,
            notify_complete: true,
            check_updates_on_start: true,
            last_input_directory: String::new(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let temp = path.with_extension("partial");
        fs::write(&temp, serde_json::to_vec_pretty(self)?).with_context(|| format!("write {}", temp.display()))?;
        fs::rename(&temp, path).with_context(|| format!("replace {}", path.display()))?;
        Ok(())
    }
}
