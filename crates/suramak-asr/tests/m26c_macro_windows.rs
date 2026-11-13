//! M26C acceptance: the 15-minute macro-window path against the single call.
//!
//! Whisper e2e, release profile, tiny model. The natural corpus and the
//! repeated-clip stress fixture are **not** committed (200–350 MB each); they
//! are built by `docs/reports/M26c-macro-windows.md` §4 and handed in through
//! environment variables:
//!
//! ```text
//! M26C_CORPUS=/path/natural-64min.wav      # >= 60 min, distinct real clips
//! M26C_REFERENCE=/path/natural-64min.txt   # the same clips' transcript
//! M26C_STRESS=/path/repeat-167s-x6.wav     # > 15 min of one repeated clip
//! M26C_RSS_30=/path/natural-30min.wav
//! M26C_RSS_90=/path/natural-90min.wav
//! SURAMAK_REQUIRE_MODEL=1 cargo test -p suramak-asr --release --test \
//!     m26c_macro_windows -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
// The RSS harness spawns itself only on Linux; `Command` is fully qualified at
// that call site so no platform can flag an unused import.
use std::sync::Mutex;

use suramak_asr::engine::{EngineDecision, EngineId};
use suramak_asr::macro_chunk::{plan_windows, MacroConfig};
use suramak_asr::macro_pipeline::{transcribe_macro, MacroOutcome};
use suramak_asr::whisper::{self, ContextCache};
use suramak_asr::CancelToken;
use suramak_core::pcm::PcmSlice;
use suramak_core::settings::LangMode;
use suramak_core::transcript::Segment;

static E2E_LOCK: Mutex<()> = Mutex::new(());

fn e2e_serial() -> std::sync::MutexGuard<'static, ()> {
    E2E_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn tiny_model_or_skip() -> Option<PathBuf> {
    let path = suramak_core::paths::models_dir()
        .ok()?
        .join("ggml-tiny.bin");
    if path.exists() {
        return Some(path);
    }
    if std::env::var("SURAMAK_REQUIRE_MODEL").as_deref() == Ok("1") {
        panic!(
            "SURAMAK_REQUIRE_MODEL=1 but the tiny model is missing at {}",
            path.display()
        );
    }
    eprintln!("skipping whisper e2e: no tiny model at {}", path.display());
    None
}

fn env_path(name: &str) -> Option<PathBuf> {
    let value = std::env::var(name).ok()?;
    let path = PathBuf::from(value);
    if path.is_file() {
        Some(path)
    } else {
        panic!("{name} points at a missing file: {}", path.display())
    }
}

fn cpu_plan() -> EngineDecision {
    EngineDecision {
        engine: EngineId::Cpu,
        warning: None,
    }
}

/// The single call, exactly as `pipeline::transcribe_file` makes it for a file
/// at or below the window target.
fn single_pass(
    cache: &mut ContextCache,
    model: &Path,
    slice: &PcmSlice,
    lang: &LangMode,
) -> Vec<Segment> {
    let ctx = cache.get_or_load(model, false).expect("load tiny model");
    let mut discard = |_: Segment| {};
    whisper::transcribe(
        ctx,
        model,
        slice,
        lang,
        None,
        whisper::default_threads(),
        &cpu_plan(),
        &CancelToken::new(),
        0,
        0,
        |_| {},
        &mut discard,
    )
    .expect("single-pass transcription")
    .segments
}

fn macro_pass(
    cache: &mut ContextCache,
    model: &Path,
    slice: &PcmSlice,
    lang: &LangMode,
) -> MacroOutcome {
    let ctx = cache.get_or_load(model, false).expect("load tiny model");
    let mut discard = |_: Segment| {};
    transcribe_macro(
        ctx,
        model,
        slice,
        lang,
        None,
        whisper::default_threads(),
        &cpu_plan(),
        &CancelToken::new(),
        0,
        0,
        &mut |_| {},
        &mut discard,
    )
    .expect("macro transcription")
}

/// Lowercase alphanumeric words; an apostrophe stays inside a word.
fn normalise(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric() && ch != '\'')
        .filter(|word| !word.is_empty())
        .map(|word| word.to_lowercase())
        .collect()
}

