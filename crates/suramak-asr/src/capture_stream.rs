//! Streaming 16 kHz mono PCM16 WAV writer for the Android capture path.
//!
//! The desktop path is: cpal delivers device-rate mono samples, the whole take
//! is buffered, [`crate::capture::resample_16k`] resamples it in one pass, and
//! [`crate::capture::write_recording_wav`] writes it with hound. A phone records
//! for thirty minutes or more, so buffering the take (48 kHz f32 ≈ 345 MB for
//! half an hour) is not an option — and a process killed while the screen is
//! locked must not lose the whole recording.
//!
//! This module performs the **same arithmetic incrementally**: the mono,
//! device-rate stream is cut into the same 1024-sample chunks
//! `suramak_core::resample::to_16khz_mono` uses, fed through
//! `rubato::FftFixedIn` with the same parameters, and the 16 kHz output is
//! appended to a canonical 44-byte-header PCM16 WAV as it arrives. The final
//! file is **byte-for-byte the desktop tail's output** for the same input
//! samples (pinned by the unit tests against `write_recording_wav`).
//!
//! Because the header is written first and the sizes are patched last, a file
//! interrupted by process death is still a WAV with all of its audio after
//! [`StreamingWav::repair`] — the recovery path the mic journal drives.
//!
//! Compiled on every platform (no cpal, no device): the tests run on the host.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rubato::{FftFixedIn, Resampler};

use crate::whisper::AsrError;

/// The canonical PCM16 WAV header length (RIFF/fmt/data, no extra chunks) —
/// exactly what hound writes for the desktop recordings.
pub const WAV_HEADER_BYTES: u64 = 44;

/// The recording rate, shared with the desktop path.
pub const TARGET_RATE: u32 = 16_000;

/// The chunk size `to_16khz_mono` feeds rubato; matching it is what makes the
/// streamed output bit-identical to the one-shot desktop resample.
const CHUNK: usize = 1024;

/// One finished recording as it sits on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamedWav {
    pub path: PathBuf,
    /// 16 kHz mono samples in the data chunk.
    pub frames: u64,
    /// File size in bytes, header included.
    pub bytes: u64,
    pub device_rate: u32,
}

/// Appends 16 kHz mono PCM16 samples to a canonical WAV file.
///
/// Construction writes the header with zero sizes (a crash before the first
/// sample leaves a valid, empty WAV); [`finish`](Self::finish) pads the final
/// resampler chunk, truncates to the exact input duration, and patches the
/// sizes — the same tail `write_recording_wav` produces.
pub struct StreamingWav {
    path: PathBuf,
    file: File,
    /// `None` when the device already records at 16 kHz (core's fast path).
    resampler: Option<FftFixedIn<f32>>,
    device_rate: u32,
    /// Mono device-rate samples not yet forming a full 1024-sample chunk.
    pending: Vec<f32>,
    /// Real (not padding) device-rate samples pushed so far.
    input_samples: u64,
    /// 16 kHz samples written to the data chunk so far.
    pcm_written: u64,
    /// Reused PCM byte buffer (never grows past one resampler chunk's output).
    scratch: Vec<u8>,
}

