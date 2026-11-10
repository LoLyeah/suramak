//! M13e — the Android job's decode is the M15.2 shared path, proven on the host.
//!
//! The workorder's deliverable 6 (decode through the shared interface) has one
//! honest host-side proof: the type the transcription queue constructs for
//! Android is `suramak_asr::FileDecoder`, and this test drives **that type**
//! over the same committed fixtures the M15.2 contract pins. It asserts:
//!
//! * the three Opus containers (Ogg-Opus WhatsApp voice note, WebM, Matroska)
//!   produce one PCM hash — M15.2's container parity, through
//!   `decode_file`, the function `FileDecoder` calls;
//! * `FileDecoder`'s output is bit-identical to `to_16khz_mono` of that same
//!   buffer, so the job adds only the documented resample and no second decoder;
//! * truncated Ogg/WebM/MKV prefixes fail with the named `Truncated` error —
//!   through `FileDecoder` too, as `DecodeFailure::Failed` carrying the name;
//! * garbage fails cleanly (no panic), and cancellation is observed.
//!
//! It duplicates the hash helper from `crates/suramak-core/tests/decode_parity.rs`
//! on purpose: the point is that an *independent* consumer of the decoder gets
//! the same bytes, not that one test file is shared.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use suramak_asr::{AudioDecoder, CancelToken, DecodeFailure, FileDecoder};
use suramak_core::decode::{decode_file, decode_file_cancellable};
use suramak_core::resample;
use suramak_core::SuramakError;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../suramak-core/fixtures/audio")
        .join(name)
}

/// The three containers are expected to be bit-identical (the Opus packets are
/// the same bytes, remuxed with `-c:a copy`), so the comparison is exact sample
/// equality rather than a perceptual tolerance. M15.2 states the same rule as a
/// PCM hash; comparing the buffers directly is the same assertion with a better
/// failure message ("first differing sample at N").
fn same_pcm(left: &[f32], right: &[f32], label: &str) {
    assert_eq!(left.len(), right.len(), "{label}: different sample counts");
    if let Some(index) = left
        .iter()
        .zip(right)
        .position(|(a, b)| a.to_bits() != b.to_bits())
    {
        panic!(
            "{label}: first differing sample at {index}: {} vs {}",
            left[index], right[index]
        );
    }
}

fn decode(name: &str) -> (Vec<f32>, u32, u16) {
    let buffer = decode_file(&fixture(name)).unwrap_or_else(|err| panic!("{name}: {err}"));
    assert_eq!(buffer.rate, 48_000, "{name}: Opus decodes at 48 kHz");
    assert_eq!(buffer.channels, 1, "{name}: mono");
    (buffer.samples, buffer.rate, buffer.channels)
}

/// M15.2's container parity, then the job decoder's exact relationship to it.
#[test]
fn the_android_job_decodes_through_the_shared_parity_path() {
    for language in ["en", "id"] {
        let (ogg, _, _) = decode(&format!("speech-{language}.opus"));
        let (webm, _, _) = decode(&format!("speech-{language}.webm"));
        let (mkv, _, _) = decode(&format!("speech-{language}.mkv"));
        same_pcm(&ogg, &webm, &format!("{language}: Ogg-Opus vs WebM-Opus"));
        same_pcm(
            &ogg,
            &mkv,
            &format!("{language}: Ogg-Opus vs Matroska-Opus"),
        );

        // The decoder the Android queue actually constructs: same file, same
        // samples after the documented 16 kHz mono resample.
        let path = fixture(&format!("speech-{language}.opus"));
        let buffer = decode_file(&path).unwrap();
        let expected = resample::to_16khz_mono(&buffer).unwrap();
        let job = FileDecoder
            .decode(&path, &CancelToken::new())
            .unwrap_or_else(|err| panic!("{language}: {err:?}"));
        assert_eq!(
            job, expected,
            "{language}: the job decoder must not fork the shared decode"
        );
        let seconds = job.len() as f32 / 16_000.0;
        assert!(
            (seconds - 4.0).abs() < 0.3,
            "{language}: unexpected duration {seconds:.2}s"
        );
    }
    // A copy-paste fixture mistake must fail loudly: the two languages are not
    // the same audio.
    let (en, _, _) = decode("speech-en.opus");
    let (id, _, _) = decode("speech-id.opus");
    assert_ne!(en.len(), 0);
    assert!(
        en.iter().zip(&id).any(|(a, b)| a.to_bits() != b.to_bits()),
        "English and Indonesian fixtures must not decode identically"
    );
}

/// Truncated containers must fail with the named error, never import a fragment.
#[test]
fn the_android_job_refuses_truncated_containers() {
    for name in [
        "truncated-en.opus",
        "truncated-en.webm",
        "truncated-en.mkv",
        "truncated-id.opus",
    ] {
        let error = decode_file(&fixture(name)).expect_err("a truncated file must not decode");
        assert!(
            matches!(error, SuramakError::Truncated { .. }),
            "{name}: expected Truncated, got {error:?}"
        );
        let through_job = FileDecoder
            .decode(&fixture(name), &CancelToken::new())
            .expect_err("the job decoder must fail too");
        match through_job {
            DecodeFailure::Failed(message) => {
                assert!(message.contains("truncated"), "{name}: {message}");
                assert!(message.contains(name), "{name} must be named: {message}");
            }
            DecodeFailure::Cancelled => panic!("{name}: a truncation is not a cancellation"),
        }
    }
}

/// Garbage in a `.webm` fails with the actionable error; no panic reaches the
/// service.
#[test]
fn the_android_job_fails_cleanly_on_garbage() {
    let path = fixture("malformed.webm");
    let error = decode_file(&path).expect_err("garbage must not decode");
    assert!(matches!(error, SuramakError::UnsupportedFormat { .. }));
    match FileDecoder.decode(&path, &CancelToken::new()) {
        Err(DecodeFailure::Failed(message)) => {
            assert!(message.contains("malformed.webm"), "{message}");
        }
        other => panic!("expected a named failure, got {other:?}"),
    }
}

/// Cancellation is the M15.2 contract: a pre-cancelled token stops before the
/// probe, and a token flipped mid-loop stops inside the packet loop.
#[test]
fn the_android_job_honours_cancellation() {
    let pre = CancelToken::new();
    pre.cancel();
    assert!(matches!(
        FileDecoder.decode(&fixture("speech-en.opus"), &pre),
        Err(DecodeFailure::Cancelled)
    ));

    let calls = AtomicUsize::new(0);
    let cancelled = || calls.fetch_add(1, Ordering::SeqCst) >= 5;
    let error = decode_file_cancellable(&fixture("speech-en.webm"), &cancelled)
        .expect_err("a mid-decode cancel must return an error");
    assert!(matches!(error, SuramakError::Cancelled), "{error:?}");
    assert!(
        calls.load(Ordering::SeqCst) >= 6,
        "the closure must be consulted inside the packet loop"
    );
}