/// Word error rate in percentage points, token-level Levenshtein with two rows
/// (a 64-minute reference is ~11k words; a full matrix would be ~1 GB).
fn wer(reference: &str, hypothesis: &str) -> f64 {
    let reference = normalise(reference);
    let hypothesis = normalise(hypothesis);
    let mut vocabulary: HashMap<String, u32> = HashMap::new();
    let mut encode = |words: &[String]| -> Vec<u32> {
        words
            .iter()
            .map(|word| {
                let next = vocabulary.len() as u32;
                *vocabulary.entry(word.clone()).or_insert(next)
            })
            .collect()
    };
    let a = encode(&reference);
    let b = encode(&hypothesis);
    if a.is_empty() {
        return if b.is_empty() { 0.0 } else { 100.0 };
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, &word) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, &other) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(word != other);
            let deletion = previous[j + 1] + 1;
            let insertion = current[j] + 1;
            current[j + 1] = substitution.min(deletion).min(insertion);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()] as f64 * 100.0 / a.len() as f64
}

/// The longest consecutive run of one 4-gram (1 = no repetition).
fn max_4gram_run(text: &str) -> usize {
    let words = normalise(text);
    let mut best = 1usize;
    let mut index = 0usize;
    while index + 4 <= words.len() {
        let gram = &words[index..index + 4];
        let mut repeats = 1usize;
        let mut next = index + 4;
        while next + 4 <= words.len() && &words[next..next + 4] == gram {
            repeats += 1;
            next += 4;
        }
        best = best.max(repeats);
        index += 1;
    }
    best
}