impl StreamingWav {
    /// Creates `path` (and its parent) with a valid, empty WAV header.
    ///
    /// `device_rate` is the rate of the samples [`push`](Self::push) will
    /// receive; it must be non-zero. An existing file is truncated: a recording
    /// path is reserved for exactly one take.
    pub fn create(path: &Path, device_rate: u32) -> Result<Self, AsrError> {
        if device_rate == 0 {
            return Err(AsrError::Capture(
                "the capture device reported a zero sample rate".to_string(),
            ));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                AsrError::Capture(format!(
                    "could not create the recording directory {}: {err}",
                    parent.display()
                ))
            })?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|err| {
                AsrError::Capture(format!(
                    "could not create the recording {}: {err}",
                    path.display()
                ))
            })?;
        file.write_all(&wav_header(TARGET_RATE, 0))
            .map_err(|err| write_error(path, err))?;
        file.seek(SeekFrom::Start(WAV_HEADER_BYTES))
            .map_err(|err| write_error(path, err))?;
        let resampler = if device_rate == TARGET_RATE {
            None
        } else {
            Some(
                FftFixedIn::<f32>::new(device_rate as usize, TARGET_RATE as usize, CHUNK, 2, 1)
                    .map_err(|err| {
                        AsrError::Capture(format!(
                            "could not start the {device_rate} Hz -> 16 kHz resampler: {err}"
                        ))
                    })?,
            )
        };
        Ok(Self {
            path: path.to_path_buf(),
            file,
            resampler,
            device_rate,
            pending: Vec::with_capacity(CHUNK),
            input_samples: 0,
            pcm_written: 0,
            scratch: Vec::with_capacity(CHUNK),
        })
    }

    /// The file being written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Device sample rate this stream was opened for.
    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    /// Pushes a block of **mono device-rate** samples (the callback's mixdown).
    ///
    /// Non-blocking apart from the disk write: the resampler consumes complete
    /// 1024-sample chunks, and a partial chunk waits in memory for the next
    /// call (or for [`finish`](Self::finish)'s zero padding).
    pub fn push(&mut self, samples: &[f32]) -> Result<(), AsrError> {
        if samples.is_empty() {
            return Ok(());
        }
        self.input_samples += samples.len() as u64;
        if self.resampler.is_none() {
            // 16 kHz native: the data chunk is the input, sample for sample.
            self.write_pcm(samples)?;
            return Ok(());
        }
        for &sample in samples {
            self.pending.push(sample);
            if self.pending.len() == CHUNK {
                self.process_chunk()?;
            }
        }
        Ok(())
    }

    /// Samples pushed (before resampling) so far.
    pub fn input_samples(&self) -> u64 {
        self.input_samples
    }

    /// 16 kHz samples written so far (before [`finish`](Self::finish)'s exact
    /// truncation — the final count is on [`StreamedWav`]).
    pub fn frames_written(&self) -> u64 {
        self.pcm_written
    }

    /// Completes the file: pads and resamples the final partial chunk, trims
    /// to the input's exact 16 kHz duration, patches the header and syncs.
    pub fn finish(mut self) -> Result<StreamedWav, AsrError> {
        if self.resampler.is_some() && !self.pending.is_empty() {
            // The desktop path pads the last chunk with zeros too
            // (`to_16khz_mono`), so this is the same tail, not an approximation.
            self.pending.resize(CHUNK, 0.0);
            self.process_chunk()?;
        }
        let expected = self.expected_frames();
        if self.pcm_written > expected {
            // rubato emits a full output chunk for the padded tail; core
            // truncates it, and so do we — on disk, before patching the sizes.
            // The reverse case (rubato's FFT tail is a few samples short of
            // `expected`) is left as-is: the desktop `to_16khz_mono` only
            // truncates, it never pads, so padding here would break the
            // byte-for-byte contract.
            let keep = WAV_HEADER_BYTES + expected * 2;
            self.file
                .set_len(keep)
                .map_err(|err| write_error(&self.path, err))?;
            self.pcm_written = expected;
        }
        patch_header(&mut self.file, &self.path, TARGET_RATE, self.pcm_written)?;
        self.file
            .sync_all()
            .map_err(|err| write_error(&self.path, err))?;
        Ok(StreamedWav {
            path: self.path.clone(),
            frames: self.pcm_written,
            bytes: WAV_HEADER_BYTES + self.pcm_written * 2,
            device_rate: self.device_rate,
        })
    }

    /// The exact 16 kHz frame count for the samples pushed so far — core's
    /// `round(len * 16000 / rate)` rule.
    fn expected_frames(&self) -> u64 {
        if self.device_rate == TARGET_RATE {
            return self.input_samples;
        }
        ((self.input_samples as f64 * f64::from(TARGET_RATE)) / f64::from(self.device_rate)).round()
            as u64
    }

    /// Resamples the full 1024-sample `pending` chunk and appends the result.
    fn process_chunk(&mut self) -> Result<(), AsrError> {
        let resampler = self
            .resampler
            .as_mut()
            .expect("process_chunk is only called with a resampler");
        let chunk = std::mem::take(&mut self.pending);
        debug_assert_eq!(chunk.len(), CHUNK);
        let output = resampler.process(&[chunk], None).map_err(|err| {
            AsrError::Capture(format!("the 16 kHz resampler failed mid-recording: {err}"))
        })?;
        if let Some(samples) = output.first() {
            self.write_pcm(samples)?;
        }
        self.pending = Vec::with_capacity(CHUNK);
        Ok(())
    }

    /// Converts f32 samples to PCM16 exactly as `write_wav_i16` does and
    /// appends them to the data chunk.
    fn write_pcm(&mut self, samples: &[f32]) -> Result<(), AsrError> {
        self.scratch.clear();
        self.scratch.reserve(samples.len() * 2);
        for &sample in samples {
            let scaled = (sample * f32::from(i16::MAX)).round();
            let value = scaled.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
            self.scratch.extend_from_slice(&value.to_le_bytes());
        }
        self.file
            .write_all(&self.scratch)
            .map_err(|err| write_error(&self.path, err))?;
        self.pcm_written += samples.len() as u64;
        Ok(())
    }
}

