//! M15.6 — measured word-alignment error against the reviewed EN+ID fixtures.
//!
//! The two fixtures under `fixtures/align/` are synthesized **one word at a
//! time** (ffmpeg's flite voice), trimmed to their acoustic bounds and joined
//! with 300 ms of silence; `*.tsv` is the exact construction timeline, so the
//! reference is sample-accurate rather than annotated. See
//! `fixtures/align/README.md` for the exact commands and hashes.
//!
//! The real-model test is `#[ignore]`d (it needs a whisper model, like the
//! other e2e tests) and is the Phase-1 evidence: it fails if the binding stops
//! yielding usable timing — median absolute start error above 250 ms, or a
//! word count that does not line up with the reference at all.
//!
//! ```text
//! SURAMAK_REQUIRE_MODEL=1 cargo test -p suramak-asr --release --test alignment -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};

use suramak_asr::pipeline::{transcribe_file, PipelineOutcome};
use suramak_asr::whisper::{self, AsrError};
use suramak_asr::CancelToken;
use suramak_core::settings::LangMode;
use suramak_core::transcript::{Segment, WordTiming};

/// One reference word: its slot in the synthesized timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reference {
    index: u32,
    word: String,
    start_ms: i64,
    end_ms: i64,
}

fn parse_reference(text: &str) -> Vec<Reference> {
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut fields = line.split('\t');
            let index: u32 = fields.next().expect("idx").parse().expect("idx number");
            let word = fields.next().expect("word").to_string();
            let start_ms: i64 = fields.next().expect("start").parse().expect("start ms");
            let end_ms: i64 = fields.next().expect("end").parse().expect("end ms");
            Reference {
                index,
                word,
                start_ms,
                end_ms,
            }
        })
        .collect()
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/align")
        .join(name)
}

