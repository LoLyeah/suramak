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
//! # M25B2 — the phone rotates its recording too
//!
//! [`PartRotator`] sits **between the resampler output and the WAV files**:
//! every 16 kHz slice the resampler emits goes through it, and a rotation cuts
//! that one continuous stream at a sample boundary. Each part is therefore a
//! slice of the same resampled stream, so concatenating the parts is the
//! uninterrupted 16 kHz take **bit for bit** — no resampler state crosses a
//! cut, because no cut is ever inside the resampler.
//!
//! The cut rule is the desktop rule of `suramak_core::session`, applied live:
//! at the part cap the rotator looks for a ≥ 600 ms silence, with a ≤ 120 s
//! grace; if none arrives it cuts at the least-energetic 50 ms window of the
//! last five seconds and flags the part. Silence is an RMS gate over 50 ms
//! windows, thresholded at three times the take's own noise floor (measured
//! over the first two seconds and clamped), logged once per part.
//!
//! Compiled on every platform (no cpal, no device): the tests run on the host.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rubato::{FftFixedIn, Resampler};

use suramak_core::session::{
    clock, part_stem, PartInfo, SplitReason, SILENCE_RMS, SPLIT_GRACE_MS, SPLIT_SILENCE_MS,
};

use crate::whisper::AsrError;

/// The canonical PCM16 WAV header length (RIFF/fmt/data, no extra chunks) —
/// exactly what hound writes for the desktop recordings.
pub const WAV_HEADER_BYTES: u64 = 44;

/// The recording rate, shared with the desktop path.
pub const TARGET_RATE: u32 = 16_000;

/// The chunk size `to_16khz_mono` feeds rubato; matching it is what makes the
/// streamed output bit-identical to the one-shot desktop resample.
const CHUNK: usize = 1024;

/// The silence analysis window on the 16 kHz stream.
pub const PART_FRAME_MS: i64 = 50;

/// The noise floor is measured over this much of the take.
pub const PART_FLOOR_MS: i64 = 2_000;

/// The threshold never drops below this (≈ −66 dBFS): digital silence at the
/// start of a take must not make the gate deaf to a real room.
pub const PART_MIN_THRESHOLD_RMS: f32 = 0.0005;

/// A take may not grow past this many parts (12 × 45 min = 9 h). Beyond it the
/// recorder refuses with one honest sentence instead of renaming files forever.
pub const MAX_PARTS: u32 = 12;

/// One finished recording as it sits on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamedWav {
    /// Part 1 (the whole take when it fits).
    pub path: PathBuf,
    /// 16 kHz mono samples across every part.
    pub frames: u64,
    /// File bytes across every part, headers included.
    pub bytes: u64,
    pub device_rate: u32,
    /// One entry per part, in order. Always at least one entry.
    pub parts: Vec<PartWav>,
}

/// One part of a rotating take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartWav {
    pub path: PathBuf,
    /// 16 kHz samples in this part.
    pub frames: u64,
    /// File bytes, header included.
    pub bytes: u64,
    /// The cut that closed this part (`End` for the last part; `NoPause` when
    /// the cut came from the least-energetic window instead of a pause).
    pub reason: SplitReason,
    /// `Part 2 of 3`; `Part 1 of 1` for a take that fits.
    pub info: PartInfo,
}

impl PartWav {
    /// The cut that closed this part had no real pause behind it.
    pub fn cut_without_pause(&self) -> bool {
        self.reason == SplitReason::NoPause
    }

    pub fn duration_ms(&self, rate: u32) -> i64 {
        if rate == 0 {
            0
        } else {
            (self.frames as i64) * 1000 / i64::from(rate)
        }
    }
}

/// How the streaming writer splits a take. The shipping policy is [`PHONE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RotationPolicy {
    pub cap_ms: i64,
    pub grace_ms: i64,
    pub silence_ms: i64,
}

impl RotationPolicy {
    /// The phone's policy: the **computed** session cap (M26A — the device's
    /// memory and the configured model, not a constant), the desktop grace and
    /// the desktop pause length, so the two hosts cut a take on the same rule.
    pub fn phone() -> Self {
        Self {
            cap_ms: suramak_core::session::session_cap_ms(),
            grace_ms: SPLIT_GRACE_MS,
            silence_ms: SPLIT_SILENCE_MS,
        }
    }

    /// A policy with another cap (tests and future hosts).
    pub const fn with_cap(cap_ms: i64) -> Self {
        Self {
            cap_ms,
            grace_ms: SPLIT_GRACE_MS,
            silence_ms: SPLIT_SILENCE_MS,
        }
    }

    /// A single-file policy: no rotation ever happens.
    pub const fn single() -> Self {
        Self {
            cap_ms: 0,
            grace_ms: 0,
            silence_ms: 0,
        }
    }
}

/// The one sentence the 12-part ceiling is allowed to say.
pub fn part_ceiling_reason(cap_ms: i64) -> String {
    let hours = (cap_ms.max(1) * i64::from(MAX_PARTS)) / 3_600_000;
    format!(
        "session_too_long: this take is longer than {hours} hours — a recording stops at \
         {MAX_PARTS} parts of {} each; start a new recording",
        clock(cap_ms.max(1))
    )
}

fn samples_for(ms: i64) -> u64 {
    if ms <= 0 {
        0
    } else {
        ((ms as f64) * f64::from(TARGET_RATE) / 1000.0).round() as u64
    }
}

fn write_error(path: &Path, err: std::io::Error) -> AsrError {
    AsrError::Capture(format!("could not write {}: {err}", path.display()))
}

