//! The polish client (M4) and its M11 audio-aware extension.
//!
//! Text-only polish is one OpenAI-compatible `/chat/completions` request whose
//! user message carries a JSON payload of the transcript's segments (the shape
//! `build_body(…, None)` pins — byte-identical to the pre-M11 request).
//!
//! M11 adds the recording as a **second data point**: with the opt-in toggle
//! on, the recording is sent as OpenAI-compatible `input_audio` content parts
//! (`{"type":"input_audio","input_audio":{"data":<base64>,"format":"wav"}}`),
//! chunked at segment boundaries into ≤ [`AUDIO_CHUNK_CAP_BYTES`] raw PCM16
//! chunks — one request per chunk, the corrected outputs concatenated in
//! order. Nothing about the endpoint is assumed: the ladder degrades, never
//! fails hard — a missing/unreadable recording is text-only with a note, and
//! an endpoint that rejects the audio (4xx) gets one automatic text-only retry
//! with a note naming the reason.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::settings::PolishSettings;
use crate::transcript::Segment;
use crate::SuramakError;

/// The system prompt every polish request carries (public so the request
/// shape — including this text — can be pinned by tests).
pub const SYSTEM_PROMPT: &str = "You are a transcript cleanup engine. \
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

/// M11: the raw-PCM cap for one audio chunk — 4 MB of PCM16 ≈ 2 minutes of
/// 16 kHz mono audio (32 KB/s). A chunk is never larger; a segment boundary is
/// the preferred cut, the cap only forces a cut when one segment alone exceeds
/// it.
pub const AUDIO_CHUNK_CAP_BYTES: usize = 4 * 1024 * 1024;

/// M11: what a polish run should do about the recording (the ladder's input).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioRequest {
    /// The toggle is off — exactly today's text-only request, no notes.
    Off,
    /// The toggle is on but no recording is available (keep-audio off, the
    /// file is gone, no audio attached): text-only + `note`, which the UI
    /// surfaces with the review like every polish message.
    Unavailable { note: String },
    /// The toggle is on and the recording lives at this path. It is read and
    /// prepared for the wire (a 16 kHz mono PCM16 WAV goes as-is; anything
    /// else — an imported file, another WAV shape — is decoded + resampled
    /// through the pipeline's own decode path); every failure degrades to
    /// text-only + a note.
    Recording(PathBuf),
}

/// M11: the outcome of a polish run — the corrected segments plus any
/// user-facing notes about graceful degradation (empty on a plain success).
#[derive(Debug, Clone, PartialEq)]
pub struct PolishOutcome {
    pub segments: Vec<Segment>,
    pub notes: Vec<String>,
}

impl PolishOutcome {
    /// A run with nothing to report.
    pub fn plain(segments: Vec<Segment>) -> Self {
        Self {
            segments,
            notes: Vec::new(),
        }
    }
}

/// A parsed 16-bit PCM WAV payload (M11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm16Wav {
    pub sample_rate: u32,
    pub channels: u16,
    /// The raw PCM16 little-endian bytes (interleaved for >1 channel).
    pub data: Vec<u8>,
}

impl Pcm16Wav {
    /// Parses a WAV's PCM16 payload. `Err` carries the human-readable reason
    /// (it becomes the fallback note's parenthetical).
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let reader = hound::WavReader::new(std::io::Cursor::new(bytes))
            .map_err(|err| format!("not a readable WAV file ({err})"))?;
        let spec = reader.spec();
        if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
            return Err(format!(
                "not 16-bit PCM ({} bits, {:?})",
                spec.bits_per_sample, spec.sample_format
            ));
        }
        let mut data = Vec::new();
        for sample in reader.into_samples::<i16>() {
            let sample = sample.map_err(|err| format!("unreadable PCM data ({err})"))?;
            data.extend_from_slice(&sample.to_le_bytes());
        }
        Ok(Self {
            sample_rate: spec.sample_rate,
            channels: spec.channels,
            data,
        })
    }

    /// Encodes this payload as a complete WAV file (canonical hound header +
    /// the raw PCM bytes).
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let spec = hound::WavSpec {
            channels: self.channels,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec).map_err(|e| e.to_string())?;
            for pair in self.data.chunks_exact(2) {
                let sample = i16::from_le_bytes([pair[0], pair[1]]);
                writer.write_sample(sample).map_err(|e| e.to_string())?;
            }
            writer.finalize().map_err(|e| e.to_string())?;
        }
        Ok(cursor.into_inner())
    }

    /// 16 kHz mono PCM16 from decoded f32 samples — the exact scaling
    /// [`crate::decode::write_wav_i16`] uses (clamp + round to nearest), so an
    /// imported file goes on the wire the same way a recording would.
    pub fn from_f32_16k_mono(samples: &[f32]) -> Self {
        let mut data = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            let scaled = (s * f32::from(i16::MAX)).round();
            let value = scaled.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
            data.extend_from_slice(&value.to_le_bytes());
        }
        Self {
            sample_rate: 16_000,
            channels: 1,
            data,
        }
    }

    /// Whether the payload is already the wire shape the recording pipeline
    /// writes (16 kHz mono PCM16 — "no encoder needed").
    pub fn is_16k_mono(&self) -> bool {
        self.sample_rate == 16_000 && self.channels == 1
    }
}

