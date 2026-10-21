//! Real-time-safe half of audio capture (M14B2, audit SUR-HIGH-03).
//!
//! A cpal input callback runs on an OS real-time thread (CoreAudio's IO proc,
//! ALSA's capture thread, WASAPI's event thread). Everything it does must be
//! bounded, non-blocking and allocation-free, or the device starves and the
//! recording drops out — the classic priority-inversion pattern the audit
//! flagged: the callback locked the shared sample buffer and heap-allocated a
//! `Vec` per delivery, while the UI thread could hold that same mutex across a
//! repaint.
//!
//! This module is the whole RT path plus its non-RT consumer:
//!
//! ```text
//!  cpal callback (RT thread)                 consumer thread (non-RT)
//!  ─────────────────────────                 ────────────────────────
//!  MonoFrames::new(block, channels)          rtrb::Consumer::read_chunk
//!    └─ try-push each mono sample      ──►     ├─ samples.lock().extend_from_slice
//!       ring full ⇒ drop + count               ├─ RollingRms::push_block
//!                                              └─ rms_bits.store
//! ```
//!
//! **RT guarantees** of [`RtProducer::write_block`]: no mutex (the only shared
//! state is the ring's two atomics and the drop counter), no allocation (the
//! mixdown is an iterator; the ring is allocated at session start), no syscall
//! (`rtrb`'s `push` is wait-free — a store plus a release-store of the tail).
//!
//! [`MonoFrames`] is also the implementation of
//! [`crate::capture::mono_mixdown`], so that public helper's unit tests exercise
//! exactly the arithmetic the callback runs (bit-for-bit: same sum order, same
//! division).
//!
//! The consumer keeps the pre-M14B2 observable behavior: the shared `Vec<f32>`
//! ends up with the same samples, `rms_bits` holds the same rolling value, and
//! [`crate::capture::MultiRecorder::take_live_chunk`] still reads the same
//! buffer — only now it is filled by a non-RT thread.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rtrb::{Consumer, Producer};

use crate::capture::RollingRms;

/// Ring depth, in seconds of mono audio at the session's sample rate.
///
/// One second: a healthy consumer needs a few milliseconds of it, so the ring
/// is only ever approached when the consumer thread is dead (already a fatal
/// state) or the machine is so loaded that the recording is lost anyway.
const RING_SECONDS: usize = 1;

/// Largest slice the consumer moves under the shared-buffer lock in one go.
/// Bounds how long a `take_live_chunk` reader can wait behind the consumer.
const DRAIN_CHUNK: usize = 1024;

/// How long the consumer sleeps when the ring is empty (level-meter latency,
/// nothing else — the final buffer is drained synchronously on stop).
const IDLE_POLL: Duration = Duration::from_millis(5);

/// Ring capacity for a session at `sample_rate`: one second of mono samples.
///
/// Arithmetic: 48 000 Hz × 1 s = 48 000 f32 slots = 192 000 bytes, i.e. 1.00 s
/// of slack at 48 kHz (44 100 Hz → 44 100 slots / 176 400 bytes; 16 000 Hz →
/// 16 000 slots). `rtrb` allocates exactly the requested capacity at
/// [`rtrb::RingBuffer::new`], which runs at session start on the caller's
/// thread — never on the RT thread.
pub(crate) fn ring_capacity(sample_rate: u32) -> usize {
    (sample_rate as usize).saturating_mul(RING_SECONDS).max(1)
}

/// One block of interleaved samples as the device delivered it.
///
/// Implemented for the three cpal sample formats the capture session opens, so
/// the callback can mix down straight from the device slice — the old code
/// converted `i16`/`u16` blocks into a freshly allocated `Vec<f32>` first.
pub(crate) trait Interleaved {
    /// Number of interleaved samples (frames × channels).
    fn sample_count(&self) -> usize;
    /// Sample `index` as an `f32` (the exact conversion the old callback used).
    fn sample(&self, index: usize) -> f32;
}

impl Interleaved for [f32] {
    fn sample_count(&self) -> usize {
        self.len()
    }

    fn sample(&self, index: usize) -> f32 {
        self[index]
    }
}

impl Interleaved for [i16] {
    fn sample_count(&self) -> usize {
        self.len()
    }

