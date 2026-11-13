//! M26B acceptance: the streaming path's transcripts are byte-identical to the
//! old in-memory path, the temp file is cleaned on success/cancel/panic, and a
//! 30-minute file peaks ≥ 200 MB below the in-memory path.
//!
//! Whisper e2e: release profile, tiny model in the standard data dir, run with
//! `SURAMAK_REQUIRE_MODEL=1 cargo test -p suramak-asr --release --test
//! m26b_streaming -- --ignored --nocapture`.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
// The child-process RSS harness is unix-only (/proc). `Command` is used fully
// qualified at its one call site instead of imported, so no platform can flag
// an unused import (CI's windows clippy did, twice, on two harnesses).
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use suramak_asr::engine::{EngineDecision, EngineId};
use suramak_asr::pipeline::transcribe_file;
use suramak_asr::whisper::{self, ContextCache};
use suramak_asr::CancelToken;
use suramak_core::decode::{
    decode_16khz_mono_in_memory_for_tests, decode_to_pcm16k_mono_cancellable,
};
use suramak_core::pcm::{self, PcmSlice};
use suramak_core::settings::LangMode;
use suramak_core::transcript::Segment;

/// One whisper session at a time in this process (the same 2-core constraint the
/// other e2e files document).
static E2E_LOCK: Mutex<()> = Mutex::new(());
const TEMP_PREFIX: &str = "suramak_";

fn e2e_serial() -> std::sync::MutexGuard<'static, ()> {
    E2E_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/audio")
        .join(name)
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
    eprintln!(
        "skipping whisper e2e test: tiny model not found at {} \
         (set SURAMAK_REQUIRE_MODEL=1 to make this a hard failure)",
        path.display()
    );
    None
}

/// Frames of this process's temp `.pcm32` files (the per-pid prefix keeps other
/// processes and test binaries out of the count).
fn live_temp_pcm_files() -> Vec<PathBuf> {
    let prefix = format!("{TEMP_PREFIX}{}_", std::process::id());
    std::fs::read_dir(pcm::temp_pcm_dir())
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".pcm32"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A stable digest of the acceptance-relevant segment fields.
fn digest(segments: &[Segment]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    };
    for segment in segments {
        eat(&segment.idx.to_le_bytes());
        eat(&segment.start_ms.to_le_bytes());
        eat(&segment.end_ms.to_le_bytes());
        eat(segment.text.as_bytes());
        eat(&[0]);
    }
    hash
}

fn assert_identical(old: &[Segment], new: &[Segment], label: &str) {
    assert_eq!(old.len(), new.len(), "{label}: segment count");
    for (index, (a, b)) in old.iter().zip(new).enumerate() {
        assert_eq!(a.idx, b.idx, "{label}: segment {index} idx");
        assert_eq!(a.start_ms, b.start_ms, "{label}: segment {index} start_ms");
        assert_eq!(a.end_ms, b.end_ms, "{label}: segment {index} end_ms");
        assert_eq!(a.text, b.text, "{label}: segment {index} text");
    }
}

/// The old path, run through the exact same pipeline with a pre-decoded buffer:
/// `decode_file` + `to_16khz_mono` into one `Vec<f32>`, wrapped without a copy.
fn transcribe_in_memory(
    cache: &mut ContextCache,
    input: &Path,
    model: &Path,
    lang: &LangMode,
) -> Vec<Segment> {
    let samples = decode_16khz_mono_in_memory_for_tests(input).expect("in-memory decode");
    let prepared = PcmSlice::from_samples(samples);
    let ctx = cache.get_or_load(model, false).expect("load tiny model");
    transcribe_file(
        ctx,
        input,
        model,
        lang,
        None,
        whisper::default_threads(),
        &EngineDecision {
            engine: EngineId::Cpu,
            warning: None,
        },
        Some(prepared),
        &CancelToken::new(),
        None,
        |_| {},
        |_| {},
    )
    .expect("old-path transcription")
    .transcript
    .segments
}

