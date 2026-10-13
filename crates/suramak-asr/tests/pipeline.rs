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
//! (`~/.local/share/Suramak/models/ggml-tiny.bin`). A missing model skips the test with
//! a message unless `SURAMAK_REQUIRE_MODEL=1` is set, in which case it fails.

use std::path::{Path, PathBuf};

use suramak_asr::pipeline::{model_id_from_path, parse_lang, transcribe_file};
use suramak_asr::whisper::{self, ContextCache};
use suramak_asr::worker::{Job, Worker, WorkerEvent};
use suramak_asr::AsrError;
use suramak_core::settings::LangMode;

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
fn context_cache_reports_missing_model() {
    let mut cache = ContextCache::new();
    assert!(cache.is_empty());
    // `WhisperContext` is not `Debug`, so match instead of `expect_err`.
    match cache.get_or_load(Path::new("/nonexistent/dir/ggml-tiny.bin")) {
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
    let mut cache = ContextCache::new();
    let ctx = cache.get_or_load(&model_path).expect("load tiny model");

    let transcript = transcribe_file(
        ctx,
        &fixture("en.wav"),
        &model_path,
        &LangMode::Fixed("en".to_string()),
        None,
        whisper::default_threads(),
        |_| {},
    )
    .expect("transcribe en.wav");

    assert!(
        !transcript.segments.is_empty(),
        "expected at least one segment"
    );
    let joined = transcript
        .segments
        .iter()
        .map(|s| s.text.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("fox"),
        "expected 'fox' in transcript, got: {joined}"
    );
}

#[test]
#[ignore = "release-only e2e: debug whisper is ~50x too slow (run with --release -- --ignored)"]
fn e2e_id_fixture_has_non_empty_segment() {
    let Some(model_path) = tiny_model_or_skip() else {
        return;
    };
    let mut cache = ContextCache::new();
    let ctx = cache.get_or_load(&model_path).expect("load tiny model");

    let transcript = transcribe_file(
        ctx,
        &fixture("id.wav"),
        &model_path,
        &LangMode::Fixed("id".to_string()),
        None,
        whisper::default_threads(),
        |_| {},
    )
    .expect("transcribe id.wav");

    assert!(
        !transcript.segments.is_empty(),
        "expected at least one segment"
    );
    assert!(
        transcript
            .segments
            .iter()
            .any(|s| !s.text.trim().is_empty()),
        "expected at least one non-empty segment, got: {:?}",
        transcript.segments
    );
}