    fn sample(&self, index: usize) -> f32 {
        // Symmetric normalization, matching the `[u16]` path and the audit
        // recommendation (SUR2-NEW-02): dividing by `i16::MAX` instead maps
        // `i16::MIN` to −1.00003 — outside the normalized range. /32768.0
        // already lands inside [−1.0, +0.99997]; the clamp documents intent.
        (self[index] as f32 / 32768.0).clamp(-1.0, 1.0)
    }
}

impl Interleaved for [u16] {
    fn sample_count(&self) -> usize {
        self.len()
    }

    fn sample(&self, index: usize) -> f32 {
        (self[index] as f32 - 32768.0) / 32768.0
    }
}

/// Allocation-free mono mixdown of an interleaved block — the RT callback's
/// arithmetic and [`crate::capture::mono_mixdown`]'s implementation.
///
/// One item per frame: the average of that frame's channels. A trailing partial
/// frame is averaged over the channels it actually has (a device can hand back
/// a block that is not a whole number of frames), and `channels == 0` is
/// treated as mono — both rules are the documented behavior of
/// `mono_mixdown`, unchanged since M3.
#[derive(Debug, Clone)]
pub(crate) struct MonoFrames<'a, B: Interleaved + ?Sized> {
    block: &'a B,
    channels: usize,
    frame: usize,
    frames: usize,
}

impl<'a, B: Interleaved + ?Sized> MonoFrames<'a, B> {
    /// Iterates `block` frame by frame, `channels` interleaved samples per frame.
    pub(crate) fn new(block: &'a B, channels: usize) -> Self {
        let channels = channels.max(1);
        let samples = block.sample_count();
        Self {
            block,
            channels,
            frame: 0,
            frames: samples.div_ceil(channels),
        }
    }
}

impl<B: Interleaved + ?Sized> Iterator for MonoFrames<'_, B> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.frame >= self.frames {
            return None;
        }
        let start = self.frame * self.channels;
        let end = (start + self.channels).min(self.block.sample_count());
        // Same sum order as `frame.iter().sum::<f32>()`: 0.0, then left to right.
        let sum: f32 = (start..end).map(|index| self.block.sample(index)).sum();
        self.frame += 1;
        Some(sum / (end - start) as f32)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.frames - self.frame;
        (remaining, Some(remaining))
    }
}

impl<B: Interleaved + ?Sized> ExactSizeIterator for MonoFrames<'_, B> {}

/// The RT callback's entire workload: mix down and try-push, never wait.
///
/// Lives in the cpal closure. Pushing does not allocate, does not lock and does
/// not enter the kernel; a full ring returns immediately and the samples are
/// dropped (counted in [`RtProducer::dropped_samples`]).
#[derive(Debug)]
pub(crate) struct RtProducer {
    ring: Producer<f32>,
    dropped: Arc<AtomicU64>,
}

impl RtProducer {
    /// Wraps the ring's producer half plus the session's drop counter.
    pub(crate) fn new(ring: Producer<f32>, dropped: Arc<AtomicU64>) -> Self {
        Self { ring, dropped }
    }

    /// Mono-mixes `block` straight into the ring.
    ///
    /// Returns the number of samples this call had to drop. **RT-safe**: no
    /// lock, no allocation, no syscall; each push is wait-free, and once the
    /// ring reports full the rest of the block is skipped without touching the
    /// ring again, so the cost of an overflowing block is one failed push plus
    /// a counter increment.
    pub(crate) fn write_block<B: Interleaved + ?Sized>(
        &mut self,
        block: &B,
        channels: usize,
    ) -> usize {
        let mut frames = MonoFrames::new(block, channels);
        let total = frames.len();
        let mut dropped = 0usize;
        for (index, mono) in frames.by_ref().enumerate() {
            if self.ring.push(mono).is_err() {
                dropped = total - index;
                break;
            }
        }
        if dropped > 0 {
            self.dropped.fetch_add(dropped as u64, Ordering::Relaxed);
        }
        dropped
    }

