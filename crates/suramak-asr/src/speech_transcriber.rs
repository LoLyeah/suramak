//! M10B: Apple **SpeechTranscriber** (the macOS 26+ SpeechAnalyzer pipeline) —
//! the macOS fast path, through the Rust **`speech` crate** (v0.8.x; the crate
//! wraps Apple's Speech.framework with a Swift bridge — `swift build` — so it
//! can only be built on a Mac with a Swift toolchain).
//!
//! Everything here is behind the `speech-transcriber` cargo feature **and** the
//! macOS target (`docs/WORKORDER-M10B.md`). Without either, the module still
//! exists with the same API shape — [`compiled()`] is `false`, every entry point
//! refuses honestly (`AsrError::SpeechTranscriberUnsupported`) — so routing can
//! never select the engine, builds stay identical, and every refusal/mapping
//! test runs on Linux.
//!
//! Facts that shape this module (verified against the crate's docs and sources,
//! v0.8.7):
//!
//! - The **analyzer path is whole-file**: `SpeechTranscriber::transcribe_in_path`
//!   runs `SpeechAnalyzer` over a file and returns the collected
//!   [`SttResult`]-shaped results. There is no in-memory sample input on this
//!   path — the analyzer reads the file itself (AVAudioFile) — so the pipeline's
//!   shared decode front half is kept for the uniform cancel/progress shape and
//!   for the clip duration used to clamp segment times (documented in
//!   `pipeline::transcribe_file_speech_transcriber`).
//! - The Swift bridge's analyzer entry point calls `spxEnsureAuthorized()`
//!   (`SFSpeechRecognizer.authorizationStatus()` must be `.authorized`), so the
//!   engine first ensures authorization — a one-time system prompt
//!   (`SpeechRecognizer::request_authorization`), the documented Apple flow.
//!   The bundle carries `NSSpeechRecognitionUsageDescription` for it
//!   (`packaging/Info.plist`).
//! - **Assets are OS-managed** (`AssetInventory`): the locale model ships via
//!   the system, auto-updates with it, and there is nothing to pin, download or
//!   host for this engine — no `models` entries, no `MODEL-PINS` rows. When the
//!   locale asset is missing, [`SpeechTranscriberEngine::transcribe`] attempts
//!   the crate's wrapped installation request; **any failure degrades to
//!   Whisper with an actionable message** (the worker does the degrade) —
//!   never a hard failure.
//! - Granularity (honest mapping): results carry a result-level audio time range
//!   and — with the time-indexed preset — per-span time-range attributes over
//!   the attributed text. [`segments_from_results`] builds segments from the
//!   time-stamped spans (splitting at pauses ≥ [`SEGMENT_GAP_S`], the same 0.5 s
//!   rule as the Parakeet fast engine); when no span carries a time range it
//!   falls back to one segment per result range, and when there is no time
//!   information at all, one whole-clip segment (all three tiers documented on
//!   the function).
//! - `live` transcription stays whisper by design; this engine runs on file
//!   jobs only. Every ST runtime behavior is **owner-verify** (the module cannot
//!   execute on Linux — see `docs/reports/M10b.md`).

use std::path::Path;

use suramak_core::transcript::Segment;

use crate::whisper::AsrError;

/// Whether this build can run Apple SpeechTranscriber: the `speech-transcriber`
/// cargo feature **and** a macOS target (the `speech` crate is a Swift bridge —
/// it cannot compile anywhere else).
pub const fn compiled() -> bool {
    cfg!(all(feature = "speech-transcriber", target_os = "macos"))
}

/// The model id recorded in transcripts produced by this engine.
pub const MODEL_ID: &str = "speech-transcriber";

/// Pause (seconds) that starts a new reconstructed segment — the same 0.5 s
/// rule as the Parakeet fast engine (`parakeet::SEGMENT_GAP_S`).
pub const SEGMENT_GAP_S: f64 = 0.5;

// ---------------------------------------------------------------------------
// The pure mapping: crate-independent result shape → `Segment`s
// ---------------------------------------------------------------------------

/// One time-stamped piece of a result's text: a span of the attributed text
/// that carries an audio time range (seconds). The macOS path slices these out
/// of the crate's `SpeechAttributedText`; tests build them by hand on Linux.
#[derive(Debug, Clone, PartialEq)]
pub struct SttSpan {
    pub text: String,
    pub start_s: f64,
    pub end_s: f64,
}

/// One SpeechTranscriber result as the pure mapping sees it — deliberately
/// crate-independent, so the mapping is unit-tested on every platform and the
/// macOS-gated code only converts the crate's values into this shape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SttResult {
    /// The result's full text (the attributed text flattened).
    pub text: String,
    /// Time-stamped spans in text order; empty when no span carried a time
    /// range (a preset without time indexing).
    pub spans: Vec<SttSpan>,
    /// The result's own audio time range `(start_s, duration_s)`, when usable.
    pub range_s: Option<(f64, f64)>,
    /// Whether the analyzer marked this result final. Volatile results are
    /// dropped whenever any final result exists (the M7 rule: provisional text
    /// never reaches a saved transcript).
    pub is_final: bool,
}

