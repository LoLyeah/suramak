//! Android microphone capture over cpal's Oboe/AAudio backend (M13F).
//!
//! This is the Android arm of the M6/M14B2 capture lineage:
//!
//! * the cpal callback does exactly what [`crate::capture_rt::RtProducer`]
//!   does on the desktop — allocation-free mono mixdown into an SPSC ring;
//! * a non-RT consumer thread drains the ring into
//!   [`crate::capture_stream::StreamingWav`], which cuts the take into the same
//!   1024-sample chunks the desktop `to_16khz_mono` uses and appends the 16 kHz
//!   PCM16 output to the WAV as it arrives.
//!
//! Why streaming instead of the desktop's buffer-then-stop: a locked-screen
//! phone take is thirty minutes or longer (≈345 MB of 48 kHz f32 if buffered),
//! and a process kill must leave a playable file. The final bytes are still
//! byte-identical to the desktop writer for the same samples — the streaming
//! math is `capture_stream`'s contract, tested on the host.
//!
//! `cpal::Stream` is `!Send`, so an [`AndroidRecorder`] must be created, polled
//! and stopped on one thread. The mic host owns a dedicated capture thread and
//! never moves the recorder across threads.
//!
//! Feature/target gate: compiled only for `target_os = "android"` with the
//! `capture` feature (cpal's Oboe backend is an Android-only dependency of
//! cpal 0.15.3; no feature flip is needed — verified against the crate's
//! manifest, which pulls `oboe 0.6` for this target unconditionally).

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::StreamConfig;
use rtrb::{Consumer, RingBuffer};

use crate::capture::RollingRms;
use crate::capture_rt::{ring_capacity, RtProducer};
use crate::capture_stream::{StreamedWav, StreamingWav};
use crate::whisper::AsrError;

/// How long the consumer sleeps when the ring is empty.
const IDLE_POLL: Duration = Duration::from_millis(5);

/// Largest block the consumer moves per iteration (bounded latency for the
/// RMS meter and the journal's elapsed updates).
const DRAIN_LIMIT: usize = 4096;

/// A live AAudio capture session.
///
/// Dropping it (or calling [`stop`](Self::stop)) ends capture. The recorder
/// deliberately exposes no samples: the WAV on disk is the recording.
pub struct AndroidRecorder {
    stream: Option<cpal::Stream>,
    /// Set before the stream is dropped, so the stop-time device race stays
    /// quiet (the same rule as the desktop `Recorder`).
    teardown: Arc<AtomicBool>,
    /// Tells the consumer thread to run its final drain and finish the WAV.
    consumer_stop: Arc<AtomicBool>,
    consumer: Option<std::thread::JoinHandle<Result<StreamedWav, String>>>,
    rms_bits: Arc<AtomicU32>,
    dropped: Arc<AtomicU64>,
    error: Arc<Mutex<Option<String>>>,
    device_name: String,
    sample_rate: u32,
    started: Instant,
}