impl Drop for StreamingWav {
    fn drop(&mut self) {
        // A dropped, unfinished stream is the crash case: flush what is there
        // so `repair` can see every byte the kernel accepted. Sizes stay zero
        // until `repair` (or finish) patches them.
        let _ = self.file.flush();
    }
}

/// Repairs a WAV left behind by a process kill (or by a service that never got
/// to call [`StreamingWav::finish`]) and returns its frame count.
///
/// Only this module's own layout (PCM16, mono, 16 kHz, 44-byte header) is
/// accepted: a foreign file is refused rather than overwritten. A torn final
/// byte (a write cut mid-sample) is dropped. Idempotent on a finished file.
pub fn repair(path: &Path) -> Result<StreamedWav, AsrError> {
    let len = std::fs::metadata(path)
        .map_err(|err| {
            AsrError::Capture(format!(
                "the interrupted recording {} is gone: {err}",
                path.display()
            ))
        })?
        .len();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|err| write_error(path, err))?;
    if len == 0 {
        // The create was interrupted before the header landed: write a valid
        // empty one rather than leaving an unreadable zero-byte file.
        file.set_len(0).map_err(|err| write_error(path, err))?;
        file.write_all(&wav_header(TARGET_RATE, 0))
            .map_err(|err| write_error(path, err))?;
        file.sync_all().map_err(|err| write_error(path, err))?;
        return Ok(StreamedWav {
            path: path.to_path_buf(),
            frames: 0,
            bytes: WAV_HEADER_BYTES,
            device_rate: TARGET_RATE,
        });
    }
    if len < WAV_HEADER_BYTES {
        return Err(AsrError::Capture(format!(
            "{} is shorter than a WAV header and is not a recording this build wrote",
            path.display()
        )));
    }
    let mut header = [0u8; WAV_HEADER_BYTES as usize];
    {
        use std::io::Read;
        file.read_exact(&mut header)
            .map_err(|err| write_error(path, err))?;
    }
    validate_header(&header, path)?;

    let mut data_bytes = len - WAV_HEADER_BYTES;
    if data_bytes % 2 != 0 {
        // A kill between the two bytes of a sample: drop the torn half.
        data_bytes -= 1;
        file.set_len(WAV_HEADER_BYTES + data_bytes)
            .map_err(|err| write_error(path, err))?;
    }
    patch_header(&mut file, path, TARGET_RATE, data_bytes / 2)?;
    file.sync_all().map_err(|err| write_error(path, err))?;
    Ok(StreamedWav {
        path: path.to_path_buf(),
        frames: data_bytes / 2,
        bytes: WAV_HEADER_BYTES + data_bytes,
        device_rate: TARGET_RATE,
    })
}

/// The canonical 44-byte PCM16 mono WAV header (hound's layout, which the
/// desktop recordings use).
fn wav_header(rate: u32, data_bytes: u64) -> [u8; WAV_HEADER_BYTES as usize] {
    let mut header = [0u8; WAV_HEADER_BYTES as usize];
    let data = u32::try_from(data_bytes).unwrap_or(u32::MAX);
    let riff = 36u32.saturating_add(data);
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&riff.to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes());
    header[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    header[22..24].copy_from_slice(&1u16.to_le_bytes()); // mono
    header[24..28].copy_from_slice(&rate.to_le_bytes());
    header[28..32].copy_from_slice(&rate.saturating_mul(2).to_le_bytes()); // byte rate
    header[32..34].copy_from_slice(&2u16.to_le_bytes()); // block align
    header[34..36].copy_from_slice(&16u16.to_le_bytes()); // bits
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data.to_le_bytes());
    header
}

/// Accepts only a PCM16 mono 16 kHz RIFF/WAVE (this module's own output).
fn validate_header(header: &[u8; WAV_HEADER_BYTES as usize], path: &Path) -> Result<(), AsrError> {
    let rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    let channels = u16::from_le_bytes([header[22], header[23]]);
    let bits = u16::from_le_bytes([header[34], header[35]]);
    let format = u16::from_le_bytes([header[20], header[21]]);
    let valid = &header[0..4] == b"RIFF"
        && &header[8..12] == b"WAVE"
        && &header[12..16] == b"fmt "
        && format == 1
        && channels == 1
        && bits == 16
        && rate == TARGET_RATE
        && &header[36..40] == b"data";
    if valid {
        Ok(())
    } else {
        Err(AsrError::Capture(format!(
            "{} is not a 16 kHz mono PCM16 recording this build wrote; refusing to repair it",
            path.display()
        )))
    }
}