fn to_ms(seconds: f64) -> i64 {
    // `as` saturates on overflow; NaN becomes 0.
    (seconds * 1000.0).round() as i64
}

/// Whether a piece holds no alphanumeric character at all (pure punctuation,
/// possibly with surrounding spaces) — the same fold rule as the Parakeet
/// engine: a pause before the full stop must not start a segment with stray
/// punctuation.
fn is_punctuation_only(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty() && !trimmed.chars().any(char::is_alphanumeric)
}

/// The end of a result's own time range, clamped into the clip (else the clip end).
fn result_end_ms(range_s: Option<(f64, f64)>, audio_ms: i64) -> i64 {
    match range_s {
        Some((start_s, dur_s)) if start_s.is_finite() && dur_s.is_finite() && dur_s > 0.0 => {
            to_ms(start_s + dur_s).clamp(0, audio_ms)
        }
        _ => audio_ms,
    }
}

/// Builds segments from the time-stamped spans of one result.
///
/// - Pieces (spans with text) are folded first: a pure-punctuation piece joins
///   the previous piece, so a full stop that lands after a pause stays with the
///   sentence it terminates.
/// - The folded pieces are grouped at pauses ≥ [`SEGMENT_GAP_S`] — a pause is
///   the gap between the previous piece's *end* and the next piece's *start*
///   (real silence), not between starts: a span can cover a whole phrase.
///   A piece without a usable time range folds into the current group (its text
///   is kept, its time is not).
/// - Each segment runs to the start of the next group; the last one runs to the
///   result's own end (when its range is usable) or the clip end. Every time is
///   clamped into `[0, audio_ms]`.
fn span_segments(
    spans: &[SttSpan],
    range_s: Option<(f64, f64)>,
    audio_ms: i64,
) -> Vec<(i64, i64, String)> {
    // Fold punctuation-only spans into the previous piece.
    let mut folded: Vec<(Option<(i64, i64)>, String)> = Vec::new();
    for span in spans {
        let text = span.text.as_str();
        if text.trim().is_empty() {
            continue;
        }
        if is_punctuation_only(text) {
            if let Some((_, prev)) = folded.last_mut() {
                prev.push_str(text);
                continue;
            }
        }
        let times = if span.start_s.is_finite()
            && span.end_s.is_finite()
            && span.start_s >= 0.0
            && span.end_s >= span.start_s
        {
            Some((to_ms(span.start_s), to_ms(span.end_s)))
        } else {
            None
        };
        folded.push((times, text.to_string()));
    }
    if folded.is_empty() {
        return Vec::new();
    }

    // Group at pauses ≥ SEGMENT_GAP_S: the silence between one piece's end and
    // the next piece's start. A piece without usable times clears the chain —
    // the gap across it cannot be measured, so it never forces a split.
    let mut groups: Vec<(Option<(i64, i64)>, String)> = Vec::new();
    let mut prev_end: Option<i64> = None;
    for (times, text) in folded {
        let split = match (prev_end, times) {
            (Some(prev_end), Some((next_start, _))) => {
                (next_start - prev_end) as f64 / 1000.0 >= SEGMENT_GAP_S
            }
            _ => false,
        };
        if split {
            groups.push((times, text));
        } else if let Some((group_times, group_text)) = groups.last_mut() {
            if group_times.is_none() {
                *group_times = times;
            }
            group_text.push_str(&text);
        } else {
            groups.push((times, text));
        }
        prev_end = times.map(|(_, end)| end);
    }

    // Resolve starts (explicit when a piece carried a time, else continue from
    // the previous segment) and ends (the next group's start, or the result end).
    let mut starts = Vec::with_capacity(groups.len());
    let mut cursor = 0i64;
    for (group_times, _) in &groups {
        let start = group_times
            .map(|(start, _)| start.clamp(0, audio_ms))
            .unwrap_or(cursor);
        starts.push(start);
        cursor = start;
    }
    let last_end = result_end_ms(range_s, audio_ms);
    let mut out = Vec::with_capacity(groups.len());
    for (i, (_, text)) in groups.iter().enumerate() {
        let start = starts[i];
        let end = starts
            .get(i + 1)
            .map(|next| (*next).max(start))
            .unwrap_or(last_end.max(start));
        let text = text.trim().to_string();
        if !text.is_empty() {
            out.push((start, end, text));
        }
    }
    out
}