/// One chunk of the recording (M11): a complete WAV file — header + ≤ cap raw
/// PCM16 bytes — covering `[start_sample, end_sample)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioChunk {
    /// First sample of the chunk (mono frames; the recordings are mono, so
    /// samples and frames coincide).
    pub start_sample: usize,
    /// One past the chunk's last sample.
    pub end_sample: usize,
    /// A complete WAV file carrying this chunk's raw PCM16 bytes.
    pub wav: Vec<u8>,
}

/// Converts a millisecond timestamp to a sample offset at `rate` (M11 —
/// whisper's segment times are milliseconds; the recording is PCM16).
pub fn sample_of_ms(ms: i64, rate: u32) -> usize {
    let ms = ms.max(0) as u128;
    let samples = ms * u128::from(rate) / 1000;
    usize::try_from(samples).unwrap_or(usize::MAX)
}

/// Splits `wav` at segment boundaries into chunks of at most `cap` raw PCM16
/// bytes each (M11).
///
/// Cut points are the segments' start times, converted to frame-aligned sample
/// offsets; a chunk ends at the latest cut that keeps it within the cap, so a
/// segment is **never split mid-segment when boundaries allow** it. Only a
/// single segment longer than the cap is split at the cap (documented — the
/// one forced mid-segment cut). Every sample of the recording appears in
/// exactly one chunk, in order; the returned chunks tile `[0, frames)`.
pub fn chunk_wav(wav: &Pcm16Wav, segments: &[Segment], cap: usize) -> Vec<AudioChunk> {
    let frame_bytes = usize::from(wav.channels) * 2;
    if frame_bytes == 0 {
        return Vec::new();
    }
    let frames = wav.data.len() / frame_bytes;
    if frames == 0 {
        return Vec::new();
    }
    // The cap in frames, at least one frame (a cap below one frame still makes
    // progress; only a pathological caller hits this).
    let frames_per_chunk = (cap / frame_bytes).max(1);

    // Candidate cuts: frame-aligned segment starts strictly inside the audio.
    let mut cuts: Vec<usize> = segments
        .iter()
        .map(|segment| sample_of_ms(segment.start_ms, wav.sample_rate))
        .filter(|&frame| frame > 0 && frame < frames)
        .collect();
    cuts.sort_unstable();
    cuts.dedup();

    let slice_wav = |start: usize, end: usize| -> Vec<u8> {
        Pcm16Wav {
            sample_rate: wav.sample_rate,
            channels: wav.channels,
            data: wav.data[start * frame_bytes..end * frame_bytes].to_vec(),
        }
        .to_bytes()
        .expect("encoding an in-memory PCM16 payload cannot fail")
    };

    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < frames {
        if frames - start <= frames_per_chunk {
            chunks.push(AudioChunk {
                start_sample: start,
                end_sample: frames,
                wav: slice_wav(start, frames),
            });
            break;
        }
        let limit = start + frames_per_chunk;
        let end = cuts
            .iter()
            .rev()
            .find(|&&cut| cut > start && cut <= limit)
            .copied()
            .unwrap_or(limit);
        chunks.push(AudioChunk {
            start_sample: start,
            end_sample: end,
            wav: slice_wav(start, end),
        });
        start = end;
    }
    chunks
}

/// Trait for polish backends.
pub trait PolishBackend {
    fn polish(
        &self,
        segments: &[Segment],
        cfg: &PolishSettings,
    ) -> Result<Vec<Segment>, SuramakError>;

    /// M11: the audio-aware entry point. The default implementation has no
    /// audio support — it degrades to text-only and notes it for a recording
    /// (never a hard failure).
    fn polish_with_audio(
        &self,
        segments: &[Segment],
        cfg: &PolishSettings,
        audio: AudioRequest,
    ) -> Result<PolishOutcome, SuramakError> {
        let notes = match audio {
            AudioRequest::Off => Vec::new(),
            AudioRequest::Unavailable { note } => vec![note],
            AudioRequest::Recording(_) => vec![AUDIO_UNSUPPORTED_NOTE.to_string()],
        };
        Ok(PolishOutcome {
            segments: self.polish(segments, cfg)?,
            notes,
        })
    }
}