fn joined(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn print_windows(outcome: &MacroOutcome) {
    for window in &outcome.windows {
        println!(
            "M26C_WINDOW index={} start_ms={} end_ms={} cut_ms={:?} overlap_ms={} \
             cut_without_pause={} carried_tokens={} prompt_reject={:?}",
            window.index,
            window.start_ms,
            window.end_ms,
            window.cut_ms,
            window.overlap_ms,
            window.cut_without_pause,
            window.carried_tokens,
            window.prompt_reject
        );
    }
}

/// Diagnostic: the segment shape of a short slice, single vs macro.
#[test]
#[ignore = "diagnostic; release profile with the tiny model"]
fn segment_shape() {
    let _serial = e2e_serial();
    let Some(model) = tiny_model_or_skip() else {
        return;
    };
    let Some(corpus) = env_path("M26C_SHAPE") else {
        eprintln!("M26C_SHAPE not set");
        return;
    };
    let temp =
        std::env::temp_dir().join(format!("suramak-m26c-shape-{}.pcm32", std::process::id()));
    let slice = suramak_core::decode::decode_to_pcm16k_mono(&corpus, &temp).expect("decode");
    let lang = LangMode::Fixed("en".into());
    let single = single_pass(&mut ContextCache::new(), &model, &slice, &lang);
    let macro_out = macro_pass(&mut ContextCache::new(), &model, &slice, &lang);
    println!(
        "M26C_SHAPE single_segments={} macro_segments={}",
        single.len(),
        macro_out.segments.len()
    );
    for (label, segments) in [("single", &single), ("macro", &macro_out.segments)] {
        for segment in segments {
            println!(
                "M26C_SEG {label} {} {} {} {:?}",
                segment.idx, segment.start_ms, segment.end_ms, segment.text
            );
        }
    }
}

/// The natural-speech gate: |ΔWER| ≤ 0.5 pt, segment counts within 2 %, no
/// 4-gram repeated more than four times consecutively.
///
/// **This test FAILS on the recorded run** (ΔWER −0.62 pt, segment count
/// −7.1 %): that failure is the M26C STOP decision, kept executable so the
/// next attempt measures the same numbers. See
/// `docs/reports/M26c-macro-windows.md`.
#[test]
#[ignore = "M26C gate FAILED: dWER -0.62 pt, segments -7.1 % (see the M26C report); manual measurement harness"]
fn natural_corpus_holds_wer_and_segment_count() {
    let _serial = e2e_serial();
    let Some(model) = tiny_model_or_skip() else {
        return;
    };
    let Some(corpus) = env_path("M26C_CORPUS") else {
        eprintln!("M26C_CORPUS not set: skipping the natural-corpus gate");
        return;
    };
    let reference_path =
        env_path("M26C_REFERENCE").expect("M26C_REFERENCE is required with M26C_CORPUS");
    let reference = std::fs::read_to_string(&reference_path).expect("reference text");

    let temp = std::env::temp_dir().join(format!("suramak-m26c-{}.pcm32", std::process::id()));
    let slice = suramak_core::decode::decode_to_pcm16k_mono(&corpus, &temp).expect("decode");
    println!(
        "M26C_CORPUS samples={} minutes={:.2}",
        slice.len(),
        slice.len() as f64 / 16_000.0 / 60.0
    );
    let windows = plan_windows(slice.as_slice(), slice.len(), &MacroConfig::default());
    println!("M26C_PLANNED_WINDOWS={}", windows.len());
    for window in &windows {
        println!(
            "M26C_PLANNED start_ms={} end_ms={} cut_ms={:?} overlap_ms={} cut_without_pause={}",
            window.start_ms(),
            window.end_ms(),
            window.cut_ms(),
            (window.overlap_samples / 16) as i64,
            window.cut_without_pause
        );
    }
    assert!(windows.len() >= 4, "a 60+ minute file must be cut");

    let lang = LangMode::Fixed("en".into());
    let single = single_pass(&mut ContextCache::new(), &model, &slice, &lang);
    println!(
        "M26C_SINGLE segments={} fourgram_run={} chars={}",
        single.len(),
        max_4gram_run(&joined(&single)),
        joined(&single).len()
    );
    let macro_out = macro_pass(&mut ContextCache::new(), &model, &slice, &lang);
    print_windows(&macro_out);
    let macro_segments = &macro_out.segments;
    let macro_text = joined(macro_segments);

    let single_text = joined(&single);
    let single_wer = wer(&reference, &single_text);
    let macro_wer = wer(&reference, &macro_text);
    let delta = macro_wer - single_wer;
    let count_delta = (macro_segments.len() as f64 - single.len() as f64).abs()
        / single.len().max(1) as f64
        * 100.0;
    let loop_run = max_4gram_run(&macro_text);
    println!(
        "M26C_WER single={single_wer:.4} macro={macro_wer:.4} delta={delta:.4} \
         segments_single={} segments_macro={} count_delta_pct={count_delta:.3} \
         fourgram_run={loop_run}",
        single.len(),
        macro_segments.len()
    );
    let out_dir = std::env::temp_dir().join("suramak-m26c-report");
    std::fs::create_dir_all(&out_dir).unwrap();
    std::fs::write(out_dir.join("single.txt"), &single_text).unwrap();
    std::fs::write(out_dir.join("macro.txt"), &macro_text).unwrap();
    println!("M26C_TRANSCRIPTS={}", out_dir.display());

    assert!(
        delta.abs() <= 0.5,
        "|ΔWER| = {:.3} pt exceeds the 0.5 pt gate",
        delta.abs()
    );
    assert!(
        count_delta <= 2.0,
        "segment count moved by {count_delta:.2} %"
    );
    assert!(loop_run <= 4, "a 4-gram repeated {loop_run} times");
}

/// The repeated-clip stress fixture: zero repetition loops.
#[test]
#[ignore = "whisper e2e: repeated 167 s clip, tiny model, release profile"]
fn repeated_clip_has_no_repetition_loop() {
    let _serial = e2e_serial();
    let Some(model) = tiny_model_or_skip() else {
        return;
    };
    let Some(stress) = env_path("M26C_STRESS") else {
        eprintln!("M26C_STRESS not set: skipping the stress fixture");
        return;
    };
    let temp = std::env::temp_dir().join(format!("suramak-m26c-s-{}.pcm32", std::process::id()));
    let slice = suramak_core::decode::decode_to_pcm16k_mono(&stress, &temp).expect("decode");
    println!(
        "M26C_STRESS samples={} minutes={:.2}",
        slice.len(),
        slice.len() as f64 / 16_000.0 / 60.0
    );
    let outcome = macro_pass(
        &mut ContextCache::new(),
        &model,
        &slice,
        &LangMode::Fixed("en".into()),
    );
    print_windows(&outcome);
    let text = joined(&outcome.segments);
    let run = max_4gram_run(&text);
    println!(
        "M26C_STRESS segments={} fourgram_run={run} chars={}",
        outcome.segments.len(),
        text.len()
    );
    assert!(run <= 4, "a 4-gram repeated {run} times");
}

#[cfg(target_os = "linux")]
fn peak_rss_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest
                .split_whitespace()
                .next()
                .and_then(|value| value.parse().ok())
                .expect("VmHWM value");
        }
    }
    panic!("VmHWM missing from /proc/self/status");
}

