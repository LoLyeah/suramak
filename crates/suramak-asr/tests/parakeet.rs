//! M10A end-to-end: the **Parakeet fast engine** really transcribes the committed
//! English fixture with the real downloaded model (sherpa-onnx, CPU provider).
//!
//! This file only exists when the `parakeet` feature is on. The heavy tests are
//! `#[ignore]`d and release-only (the model is 670 MB of ONNX artifacts; a debug
//! build of the native sherpa-onnx/onnxruntime stack is not what we ship). Run:
//!
//! ```text
//! LD_LIBRARY_PATH=target/release/deps \
//!   SURAMAK_REQUIRE_PARAKEET=1 \
//!   cargo test -p suramak-asr --release --features parakeet --test parakeet -- --ignored --nocapture
//! ```
//!
//! (`LD_LIBRARY_PATH` points at the directory the sherpa-rs build script copies
//! the shared `libsherpa-onnx-c-api.so` / `libonnxruntime.so` into — see
//! docs/ENGINES.md.)
//!
//! The test **downloads the model** (per-file sha256-verified, the same path the
//! CLI uses) when it is not installed. Without network and without an installed
//! model it skips with a message unless `SURAMAK_REQUIRE_PARAKEET=1`, which turns
//! the skip into a failure (used for the M10A acceptance run).

#![cfg(feature = "parakeet")]

use std::path::PathBuf;
use std::time::Instant;

use suramak_asr::engine::{resolve_for, EngineId};
use suramak_asr::parakeet::{self, ParakeetRecognizer};
use suramak_asr::pipeline::transcribe_file_parakeet;
use suramak_asr::whisper::{self, ContextCache};
use suramak_asr::CancelToken;
use suramak_core::models;
use suramak_core::settings::{EngineSetting, LangMode};
use suramak_core::{download, paths};

/// The known words of `fixtures/audio/en.wav` (fixtures/audio/manifest.tsv).
const EXPECTED: &str = "The quick brown fox jumps over the lazy dog. \
                        Suramak keeps every recording on this machine and never sends \
                        audio to the cloud.";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/audio")
        .join(name)
}