/// Rewrites the two size fields for `frames` 16 kHz samples.
fn patch_header(file: &mut File, path: &Path, rate: u32, frames: u64) -> Result<(), AsrError> {
    let data_bytes = frames.saturating_mul(2);
    let header = wav_header(rate, data_bytes);
    file.seek(SeekFrom::Start(0))
        .map_err(|err| write_error(path, err))?;
    file.write_all(&header)
        .map_err(|err| write_error(path, err))?;
    file.seek(SeekFrom::End(0))
        .map_err(|err| write_error(path, err))?;
    Ok(())
}

fn write_error(path: &Path, err: std::io::Error) -> AsrError {
    AsrError::Capture(format!("could not write {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "suramak-capture-stream-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sine(rate: u32, seconds: f32, freq: f32) -> Vec<f32> {
        let count = (rate as f32 * seconds) as usize;
        (0..count)
            .map(|index| {
                let t = index as f32 / rate as f32;
                (t * freq * std::f32::consts::TAU).sin() * 0.4
            })
            .collect()
    }

    /// **The contract test:** the streamed 48 kHz take is byte-for-byte the
    /// desktop tail's WAV for the same samples.
    #[test]
    fn a_streamed_48k_take_is_byte_identical_to_the_desktop_output() {
        let dir = root("48k");
        let streamed_path = dir.join("streamed.wav");
        let desktop_path = dir.join("desktop.wav");
        let mut input = sine(48_000, 3.7, 440.0);
        // A late block smaller than a frame is still part of the take; it must
        // not be dropped by the streaming chunker.
        let tail: Vec<f32> = input[..7].to_vec();
        input.extend_from_slice(&tail);

        let mut stream = StreamingWav::create(&streamed_path, 48_000).unwrap();
        // Irregular blocks: what an AAudio callback actually delivers.
        for block in input.chunks(441) {
            stream.push(block).unwrap();
        }
        let finished = stream.finish().unwrap();

        let resampled = crate::capture::resample_16k(&input, 48_000).unwrap();
        crate::capture::write_recording_wav(&desktop_path, &resampled, TARGET_RATE).unwrap();

        let streamed = std::fs::read(&streamed_path).unwrap();
        let desktop = std::fs::read(&desktop_path).unwrap();
        assert_eq!(
            streamed, desktop,
            "the streaming path must produce the desktop WAV byte for byte"
        );
        assert_eq!(finished.frames * 2 + WAV_HEADER_BYTES, finished.bytes);
        assert_eq!(finished.bytes as usize, streamed.len());

        // The tail sample (7 samples) is part of the take, not discarded.
        assert_eq!(finished.frames, resampled.len() as u64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The AAudio callback shape:** a 48 kHz **stereo** interleaved block,
    /// mixed down by the capture callback's own helper, then streamed to 16 kHz
    /// PCM16 — byte-identical to the desktop tail for the same source. This is
    /// the phone's real input shape (AAudio usually opens stereo at 48 k).
    #[test]
    fn a_stereo_48k_aaudio_callback_mixdown_matches_the_desktop_output() {
        let dir = root("aaudio-stereo");
        let path = dir.join("streamed.wav");
        let desktop = dir.join("desktop.wav");
        let left = sine(48_000, 2.3, 440.0);
        let right = sine(48_000, 2.3, 660.0);
        let mut interleaved = Vec::with_capacity(left.len() * 2);
        for (l, r) in left.iter().zip(right.iter()) {
            interleaved.push(*l);
            interleaved.push(*r);
        }
        let mono = crate::capture::mono_mixdown(&interleaved, 2);
        let mut stream = StreamingWav::create(&path, 48_000).unwrap();
        // AAudio delivers irregular callback sizes, not one clean buffer.
        for block in mono.chunks(512) {
            stream.push(block).unwrap();
        }
        stream.finish().unwrap();
        let resampled = crate::capture::resample_16k(&mono, 48_000).unwrap();
        crate::capture::write_recording_wav(&desktop, &resampled, TARGET_RATE).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            std::fs::read(&desktop).unwrap(),
            "the callback's mixdown + streaming tail must equal the desktop WAV"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A device that already records at 16 kHz short-circuits, like core's
    /// resampler.
    #[test]
    fn a_native_16k_stream_is_the_desktop_writer_unchanged() {
        let dir = root("16k");
        let path = dir.join("native.wav");
        let desktop = dir.join("desktop.wav");
        let samples = sine(16_000, 1.3, 300.0);
        let mut stream = StreamingWav::create(&path, 16_000).unwrap();
        for block in samples.chunks(100) {
            stream.push(block).unwrap();
        }
        let finished = stream.finish().unwrap();
        crate::capture::write_recording_wav(&desktop, &samples, TARGET_RATE).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            std::fs::read(&desktop).unwrap()
        );
        assert_eq!(finished.frames, samples.len() as u64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The 44.1 kHz family resamples through the same chunker (a laptop-class
    /// device is not the phone case, but the math must not assume 48 k).
    #[test]
    fn a_44k_stream_is_byte_identical_to_the_desktop_output() {
        let dir = root("44k");
        let path = dir.join("streamed.wav");
        let desktop = dir.join("desktop.wav");
        let samples = sine(44_100, 2.05, 523.0);
        let mut stream = StreamingWav::create(&path, 44_100).unwrap();
        stream.push(&samples).unwrap();
        stream.finish().unwrap();
        let resampled = crate::capture::resample_16k(&samples, 44_100).unwrap();
        crate::capture::write_recording_wav(&desktop, &resampled, TARGET_RATE).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            std::fs::read(&desktop).unwrap()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A kill before `finish` leaves a WAV whose header still says zero: repair
    /// patches it and every captured frame survives.
    #[test]
    fn repair_makes_a_killed_take_playable_and_keeps_every_frame() {
        let dir = root("repair");
        let path = dir.join("killed.wav");
        let samples = sine(48_000, 2.0, 220.0);
        {
            let mut stream = StreamingWav::create(&path, 48_000).unwrap();
            for block in samples.chunks(3200) {
                stream.push(block).unwrap();
            }
            // No finish: the process died here.
        }
        let raw_len = std::fs::metadata(&path).unwrap().len();
        assert!(raw_len > WAV_HEADER_BYTES);
        // The header still claims zero bytes.
        let before = std::fs::read(&path).unwrap();
        assert_eq!(&before[40..44], &0u32.to_le_bytes());

        let repaired = repair(&path).unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(after.len() as u64, raw_len);
        assert_eq!(
            u32::from_le_bytes([after[40], after[41], after[42], after[43]]) as u64,
            repaired.frames * 2
        );
        assert_eq!(
            u32::from_le_bytes([after[4], after[5], after[6], after[7]]) as u64,
            raw_len - 8
        );
        let decoded = suramak_core::decode::decode_file(&path).unwrap();
        assert_eq!(decoded.rate, TARGET_RATE);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.samples.len() as u64, repaired.frames);
        // Repair is idempotent.
        assert_eq!(repair(&path).unwrap().frames, repaired.frames);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A write cut between the two bytes of a sample is dropped, not decoded as
    /// noise.
    #[test]
    fn repair_drops_a_torn_final_sample() {
        let dir = root("torn");
        let path = dir.join("torn.wav");
        {
            let mut stream = StreamingWav::create(&path, 16_000).unwrap();
            stream.push(&[0.25, -0.25, 0.5]).unwrap();
        }
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.push(0x7f); // half an i16
        std::fs::write(&path, &bytes).unwrap();
        let repaired = repair(&path).unwrap();
        assert_eq!(repaired.frames, 3);
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            WAV_HEADER_BYTES + 6
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An empty take is a valid empty WAV, not a corrupt file.
    #[test]
    fn an_empty_take_repairs_to_a_valid_empty_wav() {
        let dir = root("empty");
        let path = dir.join("empty.wav");
        let finished = StreamingWav::create(&path, 48_000)
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(finished.frames, 0);
        assert_eq!(finished.bytes, WAV_HEADER_BYTES);
        let repaired = repair(&path).unwrap();
        assert_eq!(repaired.frames, 0);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), WAV_HEADER_BYTES);
        let header = std::fs::read(&path).unwrap();
        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(&header[36..40], b"data");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A foreign WAV is refused, never rewritten (the recovery path may only
    /// touch files this build wrote).
    #[test]
    fn repair_refuses_a_foreign_wav() {
        let dir = root("foreign");
        let path = dir.join("foreign.wav");
        std::fs::write(&path, b"RIFF\x00\x00\x00\x00WAVEjunk").unwrap();
        let err = repair(&path).unwrap_err().to_string();
        assert!(err.contains("this build wrote"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