/// The child half of the flat-memory measurement.
#[test]
#[ignore = "child of rss_90_minutes_is_flat"]
#[cfg(target_os = "linux")]
fn rss_child() {
    let _serial = e2e_serial();
    let Ok(fixture) = std::env::var("M26C_RSS_CHILD") else {
        return;
    };
    let model = PathBuf::from(std::env::var("M26C_RSS_MODEL").expect("model env"));
    let fixture = PathBuf::from(fixture);
    let slice = suramak_core::decode::decode_to_pcm16k_mono(
        &fixture,
        &std::env::temp_dir().join(format!("suramak-m26c-rss-{}.pcm32", std::process::id())),
    )
    .expect("decode");
    let windows = plan_windows(slice.as_slice(), slice.len(), &MacroConfig::default());
    let outcome = macro_pass(
        &mut ContextCache::new(),
        &model,
        &slice,
        &LangMode::Fixed("en".into()),
    );
    println!("M26C_RSS_KB={}", peak_rss_kb());
    println!("M26C_RSS_SEGMENTS={}", outcome.segments.len());
    println!("M26C_RSS_WINDOWS={}", windows.len());
}

/// Memory gate: a 90-minute file on tiny peaks ≤ 450 MB and within ±15 MB of
/// the 30-minute run (the mel/state are per-window, so duration stops mattering).
#[test]
#[ignore = "two long whisper runs; release profile with the tiny model"]
#[cfg(target_os = "linux")]
fn rss_90_minutes_is_flat() {
    let _serial = e2e_serial();
    let Some(model) = tiny_model_or_skip() else {
        return;
    };
    let (Some(thirty), Some(ninety)) = (env_path("M26C_RSS_30"), env_path("M26C_RSS_90")) else {
        eprintln!("M26C_RSS_30 / M26C_RSS_90 not set: skipping the memory gate");
        return;
    };
    let exe = std::env::current_exe().expect("test binary path");
    let run = |fixture: &Path| -> (u64, usize) {
        let output = std::process::Command::new(&exe)
            .args(["--ignored", "--exact", "rss_child", "--nocapture"])
            .env("M26C_RSS_CHILD", fixture)
            .env("M26C_RSS_MODEL", &model)
            .output()
            .expect("spawn the RSS child");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "RSS child failed for {}:\n{stdout}\n{stderr}",
            fixture.display()
        );
        let kb = stdout
            .lines()
            .find_map(|line| line.strip_prefix("M26C_RSS_KB="))
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or_else(|| panic!("no VmHWM in child output:\n{stdout}\n{stderr}"));
        let windows = stdout
            .lines()
            .find_map(|line| line.strip_prefix("M26C_RSS_WINDOWS="))
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0);
        (kb, windows)
    };
    let (kb30, w30) = run(&thirty);
    let (kb90, w90) = run(&ninety);
    let mb = |kb: u64| kb as f64 * 1024.0 / 1_000_000.0;
    let mib = |kb: u64| kb as f64 / 1024.0;
    println!(
        "M26C_RSS peak30={:.1} MiB ({:.1} MB, {w30} windows) peak90={:.1} MiB ({:.1} MB, \
         {w90} windows) delta={:.1} MB",
        mib(kb30),
        mb(kb30),
        mib(kb90),
        mb(kb90),
        mb(kb90) - mb(kb30)
    );
    assert!(
        mb(kb90) <= 450.0,
        "90-minute peak {:.1} MB exceeds 450 MB",
        mb(kb90)
    );
    assert!(
        (mb(kb90) - mb(kb30)).abs() <= 15.0,
        "90-minute peak differs from the 30-minute peak by {:.1} MB",
        mb(kb90) - mb(kb30)
    );
}
