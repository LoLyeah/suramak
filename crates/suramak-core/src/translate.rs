//! Opt-in bilingual export (M15.8): explicitly requested, per-segment
//! translation over the **same** OpenAI-compatible endpoint the polish flow
//! already uses.
//!
//! ## Consent and traffic
//!
//! Nothing here is ever called by opening a panel, loading a transcript or
//! exporting. The only entry point is [`run_translation`], reached from the
//! export surface's *Translate for export* action, and the caller resolves the
//! endpoint gate (base URL + API key) before a single byte is sent. A build
//! with no endpoint configured never constructs a backend at all — the
//! zero-request rule is asserted by tests (`tests/translate.rs` and the UI's
//! `bilingual` suite).
//!
//! ## Batching (the documented bound)
//!
//! A run is split into **bounded batches** by [`plan_batches`]: at most
//! [`MAX_BATCH_SEGMENTS`] segments and at most [`MAX_BATCH_CHARS`] characters
//! of source text per request. Segment boundaries are never cut: a single
//! segment longer than the character budget is sent alone (one oversized
//! request, not a torn cue). Empty segments are not sent at all — there is
//! nothing to translate, and inventing text for them would break cue parity.
//!
//! The bounds are deliberately small enough that one slow/failed request can
//! only cost a handful of segments: the failure is named per segment, the rest
//! of the run continues, and the failed set is what a retry re-sends.
//!
//! ## Failure shape
//!
//! A transport/HTTP failure fails every segment in that batch **by name** (the
//! same message on each row: the batch is the unit the endpoint saw). A
//! well-formed reply is validated per id: a segment missing from the reply, an
//! empty translation, or a duplicate id fails only that segment. Partial
//! results are returned inline; nothing is silently dropped.
//!
//! ## Cancellation
//!
//! Cancellation is cooperative and observed **between batches** (the same
//! stage-boundary rule as the transcription worker: an HTTP call already on the
//! wire is never preempted). Batches that already validated are kept and
//! returned with `cancelled = true`; the caller persists them. No request is
//! sent after a cancellation is observed.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::settings::PolishSettings;
use crate::transcript::Segment;
use crate::SuramakError;

/// The system prompt every translation request carries (public so the request
/// shape — including this text — is pinned by tests).
pub const TRANSLATE_SYSTEM_PROMPT: &str = "You are a subtitle translation engine. \
You receive JSON: {\"target_language\":string,\"segments\":[{\"id\":int,\"text\":string}]}. \
Translate every segment's text into the target language. Return ONLY JSON of the same \
shape with the same ids in the same order and no extra fields. \
Keep each translation a single subtitle line: no line breaks, no numbering, no notes. \
Do not merge, split, omit or reorder segments. \
Preserve names, numbers and punctuation intent. \
If a segment is already in the target language, return it unchanged.";

/// The largest number of segments one request may carry (M15.8).
///
/// Twenty short cues is a comfortable single request for a chat-completions
/// endpoint and keeps a failure's blast radius small; the UI's progress line
/// counts these batches.
pub const MAX_BATCH_SEGMENTS: usize = 20;

/// The largest number of source **characters** (not bytes) one request may
/// carry. 2 000 characters is roughly a minute of speech — a bounded prompt
/// that still gives the model enough context to keep terminology consistent
/// inside a batch.
pub const MAX_BATCH_CHARS: usize = 2_000;

/// The language a translation run produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetLanguage {
    English,
    Indonesian,
}

impl TargetLanguage {
    /// The primary language subtag (`en` / `id`) sent on the wire.
    pub fn code(self) -> &'static str {
        match self {
            TargetLanguage::English => "en",
            TargetLanguage::Indonesian => "id",
        }
    }

    /// The label the export surface shows.
    pub fn label(self) -> &'static str {
        match self {
            TargetLanguage::English => "English",
            TargetLanguage::Indonesian => "Indonesian",
        }
    }

    /// The pair's other side — what the transcript is expected to be in for
    /// this target to be the useful direction.
    pub fn source(self) -> &'static str {
        match self {
            TargetLanguage::English => "id",
            TargetLanguage::Indonesian => "en",
        }
    }

    /// The target the transcript's own language implies: an Indonesian
    /// transcript defaults to EN, anything else (including `auto`) to ID.
    pub fn for_source(language: Option<&str>) -> Self {
        let primary = language
            .unwrap_or_default()
            .trim()
            .split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if primary == "id" {
            TargetLanguage::English
        } else {
            TargetLanguage::Indonesian
        }
    }
}