/// Maps SpeechTranscriber results onto the existing [`Segment`] shape.
///
/// Three tiers, in order (documented in `docs/ENGINES.md`):
///
/// 1. **Time-stamped spans** — when the result's attributed text carries spans
///    with audio time ranges, segments come from [`span_segments`] (pause
///    splitting at [`SEGMENT_GAP_S`], the Parakeet rule).
/// 2. **Result range** — no time-stamped spans, but the result's own
///    `(start, duration)` is usable → one segment for that range.
/// 3. **Whole clip** — no time information at all → one segment `[0, audio_ms]`
///    carrying the whole text (the honest fallback).
///
/// Volatile results are dropped whenever any final result exists; a run that
/// returned only volatile results is mapped as-is (better than nothing — the
/// crate only returns volatile results when the caller asked for them). Empty
/// results map to no segments. Times are clamped into the clip.
pub fn segments_from_results(results: &[SttResult], audio_ms: i64) -> Vec<Segment> {
    let audio_ms = audio_ms.max(0);
    let any_final = results.iter().any(|result| result.is_final);
    let mut segments: Vec<Segment> = Vec::new();
    for result in results.iter().filter(|r| !any_final || r.is_final) {
        let pieces = if result.spans.is_empty() {
            Vec::new()
        } else {
            span_segments(&result.spans, result.range_s, audio_ms)
        };
        if !pieces.is_empty() {
            for (start_ms, end_ms, text) in pieces {
                segments.push(Segment {
                    idx: segments.len() as u32,
                    start_ms,
                    end_ms,
                    text,
                });
            }
            continue;
        }
        let text = result.text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        let (start_ms, end_ms) = match result.range_s {
            Some((start_s, dur_s)) if start_s.is_finite() && dur_s.is_finite() && dur_s > 0.0 => {
                let start_ms = to_ms(start_s).clamp(0, audio_ms);
                let end_ms = to_ms(start_s + dur_s).clamp(start_ms, audio_ms);
                (start_ms, end_ms)
            }
            _ => (0, audio_ms),
        };
        segments.push(Segment {
            idx: segments.len() as u32,
            start_ms,
            end_ms,
            text,
        });
    }
    segments
}

// ---------------------------------------------------------------------------
// Availability, assets and authorization (probes for routing + doctor)
// ---------------------------------------------------------------------------

/// The OS state of this engine's locale asset (`AssetInventory.Status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttAssetState {
    /// The OS does not support this module/locale at all.
    Unsupported,
    /// Supported and downloadable; not installed yet.
    Supported,
    /// An installation is in flight.
    Downloading,
    /// Installed and ready.
    Installed,
    /// The probe failed (honest unknown, with the reason).
    Unknown(String),
}

impl SttAssetState {
    /// A human label for doctor output.
    pub fn label(&self) -> String {
        match self {
            SttAssetState::Unsupported => "unsupported by this OS/locale".to_string(),
            SttAssetState::Supported => {
                "supported, not installed yet (installed on first use)".to_string()
            }
            SttAssetState::Downloading => "installation in progress".to_string(),
            SttAssetState::Installed => "installed".to_string(),
            SttAssetState::Unknown(reason) => format!("unknown ({reason})"),
        }
    }
}

/// The app's speech-recognition authorization state (the analyzer path requires
/// `Authorized` — the crate's Swift bridge calls `spxEnsureAuthorized()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttAuthorization {
    NotDetermined,
    Denied,
    Restricted,
    Authorized,
    Unknown,
}

impl SttAuthorization {
    /// A human label for doctor output.
    pub fn label(self) -> &'static str {
        match self {
            SttAuthorization::NotDetermined => {
                "not determined yet (a one-time system prompt appears on first use)"
            }
            SttAuthorization::Denied => {
                "denied — allow Suramak under System Settings → Privacy & Security → \
                 Speech Recognition"
            }
            SttAuthorization::Restricted => "restricted by policy on this Mac",
            SttAuthorization::Authorized => "granted",
            SttAuthorization::Unknown => "unknown",
        }
    }
}

/// Whether SpeechTranscriber can run right now for `language`: the platform
/// probe (`SpeechTranscriber.isAvailable`) **and** the OS supporting the
/// language's locale. `None` (auto-detect) is always `false` — the language is
/// unknown at plan time, so an Indonesian clip could otherwise reach the engine.
///
/// Off-macOS or without the feature this is always `false` (the honest stub).
#[cfg(all(feature = "speech-transcriber", target_os = "macos"))]
pub fn available_for(language: Option<&str>) -> bool {
    let Some(code) = language else {
        return false;
    };
    speech::analyzer::SpeechTranscriber::is_available() && imp::resolve_locale(code).is_ok()
}

/// [`available_for`] without the feature or off-macOS: always `false`.
#[cfg(not(all(feature = "speech-transcriber", target_os = "macos")))]
pub fn available_for(language: Option<&str>) -> bool {
    let _ = language;
    false
}

