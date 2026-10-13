//! The cpal output engine (M8): one `suramak-playback` thread owns the decoded
//! audio **and** the cpal stream (`cpal::Stream` is `!Send`, so it must be
//! created and dropped on the thread that keeps it). The UI thread only sends
//! [`Command`]s and polls [`Engine::poll`] once per frame — the same
//! poll-a-shared-status pattern as the capture RMS meter.
//!
//! Failure paths are values, never panics: no output device (the headless VPS
//! and CI have none), an unreadable source file, a device lost mid-play. The
//! audio callback itself is allocation-free and lock-free (atomics only) — a
//! seek is handed over through [`Shared::pending_seek`] and applied by the
//! callback on its next block.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::{FftFixedIn, Resampler};
use suramak_core::decode;

use crate::transport::Command;

/// How long the engine thread waits for a command before re-checking the
/// callback's error slot; dropping the engine closes the channel and the thread
/// exits.
const IDLE_POLL: Duration = Duration::from_millis(50);

/// Sentinel for “no seek pending” in [`Shared::pending_seek`].
const SEEK_NONE: u32 = u32::MAX;

/// A sample-rate window an output device advertises (`min..=max` Hz) — the cpal
/// type reduced to something a unit test can build without a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateRange {
    pub min: u32,
    pub max: u32,
}

/// The rate to configure the output stream with: the file's own rate when the
/// device advertises it (no resampling), otherwise the device's default rate.
/// A pure function because the VPS has no device to ask.
pub fn pick_output_rate(ranges: &[RateRange], file_rate: u32, default_rate: u32) -> u32 {
    let supported = file_rate > 0
        && ranges
            .iter()
            .any(|range| range.min <= file_rate && file_rate <= range.max);
    if supported {
        file_rate
    } else {
        default_rate.max(1)
    }
}

/// A playback failure surfaced to the UI. `retryable` is set for device errors
/// (the device may come back — plug in headphones and press play again) and
/// clear for source errors (the file cannot be read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError {
    pub message: String,
    pub retryable: bool,
}

impl EngineError {
    pub fn new(message: impl Into<String>, retryable: bool) -> Self {
        Self {
            message: message.into(),
            retryable,
        }
    }
}

/// One poll of the engine: the position/duration the UI draws from, plus a
/// **terminal-once** error (the first poll that sees it takes it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub position_ms: u32,
    pub duration_ms: u32,
    /// Set when the engine reached the end of the audio.
    pub finished: bool,
    pub error: Option<EngineError>,
}

impl Status {
    /// A status carrying only a failure (used when the engine cannot start at
    /// all — no thread, no stream).
    pub fn failure(message: impl Into<String>, retryable: bool) -> Self {
        Self {
            position_ms: 0,
            duration_ms: 0,
            finished: false,
            error: Some(EngineError::new(message, retryable)),
        }
    }
}

/// The UI-thread handle of a running engine. Dropping it closes the command
/// channel; the thread then exits and drops the stream on its own.
pub struct Engine {
    commands: Sender<Command>,
    shared: Arc<Shared>,
    /// Kept for the type's sake; dropping a `JoinHandle` detaches the thread.
    _thread: JoinHandle<()>,
}

impl Engine {
    /// Spawns the engine thread for `path`. The audio is decoded and the output
    /// stream opened lazily on the first [`Command::Play`], so the headless case
    /// is reached through the same path with a clean error.
    pub fn start(path: PathBuf) -> Result<Engine, String> {
        let shared = Arc::new(Shared::new());
        let (commands, receiver) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("suramak-playback".to_string())
            .spawn(move || run(receiver, &path, &thread_shared))
            .map_err(|err| format!("cannot start the playback thread: {err}"))?;
        Ok(Engine {
            commands,
            shared,
            _thread: thread,
        })
    }

