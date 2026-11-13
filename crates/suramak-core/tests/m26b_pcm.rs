//! M26B acceptance at the sample level: the streaming decode writes exactly the
//! bytes the old in-memory path produced, the WAV fast path matches it too, and
//! a refused/cancelled decode leaves no temp file.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use suramak_core::decode::{
    decode_16khz_mono_in_memory_for_tests, decode_to_pcm16k_mono,
    decode_to_pcm16k_mono_cancellable, decode_to_pcm16k_mono_with_free_space, write_wav_i16,
};
use suramak_core::pcm::{self, PcmSlice};
use suramak_core::SuramakError;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/audio")
        .join(name)
}

fn temp_pcm(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("suramak_{}_{name}.pcm32", std::process::id()))
}

fn assert_same_bits(left: &[f32], right: &[f32], label: &str) {
    assert_eq!(left.len(), right.len(), "{label}: sample count");
    if let Some(index) = left
        .iter()
        .zip(right)
        .position(|(a, b)| a.to_bits() != b.to_bits())
    {
        panic!(
            "{label}: first differing sample at {index}: {} ({:#010x}) vs {} ({:#010x})",
            left[index],
            left[index].to_bits(),
            right[index],
            right[index].to_bits()
        );
    }
}

/// The acceptance rule: streaming decode → `.pcm32` → `PcmSlice` is
/// sample-for-sample the old decode-into-`Vec` + resample-into-`Vec` path.
/// This covers 16 kHz (no resampler), 24 kHz MP3 (rubato) and a generated
/// 44.1 kHz stereo WAV (mixdown + rubato).
#[test]
fn the_streaming_decode_is_byte_identical_to_the_in_memory_path() {
    let dir = std::env::temp_dir().join(format!("suramak-m26b-bits-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let generated = dir.join("stereo-44k.wav");
    let frames = 44_100;
    let mut stereo = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let t = i as f32 / 44_100.0;
        let left = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.4;
        let right = (2.0 * std::f32::consts::PI * 660.0 * t).sin() * 0.3;
        stereo.push(left);
        stereo.push(right);
    }
    write_wav_i16(&generated, &stereo, 44_100, 2).unwrap();

    for (name, path) in [
        ("en.wav", fixture("en.wav")),
        ("en.mp3", fixture("en.mp3")),
        ("id.wav", fixture("id.wav")),
        ("stereo-44k.wav", generated.clone()),
    ] {
        let expected = decode_16khz_mono_in_memory_for_tests(&path).expect("in-memory reference");
        let out = temp_pcm(&format!("bits-{}", name.replace('.', "-")));
        let streamed = decode_to_pcm16k_mono(&path, &out).expect("streaming decode");
        assert_same_bits(streamed.as_slice(), &expected, name);
        assert_eq!(
            streamed.len(),
            expected.len(),
            "{name}: the temp file must be exactly the ideal duration"
        );
        drop(streamed);
        assert!(
            !out.exists(),
            "{name}: the temp file must be gone after drop"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// The fast path maps a 16 kHz mono PCM16 WAV's data chunk and converts i16
/// with the same rule as the decoder; its samples are bit-identical to both the
/// streaming and the old in-memory path.
#[test]
fn the_wav_fast_path_is_byte_identical_to_the_decode_path() {
    let path = fixture("en.wav");
    let fast = PcmSlice::from_recording_wav(&path).expect("our fixture is the fast-path shape");

    let out = temp_pcm("fast-decode");
    let streamed = decode_to_pcm16k_mono(&path, &out).expect("streaming decode");
    assert_same_bits(fast.as_slice(), streamed.as_slice(), "fast vs streaming");

    let in_memory = decode_16khz_mono_in_memory_for_tests(&path).expect("in-memory");
    assert_same_bits(fast.as_slice(), &in_memory, "fast vs in-memory");
    drop(streamed);
}

/// The preflight refuses with `temp_disk_full` before the temp file exists, and
/// the refusal names both numbers.
#[test]
fn a_full_temp_volume_refuses_before_creating_the_file() {
    let out = temp_pcm("preflight");
    let _ = std::fs::remove_file(&out);
    let error = decode_to_pcm16k_mono_with_free_space(&fixture("en.wav"), &out, Some(0), &|| false)
        .expect_err("zero free bytes must refuse");
    let message = error.to_string();
    assert!(message.starts_with("temp_disk_full:"), "{message}");
    assert!(message.contains("only 0 are free"), "{message}");
    assert!(
        !out.exists(),
        "a refused preflight must not leave a partial temp file"
    );
}

/// A cancel mid-decode returns `Cancelled` and removes the partial file: the
/// decode checks the token per packet, after the writer has opened the output.
#[test]
fn a_cancelled_decode_leaves_no_temp_file() {
    let out = temp_pcm("cancel");
    let _ = std::fs::remove_file(&out);
    let checks = AtomicUsize::new(0);
    let cancelled = || checks.fetch_add(1, Ordering::Relaxed) >= 12;
    // A long-enough MP3 to span many packets; the cancel lands mid-file.
    let error = decode_to_pcm16k_mono_cancellable(&fixture("en.mp3"), &out, &cancelled)
        .expect_err("the token must stop the decode");
    assert!(
        matches!(error, SuramakError::Cancelled),
        "unexpected: {error}"
    );
    assert!(
        !out.exists(),
        "a cancelled decode must not leave the partial temp file"
    );
    assert!(
        checks.load(Ordering::Relaxed) > 1,
        "the cancellation must be observed inside the packet loop"
    );
}

/// `temp_pcm_path` produces the exact shape the startup sweep owns, and nothing
/// else (`pcm::TEMP_PCM_PREFIX` + `.pcm32`).
#[test]
fn the_temp_path_is_the_sweeps_shape() {
    let path = pcm::temp_pcm_path();
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("suramak_"), "{name}");
    assert!(name.ends_with(".pcm32"), "{name}");
    assert_eq!(path.parent(), Some(pcm::temp_pcm_dir().as_path()));
}