fn io_error(path: &Path, err: std::io::Error) -> AsrError {
    AsrError::Capture(format!("could not rotate {}: {err}", path.display()))
}

/// The live part rotator: 16 kHz f32 in, one or more canonical PCM16 WAVs out.
///
/// It never touches the resampler. A rotation truncates the open part at the
/// chosen sample and copies the already-written tail bytes (PCM16 is
/// per-sample and deterministic) into the next part, so the parts concatenate
/// to exactly the samples that went in.
pub struct PartRotator {
    dir: PathBuf,
    stem: String,
    /// Part 1 is created at exactly this path (a take that fits keeps its name).
    first_path: PathBuf,
    policy: RotationPolicy,
    cap_samples: u64,
    grace_samples: u64,
    silence_samples: u64,
    frame_len: usize,
    file: File,
    path: PathBuf,
    /// Samples in the open part.
    frames: u64,
    /// Samples pushed in total.
    total: u64,
    /// Global sample index where the open part started.
    start: u64,
    parts: Vec<PartWav>,
    /// Reused PCM16 byte buffer for one write.
    scratch: Vec<u8>,
    frame_sq: f64,
    frame_count: usize,
    /// Global sample index where the frame being accumulated started.
    frame_start: u64,
    /// Quietest 50 ms frame in the first two seconds (the noise floor).
    floor_min: f32,
    calibrated_frames: u32,
    threshold: f32,
    calibrated: bool,
    /// Consecutive silent frames and where the run began.
    silent_run: u64,
    run_start: u64,
    seeking: bool,
    /// The cap's sample position for the open part (where a straddling pause
    /// cuts, and the floor for every later cut).
    pending_at: u64,
    /// The last five seconds of 50 ms frames (energy and start), for the
    /// no-pause fallback.
    recent: VecDeque<(u64, f32)>,
}

