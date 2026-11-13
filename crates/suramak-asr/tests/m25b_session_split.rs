//! M25B — the 50-minute split run.
//!
//! A synthesised 50-minute session (45 minutes of speech, a real 1 s pause
//! straddling the Android cap, then 5 minutes of speech) is split with the
//! shipping rule and the Android 45-minute cap. The test asserts:
//!
//! * every part is at or under the cap;
//! * the parts' sample counts sum to the input's sample count;
//! * decoding the parts in order reproduces the input sample for sample.
//!
//! It is `#[ignore]` because the fixture is ~96 MB; run it explicitly and read
//! the printed part lengths + peak RSS (VmHWM) for the M25B report:
//!
//! ```text
//! cargo test -p suramak-asr --test m25b_session_split -- --ignored --nocapture
//! ```

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use suramak_core::decode;
use suramak_core::session;
use suramak_core::session::PartInfo;

const RATE: u32 = 16_000;
const TOTAL_MS: i64 = 50 * 60 * 1000;
/// Speech 0..44:59.5, a real 1 s pause straddling 45:00, speech to 50:00.
const PAUSE_START_MS: i64 = 44 * 60 * 1000 + 59_500;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "suramak-m25b-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn sine_into(out: &mut Vec<i16>, frames: usize, phase: &mut f32) {
    for _ in 0..frames {
        *phase += 2.0 * std::f32::consts::PI * 220.0 / RATE as f32;
        if *phase > std::f32::consts::TAU {
            *phase -= std::f32::consts::TAU;
        }
        out.push((phase.sin() * 0.4 * f32::from(i16::MAX)).round() as i16);
    }
}

fn silence_into(out: &mut Vec<i16>, frames: usize) {
    out.resize(out.len() + frames, 0);
}

/// Writes a 16 kHz mono PCM16 WAV in 1 s chunks (no full 50-minute buffer).
fn write_fixture(path: &Path) {
    let mut file = std::fs::File::create(path).expect("create fixture");
    let data_bytes = (TOTAL_MS as u64) * u64::from(RATE) / 1000 * 2;
    let riff = 36 + data_bytes as u32;
    file.write_all(b"RIFF").unwrap();
    file.write_all(&riff.to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&RATE.to_le_bytes()).unwrap();
    file.write_all(&(RATE * 2).to_le_bytes()).unwrap();
    file.write_all(&2u16.to_le_bytes()).unwrap();
    file.write_all(&16u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&(data_bytes as u32).to_le_bytes()).unwrap();
    let pause_frames = (PAUSE_START_MS as f64 * f64::from(RATE) / 1000.0) as usize;
    let total_frames = (TOTAL_MS as f64 * f64::from(RATE) / 1000.0) as usize;
    let silence_frames = RATE as usize; // exactly 1 s
    let mut written = 0usize;
    let mut chunk = Vec::with_capacity(RATE as usize);
    let mut phase = 0.0f32;
    while written < total_frames {
        let n = (RATE as usize).min(total_frames - written);
        chunk.clear();
        let mut bytes = Vec::with_capacity(n * 2);
        let in_pause = written < pause_frames + silence_frames && written + n > pause_frames;
        if in_pause {
            let before = pause_frames.saturating_sub(written).min(n);
            sine_into(&mut chunk, before, &mut phase);
            let silence_end = (pause_frames + silence_frames).min(written + n);
            silence_into(&mut chunk, silence_end - (written + before));
            let after = n - before - (silence_end - (written + before));
            sine_into(&mut chunk, after, &mut phase);
        } else {
            sine_into(&mut chunk, n, &mut phase);
        }
        for sample in &chunk {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        file.write_all(&bytes).unwrap();
        written += n;
    }
    file.flush().unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
}

fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

fn concat_samples(paths: &[PathBuf]) -> Vec<f32> {
    let mut out = Vec::new();
    for path in paths {
        let buffer = decode::decode_file(path).expect("decode part");
        out.extend_from_slice(&buffer.samples);
    }
    out
}

#[test]
#[ignore = "50-minute synthesized fixture (~96 MB); run explicitly for the M25B report"]
fn a_fifty_minute_session_splits_under_a_45_minute_cap_with_the_same_samples() {
    // M26A: the cap is an input now, not a global constant. This fixture pins a
    // 45-minute device class (the old Android cap) so the 50-minute split rule
    // stays measurable without generating an 8-hour file on a large host.
    const CAP_MS: i64 = 45 * 60 * 1000;
    let dir = scratch("50m");
    let input = dir.join("Long session.wav");
    write_fixture(&input);
    let input_probe = decode::probe_duration_ms(&input).expect("probe the fixture");
    assert_eq!(input_probe, TOTAL_MS as u64);

    let output = session::split_audio_file(&input, &dir.join("parts"), CAP_MS)
        .expect("split")
        .expect("50 minutes is over the 45-minute cap");

    let cap_samples = (CAP_MS as f64 * f64::from(RATE) / 1000.0) as usize;
    let mut total = 0usize;
    for (index, part) in output.parts.iter().enumerate() {
        let samples = part.span.samples();
        total += samples;
        let ms = part.span.duration_ms(RATE);
        println!(
            "M25B_PART_{} label=\"{}\" samples={} ms={} path={}",
            index + 1,
            part.span.info.label(),
            samples,
            ms,
            part.path.display()
        );
        assert!(
            samples <= cap_samples,
            "{} is {} samples, over the cap's {}",
            part.span.info.label(),
            samples,
            cap_samples
        );
        assert_eq!(
            part.span.info,
            PartInfo::new(index as u32 + 1, output.parts.len() as u32)
        );
    }
    let input_samples = decode::decode_file(&input).expect("decode input").samples;
    assert_eq!(
        total,
        input_samples.len(),
        "the parts cover every input sample"
    );
    let joined = concat_samples(
        &output
            .parts
            .iter()
            .map(|part| part.path.clone())
            .collect::<Vec<_>>(),
    );
    assert_eq!(joined.len(), input_samples.len());
    assert_eq!(joined, input_samples, "sample-exact concatenation");
    println!("M25B_PART_COUNT={}", output.parts.len());
    println!("M25B_TOTAL_SAMPLES={total}");
    println!("M25B_PLAN=\"{}\"", output.plan.summary());
    println!("M25B_PEAK_RSS_KB={:?}", peak_rss_kb());
    let _ = std::fs::remove_dir_all(&dir);
}