fn model_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("SURAMAK_MODEL_PATH") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    // The models dir is per-OS (suramak_core::paths decides it); a hand-rolled
    // Linux-only lookup is what broke the macos CI job in v0.4.0's first run.
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let home = PathBuf::from(home);
    let local_app = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.clone());
    [
        home.join(".local/share/suramak/models/ggml-tiny.bin"),
        home.join("Library/Application Support/dev.suramak.Suramak/models/ggml-tiny.bin"),
        local_app.join("Suramak/models/ggml-tiny.bin"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

/// Normalizes a word for the lexical comparison: lowercase, no surrounding
/// punctuation. Timing is compared by position regardless — a misrecognized
/// word still occupies the slot the reference puts it in.
fn normalized(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

/// Every word of the run, in segment/word order.
fn flat_words(words: &[WordTiming]) -> Vec<&WordTiming> {
    words.iter().collect()
}

/// One measured pair.
#[derive(Debug, Clone, Copy)]
struct Error {
    start: i64,
    end: i64,
}

/// Summary of a set of absolute errors (milliseconds).
#[derive(Debug, Clone, PartialEq)]
struct Stats {
    count: usize,
    mean: f64,
    median: i64,
    p90: i64,
    max: i64,
}

fn stats(errors: &[i64]) -> Option<Stats> {
    if errors.is_empty() {
        return None;
    }
    let mut sorted = errors.to_vec();
    sorted.sort_unstable();
    let at = |q: f64| sorted[((sorted.len() as f64 * q).ceil() as usize).saturating_sub(1)];
    Some(Stats {
        count: sorted.len(),
        mean: sorted.iter().sum::<i64>() as f64 / sorted.len() as f64,
        median: at(0.5),
        p90: at(0.9),
        max: *sorted.last().expect("non-empty"),
    })
}

/// The share of |errors| at or below `limit` ms, as a percentage.
fn within(errors: &[i64], limit: i64) -> f64 {
    if errors.is_empty() {
        return 0.0;
    }
    let hits = errors.iter().filter(|error| **error <= limit).count();
    hits as f64 * 100.0 / errors.len() as f64
}

fn run_fixture(model: &Path, wav: &str, lang: &str) -> Result<PipelineOutcome, AsrError> {
    let plan = suramak_asr::engine::EngineDecision {
        engine: suramak_asr::EngineId::Cpu,
        warning: None,
    };
    let mut cache = whisper::ContextCache::new();
    let ctx = cache.get_or_load(model, false)?;
    transcribe_file(
        ctx,
        &fixture(wav),
        model,
        &LangMode::Fixed(lang.to_string()),
        None,
        2,
        &plan,
        None,
        &CancelToken::new(),
        |_| {},
        |_| {},
    )
}

/// Prints one measurement table and returns the summary the test asserts on.
fn measure(name: &str, reference: &[Reference], words: &[&WordTiming]) -> (Vec<Error>, Vec<i64>) {
    println!("\n=== {name} ===");
    println!(
        "{:>3}  {:<14} {:>9} {:>9} {:>9} {:>9}",
        "#", "word", "ref start", "est start", "d start", "d end"
    );
    let mut errors = Vec::new();
    for (reference, word) in reference.iter().zip(words.iter()) {
        let error = Error {
            start: word.start_ms - reference.start_ms,
            end: word.end_ms - reference.end_ms,
        };
        println!(
            "{:>3}  {:<14} {:>9} {:>9} {:>+9} {:>+9}   {}",
            reference.index,
            reference.word,
            reference.start_ms,
            word.start_ms,
            error.start,
            error.end,
            word.text
        );
        errors.push(error);
    }
    let start_abs: Vec<i64> = errors.iter().map(|error| error.start.abs()).collect();
    let end_abs: Vec<i64> = errors.iter().map(|error| error.end.abs()).collect();
    for (label, values) in [("start", &start_abs), ("end", &end_abs)] {
        if let Some(stats) = stats(values) {
            println!(
                "{label}: n={} mean={:.1} ms median={} ms p90={} ms max={} ms | \
                 within 100 ms {:.0}% · 250 ms {:.0}% · 500 ms {:.0}%",
                stats.count,
                stats.mean,
                stats.median,
                stats.p90,
                stats.max,
                within(values, 100),
                within(values, 250),
                within(values, 500)
            );
        }
    }
    // Signed mean: a systematic lead/lag is worth seeing (the estimate may
    // consistently start early because whisper's anchor tokens land on the
    // pause before a word).
    let signed_mean =
        errors.iter().map(|error| error.start).sum::<i64>() as f64 / errors.len() as f64;
    println!("signed mean start error: {signed_mean:+.1} ms");
    (errors, start_abs)
}

#[test]
fn the_reference_parser_reads_the_construction_timeline() {
    let reference =
        parse_reference("# idx\tword\tstart_ms\tend_ms\n0\tthe\t250\t386\n1\tquick\t686\t1050\n");
    assert_eq!(reference.len(), 2);
    assert_eq!(reference[0].index, 0);
    assert_eq!(reference[1].word, "quick");
    assert_eq!((reference[1].start_ms, reference[1].end_ms), (686, 1050));
}

#[test]
fn the_error_statistics_are_percentiles_of_absolute_errors() {
    let values = [0_i64, 10, 20, 30, 40, 50, 60, 70, 80, 90];
    let summary = stats(&values).expect("non-empty");
    assert_eq!(summary.count, 10);
    assert_eq!(summary.mean, 45.0);
    assert_eq!(
        summary.median, 40,
        "nearest rank: ceil(10 * 0.5) = 5th value"
    );
    assert_eq!(summary.p90, 80, "nearest rank: ceil(10 * 0.9) = 9th value");
    assert_eq!(summary.max, 90);
    assert_eq!(within(&values, 20), 30.0);
    assert!(stats(&[]).is_none());
    assert_eq!(within(&[], 100), 0.0);
}

#[test]
fn words_are_normalized_for_the_lexical_check_only() {
    assert_eq!(normalized("The"), "the");
    assert_eq!(normalized("dog."), "dog");
    assert_eq!(normalized("“kayak”"), "kayak");
}

#[test]
fn flat_words_keeps_segment_word_order() {
    let words = vec![
        WordTiming {
            segment_idx: 0,
            word_idx: 0,
            start_ms: 0,
            end_ms: 10,
            text: "a".into(),
        },
        WordTiming {
            segment_idx: 1,
            word_idx: 0,
            start_ms: 20,
            end_ms: 30,
            text: "b".into(),
        },
    ];
    let flat = flat_words(&words);
    assert_eq!(flat.len(), 2);
    assert_eq!(flat[1].text, "b");
}

/// The natural, contiguous fixtures (the reviewed EN+ID sentences the rest of
/// the suite uses) have no ground-truth timeline, so no error is claimed for
/// them — only the invariants the store and the UI depend on, plus the word
/// count and how much of the audio the estimates cover.
#[test]
#[ignore = "needs a whisper model (SURAMAK_MODEL_PATH or the data dir); release-only measurement"]
fn natural_fixtures_keep_every_word_inside_its_segment() {
    let Some(model) = model_path() else {
        eprintln!("skipped: no model");
        return;
    };
    for (path, lang) in [
        ("../../fixtures/audio/en.wav", "en"),
        ("../../fixtures/audio/id.wav", "id"),
    ] {
        let wav = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
        let plan = suramak_asr::engine::EngineDecision {
            engine: suramak_asr::EngineId::Cpu,
            warning: None,
        };
        let mut cache = whisper::ContextCache::new();
        let ctx = cache.get_or_load(&model, false).expect("model");
        let outcome = transcribe_file(
            ctx,
            &wav,
            &model,
            &LangMode::Fixed(lang.to_string()),
            None,
            2,
            &plan,
            None,
            &CancelToken::new(),
            |_| {},
            |_| {},
        )
        .expect("transcription");

        let words: Vec<_> = outcome.words.iter().collect();
        let covered_words = outcome
            .transcript
            .segments
            .iter()
            .filter(|segment| {
                outcome
                    .words
                    .iter()
                    .any(|word| word.segment_idx == segment.idx)
            })
            .count();
        println!(
            "\n{path}: {} segments ({covered_words} with words), {} words",
            outcome.transcript.segments.len(),
            words.len()
        );
        println!(
            "first words: {:?}",
            words
                .iter()
                .take(6)
                .map(|word| (word.text.as_str(), word.start_ms, word.end_ms))
                .collect::<Vec<_>>()
        );
        let mut previous = i64::MIN;
        for word in &words {
            let segment = outcome
                .transcript
                .segments
                .iter()
                .find(|segment| segment.idx == word.segment_idx)
                .expect("word belongs to a segment");
            assert!(word.start_ms >= segment.start_ms);
            assert!(word.end_ms <= segment.end_ms);
            assert!(word.end_ms >= word.start_ms);
            assert!(
                word.start_ms >= previous,
                "word starts must not go backwards ({:?} after {previous})",
                word.text
            );
            previous = word.start_ms;
        }
        assert_eq!(
            covered_words,
            outcome.transcript.segments.len(),
            "{path}: every segment of the natural fixture should carry words"
        );
    }
}

/// The measured Phase-1 gate. Run with a real model; skipped without one.
#[test]
#[ignore = "needs a whisper model (SURAMAK_MODEL_PATH or the data dir); release-only measurement"]
fn measured_word_alignment_error_on_the_reviewed_fixtures() {
    let Some(model) = model_path() else {
        let required = std::env::var("SURAMAK_REQUIRE_MODEL").is_ok_and(|value| value == "1");
        if required {
            panic!("SURAMAK_REQUIRE_MODEL=1 but no ggml model was found");
        }
        eprintln!("skipped: no model (set SURAMAK_MODEL_PATH or install the tiny model)");
        return;
    };

    for (wav, tsv, lang) in [
        ("en-words.wav", "en-words.tsv", "en"),
        ("id-words.wav", "id-words.tsv", "id"),
    ] {
        let reference =
            parse_reference(&std::fs::read_to_string(fixture(tsv)).expect("reference tsv"));
        let outcome = run_fixture(&model, wav, lang).expect("transcription");
        let words = flat_words(&outcome.words);
        println!(
            "\n{wav}: {} segments, {} words, {} reference slots",
            outcome.transcript.segments.len(),
            words.len(),
            reference.len()
        );
        println!(
            "segment text: {:?}",
            outcome
                .transcript
                .segments
                .iter()
                .map(|segment: &Segment| segment.text.as_str())
                .collect::<Vec<_>>()
        );

        assert_eq!(
            words.len(),
            reference.len(),
            "{wav}: the run must produce one timed word per reference slot — a count \
             mismatch means the mapping dropped or merged words and the measurement \
             cannot be trusted"
        );
        let lexical_mismatches = reference
            .iter()
            .zip(words.iter())
            .filter(|(reference, word)| normalized(&reference.word) != normalized(&word.text))
            .count();

        let (_, start_abs) = measure(wav, &reference, &words);
        println!("lexical mismatches (timing is still positional): {lexical_mismatches}");
        let stats = stats(&start_abs).expect("words exist");
        assert!(
            stats.median <= 250,
            "{wav}: median |start error| {} ms is above the 250 ms gate",
            stats.median
        );
        assert!(
            stats.p90 <= 500,
            "{wav}: p90 |start error| {} ms is above the 500 ms gate",
            stats.p90
        );
        // Every word must stay inside its segment — the store and the UI rely
        // on that, and a violation would let a click seek outside the row.
        for word in &outcome.words {
            let segment = outcome
                .transcript
                .segments
                .iter()
                .find(|segment| segment.idx == word.segment_idx)
                .expect("word belongs to a segment");
            assert!(
                word.start_ms >= segment.start_ms
                    && word.end_ms <= segment.end_ms
                    && word.end_ms >= word.start_ms,
                "{wav}: word {:?} escapes its segment",
                word.text
            );
        }
    }
}