impl PartRotator {
    /// Opens part 1 at `first_path`.
    pub fn create(first_path: &Path, policy: RotationPolicy) -> Result<Self, AsrError> {
        let dir = first_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&dir).map_err(|err| {
            AsrError::Capture(format!(
                "could not create the recording directory {}: {err}",
                dir.display()
            ))
        })?;
        let stem = first_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("recording")
            .to_string();
        let frame_len = (samples_for(PART_FRAME_MS).max(1)) as usize;
        let cap_samples = if policy.cap_ms <= 0 {
            u64::MAX
        } else {
            samples_for(policy.cap_ms).max(frame_len as u64)
        };
        let grace_samples = if policy.cap_ms <= 0 {
            0
        } else {
            samples_for(policy.grace_ms)
        };
        let silence_samples = if policy.silence_ms <= 0 {
            u64::MAX
        } else {
            samples_for(policy.silence_ms).max(1)
        };
        let rotator = Self {
            dir,
            stem,
            first_path: first_path.to_path_buf(),
            policy,
            cap_samples,
            grace_samples,
            silence_samples,
            frame_len,
            file: open_part_file(first_path)?,
            path: first_path.to_path_buf(),
            frames: 0,
            total: 0,
            start: 0,
            parts: Vec::new(),
            scratch: Vec::with_capacity(CHUNK * 2),
            frame_sq: 0.0,
            frame_count: 0,
            frame_start: 0,
            floor_min: f32::INFINITY,
            calibrated_frames: 0,
            threshold: 0.0,
            calibrated: false,
            silent_run: 0,
            run_start: 0,
            seeking: false,
            pending_at: 0,
            recent: VecDeque::with_capacity(recent_frames()),
        };
        rotator.log_part(1);
        Ok(rotator)
    }

    /// The file of the open part.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 16 kHz samples pushed so far, across every part.
    pub fn total_frames(&self) -> u64 {
        self.total
    }

    /// 1-based index of the open part (what the foreground notification names).
    pub fn part_index(&self) -> u32 {
        self.parts.len() as u32 + 1
    }

    /// The silence gate's threshold, in RMS (0 until the calibration is done).
    pub fn silence_threshold(&self) -> f32 {
        self.threshold
    }

    /// Appends one 16 kHz slice, rotating first when the rule says so.
    pub fn write(&mut self, samples: &[f32]) -> Result<(), AsrError> {
        if samples.is_empty() {
            return Ok(());
        }
        self.append(samples)?;
        let base = self.total;
        self.total += samples.len() as u64;
        self.frames += samples.len() as u64;

        let mut index = 0usize;
        while index < samples.len() {
            if self.frame_count == 0 {
                self.frame_start = base + index as u64;
            }
            let take = (self.frame_len - self.frame_count).min(samples.len() - index);
            for &sample in &samples[index..index + take] {
                self.frame_sq += f64::from(sample) * f64::from(sample);
            }
            self.frame_count += take;
            index += take;
            if self.frame_count == self.frame_len {
                let rms = (self.frame_sq / self.frame_len as f64).sqrt() as f32;
                let start = self.frame_start;
                self.frame_sq = 0.0;
                self.frame_count = 0;
                self.on_frame(start, rms)?;
            }
        }
        Ok(())
    }

    /// Trims the take to `target` samples by shortening the last part (the
    /// resampler emits a padded final chunk; core truncates it, and so do we).
    pub fn trim_to(&mut self, target: u64) -> Result<(), AsrError> {
        if self.total <= target {
            return Ok(());
        }
        let mut excess = self.total - target;
        let from_open = excess.min(self.frames);
        if from_open > 0 {
            self.frames -= from_open;
            excess -= from_open;
            self.truncate(&self.path.clone(), self.frames)?;
        }
        while excess > 0 {
            let Some(mut part) = self.parts.pop() else {
                break;
            };
            let remove = excess.min(part.frames);
            part.frames -= remove;
            excess -= remove;
            if part.frames == 0 {
                let _ = std::fs::remove_file(&part.path);
            } else {
                self.truncate(&part.path, part.frames)?;
                part.bytes = WAV_HEADER_BYTES + part.frames * 2;
                self.parts.push(part);
            }
        }
        self.total = target;
        Ok(())
    }

    /// Completes every part, names a multi-part take `Part N of M`, and returns
    /// the parts in order.
    pub fn finish(mut self) -> Result<Vec<PartWav>, AsrError> {
        // The fallback runs on 50 ms frame boundaries, so a take can stop a
        // few samples after the grace edge before any frame completes. Split
        // that sliver here: every part the queue sees stays within
        // `cap + grace`, and the tail is a real (if short) part, never a
        // refusal and never zero-length.
        let bound = self.cap_samples.saturating_add(self.grace_samples);
        if self.cap_samples != u64::MAX && self.frames > bound {
            let cut = self.start + bound;
            self.rotate(cut, SplitReason::NoPause)?;
        }
        if self.frames == 0 && !self.parts.is_empty() {
            // A padded tail trimmed to nothing: no zero-length part is emitted.
            let _ = std::fs::remove_file(&self.path);
        } else {
            patch_header(&mut self.file, &self.path, TARGET_RATE, self.frames)?;
            self.file
                .flush()
                .map_err(|err| write_error(&self.path, err))?;
            self.file
                .sync_all()
                .map_err(|err| write_error(&self.path, err))?;
            self.parts.push(PartWav {
                path: self.path.clone(),
                frames: self.frames,
                bytes: WAV_HEADER_BYTES + self.frames * 2,
                reason: SplitReason::End,
                info: PartInfo::new(self.part_index(), 0),
            });
        }
        let count = self.parts.len() as u32;
        if count <= 1 {
            if let Some(part) = self.parts.first_mut() {
                part.info = PartInfo::new(1, 1);
            }
            return Ok(self.parts);
        }
        for (index, part) in self.parts.iter_mut().enumerate() {
            let info = PartInfo::new(index as u32 + 1, count);
            let final_path = self
                .dir
                .join(format!("{}.wav", part_stem(&self.stem, &info)));
            if part.path != final_path {
                std::fs::rename(&part.path, &final_path)
                    .map_err(|err| io_error(&part.path, err))?;
                part.path = final_path;
            }
            part.info = info;
        }
        Ok(self.parts)
    }

    // -- the rule -----------------------------------------------------------

    fn on_frame(&mut self, start: u64, rms: f32) -> Result<(), AsrError> {
        if !self.calibrated {
            self.floor_min = self.floor_min.min(rms);
            self.calibrated_frames += 1;
            let enough = i64::from(self.calibrated_frames) * PART_FRAME_MS >= PART_FLOOR_MS;
            let at_cap = self.frames >= self.cap_samples;
            if enough || at_cap {
                self.threshold = (self.floor_min * 3.0).clamp(PART_MIN_THRESHOLD_RMS, SILENCE_RMS);
                self.calibrated = true;
                self.log_part(self.part_index());
            }
        }
        if !self.calibrated {
            return Ok(());
        }

        if rms <= self.threshold {
            if self.silent_run == 0 {
                self.run_start = start;
            }
            self.silent_run += 1;
        } else {
            self.silent_run = 0;
        }
        self.recent.push_back((start, rms));
        while self.recent.len() > recent_frames() {
            self.recent.pop_front();
        }

        let cap_at = self.start.saturating_add(self.cap_samples);
        let frame_end = start + self.frame_len as u64;
        if !self.seeking && frame_end >= cap_at {
            self.seeking = true;
            self.pending_at = cap_at;
            if self.run_is_a_pause() {
                // The pause straddles the cap: the part ends exactly at it.
                return self.rotate(self.pending_at, SplitReason::Pause);
            }
            return Ok(());
        }
        if !self.seeking {
            return Ok(());
        }

        let grace_end = cap_at.saturating_add(self.grace_samples);
        if self.run_is_a_pause() {
            let cut = self.run_start.max(self.pending_at).min(grace_end);
            return self.rotate(cut, SplitReason::Pause);
        }
        if frame_end >= grace_end {
            // No pause inside the grace: the least-energetic 50 ms window seen
            // in the last five seconds, never past the grace edge.
            let best = self
                .recent
                .iter()
                .filter(|(frame_start, _)| {
                    *frame_start >= self.pending_at
                        && frame_start + self.frame_len as u64 <= grace_end
                })
                .min_by(|left, right| left.1.total_cmp(&right.1))
                .map(|(frame_start, _)| *frame_start);
            let cut = best
                .unwrap_or(grace_end)
                .max(self.pending_at)
                .min(grace_end);
            return self.rotate(cut, SplitReason::NoPause);
        }
        Ok(())
    }

    fn run_is_a_pause(&self) -> bool {
        self.silent_run * (self.frame_len as u64) >= self.silence_samples
    }

    /// Closes the open part at `cut` and continues in the next one with the
    /// tail bytes that were already written.
    fn rotate(&mut self, cut: u64, reason: SplitReason) -> Result<(), AsrError> {
        let keep = cut.saturating_sub(self.start);
        if keep == 0 || keep >= self.frames {
            // Nothing to move yet (or the cut is the end of the take): wait for
            // more samples. A zero-length part is never emitted.
            return Ok(());
        }
        if self.parts.len() as u32 + 2 > MAX_PARTS {
            return Err(AsrError::Capture(part_ceiling_reason(self.policy.cap_ms)));
        }
        let tail = self.frames - keep;
        let mut bytes = vec![0u8; (tail as usize) * 2];
        self.file
            .seek(SeekFrom::Start(WAV_HEADER_BYTES + keep * 2))
            .map_err(|err| io_error(&self.path, err))?;
        self.file
            .read_exact(&mut bytes)
            .map_err(|err| io_error(&self.path, err))?;
        self.file
            .set_len(WAV_HEADER_BYTES + keep * 2)
            .map_err(|err| io_error(&self.path, err))?;
        patch_header(&mut self.file, &self.path, TARGET_RATE, keep)?;
        self.file
            .flush()
            .map_err(|err| write_error(&self.path, err))?;

        let closed = PartWav {
            path: self.path.clone(),
            frames: keep,
            bytes: WAV_HEADER_BYTES + keep * 2,
            reason,
            info: PartInfo::new(self.part_index(), 0),
        };
        self.parts.push(closed);
        let next = self.part_index();
        self.open_part(next)?;
        self.file
            .write_all(&bytes)
            .map_err(|err| write_error(&self.path, err))?;
        self.frames = tail;
        self.start = cut;
        self.seeking = false;
        self.silent_run = 0;
        self.recent.clear();
        self.log_part(next);
        Ok(())
    }

    fn open_part(&mut self, index: u32) -> Result<(), AsrError> {
        let path = if index == 1 {
            self.first_path.clone()
        } else {
            self.dir.join(format!("{} — Part {index}.wav", self.stem))
        };
        self.file = open_part_file(&path)?;
        self.path = path;
        self.frames = 0;
        Ok(())
    }

    fn append(&mut self, samples: &[f32]) -> Result<(), AsrError> {
        self.scratch.clear();
        self.scratch.reserve(samples.len() * 2);
        for &sample in samples {
            let scaled = (sample * f32::from(i16::MAX)).round();
            let value = scaled.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
            self.scratch.extend_from_slice(&value.to_le_bytes());
        }
        self.file
            .write_all(&self.scratch)
            .map_err(|err| write_error(&self.path, err))
    }

    fn truncate(&mut self, path: &Path, frames: u64) -> Result<(), AsrError> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|err| io_error(path, err))?;
        file.set_len(WAV_HEADER_BYTES + frames * 2)
            .map_err(|err| io_error(path, err))?;
        patch_header(&mut file, path, TARGET_RATE, frames)?;
        file.flush().map_err(|err| write_error(path, err))
    }

    fn log_part(&self, index: u32) {
        let at = if self.start == 0 && index == 1 {
            0
        } else {
            (self.start as i64) * 1000 / i64::from(TARGET_RATE)
        };
        let threshold = if self.calibrated { self.threshold } else { 0.0 };
        tracing::info!(
            target: "suramak::capture::parts",
            part = index,
            at = %clock(at),
            threshold_rms = threshold,
            "recording part {index} starts at {}, silence threshold {threshold:.5} ({:+.1} dBFS)",
            clock(at),
            if threshold > 0.0 {
                20.0 * threshold.log10()
            } else {
                -120.0
            }
        );
    }
}