/// What one translated segment carries besides its text: the provenance the
/// workorder asks for (model + timestamp + the source segment id set).
///
/// The record is stored per row, so a split duplicates it and a merge combines
/// it; `stale` is sticky until the user re-translates that row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationRecord {
    /// The translated text (the derived column).
    pub text: String,
    /// The model id the endpoint reported/configued for this run.
    pub model: String,
    /// RFC 3339 UTC timestamp of the run that produced the text.
    pub translated_at: String,
    /// The transcript segment ids the translation was produced from.
    pub source_ids: Vec<u32>,
    /// The source text changed after this translation was produced (M15.8:
    /// a visible badge, never a silent overwrite).
    pub stale: bool,
}

impl TranslationRecord {
    pub fn new(
        text: impl Into<String>,
        model: impl Into<String>,
        translated_at: impl Into<String>,
        source_ids: Vec<u32>,
    ) -> Self {
        Self {
            text: text.into(),
            model: model.into(),
            translated_at: translated_at.into(),
            source_ids,
            stale: false,
        }
    }

    /// Marks the translation stale (idempotent).
    pub fn mark_stale(&mut self) {
        self.stale = true;
    }

    /// The one-line provenance caption the editor shows (model, run timestamp
    /// and the source segment id set — the ids themselves when they fit).
    pub fn provenance_line(&self) -> String {
        let sources = match self.source_ids.len() {
            0 => "no source ids".to_string(),
            1..=6 => format!(
                "from segment{} {}",
                if self.source_ids.len() == 1 { "" } else { "s" },
                self.source_ids
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            count => format!("from {count} segments"),
        };
        format!(
            "{} · {} · {}",
            self.model,
            self.translated_at.replace('T', " ").trim_end_matches('Z'),
            sources
        )
    }
}

/// One successfully translated segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentTranslation {
    pub idx: u32,
    pub text: String,
}

/// One segment that could not be translated, with the reason that names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentFailure {
    pub idx: u32,
    pub message: String,
}

impl SegmentFailure {
    /// The row-level retry affordance text (`segment 4: HTTP 500 …`).
    pub fn label(&self) -> String {
        format!("segment {}: {}", self.idx, self.message)
    }
}

/// A batch's progress, reported before/after every request so the UI can name
/// where a long run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranslationProgress {
    /// 1-based index of the batch just sent (or about to be sent).
    pub batch: usize,
    /// How many batches the run planned.
    pub total: usize,
    /// Segments validated so far.
    pub translated: usize,
    /// Segments failed so far.
    pub failed: usize,
}

/// The outcome of one translation run — partial by design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationRun {
    pub translated: Vec<SegmentTranslation>,
    pub failed: Vec<SegmentFailure>,
    pub batches_total: usize,
    pub batches_done: usize,
    pub cancelled: bool,
    pub model: String,
    pub target: TargetLanguage,
    /// RFC 3339 UTC — one timestamp for the whole run (every record shares it).
    pub translated_at: String,
}

impl TranslationRun {
    pub fn failed_ids(&self) -> Vec<u32> {
        self.failed.iter().map(|failure| failure.idx).collect()
    }

    pub fn is_complete(&self) -> bool {
        !self.cancelled && self.failed.is_empty()
    }