    /// Queues a command for the engine thread.
    pub fn send(&self, command: Command) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|_| "the playback engine has stopped".to_string())
    }

    /// The current position/duration/finished flag, plus (once) the last error.
    pub fn poll(&self) -> Status {
        Status {
            position_ms: self.shared.position_ms.load(Ordering::Relaxed),
            duration_ms: self.shared.duration_ms.load(Ordering::Relaxed),
            finished: self.shared.finished.load(Ordering::Relaxed),
            error: take_error(&self.shared),
        }
    }
}

/// State shared between the engine thread, the audio callback and the UI poll.
struct Shared {
    /// Playback position in milliseconds (what the UI's playhead reads).
    position_ms: AtomicU32,
    /// Length of the loaded audio in milliseconds (0 before the first load).
    duration_ms: AtomicU32,
    /// Sample rate of the loaded audio (the callback maps samples ↔ ms).
    rate: AtomicU32,
    /// Whether the callback should advance the playhead.
    playing: AtomicBool,
    /// Set by the callback when the last sample was played.
    finished: AtomicBool,
    /// A seek the control thread wants applied by the callback (`SEEK_NONE` when
    /// there is none).
    pending_seek: AtomicU32,
    /// Set-once slot for the callback's device errors.
    callback_error: Mutex<Option<String>>,
    /// The last load/device error, taken by [`Engine::poll`].
    error: Mutex<Option<EngineError>>,
}

impl Shared {
    fn new() -> Self {
        Self {
            position_ms: AtomicU32::new(0),
            duration_ms: AtomicU32::new(0),
            rate: AtomicU32::new(0),
            playing: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            pending_seek: AtomicU32::new(SEEK_NONE),
            callback_error: Mutex::new(None),
            error: Mutex::new(None),
        }
    }
}

/// A decoded + opened session: the stream keeps the output device playing (it
/// outputs silence while paused). Lives on the engine thread only.
struct Session {
    _stream: cpal::Stream,
}

impl Session {
    /// Decodes the file, picks an output config and opens the stream.
    fn open(path: &Path, shared: &Arc<Shared>) -> Result<Session, EngineError> {
        let buffer = decode::decode_file(path)
            .map_err(|err| EngineError::new(format!("could not read the audio: {err}"), false))?;
        let mono = mono_mixdown(&buffer.samples, buffer.channels as usize);

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| EngineError::new("no audio output device on this machine", true))?;
        let default_config = device
            .default_output_config()
            .map_err(|err| EngineError::new(format!("no audio output device: {err}"), true))?;
        let supported: Vec<cpal::SupportedStreamConfigRange> = device
            .supported_output_configs()
            .map(|configs| configs.collect())
            .unwrap_or_default();
        let ranges: Vec<RateRange> = supported
            .iter()
            .map(|config| RateRange {
                min: config.min_sample_rate().0,
                max: config.max_sample_rate().0,
            })
            .collect();
        let rate = pick_output_rate(&ranges, buffer.rate, default_config.sample_rate().0);
        // Prefer a mono stream at the chosen rate; otherwise use the device's
        // own channel count (the callback duplicates the mono signal).
        let mono_supported = supported.iter().any(|config| {
            config.channels() == 1
                && config.min_sample_rate().0 <= rate
                && rate <= config.max_sample_rate().0
        });
        let channels = if mono_supported {
            1
        } else {
            default_config.channels()
        };
        let sample_format = default_config.sample_format();
        let samples = if rate == buffer.rate {
            mono
        } else {
            resample_mono(&mono, buffer.rate, rate)?
        };

        tracing::info!(
            path = %path.display(),
            file_rate = buffer.rate,
            rate,
            channels,
            format = ?sample_format,
            frames = samples.len(),
            "playback: output stream opened"
        );

