//! The transcription pipeline: decode (core) → resample to 16 kHz mono (core) →
//! engine → core [`Transcript`].
//!
//! Three engines share this shape (docs/ENGINES.md): the whisper path
//! ([`transcribe_file`]), the M10A Parakeet fast path
//! ([`transcribe_file_parakeet`]) and the M10B Apple SpeechTranscriber macOS fast
//! path ([`transcribe_file_speech_transcriber`]). All decode and resample
//! identically and all report the engine that actually ran.
//!
//! M9e (P2-2): the pipeline carries the job's cooperative [`CancelToken`] and
//! checks it at every stage boundary — before the decode, after the decode,
//! after the resample and after the engine (where the abort callback has already
//! stopped the run between chunks). A cancellation returns
//! [`AsrError::Cancelled`] and drops the partial work; the audio decode itself
//! (core's path) is untouched — the token is observed between its stages.

use std::path::Path;

use suramak_core::pcm::PcmSlice;
use suramak_core::settings::LangMode;
use suramak_core::transcript::{Segment, Source, Transcript};
use suramak_core::{decode, pcm, SuramakError};
use whisper_rs::WhisperContext;

use crate::cancel::CancelToken;
use crate::engine::EngineId;
use crate::whisper::{self, AsrError};
use crate::worker::JobStage;

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
    /// Estimated per-word timing from the engine's own token spans (M15.6);
    /// empty when the engine exposed none (Parakeet/SpeechTranscriber paths).
    pub words: Vec<suramak_core::transcript::WordTiming>,
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
///
/// `prepared` (M16.1) is the queue's decoded-ahead slice: when present it is
/// already 16 kHz mono (the prefetch pool ran the same decode + resample), so
/// the front half only has to re-check the cancel token. Without it the file is
/// decoded here through one streaming path (M26B):
///
/// 1. a 16 kHz mono PCM16 WAV this app wrote takes [`PcmSlice::from_recording_wav`]
///    — its data chunk is mapped, no decoder and no resampler;
/// 2. everything else is decoded and resampled incrementally into
///    `{temp}/suramak_*.pcm32` and mapped by [`decode::decode_to_pcm16k_mono_cancellable`].
///
/// Neither branch holds a full-file sample buffer in this process.
fn prepare_samples(
    input: &Path,
    prepared: Option<PcmSlice>,
    cancel: &CancelToken,
) -> Result<PcmSlice, AsrError> {
    stage_check(cancel)?;
    if let Some(samples) = prepared {
        stage_check(cancel)?;
        return Ok(samples);
    }
    if let Some(samples) = PcmSlice::from_recording_wav(input) {
        stage_check(cancel)?;
        return Ok(samples);
    }
    let out_path = pcm::temp_pcm_path();
    let samples = match decode::decode_to_pcm16k_mono_cancellable(input, &out_path, &|| {
        cancel.is_cancelled()
    }) {
        Ok(samples) => samples,
        // A cancel mid-decode is the job's terminal `Cancelled`, not a decode
        // failure the queue would report as an error.
        Err(SuramakError::Cancelled) => return Err(AsrError::Cancelled),
        Err(err) => return Err(err.into()),
    };
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
    let raw_title = input
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("transcript")
        .to_string();
    // M25B: a `… — Part 2 of 3.wav` transcript keeps the base title; the part
    // label is metadata beside it, never repeated in the title.
    let title = suramak_core::session::base_name(&raw_title);
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

/// M22A: the checkpoint stream handed to a pipeline run.
///
/// `committed` is the durable prefix a resumed job continues after; `from_ms`
/// is the audio boundary it ends at; `on_segment` receives every segment the
/// engine finalizes while it runs (the worker turns those into checkpoints).
pub struct CheckpointRun<'a> {
    pub committed: &'a [Segment],
    pub from_ms: i64,
    pub next_idx: u32,
    pub on_segment: &'a mut dyn FnMut(Segment),
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
///
/// `checkpoint` (M22A) makes the run resumable: the engine is fed only the
/// window after `from_ms`, every new segment is streamed to `on_segment`, and
/// the returned transcript is `committed` followed by the new tail — the same
/// text an uninterrupted run would have produced, with absolute timestamps.
#[allow(clippy::too_many_arguments)]
pub fn transcribe_file(
    ctx: &WhisperContext,
    input: &Path,
    model_path: &Path,
    lang: &LangMode,
    initial_prompt: Option<&str>,
    n_threads: i32,
    plan: &crate::engine::EngineDecision,
    prepared: Option<PcmSlice>,
    cancel: &CancelToken,
    checkpoint: Option<CheckpointRun<'_>>,
    mut on_stage: impl FnMut(JobStage),
    mut on_progress: impl FnMut(u8),
) -> Result<PipelineOutcome, AsrError> {
    on_stage(JobStage::Decoding);
    let samples = prepare_samples(input, prepared, cancel)?;
    on_stage(JobStage::Transcribing);

    let mut noop = |_: Segment| {};
    let (committed, from_ms, next_idx, stream): (Vec<Segment>, i64, u32, &mut dyn FnMut(Segment)) =
        match checkpoint {
            Some(run) => (
                run.committed.to_vec(),
                run.from_ms.max(0),
                run.next_idx,
                run.on_segment,
            ),
            None => (Vec::new(), 0, 0, &mut noop),
        };
    let model_id = model_id_from_path(model_path);

    // Everything is already committed (the crash landed after the last segment):
    // there is no tail to feed the engine, and loading it would be pure waste.
    // 16 samples/ms at 16 kHz; saturate so a corrupt 32-bit bound cannot wrap.
    let start_sample = usize::try_from(from_ms.max(0))
        .unwrap_or(usize::MAX)
        .saturating_mul(16);
    if from_ms > 0 && start_sample >= samples.len() {
        on_progress(100);
        return Ok(PipelineOutcome {
            transcript: transcript_for(input, model_id, lang, committed),
            words: Vec::new(),
            engine_used: plan.engine,
            warnings: plan.warning.iter().cloned().collect(),
        });
    }

    // A prompt is a start-of-file hint. Repeating it on a mid-file resume invites
    // the model to re-emit committed words, which is exactly the duplication the
    // checkpoint contract forbids.
    let prompt = if from_ms > 0 { None } else { initial_prompt };

    // M26C: a slice longer than the 15-minute window target is decoded as
    // silence-aligned macro-windows; at or below it, the single call below is
    // untouched (the byte-identical guarantee).
    if crate::macro_pipeline::applies(samples.len()) {
        let outcome = crate::macro_pipeline::transcribe_macro(
            ctx,
            model_path,
            &samples,
            lang,
            prompt,
            n_threads,
            plan,
            cancel,
            from_ms,
            next_idx,
            &mut on_progress,
            stream,
        )?;
        stage_check(cancel)?;
        let mut segments = committed;
        segments.extend(outcome.segments);
        debug_assert!(
            segments
                .iter()
                .enumerate()
                .all(|(i, seg)| seg.idx as usize == i),
            "a macro-window transcript must have a contiguous idx sequence"
        );
        return Ok(PipelineOutcome {
            transcript: transcript_for(input, model_id, lang, segments),
            words: outcome.words,
            engine_used: outcome.engine_used,
            warnings: outcome.warnings,
        });
    }

    let outcome = whisper::transcribe(
        ctx,
        model_path,
        &samples,
        lang,
        prompt,
        n_threads,
        plan,
        cancel,
        from_ms,
        next_idx,
        on_progress,
        stream,
    )?;
    stage_check(cancel)?;
    let mut segments = committed;
    segments.extend(outcome.segments);
    debug_assert!(
        segments
            .iter()
            .enumerate()
            .all(|(i, seg)| seg.idx as usize == i),
        "a resumed transcript must have a contiguous idx sequence"
    );
    Ok(PipelineOutcome {
        transcript: transcript_for(input, model_id, lang, segments),
        words: outcome.words,
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
#[allow(clippy::too_many_arguments)]
pub fn transcribe_file_parakeet(
    recognizer: &mut crate::parakeet::ParakeetRecognizer,
    input: &Path,
    lang: &LangMode,
    plan: &crate::engine::EngineDecision,
    prepared: Option<PcmSlice>,
    cancel: &CancelToken,
    mut on_stage: impl FnMut(JobStage),
    mut on_progress: impl FnMut(u8),
) -> Result<PipelineOutcome, AsrError> {
    let mut warnings = Vec::new();
    if let Some(warning) = &plan.warning {
        warnings.push(warning.clone());
    }
    on_stage(JobStage::Decoding);
    let samples = prepare_samples(input, prepared, cancel)?;
    on_stage(JobStage::Transcribing);
    on_progress(50);
    let result = recognizer.transcribe(samples.as_slice(), 16_000)?;
    stage_check(cancel)?;
    on_progress(100);
    let audio_ms = samples.len() as i64 * 1000 / 16_000;
    let segments = crate::parakeet::segments_from_result(&result, audio_ms);
    Ok(PipelineOutcome {
        transcript: transcript_for(input, crate::parakeet::MODEL_ID.to_string(), lang, segments),
        // Parakeet reports one timestamp per token but this slice neither
        // measures nor persists word timing for it (see M15.6 report).
        words: Vec::new(),
        engine_used: EngineId::Parakeet,
        warnings,
    })
}

/// Runs the M10B Apple SpeechTranscriber fast pipeline on an audio file
/// (macOS 26+ builds with the `speech-transcriber` feature; the stub refuses
/// honestly everywhere else, so routing never reaches it off-macOS).
///
/// Same decode → resample front half as [`transcribe_file`]: the analyzer itself
/// reads the file (whole-file `transcribe_in_path`), so the prepared samples are
/// what makes the clip duration available for clamping segment times and keep
/// the uniform cancel/progress shape. Two honest progress milestones: 50 when
/// the audio is ready, 100 when the analyzer finished (the crate reports none;
/// the first run may also sit in the one-time authorization prompt and the OS
/// asset download before the analyzer starts). A cancelled job returns
/// [`AsrError::Cancelled`]; any other failure is the caller's cue to degrade to
/// Whisper (the worker does that — never a hard failure).
#[allow(clippy::too_many_arguments)]
pub fn transcribe_file_speech_transcriber(
    input: &Path,
    lang: &LangMode,
    plan: &crate::engine::EngineDecision,
    prepared: Option<PcmSlice>,
    cancel: &CancelToken,
    mut on_stage: impl FnMut(JobStage),
    mut on_progress: impl FnMut(u8),
) -> Result<PipelineOutcome, AsrError> {
    let mut warnings = Vec::new();
    if let Some(warning) = &plan.warning {
        warnings.push(warning.clone());
    }
    on_stage(JobStage::Decoding);
    let samples = prepare_samples(input, prepared, cancel)?;
    on_stage(JobStage::Transcribing);
    on_progress(50);
    let code = match lang {
        LangMode::Fixed(code) => code.clone(),
        // Routing never selects this engine for auto-detect (an Indonesian clip
        // could otherwise reach it); this is the honest refusal if it ever did.
        LangMode::Auto => {
            return Err(AsrError::SpeechTranscriber(
                "SpeechTranscriber needs a forced language (auto-detect stays whisper)".to_string(),
            ))
        }
    };
    let engine = crate::speech_transcriber::SpeechTranscriberEngine::for_language(&code)?;
    let results = engine.transcribe(input)?;
    stage_check(cancel)?;
    on_progress(100);
    let audio_ms = samples.len() as i64 * 1000 / 16_000;
    let segments = crate::speech_transcriber::segments_from_results(&results, audio_ms);
    Ok(PipelineOutcome {
        transcript: transcript_for(
            input,
            crate::speech_transcriber::MODEL_ID.to_string(),
            lang,
            segments,
        ),
        // SpeechTranscriber exposes no per-token timing here.
        words: Vec::new(),
        engine_used: EngineId::SpeechTranscriber,
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
        match prepare_samples(Path::new("/nonexistent/en.wav"), None, &token) {
            Err(AsrError::Cancelled) => {}
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    /// M10B: the SpeechTranscriber path shares the same front half — a cancelled
    /// token returns `Cancelled` before the (here missing) input is touched.
    #[test]
    fn a_cancelled_token_stops_the_speech_transcriber_pipeline_before_decoding() {
        let token = CancelToken::new();
        token.cancel();
        let plan = crate::engine::EngineDecision {
            engine: EngineId::SpeechTranscriber,
            warning: None,
        };
        match transcribe_file_speech_transcriber(
            Path::new("/nonexistent/en.wav"),
            &LangMode::Fixed("en".to_string()),
            &plan,
            None,
            &token,
            |_| {},
            |_| {},
        ) {
            Err(AsrError::Cancelled) => {}
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    /// The committed English fixture (decodable on every platform).
    fn en_fixture() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/audio/en.wav")
    }

    /// M10B (the Linux surface): auto-detect is refused by the engine itself —
    /// routing never plans SpeechTranscriber for it, and the refusal is the
    /// honest reason (a None language could be Indonesian).
    #[test]
    fn speech_transcriber_refuses_auto_detect_after_the_shared_front_half() {
        let plan = crate::engine::EngineDecision {
            engine: EngineId::SpeechTranscriber,
            warning: None,
        };
        let err = transcribe_file_speech_transcriber(
            &en_fixture(),
            &LangMode::Auto,
            &plan,
            None,
            &CancelToken::new(),
            |_| {},
            |_| {},
        )
        .expect_err("auto-detect must be refused");
        match err {
            AsrError::SpeechTranscriber(message) => {
                assert!(message.contains("forced language"), "unexpected: {message}")
            }
            other => panic!("expected SpeechTranscriber, got {other:?}"),
        }
    }

    /// M10B (the stub path, Linux): with a real decodable file the stub engine
    /// refuses with `SpeechTranscriberUnsupported` after the shared front half —
    /// which the worker turns into the Whisper degradation on a Mac. Skipped on
    /// a macOS + feature build, where the real engine would run instead
    /// (owner/CI territory).
    #[test]
    fn the_stub_speech_transcriber_pipeline_refuses_after_the_shared_front_half() {
        if cfg!(all(feature = "speech-transcriber", target_os = "macos")) {
            return;
        }
        let plan = crate::engine::EngineDecision {
            engine: EngineId::SpeechTranscriber,
            warning: Some("fast engine requested but x — using cpu".to_string()),
        };
        let err = transcribe_file_speech_transcriber(
            &en_fixture(),
            &LangMode::Fixed("en".to_string()),
            &plan,
            None,
            &CancelToken::new(),
            |_| {},
            |_| {},
        )
        .expect_err("the stub must refuse");
        assert!(
            matches!(err, AsrError::SpeechTranscriberUnsupported),
            "unexpected: {err:?}"
        );
    }
}