impl AndroidRecorder {
    /// Opens the default input device and starts delivering into a new
    /// streaming WAV at `path`.
    ///
    /// The WAV is created after the device's native rate is known, so the
    /// resampler is configured for exactly that rate. A failed open leaves a
    /// 44-byte empty WAV at `path` (the mic host removes it or journals it).
    pub fn start(path: &Path) -> Result<Self, AsrError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| AsrError::Capture("no default microphone on this device".to_string()))?;
        let device_name = device
            .name()
            .unwrap_or_else(|_| "<unnamed microphone>".to_string());
        let supported = device.default_input_config().map_err(|err| {
            AsrError::Capture(format!("the microphone has no input config: {err}"))
        })?;
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let sample_rate = config.sample_rate.0;
        let wav = StreamingWav::create(path, sample_rate)?;
        let channels = config.channels as usize;
        let window = ((u128::from(sample_rate) * 100) / 1000).max(1) as usize; // 100 ms

        let (producer, consumer) = RingBuffer::new(ring_capacity(sample_rate));
        let rms_bits = Arc::new(AtomicU32::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let consumer_stop = Arc::new(AtomicBool::new(false));
        let teardown = Arc::new(AtomicBool::new(false));
        let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let consumer_handle = {
            let stop = Arc::clone(&consumer_stop);
            let rms = Arc::clone(&rms_bits);
            std::thread::Builder::new()
                .name("suramak-android-capture".to_string())
                .spawn(move || consume(consumer, wav, rms, stop, window))
                .map_err(|err| {
                    AsrError::Capture(format!(
                        "could not start the audio capture consumer thread: {err}"
                    ))
                })?
        };

        let stream = match build_stream(
            &device,
            &config,
            sample_format,
            channels,
            producer,
            Arc::clone(&dropped),
            Arc::clone(&teardown),
            Arc::clone(&error),
        ) {
            Ok(stream) => stream,
            Err(err) => {
                // Do not leave the consumer parked on a session that never
                // opened: it finishes the empty WAV and joins.
                consumer_stop.store(true, Ordering::Release);
                let _ = consumer_handle.join();
                return Err(err);
            }
        };
        if let Err(err) = stream.play() {
            // Tear the consumer down before reporting: no thread may outlive a
            // failed session.
            teardown.store(true, Ordering::Relaxed);
            drop(stream);
            consumer_stop.store(true, Ordering::Release);
            let _ = consumer_handle.join();
            return Err(AsrError::Capture(format!(
                "the microphone stream could not start: {err}"
            )));
        }

        Ok(Self {
            stream: Some(stream),
            teardown,
            consumer_stop,
            consumer: Some(consumer_handle),
            rms_bits,
            dropped,
            error,
            device_name,
            sample_rate,
            started: Instant::now(),
        })
    }

    /// The input device's name as the platform reports it.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// The device's native sample rate (what the WAV is resampled from).
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Live rolling RMS (100 ms window) of the capture.
    pub fn rms(&self) -> f32 {
        f32::from_bits(self.rms_bits.load(Ordering::Relaxed))
    }

    /// Samples the ring had to drop (only non-zero when the consumer thread
    /// starved — a dead thread or an overloaded phone).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// A stream error reported by the backend since the last call, if any.
    ///
    /// The mic host polls this on its status path and turns it into a clean
    /// stop ([`crate::mic::StopReason::StreamError`]) rather than letting the
    /// take silently stop growing.
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|mut slot| slot.take())
    }

    /// Wall-clock time since the stream started.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Stops the stream, drains the ring and finalises the WAV.
    pub fn stop(mut self) -> Result<StreamedWav, AsrError> {
        self.teardown.store(true, Ordering::Relaxed);
        // The stream first: its drop stops the RT callback and drops the ring
        // producer, so the consumer's final drain sees a closed ring.
        drop(self.stream.take());
        self.consumer_stop.store(true, Ordering::Release);
        let joined = self
            .consumer
            .take()
            .map(|handle| handle.join())
            .unwrap_or_else(|| {
                Ok(Err(
                    "the capture consumer thread was already gone".to_string()
                ))
            });
        match joined {
            Ok(Ok(wav)) => Ok(wav),
            Ok(Err(reason)) => Err(AsrError::Capture(reason)),
            Err(_) => Err(AsrError::Capture(
                "the audio capture consumer thread panicked; the recording may be short"
                    .to_string(),
            )),
        }
    }
}

impl Drop for AndroidRecorder {
    fn drop(&mut self) {
        // A recorder dropped without `stop` (the failure paths) still has to
        // end the stream and join the consumer; `stop` is the only way to get
        // the finished WAV, and `Drop` mirrors its teardown order.
        self.teardown.store(true, Ordering::Relaxed);
        drop(self.stream.take());
        self.consumer_stop.store(true, Ordering::Release);
        if let Some(handle) = self.consumer.take() {
            let _ = handle.join();
        }
    }
}

