//! Tests for `suramak-asr`.
//!
//! The whisper end-to-end tests are `#[ignore]`d and release-only: the debug-profile
//! whisper.cpp build is ~50× slower. Run them with:
//!
//! ```text
//! SURAMAK_REQUIRE_MODEL=1 cargo test -p suramak-asr --release -- --ignored
//! ```
//!
//! They use the committed fixtures and the tiny model from the standard data dir
//! (`~/.local/share/suramak/models/ggml-tiny.bin`). A missing model skips the test with
//! a message unless `SURAMAK_REQUIRE_MODEL=1` is set, in which case it fails.
//!
//! `e2e_video_fixture_matches_the_source_mp3_words` compares the video fixture against
//! the committed `fixtures/audio/en.wav` (same sentences; always present) and — when the
//! gitignored TTS sample `build/share/suramak-sample-en.mp3` is present — against that
//! source mp3 too (the file the video fixtures were built from). A missing sample skips
//! the second comparison with a message; `SURAMAK_REQUIRE_SAMPLE=1` turns that skip into
//! a failure (used for the M5 acceptance run).
//!
//! `e2e_coreml_encoder_engaged_macos` additionally needs macOS + the tiny Core ML
//! encoder package; `SURAMAK_REQUIRE_COREML=1` turns its skips into failures (used by
//! the macOS CI job).

use std::path::{Path, PathBuf};

use suramak_asr::engine::{EngineAvailability, EngineId};
use suramak_asr::live::{LiveConfig, LiveSession};
use suramak_asr::pipeline::{model_id_from_path, parse_lang, transcribe_file};
use suramak_asr::whisper::{self, ContextCache};
use suramak_asr::worker::{Job, Worker, WorkerEvent};
use suramak_asr::{resolve_for, AsrError, CancelToken};
use suramak_core::settings::{EngineSetting, LangMode};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/audio")
        .join(name)
}

/// Serializes the whisper e2e tests in this file.
///
/// Each e2e test runs a full whisper session (all cores); running two sessions
/// concurrently on a small machine corrupts the process heap — observed as
/// glibc's `malloc(): unaligned tcache chunk detected` aborting ~1 in 4 runs of
/// this file on the 2-core dev VPS, on the pre-M5 test set as well. Serializing
/// makes the suite deterministic (and faster there: ~11 s vs ~22 s). Poisoning
/// is ignored so one failing test never cascades into the others.
static E2E_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn e2e_serial() -> std::sync::MutexGuard<'static, ()> {
    E2E_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// An M5 video fixture (`fixtures/video/`): real speech + a testsrc video stream.
fn video_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/video")
        .join(name)
}

/// The gitignored TTS sample the video fixtures were built from
/// (`build/share/suramak-sample-en.mp3`) — the mp3 baseline for the video e2e.
fn source_sample_mp3() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../build/share/suramak-sample-en.mp3")
}

/// The words of a transcript, normalized for comparison: lowercased, single spaces.
fn words(transcript: &suramak_core::transcript::Transcript) -> String {
    segment_words(&transcript.segments)
}

