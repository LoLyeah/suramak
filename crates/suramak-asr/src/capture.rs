//! Microphone capture (feature `capture`, OFF by default): default input device →
//! f32 mono accumulation with a live RMS value. No UI code lives here.
//!
//! With the feature off the module still compiles and reports capture as unsupported,
//! so dependents can call it unconditionally.

#[cfg(feature = "capture")]
mod imp {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::StreamConfig;
    use suramak_core::decode::AudioBuffer;

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

        /// Live RMS level (0.0–1.0) of the most recent captured block.
        pub fn rms(&self) -> f32 {
            f32::from_bits(self.rms_bits.load(Ordering::Relaxed))
        }

        /// Stops capture and returns the accumulated mono samples as an [`AudioBuffer`]
        /// (channels = 1) ready for 16 kHz resampling via `suramak_core::resample`.
        pub fn stop(self) -> AudioBuffer {
            let Recorder {
                stream,
                samples,
                sample_rate,
                device_name: _,
                rms_bits: _,
            } = self;
            drop(stream); // stops the cpal stream
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

    /// Lists input device names on the default host.
    pub fn list_input_devices() -> Result<Vec<String>, AsrError> {
        let host = cpal::default_host();
        let devices = host
            .input_devices()
            .map_err(|e| AsrError::Capture(e.to_string()))?;
        let mut names = Vec::new();
        for device in devices {
            let name = device
                .name()
                .unwrap_or_else(|_| "<unnamed device>".to_string());
            names.push(name);
        }
        Ok(names)
    }

    /// Starts capture on the default input device (default stream config, f32 mono).
    pub fn start_recording() -> Result<Recorder, AsrError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| AsrError::Capture("no default input device".to_string()))?;
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

        let err_fn = |err: cpal::StreamError| eprintln!("audio capture error: {err}");

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let (samples, rms_bits) = (samples.clone(), rms_bits.clone());
                let channels = config.channels as usize;
                device
                    .build_input_stream(
                        &config,
                        move |data: &[f32], _| accumulate(&samples, &rms_bits, data, channels),
                        err_fn,
                        None,
                    )
                    .map_err(|e| AsrError::Capture(e.to_string()))?
            }
            cpal::SampleFormat::I16 => {
                let (samples, rms_bits) = (samples.clone(), rms_bits.clone());
                let channels = config.channels as usize;
                device
                    .build_input_stream(
                        &config,
                        move |data: &[i16], _| {
                            let floats: Vec<f32> =
                                data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                            accumulate(&samples, &rms_bits, &floats, channels);
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| AsrError::Capture(e.to_string()))?
            }
            cpal::SampleFormat::U16 => {
                let (samples, rms_bits) = (samples.clone(), rms_bits.clone());
                let channels = config.channels as usize;
                device
                    .build_input_stream(
                        &config,
                        move |data: &[u16], _| {
                            let floats: Vec<f32> = data
                                .iter()
                                .map(|s| (*s as f32 - 32768.0) / 32768.0)
                                .collect();
                            accumulate(&samples, &rms_bits, &floats, channels);
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
        })
    }

    /// Mono-mixes one interleaved block into `samples` and updates the live RMS value.
    fn accumulate(samples: &Mutex<Vec<f32>>, rms_bits: &AtomicU32, data: &[f32], channels: usize) {
        let channels = channels.max(1);
        let mut buffer = samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut sum_sq = 0.0f32;
        let mut frames = 0usize;
        for frame in data.chunks(channels) {
            let mono = frame.iter().sum::<f32>() / frame.len() as f32;
            buffer.push(mono);
            sum_sq += mono * mono;
            frames += 1;
        }
        if frames > 0 {
            let rms = (sum_sq / frames as f32).sqrt();
            rms_bits.store(rms.to_bits(), Ordering::Relaxed);
        }
    }
}

#[cfg(feature = "capture")]
pub use imp::{list_input_devices, start_recording, Recorder};

#[cfg(not(feature = "capture"))]
mod imp {
    use crate::whisper::AsrError;

    /// Placeholder for builds without the `capture` feature; can never be constructed.
    pub struct Recorder {
        _private: (),
    }

    /// Capture support is not compiled in — reports unsupported.
    pub fn list_input_devices() -> Result<Vec<String>, AsrError> {
        Err(AsrError::CaptureUnsupported)
    }

    /// Capture support is not compiled in — reports unsupported.
    pub fn start_recording() -> Result<Recorder, AsrError> {
        Err(AsrError::CaptureUnsupported)
    }
}

#[cfg(not(feature = "capture"))]
pub use imp::{list_input_devices, start_recording, Recorder};