fn recent_frames() -> usize {
    (5_000 / PART_FRAME_MS).max(1) as usize
}

/// Opens (creates/truncates) a canonical empty part file.
fn open_part_file(path: &Path) -> Result<File, AsrError> {
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
    Ok(file)
}

/// Every file that belongs to the take at `path`, in part order.
///
/// Part 1 is exactly `path`; later parts are `{stem} — Part N` while open and
/// `{stem} — Part N of M` once finished. A killed process leaves the former.
pub fn take_part_paths(path: &Path) -> Vec<PathBuf> {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("recording");
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let prefix = format!("{stem} — Part ");
    // `(index, the reserved path?, path)`: the reserved path wins for part 1
    // when it still exists, so a finished take is not counted twice.
    let mut found: Vec<(u32, bool, PathBuf)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![path.to_path_buf()];
    };
    for entry in entries.flatten() {
        let candidate = entry.path();
        if candidate == path {
            found.push((1, true, candidate));
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let Some(rest) = rest.strip_suffix(".wav") else {
            continue;
        };
        let index = rest.split(" of ").next().unwrap_or(rest);
        if let Ok(index) = index.trim().parse::<u32>() {
            if index >= 1 {
                found.push((index, false, candidate));
            }
        }
    }
    if found.is_empty() {
        return vec![path.to_path_buf()];
    }
    found.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| right.1.cmp(&left.1)));
    found.dedup_by_key(|(index, _, _)| *index);
    found.into_iter().map(|(_, _, path)| path).collect()
}

