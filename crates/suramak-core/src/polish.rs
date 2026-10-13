use serde::{Deserialize, Serialize};

use crate::settings::PolishSettings;
use crate::transcript::Segment;
use crate::SuramakError;

const SYSTEM_PROMPT: &str = "You are a transcript cleanup engine. \
You receive JSON: {\"segments\":[{\"id\":int,\"text\":string}]}. \
Return ONLY JSON of the same shape with the same ids in the same order. \
Fix punctuation, capitalization, spacing, and obvious misrecognitions. \
Merge words broken mid-word. \
Do not add, remove, summarize, or translate content. \
Keep the original language.";

/// Wire type for the polish request/response payload.
#[derive(Debug, Serialize, Deserialize)]
struct PolishPayload {
    segments: Vec<PolishSegment>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PolishSegment {
    id: u32,
    text: String,
}

/// Trait for polish backends.
pub trait PolishBackend {
    fn polish(
        &self,
        segments: &[Segment],
        cfg: &PolishSettings,
    ) -> Result<Vec<Segment>, SuramakError>;
}

/// OpenAI-compatible blocking polish client.
pub struct OpenAiCompat {
    http: reqwest::blocking::Client,
}

impl OpenAiCompat {
    /// Creates a new client with a 120 s timeout.
    pub fn new() -> Result<Self, SuramakError> {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .use_rustls_tls()
            .build()?;
        Ok(Self { http })
    }
}

impl Default for OpenAiCompat {
    fn default() -> Self {
        Self::new().expect("failed to build reqwest client")
    }
}

impl PolishBackend for OpenAiCompat {
    fn polish(
        &self,
        segments: &[Segment],
        cfg: &PolishSettings,
    ) -> Result<Vec<Segment>, SuramakError> {
        let api_key = crate::keychain::get_api_key()?;

        let payload = PolishPayload {
            segments: segments
                .iter()
                .map(|s| PolishSegment {
                    id: s.idx,
                    text: s.text.clone(),
                })
                .collect(),
        };
        let user_content = serde_json::to_string(&payload)?;

        let body = serde_json::json!({
            "model": cfg.model,
            "temperature": 0,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user",   "content": user_content },
            ],
        });

        let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&api_key)
            .json(&body)
            .send()?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().unwrap_or_default();
            return Err(SuramakError::Polish(format!("HTTP {status}: {text}")));
        }

        let raw: serde_json::Value = resp.json()?;
        let content = raw["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| SuramakError::BadResponse("no content field".to_string()))?;

        parse_and_validate(content, segments)
    }
}

/// Parses the LLM response (tolerating ```json fences) and validates ids.
pub fn parse_and_validate(
    content: &str,
    original: &[Segment],
) -> Result<Vec<Segment>, SuramakError> {
    let json_str = strip_fences(content);
    let payload: PolishPayload = serde_json::from_str(json_str.trim()).map_err(|_| {
        SuramakError::BadResponse(format!(
            "not valid JSON: {}",
            &content[..content.len().min(200)]
        ))
    })?;

    // Validate: same ids, same order, non-empty texts.
    if payload.segments.len() != original.len() {
        return Err(SuramakError::BadResponse(format!(
            "segment count mismatch: expected {}, got {}",
            original.len(),
            payload.segments.len()
        )));
    }
    for (orig, polished) in original.iter().zip(&payload.segments) {
        if polished.id != orig.idx {
            return Err(SuramakError::BadResponse(format!(
                "id mismatch: expected {}, got {}",
                orig.idx, polished.id
            )));
        }
        if polished.text.trim().is_empty() {
            return Err(SuramakError::BadResponse(format!(
                "empty text for segment {}",
                polished.id
            )));
        }
    }

    let result = original
        .iter()
        .zip(payload.segments)
        .map(|(orig, polished)| Segment {
            idx: orig.idx,
            start_ms: orig.start_ms,
            end_ms: orig.end_ms,
            text: polished.text,
        })
        .collect();

    Ok(result)
}

/// Strips optional ```json ... ``` fences from LLM output.
fn strip_fences(s: &str) -> &str {
    let trimmed = s.trim();
    if let Some(inner) = trimmed.strip_prefix("```json") {
        if let Some(body) = inner.strip_suffix("```") {
            return body.trim();
        }
    }
    if let Some(inner) = trimmed.strip_prefix("```") {
        if let Some(body) = inner.strip_suffix("```") {
            return body.trim();
        }
    }
    trimmed
}