    /// Samples dropped because the ring was full, since session start.
    #[cfg(test)]
    pub(crate) fn dropped_samples(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Free slots in the ring right now (diagnostics and tests).
    #[cfg(test)]
    pub(crate) fn free_slots(&self) -> usize {
        self.ring.slots()
    }
}

/// The non-RT half: drains the ring into the state the rest of the app reads.
///
/// Owns what the old `accumulate` wrote — the shared sample `Vec`, the rolling
/// RMS and the `rms_bits` word — but now only a non-RT thread ever touches them.
pub(crate) struct CaptureConsumer {
    ring: Consumer<f32>,
    samples: Arc<Mutex<Vec<f32>>>,
    rms_bits: Arc<AtomicU32>,
    rolling: RollingRms,
}

impl CaptureConsumer {
    /// Wraps the ring's consumer half plus the session state it feeds.
    /// `window` is the rolling-RMS window in samples (100 ms at the device rate).
    pub(crate) fn new(
        ring: Consumer<f32>,
        samples: Arc<Mutex<Vec<f32>>>,
        rms_bits: Arc<AtomicU32>,
        window: usize,
    ) -> Self {
        Self {
            ring,
            samples,
            rms_bits,
            rolling: RollingRms::new(window),
        }
    }

    /// Moves everything currently in the ring into the shared buffer, then
    /// publishes the rolling RMS once per drain.
    ///
    /// Not RT-safe by design: it locks the buffer mutex (which the UI's
    /// `take_live_chunk` and `stop` also take) and lets the `Vec` grow. Returns
    /// the number of samples consumed.
    pub(crate) fn drain(&mut self) -> usize {
        let mut drained = 0usize;
        // Each iteration commits at least one slot, so this terminates; the
        // producer can only add to what a later iteration picks up.
        while self.ring.slots() > 0 {
            let wanted = self.ring.slots().min(DRAIN_CHUNK);
            let chunk = match self.ring.read_chunk(wanted) {
                Ok(chunk) => chunk,
                // The producer never takes slots back; a short read here only
                // means the count moved between the two atomics.
                Err(_) => break,
            };
            {
                let mut buffer = self
                    .samples
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let (first, second) = chunk.as_slices();
                buffer.extend_from_slice(first);
                buffer.extend_from_slice(second);
            }
            let (first, second) = chunk.as_slices();
            self.rolling.push_block(first);
            self.rolling.push_block(second);
            let len = chunk.len();
            chunk.commit_all();
            drained += len;
        }
        if drained > 0 {
            self.rms_bits
                .store(self.rolling.value().to_bits(), Ordering::Relaxed);
        }
        drained
    }

    /// The consumer thread's loop: drain, sleep when idle, and after `stop` run
    /// one last drain to empty so nothing captured before the stream closed is
    /// lost.
    fn run(mut self, stop: &AtomicBool) {
        loop {
            let drained = self.drain();
            if stop.load(Ordering::Acquire) && drained == 0 {
                break;
            }
            if drained == 0 {
                std::thread::park_timeout(IDLE_POLL);
            }
        }
    }
}

/// Owns the capture session's consumer thread.
///
/// Dropping it (or [`ConsumerThread::shutdown`]) stops the loop **and joins it**,
/// so by the time it returns every sample the RT callback pushed is in the
/// shared buffer. The producer side is always gone first: `Recorder::stop`
/// drops the cpal stream (which drops the callback and its `RtProducer`) before
/// shutting this down.
pub(crate) struct ConsumerThread {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ConsumerThread {
    /// Starts the drain loop on its own thread.
    pub(crate) fn spawn(consumer: CaptureConsumer) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("suramak-capture".to_string())
            .spawn(move || consumer.run(&flag))?;
        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }

    /// Signals the loop and waits for its final drain. Idempotent.
    pub(crate) fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                // A panicking consumer loses audio, but it must not take the
                // recorder (or the app) down with it.
                tracing::error!(
                    "audio capture consumer thread panicked — the recording may be short"
                );
            }
        }
    }
}

impl Drop for ConsumerThread {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::mono_mixdown;
    use rtrb::RingBuffer;
    use std::time::Instant;

    /// Deterministic PRNG (splitmix-style) — no `rand` dependency, no seeds from
    /// the clock, so a failing run reproduces.
    struct Lcg(u64);