/// The words of a segment list, normalized the same way (M7 live e2e).
fn segment_words(segments: &[suramak_core::transcript::Segment]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Resolves the tiny model from the standard data dir; `None` (skip) when absent,
/// unless `SURAMAK_REQUIRE_MODEL=1` which turns the skip into a failure.
fn tiny_model_or_skip() -> Option<PathBuf> {
    let path = suramak_core::paths::models_dir()
        .ok()?
        .join("ggml-tiny.bin");
    if path.exists() {
        return Some(path);
    }
    if std::env::var("SURAMAK_REQUIRE_MODEL").as_deref() == Ok("1") {
        panic!(
            "SURAMAK_REQUIRE_MODEL=1 but the tiny model is missing at {} \
             (run `suramak-cli models download tiny`)",
            path.display()
        );
    }
    eprintln!(
        "skipping whisper e2e test: tiny model not found at {} \
         (set SURAMAK_REQUIRE_MODEL=1 to make this a hard failure)",
        path.display()
    );
    None
}

#[test]
fn parse_lang_auto_and_fixed() {
    assert_eq!(parse_lang("auto"), LangMode::Auto);
    assert_eq!(parse_lang("AUTO"), LangMode::Auto);
    assert_eq!(parse_lang("en"), LangMode::Fixed("en".to_string()));
    assert_eq!(parse_lang("id"), LangMode::Fixed("id".to_string()));
}

#[test]
fn model_id_strips_ggml_prefix_and_extension() {
    assert_eq!(model_id_from_path(Path::new("/x/ggml-tiny.bin")), "tiny");
    assert_eq!(
        model_id_from_path(Path::new("/x/ggml-large-v3-turbo-q5_0.bin")),
        "large-v3-turbo-q5_0"
    );
    assert_eq!(model_id_from_path(Path::new("custom.bin")), "custom");
}

#[test]
fn system_info_names_the_linked_whisper_build() {
    // whisper.cpp reports its own compiled backends; doctor prints this line and the
    // engine availability is derived from it.
    let info = whisper::system_info();
    assert!(info.contains("WHISPER :"), "unexpected system info: {info}");
    assert!(
        info.contains("COREML = "),
        "Core ML flag missing from: {info}"
    );
    // On Linux CI the build has neither tier; on macOS it has both.
    if cfg!(target_os = "linux") {
        assert!(!whisper::coreml_compiled(), "linux builds have no Core ML");
    }
    if cfg!(target_os = "macos") {
        assert!(
            whisper::metal_compiled(),
            "macOS builds enable the Metal feature: {info}"
        );
    }
}

#[test]
fn engine_resolution_uses_availability_not_the_platform_guess() {
    // A path with no encoder package installed: Core ML is only ever planned when the
    // encoder directory exists, so on a non-macOS build this must resolve to CPU.
    let missing = Path::new("/nonexistent/models/ggml-tiny.bin");
    let avail = EngineAvailability::current(missing, None);
    assert!(!avail.encoder_installed);
    if !cfg!(target_os = "macos") {
        assert_eq!(
            resolve_for(missing, EngineSetting::Auto, false, None).engine,
            EngineId::Cpu
        );
        assert_eq!(
            resolve_for(missing, EngineSetting::CoreML, false, None).engine,
            EngineId::Cpu
        );
        assert!(resolve_for(missing, EngineSetting::CoreML, false, None)
            .warning
            .is_some());
    }
}

#[test]
fn context_cache_reports_missing_model() {
    let mut cache = ContextCache::new();
    assert!(cache.is_empty());
    // `WhisperContext` is not `Debug`, so match instead of `expect_err`.
    match cache.get_or_load(Path::new("/nonexistent/dir/ggml-tiny.bin"), false) {
        Err(AsrError::ModelFileMissing(_)) => {}
        Err(other) => panic!("unexpected error variant: {other:?}"),
        Ok(_) => panic!("expected an error for a missing model"),
    }
    assert!(cache.is_empty(), "failed loads must not populate the cache");
}

#[test]
fn worker_reports_missing_model_as_error_event() {
    let worker = Worker::spawn();
    worker
        .submit(Job {
            id: 7,
            input: fixture("en.wav"),
            model_path: PathBuf::from("/nonexistent/dir/ggml-tiny.bin"),
            lang: LangMode::Fixed("en".to_string()),
            initial_prompt: None,
            engine: EngineSetting::Auto,
            fast_engine: false,
            cancel: CancelToken::new(),
        })
        .expect("submit job");

    let mut saw_progress = false;
    loop {
        match worker.recv().expect("worker channel closed early") {
            WorkerEvent::Progress { job_id, pct } => {
                assert_eq!(job_id, 7);
                assert_eq!(pct, 0);
                saw_progress = true;
            }
            WorkerEvent::Error { job_id, message } => {
                assert_eq!(job_id, 7);
                assert!(
                    message.contains("model file not found"),
                    "unexpected message: {message}"
                );
                break;
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
    assert!(saw_progress, "worker should emit an initial Progress event");
}

#[test]
#[ignore = "release-only e2e: debug whisper is ~50x too slow (run with --release -- --ignored)"]
fn e2e_en_fixture_contains_fox() {
    let _serial = e2e_serial();
    let Some(model_path) = tiny_model_or_skip() else {
        return;
    };
    let plan = resolve_for(&model_path, EngineSetting::Auto, false, Some("en"));
    let mut cache = ContextCache::new();
    let ctx = cache
        .get_or_load(&model_path, plan.engine.use_gpu())
        .expect("load tiny model");

    let outcome = transcribe_file(
        ctx,
        &fixture("en.wav"),
        &model_path,
        &LangMode::Fixed("en".to_string()),
        None,
        whisper::default_threads(),
        &plan,
        &CancelToken::new(),
        |_| {},
    )
    .expect("transcribe en.wav");

    assert!(
        !outcome.transcript.segments.is_empty(),
        "expected at least one segment"
    );
    let joined = outcome
        .transcript
        .segments
        .iter()
        .map(|s| s.text.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("fox"),
        "expected 'fox' in transcript, got: {joined}"
    );
    if !cfg!(target_os = "macos") {
        // Non-macOS has exactly one tier; the engine that ran must say so.
        assert_eq!(outcome.engine_used, EngineId::Cpu);
    }
    assert_eq!(outcome.engine_used, plan.engine, "{:?}", outcome.warnings);
}

#[test]
#[ignore = "release-only e2e: debug whisper is ~50x too slow (run with --release -- --ignored)"]
fn e2e_id_fixture_has_non_empty_segment() {
    let _serial = e2e_serial();
    let Some(model_path) = tiny_model_or_skip() else {
        return;
    };
    let plan = resolve_for(&model_path, EngineSetting::Auto, false, Some("id"));
    let mut cache = ContextCache::new();
    let ctx = cache
        .get_or_load(&model_path, plan.engine.use_gpu())
        .expect("load tiny model");

    let outcome = transcribe_file(
        ctx,
        &fixture("id.wav"),
        &model_path,
        &LangMode::Fixed("id".to_string()),
        None,
        whisper::default_threads(),
        &plan,
        &CancelToken::new(),
        |_| {},
    )
    .expect("transcribe id.wav");

    assert!(
        !outcome.transcript.segments.is_empty(),
        "expected at least one segment"
    );
    assert!(
        outcome
            .transcript
            .segments
            .iter()
            .any(|s| !s.text.trim().is_empty()),
        "expected at least one non-empty segment, got: {:?}",
        outcome.transcript.segments
    );
}

/// M5 end-to-end: the mp4 fixture (speech + a testsrc video stream) must
/// transcribe to the same words as the source speech — the video rides the
/// identical pipeline (decode the first audio track → resample → whisper).
///
/// Baselines: the committed `fixtures/audio/en.wav` (same sentences, always
/// present — this is what runs in CI) and, when it exists, the gitignored TTS
/// sample `build/share/suramak-sample-en.mp3` the video fixtures were built
/// from. A missing sample skips the second comparison with a message;
/// `SURAMAK_REQUIRE_SAMPLE=1` turns that skip into a failure (M5 acceptance).
#[test]
#[ignore = "release-only e2e: debug whisper is ~50x too slow (run with --release -- --ignored)"]
fn e2e_video_fixture_matches_the_source_mp3_words() {
    let _serial = e2e_serial();
    let Some(model_path) = tiny_model_or_skip() else {
        return;
    };

    let plan = resolve_for(&model_path, EngineSetting::Auto, false, Some("en"));
    let mut cache = ContextCache::new();
    let ctx = cache
        .get_or_load(&model_path, plan.engine.use_gpu())
        .expect("load tiny model");

    let run = |path: &Path| {
        transcribe_file(
            ctx,
            path,
            &model_path,
            &LangMode::Fixed("en".to_string()),
            None,
            whisper::default_threads(),
            &plan,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap_or_else(|err| panic!("transcribe {}: {err}", path.display()))
    };

    let video = run(&video_fixture("sample-en.mp4"));
    eprintln!("video (sample-en.mp4): {}", words(&video.transcript));
    assert!(
        !video.transcript.segments.is_empty(),
        "expected at least one segment from the video"
    );

    // The committed fixture carries the same sentences; it is always available.
    let committed = run(&fixture("en.wav"));
    eprintln!("committed en.wav: {}", words(&committed.transcript));
    assert_eq!(
        words(&video.transcript),
        words(&committed.transcript),
        "the video's audio track must transcribe to the same words as the committed en.wav"
    );

    // The workorder's baseline: the source mp3 the fixtures were built from.
    let source = source_sample_mp3();
    if !source.is_file() {
        if std::env::var("SURAMAK_REQUIRE_SAMPLE").as_deref() == Ok("1") {
            panic!(
                "SURAMAK_REQUIRE_SAMPLE=1 but the source sample is missing at {} \
                 (build/share is gitignored)",
                source.display()
            );
        }
        eprintln!(
            "skipping the source-mp3 comparison: sample not found at {} \
             (set SURAMAK_REQUIRE_SAMPLE=1 to make this a hard failure)",
            source.display()
        );
        return;
    }
    let baseline = run(&source);
    eprintln!(
        "source mp3 (suramak-sample-en.mp3): {}",
        words(&baseline.transcript)
    );
    assert_eq!(
        words(&video.transcript),
        words(&baseline.transcript),
        "the video's audio track must transcribe to the same words as the source mp3"
    );
    if !cfg!(target_os = "macos") {
        assert_eq!(video.engine_used, EngineId::Cpu);
    }
}

/// M7 end-to-end (Linux, tiny model): the fixture is fed through the **live
/// scheduler** on a simulated capture clock — every window decodes through the
/// same whisper path the session uses — and provisional segments appear; the
/// full pass then runs and its transcript **replaces** the provisional one.
///
/// The final pass is pinned byte-for-byte across the live session: it runs
/// once before the live machinery (the baseline today's flow produces) and
/// once after, and every segment (idx, timestamps, text) must match — the live
/// machinery shares no state with the final pipeline.
#[test]
#[ignore = "release-only e2e: debug whisper is ~50x too slow (run with --release -- --ignored)"]
fn e2e_live_partials_then_the_final_pass_replaces_them() {
    let _serial = e2e_serial();
    let Some(model_path) = tiny_model_or_skip() else {
        return;
    };

    // The fixture as the capture layer would deliver it: 16 kHz mono f32.
    let buffer = suramak_core::decode::decode_file(&fixture("en.wav")).expect("decode en.wav");
    let samples = suramak_core::resample::to_16khz_mono(&buffer).expect("resample");
    let lang = LangMode::Fixed("en".to_string());
    let plan = resolve_for(&model_path, EngineSetting::Auto, false, Some("en"));
    let mut cache = ContextCache::new();
    let ctx = cache
        .get_or_load(&model_path, plan.engine.use_gpu())
        .expect("load tiny model");

    // The final pass, run BEFORE any live machinery — the baseline this input
    // always produces (today's record-then-transcribe artifact).
    let baseline = transcribe_file(
        ctx,
        &fixture("en.wav"),
        &model_path,
        &lang,
        None,
        whisper::default_threads(),
        &plan,
        &CancelToken::new(),
        |_| {},
    )
    .expect("baseline final pass");
    eprintln!(
        "[live] final pass (baseline): {}",
        words(&baseline.transcript)
    );

    // ---- the live session on a simulated capture clock ----------------------
    let config = LiveConfig::default();
    let mut live = LiveSession::new(config.clone()).expect("live config");
    let tick = (config.sample_rate / 10) as usize; // 100 ms of audio per tick
    let mut decoded_windows = 0usize;
    let mut index = 0usize;
    while index < samples.len() {
        let end = (index + tick).min(samples.len());
        // The simulated decoder is synchronous: it is never busy when a window
        // closes (the skip-and-merge path is unit-tested; here every window
        // decodes, exactly as a fast machine behaves).
        for window in live.push_audio(&samples[index..end], false) {
            let outcome = whisper::transcribe(
                ctx,
                &model_path,
                &window.samples,
                &lang,
                None,
                whisper::default_threads(),
                &plan,
                &CancelToken::new(),
                |_| {},
            )
            .expect("live window decode");
            eprintln!(
                "[live] window {} [{:.2}–{:.2} s]: {}",
                window.seq,
                window.start_ms(config.sample_rate) as f64 / 1000.0,
                window.end_ms(config.sample_rate) as f64 / 1000.0,
                segment_words(&outcome.segments)
            );
            decoded_windows += 1;
            live.ingest_decoded(&window, &outcome.segments);
        }
        index = end;
    }
    assert!(
        decoded_windows >= 2,
        "the 9 s fixture must close at least two windows"
    );
    eprintln!(
        "[live] provisional ({} window(s), {} segment(s), {} skipped): {}",
        decoded_windows,
        live.segments().len(),
        live.skipped_windows(),
        live.text()
    );
    assert!(
        !live.segments().is_empty(),
        "provisional segments must appear while the audio streams"
    );
    assert!(
        live.text().to_lowercase().contains("fox"),
        "the provisional words must include the fixture's sentence: {}",
        live.text()
    );

    // ---- the final pass replaces the provisional text ------------------------
    let final_outcome = transcribe_file(
        ctx,
        &fixture("en.wav"),
        &model_path,
        &lang,
        None,
        whisper::default_threads(),
        &plan,
        &CancelToken::new(),
        |_| {},
    )
    .expect("final pass after the live session");
    eprintln!(
        "[live] final pass (after the live session): {}",
        words(&final_outcome.transcript)
    );
    assert!(words(&final_outcome.transcript).contains("fox"));

    // Byte-identical pin: the same input produces the same final segments
    // whether or not the live machinery ran (idx, timestamps and text all
    // compared — the provisional list is display-only and discarded).
    assert_eq!(
        final_outcome.transcript.segments.len(),
        baseline.transcript.segments.len(),
        "the final pass must produce the same segment count across a live session"
    );
    for (after, before) in final_outcome
        .transcript
        .segments
        .iter()
        .zip(baseline.transcript.segments.iter())
    {
        assert_eq!(
            after, before,
            "the final pass must be byte-identical across a live session"
        );
    }
    if !cfg!(target_os = "macos") {
        assert_eq!(final_outcome.engine_used, EngineId::Cpu);
    }
}

/// macOS-only e2e: with `--engine coreml` the Core ML encoder must be the engine that
/// actually ran (whisper.cpp loaded the `.mlmodelc` package next to the model file).
///
/// Skips with a message on non-macOS builds and when the encoder package is missing;
/// `SURAMAK_REQUIRE_COREML=1` turns those skips into failures (macOS CI).
#[test]
#[ignore = "macOS-only: needs Core ML + the tiny encoder package (run with --release -- --ignored on a Mac)"]
fn e2e_coreml_encoder_engaged_macos() {
    let _serial = e2e_serial();
    let require = std::env::var("SURAMAK_REQUIRE_COREML").as_deref() == Ok("1");
    if !cfg!(target_os = "macos") {
        if require {
            panic!("SURAMAK_REQUIRE_COREML=1 but this is not macOS — Core ML is macOS-only");
        }
        eprintln!("skipping Core ML e2e: Core ML is macOS-only (this build is not macOS)");
        return;
    }
    let Some(model_path) = tiny_model_or_skip() else {
        return;
    };
    let encoder = suramak_core::models::encoder_dir_for(&model_path);
    if !suramak_core::models::encoder_dir_ready(&encoder) {
        if require {
            panic!(
                "SURAMAK_REQUIRE_COREML=1 but the encoder package is missing at {} \
                 (run `suramak-cli models download tiny --encoder`)",
                encoder.display()
            );
        }
        eprintln!(
            "skipping Core ML e2e: encoder package not installed at {} \
             (run `suramak-cli models download tiny --encoder`)",
            encoder.display()
        );
        return;
    }
    assert!(
        whisper::coreml_compiled(),
        "macOS builds must compile the `coreml` feature: {}",
        whisper::system_info()
    );

    let plan = resolve_for(&model_path, EngineSetting::CoreML, false, Some("en"));
    assert_eq!(
        plan.engine,
        EngineId::CoreML,
        "Core ML was requested but resolved to {:?} ({:?})",
        plan.engine,
        plan.warning
    );
    let mut cache = ContextCache::new();
    let ctx = cache
        .get_or_load(&model_path, plan.engine.use_gpu())
        .expect("load tiny model");

    let outcome = transcribe_file(
        ctx,
        &fixture("en.wav"),
        &model_path,
        &LangMode::Fixed("en".to_string()),
        None,
        whisper::default_threads(),
        &plan,
        &CancelToken::new(),
        |_| {},
    )
    .expect("transcribe en.wav with Core ML");

    assert_eq!(
        outcome.engine_used,
        EngineId::CoreML,
        "the Core ML encoder did not engage: {:?}",
        outcome.warnings
    );
    assert!(!outcome.transcript.segments.is_empty());
}

/// M9e (P2-2) end-to-end: a cancel that lands while a whisper run is in
/// flight stops the job at a chunk boundary (the abort callback) — the run
/// returns `Cancelled` with no segments — and the cached whisper context (the
/// engine) is **not corrupted**: the same context transcribes the fixture
/// normally straight after.
///
/// The token is set from the progress callback's first tick: the cancel then
/// races the decoding loop, which is exactly the production shape (the UI sets
/// the token while the worker decodes).
#[test]
#[ignore = "release-only e2e: debug whisper is ~50x too slow (run with --release -- --ignored)"]
fn e2e_cancel_mid_run_stops_cleanly_and_the_engine_stays_healthy() {
    let _serial = e2e_serial();
    let Some(model_path) = tiny_model_or_skip() else {
        return;
    };
    let plan = resolve_for(&model_path, EngineSetting::Auto, false, Some("en"));
    let mut cache = ContextCache::new();
    let ctx = cache
        .get_or_load(&model_path, plan.engine.use_gpu())
        .expect("load tiny model");

    let token = CancelToken::new();
    let cancel_from_progress = token.clone();
    let result = transcribe_file(
        ctx,
        &fixture("en.wav"),
        &model_path,
        &LangMode::Fixed("en".to_string()),
        None,
        whisper::default_threads(),
        &plan,
        &token,
        move |_pct| cancel_from_progress.cancel(),
    );
    match result {
        Err(AsrError::Cancelled) => {}
        Err(other) => panic!("expected Cancelled, got an error: {other:?}"),
        Ok(outcome) => panic!(
            "expected Cancelled; the run finished first with {} segment(s) — \
             a cancelled job must never produce segments",
            outcome.transcript.segments.len()
        ),
    }

    // The engine is healthy: the same context runs the fixture again, whole.
    let outcome = transcribe_file(
        ctx,
        &fixture("en.wav"),
        &model_path,
        &LangMode::Fixed("en".to_string()),
        None,
        whisper::default_threads(),
        &plan,
        &CancelToken::new(),
        |_| {},
    )
    .expect("the context must still transcribe after a cancelled run");
    assert!(
        words(&outcome.transcript).contains("fox"),
        "unexpected transcript after a cancelled run: {}",
        words(&outcome.transcript)
    );
}