/// Builds the cpal input stream for the device's sample format.
#[allow(clippy::too_many_arguments)]
fn build_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    sample_format: cpal::SampleFormat,
    channels: usize,
    producer: rtrb::Producer<f32>,
    dropped: Arc<AtomicU64>,
    teardown: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, AsrError> {
    let sink = RtProducer::new(producer, dropped);
    let err_slot = Arc::clone(&error);
    let teardown_for_error = Arc::clone(&teardown);
    let on_error = move |err: cpal::StreamError| {
        if !teardown_for_error.load(Ordering::Relaxed) {
            let line = format!("AAudio capture error: {err}");
            eprintln!("{line}");
            if let Ok(mut slot) = err_slot.lock() {
                *slot = Some(line);
            }
        }
    };
    match sample_format {
        cpal::SampleFormat::F32 => open::<f32>(device, config, sink, channels, on_error),
        cpal::SampleFormat::I16 => open::<i16>(device, config, sink, channels, on_error),
        cpal::SampleFormat::U16 => open::<u16>(device, config, sink, channels, on_error),
        other => Err(AsrError::Capture(format!(
            "the microphone reports an unsupported sample format {other:?}"
        ))),
    }
}

/// One sample format's stream: the callback is [`RtProducer::write_block`],
/// exactly the desktop capture callback whose mixdown the ring tests pin.
fn open<T: cpal::SizedSample + 'static>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut sink: RtProducer,
    channels: usize,
    on_error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, AsrError>
where
    [T]: WriteInterleaved,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                data.write_interleaved(&mut sink, channels);
            },
            on_error,
            None,
        )
        .map_err(|err| AsrError::Capture(format!("could not open the microphone: {err}")))
}

/// The three formats cpal's Oboe backend opens, connected to the same
/// allocation-free mixdown the desktop callback uses.
trait WriteInterleaved {
    fn write_interleaved(&self, sink: &mut RtProducer, channels: usize);
}

impl WriteInterleaved for [f32] {
    fn write_interleaved(&self, sink: &mut RtProducer, channels: usize) {
        sink.write_block(self, channels);
    }
}

impl WriteInterleaved for [i16] {
    fn write_interleaved(&self, sink: &mut RtProducer, channels: usize) {
        sink.write_block(self, channels);
    }
}

impl WriteInterleaved for [u16] {
    fn write_interleaved(&self, sink: &mut RtProducer, channels: usize) {
        sink.write_block(self, channels);
    }
}

/// The non-RT consumer: drains the ring into the streaming WAV writer and the
/// rolling RMS, then finishes the file after the producer is gone.
fn consume(
    mut ring: Consumer<f32>,
    mut wav: StreamingWav,
    rms_bits: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    window: usize,
) -> Result<StreamedWav, String> {
    let mut rolling = RollingRms::new(window);
    let mut block: Vec<f32> = Vec::with_capacity(DRAIN_LIMIT);
    loop {
        let available = ring.slots();
        if available > 0 {
            let wanted = available.min(DRAIN_LIMIT);
            match ring.read_chunk(wanted) {
                Ok(chunk) => {
                    let (first, second) = chunk.as_slices();
                    block.clear();
                    block.extend_from_slice(first);
                    block.extend_from_slice(second);
                    chunk.commit_all();
                    if let Err(err) = wav.push(&block) {
                        return Err(err.to_string());
                    }
                    rolling.push_block(&block);
                    rms_bits.store(rolling.value().to_bits(), Ordering::Relaxed);
                    continue;
                }
                Err(_) => break,
            }
        }
        if stop.load(Ordering::Acquire) && ring.slots() == 0 {
            break;
        }
        std::thread::park_timeout(IDLE_POLL);
    }
    wav.finish().map_err(|err| err.to_string())
}
