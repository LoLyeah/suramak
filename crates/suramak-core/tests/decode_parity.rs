//! M15.2 — import parity through the shipped decode path.
//!
//! Every fixture under `fixtures/audio/` is a real ffmpeg artifact; the exact
//! generator commands and sha256 pins live in `fixtures/audio/README.md`. The
//! Opus packets are encoded **once** and remuxed with `-c:a copy` into WebM and
//! Matroska, so the three containers carry the same packets at the same 48 kHz
//! mono shape: a correct importer must produce bit-identical PCM from all three.
//! That is what "container parity" means here — not "sounds similar".
//!
//! Gates:
//! - Ogg-Opus / WebM-Opus / MKV-Opus decode to the same PCM hash per language;
//! - truncated byte prefixes fail with [`SuramakError::Truncated`] (named error,
//!   no panic) instead of importing a short fragment;
//! - garbage in a `.webm` fails with the actionable unsupported-format error;
//! - cancellation is observed *mid-decode*, after packets have been read.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use sha2::{Digest, Sha256};
use suramak_core::decode::{decode_file, decode_file_cancellable, AudioBuffer};
use suramak_core::SuramakError;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/audio")
        .join(name)
}

/// Mixes interleaved channels down to mono (the parity normalization's first
/// half; all fixtures are mono already, so this is a guard, not a conversion).
fn mono(samples: &[f32], channels: u16) -> Vec<f32> {
    let channels = usize::from(channels.max(1));
    if channels == 1 {
        return samples.to_vec();
    }
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

/// Linearly resamples mono `input` from `from` to 48 kHz (the second half of the
/// normalization). Unused by the Opus fixtures — Opus always decodes at 48 kHz —
/// but keeps the helper honest if a future fixture carries another rate.
fn to_48k(input: &[f32], from: u32) -> Vec<f32> {
    if from == 48_000 {
        return input.to_vec();
    }
    let ratio = 48_000.0 / f64::from(from);
    let out_len = (input.len() as f64 * ratio).round() as usize;
    (0..out_len)
        .map(|i| {
            let src = i as f64 / ratio;
            let left = src.floor() as usize;
            let right = (left + 1).min(input.len().saturating_sub(1));
            let frac = (src - left as f64) as f32;
            let a = input.get(left).copied().unwrap_or(0.0);
            let b = input.get(right).copied().unwrap_or(a);
            a + (b - a) * frac
        })
        .collect()
}

/// SHA-256 over the normalized PCM's little-endian f32 bytes.
fn pcm_hash(buffer: &AudioBuffer) -> String {
    let mono = mono(&buffer.samples, buffer.channels);
    let normalized = to_48k(&mono, buffer.rate);
    let mut hasher = Sha256::new();
    for sample in &normalized {
        hasher.update(sample.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Decodes `name`, asserting the 48 kHz mono shape and returning its hash +
/// duration. Complete fixtures must never be mistaken for truncated ones.
fn decode_and_hash(name: &str) -> (String, f64) {
    let buffer = decode_file(&fixture(name)).unwrap_or_else(|err| panic!("{name}: {err}"));
    assert_eq!(buffer.rate, 48_000, "{name}: Opus decodes at 48 kHz");
    assert_eq!(buffer.channels, 1, "{name}: mono");
    let seconds = buffer.samples.len() as f64 / f64::from(buffer.rate);
    assert!(
        (seconds - 4.0).abs() < 0.25,
        "{name}: expected ~4 s, got {seconds:.3}"
    );
    let peak = buffer
        .samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    assert!(peak > 0.02, "{name}: decoded audio is silent");
    (pcm_hash(&buffer), seconds)
}

#[test]
fn every_container_yields_the_same_pcm_for_english_speech() {
    let (ogg, secs) = decode_and_hash("speech-en.opus");
    let (webm, _) = decode_and_hash("speech-en.webm");
    let (mkv, _) = decode_and_hash("speech-en.mkv");
    eprintln!("EN: {secs:.3} s, hash {ogg}");
    assert_eq!(ogg, webm, "Ogg-Opus and WebM-Opus must decode identically");
    assert_eq!(ogg, mkv, "Ogg-Opus and MKV-Opus must decode identically");
}

#[test]
fn every_container_yields_the_same_pcm_for_indonesian_speech() {
    let (ogg, secs) = decode_and_hash("speech-id.opus");
    let (webm, _) = decode_and_hash("speech-id.webm");
    let (mkv, _) = decode_and_hash("speech-id.mkv");
    eprintln!("ID: {secs:.3} s, hash {ogg}");
    assert_eq!(ogg, webm, "Ogg-Opus and WebM-Opus must decode identically");
    assert_eq!(ogg, mkv, "Ogg-Opus and MKV-Opus must decode identically");
}

#[test]
fn the_two_languages_are_not_the_same_audio() {
    let (en, _) = decode_and_hash("speech-en.opus");
    let (id, _) = decode_and_hash("speech-id.opus");
    assert_ne!(en, id, "a copy-paste fixture mistake must fail loudly");
}

#[test]
fn truncated_prefixes_fail_with_a_named_error_and_no_panic() {
    // 60 % prefixes of every container: the demuxer ends mid-packet while the
    // header still declares ~4 s. The import must refuse, not transcribe 2.4 s
    // of a 4 s voice note as if that were the whole message.
    let mut decoded = Vec::new();
    for name in [
        "truncated-en.opus",
        "truncated-en.webm",
        "truncated-en.mkv",
        "truncated-id.opus",
        "truncated-id.webm",
        "truncated-id.mkv",
    ] {
        let error = match decode_file(&fixture(name)) {
            Ok(buffer) => {
                decoded.push((name, buffer.samples.len()));
                continue;
            }
            Err(error) => error,
        };
        assert!(
            matches!(&error, SuramakError::Truncated { .. }),
            "{name}: expected Truncated, got {error:?}"
        );
        let message = error.to_string();
        assert!(message.contains("truncated"), "{name}: {message}");
        assert!(
            message.contains(name),
            "{name}: must name the file: {message}"
        );
    }
    assert!(
        decoded.is_empty(),
        "these prefixes decoded silently instead of failing: {decoded:?}"
    );
}

#[test]
fn a_malformed_container_fails_cleanly() {
    let error = decode_file(&fixture("malformed.webm")).expect_err("garbage must not decode");
    assert!(
        matches!(&error, SuramakError::UnsupportedFormat { .. }),
        "expected the actionable UnsupportedFormat, got {error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains("cannot decode malformed.webm"),
        "{message}"
    );
}

#[test]
fn cancellation_mid_decode_stops_the_packet_loop() {
    let calls = AtomicUsize::new(0);
    // The first five checks pass (the pre-probe check plus four packets), then
    // the sixth cancels. Four packets is ~0.2 s of a 4 s file — proof the stop
    // happened inside the loop, not at the stage boundary before it.
    let cancelled = || calls.fetch_add(1, Ordering::SeqCst) >= 5;

    let error = decode_file_cancellable(&fixture("speech-en.webm"), &cancelled)
        .expect_err("a cancelled decode must return an error");
    assert!(matches!(error, SuramakError::Cancelled), "got {error:?}");
    let checks = calls.load(Ordering::SeqCst);
    assert!(
        checks >= 6,
        "the closure must have been consulted before and inside the packet loop (got {checks})"
    );

    // A pre-cancelled token stops before the probe reads anything.
    let pre = AtomicUsize::new(0);
    let error = decode_file_cancellable(&fixture("speech-en.opus"), &|| {
        pre.fetch_add(1, Ordering::SeqCst);
        true
    })
    .expect_err("pre-cancelled");
    assert!(matches!(error, SuramakError::Cancelled));
    assert_eq!(pre.load(Ordering::SeqCst), 1);
}

/// The fixtures are committed data, not build output: a missing or empty file
/// must fail this suite with a readable name rather than a confusing decode.
#[test]
fn every_fixture_is_present_and_non_empty() {
    for name in [
        "speech-en.opus",
        "speech-en.webm",
        "speech-en.mkv",
        "speech-id.opus",
        "speech-id.webm",
        "speech-id.mkv",
        "truncated-en.opus",
        "truncated-en.webm",
        "truncated-en.mkv",
        "truncated-id.opus",
        "truncated-id.webm",
        "truncated-id.mkv",
        "malformed.webm",
    ] {
        let path = fixture(name);
        let size = std::fs::metadata(&path)
            .unwrap_or_else(|err| panic!("missing fixture {}: {err}", path.display()))
            .len();
        assert!(size > 0, "{name} is empty");
    }
}

#[test]
fn the_fixture_readme_carries_the_generator_commands_and_pins() {
    let readme = std::fs::read_to_string(fixture("README.md")).expect("fixtures README");
    for needle in [
        "ffmpeg",
        "-c:a copy",
        "sha256sum",
        "speech-en.opus",
        "speech-id.opus",
        "truncated-en.webm",
    ] {
        assert!(readme.contains(needle), "README is missing {needle:?}");
    }
    assert!(
        readme
            .lines()
            .filter(|line| {
                line.split_whitespace().next().is_some_and(|hash| {
                    hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())
                })
            })
            .count()
            >= 13,
        "README must pin every fixture's sha256"
    );
}
