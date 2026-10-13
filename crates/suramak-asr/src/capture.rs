//! Microphone / system-audio capture (feature `capture`, OFF by default): input
//! device enumeration, stream start/stop, frame delivery with a live rolling-RMS
//! value. No UI code lives here.
//!
//! Two layers:
//!
//! * **Pure helpers** — [`mono_mixdown`], [`resample_16k`], [`RollingRms`],
//!   [`write_recording_wav`], [`resolve_input_source`], [`resolve_app_processes`]
//!   — compiled and unit-tested on *every* build (no cpal, no device, no ALSA
//!   runtime).
//! * **The cpal session** — [`start_recording`] / [`start_recording_with`] /
//!   [`start_recording_multi`], [`list_input_devices`] /
//!   [`list_input_devices_detailed`] / [`list_audio_processes`], [`Recorder`] /
//!   [`MultiRecorder`] — only with the `capture` feature. Without it this
//!   module still compiles and reports capture as unsupported
//!   ([`CAPTURE_COMPILED`] is the compile-time probe), so dependents can call
//!   it unconditionally.
//!
//! Addendum A: [`AudioSource`] settings select the input — default microphone, a
//! named device (case-insensitive enumeration match, documented fallback), or
//! macOS system audio through `suramak-taps` (CoreAudio process taps).
//!
//! M6: [`CaptureInput`] generalises that to a **list** of inputs recorded
//! concurrently (two mics, mic + system audio, per-app system audio) — one
//! [`Recorder`] per input, combined at stop by the pure mixing engine
//! ([`crate::mix`]), which also measures each source's clock drift from the
//! session span. [`list_audio_processes`] + [`resolve_app_processes`] back the
//! per-app picker: settings store app keys (bundle ids), live pids are
//! resolved at record time.

use std::path::Path;

use suramak_core::decode::{self, AudioBuffer};
use suramak_core::resample;
use suramak_core::settings::AudioSource;
use suramak_core::SuramakError;

use crate::whisper::AsrError;

/// Whether this build has the microphone/system-audio session compiled in
/// (`feature = "capture"`). The UI uses this to enable/disable the record control.
pub const CAPTURE_COMPILED: bool = cfg!(feature = "capture");

/// Rolling-RMS window: the level meter averages the most recent 100 ms of audio.
pub const RMS_WINDOW: std::time::Duration = std::time::Duration::from_millis(100);

/// One enumerated input device (name + whether the host considers it the default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDeviceInfo {
    pub name: String,
    pub is_default: bool,
}

/// Averages one interleaved block to mono (per-frame channel average).
///
/// A mono block passes through unchanged; a trailing partial frame is averaged
/// over the channels it actually has. This is the same mixdown the stream
/// callback performs, so the unit tests cover the live path's math.
pub fn mono_mixdown(block: &[f32], channels: usize) -> Vec<f32> {
    let channels = channels.max(1);
    block
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// Resamples mono samples at `rate` to 16 kHz (core's rubato path).
///
/// `rate == 16000` short-circuits (core's own fast path).
pub fn resample_16k(samples: &[f32], rate: u32) -> Result<Vec<f32>, AsrError> {
    let buffer = AudioBuffer {
        rate,
        channels: 1,
        samples: samples.to_vec(),
    };
    Ok(resample::to_16khz_mono(&buffer)?)
}

/// Writes mono f32 samples as a **16 kHz mono, 16-bit signed PCM** WAV file
/// (the one recording format — kept and temp alike; storage decision approved
/// during M3), creating missing parent directories first.
///
/// The samples are the same 16 kHz mono buffer whisper consumes, so a kept
/// recording re-transcribes with zero conversion; i16 halves the size of f32
/// (~115 MB/hour vs ~230 MB/h). Quantization error is ≤ 1/32767 (≈ −90 dBFS) —
/// far below any microphone noise floor.
pub fn write_recording_wav(path: &Path, samples: &[f32], rate: u32) -> Result<(), AsrError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(SuramakError::from)?;
    }
    decode::write_wav_i16(path, samples, rate, 1)?;
    Ok(())
}

/// Sliding-window RMS over the most recent `window` samples.
///
/// O(1) per sample: the running sum of squares is corrected when a sample leaves
/// the window. This is what the live meter reads.
#[derive(Debug, Clone)]
pub struct RollingRms {
    window: usize,
    samples: Vec<f32>,
    next: usize,
    filled: usize,
    sum_sq: f64,
}

impl RollingRms {
    /// Window length in samples (clamped to at least 1).
    pub fn new(window: usize) -> Self {
        let window = window.max(1);
        Self {
            window,
            samples: vec![0.0; window],
            next: 0,
            filled: 0,
            sum_sq: 0.0,
        }
    }

    /// Window length in samples.
    pub fn window(&self) -> usize {
        self.window
    }

    /// Pushes one sample into the window.
    pub fn push(&mut self, sample: f32) {
        let outgoing = self.samples[self.next];
        self.sum_sq +=
            f64::from(sample) * f64::from(sample) - f64::from(outgoing) * f64::from(outgoing);
        // Guard against f64 cancellation noise on long constant runs.
        if self.sum_sq < 0.0 {
            self.sum_sq = 0.0;
        }
        self.samples[self.next] = sample;
        self.next = (self.next + 1) % self.window;
        self.filled = (self.filled + 1).min(self.window);
    }

    /// Pushes a whole block.
    pub fn push_block(&mut self, block: &[f32]) {
        for &sample in block {
            self.push(sample);
        }
    }