/// Whether the OS has a working SpeechTranscriber at all (macOS 26+).
#[cfg(all(feature = "speech-transcriber", target_os = "macos"))]
pub fn os_available() -> bool {
    speech::analyzer::SpeechTranscriber::is_available()
}

/// [`os_available`] without the feature or off-macOS: `false`.
#[cfg(not(all(feature = "speech-transcriber", target_os = "macos")))]
pub fn os_available() -> bool {
    false
}

/// The OS's SpeechTranscriber locale list (for doctor). Refuses off-macOS or
/// without the feature.
#[cfg(all(feature = "speech-transcriber", target_os = "macos"))]
pub fn supported_locales() -> Result<Vec<String>, AsrError> {
    speech::analyzer::SpeechTranscriber::supported_locales().map_err(|err| {
        AsrError::SpeechTranscriber(format!("could not query supported locales: {err}"))
    })
}

/// [`supported_locales`] without the feature or off-macOS: honest refusal.
#[cfg(not(all(feature = "speech-transcriber", target_os = "macos")))]
pub fn supported_locales() -> Result<Vec<String>, AsrError> {
    Err(AsrError::SpeechTranscriberUnsupported)
}

/// The app's speech-recognition authorization state (`None` off-macOS or
/// without the feature — there is nothing to report).
#[cfg(all(feature = "speech-transcriber", target_os = "macos"))]
pub fn authorization() -> Option<SttAuthorization> {
    use speech::error::AuthorizationStatus;
    use speech::recognizer::SpeechRecognizer;
    Some(match SpeechRecognizer::authorization_status() {
        AuthorizationStatus::NotDetermined => SttAuthorization::NotDetermined,
        AuthorizationStatus::Denied => SttAuthorization::Denied,
        AuthorizationStatus::Restricted => SttAuthorization::Restricted,
        AuthorizationStatus::Authorized => SttAuthorization::Authorized,
        _ => SttAuthorization::Unknown,
    })
}

/// [`authorization`] without the feature or off-macOS: `None`.
#[cfg(not(all(feature = "speech-transcriber", target_os = "macos")))]
pub fn authorization() -> Option<SttAuthorization> {
    None
}

/// The locale asset state for a forced language code (probe only — no install).
#[cfg(all(feature = "speech-transcriber", target_os = "macos"))]
pub fn asset_status(language_code: &str) -> Result<SttAssetState, AsrError> {
    let locale = imp::resolve_locale(language_code)?;
    Ok(imp::asset_state(&locale))
}

/// [`asset_status`] without the feature or off-macOS: honest refusal.
#[cfg(not(all(feature = "speech-transcriber", target_os = "macos")))]
pub fn asset_status(language_code: &str) -> Result<SttAssetState, AsrError> {
    let _ = language_code;
    Err(AsrError::SpeechTranscriberUnsupported)
}

// ---------------------------------------------------------------------------
// The engine (same API shape as `ParakeetRecognizer`)
// ---------------------------------------------------------------------------

/// The SpeechTranscriber engine for one forced language.
///
/// There is no model to load — assets are OS-managed — so the engine is a thin
/// configuration handle (the OS locale identifier). [`SpeechTranscriberEngine::transcribe`]
/// ensures authorization and the locale asset first; the first run may trigger
/// the one-time permission prompt and the OS asset download.
#[derive(Debug, Clone)]
pub struct SpeechTranscriberEngine {
    /// The OS locale identifier resolved from the forced language (`en` → `en-US`).
    locale: String,
}

impl SpeechTranscriberEngine {
    /// Builds the engine for a forced language code (e.g. `en`).
    ///
    /// macOS + feature: resolves the language to a locale the OS supports
    /// (`supportedLocale(equivalentTo:)`, with region hints); an unsupported
    /// locale is an honest refusal. Everywhere else: the stub refusal.
    #[cfg(all(feature = "speech-transcriber", target_os = "macos"))]
    pub fn for_language(code: &str) -> Result<Self, AsrError> {
        Ok(Self {
            locale: imp::resolve_locale(code)?,
        })
    }

    /// [`SpeechTranscriberEngine::for_language`] without the feature or off-macOS.
    #[cfg(not(all(feature = "speech-transcriber", target_os = "macos")))]
    pub fn for_language(code: &str) -> Result<Self, AsrError> {
        let _ = code;
        Err(AsrError::SpeechTranscriberUnsupported)
    }

    /// The resolved OS locale identifier (e.g. `en-US`).
    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// Transcribes a whole file, ensuring authorization and the locale asset
    /// first. macOS + feature only; every failure is an honest `Err` that the
    /// worker degrades to Whisper.
    #[cfg(all(feature = "speech-transcriber", target_os = "macos"))]
    pub fn transcribe(&self, input: &Path) -> Result<Vec<SttResult>, AsrError> {
        imp::ensure_authorized()?;
        imp::ensure_asset(&self.locale)?;
        imp::transcribe(&self.locale, input)
    }