fn transcribe_streaming(
    cache: &mut ContextCache,
    input: &Path,
    model: &Path,
    lang: &LangMode,
) -> Vec<Segment> {
    let ctx = cache.get_or_load(model, false).expect("load tiny model");
    transcribe_file(
        ctx,
        input,
        model,
        lang,
        None,
        whisper::default_threads(),
        &EngineDecision {
            engine: EngineId::Cpu,
            warning: None,
        },
        None,
        &CancelToken::new(),
        None,
        |_| {},
        |_| {},
    )
    .expect("streaming transcription")
    .transcript
    .segments
}

/// Writes a PCM16 WAV without holding the whole clip: 8 192 frames at a time.
fn write_wav_streaming(
    path: &Path,
    rate: u32,
    channels: u16,
    total_frames: u64,
    mut sample: impl FnMut(u64) -> i16,
) {
    let data_bytes = total_frames * u64::from(channels) * 2;
    let mut file = BufWriter::new(std::fs::File::create(path).expect("create wav"));
    file.write_all(b"RIFF").unwrap();
    file.write_all(&((36 + data_bytes) as u32).to_le_bytes())
        .unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap(); // PCM
    file.write_all(&channels.to_le_bytes()).unwrap();
    file.write_all(&rate.to_le_bytes()).unwrap();
    file.write_all(&(rate * u32::from(channels) * 2).to_le_bytes())
        .unwrap();
    file.write_all(&(channels * 2).to_le_bytes()).unwrap();
    file.write_all(&16u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&(data_bytes as u32).to_le_bytes()).unwrap();
    for frame in 0..total_frames {
        let value = sample(frame).to_le_bytes();
        for _ in 0..channels {
            file.write_all(&value).unwrap();
        }
    }
    file.flush().unwrap();
}

/// A long-form fixture: 10 s of speech every 30 s at `rate`, natural pitch
/// (linearly upsampled from the 16 kHz fixture), with silence between so a
/// 20–30 minute run does not turn into a repetition stress test.
fn write_long_form(path: &Path, minutes: u64, rate: u32, channels: u16) {
    let source = suramak_core::decode::decode_file(&fixture("en.wav"))
        .expect("decode en.wav")
        .samples;
    // 16 kHz → `rate` linear interpolation, so the speech keeps its duration.
    let upsampled: Vec<f32> = (0..source.len() * rate as usize / 16_000)
        .map(|index| {
            let position = index as f64 * 16_000.0 / f64::from(rate);
            let left = position.floor() as usize;
            let frac = (position - left as f64) as f32;
            let a = source.get(left).copied().unwrap_or(0.0);
            let b = source.get(left + 1).copied().unwrap_or(a);
            a + (b - a) * frac
        })
        .collect();
    let rate = u64::from(rate);
    let speech_frames = upsampled.len().min(10 * rate as usize);
    let total_frames = minutes * 60 * rate;
    write_wav_streaming(path, rate as u32, channels, total_frames, |frame| {
        let within = frame % (30 * rate);
        if within < speech_frames as u64 {
            let value = upsampled[within as usize];
            (value * f32::from(i16::MAX))
                .round()
                .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
        } else {
            0
        }
    });
}

