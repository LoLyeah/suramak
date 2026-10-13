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
//! `e2e_coreml_encoder_engaged_macos` additionally needs macOS + the tiny Core ML
//! encoder package; `SURAMAK_REQUIRE_COREML=1` turns its skips into failures (used by
//! the macOS CI job).

use std::path::{Path, PathBuf};

use suramak_asr::engine::{EngineAvailability, EngineId};
use suramak_asr::pipeline::{model_id_from_path, parse_lang, transcribe_file};
use suramak_asr::whisper::{self, ContextCache};
use suramak_asr::worker::{Job, Worker, WorkerEvent};
use suramak_asr::{resolve_for, AsrError};
use suramak_core::settings::{EngineSetting, LangMode};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/audio")
        .join(name)
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
    let avail = EngineAvailability::current(missing);
    assert!(!avail.encoder_installed);
    if !cfg!(target_os = "macos") {
        assert_eq!(
            resolve_for(missing, EngineSetting::Auto, None).engine,
            EngineId::Cpu
        );
        assert_eq!(
            resolve_for(missing, EngineSetting::CoreML, None).engine,
            EngineId::Cpu
        );
        assert!(resolve_for(missing, EngineSetting::CoreML, None)
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
    let Some(model_path) = tiny_model_or_skip() else {
        return;
    };
    let plan = resolve_for(&model_path, EngineSetting::Auto, Some("en"));
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
    let Some(model_path) = tiny_model_or_skip() else {
        return;
    };
    let plan = resolve_for(&model_path, EngineSetting::Auto, Some("id"));
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

/// macOS-only e2e: with `--engine coreml` the Core ML encoder must be the engine that
/// actually ran (whisper.cpp loaded the `.mlmodelc` package next to the model file).
///
/// Skips with a message on non-macOS builds and when the encoder package is missing;
/// `SURAMAK_REQUIRE_COREML=1` turns those skips into failures (macOS CI).
#[test]
#[ignore = "macOS-only: needs Core ML + the tiny encoder package (run with --release -- --ignored on a Mac)"]
fn e2e_coreml_encoder_engaged_macos() {
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

    let plan = resolve_for(&model_path, EngineSetting::CoreML, Some("en"));
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