/// Repairs a take a killed process left behind — every part, not just part 1 —
/// and names a multi-part take `Part N of M`.
pub fn repair_take(path: &Path) -> Result<StreamedWav, AsrError> {
    let paths = take_part_paths(path);
    let mut parts = Vec::with_capacity(paths.len());
    for candidate in &paths {
        let repaired = repair(candidate)?;
        parts.push(PartWav {
            path: candidate.clone(),
            frames: repaired.frames,
            bytes: repaired.bytes,
            reason: SplitReason::End,
            info: PartInfo::new(0, 0),
        });
    }
    let count = parts.len() as u32;
    let mut renamed: Vec<PartWav> = Vec::with_capacity(parts.len());
    for (index, mut part) in parts.into_iter().enumerate() {
        let info = PartInfo::new(index as u32 + 1, count);
        if count > 1 {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("recording");
            let dir = path.parent().unwrap_or_else(|| Path::new("."));
            let final_path = dir.join(format!("{}.wav", part_stem(stem, &info)));
            if part.path != final_path {
                std::fs::rename(&part.path, &final_path)
                    .map_err(|err| io_error(&part.path, err))?;
                part.path = final_path;
            }
        }
        part.info = info;
        renamed.push(part);
    }
    let frames = renamed.iter().map(|part| part.frames).sum();
    let bytes = renamed.iter().map(|part| part.bytes).sum();
    Ok(StreamedWav {
        path: renamed
            .first()
            .map(|part| part.path.clone())
            .unwrap_or_else(|| path.to_path_buf()),
        frames,
        bytes,
        device_rate: TARGET_RATE,
        parts: renamed,
    })
}

/// Appends 16 kHz mono PCM16 samples to a canonical WAV file.
///
/// Construction writes the header with zero sizes (a crash before the first
/// sample leaves a valid, empty WAV); [`finish`](Self::finish) pads the final
/// resampler chunk, truncates to the exact input duration, and patches the
/// sizes — the same tail `write_recording_wav` produces.
pub struct StreamingWav {
    path: PathBuf,
    /// `None` when the device already records at 16 kHz (core's fast path).
    resampler: Option<FftFixedIn<f32>>,
    device_rate: u32,
    /// Mono device-rate samples not yet forming a full 1024-sample chunk.
    pending: Vec<f32>,
    /// Real (not padding) device-rate samples pushed so far.
    input_samples: u64,
    /// The 16 kHz tail: one part, or a rotating take.
    rotator: Option<PartRotator>,
}

impl StreamingWav {
    /// Creates `path` (and its parent) with a valid, empty WAV header.
    ///
    /// `device_rate` is the rate of the samples [`push`](Self::push) will
    /// receive; it must be non-zero. An existing file is truncated: a recording
    /// path is reserved for exactly one take.
    pub fn create(path: &Path, device_rate: u32) -> Result<Self, AsrError> {
        Self::create_with_policy(path, device_rate, RotationPolicy::single())
    }

    /// Creates a take that rotates at this device's computed session cap.
    pub fn create_rotating(path: &Path, device_rate: u32) -> Result<Self, AsrError> {
        Self::create_with_policy(path, device_rate, RotationPolicy::phone())
    }

    /// Creates a take with an explicit rotation policy (tests, other hosts).
    pub fn create_with_policy(
        path: &Path,
        device_rate: u32,
        policy: RotationPolicy,
    ) -> Result<Self, AsrError> {
        if device_rate == 0 {
            return Err(AsrError::Capture(
                "the capture device reported a zero sample rate".to_string(),
            ));
        }
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
            resampler,
            device_rate,
            pending: Vec::with_capacity(CHUNK),
            input_samples: 0,
            rotator: Some(PartRotator::create(path, policy)?),
        })
    }

    /// The file being written (the first part).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Device sample rate this stream was opened for.
    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    /// 1-based index of the part being written (the notification's number).
    pub fn part_index(&self) -> u32 {
        self.rotator
            .as_ref()
            .map(PartRotator::part_index)
            .unwrap_or(1)
    }

    /// The live silence threshold (0 until the first two seconds are seen).
    pub fn silence_threshold(&self) -> f32 {
        self.rotator
            .as_ref()
            .map(PartRotator::silence_threshold)
            .unwrap_or(0.0)
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
            self.rotator
                .as_mut()
                .expect("the rotator is open until finish")
                .write(samples)?;
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
        self.rotator
            .as_ref()
            .map(PartRotator::total_frames)
            .unwrap_or(0)
    }

    /// Completes the file(s): pads and resamples the final partial chunk, trims
    /// to the input's exact 16 kHz duration, patches every header and syncs.
    /// A multi-part take lands as `Part N of M`.
    pub fn finish(mut self) -> Result<StreamedWav, AsrError> {
        if self.resampler.is_some() && !self.pending.is_empty() {
            // The desktop path pads the last chunk with zeros too
            // (`to_16khz_mono`), so this is the same tail, not an approximation.
            self.pending.resize(CHUNK, 0.0);
            self.process_chunk()?;
        }
        let expected = self.expected_frames();
        let mut rotator = self
            .rotator
            .take()
            .expect("the rotator is open until finish");
        rotator.trim_to(expected)?;
        let parts = rotator.finish()?;
        let frames = parts.iter().map(|part| part.frames).sum();
        let bytes = parts.iter().map(|part| part.bytes).sum();
        Ok(StreamedWav {
            path: parts
                .first()
                .map(|part| part.path.clone())
                .unwrap_or_else(|| self.path.clone()),
            frames,
            bytes,
            device_rate: self.device_rate,
            parts,
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
            self.rotator
                .as_mut()
                .expect("the rotator is open until finish")
                .write(samples)?;
        }
        self.pending = Vec::with_capacity(CHUNK);
        Ok(())
    }
}

