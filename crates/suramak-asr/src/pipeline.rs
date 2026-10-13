//! The transcription pipeline: decode (core) → resample to 16 kHz mono (core) →
//! whisper → core [`Transcript`].

use std::path::Path;

use suramak_core::settings::LangMode;
use suramak_core::transcript::{Source, Transcript};
use suramak_core::{decode, resample};
use whisper_rs::WhisperContext;

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

/// Runs the full pipeline on an audio file and returns a library-ready transcript.
///
/// Progress (0–100) comes from whisper's own progress callback; decode and resample
/// are fast for typical files and do not report progress.
#[allow(clippy::too_many_arguments)]
pub fn transcribe_file(
    ctx: &WhisperContext,
    input: &Path,
    model_path: &Path,
    lang: &LangMode,
    initial_prompt: Option<&str>,
    n_threads: i32,
    plan: &crate::engine::EngineDecision,
    on_progress: impl FnMut(u8),
) -> Result<PipelineOutcome, AsrError> {
    let buffer = decode::decode_file(input)?;
    let samples = resample::to_16khz_mono(&buffer)?;
    let outcome = whisper::transcribe(
        ctx,
        model_path,
        &samples,
        lang,
        initial_prompt,
        n_threads,
        plan,
        on_progress,
    )?;

    let title = input
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("transcript")
        .to_string();
    let language = match lang {
        LangMode::Auto => None,
        LangMode::Fixed(code) => Some(code.clone()),
    };
    let transcript = Transcript::new(
        title.clone(),
        Source::File { name: title },
        model_id_from_path(model_path),
        language,
        Some(input.to_path_buf()),
        outcome.segments,
    );
    Ok(PipelineOutcome {
        transcript,
        engine_used: outcome.engine_used,
        warnings: outcome.warnings,
    })
}
