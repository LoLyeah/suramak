//! The transcription pipeline: decode (core) → resample to 16 kHz mono (core) →
//! engine → core [`Transcript`].
//!
//! Two engines share this shape (docs/ENGINES.md): the whisper path
//! ([`transcribe_file`]) and the M10A Parakeet fast path
//! ([`transcribe_file_parakeet`]). Both decode and resample identically and both
//! report the engine that actually ran.
//!
//! M9e (P2-2): the pipeline carries the job's cooperative [`CancelToken`] and
//! checks it at every stage boundary — before the decode, after the decode,
//! after the resample and after the engine (where the abort callback has already
//! stopped the run between chunks). A cancellation returns
//! [`AsrError::Cancelled`] and drops the partial work; the audio decode itself
//! (core's path) is untouched — the token is observed between its stages.

use std::path::Path;

use suramak_core::settings::LangMode;
use suramak_core::transcript::{Segment, Source, Transcript};
use suramak_core::{decode, resample};
use whisper_rs::WhisperContext;

use crate::cancel::CancelToken;
use crate::engine::EngineId;
use crate::whisper::{self, AsrError};

/// Parses a language selector string (`auto`, `en`, `id`, …) into a [`LangMode`].
///
/// `auto` (case-insensitive) means whisper language auto-detection; anything else is
/// treated as a forced language code.
pub fn parse_lang(selector: &str) -> LangMode {
    if selector.eq_ignore_ascii_case("auto") {
        LangMode::Auto
    } else {
        LangMode::Fixed(selector.to_string())
    }
}

/// Derives the manifest-style model id from a model file path
/// (`…/ggml-tiny.bin` → `tiny`).
pub fn model_id_from_path(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    stem.strip_prefix("ggml-").unwrap_or(stem).to_string()
}

/// Result of a pipeline run: the transcript plus engine diagnostics.
#[derive(Debug, Clone)]
pub struct PipelineOutcome {
    pub transcript: Transcript,
    /// Engine that actually ran (may differ from the plan when Core ML fell back).
    pub engine_used: EngineId,
    /// Non-fatal notes: engine downgrades, Core ML fallback, ignored requests.
    pub warnings: Vec<String>,
}

/// The cooperative check point at a pipeline stage boundary (M9e, P2-2):
/// `Err(Cancelled)` when the token is set. Pure, so the rule is pinned without
/// a model; the pipeline calls it before the decode, after the decode, after
/// the resample and after the engine.
pub fn stage_check(cancel: &CancelToken) -> Result<(), AsrError> {
    if cancel.is_cancelled() {
        Err(AsrError::Cancelled)
    } else {
        Ok(())
    }
}

/// Decode + resample to 16 kHz mono with the stage-boundary checks — the shared
/// front half of every engine path.
fn prepare_samples(input: &Path, cancel: &CancelToken) -> Result<Vec<f32>, AsrError> {
    stage_check(cancel)?;
    let buffer = decode::decode_file(input)?;
    stage_check(cancel)?;
    let samples = resample::to_16khz_mono(&buffer)?;
    stage_check(cancel)?;
    Ok(samples)
}

/// Builds the library-ready transcript for `input`.
fn transcript_for(
    input: &Path,
    model_id: String,
    lang: &LangMode,
    segments: Vec<Segment>,
) -> Transcript {
    let title = input
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("transcript")
        .to_string();
    let language = match lang {
        LangMode::Auto => None,
        LangMode::Fixed(code) => Some(code.clone()),
    };
    Transcript::new(
        title.clone(),
        Source::File { name: title },
        model_id,
        language,
        Some(input.to_path_buf()),
        segments,
    )
}

/// Runs the full whisper pipeline on an audio file and returns a library-ready
/// transcript.
///
/// Progress (0–100) comes from whisper's own progress callback; decode and resample
/// are fast for typical files and do not report progress.
///
/// `cancel` (M9e, P2-2) is the job's cooperative token: it is checked at every
/// stage boundary and, inside whisper, between decoding chunks (the engine's
/// abort callback — see [`whisper::transcribe`]). A cancellation returns
/// [`AsrError::Cancelled`] and no partial transcript ever leaves this function.
#[allow(clippy::too_many_arguments)]
pub fn transcribe_file(
    ctx: &WhisperContext,
    input: &Path,
    model_path: &Path,
    lang: &LangMode,
    initial_prompt: Option<&str>,
    n_threads: i32,
    plan: &crate::engine::EngineDecision,
    cancel: &CancelToken,
    on_progress: impl FnMut(u8),
) -> Result<PipelineOutcome, AsrError> {
    let samples = prepare_samples(input, cancel)?;
    let outcome = whisper::transcribe(
        ctx,
        model_path,
        &samples,
        lang,
        initial_prompt,
        n_threads,
        plan,
        cancel,
        on_progress,
    )?;
    stage_check(cancel)?;
    Ok(PipelineOutcome {
        transcript: transcript_for(
            input,
            model_id_from_path(model_path),
            lang,
            outcome.segments,
        ),
        engine_used: outcome.engine_used,
        warnings: outcome.warnings,
    })
}

/// Runs the M10A Parakeet fast pipeline on an audio file.
///
/// Same decode → resample front half as [`transcribe_file`]; the engine decodes
/// the whole clip in one sherpa-onnx call. sherpa-onnx reports no progress, so
/// `on_progress` receives two honest milestones: 50 when the audio is ready and
/// 100 when the decode finished. A cancelled job returns
/// [`AsrError::Cancelled`] and no partial transcript.
pub fn transcribe_file_parakeet(
    recognizer: &mut crate::parakeet::ParakeetRecognizer,
    input: &Path,
    lang: &LangMode,
    plan: &crate::engine::EngineDecision,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(u8),
) -> Result<PipelineOutcome, AsrError> {
    let mut warnings = Vec::new();
    if let Some(warning) = &plan.warning {
        warnings.push(warning.clone());
    }
    let samples = prepare_samples(input, cancel)?;
    on_progress(50);
    let result = recognizer.transcribe(&samples, 16_000)?;
    stage_check(cancel)?;
    on_progress(100);
    let audio_ms = samples.len() as i64 * 1000 / 16_000;
    let segments = crate::parakeet::segments_from_result(&result, audio_ms);
    Ok(PipelineOutcome {
        transcript: transcript_for(input, crate::parakeet::MODEL_ID.to_string(), lang, segments),
        engine_used: EngineId::Parakeet,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M9e (P2-2): the stage-boundary rule — an uncancelled token passes, a
    /// cancelled one yields `Cancelled` (which the worker maps to its terminal
    /// `Cancelled` event; the partial work never leaves the pipeline).
    #[test]
    fn a_cancelled_token_fails_the_stage_check() {
        let token = CancelToken::new();
        assert!(stage_check(&token).is_ok());
        token.cancel();
        match stage_check(&token) {
            Err(AsrError::Cancelled) => {}
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    /// The engine paths share `prepare_samples`: a cancelled token returns
    /// `Cancelled` before the (here missing) input file is even touched.
    #[test]
    fn a_cancelled_token_stops_the_pipeline_before_decoding() {
        let token = CancelToken::new();
        token.cancel();
        match prepare_samples(Path::new("/nonexistent/en.wav"), &token) {
            Err(AsrError::Cancelled) => {}
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }
}