/// The note a backend without audio support shows for a recording (M11).
pub const AUDIO_UNSUPPORTED_NOTE: &str =
    "the recording was not sent (this backend has no audio support) — the text was polished on its own";

/// OpenAI-compatible blocking polish client.
pub struct OpenAiCompat {
    http: reqwest::blocking::Client,
}

/// A response's status + body (M11: the audio ladder needs the status, not
/// only the formatted error).
struct HttpReply {
    status: u16,
    body: String,
}

impl HttpReply {
    fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The raw error string the polish flow has always surfaced verbatim.
    fn error_message(&self) -> String {
        format!("HTTP {}: {}", self.status, self.body)
    }

    /// Whether the endpoint rejected the request itself (4xx) — the signal
    /// that the audio is the problem (unsupported content part, payload too
    /// large, …) and the text-only retry is worth one shot.
    fn rejected(&self) -> bool {
        (400..500).contains(&self.status)
    }

    /// The note shown after a 4xx-triggered text-only retry, naming the
    /// reason (the status + a short excerpt of the endpoint's message).
    fn audio_rejected_note(&self) -> String {
        let mut excerpt = self.body.trim();
        if excerpt.len() > 200 {
            // Never cut inside a UTF-8 character.
            let mut cut = 200;
            while cut > 0 && !excerpt.is_char_boundary(cut) {
                cut -= 1;
            }
            excerpt = &excerpt[..cut];
        }
        if excerpt.is_empty() {
            format!(
                "the endpoint rejected the recording (HTTP {}) — retried without it",
                self.status
            )
        } else {
            format!(
                "the endpoint rejected the recording (HTTP {}: {}) — retried without it",
                self.status, excerpt
            )
        }
    }
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

    fn completions_url(cfg: &PolishSettings) -> String {
        format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'))
    }

    fn post(
        &self,
        url: &str,
        api_key: &str,
        body: &serde_json::Value,
    ) -> Result<HttpReply, SuramakError> {
        let resp = self.http.post(url).bearer_auth(api_key).json(body).send()?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        Ok(HttpReply { status, body })
    }

    /// One text-only request — today's exact body — with response validation.
    fn text_only(
        &self,
        url: &str,
        api_key: &str,
        segments: &[Segment],
        cfg: &PolishSettings,
    ) -> Result<Vec<Segment>, SuramakError> {
        let body = build_body(segments, cfg, None)?;
        let reply = self.post(url, api_key, &body)?;
        if !reply.is_success() {
            return Err(SuramakError::Polish(reply.error_message()));
        }
        let content = content_of(&reply.body)?;
        parse_and_validate(&content, segments)
    }

    /// The M11 audio ladder: prepare → chunk → one request per chunk (the
    /// corrected outputs concatenated in order), with text-only fallbacks for
    /// every way the recording can let us down.
    fn audio_aware(
        &self,
        url: &str,
        api_key: &str,
        segments: &[Segment],
        cfg: &PolishSettings,
        path: &Path,
    ) -> Result<PolishOutcome, SuramakError> {
        let wav = match prepare_recording(path) {
            Ok(wav) => wav,
            Err(reason) => {
                let polished = self.text_only(url, api_key, segments, cfg)?;
                return Ok(PolishOutcome {
                    segments: polished,
                    notes: vec![format!(
                        "the recording was not sent ({reason}) — the text was polished on its own"
                    )],
                });
            }
        };
        let chunks = chunk_wav(&wav, segments, AUDIO_CHUNK_CAP_BYTES);
        if chunks.is_empty() {
            let polished = self.text_only(url, api_key, segments, cfg)?;
            return Ok(PolishOutcome {
                segments: polished,
                notes: vec![
                    "the recording was not sent (it holds no audio) — the text was polished on its own"
                        .to_string(),
                ],
            });
        }

        let last = chunks.len() - 1;
        let mut polished: Vec<Segment> = Vec::with_capacity(segments.len());
        for (index, chunk) in chunks.iter().enumerate() {
            // A segment belongs to the chunk its start falls in; the last
            // chunk also catches any start at or past the audio's end, so
            // every segment is sent exactly once.
            let subset: Vec<Segment> = segments
                .iter()
                .filter(|segment| {
                    let frame = sample_of_ms(segment.start_ms, wav.sample_rate);
                    if index == last {
                        frame >= chunk.start_sample
                    } else {
                        frame >= chunk.start_sample && frame < chunk.end_sample
                    }
                })
                .cloned()
                .collect();
            if subset.is_empty() {
                // Only possible when one segment alone exceeds the cap (the
                // forced mid-segment split): this chunk's audio continues a
                // segment already sent. Its audio carries no new text — skip
                // the request rather than send an empty payload.
                continue;
            }
            let body = build_body(&subset, cfg, Some(chunk))?;
            let reply = self.post(url, api_key, &body)?;
            if !reply.is_success() {
                if reply.rejected() {
                    // The endpoint refused the audio — one text-only retry for
                    // the whole transcript, with a note naming the reason.
                    let retried = self.text_only(url, api_key, segments, cfg)?;
                    return Ok(PolishOutcome {
                        segments: retried,
                        notes: vec![reply.audio_rejected_note()],
                    });
                }
                return Err(SuramakError::Polish(reply.error_message()));
            }
            let content = content_of(&reply.body)?;
            polished.extend(parse_and_validate(&content, &subset)?);
        }

        // By construction the concatenation covers every segment once, in
        // order — re-check it so a stray bug can never pair texts wrongly.
        let aligned = polished.len() == segments.len()
            && polished
                .iter()
                .zip(segments.iter())
                .all(|(out, original)| out.idx == original.idx);
        if !aligned {
            return Err(SuramakError::BadResponse(
                "the per-chunk replies do not line up with the transcript".to_string(),
            ));
        }
        Ok(PolishOutcome::plain(polished))
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
        let url = Self::completions_url(cfg);
        self.text_only(&url, &api_key, segments, cfg)
    }