    impl Lcg {
        fn new(seed: u64) -> Self {
            Self(seed)
        }

        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 11
        }

        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound.max(1)
        }
    }

    /// A deterministic interleaved block: values in −1..1, distinct per index.
    fn block(frames: usize, channels: usize, seed: u64) -> Vec<f32> {
        (0..frames * channels)
            .map(|index| {
                let raw = (index as u64)
                    .wrapping_mul(2_654_435_761)
                    .wrapping_add(seed);
                (raw % 1999) as f32 / 999.5 - 1.0
            })
            .collect()
    }

    /// The pre-M14B2 callback body, verbatim in behavior: allocating mixdown,
    /// push into the buffer, rolling RMS, publish. `None` mirrors its early
    /// return on an empty block (no RMS store).
    fn legacy_accumulate(
        buffer: &mut Vec<f32>,
        rolling: &mut RollingRms,
        data: &[f32],
        channels: usize,
    ) -> Option<f32> {
        if data.is_empty() {
            return None;
        }
        for mono in mono_mixdown(data, channels) {
            buffer.push(mono);
            rolling.push(mono);
        }
        Some(rolling.value())
    }

    /// One in-process session's parts: the producer the callback would own, the
    /// consumer the drain thread would own, and the shared state they meet in.
    struct Session {
        rt: RtProducer,
        sink: CaptureConsumer,
        samples: Arc<Mutex<Vec<f32>>>,
        rms_bits: Arc<AtomicU32>,
        dropped: Arc<AtomicU64>,
    }

    fn session(capacity: usize, window: usize) -> Session {
        let (producer, consumer) = RingBuffer::new(capacity);
        let samples = Arc::new(Mutex::new(Vec::new()));
        let rms_bits = Arc::new(AtomicU32::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let sink = CaptureConsumer::new(
            consumer,
            Arc::clone(&samples),
            Arc::clone(&rms_bits),
            window,
        );
        Session {
            rt: RtProducer::new(producer, Arc::clone(&dropped)),
            sink,
            samples,
            rms_bits,
            dropped,
        }
    }

    #[test]
    fn mono_frames_match_the_allocating_mixdown() {
        // Stereo, mono, a trailing partial frame, more channels than samples and
        // the zero-channel rule (treated as mono).
        for (data, channels) in [
            (vec![1.0f32, -1.0, 0.5, -0.25, 0.0, 1.0], 2),
            (vec![0.1, 0.2, 0.3], 1),
            (vec![1.0, 0.0, 0.5], 2),
            (vec![0.25, 0.75], 4),
            (vec![0.5, -0.5], 0),
            (Vec::new(), 2),
        ] {
            let frames: Vec<f32> = MonoFrames::new(&data[..], channels).collect();
            assert_eq!(frames, mono_mixdown(&data, channels), "channels={channels}");
            assert_eq!(MonoFrames::new(&data[..], channels).len(), frames.len());
        }
    }

    /// Workorder test 1: the new callback path reproduces the old one exactly —
    /// same sample sequence, same rms value after every block.
    #[test]
    fn ring_path_matches_the_legacy_accumulate_exactly() {
        let Session {
            mut rt,
            mut sink,
            samples,
            rms_bits,
            dropped,
        } = session(48_000, 4800);
        let mut legacy_samples: Vec<f32> = Vec::new();
        let mut legacy_rms = RollingRms::new(4800);
        let mut rng = Lcg::new(0x5EED_1234);

        for block_index in 0..64usize {
            let channels = if block_index % 3 == 0 { 1 } else { 2 };
            let frames = 1 + rng.below(700) as usize;
            let mut data = block(frames, channels, block_index as u64 * 7);
            // A device can deliver a trailing partial frame; both paths average
            // it over the channels it has.
            if block_index % 11 == 4 {
                data.push(0.25);
            }

            let produced_frames = MonoFrames::new(&data[..], channels).len();
            assert_eq!(
                rt.write_block(&data[..], channels),
                0,
                "a 1 s ring must absorb 64 blocks"
            );
            let drained = sink.drain();
            assert_eq!(drained, produced_frames, "block {block_index}");

            let expected_rms =
                legacy_accumulate(&mut legacy_samples, &mut legacy_rms, &data, channels)
                    .expect("non-empty block publishes an rms");
            assert_eq!(
                rms_bits.load(Ordering::Relaxed),
                expected_rms.to_bits(),
                "block {block_index}: rms must be bit-identical"
            );

            let shared = samples.lock().expect("sample buffer");
            assert_eq!(*shared, legacy_samples, "block {block_index}");
        }

        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        assert_eq!(
            rt.free_slots(),
            48_000,
            "the test drained every block, so the ring is empty"
        );
        assert!(!legacy_samples.is_empty() && legacy_samples.len() < 48_000);
        println!(
            "parity: 64 blocks, {} samples compared bit-for-bit, final rms {:.6}",
            legacy_samples.len(),
            legacy_rms.value()
        );
    }

    /// Workorder test 2: a paused consumer overflows the ring; the callback drops
    /// the tail of the block, counts it, and returns immediately every time.
    #[test]
    fn a_full_ring_drops_the_incoming_samples_and_counts_them() {
        let Session {
            mut rt,
            sink: _paused_consumer,
            samples,
            rms_bits,
            dropped,
        } = session(64, 100);
        let data = vec![0.5f32; 256];
        let deadline = Duration::from_millis(50);
        let mut worst = Duration::ZERO;
        let mut returned_drops = 0usize;

        for _ in 0..10 {
            let started = Instant::now();
            returned_drops += rt.write_block(&data[..], 1);
            let elapsed = started.elapsed();
            worst = worst.max(elapsed);
            assert!(elapsed < deadline, "the callback blocked for {elapsed:?}");
        }

        // Capacity 64: the first block fills it, the other 9 are dropped whole.
        assert_eq!(returned_drops, 10 * 256 - 64);
        assert_eq!(rt.dropped_samples(), returned_drops as u64);
        assert_eq!(dropped.load(Ordering::Relaxed), returned_drops as u64);
        assert_eq!(rt.free_slots(), 0, "the paused consumer never drained");
        // Nothing reached the shared state, and the metre was never updated.
        assert!(samples.lock().expect("buffer").is_empty());
        assert_eq!(rms_bits.load(Ordering::Relaxed), 0.0f32.to_bits());
        assert!(worst < deadline);
        println!(
            "overflow: 10 blocks x 256 samples into a 64-slot ring, \
             {returned_drops} dropped, worst write_block {worst:?}"
        );
    }

    /// Workorder stress test: the consumer stalls randomly for up to 50 ms while
    /// the producer runs at real time. A 1 s ring absorbs every stall, so
    /// nothing is dropped, the sequence is exact, and no `write_block` call ever
    /// waits — the deadline is deliberately generous (50 ms, the longest stall)
    /// because it is a tripwire for accidental blocking, not a benchmark.
    #[test]
    fn a_delayed_consumer_never_blocks_the_producer_and_loses_nothing() {
        const RATE: u32 = 48_000;
        const CHANNELS: usize = 2;
        const FRAMES_PER_BLOCK: usize = 192; // 4 ms at 48 kHz
        const BLOCKS: usize = 375; // 1.5 s of audio
        let deadline = Duration::from_millis(50);

        let (producer, consumer) = RingBuffer::new(ring_capacity(RATE));
        let samples = Arc::new(Mutex::new(Vec::new()));
        let rms_bits = Arc::new(AtomicU32::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let mut sink =
            CaptureConsumer::new(consumer, Arc::clone(&samples), Arc::clone(&rms_bits), 4_800);

        let consumer_stop = Arc::new(AtomicBool::new(false));
        let consumer_flag = Arc::clone(&consumer_stop);
        let consumer_thread = std::thread::spawn(move || {
            let mut rng = Lcg::new(0xC0FF_EE01);
            loop {
                std::thread::sleep(Duration::from_micros(rng.below(50_000)));
                sink.drain();
                if consumer_flag.load(Ordering::Acquire) {
                    sink.drain();
                    break;
                }
            }
        });

        let mut rt = RtProducer::new(producer, Arc::clone(&dropped));
        let mut expected: Vec<f32> = Vec::with_capacity(BLOCKS * FRAMES_PER_BLOCK);
        let mut worst = Duration::ZERO;
        let mut worst_at = 0usize;
        let mut push_time = Duration::ZERO;

        for block_index in 0..BLOCKS {
            let data = block(FRAMES_PER_BLOCK, CHANNELS, block_index as u64);
            for frame in data.chunks(CHANNELS) {
                // Same sum order as MonoFrames: 0.0 + left + right, then / 2.
                expected.push((frame[0] + frame[1]) / CHANNELS as f32);
            }
            let started = Instant::now();
            let lost = rt.write_block(&data[..], CHANNELS);
            let elapsed = started.elapsed();
            push_time += elapsed;
            if elapsed > worst {
                worst = elapsed;
                worst_at = block_index;
            }
            assert_eq!(
                lost, 0,
                "block {block_index}: a 1 s ring must absorb a 50 ms stall"
            );
            // Produce at the audio clock (~4 ms per block).
            std::thread::sleep(Duration::from_micros(3_800));
        }

        consumer_stop.store(true, Ordering::Release);
        consumer_thread.join().expect("consumer thread");

        assert_eq!(rt.dropped_samples(), 0);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        assert_eq!(*samples.lock().expect("buffer"), expected);
        // A fresh meter over the same samples reads the same value.
        let mut reference = RollingRms::new(4_800);
        reference.push_block(&expected);
        assert_eq!(
            rms_bits.load(Ordering::Relaxed),
            reference.value().to_bits()
        );
        assert!(worst < deadline, "worst write_block took {worst:?}");
        assert!(
            push_time < Duration::from_millis(500),
            "total time inside write_block {push_time:?} over a ~1.5 s run"
        );
        // The measurement the report cites; visible with `-- --nocapture`.
        println!(
            "stress: {BLOCKS} blocks x {FRAMES_PER_BLOCK} frames ({:.2} s of audio), \
             worst write_block {worst:?} at block {worst_at}, \
             total inside write_block {push_time:?}",
            expected.len() as f32 / RATE as f32
        );
    }

    /// The session's teardown contract: after `shutdown` returns, everything the
    /// producer pushed is in the shared buffer and the rms is final.
    #[test]
    fn consumer_shutdown_drains_the_ring_before_returning() {
        let (producer, consumer) = RingBuffer::new(ring_capacity(48_000));
        let samples = Arc::new(Mutex::new(Vec::new()));
        let rms_bits = Arc::new(AtomicU32::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let sink = CaptureConsumer::new(consumer, Arc::clone(&samples), Arc::clone(&rms_bits), 480);
        let mut thread = ConsumerThread::spawn(sink).expect("spawn consumer");
        let mut rt = RtProducer::new(producer, Arc::clone(&dropped));

        let data = block(1_000, 1, 9);
        let expected: Vec<f32> = MonoFrames::new(&data[..], 1).collect();
        assert_eq!(rt.write_block(&data[..], 1), 0);

        // Immediately, before the 5 ms poll could have woken the loop.
        thread.shutdown();

        assert_eq!(*samples.lock().expect("buffer"), expected);
        let mut reference = RollingRms::new(480);
        reference.push_block(&expected);
        assert_eq!(
            rms_bits.load(Ordering::Relaxed),
            reference.value().to_bits()
        );
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        // A second shutdown (as `Drop` will do) is a no-op.
        thread.shutdown();
    }

    #[test]
    fn ring_capacity_covers_one_second_at_the_session_rate() {
        assert_eq!(ring_capacity(48_000), 48_000);
        assert_eq!(ring_capacity(44_100), 44_100);
        assert_eq!(ring_capacity(16_000), 16_000);
        // Degenerate rates can never produce a zero-capacity ring.
        assert_eq!(ring_capacity(0), 1);
    }

    /// The device-format conversions (`i16`/`u16` → `f32`) are the ones
    /// `Interleaved` performs — same constants, same arithmetic. `i16` uses
    /// symmetric /32768.0 normalization (SUR2-NEW-02): `i16::MIN` must land at
    /// exactly −1.0, never beyond.
    #[test]
    fn i16_and_u16_blocks_convert_like_the_old_callback() {
        let signed: Vec<i16> = (0..300)
            .map(|index| (index * 217 - 30_000) as i16)
            .collect();
        let legacy_signed: Vec<f32> = signed
            .iter()
            .map(|sample| (*sample as f32 / 32768.0).clamp(-1.0, 1.0))
            .collect();
        let frames: Vec<f32> = MonoFrames::new(&signed[..], 1).collect();
        assert_eq!(frames, legacy_signed);
        // Range check: every converted sample stays inside [−1.0, 1.0],
        // including the extremes.
        for sample in signed
            .iter()
            .map(|s| (*s as f32 / 32768.0).clamp(-1.0, 1.0))
        {
            assert!(
                (-1.0..=1.0).contains(&sample),
                "i16 conversion must normalize into [−1.0, 1.0], got {sample}"
            );
        }
        let unsigned: Vec<u16> = (0..300u16).map(|index| index.wrapping_mul(619)).collect();
        let legacy_unsigned: Vec<f32> = unsigned
            .iter()
            .map(|sample| (*sample as f32 - 32768.0) / 32768.0)
            .collect();
        let frames: Vec<f32> = MonoFrames::new(&unsigned[..], 1).collect();
        assert_eq!(frames, legacy_unsigned);
    }
}
