//! M25B2 — the measured phone rotation run.
//!
//! A synthesised **47:00 take at 16 kHz** is pushed through the streaming
//! writer twice: once with the phone policy (rotate at 45:00, grace 120 s) and
//! once as a single reference WAV. The take has its first real pause at 45:10,
//! so the rotation must land exactly there; the parts' PCM16 data must equal
//! the reference's data byte for byte. Run explicitly:
//!
//! ```text
//! cargo test -p suramak-asr --test m25b2_phone_rotation -- --ignored --nocapture
//! ```
//!
//! It prints the real part lengths, the part file sizes and the process peak
//! RSS (VmHWM) that the M25B2 report quotes.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use suramak_asr::capture_stream::{StreamedWav, StreamingWav, WAV_HEADER_BYTES};

const RATE: u32 = 16_000;
/// 47:00.
const TOTAL_MS: i64 = 47 * 60 * 1000;
/// The first real pause after the 45:00 cap.
const PAUSE_AT_MS: i64 = 45 * 60 * 1000 + 10_000;
const PAUSE_MS: i64 = 1_000;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("suramak-m25b2-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One second of 16 kHz speech-like tone (a 220 Hz sine at −8 dBFS).
fn second_block(second: i64) -> Vec<f32> {
    let start = second * RATE as i64;
    (0..RATE)
        .map(|offset| {
            let index = start + i64::from(offset);
            let phase = std::f32::consts::TAU * 220.0 * index as f32 / RATE as f32;
            0.4 * phase.sin()
        })
        .collect()
}

fn silent_block() -> Vec<f32> {
    vec![0.0; RATE as usize]
}

fn peak_rss_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            if let Some(kb) = rest.split_whitespace().next() {
                return kb.parse().unwrap_or(0);
            }
        }
    }
    0
}

/// A sequential reader over the data chunks of a list of part files.
struct PartReader {
    parts: Vec<(File, u64)>,
    index: usize,
}

impl PartReader {
    fn open(parts: &[PathBuf]) -> Self {
        let parts = parts
            .iter()
            .map(|path| {
                let mut file = File::open(path).unwrap();
                file.seek(SeekFrom::Start(WAV_HEADER_BYTES)).unwrap();
                let data = file.metadata().unwrap().len() - WAV_HEADER_BYTES;
                (file, data)
            })
            .collect();
        Self { parts, index: 0 }
    }

    fn read_exact(&mut self, mut buffer: &mut [u8]) {
        while !buffer.is_empty() {
            let (file, remaining) = &mut self.parts[self.index];
            if *remaining == 0 {
                self.index += 1;
                assert!(self.index < self.parts.len(), "parts ended early");
                continue;
            }
            let take = (*remaining).min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..take]).unwrap();
            *remaining -= take as u64;
            buffer = &mut buffer[take..];
        }
    }
}

fn part_paths(wav: &StreamedWav) -> Vec<PathBuf> {
    wav.parts.iter().map(|part| part.path.clone()).collect()
}

#[test]
#[ignore = "47-minute measured rotation run"]
fn a_47_minute_take_rotates_once_at_the_first_silence_after_45_minutes() {
    let dir = scratch("47min");
    let path = dir.join("Townhall.wav");
    let reference_path = dir.join("reference.wav");

    let started = std::time::Instant::now();
    let mut rotating = StreamingWav::create_rotating(&path, RATE).unwrap();
    let mut reference = StreamingWav::create(&reference_path, RATE).unwrap();

    let seconds = TOTAL_MS / 1000;
    for second in 0..seconds {
        let ms = second * 1000;
        let block = if (PAUSE_AT_MS..PAUSE_AT_MS + PAUSE_MS).contains(&ms) {
            silent_block()
        } else {
            second_block(second)
        };
        rotating.push(&block).unwrap();
        reference.push(&block).unwrap();
    }
    let finished = rotating.finish().unwrap();
    reference.finish().unwrap();
    let elapsed = started.elapsed();
    let peak_kb = peak_rss_kb();

    assert_eq!(finished.parts.len(), 2, "one rotation at the one pause");
    let first = &finished.parts[0];
    let second = &finished.parts[1];
    assert_eq!(first.duration_ms(RATE), PAUSE_AT_MS);
    assert_eq!(second.duration_ms(RATE), TOTAL_MS - PAUSE_AT_MS);
    assert_eq!(first.frames, (PAUSE_AT_MS as u64) * 16);
    assert_eq!(second.frames, ((TOTAL_MS - PAUSE_AT_MS) as u64) * 16);
    assert_eq!(first.info.label(), "Part 1 of 2");
    assert_eq!(second.info.label(), "Part 2 of 2");
    assert_eq!(finished.frames, (TOTAL_MS as u64) * 16);
    assert_eq!(
        first.path.file_name().unwrap().to_str().unwrap(),
        "Townhall — Part 1 of 2.wav"
    );
    assert_eq!(
        second.path.file_name().unwrap().to_str().unwrap(),
        "Townhall — Part 2 of 2.wav"
    );

    // Bit-for-bit: the parts' data chunks in order are the reference's data.
    let parts = part_paths(&finished);
    let mut reader = PartReader::open(&parts);
    let mut reference_file = File::open(&reference_path).unwrap();
    reference_file
        .seek(SeekFrom::Start(WAV_HEADER_BYTES))
        .unwrap();
    let mut expected = vec![0u8; 1 << 20];
    let mut actual = vec![0u8; 1 << 20];
    let mut compared = 0u64;
    loop {
        let read = reference_file.read(&mut expected).unwrap();
        if read == 0 {
            break;
        }
        reader.read_exact(&mut actual[..read]);
        assert_eq!(actual[..read], expected[..read], "byte {compared} differs");
        compared += read as u64;
    }
    assert_eq!(compared, finished.frames * 2);

    println!(
        "M25B2 47:00 take: part 1 = {} ms ({} bytes), part 2 = {} ms ({} bytes)",
        first.duration_ms(RATE),
        first.bytes,
        second.duration_ms(RATE),
        second.bytes
    );
    println!(
        "M25B2: {} samples compared bit-for-bit; wall {:.1} s; peak RSS (VmHWM) {} KB = {:.1} MiB",
        finished.frames,
        elapsed.as_secs_f64(),
        peak_kb,
        peak_kb as f64 / 1024.0
    );

    // VmHWM lives in /proc: on macOS the sampler reads nothing, and the peak-RSS
    // figure is a LINUX measurement (the gate's numbers came from this box).
    // The rotation itself is platform-independent — the bit-for-bit comparison
    // above is the assertion that matters here.
    if cfg!(target_os = "linux") {
        assert!(peak_kb > 0, "the Linux RSS sampler must report a peak");
    } else {
        println!("M25B2: peak RSS is a Linux measurement — not asserted on this platform");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_phone_policy_is_the_45_minute_android_cap() {
    use suramak_asr::capture_stream::RotationPolicy;
    assert_eq!(RotationPolicy::PHONE.cap_ms, 45 * 60 * 1000);
    assert_eq!(RotationPolicy::PHONE.grace_ms, 120_000);
    assert_eq!(RotationPolicy::PHONE.silence_ms, 600);
}

#[test]
fn a_twelve_part_take_can_exist_but_a_thirteenth_is_refused() {
    // The ceiling is a named constant, not an accident of the loop.
    assert_eq!(suramak_asr::capture_stream::MAX_PARTS, 12);
    let message = suramak_asr::capture_stream::part_ceiling_reason(45 * 60 * 1000);
    assert!(message.contains("longer than 9 hours"), "{message}");
}