    fn polish_with_audio(
        &self,
        segments: &[Segment],
        cfg: &PolishSettings,
        audio: AudioRequest,
    ) -> Result<PolishOutcome, SuramakError> {
        let api_key = crate::keychain::get_api_key()?;
        let url = Self::completions_url(cfg);
        match audio {
            AudioRequest::Off => Ok(PolishOutcome::plain(
                self.text_only(&url, &api_key, segments, cfg)?,
            )),
            AudioRequest::Unavailable { note } => Ok(PolishOutcome {
                segments: self.text_only(&url, &api_key, segments, cfg)?,
                notes: vec![note],
            }),
            AudioRequest::Recording(path) => self.audio_aware(&url, &api_key, segments, cfg, &path),
        }
    }
}

/// Reads a recording for the audio-aware polish (M11): a 16 kHz mono PCM16
/// WAV goes on the wire as-is; anything else — an imported file, another WAV
/// shape — is decoded + resampled through the pipeline's own path. `Err`
/// carries the human-readable reason for the text-only fallback note.
fn prepare_recording(path: &Path) -> Result<Pcm16Wav, String> {
    let bytes =
        std::fs::read(path).map_err(|err| format!("could not read the recording: {err}"))?;
    if let Ok(wav) = Pcm16Wav::parse(&bytes) {
        if wav.is_16k_mono() {
            return Ok(wav);
        }
    }
    let buffer = crate::decode::decode_file(path)
        .map_err(|err| format!("could not decode the recording: {err}"))?;
    let samples = crate::resample::to_16khz_mono(&buffer)
        .map_err(|err| format!("could not resample the recording: {err}"))?;
    Ok(Pcm16Wav::from_f32_16k_mono(&samples))
}

/// The exact JSON body of one polish request (M11: `audio` adds the recording
/// as an OpenAI-compatible `input_audio` content part — base64 WAV, format
/// `"wav"`).
///
/// With `audio: None` the body is **byte-identical** to the pre-M11 text-only
/// request (pinned by tests).
pub fn build_body(
    segments: &[Segment],
    cfg: &PolishSettings,
    audio: Option<&AudioChunk>,
) -> Result<serde_json::Value, SuramakError> {
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

    let messages = match audio {
        None => serde_json::json!([
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user",   "content": user_content },
        ]),
        Some(chunk) => {
            let data = base64::engine::general_purpose::STANDARD.encode(&chunk.wav);
            serde_json::json!([
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": [
                    { "type": "text", "text": user_content },
                    { "type": "input_audio", "input_audio": { "data": data, "format": "wav" } },
                ]},
            ])
        }
    };

    Ok(serde_json::json!({
        "model": cfg.model,
        "temperature": 0,
        "messages": messages,
    }))
}

/// Extracts the assistant message's content from a chat-completions reply.
fn content_of(body: &str) -> Result<String, SuramakError> {
    let raw: serde_json::Value = serde_json::from_str(body)?;
    raw["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| SuramakError::BadResponse("no content field".to_string()))
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
