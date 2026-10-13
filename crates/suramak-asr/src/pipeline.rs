//! The transcription pipeline: decode (core) → resample to 16 kHz mono (core) →
//! whisper → core [`Transcript`].

use std::path::Path;

use suramak_core::settings::LangMode;
use suramak_core::transcript::{Source, Transcript};
use suramak_core::{decode, resample};
use whisper_rs::WhisperContext;

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

/// Runs the full pipeline on an audio file and returns a library-ready transcript.
///
/// Progress (0–100) comes from whisper's own progress callback; decode and resample
/// are fast for typical files and do not report progress.
pub fn transcribe_file(
    ctx: &WhisperContext,
    input: &Path,
    model_path: &Path,
    lang: &LangMode,
    initial_prompt: Option<&str>,
    n_threads: i32,
    on_progress: impl FnMut(u8),
) -> Result<Transcript, AsrError> {
    let buffer = decode::decode_file(input)?;
    let samples = resample::to_16khz_mono(&buffer)?;
    let segments =
        whisper::transcribe(ctx, &samples, lang, initial_prompt, n_threads, on_progress)?;

    let title = input
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("transcript")
        .to_string();
    let language = match lang {
        LangMode::Auto => None,
        LangMode::Fixed(code) => Some(code.clone()),
    };
    Ok(Transcript::new(
        title.clone(),
        Source::File { name: title },
        model_id_from_path(model_path),
        language,
        Some(input.to_path_buf()),
        segments,
    ))
}