    /// [`SpeechTranscriberEngine::transcribe`] without the feature or off-macOS.
    #[cfg(not(all(feature = "speech-transcriber", target_os = "macos")))]
    pub fn transcribe(&self, input: &Path) -> Result<Vec<SttResult>, AsrError> {
        let _ = input;
        Err(AsrError::SpeechTranscriberUnsupported)
    }
}

// ---------------------------------------------------------------------------
// The macOS implementation (feature `speech-transcriber`, target macOS)
// ---------------------------------------------------------------------------

#[cfg(all(feature = "speech-transcriber", target_os = "macos"))]
mod imp {
    //! The only place the `speech` crate is touched. Every call is defended:
    //! every failure becomes an `AsrError::SpeechTranscriber` message the worker
    //! turns into a Whisper degradation.
    //!
    //! API surface pinned against `speech` 0.8.7 (docs.rs + the crate's sources
    //! on `doom-fish/speech-rs`):
    //! - `analyzer::SpeechTranscriber::{is_available, supported_locales,
    //!   supported_locale_equivalent_to, new(locale, preset), transcribe_in_path}`
    //! - `analyzer::SpeechTranscriberPreset::TimeIndexedTranscriptionWithAlternatives`
    //! - `asset_inventory::{AssetInventory, AssetInventoryStatus}` +
    //!   `AssetInstallationRequest::download_and_install`
    //! - `recognizer::SpeechRecognizer::{authorization_status, request_authorization}`
    //!   + `error::AuthorizationStatus` (the bridge's `spxEnsureAuthorized` gate)
    //! - result shape: `SpeechTranscriptionResult { text: SpeechAttributedText
    //!   { text, spans: Vec<SpeechAttributeSpan { start, end, audio_time_range }> },
    //!   audio_time_range, is_final, .. }` — `start`/`end` are byte offsets into
    //!   the flattened text.

    use std::path::Path;

    use speech::analyzer::{SpeechTranscriber, SpeechTranscriberPreset, SpeechTranscriptionResult};
    use speech::asset_inventory::{AssetInventory, AssetInventoryStatus};

    use super::{SttAssetState, SttResult, SttSpan};
    use crate::whisper::AsrError;

    /// Whole-file transcription with time-indexed results (per-span audio time
    /// ranges) — the preset the crate's own macOS-26 file smoke uses.
    const PRESET: SpeechTranscriberPreset =
        SpeechTranscriberPreset::TimeIndexedTranscriptionWithAlternatives;