    /// RMS (0.0+; f32 samples in −1..=1 give 0..=1) over the window so far.
    /// An empty window reads 0.0.
    pub fn value(&self) -> f32 {
        if self.filled == 0 {
            0.0
        } else {
            (self.sum_sq / self.filled as f64).sqrt() as f32
        }
    }
}

/// What [`start_recording_with`] will open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedSource {
    /// The system default input device.
    Default,
    /// A named input device, already matched against enumeration.
    Device(String),
    /// The system-audio tap's aggregate device (macOS builds).
    SystemAudio,
}

/// [`resolve_input_source`]'s answer: what to open, plus a user-visible warning
/// (e.g. a configured device that is no longer present).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceResolution {
    pub source: ResolvedSource,
    pub warning: Option<String>,
}

/// Whether system-audio taps are available in this build/OS.
///
/// Compile-time gate: only macOS builds carry the tap code. The macOS *runtime*
/// gate (14.2+) is enforced by `suramak-taps`, whose `start` answer is surfaced
/// as the recording error.
pub fn system_audio_available() -> bool {
    cfg!(target_os = "macos")
}

/// Pure resolution of a settings [`AudioSource`] against enumerated devices.
///
/// Documented rules (Addendum A):
/// - `Default` → the system default input device.
/// - `Device { name }` → case-insensitive match on the display name; when the
///   device is absent (or the configured name is empty), fall back to the
///   default **with a warning** (never a silent switch).
/// - `SystemAudio` → the tap path; when `system_audio_available` is false this
///   is an error (a non-macOS build can never record system audio — no silent
///   fallback to a microphone).
pub fn resolve_input_source(
    source: &AudioSource,
    devices: &[InputDeviceInfo],
    system_audio_available: bool,
) -> Result<SourceResolution, AsrError> {
    match source {
        AudioSource::Default => Ok(SourceResolution {
            source: ResolvedSource::Default,
            warning: None,
        }),
        AudioSource::Device { name } => {
            let wanted = name.trim();
            if !wanted.is_empty() {
                if let Some(device) = devices
                    .iter()
                    .find(|device| device.name.eq_ignore_ascii_case(wanted))
                {
                    return Ok(SourceResolution {
                        source: ResolvedSource::Device(device.name.clone()),
                        warning: None,
                    });
                }
            }
            let display = if wanted.is_empty() {
                "the configured device name is empty"
            } else {
                wanted
            };
            Ok(SourceResolution {
                source: ResolvedSource::Default,
                warning: Some(format!(
                    "input device '{display}' was not found — using the system default microphone"
                )),
            })
        }
        AudioSource::SystemAudio => {
            if !system_audio_available {
                return Err(system_audio_unavailable_error());
            }
            Ok(SourceResolution {
                source: ResolvedSource::SystemAudio,
                warning: None,
            })
        }
    }
}

/// The one error for "this build/platform has no tap support" (Addendum A
/// wording; shared by `resolve_input_source` and the M6 per-app path).
fn system_audio_unavailable_error() -> AsrError {
    AsrError::Capture(
        "system audio capture needs macOS 14.2+ (CoreAudio process taps) — \
         this build/platform has no tap support; pick a microphone in Settings"
            .to_string(),
    )
}

/// One input the M6 multi-source recorder opens (a generalisation of
/// [`AudioSource`] to per-app taps and to lists).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureInput {
    /// The system default input device.
    Default,
    /// A named input device, matched case-insensitively (documented fallback:
    /// a missing name uses the default with a warning).
    Device(String),
    /// macOS system audio: every process except Suramak (Addendum A).
    SystemAudio,
    /// macOS system audio restricted to these pids (M6 per-app capture) —
    /// resolved from settings app keys by [`resolve_app_processes`].
    AppAudio(Vec<u32>),
}

impl From<&AudioSource> for CaptureInput {
    fn from(source: &AudioSource) -> Self {
        match source {
            AudioSource::Default => CaptureInput::Default,
            AudioSource::Device { name } => CaptureInput::Device(name.clone()),
            AudioSource::SystemAudio => CaptureInput::SystemAudio,
        }
    }
}

/// One process the HAL knows to have audio (M6 per-app picker). Mirror of
/// `suramak_taps::AudioProcessInfo` so this module's type surface exists in
/// builds without the `capture` feature (where the tap crate is not compiled).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioProcessInfo {
    /// BSD process id (valid now, not across app restarts).
    pub pid: u32,
    /// Bundle id (e.g. `us.zoom.xos`); empty for processes without one.
    pub bundle_id: String,
    /// The process is currently rendering output audio.
    pub is_running_output: bool,
    /// The process is currently capturing input audio.
    pub is_running_input: bool,
}

impl AudioProcessInfo {
    /// Settings key for this process: the bundle id when present (stable across
    /// restarts), else `pid:<pid>` (matches only while that pid lives).
    pub fn settings_key(&self) -> String {
        if self.bundle_id.is_empty() {
            format!("pid:{}", self.pid)
        } else {
            self.bundle_id.clone()
        }
    }
}

#[cfg(feature = "capture")]
impl From<suramak_taps::AudioProcessInfo> for AudioProcessInfo {
    fn from(info: suramak_taps::AudioProcessInfo) -> Self {
        Self {
            pid: info.pid,
            bundle_id: info.bundle_id,
            is_running_output: info.is_running_output,
            is_running_input: info.is_running_input,
        }
    }
}

