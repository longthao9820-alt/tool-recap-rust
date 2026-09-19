use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use serde_json::{Value, json};

use crate::{config::Settings, media::CancellationToken, model::{MediaInfo, RecapPlan, TranscriptSegment}};

pub fn analyze(
    settings: &Settings,
    source_name: &str,
    media: &MediaInfo,
    transcript: &[TranscriptSegment],
    cancel: &CancellationToken,
) -> Result<RecapPlan> {
    cancel.check()?;
    if settings.api_endpoint.trim().is_empty() || settings.api_model.trim().is_empty() {
        bail!("AI API endpoint and model must be configured before Start.");
    }
    let transcript_text = transcript.iter().map(|item| {
        format!("[{:.3}-{:.3}] {}", item.start, item.end, item.text)
    }).collect::<Vec<_>>().join("\n");

    let system = format!(
        "You are a precise video recap editor. Return JSON only. Create concise {} narration using only facts supported by the timestamped transcript. Never invent timestamps. The output schema is exactly {{\"title\":string,\"segments\":[{{\"start_ms\":integer,\"end_ms\":integer,\"narration\":string}}]}}. Segments must be chronological, non-overlapping, within the source duration, and each narration must describe the selected source interval. Prefer 6-24 segments and a finished recap around 15%-30% of source duration. Do not add markdown.",
        settings.analysis_language
    );
    let user = format!(
        "SOURCE: {source_name}\nDURATION_MS: {}\nVIDEO_CODEC: {}\nSIZE: {}x{}\n\nTIMESTAMPED TRANSCRIPT:\n{}",
        media.duration_ms, media.video_codec, media.width, media.height, transcript_text
    );

    let endpoint = chat_endpoint(&settings.api_endpoint);
    let body = json!({
        "model": settings.api_model,
        "messages": [
            {"role":"system","content":system},
            {"role":"user","content":user}
        ],
        "temperature": 0.2
    });
    let client = Client::builder().connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(60 * 20)).build()?;
    let mut request = client.post(endpoint).json(&body);
    if !settings.api_key.trim().is_empty() {
        request = request.bearer_auth(settings.api_key.trim());
    }
    let response = request.send().context("send analysis request")?;
    let status = response.status();
    let value: Value = response.json().context("parse analysis API response")?;
    if !status.is_success() { bail!("Analysis API failed ({status}): {value}"); }
    let content = value.pointer("/choices/0/message/content").and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Analysis API response did not contain choices[0].message.content"))?;
    let cleaned = strip_code_fence(content);
    let plan: RecapPlan = serde_json::from_str(cleaned).context("parse recap plan JSON")?;
    plan.validate(media.duration_ms).map_err(anyhow::Error::msg)?;
    Ok(plan)
}

fn chat_endpoint(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") { base.to_owned() } else { format!("{base}/chat/completions") }
}

fn strip_code_fence(input: &str) -> &str {
    let trimmed = input.trim();
    if !trimmed.starts_with("```") { return trimmed; }
    let after_first = trimmed.find('\n').map(|i| &trimmed[i + 1..]).unwrap_or(trimmed);
    after_first.strip_suffix("```").unwrap_or(after_first).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_normalizes_v1_base() {
        assert_eq!(chat_endpoint("http://localhost:1234/v1/"), "http://localhost:1234/v1/chat/completions");
    }

    #[test]
    fn removes_markdown_fence_without_touching_json() {
        assert_eq!(strip_code_fence("```json\n{\"x\":1}\n```"), "{\"x\":1}");
    }
}