impl Drop for StreamingWav {
    fn drop(&mut self) {
        // A dropped, unfinished stream is the crash case: flush what is there
        // so `repair` can see every byte the kernel accepted. Sizes stay zero
        // until `repair` (or finish) patches them.
        if let Some(rotator) = self.rotator.as_mut() {
            let _ = rotator.file.flush();
        }
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
        return Ok(empty_take(path, TARGET_RATE));
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
    let mut wav = empty_take(path, TARGET_RATE);
    wav.frames = data_bytes / 2;
    wav.bytes = WAV_HEADER_BYTES + data_bytes;
    wav.parts[0].frames = wav.frames;
    wav.parts[0].bytes = wav.bytes;
    Ok(wav)
}

fn empty_take(path: &Path, device_rate: u32) -> StreamedWav {
    StreamedWav {
        path: path.to_path_buf(),
        frames: 0,
        bytes: WAV_HEADER_BYTES,
        device_rate,
        parts: vec![PartWav {
            path: path.to_path_buf(),
            frames: 0,
            bytes: WAV_HEADER_BYTES,
            reason: SplitReason::End,
            info: PartInfo::new(1, 1),
        }],
    }
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

    /// The same contract through the rotating constructor: a take that fits
    /// the cap is still exactly one file, byte-identical to the desktop tail.
    #[test]
    fn a_rotating_take_that_fits_is_still_one_desktop_identical_wav() {
        let dir = root("rotating-fits");
        let streamed_path = dir.join("streamed.wav");
        let desktop_path = dir.join("desktop.wav");
        let input = sine(48_000, 2.4, 440.0);
        let mut stream = StreamingWav::create_rotating(&streamed_path, 48_000).unwrap();
        for block in input.chunks(441) {
            stream.push(block).unwrap();
        }
        let finished = stream.finish().unwrap();
        assert_eq!(finished.parts.len(), 1);
        assert_eq!(finished.parts[0].info.label(), "Part 1 of 1");
        let resampled = crate::capture::resample_16k(&input, 48_000).unwrap();
        crate::capture::write_recording_wav(&desktop_path, &resampled, TARGET_RATE).unwrap();
        assert_eq!(
            std::fs::read(&streamed_path).unwrap(),
            std::fs::read(&desktop_path).unwrap()
        );
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

#[cfg(test)]
mod rotation_tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("suramak-rotation-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tone(ms: i64) -> Vec<f32> {
        let count = samples_for(ms) as usize;
        (0..count)
            .map(|index| {
                let phase = std::f32::consts::TAU * 220.0 * index as f32 / TARGET_RATE as f32;
                0.4 * phase.sin()
            })
            .collect()
    }

    fn quiet(ms: i64, amplitude: f32) -> Vec<f32> {
        vec![amplitude; samples_for(ms) as usize]
    }

    fn silent(ms: i64) -> Vec<f32> {
        vec![0.0; samples_for(ms) as usize]
    }

    fn part_bytes(path: &Path) -> Vec<u8> {
        std::fs::read(path).unwrap()[WAV_HEADER_BYTES as usize..].to_vec()
    }

    fn concatenated(parts: &[PartWav]) -> Vec<u8> {
        let mut out = Vec::new();
        for part in parts {
            out.extend_from_slice(&part_bytes(&part.path));
        }
        out
    }

    /// Pushes `signal` in uneven blocks, the shape an AAudio callback has.
    fn push(stream: &mut StreamingWav, signal: &[f32]) {
        for block in signal.chunks(3_721) {
            stream.push(block).unwrap();
        }
    }

    fn assert_no_empty_parts(parts: &[PartWav]) {
        assert!(!parts.is_empty());
        for part in parts {
            assert!(
                part.frames > 0,
                "{} is a zero-length part",
                part.path.display()
            );
            assert_eq!(std::fs::metadata(&part.path).unwrap().len(), part.bytes);
            assert_eq!(part.bytes, WAV_HEADER_BYTES + part.frames * 2);
        }
    }

    #[test]
    fn a_pause_after_the_cap_rotates_once_and_the_parts_are_bit_exact() {
        let dir = root("pause");
        let path = dir.join("Meeting.wav");
        let reference_path = dir.join("reference.wav");
        let policy = RotationPolicy {
            cap_ms: 2_000,
            grace_ms: 1_000,
            silence_ms: 600,
        };
        let signal = [tone(2_100), silent(800), tone(1_500)].concat();

        let mut stream = StreamingWav::create_with_policy(&path, TARGET_RATE, policy).unwrap();
        let mut reference = StreamingWav::create(&reference_path, TARGET_RATE).unwrap();
        push(&mut stream, &signal);
        push(&mut reference, &signal);
        let finished = stream.finish().unwrap();
        reference.finish().unwrap();

        assert_eq!(finished.parts.len(), 2);
        assert_eq!(finished.parts[0].reason, SplitReason::Pause);
        assert_eq!(finished.parts[0].duration_ms(TARGET_RATE), 2_100);
        assert_eq!(finished.parts[1].reason, SplitReason::End);
        assert_eq!(finished.parts[0].info.label(), "Part 1 of 2");
        assert_eq!(finished.parts[1].info.label(), "Part 2 of 2");
        assert_eq!(finished.frames, samples_for(2_100 + 800 + 1_500));
        assert_eq!(
            concatenated(&finished.parts),
            part_bytes(&reference_path),
            "the parts concatenate to the uninterrupted take bit for bit"
        );
        assert_no_empty_parts(&finished.parts);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_take_with_no_pause_cuts_at_the_least_energetic_window_and_is_flagged() {
        let dir = root("no-pause");
        let path = dir.join("Lecture.wav");
        let policy = RotationPolicy {
            cap_ms: 1_000,
            grace_ms: 400,
            silence_ms: 600,
        };
        // Speech, a 50 ms quiet notch at 1.15 s, speech to 1.4 s (the grace
        // edge), then speech. No run of 600 ms silence anywhere.
        let signal = [tone(1_150), quiet(50, 0.002), tone(250), tone(900)].concat();
        let mut stream = StreamingWav::create_with_policy(&path, TARGET_RATE, policy).unwrap();
        push(&mut stream, &signal);
        let finished = stream.finish().unwrap();

        assert_eq!(finished.parts.len(), 2);
        assert_eq!(finished.parts[0].reason, SplitReason::NoPause);
        assert!(finished.parts[0].cut_without_pause());
        assert_eq!(
            finished.parts[0].duration_ms(TARGET_RATE),
            1_150,
            "the cut is the quiet window, not the cap"
        );
        assert!(finished.parts[0].frames <= samples_for(1_400));
        assert_eq!(finished.frames, samples_for(1_150 + 50 + 250 + 900));
        assert_eq!(
            concatenated(&finished.parts),
            part_bytes(&path_with_reference(&dir, &signal)),
            "a no-pause cut is still sample-exact"
        );
        assert_no_empty_parts(&finished.parts);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Writes `signal` as one ordinary WAV and returns its path.
    fn path_with_reference(dir: &Path, signal: &[f32]) -> PathBuf {
        let path = dir.join("reference.wav");
        let mut reference = StreamingWav::create(&path, TARGET_RATE).unwrap();
        push(&mut reference, signal);
        reference.finish().unwrap();
        path
    }

    #[test]
    fn a_long_silence_before_the_cap_does_not_rotate_early() {
        let dir = root("pre-cap-silence");
        let path = dir.join("Standup.wav");
        let policy = RotationPolicy {
            cap_ms: 5_000,
            grace_ms: 2_000,
            silence_ms: 600,
        };
        // 1 s speech, a 3 s silence that ends well before the 5 s cap, speech
        // through the cap, then the first real pause at 6 s.
        let signal = [
            tone(1_000),
            silent(3_000),
            tone(2_000),
            silent(800),
            tone(500),
        ]
        .concat();
        let mut stream = StreamingWav::create_with_policy(&path, TARGET_RATE, policy).unwrap();
        push(&mut stream, &signal);
        let finished = stream.finish().unwrap();

        assert_eq!(finished.parts.len(), 2, "one rotation, not two");
        assert_eq!(finished.parts[0].reason, SplitReason::Pause);
        assert_eq!(
            finished.parts[0].duration_ms(TARGET_RATE),
            6_000,
            "the pre-cap silence is not a cut; the post-cap pause is"
        );
        assert_no_empty_parts(&finished.parts);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_three_part_take_is_named_part_n_of_m() {
        let dir = root("three");
        let path = dir.join("Interview.wav");
        let policy = RotationPolicy {
            cap_ms: 1_000,
            grace_ms: 1_000,
            silence_ms: 600,
        };
        let signal = [
            tone(1_050),
            silent(700),
            tone(1_050),
            silent(700),
            tone(800),
        ]
        .concat();
        let mut stream = StreamingWav::create_with_policy(&path, TARGET_RATE, policy).unwrap();
        push(&mut stream, &signal);
        let finished = stream.finish().unwrap();

        assert_eq!(finished.parts.len(), 3);
        let expected: Vec<String> = (1..=3)
            .map(|index| {
                format!(
                    "{}",
                    dir.join(format!("Interview — Part {index} of 3.wav"))
                        .display()
                )
            })
            .collect();
        for (part, name) in finished.parts.iter().zip(&expected) {
            assert_eq!(part.path.display().to_string(), *name);
            assert!(part.path.is_file());
            assert_eq!(
                PartInfo::from_name(part.path.file_name().unwrap().to_str().unwrap()),
                Some(part.info.clone())
            );
        }
        assert_eq!(finished.parts[0].info.label(), "Part 1 of 3");
        assert_eq!(finished.parts[2].info.label(), "Part 3 of 3");
        assert!(
            !dir.join("Interview.wav").exists(),
            "the provisional name is gone"
        );
        assert_no_empty_parts(&finished.parts);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_rotator_never_emits_a_zero_length_part() {
        let dir = root("zero");
        let path = dir.join("Silence.wav");
        // Every part begins inside a silence that straddles its cap: the cut
        // is exactly at the cap, never an empty next part.
        let policy = RotationPolicy {
            cap_ms: 500,
            grace_ms: 200,
            silence_ms: 600,
        };
        let mut stream = StreamingWav::create_with_policy(&path, TARGET_RATE, policy).unwrap();
        push(&mut stream, &silent(2_200));
        let finished = stream.finish().unwrap();
        assert_eq!(finished.parts.len(), 5);
        assert_eq!(finished.parts[0].frames, samples_for(500));
        assert_no_empty_parts(&finished.parts);
        assert_eq!(finished.frames, samples_for(2_200));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_take_that_ends_exactly_at_the_cap_is_one_file() {
        let dir = root("exact-cap");
        let path = dir.join("Short.wav");
        let policy = RotationPolicy {
            cap_ms: 1_000,
            grace_ms: 200,
            silence_ms: 600,
        };
        let mut stream = StreamingWav::create_with_policy(&path, TARGET_RATE, policy).unwrap();
        push(&mut stream, &tone(1_000));
        let finished = stream.finish().unwrap();
        assert_eq!(finished.parts.len(), 1);
        assert_eq!(finished.parts[0].info.label(), "Part 1 of 1");
        assert_eq!(finished.path, path);
        assert_eq!(finished.frames, samples_for(1_000));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_take_stopping_just_past_the_grace_edge_still_lands_inside_the_bound() {
        let dir = root("grace-edge");
        let path = dir.join("Edge.wav");
        // Grace 220 ms ends at 720 ms, between two 50 ms frames; the take ends
        // at 730 ms, before the frame that would run the fallback. Stop must
        // split the 10 ms sliver rather than leave a part the queue would
        // refuse as over cap + grace.
        let policy = RotationPolicy {
            cap_ms: 500,
            grace_ms: 220,
            silence_ms: 600,
        };
        let mut stream = StreamingWav::create_with_policy(&path, TARGET_RATE, policy).unwrap();
        push(&mut stream, &tone(730));
        let finished = stream.finish().unwrap();
        assert_eq!(finished.parts.len(), 2);
        assert_eq!(finished.parts[0].frames, samples_for(720));
        assert_eq!(
            finished.parts[1].frames,
            samples_for(730) - samples_for(720)
        );
        assert!(finished.parts[1].frames > 0);
        assert!(finished.parts[0].cut_without_pause());
        assert_no_empty_parts(&finished.parts);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_twelve_part_ceiling_refuses_with_one_honest_message() {
        let dir = root("ceiling");
        let path = dir.join("Marathon.wav");
        let policy = RotationPolicy {
            cap_ms: 300,
            grace_ms: 100,
            silence_ms: 600,
        };
        let mut stream = StreamingWav::create_with_policy(&path, TARGET_RATE, policy).unwrap();
        let error = stream
            .push(&tone(6_000))
            .expect_err("the 13th part must be refused");
        let message = error.to_string();
        assert!(message.contains("session_too_long:"), "{message}");
        assert!(message.contains("stops at 12 parts"), "{message}");
        assert!(message.contains("start a new recording"), "{message}");
        drop(stream);
        let repaired = repair_take(&path).unwrap();
        assert_eq!(repaired.parts.len(), MAX_PARTS as usize);
        assert_no_empty_parts(&repaired.parts);
        assert_eq!(repaired.parts[11].info.label(), "Part 12 of 12");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ceiling_message_names_the_computed_cap() {
        let cap = suramak_core::session::session_cap_ms();
        let message = part_ceiling_reason(cap);
        let hours = (cap.max(1) * i64::from(MAX_PARTS)) / 3_600_000;
        assert!(
            message.contains(&format!("longer than {hours} hours")),
            "{message}"
        );
        assert!(
            message.contains(&format!("12 parts of {}", clock(cap))),
            "{message}"
        );
    }

    #[test]
    fn the_threshold_is_three_times_the_first_two_seconds_noise_floor() {
        let dir = root("threshold");
        let path = dir.join("Room.wav");
        let policy = RotationPolicy {
            cap_ms: 2_000,
            grace_ms: 1_000,
            silence_ms: 600,
        };
        let mut stream = StreamingWav::create_with_policy(&path, TARGET_RATE, policy).unwrap();
        push(&mut stream, &quiet(2_000, 0.002));
        let threshold = stream.silence_threshold();
        assert!(
            (threshold - 0.006).abs() < 1e-6,
            "threshold {threshold} should be 3 × 0.002"
        );
        push(&mut stream, &tone(300));
        push(&mut stream, &silent(700));
        push(&mut stream, &tone(500));
        let finished = stream.finish().unwrap();
        assert_eq!(finished.parts.len(), 2);
        assert_eq!(finished.parts[0].duration_ms(TARGET_RATE), 2_300);
        assert_eq!(finished.parts[0].reason, SplitReason::Pause);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repair_take_accepts_an_already_finished_multi_part_take() {
        let dir = root("repair-finished");
        let path = dir.join("Finished.wav");
        let policy = RotationPolicy {
            cap_ms: 500,
            grace_ms: 100,
            silence_ms: 600,
        };
        let mut stream = StreamingWav::create_with_policy(&path, TARGET_RATE, policy).unwrap();
        push(&mut stream, &tone(1_600));
        let finished = stream.finish().unwrap();
        assert!(finished.parts.len() > 1);
        assert!(!path.exists(), "part 1 was renamed at finish");
        let repaired = repair_take(&path).unwrap();
        assert_eq!(repaired.parts.len(), finished.parts.len());
        for (left, right) in repaired.parts.iter().zip(&finished.parts) {
            assert_eq!(left.path, right.path);
            assert_eq!(left.info, right.info);
            assert_eq!(left.frames, right.frames);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repair_take_finds_every_provisional_part_of_a_killed_take() {
        let dir = root("repair-parts");
        let path = dir.join("Killed.wav");
        let policy = RotationPolicy {
            cap_ms: 500,
            grace_ms: 100,
            silence_ms: 600,
        };
        {
            let mut stream = StreamingWav::create_with_policy(&path, TARGET_RATE, policy).unwrap();
            push(&mut stream, &tone(1_600));
            // No finish: the process died here, mid-take.
        }
        let repaired = repair_take(&path).unwrap();
        assert_eq!(repaired.parts.len(), 3);
        assert_no_empty_parts(&repaired.parts);
        assert_eq!(repaired.parts[0].info.label(), "Part 1 of 3");
        assert_eq!(repaired.frames, samples_for(1_600));
        for part in &repaired.parts {
            let decoded = suramak_core::decode::decode_file(&part.path).unwrap();
            assert_eq!(decoded.samples.len() as u64, part.frames);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