    /// Region hints tried when the OS does not accept the bare language code as
    /// a locale identifier. The OS check stays authoritative: a hint only helps
    /// it resolve the code we already know it covers.
    fn locale_hints(code: &str) -> &'static [&'static str] {
        match code {
            "en" => &["en-US", "en-GB"],
            "es" => &["es-ES", "es-US"],
            "fr" => &["fr-FR", "fr-CA"],
            "de" => &["de-DE"],
            "it" => &["it-IT"],
            "pt" => &["pt-BR", "pt-PT"],
            "ja" => &["ja-JP"],
            "ko" => &["ko-KR"],
            "zh" => &["zh-CN", "zh-Hans"],
            "yue" => &["yue-CN", "yue-Hant"],
            _ => &[],
        }
    }

    /// Resolves a language code to a locale identifier the OS supports, via the
    /// crate's `supportedLocale(equivalentTo:)` (then the region hints).
    pub(super) fn resolve_locale(code: &str) -> Result<String, AsrError> {
        let code = code.trim().to_ascii_lowercase();
        if code.is_empty() {
            return Err(AsrError::SpeechTranscriber(
                "SpeechTranscriber needs a forced language code".to_string(),
            ));
        }
        match SpeechTranscriber::supported_locale_equivalent_to(&code) {
            Ok(Some(locale)) => return Ok(locale),
            Ok(None) => {}
            Err(err) => {
                return Err(AsrError::SpeechTranscriber(format!(
                    "the OS locale check failed for {code:?}: {err}"
                )))
            }
        }
        for hint in locale_hints(&code) {
            match SpeechTranscriber::supported_locale_equivalent_to(hint) {
                Ok(Some(locale)) => return Ok(locale),
                Ok(None) => {}
                Err(err) => {
                    return Err(AsrError::SpeechTranscriber(format!(
                        "the OS locale check failed for {hint:?}: {err}"
                    )))
                }
            }
        }
        Err(AsrError::SpeechTranscriber(format!(
            "this Mac's SpeechTranscriber does not support language {code:?} \
             (see `suramak-cli doctor` for the supported locales)"
        )))
    }

    /// Ensures speech-recognition authorization: the bridge's analyzer entry
    /// point requires `.authorized`, so a not-determined app asks once (the
    /// documented Apple flow — the system prompt needs
    /// `NSSpeechRecognitionUsageDescription`, which the bundle carries). A
    /// denial/restriction is an honest refusal with the fix.
    pub(super) fn ensure_authorized() -> Result<(), AsrError> {
        use speech::error::AuthorizationStatus;
        use speech::recognizer::SpeechRecognizer;

        let status = SpeechRecognizer::authorization_status();
        if status.is_authorized() {
            return Ok(());
        }
        let status = if status == AuthorizationStatus::NotDetermined {
            SpeechRecognizer::request_authorization()
        } else {
            status
        };
        if status.is_authorized() {
            return Ok(());
        }
        Err(AsrError::SpeechTranscriber(format!(
            "speech recognition is not authorized ({status:?}) — allow Suramak under \
             System Settings → Privacy & Security → Speech Recognition, then retry \
             (whisper is unaffected)"
        )))
    }

    /// The locale asset state (`AssetInventory.Status` for the transcriber). A
    /// probe failure is `Unknown(reason)` — [`ensure_asset`] treats that as a
    /// failure, `doctor` just prints it.
    pub(super) fn asset_state(locale: &str) -> SttAssetState {
        let transcriber = SpeechTranscriber::new(locale.to_string(), PRESET);
        match AssetInventory::status_for_modules([&transcriber]) {
            // `AssetInventoryStatus` is a plain (non-`non_exhaustive`) enum in
            // 0.8.x: all four variants are matched, so a future variant is a
            // compile error here rather than a silent mapping.
            Ok(AssetInventoryStatus::Unsupported) => SttAssetState::Unsupported,
            Ok(AssetInventoryStatus::Supported) => SttAssetState::Supported,
            Ok(AssetInventoryStatus::Downloading) => SttAssetState::Downloading,
            Ok(AssetInventoryStatus::Installed) => SttAssetState::Installed,
            Err(err) => SttAssetState::Unknown(format!("asset check failed for {locale}: {err}")),
        }
    }

    /// Ensures the locale asset is installed, attempting the crate's wrapped
    /// installation request when it is not. Any failure is an honest, actionable
    /// `Err` — the worker degrades to Whisper.
    pub(super) fn ensure_asset(locale: &str) -> Result<(), AsrError> {
        match asset_state(locale) {
            SttAssetState::Installed => return Ok(()),
            SttAssetState::Unsupported => {
                return Err(AsrError::SpeechTranscriber(format!(
                    "the OS reports SpeechTranscriber assets unsupported for {locale}"
                )))
            }
            SttAssetState::Supported | SttAssetState::Downloading => {}
            SttAssetState::Unknown(reason) => {
                return Err(AsrError::SpeechTranscriber(format!(
                    "the asset state for {locale} could not be determined: {reason}"
                )))
            }
        }
        let transcriber = SpeechTranscriber::new(locale.to_string(), PRESET);
        let request = AssetInventory::asset_installation_request_for_modules([&transcriber])
            .map_err(|err| {
                AsrError::SpeechTranscriber(format!(
                    "the OS refused an asset installation request for {locale}: {err}"
                ))
            })?;
        let Some(request) = request else {
            return Err(AsrError::SpeechTranscriber(format!(
                "the OS offered no installable SpeechTranscriber asset for {locale} — \
                 check System Settings → General → Language & Region, then retry"
            )));
        };
        request.download_and_install().map_err(|err| {
            AsrError::SpeechTranscriber(format!(
                "installing the SpeechTranscriber asset for {locale} failed: {err} — \
                 check the network, then retry"
            ))
        })?;
        match asset_state(locale) {
            SttAssetState::Installed => Ok(()),
            other => Err(AsrError::SpeechTranscriber(format!(
                "the SpeechTranscriber asset for {locale} is still not installed after \
                 the install request ({})",
                other.label()
            ))),
        }
    }

    /// Runs the analyzer over `input` and converts the results to [`SttResult`]s.
    pub(super) fn transcribe(locale: &str, input: &Path) -> Result<Vec<SttResult>, AsrError> {
        let transcriber = SpeechTranscriber::new(locale.to_string(), PRESET);
        let results = transcriber.transcribe_in_path(input).map_err(|err| {
            AsrError::SpeechTranscriber(format!(
                "SpeechTranscriber failed on {}: {err}",
                input.display()
            ))
        })?;
        Ok(results.iter().map(result_to_stt).collect())
    }

    /// Converts one crate result into the pure mapping's shape. Span byte
    /// ranges that do not slice cleanly (a non-UTF-8 boundary) are skipped —
    /// the result-level/whole-clip fallbacks still carry the text.
    fn result_to_stt(result: &SpeechTranscriptionResult) -> SttResult {
        let spans = result
            .text
            .spans
            .iter()
            .filter_map(|span| {
                let time_range = span.audio_time_range?;
                let text = result.text.text.get(span.start..span.end)?;
                let range = time_range.value();
                Some(SttSpan {
                    text: text.to_string(),
                    start_s: range.start_seconds,
                    end_s: range.end_seconds(),
                })
            })
            .collect();
        SttResult {
            text: result.text.text.clone(),
            spans,
            range_s: Some((
                result.audio_time_range.start_seconds,
                result.audio_time_range.duration_seconds,
            )),
            is_final: result.is_final,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str, start_s: f64, end_s: f64) -> SttSpan {
        SttSpan {
            text: text.to_string(),
            start_s,
            end_s,
        }
    }

    fn result(
        text: &str,
        spans: Vec<SttSpan>,
        range_s: Option<(f64, f64)>,
        is_final: bool,
    ) -> SttResult {
        SttResult {
            text: text.to_string(),
            spans,
            range_s,
            is_final,
        }
    }

    // ---- the stub refusals (the Linux surface) ------------------------------

    #[test]
    fn without_the_feature_or_off_macos_the_engine_is_not_compiled() {
        if cfg!(all(feature = "speech-transcriber", target_os = "macos")) {
            assert!(compiled());
            return;
        }
        assert!(!compiled());
        assert!(!os_available());
        assert!(!available_for(Some("en")));
        assert!(!available_for(None));
        assert_eq!(authorization(), None);
        assert!(matches!(
            supported_locales(),
            Err(AsrError::SpeechTranscriberUnsupported)
        ));
        assert!(matches!(
            asset_status("en"),
            Err(AsrError::SpeechTranscriberUnsupported)
        ));
        let err = SpeechTranscriberEngine::for_language("en")
            .expect_err("the stub must refuse to build an engine");
        assert!(matches!(err, AsrError::SpeechTranscriberUnsupported));
    }

    #[test]
    fn model_id_is_the_engine_name() {
        assert_eq!(MODEL_ID, "speech-transcriber");
        assert_eq!(SEGMENT_GAP_S, 0.5);
    }

    // ---- the pure mapping ---------------------------------------------------

    #[test]
    fn spans_split_at_pauses_and_span_the_clip() {
        // Two sentences, a 0.6 s pause between them.
        let spans = vec![
            span(" The quick brown fox", 0.0, 1.2),
            span(" jumps over the lazy dog.", 1.2, 2.0),
            span(" Sur", 2.6, 2.8),
            span("amak keeps every recording.", 2.8, 4.4),
        ];
        let segments = segments_from_results(
            &[result(
                "The quick brown fox jumps over the lazy dog. Suramak keeps every recording.",
                spans,
                Some((0.0, 4.4)),
                true,
            )],
            9_000,
        );
        assert_eq!(segments.len(), 2, "{segments:?}");
        assert_eq!(segments[0].idx, 0);
        assert_eq!(
            segments[0].text,
            "The quick brown fox jumps over the lazy dog."
        );
        assert_eq!(segments[0].start_ms, 0);
        // The first segment runs to the start of the second one.
        assert_eq!(segments[0].end_ms, 2_600);
        assert_eq!(segments[1].idx, 1);
        assert_eq!(segments[1].text, "Suramak keeps every recording.");
        assert_eq!(segments[1].start_ms, 2_600);
        // The last segment runs to the result's end (not the clip end).
        assert_eq!(segments[1].end_ms, 4_400);
    }

    #[test]
    fn contiguous_spans_stay_one_segment() {
        let spans = vec![
            span(" hello", 0.0, 0.4),
            span(" world", 0.4, 0.8),
            span(" again", 0.9, 1.2),
        ];
        let segments = segments_from_results(
            &[result("hello world again", spans, Some((0.0, 1.2)), true)],
            2_000,
        );
        assert_eq!(segments.len(), 1, "{segments:?}");
        assert_eq!(segments[0].text, "hello world again");
        assert_eq!(segments[0].start_ms, 0);
        assert_eq!(segments[0].end_ms, 1_200);
    }

    #[test]
    fn punctuation_after_a_pause_folds_into_the_sentence_it_ends() {
        // The pause lands before the full stop: the "." span must not start a
        // segment of its own (the Parakeet fold rule).
        let spans = vec![
            span(" dog", 1.2, 1.6),
            span(".", 2.2, 2.3),
            span(" Next", 2.4, 2.8),
        ];
        let segments =
            segments_from_results(&[result("dog. Next", spans, Some((1.2, 1.6)), true)], 3_000);
        assert_eq!(segments.len(), 2, "{segments:?}");
        assert_eq!(segments[0].text, "dog.");
        assert_eq!(segments[1].text, "Next");
        assert_eq!(segments[1].start_ms, 2_400);
    }

    #[test]
    fn a_span_without_a_time_folds_into_the_current_group() {
        let spans = vec![
            span(" one", 0.0, 0.3),
            SttSpan {
                text: " two".to_string(),
                start_s: f64::NAN,
                end_s: f64::NAN,
            },
            span(" three", 1.0, 1.4),
        ];
        let segments = segments_from_results(
            &[result("one two three", spans, Some((0.0, 1.4)), true)],
            2_000,
        );
        assert_eq!(segments.len(), 1, "{segments:?}");
        assert_eq!(segments[0].text, "one two three");
        assert_eq!(segments[0].start_ms, 0);
    }

    #[test]
    fn without_spans_the_result_range_makes_one_segment() {
        let segments = segments_from_results(
            &[result(" hello world", Vec::new(), Some((0.5, 1.5)), true)],
            3_000,
        );
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "hello world");
        assert_eq!(segments[0].start_ms, 500);
        assert_eq!(segments[0].end_ms, 2_000);
    }

    #[test]
    fn without_any_time_information_the_whole_clip_is_one_segment() {
        let segments =
            segments_from_results(&[result(" hello world", Vec::new(), None, true)], 2_500);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start_ms, 0);
        assert_eq!(segments[0].end_ms, 2_500);
        assert_eq!(segments[0].text, "hello world");
    }

    #[test]
    fn volatile_results_are_dropped_when_a_final_one_exists() {
        let results = vec![
            result("volatile part", Vec::new(), Some((0.0, 1.0)), false),
            result("final text", Vec::new(), Some((0.0, 2.0)), true),
        ];
        let segments = segments_from_results(&results, 2_000);
        assert_eq!(segments.len(), 1, "{segments:?}");
        assert_eq!(segments[0].text, "final text");

        // A run that returned only volatile results is mapped as-is.
        let only_volatile = vec![result("volatile", Vec::new(), None, false)];
        let segments = segments_from_results(&only_volatile, 1_000);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "volatile");
    }

    #[test]
    fn empty_results_map_to_no_segments() {
        assert!(segments_from_results(&[], 1_000).is_empty());
        assert!(segments_from_results(&[result("   ", Vec::new(), None, true)], 1_000).is_empty());
        // Empty text with empty spans too.
        assert!(segments_from_results(
            &[result(
                "",
                vec![span(" ", 0.0, 0.1)],
                Some((0.0, 0.1)),
                true
            )],
            1_000
        )
        .is_empty());
    }

    #[test]
    fn times_are_clamped_into_the_clip() {
        // Span times past the audio end (padding) cannot leave the clip.
        let spans = vec![span(" a", 0.0, 1.0), span(" b", 5.0, 9.0)];
        let segments =
            segments_from_results(&[result("a b", spans, Some((0.0, 9.0)), true)], 1_000);
        for segment in &segments {
            assert!(
                segment.start_ms >= 0 && segment.start_ms <= segment.end_ms,
                "{segment:?}"
            );
            assert!(segment.end_ms <= 1_000, "{segment:?}");
        }
        // A result range that starts past the audio end still yields a
        // well-formed segment.
        let segments = segments_from_results(
            &[result(" late", Vec::new(), Some((5.0, 1.0)), true)],
            1_000,
        );
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start_ms, 1_000);
        assert_eq!(segments[0].end_ms, 1_000);
    }

    #[test]
    fn multiple_results_are_mapped_in_order_with_sequential_indices() {
        let results = vec![
            result(" first", Vec::new(), Some((0.0, 1.0)), true),
            result(" second", Vec::new(), Some((1.0, 1.0)), true),
        ];
        let segments = segments_from_results(&results, 3_000);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].idx, 0);
        assert_eq!(segments[1].idx, 1);
        assert_eq!(segments[0].text, "first");
        assert_eq!(segments[1].text, "second");
    }

    #[test]
    fn zero_length_audio_keeps_the_text_with_zeroed_times() {
        let segments =
            segments_from_results(&[result(" text", Vec::new(), Some((0.0, 1.0)), true)], 0);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start_ms, 0);
        assert_eq!(segments[0].end_ms, 0);
        assert_eq!(segments[0].text, "text");
    }

    #[test]
    fn asset_state_labels_are_honest() {
        assert_eq!(SttAssetState::Installed.label(), "installed".to_string());
        assert!(SttAssetState::Supported.label().contains("first use"));
        assert!(SttAssetState::Unknown("boom".to_string())
            .label()
            .contains("boom"));
        assert!(SttAuthorization::Denied
            .label()
            .contains("Speech Recognition"));
        assert_eq!(SttAuthorization::Authorized.label(), "granted");
    }
}