    /// The status line: `translated 12 segments · 1 failed · stopped after 2/3
    /// batches` (only the parts that apply).
    pub fn summary(&self) -> String {
        let mut parts = vec![format!(
            "translated {} {}",
            self.translated.len(),
            if self.translated.len() == 1 {
                "segment"
            } else {
                "segments"
            }
        )];
        if !self.failed.is_empty() {
            parts.push(format!("{} failed", self.failed.len()));
        }
        if self.cancelled {
            parts.push(format!(
                "cancelled after {}/{} batches",
                self.batches_done, self.batches_total
            ));
        }
        parts.join(" · ")
    }

    /// The provenance record for one translated segment.
    pub fn record(
        &self,
        translation: &SegmentTranslation,
        source_ids: Vec<u32>,
    ) -> TranslationRecord {
        TranslationRecord::new(
            translation.text.clone(),
            self.model.clone(),
            self.translated_at.clone(),
            source_ids,
        )
    }
}

/// Splits a transcript's segments into the bounded request batches the run
/// sends (see the module docs).
///
/// Returns the segment ids per batch, in transcript order. Empty/whitespace
/// text is skipped. A segment longer than [`MAX_BATCH_CHARS`] is its own batch.
pub fn plan_batches(segments: &[Segment]) -> Vec<Vec<u32>> {
    let mut batches: Vec<Vec<u32>> = Vec::new();
    let mut current: Vec<u32> = Vec::new();
    let mut chars = 0usize;
    for segment in segments {
        let text = segment.text.trim();
        if text.is_empty() {
            continue;
        }
        let len = text.chars().count();
        let would_overflow = chars + len > MAX_BATCH_CHARS;
        if !current.is_empty() && (would_overflow || current.len() == MAX_BATCH_SEGMENTS) {
            batches.push(std::mem::take(&mut current));
            chars = 0;
        }
        current.push(segment.idx);
        chars += len;
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// The wire type of a translation request/response payload.
#[derive(Debug, Serialize, Deserialize)]
struct TranslatePayload {
    /// Present in the request; the reply only has to carry `segments`, so a
    /// missing value deserializes as empty (the request builder always sets it).
    #[serde(default)]
    target_language: String,
    segments: Vec<TranslateSegment>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TranslateSegment {
    id: u32,
    text: String,
}

/// The exact JSON body of one translation request.
///
/// Same OpenAI-compatible envelope as the polish request (`temperature: 0`,
/// a system + user message); the user message is the JSON payload with the
/// target language and the batch's segments.
pub fn build_body(
    segments: &[Segment],
    cfg: &PolishSettings,
    target: TargetLanguage,
) -> Result<serde_json::Value, SuramakError> {
    let payload = TranslatePayload {
        target_language: target.code().to_string(),
        segments: segments
            .iter()
            .map(|segment| TranslateSegment {
                id: segment.idx,
                text: segment.text.clone(),
            })
            .collect(),
    };
    let user_content = serde_json::to_string(&payload)?;
    Ok(serde_json::json!({
        "model": cfg.model,
        "temperature": 0,
        "messages": [
            { "role": "system", "content": TRANSLATE_SYSTEM_PROMPT },
            { "role": "user", "content": user_content },
        ],
    }))
}

/// A transport for one batch: sends the request and returns the assistant
/// message's raw content. Implementations own their HTTP client; tests
/// substitute fixtures (never a real endpoint).
pub trait BatchSender {
    fn send(
        &self,
        segments: &[Segment],
        cfg: &PolishSettings,
        target: TargetLanguage,
    ) -> Result<String, String>;
}

/// Sends one batch over HTTP. Owns a blocking client + the API key resolved
/// once at construction (never read per batch).
pub struct OpenAiCompatTranslate {
    http: reqwest::blocking::Client,
    api_key: String,
}

impl OpenAiCompatTranslate {
    /// Builds the client and resolves the API key (keychain on macOS/Windows,
    /// `SURAMAK_OPENAI_API_KEY` on Linux). Fails before any request when no key
    /// is available.
    pub fn new() -> Result<Self, SuramakError> {
        let api_key = crate::keychain::get_api_key()?;
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .use_rustls_tls()
            .build()?;
        Ok(Self { http, api_key })
    }

    fn completions_url(cfg: &PolishSettings) -> String {
        format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'))
    }
}

impl BatchSender for OpenAiCompatTranslate {
    fn send(
        &self,
        segments: &[Segment],
        cfg: &PolishSettings,
        target: TargetLanguage,
    ) -> Result<String, String> {
        let body = build_body(segments, cfg, target).map_err(|err| err.to_string())?;
        let response = self
            .http
            .post(Self::completions_url(cfg))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .map_err(|err| format!("the endpoint could not be reached: {err}"))?;
        let status = response.status().as_u16();
        let text = response.text().unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(format!("HTTP {status}: {}", excerpt(&text)));
        }
        content_of(&text)
    }
}

/// Extracts the assistant message's content from a chat-completions reply.
fn content_of(body: &str) -> Result<String, String> {
    let raw: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "the reply was not JSON".to_string())?;
    raw["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "the reply carried no assistant content".to_string())
}

/// A short, UTF-8-safe excerpt of an endpoint's error body.
fn excerpt(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.len() <= 200 {
        return trimmed.to_string();
    }
    let mut cut = 200;
    while cut > 0 && !trimmed.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &trimmed[..cut])
}

/// Parses one reply and validates it **per requested id**.
///
/// Returns the validated translations plus one named failure per requested
/// segment that the reply did not validly cover. A completely unparseable
/// reply fails every requested segment with the parse reason (never a panic,
/// never a silent empty batch).
pub fn parse_reply(
    content: &str,
    requested: &[Segment],
) -> (Vec<SegmentTranslation>, Vec<SegmentFailure>) {
    let text = strip_fences(content).trim();
    let payload: TranslatePayload = match serde_json::from_str(text) {
        Ok(payload) => payload,
        Err(_) => {
            let reason = format!(
                "the reply was not valid JSON: {}",
                excerpt(text).replace('\n', " ")
            );
            return (
                Vec::new(),
                requested
                    .iter()
                    .map(|segment| SegmentFailure {
                        idx: segment.idx,
                        message: reason.clone(),
                    })
                    .collect(),
            );
        }
    };

    let mut by_id: std::collections::HashMap<u32, (String, usize)> =
        std::collections::HashMap::new();
    for segment in &payload.segments {
        let entry = by_id
            .entry(segment.id)
            .or_insert_with(|| (segment.text.clone(), 0));
        entry.1 += 1;
    }

    let mut translated = Vec::new();
    let mut failed = Vec::new();
    for segment in requested {
        match by_id.get(&segment.idx) {
            None => failed.push(SegmentFailure {
                idx: segment.idx,
                message: "missing from the endpoint's reply".to_string(),
            }),
            Some((_, count)) if *count > 1 => failed.push(SegmentFailure {
                idx: segment.idx,
                message: "the endpoint returned this id more than once".to_string(),
            }),
            Some((text, _)) if text.trim().is_empty() => failed.push(SegmentFailure {
                idx: segment.idx,
                message: "the endpoint returned an empty translation".to_string(),
            }),
            Some((text, _)) => translated.push(SegmentTranslation {
                idx: segment.idx,
                text: text.trim().to_string(),
            }),
        }
    }
    (translated, failed)
}

/// Runs the whole translation over `segments` in bounded batches.
///
/// `sender` is the only thing that can touch the network; `cancelled` is
/// polled before every batch and immediately after every reply (the
/// stage-boundary seam). `progress` is called once per batch, before the send,
/// so a UI can name the batch in flight. The timestamp is taken once, so every
/// record from one run shares it.
pub fn run_translation<S, C, P>(
    segments: &[Segment],
    cfg: &PolishSettings,
    target: TargetLanguage,
    sender: &S,
    mut cancelled: C,
    mut progress: P,
) -> TranslationRun
where
    S: BatchSender + ?Sized,
    C: FnMut() -> bool,
    P: FnMut(TranslationProgress),
{
    let batches = plan_batches(segments);
    let total = batches.len();
    let by_idx: std::collections::HashMap<u32, &Segment> = segments
        .iter()
        .map(|segment| (segment.idx, segment))
        .collect();
    let translated_at = chrono::Utc::now().to_rfc3339();
    let mut translated = Vec::new();
    let mut failed = Vec::new();
    let mut done = 0usize;
    let mut was_cancelled = false;

    for (index, batch) in batches.iter().enumerate() {
        if cancelled() {
            was_cancelled = true;
            break;
        }
        progress(TranslationProgress {
            batch: index + 1,
            total,
            translated: translated.len(),
            failed: failed.len(),
        });
        let batch_segments: Vec<Segment> = batch
            .iter()
            .filter_map(|idx| by_idx.get(idx).map(|segment| (*segment).clone()))
            .collect();
        match sender.send(&batch_segments, cfg, target) {
            Ok(content) => {
                let (ok, bad) = parse_reply(&content, &batch_segments);
                translated.extend(ok);
                failed.extend(bad);
            }
            Err(message) => {
                for segment in &batch_segments {
                    failed.push(SegmentFailure {
                        idx: segment.idx,
                        message: message.clone(),
                    });
                }
            }
        }
        done += 1;
        // The post-reply check is what makes a cancel land before the next
        // request instead of after the whole run.
        if cancelled() {
            was_cancelled = true;
            break;
        }
    }

    translated.sort_by_key(|translation| translation.idx);
    failed.sort_by_key(|failure| failure.idx);
    TranslationRun {
        translated,
        failed,
        batches_total: total,
        batches_done: done,
        cancelled: was_cancelled,
        model: cfg.model.clone(),
        target,
        translated_at,
    }
}

/// Strips optional ```json … ``` fences from model output.
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

impl fmt::Display for TargetLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(idx: u32, text: &str) -> Segment {
        Segment {
            idx,
            start_ms: i64::from(idx) * 1_000,
            end_ms: i64::from(idx) * 1_000 + 900,
            text: text.to_string(),
        }
    }

    fn cfg() -> PolishSettings {
        PolishSettings {
            base_url: "http://localhost:9/v1".to_string(),
            model: "mock-model".to_string(),
            send_audio: false,
        }
    }

    /// The fixture endpoint: a deterministic dictionary per direction. Any
    /// text it does not know is echoed with a marker, so a test can always tell
    /// exactly which request produced which cue.
    struct FixtureSender {
        pairs: Vec<(&'static str, &'static str)>,
        /// Fail the batch containing this id (once), to exercise partial runs.
        fail_once: std::sync::Mutex<Option<u32>>,
    }

    impl FixtureSender {
        fn en_id() -> Self {
            Self {
                pairs: vec![
                    ("Hello world.", "Halo dunia."),
                    ("This is a test.", "Ini adalah ujian."),
                    ("Goodbye.", "Selamat tinggal."),
                    ("Thank you.", "Terima kasih."),
                ],
                fail_once: std::sync::Mutex::new(None),
            }
        }

        fn id_en() -> Self {
            Self {
                pairs: vec![
                    ("Halo dunia.", "Hello world."),
                    ("Ini adalah ujian.", "This is a test."),
                    ("Selamat tinggal.", "Goodbye."),
                    ("Terima kasih.", "Thank you."),
                ],
                fail_once: std::sync::Mutex::new(None),
            }
        }
    }

    impl BatchSender for FixtureSender {
        fn send(
            &self,
            segments: &[Segment],
            _cfg: &PolishSettings,
            _target: TargetLanguage,
        ) -> Result<String, String> {
            let mut fail_once = self.fail_once.lock().unwrap();
            if let Some(id) = *fail_once {
                if segments.iter().any(|segment| segment.idx == id) {
                    *fail_once = None;
                    return Err(format!("HTTP 500: fixture refused segment {id}"));
                }
            }
            let reply: Vec<serde_json::Value> = segments
                .iter()
                .map(|segment| {
                    let text = self
                        .pairs
                        .iter()
                        .find(|(source, _)| *source == segment.text.as_str())
                        .map(|(_, target)| (*target).to_string())
                        .unwrap_or_else(|| format!("[unmapped:{}]", segment.idx));
                    serde_json::json!({ "id": segment.idx, "text": text })
                })
                .collect();
            Ok(serde_json::json!({ "segments": reply }).to_string())
        }
    }

    // ---- batching -----------------------------------------------------------

    #[test]
    fn batches_are_bounded_by_count_and_characters() {
        let many: Vec<Segment> = (0..45).map(|idx| segment(idx, "short line")).collect();
        let batches = plan_batches(&many);
        assert_eq!(
            batches.len(),
            3,
            "45 segments at 20 per request is three batches"
        );
        assert_eq!(batches[0].len(), MAX_BATCH_SEGMENTS);
        assert_eq!(batches[1].len(), MAX_BATCH_SEGMENTS);
        assert_eq!(batches[2].len(), 5);
        assert_eq!(batches[0][0], 0);
        assert_eq!(batches[2][4], 44);

        // 500 chars per segment: four fit in 2 000, the fifth starts a batch.
        let long: Vec<Segment> = (0..6).map(|idx| segment(idx, &"a".repeat(500))).collect();
        let batches = plan_batches(&long);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), 4);
        assert_eq!(batches[1].len(), 2);
    }

    #[test]
    fn an_oversized_segment_is_never_cut_and_empty_segments_are_skipped() {
        let oversized = "b".repeat(MAX_BATCH_CHARS + 50);
        let segments = vec![
            segment(0, "  "),
            segment(1, &oversized),
            segment(2, ""),
            segment(3, "after"),
        ];
        let batches = plan_batches(&segments);
        assert_eq!(batches, vec![vec![1], vec![3]]);
        assert_eq!(
            batches.iter().flatten().copied().collect::<Vec<_>>(),
            vec![1, 3],
            "every non-empty segment is sent exactly once, in order"
        );
    }

    #[test]
    fn body_carries_the_target_language_and_the_exact_ids() {
        let segments = vec![segment(3, "Halo dunia."), segment(4, "Terima kasih.")];
        let body = build_body(&segments, &cfg(), TargetLanguage::English).unwrap();
        assert_eq!(body["model"], "mock-model");
        assert_eq!(body["temperature"], 0);
        assert_eq!(body["messages"][0]["content"], TRANSLATE_SYSTEM_PROMPT);
        let payload: serde_json::Value =
            serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(payload["target_language"], "en");
        assert_eq!(payload["segments"][0]["id"], 3);
        assert_eq!(payload["segments"][1]["text"], "Terima kasih.");

        let body = build_body(&segments, &cfg(), TargetLanguage::Indonesian).unwrap();
        let payload: serde_json::Value =
            serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(payload["target_language"], "id");
    }

    #[test]
    fn the_target_defaults_from_the_transcripts_own_language() {
        assert_eq!(
            TargetLanguage::for_source(Some("id")),
            TargetLanguage::English
        );
        assert_eq!(
            TargetLanguage::for_source(Some("id-ID")),
            TargetLanguage::English
        );
        assert_eq!(
            TargetLanguage::for_source(Some("en")),
            TargetLanguage::Indonesian
        );
        assert_eq!(TargetLanguage::for_source(None), TargetLanguage::Indonesian);
    }

    // ---- reply validation ---------------------------------------------------

    #[test]
    fn parse_reply_names_each_bad_segment_and_keeps_the_good_ones() {
        let requested = vec![segment(0, "one"), segment(1, "two"), segment(2, "three")];
        let content = r#"{"segments":[{"id":0,"text":"satu"},{"id":2,"text":"  "}]}"#;
        let (ok, failed) = parse_reply(content, &requested);
        assert_eq!(
            ok,
            vec![SegmentTranslation {
                idx: 0,
                text: "satu".to_string()
            }]
        );
        assert_eq!(failed.len(), 2);
        assert_eq!(failed[0].idx, 1);
        assert!(failed[0].message.contains("missing"));
        assert_eq!(failed[1].idx, 2);
        assert!(failed[1].message.contains("empty"));
    }

    #[test]
    fn parse_reply_fails_the_batch_on_garbage_or_duplicate_ids() {
        let requested = vec![segment(0, "one"), segment(1, "two")];
        let (ok, failed) = parse_reply("not json at all", &requested);
        assert!(ok.is_empty());
        assert_eq!(failed.len(), 2);
        assert!(failed[0].message.contains("not valid JSON"));

        let duplicated =
            r#"{"segments":[{"id":0,"text":"a"},{"id":0,"text":"b"},{"id":1,"text":"c"}]}"#;
        let (ok, failed) = parse_reply(duplicated, &requested);
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].idx, 1);
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].idx, 0);
        assert!(failed[0].message.contains("more than once"));
    }

    #[test]
    fn fenced_replies_are_accepted() {
        let requested = vec![segment(0, "Hello world.")];
        let (ok, failed) = parse_reply(
            "```json\n{\"segments\":[{\"id\":0,\"text\":\"Halo dunia.\"}]}\n```",
            &requested,
        );
        assert!(failed.is_empty());
        assert_eq!(ok[0].text, "Halo dunia.");
    }

    // ---- whole runs (the EN→ID / ID→EN fixtures) ----------------------------

    #[test]
    fn en_to_id_run_translates_every_cue_in_order() {
        let segments = vec![
            segment(0, "Hello world."),
            segment(1, "This is a test."),
            segment(2, "Goodbye."),
        ];
        let run = run_translation(
            &segments,
            &cfg(),
            TargetLanguage::Indonesian,
            &FixtureSender::en_id(),
            || false,
            |_| {},
        );
        assert!(run.is_complete(), "{:?}", run.failed);
        assert_eq!(run.translated.len(), 3);
        let texts: Vec<&str> = run
            .translated
            .iter()
            .map(|translation| translation.text.as_str())
            .collect();
        assert_eq!(
            texts,
            vec!["Halo dunia.", "Ini adalah ujian.", "Selamat tinggal."]
        );
        assert_eq!(run.model, "mock-model");
        assert_eq!(run.target, TargetLanguage::Indonesian);
        assert_eq!(run.batches_total, 1);
    }

    #[test]
    fn id_to_en_run_is_the_mirror_direction() {
        let segments = vec![
            segment(0, "Halo dunia."),
            segment(1, "Ini adalah ujian."),
            segment(2, "Terima kasih."),
        ];
        let run = run_translation(
            &segments,
            &cfg(),
            TargetLanguage::English,
            &FixtureSender::id_en(),
            || false,
            |_| {},
        );
        assert!(run.is_complete());
        let texts: Vec<&str> = run
            .translated
            .iter()
            .map(|translation| translation.text.as_str())
            .collect();
        assert_eq!(texts, vec!["Hello world.", "This is a test.", "Thank you."]);
    }

    #[test]
    fn a_failed_batch_names_its_segments_and_the_rest_still_lands() {
        // 21 segments → two batches; the fixture refuses the first batch once.
        let segments: Vec<Segment> = (0..21).map(|idx| segment(idx, "Hello world.")).collect();
        let sender = FixtureSender::en_id();
        *sender.fail_once.lock().unwrap() = Some(3);
        let run = run_translation(
            &segments,
            &cfg(),
            TargetLanguage::Indonesian,
            &sender,
            || false,
            |_| {},
        );
        assert!(!run.cancelled);
        assert_eq!(run.batches_total, 2);
        assert_eq!(run.batches_done, 2);
        assert_eq!(run.failed.len(), MAX_BATCH_SEGMENTS);
        assert_eq!(run.failed[0].idx, 0);
        assert!(run.failed[0].message.contains("HTTP 500"));
        assert_eq!(run.translated.len(), 1);
        assert_eq!(run.translated[0].idx, 20, "the second batch still lands");

        // Retry only the failed ids: the same sender now succeeds.
        let retry: Vec<Segment> = segments
            .iter()
            .filter(|segment| run.failed_ids().contains(&segment.idx))
            .cloned()
            .collect();
        let retried = run_translation(
            &retry,
            &cfg(),
            TargetLanguage::Indonesian,
            &sender,
            || false,
            |_| {},
        );
        assert!(retried.is_complete());
        assert_eq!(retried.translated.len(), MAX_BATCH_SEGMENTS);
    }

    #[test]
    fn cancellation_between_batches_keeps_what_validated_and_sends_no_more() {
        let segments: Vec<Segment> = (0..45).map(|idx| segment(idx, "Hello world.")).collect();
        let seen = std::sync::Mutex::new(Vec::new());
        struct Counting<'a> {
            inner: &'a FixtureSender,
            seen: &'a std::sync::Mutex<Vec<usize>>,
        }
        impl BatchSender for Counting<'_> {
            fn send(
                &self,
                segments: &[Segment],
                cfg: &PolishSettings,
                target: TargetLanguage,
            ) -> Result<String, String> {
                self.seen.lock().unwrap().push(segments.len());
                self.inner.send(segments, cfg, target)
            }
        }
        let sender = FixtureSender::en_id();
        let counting = Counting {
            inner: &sender,
            seen: &seen,
        };
        let mut polls = 0;
        let run = run_translation(
            &segments,
            &cfg(),
            TargetLanguage::Indonesian,
            &counting,
            || {
                polls += 1;
                // Fresh polls: pre-batch (1), post-batch (2), pre-batch 2 (3)
                // → cancel lands before the second request.
                polls >= 3
            },
            |_| {},
        );
        assert!(run.cancelled);
        assert_eq!(run.batches_done, 1);
        assert_eq!(run.translated.len(), MAX_BATCH_SEGMENTS);
        assert_eq!(*seen.lock().unwrap(), vec![MAX_BATCH_SEGMENTS]);
        assert!(run.summary().contains("cancelled after 1/3"));
    }

    #[test]
    fn a_cancel_before_the_first_batch_sends_nothing() {
        let segments = vec![segment(0, "Hello world.")];
        let seen = std::sync::Mutex::new(0usize);
        struct Never;
        impl BatchSender for Never {
            fn send(
                &self,
                _: &[Segment],
                _: &PolishSettings,
                _: TargetLanguage,
            ) -> Result<String, String> {
                panic!("no request may be sent after cancellation");
            }
        }
        let run = run_translation(
            &segments,
            &cfg(),
            TargetLanguage::Indonesian,
            &Never,
            || {
                *seen.lock().unwrap() += 1;
                true
            },
            |_| {},
        );
        assert!(run.cancelled);
        assert_eq!(run.batches_done, 0);
        assert!(run.translated.is_empty() && run.failed.is_empty());
        assert_eq!(*seen.lock().unwrap(), 1, "the pre-batch check ran once");
    }

    #[test]
    fn progress_names_every_batch_before_it_is_sent() {
        let segments: Vec<Segment> = (0..25).map(|idx| segment(idx, "Hello world.")).collect();
        let seen = std::sync::Mutex::new(Vec::new());
        let run = run_translation(
            &segments,
            &cfg(),
            TargetLanguage::Indonesian,
            &FixtureSender::en_id(),
            || false,
            |progress| seen.lock().unwrap().push((progress.batch, progress.total)),
        );
        assert_eq!(*seen.lock().unwrap(), vec![(1, 2), (2, 2)]);
        assert_eq!(run.translated.len(), 25);
    }

    #[test]
    fn provenance_round_trips_through_the_record() {
        let run = run_translation(
            &[segment(0, "Hello world.")],
            &cfg(),
            TargetLanguage::Indonesian,
            &FixtureSender::en_id(),
            || false,
            |_| {},
        );
        let record = run.record(&run.translated[0], vec![0]);
        assert_eq!(record.text, "Halo dunia.");
        assert_eq!(record.model, "mock-model");
        assert!(!record.stale);
        assert!(record.provenance_line().contains("mock-model"));
        assert!(record.provenance_line().contains("from segment 0"));
        assert!(!record.translated_at.is_empty());
    }
}