/// The acceptance test: 3 fixtures, old in-memory path vs streaming path,
/// identical segment text, start/end ms and count.
#[test]
#[ignore = "whisper e2e; release profile with the tiny model"]
fn three_fixtures_produce_byte_identical_transcripts() {
    let _serial = e2e_serial();
    let Some(model) = tiny_model_or_skip() else {
        return;
    };
    let long_form =
        std::env::temp_dir().join(format!("suramak-m26b-20min-{}.wav", std::process::id()));
    write_long_form(&long_form, 20, 44_100, 1);
    let cases = [
        (
            "short en (16 kHz WAV, fast path)",
            fixture("en.wav"),
            LangMode::Fixed("en".into()),
        ),
        (
            "id (16 kHz WAV, fast path)",
            fixture("id.wav"),
            LangMode::Fixed("id".into()),
        ),
        (
            "20-minute synthesised 44.1 kHz WAV (streaming resample)",
            long_form.clone(),
            LangMode::Fixed("en".into()),
        ),
    ];
    let mut cache = ContextCache::new();
    for (label, input, lang) in &cases {
        let old = transcribe_in_memory(&mut cache, input, &model, lang);
        let new = transcribe_streaming(&mut cache, input, &model, lang);
        assert_identical(&old, &new, label);
        eprintln!(
            "{label}: {} segments, digest old={:016x} new={:016x}",
            new.len(),
            digest(&old),
            digest(&new)
        );
        assert_eq!(digest(&old), digest(&new), "{label}: digest");
        assert!(
            live_temp_pcm_files().is_empty(),
            "{label}: the temp .pcm32 must be gone after a successful run"
        );
    }
    let _ = std::fs::remove_file(&long_form);
}

/// Success, mid-decode cancellation and a simulated panic all leave zero
/// `suramak_*.pcm32`; a startup sweep removes a planted orphan.
#[test]
#[ignore = "whisper e2e; release profile with the tiny model"]
fn temp_hygiene_on_success_cancel_and_panic() {
    let _serial = e2e_serial();
    let Some(model) = tiny_model_or_skip() else {
        return;
    };
    // 1. Success through the streaming path (MP3, so no WAV fast path).
    let mut cache = ContextCache::new();
    let segments = transcribe_streaming(
        &mut cache,
        &fixture("en.mp3"),
        &model,
        &LangMode::Fixed("en".into()),
    );
    assert!(!segments.is_empty(), "the MP3 run must produce segments");
    assert!(
        live_temp_pcm_files().is_empty(),
        "success left {:?}",
        live_temp_pcm_files()
    );

    // 2. Cancel mid-decode: the token trips after the writer opened the file.
    let checks = AtomicUsize::new(0);
    let cancelled = || checks.fetch_add(1, Ordering::Relaxed) >= 10;
    let out = std::env::temp_dir().join(format!("suramak_{}_midcancel.pcm32", std::process::id()));
    let error = decode_to_pcm16k_mono_cancellable(&fixture("en.mp3"), &out, &cancelled)
        .expect_err("mid-decode cancel");
    assert!(
        matches!(error, suramak_core::SuramakError::Cancelled),
        "{error}"
    );
    assert!(!out.exists(), "cancelled decode left {}", out.display());

    // 3. A simulated panic while the mapped slice is alive.
    let panic_path =
        std::env::temp_dir().join(format!("suramak_{}_panic.pcm32", std::process::id()));
    let slice = suramak_core::decode::decode_to_pcm16k_mono(&fixture("en.mp3"), &panic_path)
        .expect("decode for the panic case");
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _slice = slice;
        panic!("simulated job panic");
    }));
    assert!(outcome.is_err());
    assert!(!panic_path.exists(), "a panic leaked the temp file");
    assert!(
        live_temp_pcm_files().is_empty(),
        "panic left {:?}",
        live_temp_pcm_files()
    );

    // 4. The startup sweep removes a planted orphan.
    let orphan = std::env::temp_dir().join(format!("suramak_{}_orphan.pcm32", std::process::id()));
    std::fs::write(&orphan, [0u8; 8]).unwrap();
    let removed = pcm::sweep_orphaned_temp_pcm(&std::env::temp_dir());
    assert!(removed >= 1, "the sweep must remove the planted orphan");
    assert!(!orphan.exists());
}

/// Reads this process's peak resident set in KiB (`VmHWM`), Linux only.
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

