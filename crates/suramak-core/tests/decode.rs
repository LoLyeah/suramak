//! Decode-path tests for the M5 container matrix.
//!
//! Every fixture in `fixtures/video/` is a real ffmpeg file (see
//! `fixtures/video/manifest.tsv` for the exact commands) carrying the same 9 s
//! speech track plus a testsrc video stream. The matrix these tests pin:
//!
//! | container      | audio codec | result                          |
//! |----------------|-------------|---------------------------------|
//! | mp4 / mov / m4v| aac         | decoded (audio track, video ignored) |
//! | mkv            | aac         | decoded                         |
//! | mkv            | opus        | `UnsupportedFormat` with a conversion hint |
//! | webm           | opus        | `UnsupportedFormat` with a conversion hint |

use std::path::PathBuf;

use suramak_core::decode::decode_file;
use suramak_core::SuramakError;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/video")
        .join(name)
}

/// The speech every fixture was built from is 9.0 s of 16 kHz mono audio.
const EXPECTED_SECONDS: f64 = 9.0;

/// Decodes `name` and asserts it carries real 16 kHz mono speech: the right
/// rate, roughly the source length (AAC adds priming/padding), non-silent.
fn assert_speech_track(name: &str) {
    let buffer = decode_file(&fixture(name)).unwrap_or_else(|err| panic!("{name}: {err}"));
    assert_eq!(buffer.rate, 16_000, "{name}: sample rate");
    assert_eq!(buffer.channels, 1, "{name}: channels");
    let seconds = buffer.samples.len() as f64 / f64::from(buffer.rate) / f64::from(buffer.channels);
    assert!(
        (seconds - EXPECTED_SECONDS).abs() < 0.2,
        "{name}: expected ~{EXPECTED_SECONDS} s of audio, got {seconds:.3} s"
    );
    let peak = buffer
        .samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    assert!(peak > 0.05, "{name}: audio is silent (peak {peak})");
}

/// Asserts `name` fails with the actionable error and returns its message.
fn assert_actionable_unsupported(name: &str) -> String {
    let error = decode_file(&fixture(name))
        .err()
        .unwrap_or_else(|| panic!("{name}: expected an error"));
    let SuramakError::UnsupportedFormat { .. } = &error else {
        panic!("{name}: expected UnsupportedFormat, got {error:?}");
    };
    error.to_string()
}

#[test]
fn mp4_aac_decodes_its_audio_track() {
    assert_speech_track("sample-en.mp4");
}

#[test]
fn mov_aac_decodes_its_audio_track() {
    assert_speech_track("sample-en.mov");
}

#[test]
fn m4v_aac_decodes_its_audio_track() {
    assert_speech_track("sample-en.m4v");
}

#[test]
fn mkv_aac_decodes_its_audio_track() {
    assert_speech_track("sample-en.mkv");
}

#[test]
fn webm_opus_is_rejected_with_a_conversion_hint() {
    let message = assert_actionable_unsupported("sample-en-opus.webm");
    assert!(message.contains("sample-en-opus.webm"), "{message}");
    assert!(message.contains("Opus"), "{message}");
    assert!(
        message.contains(r#"ffmpeg -i "sample-en-opus.webm" "sample-en-opus.mp4""#),
        "{message}"
    );
}

#[test]
fn mkv_opus_is_rejected_with_a_conversion_hint() {
    let message = assert_actionable_unsupported("sample-en-opus.mkv");
    assert!(message.contains("Opus"), "{message}");
    assert!(message.contains("ffmpeg -i"), "{message}");
}

#[test]
fn a_garbage_webm_file_fails_with_the_same_actionable_error() {
    // A `.webm` name over bytes that are not a media container: the probe finds
    // no reader — still the actionable error, never a panic or a raw symphonia
    // message.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("clip.webm");
    std::fs::write(&path, b"this is not a webm file").expect("write");
    let error = decode_file(&path).expect_err("garbage must not decode");
    let message = error.to_string();
    assert!(message.contains("cannot decode clip.webm"), "{message}");
    assert!(
        message.contains(r#"ffmpeg -i "clip.webm" "clip.mp4""#),
        "{message}"
    );
    assert!(matches!(error, SuramakError::UnsupportedFormat { .. }));
}

#[test]
fn a_file_ending_mid_marker_fails_with_the_actionable_error() {
    // A marker candidate at the very end of the file makes the probe's scan run
    // off the end (`IoError(UnexpectedEof)`, the real `.aiff` case) — the same
    // actionable error as the no-reader case, not a raw "end of stream".
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("clip.aiff");
    std::fs::write(&path, b"FORM\0\0\0\0AIFF not really audio, ends in RIFF").expect("write");
    let error = decode_file(&path).expect_err("garbage must not decode");
    let message = error.to_string();
    assert!(message.contains("cannot decode clip.aiff"), "{message}");
    assert!(message.contains("format is not supported"), "{message}");
    assert!(
        message.contains(r#"ffmpeg -i "clip.aiff" "clip.mp4""#),
        "{message}"
    );
    assert!(matches!(error, SuramakError::UnsupportedFormat { .. }));
}