        shared
            .duration_ms
            .store(ms_of(samples.len(), rate), Ordering::Relaxed);
        shared.rate.store(rate, Ordering::Relaxed);
        let samples = Arc::new(samples);
        let config = cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(rate),
            buffer_size: cpal::BufferSize::Default,
        };
        let error_slot = Arc::clone(shared);
        let err_fn = move |err: cpal::StreamError| {
            if let Ok(mut guard) = error_slot.callback_error.lock() {
                *guard = Some(err.to_string());
            }
        };
        let channels = channels as usize;
        let stream = match sample_format {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config,
                data_callback::<f32>(&samples, shared, rate, channels, |value| value),
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                &config,
                data_callback::<i16>(&samples, shared, rate, channels, |value| {
                    (value.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
                }),
                err_fn,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_output_stream(
                &config,
                data_callback::<u16>(&samples, shared, rate, channels, |value| {
                    (value.clamp(-1.0, 1.0) * 32_768.0 + 32_768.0).clamp(0.0, 65_535.0) as u16
                }),
                err_fn,
                None,
            ),
            other => {
                return Err(EngineError::new(
                    format!("unsupported output sample format {other:?}"),
                    true,
                ))
            }
        }
        .map_err(|err| {
            EngineError::new(
                format!("could not open the audio output stream: {err}"),
                true,
            )
        })?;

        stream.play().map_err(|err| {
            EngineError::new(format!("could not start the audio stream: {err}"), true)
        })?;
        Ok(Session { _stream: stream })
    }
}

/// The audio callback for one sample format: renders `samples` (mono, at `rate`)
/// into the interleaved output buffer, honouring the shared transport flags.
fn data_callback<T>(
    samples: &Arc<Vec<f32>>,
    shared: &Arc<Shared>,
    rate: u32,
    channels: usize,
    convert: fn(f32) -> T,
) -> impl FnMut(&mut [T], &cpal::OutputCallbackInfo) + Send + 'static
where
    T: cpal::SizedSample + 'static,
{
    let samples = Arc::clone(samples);
    let shared = Arc::clone(shared);
    let mut cursor = 0usize;
    move |data: &mut [T], _| {
        fill(
            data,
            channels,
            &samples,
            &shared,
            rate,
            &mut cursor,
            convert,
        );
    }
}

/// Renders one interleaved output block. Runs in the audio callback: atomics
/// only, no locks, no allocation. `cursor` is the callback's own position (in
/// samples); [`Shared::pending_seek`] moves it when the control thread asked.
fn fill<T: Copy>(
    data: &mut [T],
    channels: usize,
    samples: &[f32],
    shared: &Shared,
    rate: u32,
    cursor: &mut usize,
    convert: impl Fn(f32) -> T,
) {
    let channels = channels.max(1);
    let frames = data.len() / channels;
    let playing = shared.playing.load(Ordering::Relaxed);
    let pending = shared.pending_seek.swap(SEEK_NONE, Ordering::Relaxed);
    if pending != SEEK_NONE {
        *cursor = ms_to_index(pending, rate);
    }
    for frame in 0..frames {
        let value = if playing && *cursor < samples.len() {
            let value = samples[*cursor];
            *cursor += 1;
            value
        } else {
            0.0
        };
        let output = convert(value);
        for channel in 0..channels {
            data[frame * channels + channel] = output;
        }
    }
    if playing {
        if *cursor >= samples.len() {
            shared.finished.store(true, Ordering::Relaxed);
            shared.playing.store(false, Ordering::Relaxed);
        }
        shared
            .position_ms
            .store(ms_of(*cursor, rate), Ordering::Relaxed);
    }
}