/// The child half of the RSS measurement. The parent sets `M26B_RSS_CHILD` to
/// `old` or `new`; without it this is a no-op (so a full `--ignored` run does
/// not transcribe a 30-minute file twice by accident).
#[test]
#[ignore = "child of rss_30_minutes_streaming_is_200mb_lower"]
#[cfg(target_os = "linux")]
fn rss_child() {
    let _serial = e2e_serial();
    let Ok(mode) = std::env::var("M26B_RSS_CHILD") else {
        return;
    };
    let fixture_path = PathBuf::from(std::env::var("M26B_RSS_FIXTURE").expect("fixture env"));
    let model = PathBuf::from(std::env::var("M26B_RSS_MODEL").expect("model env"));
    let mut cache = ContextCache::new();
    let lang = LangMode::Fixed("en".into());
    let segments = if mode == "old" {
        // The same two full-file allocations the old pipeline made, with the
        // phase peaks printed so the report can name where the memory goes.
        let buffer = suramak_core::decode::decode_file(&fixture_path).expect("decode");
        println!(
            "M26B_PHASE_DECODE_KB={} samples={}",
            peak_rss_kb(),
            buffer.samples.len()
        );
        let samples = suramak_core::resample::to_16khz_mono(&buffer).expect("resample");
        println!("M26B_PHASE_RESAMPLE_KB={}", peak_rss_kb());
        drop(buffer);
        let prepared = PcmSlice::from_samples(samples);
        let ctx = cache.get_or_load(&model, false).expect("load tiny model");
        println!("M26B_PHASE_MODEL_KB={}", peak_rss_kb());
        transcribe_file(
            ctx,
            &fixture_path,
            &model,
            &lang,
            None,
            whisper::default_threads(),
            &EngineDecision {
                engine: EngineId::Cpu,
                warning: None,
            },
            Some(prepared),
            &CancelToken::new(),
            None,
            |_| {},
            |_| {},
        )
        .expect("old-path transcription")
        .transcript
        .segments
    } else {
        transcribe_streaming(&mut cache, &fixture_path, &model, &lang)
    };
    println!("M26B_RSS_KB={}", peak_rss_kb());
    println!("M26B_SEGMENTS={}", segments.len());
    println!("M26B_MODE={mode}");
}

/// The RSS acceptance: a 30-minute 48 kHz **stereo** file through the streaming
/// path peaks at least 200 MB below the in-memory path on this box.
#[test]
#[ignore = "30-minute whisper run twice; release profile with the tiny model"]
#[cfg(target_os = "linux")]
fn rss_30_minutes_streaming_is_200mb_lower() {
    let _serial = e2e_serial();
    let Some(model) = tiny_model_or_skip() else {
        return;
    };
    let long_form =
        std::env::temp_dir().join(format!("suramak-m26b-30min-{}.wav", std::process::id()));
    if !long_form.exists() {
        write_long_form(&long_form, 30, 48_000, 2);
    }
    let exe = std::env::current_exe().expect("test binary path");
    let run = |mode: &str| -> u64 {
        let output = std::process::Command::new(&exe)
            .args(["--ignored", "--exact", "rss_child", "--nocapture"])
            .env("M26B_RSS_CHILD", mode)
            .env("M26B_RSS_FIXTURE", &long_form)
            .env("M26B_RSS_MODEL", &model)
            .output()
            .expect("spawn the RSS child");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "RSS child ({mode}) failed:\n{stdout}\n{stderr}"
        );
        stdout
            .lines()
            .find_map(|line| line.strip_prefix("M26B_RSS_KB="))
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or_else(|| panic!("no VmHWM in child output:\n{stdout}\n{stderr}"))
    };
    let old_kb = run("old");
    let new_kb = run("new");
    let delta_mb = (old_kb as f64 - new_kb as f64) / 1024.0;
    eprintln!(
        "peak VmHWM: in-memory {} MiB, streaming {} MiB, delta {:.1} MiB",
        old_kb / 1024,
        new_kb / 1024,
        delta_mb
    );
    assert!(
        delta_mb >= 200.0,
        "the streaming path must peak >= 200 MiB below the in-memory path, got {delta_mb:.1} MiB"
    );
    let _ = std::fs::remove_file(&long_form);
}