/// Resolves settings app keys to live pids (M6 per-app capture). **Pure** —
/// unit-tested on every platform.
///
/// Keys are what [`AudioProcessInfo::settings_key`] produced when the user
/// picked the app: a bundle id (`us.zoom.xos`, matched case-insensitively) or
/// `pid:<pid>` (matches only while that exact pid lives). Every key must match
/// a listed process; a key with no running process is an error — never a
/// silent widening to full system audio.
pub fn resolve_app_processes(
    apps: &[String],
    processes: &[AudioProcessInfo],
) -> Result<Vec<u32>, String> {
    if apps.is_empty() {
        return Err("no apps selected for the app-audio source".to_string());
    }
    let mut pids = Vec::new();
    let mut missing = Vec::new();
    for key in apps {
        let key = key.trim();
        let found = match key.strip_prefix("pid:") {
            Some(pid) => pid
                .parse::<u32>()
                .ok()
                .and_then(|pid| processes.iter().find(|process| process.pid == pid)),
            None => processes
                .iter()
                .find(|process| process.bundle_id.eq_ignore_ascii_case(key)),
        };
        match found {
            Some(process) => pids.push(process.pid),
            None => missing.push(key.to_string()),
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "no running audio process matches {} — start the app (and play audio), \
             refresh the list, and retry",
            missing
                .iter()
                .map(|key| format!("'{key}'"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    pids.sort_unstable();
    pids.dedup();
    Ok(pids)
}

#[cfg(feature = "capture")]
mod imp {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::StreamConfig;
    use suramak_core::decode::AudioBuffer;
    use suramak_core::settings::AudioSource;

    use super::{
        mono_mixdown, resolve_input_source, system_audio_available, AudioProcessInfo, CaptureInput,
        InputDeviceInfo, ResolvedSource, RollingRms, RMS_WINDOW,
    };
    use crate::whisper::AsrError;

    /// A running capture session. Dropping it (or calling [`Recorder::stop`]) ends capture.
    ///
    /// `cpal::Stream` is `!Send`, so a `Recorder` must be used from the thread that
    /// created it (main thread in the CLI/UI).
    pub struct Recorder {
        stream: cpal::Stream,
        samples: Arc<Mutex<Vec<f32>>>,
        rms_bits: Arc<AtomicU32>,
        device_name: String,
        sample_rate: u32,
        /// Warning produced while resolving the audio source (device fallback).
        warning: Option<String>,
        /// Keeps a system-audio tap (and its private aggregate device) alive for
        /// as long as the recording runs; `None` for microphone sessions.
        tap: Option<suramak_taps::SystemTap>,
    }

    impl Recorder {
        /// Name of the captured input device.
        pub fn device_name(&self) -> &str {
            &self.device_name
        }

        /// Device sample rate (Hz) of the accumulated samples.
        pub fn sample_rate(&self) -> u32 {
            self.sample_rate
        }

        /// Live rolling RMS level (0.0–1.0) of the most recent audio (100 ms window).
        pub fn rms(&self) -> f32 {
            f32::from_bits(self.rms_bits.load(Ordering::Relaxed))
        }

        /// Non-fatal warning from source resolution (e.g. missing named device).
        pub fn warning(&self) -> Option<&str> {
            self.warning.as_deref()
        }

        /// Stops capture (stream first, then any system-audio tap) and returns the
        /// accumulated mono samples as an [`AudioBuffer`] (channels = 1) ready for
        /// 16 kHz resampling via [`super::resample_16k`].
        pub fn stop(self) -> AudioBuffer {
            let Recorder {
                stream,
                samples,
                sample_rate,
                device_name: _,
                rms_bits: _,
                warning: _,
                tap,
            } = self;
            drop(stream); // stops the cpal stream
            drop(tap); // then tears down aggregate device + tap (macOS system audio)
            let samples = samples
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
            AudioBuffer {
                rate: sample_rate,
                channels: 1,
                samples,
            }
        }
    }

    /// Lists input devices with the host's default marker (uses one enumeration).
    pub fn list_input_devices_detailed() -> Result<Vec<InputDeviceInfo>, AsrError> {
        let host = cpal::default_host();
        let default_name = host.default_input_device().and_then(|d| d.name().ok());
        let devices = host
            .input_devices()
            .map_err(|e| AsrError::Capture(e.to_string()))?;
        let mut names = Vec::new();
        for device in devices {
            let name = device
                .name()
                .unwrap_or_else(|_| "<unnamed device>".to_string());
            let is_default = default_name.as_deref() == Some(name.as_str());
            names.push(InputDeviceInfo { name, is_default });
        }
        Ok(names)
    }

    /// Lists input device names on the default host.
    pub fn list_input_devices() -> Result<Vec<String>, AsrError> {
        Ok(list_input_devices_detailed()?
            .into_iter()
            .map(|device| device.name)
            .collect())
    }

    /// Starts capture on the default input device.
    pub fn start_recording() -> Result<Recorder, AsrError> {
        start_recording_with(&AudioSource::Default)
    }

    /// Starts capture for the settings' [`AudioSource`] (Addendum A).
    pub fn start_recording_with(source: &AudioSource) -> Result<Recorder, AsrError> {
        start_one(&CaptureInput::from(source))
    }

    /// Starts **one capture session per input** (M6): two mics, mic + system
    /// audio, per-app system audio. The first failure tears down the sessions
    /// already started (dropping a `Recorder` stops its stream/tap) — never a
    /// half-open multi session.
    ///
    /// The returned [`MultiRecorder`] carries the session's start instant; the
    /// mixer measures each source's clock drift from the span between it and
    /// [`MultiRecorder::stop`].
    pub fn start_recording_multi(inputs: &[CaptureInput]) -> Result<MultiRecorder, AsrError> {
        if inputs.is_empty() {
            return Err(AsrError::Capture("no input sources selected".to_string()));
        }
        let started = std::time::Instant::now();
        let mut recorders = Vec::with_capacity(inputs.len());
        for input in inputs {
            recorders.push(start_one(input)?);
        }
        Ok(MultiRecorder {
            live_cursors: vec![0; recorders.len()],
            recorders,
            started,
        })
    }

    /// Lists the running processes with audio (M6 per-app picker): macOS
    /// process taps only. Every other build reports
    /// [`AsrError::CaptureUnsupported`].
    pub fn list_audio_processes() -> Result<Vec<AudioProcessInfo>, AsrError> {
        match suramak_taps::list_audio_processes() {
            Ok(processes) => Ok(processes.into_iter().map(AudioProcessInfo::from).collect()),
            Err(suramak_taps::TapError::Unsupported) => Err(AsrError::CaptureUnsupported),
            Err(err) => Err(AsrError::Capture(err.to_string())),
        }
    }

    /// Starts one capture session (the shared body of
    /// [`start_recording_with`] and [`start_recording_multi`]).
    fn start_one(input: &CaptureInput) -> Result<Recorder, AsrError> {
        let host = cpal::default_host();
        let devices = list_input_devices_detailed()?;

        // System audio keeps its tap alive for the whole session (the aggregate
        // device disappears with it), so it is created before the stream and moved
        // into the `Recorder` on success.
        let (device, tap, warning) = match input {
            CaptureInput::Default => (default_input_device(&host)?, None, None),
            CaptureInput::Device(name) => {
                // Addendum A's documented resolution: case-insensitive match,
                // missing name → system default *with a warning*.
                let resolution = resolve_input_source(
                    &AudioSource::Device { name: name.clone() },
                    &devices,
                    system_audio_available(),
                )?;
                match resolution.source {
                    ResolvedSource::Device(canonical) => {
                        (device_by_name(&host, &canonical)?, None, None)
                    }
                    _ => (default_input_device(&host)?, None, resolution.warning),
                }
            }
            CaptureInput::SystemAudio => {
                let resolution = resolve_input_source(
                    &AudioSource::SystemAudio,
                    &devices,
                    system_audio_available(),
                )?;
                debug_assert_eq!(resolution.source, ResolvedSource::SystemAudio);
                let tap = suramak_taps::SystemTap::start(&suramak_taps::TapConfig::system_audio())
                    .map_err(|err| AsrError::Capture(err.to_string()))?;
                let device = device_for_tap(&host, &tap)?;
                (device, Some(tap), None)
            }
            CaptureInput::AppAudio(pids) => {
                if pids.is_empty() {
                    return Err(AsrError::Capture(
                        "the app-audio source has no processes — pick at least one app".to_string(),
                    ));
                }
                if !system_audio_available() {
                    return Err(super::system_audio_unavailable_error());
                }
                let tap = suramak_taps::SystemTap::start(&suramak_taps::TapConfig::app_audio(
                    pids.clone(),
                ))
                .map_err(|err| AsrError::Capture(err.to_string()))?;
                let device = device_for_tap(&host, &tap)?;
                (device, Some(tap), None)
            }
        };

        let device_name = device
            .name()
            .unwrap_or_else(|_| "<unnamed device>".to_string());
        let supported = device
            .default_input_config()
            .map_err(|e| AsrError::Capture(e.to_string()))?;
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.into();

        let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let rms_bits = Arc::new(AtomicU32::new(0));
        let window =
            ((config.sample_rate.0 as u128 * RMS_WINDOW.as_millis()) / 1000).max(1) as usize;

        let err_fn = |err: cpal::StreamError| eprintln!("audio capture error: {err}");

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let (samples, rms_bits) = (samples.clone(), rms_bits.clone());
                let channels = config.channels as usize;
                let mut rolling = RollingRms::new(window);
                device
                    .build_input_stream(
                        &config,
                        move |data: &[f32], _| {
                            accumulate(&samples, &rms_bits, &mut rolling, data, channels);
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| AsrError::Capture(e.to_string()))?
            }
            cpal::SampleFormat::I16 => {
                let (samples, rms_bits) = (samples.clone(), rms_bits.clone());
                let channels = config.channels as usize;
                let mut rolling = RollingRms::new(window);
                device
                    .build_input_stream(
                        &config,
                        move |data: &[i16], _| {
                            let floats: Vec<f32> =
                                data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                            accumulate(&samples, &rms_bits, &mut rolling, &floats, channels);
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| AsrError::Capture(e.to_string()))?
            }
            cpal::SampleFormat::U16 => {
                let (samples, rms_bits) = (samples.clone(), rms_bits.clone());
                let channels = config.channels as usize;
                let mut rolling = RollingRms::new(window);
                device
                    .build_input_stream(
                        &config,
                        move |data: &[u16], _| {
                            let floats: Vec<f32> = data
                                .iter()
                                .map(|s| (*s as f32 - 32768.0) / 32768.0)
                                .collect();
                            accumulate(&samples, &rms_bits, &mut rolling, &floats, channels);
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| AsrError::Capture(e.to_string()))?
            }
            other => {
                return Err(AsrError::Capture(format!(
                    "unsupported sample format {other:?}"
                )))
            }
        };

        stream
            .play()
            .map_err(|e| AsrError::Capture(e.to_string()))?;

        Ok(Recorder {
            stream,
            samples,
            rms_bits,
            device_name,
            sample_rate: config.sample_rate.0,
            warning,
            tap,
        })
    }

    /// A running multi-source capture session (M6): one [`Recorder`] per input
    /// plus the session's start instant — the wall-clock span the mixer's
    /// clock-drift measurement uses.
    pub struct MultiRecorder {
        recorders: Vec<Recorder>,
        started: std::time::Instant,
        /// Per-source read cursors for [`MultiRecorder::take_live_chunk`] (M7).
        live_cursors: Vec<usize>,
    }

    impl MultiRecorder {
        /// Number of live sessions.
        pub fn len(&self) -> usize {
            self.recorders.len()
        }

        /// Whether no session is live (cannot happen; a `MultiRecorder` always
        /// holds ≥1 recorder — the constructor refuses an empty list).
        pub fn is_empty(&self) -> bool {
            self.recorders.is_empty()
        }

        /// Device names of the open sessions (enumeration spelling), in input
        /// order.
        pub fn device_names(&self) -> Vec<String> {
            self.recorders
                .iter()
                .map(|recorder| recorder.device_name().to_string())
                .collect()
        }

        /// Combined live level: the **loudest source's** rolling RMS. A true
        /// post-mix meter would need the mix, which happens at stop (M6
        /// decision, documented in the report) — the loudest source is the
        /// honest cheap stand-in.
        pub fn rms(&self) -> f32 {
            self.recorders
                .iter()
                .map(Recorder::rms)
                .fold(0.0f32, f32::max)
        }

        /// Non-fatal warnings from source resolution, in input order.
        pub fn warnings(&self) -> Vec<String> {
            self.recorders
                .iter()
                .filter_map(|recorder| recorder.warning().map(str::to_string))
                .collect()
        }

        /// New mono samples (device rate) per source since the previous call —
        /// the live scheduler's feed (M7).
        ///
        /// Read-only: the accumulated buffers (and therefore the final
        /// recording) are untouched; only the per-source read cursor moves.
        /// Each entry is `(rate, new samples)` in recorder order; a source with
        /// no new audio yields an empty chunk, so the caller's index mapping
        /// never shifts.
        pub fn take_live_chunk(&mut self) -> Vec<(u32, Vec<f32>)> {
            self.recorders
                .iter()
                .zip(self.live_cursors.iter_mut())
                .map(|(recorder, cursor)| {
                    let samples = recorder
                        .samples
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let from = (*cursor).min(samples.len());
                    let chunk = samples[from..].to_vec();
                    *cursor = samples.len();
                    (recorder.sample_rate, chunk)
                })
                .collect()
        }

        /// Stops every session (each stream, then its tap) and returns each
        /// source's accumulated mono samples plus the session's wall-clock
        /// span — exactly what [`crate::mix::mix`] consumes.
        pub fn stop(self) -> (Vec<AudioBuffer>, std::time::Duration) {
            let duration = self.started.elapsed();
            let buffers = self.recorders.into_iter().map(Recorder::stop).collect();
            (buffers, duration)
        }
    }

    /// The system host's default input device or a clear error.
    fn default_input_device(host: &cpal::Host) -> Result<cpal::Device, AsrError> {
        host.default_input_device()
            .ok_or_else(|| AsrError::Capture("no default input device".to_string()))
    }

    /// Case-insensitive input-device lookup by display name.
    fn device_by_name(host: &cpal::Host, name: &str) -> Result<cpal::Device, AsrError> {
        let devices = host
            .input_devices()
            .map_err(|e| AsrError::Capture(e.to_string()))?;
        for device in devices {
            if device
                .name()
                .is_ok_and(|candidate| candidate.eq_ignore_ascii_case(name))
            {
                return Ok(device);
            }
        }
        Err(AsrError::Capture(format!(
            "input device '{name}' is not available"
        )))
    }

    /// The device a system-audio tap appears as.
    ///
    /// cpal 0.15.3's coreaudio backend exposes **no UID/AudioDeviceID lookup**
    /// (verified against its source: `Device { audio_device_id, is_default }` is
    /// opaque and only `name()` is public), so the private aggregate device is
    /// matched by the distinctive name `suramak-taps` gave it. The HAL registers
    /// newly created aggregate devices asynchronously, so this retries briefly
    /// before giving up; the tap UID in the error is the authoritative handle.
    ///
    /// Deliberately does NOT call `SystemTap::device_id()`: translating the UID
    /// while the HAL device list was still settling crashed inside the HAL
    /// (`HALDeviceList::CopyDeviceByUID` → `CFStringCompare`, SIGSEGV on
    /// macOS 27, 2026-10-13 — probed live on the owner's Mac).
    fn device_for_tap(
        host: &cpal::Host,
        tap: &suramak_taps::SystemTap,
    ) -> Result<cpal::Device, AsrError> {
        const ATTEMPTS: u32 = 6;
        const DELAY: std::time::Duration = std::time::Duration::from_millis(250);
        for attempt in 0..ATTEMPTS {
            if let Ok(device) = device_by_name(host, tap.device_name()) {
                return Ok(device);
            }
            if attempt + 1 < ATTEMPTS {
                std::thread::sleep(DELAY);
            }
        }
        Err(AsrError::Capture(format!(
            "the system-audio tap (uid '{}') is not visible to the audio layer yet — \
             grant the audio-capture permission and retry",
            tap.device_uid()
        )))
    }

    /// Mono-mixes one interleaved block into `samples` and updates the rolling RMS.
    fn accumulate(
        samples: &Mutex<Vec<f32>>,
        rms_bits: &AtomicU32,
        rolling: &mut RollingRms,
        data: &[f32],
        channels: usize,
    ) {
        if data.is_empty() {
            return;
        }
        let mut buffer = samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for mono in mono_mixdown(data, channels) {
            buffer.push(mono);
            rolling.push(mono);
        }
        rms_bits.store(rolling.value().to_bits(), Ordering::Relaxed);
    }
}

#[cfg(feature = "capture")]
pub use imp::{
    list_audio_processes, list_input_devices, list_input_devices_detailed, start_recording,
    start_recording_multi, start_recording_with, MultiRecorder, Recorder,
};

#[cfg(not(feature = "capture"))]
mod imp {
    use suramak_core::decode::AudioBuffer;
    use suramak_core::settings::AudioSource;

    use super::InputDeviceInfo;
    use crate::whisper::AsrError;

    /// Placeholder for builds without the `capture` feature; can never be constructed
    /// (`start_recording*` always fail with [`AsrError::CaptureUnsupported`]), but it
    /// carries the real session's method surface so callers type-check everywhere.
    pub struct Recorder {
        _private: (),
    }

    impl Recorder {
        /// Never reached (no instance can exist).
        pub fn device_name(&self) -> &str {
            ""
        }

        /// Never reached (no instance can exist).
        pub fn sample_rate(&self) -> u32 {
            0
        }

        /// Never reached (no instance can exist).
        pub fn rms(&self) -> f32 {
            0.0
        }

        /// Never reached (no instance can exist).
        pub fn warning(&self) -> Option<&str> {
            None
        }

        /// Never reached (no instance can exist): an empty buffer keeps the stub
        /// panic-free for any caller that ignores the error path.
        pub fn stop(self) -> AudioBuffer {
            AudioBuffer {
                rate: 0,
                channels: 1,
                samples: Vec::new(),
            }
        }
    }

    /// Capture support is not compiled in — reports unsupported.
    pub fn list_input_devices() -> Result<Vec<String>, AsrError> {
        Err(AsrError::CaptureUnsupported)
    }

    /// Capture support is not compiled in — reports unsupported.
    pub fn list_input_devices_detailed() -> Result<Vec<InputDeviceInfo>, AsrError> {
        Err(AsrError::CaptureUnsupported)
    }

    /// Capture support is not compiled in — reports unsupported.
    pub fn start_recording() -> Result<Recorder, AsrError> {
        Err(AsrError::CaptureUnsupported)
    }

    /// Capture support is not compiled in — reports unsupported.
    pub fn start_recording_with(_source: &AudioSource) -> Result<Recorder, AsrError> {
        Err(AsrError::CaptureUnsupported)
    }

    /// Placeholder for builds without the `capture` feature; can never be
    /// constructed (`start_recording_multi` always fails), but it carries the
    /// real session's method surface so callers type-check everywhere.
    pub struct MultiRecorder {
        _private: (),
    }

    impl MultiRecorder {
        /// Never reached (no instance can exist).
        pub fn len(&self) -> usize {
            0
        }

        /// Never reached (no instance can exist).
        pub fn is_empty(&self) -> bool {
            true
        }

        /// Never reached (no instance can exist).
        pub fn device_names(&self) -> Vec<String> {
            Vec::new()
        }

        /// Never reached (no instance can exist).
        pub fn rms(&self) -> f32 {
            0.0
        }

        /// Never reached (no instance can exist).
        pub fn warnings(&self) -> Vec<String> {
            Vec::new()
        }

        /// Never reached (no instance can exist): no live audio without capture.
        pub fn take_live_chunk(&mut self) -> Vec<(u32, Vec<f32>)> {
            Vec::new()
        }

        /// Never reached (no instance can exist): empty buffers keep the stub
        /// panic-free for any caller that ignores the error path.
        pub fn stop(self) -> (Vec<AudioBuffer>, std::time::Duration) {
            (Vec::new(), std::time::Duration::ZERO)
        }
    }

    /// Capture support is not compiled in — reports unsupported.
    pub fn start_recording_multi(
        _inputs: &[super::CaptureInput],
    ) -> Result<MultiRecorder, AsrError> {
        Err(AsrError::CaptureUnsupported)
    }

    /// Process taps are macOS-only; this build has no listing.
    pub fn list_audio_processes() -> Result<Vec<super::AudioProcessInfo>, AsrError> {
        Err(AsrError::CaptureUnsupported)
    }
}

#[cfg(not(feature = "capture"))]
pub use imp::{
    list_audio_processes, list_input_devices, list_input_devices_detailed, start_recording,
    start_recording_multi, start_recording_with, MultiRecorder, Recorder,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine wave at `freq` Hz, `rate` Hz, `secs` seconds, mono.
    fn sine(freq: f32, rate: u32, secs: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn device(name: &str, is_default: bool) -> InputDeviceInfo {
        InputDeviceInfo {
            name: name.to_string(),
            is_default,
        }
    }

    #[test]
    fn mono_mixdown_averages_stereo_frames() {
        let stereo = [1.0f32, -1.0, 0.5, -0.25, 0.0, 1.0];
        let mono = mono_mixdown(&stereo, 2);
        assert_eq!(mono.len(), 3);
        assert!((mono[0] - 0.0).abs() < 1e-6);
        assert!((mono[1] - 0.125).abs() < 1e-6);
        assert!((mono[2] - 0.5).abs() < 1e-6);

        // Mono passes through; a trailing partial frame averages what is there.
        assert_eq!(mono_mixdown(&[0.1, 0.2, 0.3], 1), vec![0.1, 0.2, 0.3]);
        assert_eq!(mono_mixdown(&[1.0, 0.0, 0.5], 2), vec![0.5, 0.5]);
        assert!(mono_mixdown(&[], 2).is_empty());
    }

    #[test]
    fn resample_16k_preserves_a_440hz_tone() {
        let rate = 44_100u32;
        let samples = sine(440.0, rate, 0.5);
        let resampled = resample_16k(&samples, rate).expect("resample");
        // ~0.5 s at 16 kHz (rubato pads the last chunk; allow the padding).
        let expected = (16_000.0 * 0.5) as usize;
        assert!(
            resampled.len() >= expected && resampled.len() <= expected + 2048,
            "unexpected length {}",
            resampled.len()
        );
        // Measure in the steady middle: the FFT resampler's first ~25 ms are
        // near-zero numerical warm-up (sign-jitter around 1e-8), which would
        // swamp a naive crossing count.
        let steady = &resampled[400..resampled.len().min(7600)];
        let crossings = steady
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        // 440 Hz over 7200 samples (0.45 s) → 2·440·0.45 = 396 crossings (±10%).
        let expected_crossings = 396.0;
        assert!(
            (crossings as f32 - expected_crossings).abs() <= expected_crossings * 0.1,
            "expected ~396 zero crossings in the steady region, got {crossings}"
        );
        // Non-silent, full-scale-ish.
        assert!(steady.iter().any(|s| s.abs() > 0.5));
    }

    #[test]
    fn resample_16k_is_a_no_op_at_target_rate() {
        let samples = vec![0.25f32; 1600];
        let out = resample_16k(&samples, 16_000).expect("resample");
        assert_eq!(out, samples);
    }

    #[test]
    fn rolling_rms_averages_the_window() {
        // Empty window reads 0.
        assert_eq!(RollingRms::new(4).value(), 0.0);

        let mut rms = RollingRms::new(2);
        rms.push(1.0);
        assert!((rms.value() - 1.0).abs() < 1e-6);
        rms.push(0.0);
        assert!((rms.value() - 0.707_106_77).abs() < 1e-5);
        // The oldest sample left the window: constant-zero run reads 0.
        rms.push(0.0);
        assert_eq!(rms.value(), 0.0);

        // A block push equals sample pushes, and the window slides.
        let mut block = RollingRms::new(4);
        block.push_block(&[0.5, 0.5, 0.5, 0.5, 0.5, 0.5]);
        assert!((block.value() - 0.5).abs() < 1e-6);
        block.push_block(&[0.0, 0.0, 0.0, 0.0]);
        assert_eq!(block.value(), 0.0);
        assert_eq!(block.window(), 4);

        // Window clamped to ≥1.
        let mut tiny = RollingRms::new(0);
        tiny.push(0.25);
        assert!((tiny.value() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn write_recording_wav_roundtrips_through_the_decoder() {
        let dir = std::env::temp_dir().join(format!("suramak-capture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("nested").join("recording.wav");

        let samples = sine(440.0, 16_000, 0.25);
        write_recording_wav(&path, &samples, 16_000).expect("write wav");
        assert!(path.is_file(), "parent directories are created");

        let decoded = decode::decode_file(&path).expect("decode back");
        assert_eq!(decoded.rate, 16_000);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.samples.len(), samples.len());
        // 16-bit PCM: samples survive to within one quantization step (1/32767).
        assert!((decoded.samples[10] - samples[10]).abs() < 1.0 / 32_767.0 + 1e-6);

        // The file really is 16-bit PCM: 44-byte header + 2 bytes/sample.
        let bytes = std::fs::metadata(&path).unwrap().len();
        assert_eq!(bytes, 44 + 2 * samples.len() as u64);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_source_default_reads_the_host_default() {
        let devices = [
            device("MacBook Microphone", true),
            device("BlackHole 2ch", false),
        ];
        let resolution =
            resolve_input_source(&AudioSource::Default, &devices, true).expect("resolve");
        assert_eq!(resolution.source, ResolvedSource::Default);
        assert_eq!(resolution.warning, None);
    }

    #[test]
    fn resolve_source_matches_a_named_device_case_insensitively() {
        let devices = [
            device("MacBook Microphone", true),
            device("BlackHole 2ch", false),
        ];
        let resolution = resolve_input_source(
            &AudioSource::Device {
                name: "  blackhole 2CH ".to_string(),
            },
            &devices,
            true,
        )
        .expect("resolve");
        assert_eq!(
            resolution.source,
            ResolvedSource::Device("BlackHole 2ch".to_string()),
            "the canonical enumeration spelling is used"
        );
        assert_eq!(resolution.warning, None);
    }

    #[test]
    fn resolve_source_falls_back_to_default_with_a_warning() {
        let devices = [device("MacBook Microphone", true)];
        let warning_shown = resolve_input_source(
            &AudioSource::Device {
                name: "BlackHole 2ch".to_string(),
            },
            &devices,
            true,
        )
        .expect("resolve");
        assert_eq!(warning_shown.source, ResolvedSource::Default);
        let warning = warning_shown.warning.expect("fallback must warn");
        assert!(warning.contains("BlackHole 2ch"), "unexpected: {warning}");

        // An empty configured name also falls back (with a warning).
        let empty = resolve_input_source(
            &AudioSource::Device {
                name: "   ".to_string(),
            },
            &devices,
            true,
        )
        .expect("resolve");
        assert_eq!(empty.source, ResolvedSource::Default);
        assert!(empty.warning.is_some());
    }

    #[test]
    fn resolve_source_system_audio_requires_macos_support() {
        let devices = [device("MacBook Microphone", true)];
        // macOS (available): the tap path is selected.
        let resolution =
            resolve_input_source(&AudioSource::SystemAudio, &devices, true).expect("resolve");
        assert_eq!(resolution.source, ResolvedSource::SystemAudio);
        assert_eq!(resolution.warning, None);

        // Non-macOS / older macOS: a clear error, never a silent microphone switch.
        let err = resolve_input_source(&AudioSource::SystemAudio, &devices, false)
            .expect_err("must fail without tap support");
        match err {
            AsrError::Capture(message) => {
                assert!(message.contains("macOS 14.2"), "unexpected: {message}");
                assert!(message.contains("microphone"), "unexpected: {message}");
            }
            other => panic!("expected a capture error, got {other:?}"),
        }

        // The compile-time probe agrees with the platform.
        assert_eq!(system_audio_available(), cfg!(target_os = "macos"));
    }

    /// Device enumeration smoke test (feature `capture-test`).
    ///
    /// CI runners have no microphone, so this asserts the *shape* (a list, names
    /// non-empty, at most one default marker) — never that a device exists. It
    /// opens no stream.
    #[cfg(feature = "capture-test")]
    #[test]
    fn enumeration_returns_a_list() {
        let devices = list_input_devices_detailed().expect("enumeration must not fail");
        for device in &devices {
            assert!(!device.name.is_empty(), "device names are never empty");
        }
        let defaults = devices.iter().filter(|d| d.is_default).count();
        assert!(defaults <= devices.len());
        // The plain-name list is the same enumeration.
        let names = list_input_devices().expect("enumeration must not fail");
        assert_eq!(names.len(), devices.len());
    }

    // ---- M6: per-app selection plumbing -------------------------------------

    fn process(pid: u32, bundle_id: &str) -> AudioProcessInfo {
        AudioProcessInfo {
            pid,
            bundle_id: bundle_id.to_string(),
            is_running_output: true,
            is_running_input: false,
        }
    }

    #[test]
    fn resolve_app_processes_matches_bundle_ids_case_insensitively() {
        let processes = [
            process(700, "us.zoom.xos"),
            process(701, "com.google.Chrome"),
            process(702, "com.spotify.client"),
        ];
        let pids = resolve_app_processes(
            &["US.Zoom.XOS".to_string(), "com.google.chrome".to_string()],
            &processes,
        )
        .expect("both apps are running");
        assert_eq!(pids, vec![700, 701]);
    }

    #[test]
    fn resolve_app_processes_supports_pid_keys_and_dedupes() {
        let processes = [process(700, "us.zoom.xos"), process(704, "")];
        // A pid key matches that exact pid; a bundle key the same process.
        let pids = resolve_app_processes(
            &[
                "pid:704".to_string(),
                "us.zoom.xos".to_string(),
                "us.zoom.xos".to_string(),
            ],
            &processes,
        )
        .expect("resolve");
        assert_eq!(pids, vec![700, 704], "sorted and deduped");
        // A process without a bundle id keys by pid (the settings-key rule).
        assert_eq!(processes[1].settings_key(), "pid:704");
    }

    #[test]
    fn resolve_app_processes_reports_missing_apps() {
        let processes = [process(700, "us.zoom.xos")];
        let err = resolve_app_processes(
            &["us.zoom.xos".to_string(), "com.unknown.app".to_string()],
            &processes,
        )
        .expect_err("a key with no running process must fail");
        assert!(err.contains("com.unknown.app"), "unexpected: {err}");
        assert!(
            err.contains("no running audio process"),
            "unexpected: {err}"
        );

        // No keys at all is refused too (the tap would capture nothing).
        let err = resolve_app_processes(&[], &processes).expect_err("empty keys");
        assert!(err.contains("no apps selected"), "unexpected: {err}");
    }

    #[test]
    fn capture_input_maps_from_the_legacy_audio_source() {
        assert_eq!(
            CaptureInput::from(&AudioSource::Default),
            CaptureInput::Default
        );
        assert_eq!(
            CaptureInput::from(&AudioSource::Device {
                name: "BlackHole 2ch".to_string()
            }),
            CaptureInput::Device("BlackHole 2ch".to_string())
        );
        assert_eq!(
            CaptureInput::from(&AudioSource::SystemAudio),
            CaptureInput::SystemAudio
        );
        // App audio has no legacy spelling — it is M6-only.
        match CaptureInput::AppAudio(vec![700]) {
            CaptureInput::AppAudio(pids) => assert_eq!(pids, vec![700]),
            other => panic!("expected app audio, got {other:?}"),
        }
    }
}