/// Normalizes text for comparison: lowercased, punctuation dropped, single spaces.
fn normalized(text: &str) -> String {
    let cleaned: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Fraction of the expected words that appear (in order) in `got` — a simple
/// approximate-match score for ASR output.
fn in_order_overlap(expected: &str, got: &str) -> f64 {
    let expected_norm = normalized(expected);
    let got_norm = normalized(got);
    let expected: Vec<&str> = expected_norm.split_whitespace().collect();
    let got: Vec<&str> = got_norm.split_whitespace().collect();
    if expected.is_empty() {
        return 1.0;
    }
    let mut matched = 0usize;
    let mut cursor = 0usize;
    for word in &expected {
        if let Some(pos) = got[cursor..].iter().position(|w| w == word) {
            matched += 1;
            cursor += pos + 1;
        }
    }
    matched as f64 / expected.len() as f64
}

/// Ensures the Parakeet model is installed, downloading it (the real pinned
/// per-file path) when missing. `None` = skip (offline, no model), unless
/// `SURAMAK_REQUIRE_PARAKEET=1` which turns the skip into a failure.
fn ensure_model_or_skip() -> Option<PathBuf> {
    let models_dir = paths::models_dir().ok()?;
    let dir = models::parakeet_dir(&models_dir);
    if models::parakeet_installed(&models_dir) {
        return Some(dir);
    }
    eprintln!(
        "parakeet model not installed — downloading ({} files, ~670 MB) into {}",
        models::PARAKEET.files.len(),
        dir.display()
    );
    let info = &models::PARAKEET;
    let pins: Vec<(&str, u64, &str)> = info
        .files
        .iter()
        .map(|f| (f.name, f.size_bytes, f.sha256))
        .collect();
    let result = download::download_files(
        &dir,
        &pins,
        |name| models::parakeet_file_url(info, name),
        models::PARAKEET_REQUIRED_FILES,
    );
    match result {
        Ok(()) if models::parakeet_installed(&models_dir) => Some(dir),
        other => {
            if std::env::var("SURAMAK_REQUIRE_PARAKEET").as_deref() == Ok("1") {
                panic!(
                    "SURAMAK_REQUIRE_PARAKEET=1 but the Parakeet model could not be installed: {other:?}"
                );
            }
            eprintln!(
                "skipping parakeet e2e test: the model could not be installed ({other:?}); \
                 set SURAMAK_REQUIRE_PARAKEET=1 to make this a hard failure"
            );
            None
        }
    }
}

/// The tiny whisper model from the standard data dir (the comparison baseline).
fn tiny_model() -> Option<PathBuf> {
    let path = paths::models_dir().ok()?.join("ggml-tiny.bin");
    path.is_file().then_some(path)
}

/// The routing decision the worker would make for `lang` with the fast engine on,
/// under this build and this model install.
#[test]
#[ignore = "release-only e2e: needs the 670 MB model (run with --release --features parakeet -- --ignored)"]
fn e2e_routing_selects_parakeet_for_english_and_never_for_indonesian() {
    let Some(model_dir) = ensure_model_or_skip() else {
        return;
    };
    let whisper_model = paths::models_dir().unwrap().join("ggml-tiny.bin");

    let en = resolve_for(&whisper_model, EngineSetting::Auto, true, Some("en"));
    assert_eq!(
        en.engine,
        EngineId::Parakeet,
        "en must route to Parakeet (installed at {}, warning {:?})",
        model_dir.display(),
        en.warning
    );
    assert_eq!(en.warning, None);

    // Indonesian never — even with the fast engine on and the model installed.
    for lang in ["id", "ID", "id-ID"] {
        let decision = resolve_for(&whisper_model, EngineSetting::Auto, true, Some(lang));
        assert_ne!(decision.engine, EngineId::Parakeet, "{lang}");
    }
    // Auto-detect stays whisper; the fast engine off keeps today's routing.
    assert_ne!(
        resolve_for(&whisper_model, EngineSetting::Auto, true, None).engine,
        EngineId::Parakeet
    );
    assert_ne!(
        resolve_for(&whisper_model, EngineSetting::Auto, false, Some("en")).engine,
        EngineId::Parakeet
    );
}

/// The real thing: download → load → transcribe `fixtures/audio/en.wav` through
/// the pipeline, assert the approximate text match, and print wall times next to
/// whisper tiny on the same file.
#[test]
#[ignore = "release-only e2e: needs the 670 MB model (run with --release --features parakeet -- --ignored)"]
fn e2e_parakeet_transcribes_the_english_fixture() {
    let Some(model_dir) = ensure_model_or_skip() else {
        return;
    };
    let wav = fixture("en.wav");

    // Load (first run pays the model-load cost).
    let load_start = Instant::now();
    let mut recognizer =
        ParakeetRecognizer::load(&model_dir, whisper::default_threads(), parakeet::provider())
            .expect("load the Parakeet model");
    let load_time = load_start.elapsed();

    // Transcribe through the pipeline (decode → resample → engine → transcript).
    let plan = suramak_asr::engine::EngineDecision {
        engine: EngineId::Parakeet,
        warning: None,
    };
    let decode_start = Instant::now();
    let outcome = transcribe_file_parakeet(
        &mut recognizer,
        &wav,
        &LangMode::Fixed("en".to_string()),
        &plan,
        &CancelToken::new(),
        |_| {},
    )
    .expect("transcribe en.wav with Parakeet");
    let decode_time = decode_start.elapsed();

    assert_eq!(outcome.engine_used, EngineId::Parakeet);
    assert_eq!(outcome.transcript.model, "parakeet");
    assert!(
        !outcome.transcript.segments.is_empty(),
        "expected at least one segment"
    );
    let text = outcome
        .transcript
        .segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    // The approximate match: the fixture's two known sentences (case/punctuation
    // normalized; the brand word may be misheard — that is what the overlap score
    // is for).
    let overlap = in_order_overlap(EXPECTED, &text);
    let norm = normalized(&text);
    assert!(
        norm.contains("the quick brown fox jumps over the lazy dog"),
        "expected the first known sentence, got: {text}"
    );
    assert!(
        norm.contains("keeps every recording on this machine and never sends audio to the cloud"),
        "expected the second known sentence, got: {text}"
    );
    assert!(
        overlap >= 0.8,
        "word overlap {overlap:.2} below 0.8 — transcript: {text}"
    );

    let audio_s = 9.0_f64; // fixtures/audio/en.wav is exactly 9 s @ 16 kHz mono
    eprintln!(
        "parakeet: load {load_time:.2?} | pipeline {decode_time:.2?} for {audio_s:.2} s of audio \
         ({:.1}x realtime) | overlap {overlap:.2} | segments {} | text: {text}",
        audio_s / decode_time.as_secs_f64(),
        outcome.transcript.segments.len()
    );

    // Honest comparison note: whisper tiny on the same file (load + decode timed
    // separately). Skipped when the tiny model is not installed.
    if let Some(tiny) = tiny_model() {
        let mut cache = ContextCache::new();
        let load_start = Instant::now();
        let ctx = cache
            .get_or_load(&tiny, false)
            .expect("load the tiny whisper model");
        let load_time = load_start.elapsed();
        let plan = resolve_for(&tiny, EngineSetting::Cpu, false, Some("en"));
        let decode_start = Instant::now();
        let whisper_outcome = suramak_asr::pipeline::transcribe_file(
            ctx,
            &wav,
            &tiny,
            &LangMode::Fixed("en".to_string()),
            None,
            whisper::default_threads(),
            &plan,
            &CancelToken::new(),
            |_| {},
        )
        .expect("transcribe en.wav with whisper tiny");
        let decode_time = decode_start.elapsed();
        let whisper_text = whisper_outcome
            .transcript
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        eprintln!(
            "whisper tiny: load {load_time:.2?} | pipeline {decode_time:.2?} for {audio_s:.2} s of audio \
             ({:.1}x realtime) | text: {whisper_text}",
            audio_s / decode_time.as_secs_f64()
        );
    } else {
        eprintln!("whisper tiny not installed — comparison numbers skipped");
    }
}

/// The normalization helpers themselves (portable, no model needed).
#[test]
fn approximate_match_helpers_behave() {
    assert_eq!(normalized("Hello,  World!"), "hello world");
    assert_eq!(
        in_order_overlap("the quick fox", "The quick brown fox"),
        1.0
    );
    // A misheard brand word costs one word, not the sentence.
    let got = "The quick brown fox jumps over the lazy dog. Surimat keeps every recording \
               on this machine and never sends audio to the cloud.";
    let overlap = in_order_overlap(EXPECTED, got);
    assert!(overlap > 0.9, "overlap {overlap}");
    assert_eq!(in_order_overlap("a b c", ""), 0.0);
    assert_eq!(in_order_overlap("", "anything"), 1.0);
}
