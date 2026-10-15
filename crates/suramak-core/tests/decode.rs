//! Decode-path tests for the M5 container matrix and the M10C Opus decoder.
//!
//! Every fixture in `fixtures/video/` is a real ffmpeg file (see
//! `fixtures/video/manifest.tsv` for the exact commands) carrying the same 9 s
//! speech track plus a testsrc video stream — the raw `.opus` fixture is the
//! webm's Opus stream copied out of its container (Ogg-Opus; the Ogg-Opus
//! mapping declares 48 kHz, while the webm/mkv fixtures declare 16 kHz in their
//! Matroska audio element). The matrix these tests pin:
//!
//! | container      | audio codec | result                          |
//! |----------------|-------------|---------------------------------|
//! | mp4 / mov / m4v| aac         | decoded (audio track, video ignored) |
//! | mkv            | aac         | decoded                         |
//! | mkv            | opus        | decoded (M10C — libopus adapter) |
//! | webm           | opus        | decoded (M10C — libopus adapter) |
//! | ogg (.opus)    | opus        | decoded (M10C — libopus adapter) |
//! | m4a            | alac        | `UnsupportedFormat` with a conversion hint |

use std::path::PathBuf;

use suramak_core::decode::decode_file;
use suramak_core::SuramakError;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/video")
        .join(name)
}

/// The speech every fixture was built from is 9.0 s of mono audio.
const EXPECTED_SECONDS: f64 = 9.0;

/// Decodes `name` and asserts it carries real mono speech at `rate`: the right
/// rate, roughly the source length (codecs add priming/padding), non-silent.
fn assert_speech_track_at(name: &str, rate: u32) {
    let buffer = decode_file(&fixture(name)).unwrap_or_else(|err| panic!("{name}: {err}"));
    let seconds = buffer.samples.len() as f64 / f64::from(buffer.rate) / f64::from(buffer.channels);
    // Visible with `--nocapture` — the decoded shape the report cites.
    eprintln!(
        "{name}: rate {}, {} ch, {seconds:.4} s, {} samples",
        buffer.rate,
        buffer.channels,
        buffer.samples.len()
    );
    assert_eq!(buffer.rate, rate, "{name}: sample rate");
    assert_eq!(buffer.channels, 1, "{name}: channels");
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

/// The AAC fixtures carry the source's 16 kHz rate (ffmpeg encoded them at the
/// input rate). The mkv/webm Opus fixtures declare 16 kHz in their Matroska
/// audio element (`SamplingFrequency` — verified in the file bytes; ffmpeg wrote
/// the source rate there), so libopus decodes at 16 kHz; a raw Ogg-Opus file
/// declares 48 kHz per the Ogg-Opus mapping.
fn assert_speech_track(name: &str) {
    assert_speech_track_at(name, 16_000);
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
fn webm_opus_decodes_its_audio_track() {
    // M10C: used to be the rejection test — the libopus adapter decodes it now.
    assert_speech_track_at("sample-en-opus.webm", 16_000);
}

#[test]
fn mkv_opus_decodes_its_audio_track() {
    assert_speech_track_at("sample-en-opus.mkv", 16_000);
}

#[test]
fn raw_opus_voice_note_decodes_its_audio_track() {
    // The WhatsApp/Discord voice-message shape: a raw Ogg-Opus `.opus` file.
    assert_speech_track_at("sample-en-opus.opus", 48_000);
}

#[test]
fn m4a_alac_is_rejected_with_a_conversion_hint() {
    // A genuinely unsupported codec: the isomp4 reader demuxes the ALAC track
    // (codec params filled from the `alac` atom) but this build has no ALAC
    // decoder — the recipe must still be handed to the user.
    let message = assert_actionable_unsupported("sample-en-alac.m4a");
    assert!(message.contains("sample-en-alac.m4a"), "{message}");
    assert!(message.contains("ALAC"), "{message}");
    assert!(
        message.contains(r#"ffmpeg -i "sample-en-alac.m4a" "sample-en-alac.mp4""#),
        "{message}"
    );
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