/// The engine thread: owns the session and answers commands.
fn run(receiver: Receiver<Command>, path: &Path, shared: &Arc<Shared>) {
    let mut session: Option<Session> = None;
    loop {
        match receiver.recv_timeout(IDLE_POLL) {
            Ok(command) => match command {
                Command::Play { from_ms } => {
                    if session.is_none() {
                        match Session::open(path, shared) {
                            Ok(open) => session = Some(open),
                            Err(error) => {
                                publish(shared, error);
                                continue;
                            }
                        }
                    }
                    play(shared, from_ms);
                }
                Command::Pause => pause(shared),
                Command::Resume => resume(shared),
                Command::Seek { ms } => seek(shared, ms),
                Command::Stop => stop(shared),
            },
            Err(RecvTimeoutError::Timeout) => {
                // A device error during playback: report it and drop the stream
                // so the next Play rebuilds it (never a silent death).
                if let Some(message) = take_callback_error(shared) {
                    publish(
                        shared,
                        EngineError::new(format!("the audio output failed: {message}"), true),
                    );
                    pause(shared);
                    stop(shared);
                    session = None;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Starts/restarts the playhead at `from_ms` (the callback applies the jump).
fn play(shared: &Shared, from_ms: u32) {
    shared.pending_seek.store(from_ms, Ordering::Relaxed);
    shared.position_ms.store(from_ms, Ordering::Relaxed);
    shared.finished.store(false, Ordering::Relaxed);
    shared.playing.store(true, Ordering::Relaxed);
}

fn pause(shared: &Shared) {
    shared.playing.store(false, Ordering::Relaxed);
}

fn resume(shared: &Shared) {
    shared.playing.store(true, Ordering::Relaxed);
}

fn seek(shared: &Shared, ms: u32) {
    shared.pending_seek.store(ms, Ordering::Relaxed);
    shared.position_ms.store(ms, Ordering::Relaxed);
}

fn stop(shared: &Shared) {
    shared.playing.store(false, Ordering::Relaxed);
    shared.finished.store(false, Ordering::Relaxed);
    shared.pending_seek.store(0, Ordering::Relaxed);
    shared.position_ms.store(0, Ordering::Relaxed);
}

fn publish(shared: &Shared, error: EngineError) {
    tracing::warn!(
        message = %error.message,
        retryable = error.retryable,
        "playback: engine error"
    );
    if let Ok(mut guard) = shared.error.lock() {
        *guard = Some(error);
    }
}

fn take_error(shared: &Shared) -> Option<EngineError> {
    shared.error.lock().ok().and_then(|mut guard| guard.take())
}

fn take_callback_error(shared: &Shared) -> Option<String> {
    shared
        .callback_error
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
}

/// Milliseconds for a sample cursor.
fn ms_of(samples: usize, rate: u32) -> u32 {
    let ms = (samples as u64 * 1_000) / u64::from(rate.max(1));
    ms.min(u64::from(u32::MAX)) as u32
}

/// The sample cursor for a millisecond position.
fn ms_to_index(ms: u32, rate: u32) -> usize {
    ((u64::from(ms) * u64::from(rate.max(1))) / 1_000) as usize
}

/// Averages interleaved samples to mono (a mono buffer passes through).
///
/// Playback is mono: the output stream duplicates the sample into every channel
/// it was configured with.
pub fn mono_mixdown(samples: &[f32], channels: usize) -> Vec<f32> {
    let channels = channels.max(1);
    if channels == 1 {
        return samples.to_vec();
    }
    samples
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// Resamples mono f32 samples from `from` Hz to `to` Hz with rubato's FFT
/// resampler — the same one `suramak_core::resample` uses for whisper (rubato
/// pads the final chunk, so the tail carries a little silence).
pub fn resample_mono(samples: &[f32], from: u32, to: u32) -> Result<Vec<f32>, EngineError> {
    if from == to || from == 0 || to == 0 {
        return Ok(samples.to_vec());
    }
    let chunk = 1_024;
    let mut resampler = FftFixedIn::<f32>::new(from as usize, to as usize, chunk, 2, 1)
        .map_err(|err| EngineError::new(format!("resampler init failed: {err}"), false))?;
    let mut output = Vec::with_capacity(samples.len() * to as usize / from as usize + chunk);
    let mut pos = 0;
    while pos < samples.len() {
        let end = (pos + chunk).min(samples.len());
        let mut block = samples[pos..end].to_vec();
        if block.len() < chunk {
            block.resize(chunk, 0.0);
        }
        let frames = resampler
            .process(&[block], None)
            .map_err(|err| EngineError::new(format!("resample failed: {err}"), false))?;
        output.extend_from_slice(&frames[0]);
        pos += chunk;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(min: u32, max: u32) -> RateRange {
        RateRange { min, max }
    }

    /// A sine wave at `freq` Hz, `rate` Hz, `secs` seconds, mono.
    fn sine(freq: f32, rate: u32, secs: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn the_output_rate_is_the_file_rate_when_the_device_has_it() {
        let ranges = [range(8_000, 44_100), range(44_100, 192_000)];
        assert_eq!(pick_output_rate(&ranges, 16_000, 48_000), 16_000);
        assert_eq!(pick_output_rate(&ranges, 8_000, 48_000), 8_000);
        assert_eq!(pick_output_rate(&ranges, 192_000, 48_000), 192_000);
        // Not advertised (or nothing advertised): the device default stands in
        // and the audio is resampled.
        assert_eq!(pick_output_rate(&ranges, 4_000, 48_000), 48_000);
        assert_eq!(pick_output_rate(&[], 16_000, 48_000), 48_000);
        assert_eq!(pick_output_rate(&ranges, 0, 0), 1);
    }

    #[test]
    fn mono_mixdown_averages_a_stereo_pair() {
        let stereo = [1.0f32, -1.0, 0.5, -0.5, 0.25, 0.75];
        let mono = mono_mixdown(&stereo, 2);
        assert_eq!(mono, vec![0.0, 0.0, 0.5]);
        assert_eq!(mono_mixdown(&[0.1, 0.2], 1), vec![0.1, 0.2]);
        assert!(mono_mixdown(&[], 2).is_empty());
    }

    #[test]
    fn resample_mono_keeps_the_tone_and_roughly_the_duration() {
        let from = 16_000u32;
        let to = 48_000u32;
        let samples = sine(440.0, from, 0.5);
        let resampled = resample_mono(&samples, from, to).expect("resample");

        // Three times the samples, plus rubato's padded tail chunk.
        let expected = samples.len() * 3;
        assert!(
            resampled.len() >= expected && resampled.len() <= expected + 1_024 * 3,
            "unexpected length {}",
            resampled.len()
        );
        // The 440 Hz tone survives: count zero crossings in the steady middle
        // (the FFT resampler's first ~25 ms are numerical warm-up).
        let steady = &resampled[1_200..resampled.len().min(22_800)];
        let crossings = steady
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        // 440 Hz over 0.45 s → 2·440·0.45 = 396 crossings (±10%).
        let expected_crossings = 396.0;
        assert!(
            (crossings as f32 - expected_crossings).abs() <= expected_crossings * 0.1,
            "expected ~396 zero crossings, got {crossings}"
        );
        // A same-rate call passes the samples through untouched.
        assert_eq!(
            resample_mono(&samples, from, from).expect("same rate"),
            samples
        );
    }

    #[test]
    fn milliseconds_and_sample_cursors_round_trip() {
        assert_eq!(ms_of(0, 48_000), 0);
        assert_eq!(ms_of(48_000, 48_000), 1_000);
        assert_eq!(ms_of(16_000, 16_000), 1_000);
        assert_eq!(ms_to_index(1_000, 16_000), 16_000);
        assert_eq!(ms_to_index(500, 48_000), 24_000);
        // A zero rate can never divide by zero (it is treated as 1 Hz).
        assert_eq!(ms_of(100, 0), 100_000);
        assert_eq!(ms_to_index(1_000, 0), 1);

        // The pair is what the playhead relies on: a cursor stored and read back
        // through milliseconds is within one millisecond of the original.
        let cursor = 12_345usize;
        let ms = ms_of(cursor, 48_000);
        assert!(ms_to_index(ms, 48_000).abs_diff(cursor) <= 48);
    }
}
