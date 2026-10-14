//! Live transcription (M7) — windowed partial decodes while recording.
//!
//! The record view shows a growing **provisional** transcript while capture
//! runs. This module owns the whole live story except the device I/O:
//!
//! * [`LiveScheduler`] — the **pure** windowing/backlog state machine: it
//!   consumes the 16 kHz mono stream (the same stream [`crate::mix`] produces
//!   for the recording), closes a window every `step_ms` of new audio, carries
//!   `overlap_ms` of context into the next window's head, and implements the
//!   backlog guard (a window that closes while the previous decode is still
//!   running is **skipped and merged** into the next one; the per-session count
//!   of skipped windows is kept). Values in, values out — no devices, no
//!   threads, no cargo feature, so Linux CI unit-tests the whole policy.
//! * [`LiveWindowQueue`] — the bounded **drop-oldest** queue between the
//!   scheduler and the decoder (the newest audio is what the provisional view
//!   needs most).
//! * [`ProvisionalTranscript`] — the merge of decoded window segments onto the
//!   session timeline: segments fully inside the already-covered range (the
//!   overlap re-decode) are dropped, so a window boundary never duplicates the
//!   text in the provisional list.
//! * [`LiveSession`] — the pure glue the runtime drives: audio in, windows out,
//!   decoded segments in, a growing segment list out.
//! * [`LiveDecoder`] — the (impure) decode thread: it rides the **existing**
//!   whisper machinery ([`crate::whisper::transcribe`] with the session's own
//!   model / language / initial prompt / engine plan), one window at a time,
//!   on its own thread — capture is never blocked.
//!
//! ## The hard rule (the final Stop pass is untouched)
//!
//! Nothing here touches the final (Stop) pass: the recording WAV write, the
//! `transcribe_file` pipeline, the worker and its events are all unchanged, so
//! the final artifact for the same input is exactly what the record-then-
//! transcribe flow produced before M7. The provisional text is display-only and
//! is discarded on Stop; the final pass **replaces** it. The ignored tiny-model
//! e2e (`e2e_live_partials_then_the_final_pass_replaces_them`) pins the final
//! pass byte-for-byte across a live session.
//!
//! ## Cost policy (never starve capture)
//!
//! The runtime's per-frame feed only *reads* the already-captured buffers and
//! appends them to the scheduler; decoding happens on its own thread behind a
//! bounded queue. When the decoder cannot keep up, the guard degrades **live
//! updates only** — never the recording: skipped windows are counted, logged,
//! and surfaced subtly in the record view. Slower machines may lag live
//! updates; the final pass is unaffected (the workorder's accepted trade-off).
//!
//! ## Known limits (documented, provisional-only)
//!
//! * A phrase spoken across a window boundary can briefly appear twice in the
//!   provisional list: whisper re-decodes the boundary with the overlap context
//!   and may re-segment it. The overlap drop removes the exact duplicates; the
//!   final pass replaces the whole list, so this never reaches a transcript.
//! * The live stream is mixed chunk-wise by the runtime (per-source chunks
//!   through [`crate::mix`] with no session, i.e. no clock-drift correction) —
//!   the final recording is still the one full-session drift-corrected mix.
//! * The live decode loads the model on its own thread; the first provisional
//!   words appear after the first window (~`step_ms`) plus model load.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use suramak_core::resample::TARGET_RATE;
use suramak_core::settings::LangMode;
use suramak_core::transcript::Segment;

use crate::cancel::CancelToken;
use crate::engine::EngineDecision;
use crate::whisper::{self, AsrError, ContextCache};

/// Name of the live decode thread (visible in debuggers/profilers).
pub const LIVE_DECODE_THREAD_NAME: &str = "suramak-live-decode";

/// Poll interval while [`LiveDecoder::shutdown_and_wait`] waits for the decode
/// thread to exit (the quit path's teardown — WORKORDER-HOTFIX-QUIT).
const SHUTDOWN_POLL: std::time::Duration = std::time::Duration::from_millis(2);

/// Workorder bound of the window cadence (new audio per window).
pub const STEP_MS_RANGE: std::ops::RangeInclusive<u32> = 4_000..=6_000;
/// Workorder bound of the context overlap carried between windows.
pub const OVERLAP_MS_RANGE: std::ops::RangeInclusive<u32> = 1_000..=2_000;
/// Bound of the decoder's window queue (drop-oldest past this).
pub const MAX_QUEUED_RANGE: std::ops::RangeInclusive<usize> = 1..=8;

/// A segment whose end lies within this many ms of the covered frontier is a
/// boundary artifact of the overlap re-decode, not new content (whisper
/// timestamps are centiseconds; 50 ms is below any word).
pub const MERGE_TOLERANCE_MS: i64 = 50;

/// Live window cadence and backlog policy (M7).
///
/// The defaults sit inside the workorder bounds (cadence 4–6 s, overlap
/// 1–2 s): a window closes every 4 s of new audio and carries the previous
/// 1.5 s of context into its head, so words spoken across a boundary decode
/// with their context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveConfig {
    /// Sample rate of the live stream — the one recording rate (16 kHz).
    pub sample_rate: u32,
    /// New audio per window (the cadence), in milliseconds.
    pub step_ms: u32,
    /// Context overlap from the previous window, in milliseconds.
    pub overlap_ms: u32,
    /// Bound of the decoder's window queue (drop-oldest past this).
    pub max_queued: usize,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            sample_rate: TARGET_RATE,
            step_ms: 4_000,
            overlap_ms: 1_500,
            max_queued: 2,
        }
    }
}

impl LiveConfig {
    /// Checks the config against the documented bounds.
    pub fn validate(&self) -> Result<(), String> {
        if self.sample_rate != TARGET_RATE {
            return Err(format!(
                "the live stream must be {TARGET_RATE} Hz mono (got {})",
                self.sample_rate
            ));
        }
        if !STEP_MS_RANGE.contains(&self.step_ms) {
            return Err(format!(
                "window cadence {} ms is outside the supported {}–{} ms",
                self.step_ms,
                STEP_MS_RANGE.start(),
                STEP_MS_RANGE.end()
            ));
        }
        if !OVERLAP_MS_RANGE.contains(&self.overlap_ms) {
            return Err(format!(
                "context overlap {} ms is outside the supported {}–{} ms",
                self.overlap_ms,
                OVERLAP_MS_RANGE.start(),
                OVERLAP_MS_RANGE.end()
            ));
        }
        if self.overlap_ms >= self.step_ms {
            return Err("the context overlap must be shorter than the window step".to_string());
        }
        if !MAX_QUEUED_RANGE.contains(&self.max_queued) {
            return Err(format!(
                "queue bound {} is outside the supported {}–{}",
                self.max_queued,
                MAX_QUEUED_RANGE.start(),
                MAX_QUEUED_RANGE.end()
            ));
        }
        Ok(())
    }

    /// Samples per window step (the cadence).
    pub fn step_samples(&self) -> usize {
        ms_to_samples(self.step_ms, self.sample_rate)
    }

    /// Samples of context overlap carried between windows.
    pub fn overlap_samples(&self) -> usize {
        ms_to_samples(self.overlap_ms, self.sample_rate)
    }
}

/// `ms` → samples at `rate` (exact for the 16 kHz recording rate).
pub fn ms_to_samples(ms: u32, rate: u32) -> usize {
    (u64::from(ms) * u64::from(rate) / 1000) as usize
}

/// `samples` → session milliseconds at `rate` (truncating).
pub fn samples_to_ms(samples: usize, rate: u32) -> i64 {
    (samples as u64 * 1000 / u64::from(rate.max(1))) as i64
}

/// One window handed to the decoder: 16 kHz mono samples plus their place on
/// the session timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveWindow {
    /// Window sequence number, 0-based, in emission order.
    pub seq: u64,
    /// Session sample index of `samples[0]` (includes the overlap context).
    pub start_sample: usize,
    /// Session sample index where this window's **new** content begins; the
    /// samples before it are the overlap re-decode of the previous window.
    pub new_start_sample: usize,
    /// Session sample index just past the window (exclusive).
    pub end_sample: usize,
    /// The audio: `[start_sample, end_sample)` at the live stream's rate.
    pub samples: Vec<f32>,
}

impl LiveWindow {
    /// Session-time start of the whole window (ms, includes the overlap).
    pub fn start_ms(&self, rate: u32) -> i64 {
        samples_to_ms(self.start_sample, rate)
    }

    /// Session-time start of the window's new content (ms).
    pub fn new_start_ms(&self, rate: u32) -> i64 {
        samples_to_ms(self.new_start_sample, rate)
    }

    /// Session-time end of the window (ms, exclusive).
    pub fn end_ms(&self, rate: u32) -> i64 {
        samples_to_ms(self.end_sample, rate)
    }
}

/// The pure windowing + backlog state machine.
///
/// The runtime feeds it the incoming 16 kHz mono stream (`push`) and the
/// decoder's live state (`decoder_busy`), and receives the windows that are
/// ready to decode. The rule set is exactly the workorder's:
///
/// * a window closes every [`LiveConfig::step_ms`] of new audio and carries
///   [`LiveConfig::overlap_ms`] of context from the previous window's tail
///   (the very first window is clamped at the session start — no negative
///   ranges);
/// * if the previous window's decode is still running when a window closes,
///   the window is **skipped** (counted in [`LiveScheduler::skipped_windows`])
///   and its span **merges into the next window** — no audio is ever dropped
///   from the live transcript, only the cadence is degraded;
/// * a window's samples always cover exactly `[window_start − overlap,
///   boundary)`, so the next decode re-reads the context it needs.
#[derive(Debug)]
pub struct LiveScheduler {
    config: LiveConfig,
    /// Samples from `buffer_start` (a session index) onward — the pending
    /// window plus its overlap context.
    buffer: Vec<f32>,
    /// Session sample index of `buffer[0]`.
    buffer_start: usize,
    /// Total samples pushed (the session frontier).
    frontier: usize,
    /// Session index where the pending window's new content begins.
    window_start: usize,
    /// Session index at which the pending window closes.
    next_boundary: usize,
    skipped: u64,
    emitted: u64,
}

impl LiveScheduler {
    /// Creates a scheduler for a validated config.
    pub fn new(config: LiveConfig) -> Result<Self, String> {
        config.validate()?;
        let step = config.step_samples();
        Ok(Self {
            config,
            buffer: Vec::new(),
            buffer_start: 0,
            frontier: 0,
            window_start: 0,
            next_boundary: step,
            skipped: 0,
            emitted: 0,
        })
    }

    /// Feeds new audio; returns the windows that are ready to decode **now**.
    ///
    /// `decoder_busy` is the backlog guard's input: `true` while the previous
    /// window's decode is still running. Every boundary crossed while busy is
    /// skipped and merged into the window that closes after the decoder frees
    /// up (a single `push` may therefore return several windows when a long
    /// stall is followed by an idle decoder — the queue bound handles the
    /// rest).
    pub fn push(&mut self, samples: &[f32], decoder_busy: bool) -> Vec<LiveWindow> {
        self.buffer.extend_from_slice(samples);
        self.frontier += samples.len();

        let step = self.config.step_samples();
        let overlap = self.config.overlap_samples();
        let mut windows = Vec::new();
        while self.frontier >= self.next_boundary {
            if decoder_busy {
                // Backlog guard: the previous decode is still running — skip
                // this window (count it) and merge its span into the next one.
                self.skipped += 1;
                self.next_boundary += step;
                continue;
            }
            let window_start = self.window_start.saturating_sub(overlap);
            let lo = window_start - self.buffer_start;
            let hi = self.next_boundary - self.buffer_start;
            windows.push(LiveWindow {
                seq: self.emitted,
                start_sample: window_start,
                new_start_sample: self.window_start,
                end_sample: self.next_boundary,
                samples: self.buffer[lo..hi].to_vec(),
            });
            self.emitted += 1;
            self.window_start = self.next_boundary;
            self.next_boundary += step;
        }

        // Keep only what the next window needs: its overlap context onward.
        let keep_from = self.window_start.saturating_sub(overlap);
        if keep_from > self.buffer_start {
            let drop = keep_from - self.buffer_start;
            self.buffer.drain(..drop);
            self.buffer_start = keep_from;
        }
        windows
    }

    /// Windows skipped by the backlog guard this session.
    pub fn skipped_windows(&self) -> u64 {
        self.skipped
    }

    /// Windows emitted for decode this session.
    pub fn emitted_windows(&self) -> u64 {
        self.emitted
    }

    /// Audio pushed but not yet inside an emitted window (the tail that the
    /// final pass owns at Stop).
    pub fn pending_samples(&self) -> usize {
        self.frontier - self.window_start
    }

    /// The scheduler's config.
    pub fn config(&self) -> &LiveConfig {
        &self.config
    }
}

/// Bounded FIFO of windows waiting for the decoder — **drop-oldest**: when the
/// queue is full the oldest window is evicted (the newest audio is what the
/// provisional view needs most; a stale window would only add latency).
#[derive(Debug)]
pub struct LiveWindowQueue {
    capacity: usize,
    queue: VecDeque<LiveWindow>,
}

impl LiveWindowQueue {
    /// Creates a queue with at least capacity 1.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            queue: VecDeque::new(),
        }
    }

    /// Pushes a window; returns the evicted oldest window when the queue was
    /// full.
    pub fn push(&mut self, window: LiveWindow) -> Option<LiveWindow> {
        let evicted = if self.queue.len() >= self.capacity {
            self.queue.pop_front()
        } else {
            None
        };
        self.queue.push_back(window);
        evicted
    }

    /// Takes the oldest window.
    pub fn pop(&mut self) -> Option<LiveWindow> {
        self.queue.pop_front()
    }

    /// Number of queued windows.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// The bound.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// The growing provisional transcript: session-timeline segments, appended in
/// order, deduplicated across the window overlaps.
#[derive(Debug, Default, Clone)]
pub struct ProvisionalTranscript {
    segments: Vec<Segment>,
    covered_until_ms: i64,
    revision: u64,
}

impl ProvisionalTranscript {
    /// An empty transcript.
    pub fn new() -> Self {
        Self::default()
    }

    /// Merges one decoded window into the list and returns how many segments
    /// were added.
    ///
    /// The merge rule (documented, provisional-only):
    /// * decoded timestamps are window-relative; they are translated to the
    ///   session timeline using the window's `start_sample`;
    /// * a segment whose end lies within [`MERGE_TOLERANCE_MS`] of the covered
    ///   frontier is the overlap re-decode of already-covered audio — dropped;
    /// * empty-text and degenerate segments are dropped;
    /// * everything else is appended with a fresh `idx` (the list is
    ///   append-only, so the provisional view never reshuffles under the user).
    pub fn ingest(&mut self, window: &LiveWindow, decoded: &[Segment], rate: u32) -> usize {
        let window_start_ms = window.start_ms(rate);
        let mut added = 0;
        for segment in decoded {
            let text = segment.text.trim();
            if text.is_empty() {
                continue;
            }
            let start_ms = window_start_ms + segment.start_ms;
            let end_ms = window_start_ms + segment.end_ms;
            if end_ms <= start_ms {
                continue;
            }
            if end_ms <= self.covered_until_ms + MERGE_TOLERANCE_MS {
                continue;
            }
            self.segments.push(Segment {
                idx: self.segments.len() as u32,
                start_ms,
                end_ms,
                text: text.to_string(),
            });
            added += 1;
        }
        self.covered_until_ms = self.covered_until_ms.max(window.end_ms(rate));
        self.revision += 1;
        added
    }

    /// The provisional segments so far (session timeline, append-only order).
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Session ms covered by ingested windows (the frontier the merge uses).
    pub fn covered_until_ms(&self) -> i64 {
        self.covered_until_ms
    }

    /// Bumped on every ingest (the runtime publishes only on change).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The provisional words, joined (tests and the e2e report use this).
    pub fn text(&self) -> String {
        self.segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The pure live session the runtime drives: scheduler + provisional transcript.
#[derive(Debug)]
pub struct LiveSession {
    config: LiveConfig,
    scheduler: LiveScheduler,
    provisional: ProvisionalTranscript,
}

impl LiveSession {
    /// Creates a session for a validated config.
    pub fn new(config: LiveConfig) -> Result<Self, String> {
        let scheduler = LiveScheduler::new(config.clone())?;
        Ok(Self {
            config,
            scheduler,
            provisional: ProvisionalTranscript::new(),
        })
    }

    /// Feeds new 16 kHz mono audio; returns the windows to decode now.
    pub fn push_audio(&mut self, samples: &[f32], decoder_busy: bool) -> Vec<LiveWindow> {
        self.scheduler.push(samples, decoder_busy)
    }

    /// Merges one decoded window's segments; returns how many were added.
    pub fn ingest_decoded(&mut self, window: &LiveWindow, decoded: &[Segment]) -> usize {
        self.provisional
            .ingest(window, decoded, self.config.sample_rate)
    }

    /// The provisional segments so far.
    pub fn segments(&self) -> &[Segment] {
        self.provisional.segments()
    }

    /// The provisional words, joined.
    pub fn text(&self) -> String {
        self.provisional.text()
    }

    /// Bumped whenever the provisional content changes.
    pub fn revision(&self) -> u64 {
        self.provisional.revision()
    }

    /// Windows skipped by the backlog guard this session.
    pub fn skipped_windows(&self) -> u64 {
        self.scheduler.skipped_windows()
    }

    /// Windows emitted for decode this session.
    pub fn emitted_windows(&self) -> u64 {
        self.scheduler.emitted_windows()
    }

    /// The session's config.
    pub fn config(&self) -> &LiveConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// The decode thread (impure: whisper + threads; no capture feature needed)
// ---------------------------------------------------------------------------

/// What the live decoder needs to run the **same** whisper path as the final
/// session: the model, language, initial prompt, engine plan and thread count
/// the job flow would use.
#[derive(Debug, Clone)]
pub struct LiveDecodeSettings {
    pub model_path: PathBuf,
    pub lang: LangMode,
    pub initial_prompt: Option<String>,
    pub plan: EngineDecision,
    pub n_threads: i32,
}

/// One finished live decode.
#[derive(Debug)]
pub struct LiveResult {
    /// The window that was decoded.
    pub window: LiveWindow,
    /// The decoded segments (window-relative ms) — empty on error.
    pub segments: Vec<Segment>,
    /// `Some` when the decode failed (missing model, inference failure, …).
    pub error: Option<String>,
}

#[derive(Debug, Default)]
struct DecoderShared {
    queue: VecDeque<LiveWindow>,
    running: bool,
    shutdown: bool,
    results: Vec<LiveResult>,
    dropped: u64,
}

/// The live decode thread: windows in, [`LiveResult`]s out.
///
/// One window is decoded at a time on a dedicated thread (`!Send` state stays
/// inside); the queue is bounded with drop-oldest ([`LiveWindowQueue`]'s rule).
/// The whisper context is loaded lazily on the first window and kept for the
/// session — the same [`ContextCache`] + [`whisper::transcribe`] path the
/// worker uses. Dropping the decoder signals shutdown and **detaches**: an
/// in-flight decode finishes and the thread exits on its own, so Stop never
/// blocks the UI thread (the polish/download-thread rule).
pub struct LiveDecoder {
    shared: Arc<(Mutex<DecoderShared>, Condvar)>,
    handle: Option<JoinHandle<()>>,
    capacity: usize,
}

impl LiveDecoder {
    /// Spawns the decode thread.
    ///
    /// Panics if the OS refuses to spawn a thread (the app would be unusable).
    pub fn spawn(settings: LiveDecodeSettings, max_queued: usize) -> LiveDecoder {
        let shared = Arc::new((Mutex::new(DecoderShared::default()), Condvar::new()));
        let thread_shared = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name(LIVE_DECODE_THREAD_NAME.to_string())
            .spawn(move || run_decoder(thread_shared, settings))
            .expect("failed to spawn the suramak-live-decode thread");
        LiveDecoder {
            shared,
            handle: Some(handle),
            capacity: max_queued.max(1),
        }
    }

    /// Queues a window for decode. Returns the evicted oldest window when the
    /// queue bound forced a drop (the caller logs/counts it).
    pub fn submit(&self, window: LiveWindow) -> Option<LiveWindow> {
        let (lock, cvar) = &*self.shared;
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.shutdown {
            return Some(window);
        }
        let evicted = if state.queue.len() >= self.capacity {
            state.queue.pop_front()
        } else {
            None
        };
        if evicted.is_some() {
            state.dropped += 1;
        }
        state.queue.push_back(window);
        drop(state);
        cvar.notify_one();
        evicted
    }

    /// Whether a decode is running right now (the backlog guard's input).
    pub fn is_decoding(&self) -> bool {
        let (lock, _) = &*self.shared;
        lock.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .running
    }

    /// Takes every finished decode since the last call.
    pub fn take_results(&self) -> Vec<LiveResult> {
        let (lock, _) = &*self.shared;
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut state.results)
    }

    /// Windows the queue bound dropped this session.
    pub fn dropped_windows(&self) -> u64 {
        let (lock, _) = &*self.shared;
        lock.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .dropped
    }

    /// Windows queued but not started.
    pub fn queued_windows(&self) -> usize {
        let (lock, _) = &*self.shared;
        lock.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .queue
            .len()
    }

    /// Signals shutdown and waits at most `grace` for the decode thread — the
    /// quit path's teardown (WORKORDER-HOTFIX-QUIT).
    ///
    /// The existing teardown (see [`Drop`]) signals and **detaches**: an
    /// in-flight decode finishes on its own, so Stop never blocks the UI
    /// thread. This bounded wait exists so the quit path can let an *idle*
    /// thread exit right away — which drops its whisper context, and with it
    /// the Metal residency sets whose survival aborts the process at exit on
    /// macOS (ggml's `[rsets->data count] == 0` assert). Returns whether the
    /// thread exited within `grace`; on timeout it is left detached exactly
    /// like [`Drop`] leaves it. Idempotent.
    pub fn shutdown_and_wait(&mut self, grace: std::time::Duration) -> bool {
        {
            let (lock, cvar) = &*self.shared;
            if let Ok(mut state) = lock.lock() {
                state.shutdown = true;
            }
            cvar.notify_all();
        }
        let deadline = std::time::Instant::now() + grace;
        let mut finished = self.handle.as_ref().is_none_or(|h| h.is_finished());
        while !finished && std::time::Instant::now() < deadline {
            std::thread::sleep(SHUTDOWN_POLL);
            finished = self.handle.as_ref().is_none_or(|h| h.is_finished());
        }
        if finished {
            // Reap it: the join is instant (the thread already returned) and it
            // proves the decode thread — and its context cache — is gone.
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
        finished
    }
}

impl Drop for LiveDecoder {
    fn drop(&mut self) {
        {
            let (lock, cvar) = &*self.shared;
            if let Ok(mut state) = lock.lock() {
                state.shutdown = true;
            }
            cvar.notify_all();
        }
        // Detach: an in-flight decode finishes and the thread exits on its own.
        self.handle.take();
    }
}

fn run_decoder(shared: Arc<(Mutex<DecoderShared>, Condvar)>, settings: LiveDecodeSettings) {
    let mut cache = ContextCache::new();
    loop {
        let window = {
            let (lock, cvar) = &*shared;
            let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            loop {
                if state.shutdown {
                    return;
                }
                if let Some(window) = state.queue.pop_front() {
                    state.running = true;
                    break window;
                }
                state = cvar
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        let result = decode_window(&mut cache, &settings, &window);
        let (lock, _) = &*shared;
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.running = false;
        state.results.push(result);
    }
}

/// Runs one window through the existing whisper path.
fn decode_window(
    cache: &mut ContextCache,
    settings: &LiveDecodeSettings,
    window: &LiveWindow,
) -> LiveResult {
    let run = (|| -> Result<Vec<Segment>, AsrError> {
        let ctx = cache.get_or_load(&settings.model_path, settings.plan.engine.use_gpu())?;
        let outcome = whisper::transcribe(
            ctx,
            &settings.model_path,
            &window.samples,
            &settings.lang,
            settings.initial_prompt.as_deref(),
            settings.n_threads,
            &settings.plan,
            // M9e (P2-2): the live path takes the signature's token but never
            // sets it — a fresh, uncancelled token per window. The live
            // machinery is torn down by its existing shutdown path, not by the
            // job-cancel mechanism (the audio path stays untouched).
            &CancelToken::new(),
            |_| {},
        )?;
        Ok(outcome.segments)
    })();
    match run {
        Ok(segments) => LiveResult {
            window: window.clone(),
            segments,
            error: None,
        },
        Err(err) => LiveResult {
            window: window.clone(),
            segments: Vec::new(),
            error: Some(err.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineId;

    fn window(seq: u64, start: usize, new_start: usize, end: usize) -> LiveWindow {
        LiveWindow {
            seq,
            start_sample: start,
            new_start_sample: new_start,
            end_sample: end,
            samples: Vec::new(),
        }
    }

    fn segment(start_ms: i64, end_ms: i64, text: &str) -> Segment {
        Segment {
            idx: 0,
            start_ms,
            end_ms,
            text: text.to_string(),
        }
    }

    /// 100 ms of audio at the live stream's rate.
    fn tick() -> Vec<f32> {
        vec![0.1; 1_600]
    }

    // ---- cadence + overlap --------------------------------------------------

    #[test]
    fn windows_close_on_the_cadence_and_carry_the_overlap_context() {
        let config = LiveConfig::default();
        let mut scheduler = LiveScheduler::new(config.clone()).expect("valid config");

        // 3.9 s of audio: not a single window yet (the cadence is 4 s).
        for _ in 0..39 {
            assert!(
                scheduler.push(&tick(), false).is_empty(),
                "no window before the first boundary"
            );
        }

        // 4 s: the first window — clamped at the session start, no overlap.
        let first = scheduler.push(&tick(), false);
        assert_eq!(first.len(), 1);
        let first = &first[0];
        assert_eq!(
            (
                first.seq,
                first.start_sample,
                first.new_start_sample,
                first.end_sample
            ),
            (0, 0, 0, 64_000)
        );
        assert_eq!(first.samples.len(), 64_000, "4 s at 16 kHz");

        // 8 s: the second window carries 1.5 s of the first window's tail.
        let mut second = Vec::new();
        for _ in 0..40 {
            second.extend(scheduler.push(&tick(), false));
        }
        assert_eq!(second.len(), 1);
        let second = &second[0];
        assert_eq!(
            (
                second.seq,
                second.start_sample,
                second.new_start_sample,
                second.end_sample
            ),
            (1, 40_000, 64_000, 128_000)
        );
        assert_eq!(second.samples.len(), 88_000);
        assert_eq!(
            second.samples.len() - (second.end_sample - second.new_start_sample),
            config.overlap_samples(),
            "the head is exactly the overlap"
        );
        assert_eq!(
            &second.samples[..24_000],
            &first.samples[40_000..],
            "the overlap head is the previous window's tail — the context whisper re-reads"
        );

        assert_eq!(scheduler.emitted_windows(), 2);
        assert_eq!(scheduler.skipped_windows(), 0);

        // 8–9 s: the tail stays pending (the final pass owns it at Stop).
        for _ in 0..10 {
            assert!(scheduler.push(&tick(), false).is_empty());
        }
        assert_eq!(
            scheduler.pending_samples(),
            16_000,
            "9 s pushed, 8 s windowed — the last second is the final pass's"
        );
    }

    // ---- backlog guard: skip and merge -------------------------------------

    #[test]
    fn a_busy_decoder_skips_the_window_and_merges_it_into_the_next() {
        let mut scheduler = LiveScheduler::new(LiveConfig::default()).expect("valid config");

        // 0–4 s: the first window closes while the decoder is busy → skipped.
        let mut emitted = Vec::new();
        for _ in 0..40 {
            emitted.extend(scheduler.push(&tick(), true));
        }
        assert!(emitted.is_empty(), "a busy decoder gets no window");
        assert_eq!(scheduler.skipped_windows(), 1);

        // 4–8 s: still busy → skipped again (both spans now wait).
        for _ in 0..40 {
            emitted.extend(scheduler.push(&tick(), true));
        }
        assert_eq!(scheduler.skipped_windows(), 2);
        assert!(emitted.is_empty());

        // 8–12 s: the decoder is free — one window covers everything since the
        // last decode (the skipped spans merged, no audio lost).
        for _ in 0..40 {
            emitted.extend(scheduler.push(&tick(), false));
        }
        assert_eq!(emitted.len(), 1);
        let merged = &emitted[0];
        assert_eq!(
            (
                merged.seq,
                merged.start_sample,
                merged.new_start_sample,
                merged.end_sample
            ),
            (0, 0, 0, 192_000)
        );
        assert_eq!(merged.samples.len(), 192_000);

        // Steady state resumes: the next window keeps the cadence + overlap.
        for _ in 0..40 {
            emitted.extend(scheduler.push(&tick(), false));
        }
        assert_eq!(emitted.len(), 2);
        let next = &emitted[1];
        assert_eq!(
            (next.start_sample, next.new_start_sample, next.end_sample),
            (168_000, 192_000, 256_000)
        );
        assert_eq!(scheduler.emitted_windows(), 2);
        assert_eq!(scheduler.skipped_windows(), 2);
    }

    // ---- bounded queue: drop-oldest ----------------------------------------

    #[test]
    fn the_window_queue_is_bounded_and_drops_the_oldest() {
        let mut queue = LiveWindowQueue::new(2);
        assert_eq!(queue.capacity(), 2);
        assert!(queue.is_empty());
        assert!(queue.push(window(0, 0, 0, 1)).is_none());
        assert!(queue.push(window(1, 0, 0, 1)).is_none());
        let evicted = queue
            .push(window(2, 0, 0, 1))
            .expect("the oldest is evicted");
        assert_eq!(evicted.seq, 0);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.pop().expect("oldest first").seq, 1);
        assert_eq!(queue.pop().expect("then the newest").seq, 2);
        assert!(queue.pop().is_none());

        // Capacity 0 clamps to 1: only the newest window survives.
        let mut one = LiveWindowQueue::new(0);
        assert_eq!(one.capacity(), 1);
        assert!(one.push(window(7, 0, 0, 1)).is_none());
        assert_eq!(one.push(window(8, 0, 0, 1)).expect("evicted").seq, 7);
        assert_eq!(one.pop().expect("the newest stays").seq, 8);
    }

    // ---- session bounds -----------------------------------------------------

    #[test]
    fn the_config_bounds_are_enforced() {
        let config = LiveConfig::default();
        assert!(
            config.validate().is_ok(),
            "the default sits inside the bounds"
        );
        assert!(STEP_MS_RANGE.contains(&config.step_ms));
        assert!(OVERLAP_MS_RANGE.contains(&config.overlap_ms));
        assert_eq!(config.step_samples(), 64_000);
        assert_eq!(config.overlap_samples(), 24_000);

        for bad in [
            LiveConfig {
                step_ms: 3_999,
                ..config.clone()
            },
            LiveConfig {
                step_ms: 6_001,
                ..config.clone()
            },
            LiveConfig {
                overlap_ms: 999,
                ..config.clone()
            },
            LiveConfig {
                overlap_ms: 2_001,
                ..config.clone()
            },
            LiveConfig {
                max_queued: 0,
                ..config.clone()
            },
            LiveConfig {
                max_queued: 9,
                ..config.clone()
            },
            LiveConfig {
                sample_rate: 44_100,
                ..config.clone()
            },
        ] {
            assert!(bad.validate().is_err(), "must be rejected: {bad:?}");
            assert!(
                LiveScheduler::new(bad).is_err(),
                "the scheduler refuses invalid configs"
            );
        }

        // Pushing nothing changes nothing; a sub-window push stays pending.
        let mut scheduler = LiveScheduler::new(config).expect("valid");
        assert!(scheduler.push(&[], false).is_empty());
        assert!(scheduler.push(&[0.0; 100], false).is_empty());
        assert_eq!(scheduler.pending_samples(), 100);
        assert_eq!(scheduler.emitted_windows(), 0);
    }

    // ---- provisional merging ------------------------------------------------

    #[test]
    fn provisional_segments_merge_across_the_overlap_without_duplicates() {
        let mut transcript = ProvisionalTranscript::new();

        // Window 0: 0–4 s (the session's first, no overlap head).
        let w0 = window(0, 0, 0, 64_000);
        let decoded0 = [
            segment(0, 1_900, "The quick brown fox"),
            segment(2_000, 3_950, "jumps over the lazy dog"),
        ];
        assert_eq!(transcript.ingest(&w0, &decoded0, 16_000), 2);
        assert_eq!(transcript.covered_until_ms(), 4_000);

        // Window 1: 2.5–8 s. Its head re-decodes window 0's tail (the overlap)
        // and must not duplicate it.
        let w1 = window(1, 40_000, 64_000, 128_000);
        let decoded1 = [
            segment(0, 1_400, "jumps over the lazy"), // session 2 500–3 900 → covered → dropped
            segment(1_500, 2_600, "dog. Suramak keeps"), // session 4 000–5 100 → new
            segment(2_700, 3_500, "every recording"), // session 5 200–6 000 → new
        ];
        assert_eq!(transcript.ingest(&w1, &decoded1, 16_000), 2);

        let segments = transcript.segments();
        assert_eq!(segments.len(), 4);
        assert_eq!(
            segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
            [
                "The quick brown fox",
                "jumps over the lazy dog",
                "dog. Suramak keeps",
                "every recording"
            ]
        );
        // Session timeline + append-only idx.
        assert_eq!(
            segments
                .iter()
                .map(|s| (s.idx, s.start_ms, s.end_ms))
                .collect::<Vec<_>>(),
            [
                (0, 0, 1_900),
                (1, 2_000, 3_950),
                (2, 4_000, 5_100),
                (3, 5_200, 6_000)
            ]
        );
        assert_eq!(transcript.covered_until_ms(), 8_000);
        assert_eq!(transcript.revision(), 2);

        // Window 2: 6.5–10 s — everything it re-decodes from ≤ 8 s is dropped.
        let w2 = window(2, 104_000, 128_000, 160_000);
        let decoded2 = [
            segment(0, 1_400, "every recording"), // session 6 500–7 900 → covered → dropped
            segment(1_600, 2_400, "on this machine"), // session 8 100–8 900 → new
        ];
        assert_eq!(transcript.ingest(&w2, &decoded2, 16_000), 1);
        assert_eq!(transcript.revision(), 3);
        assert!(transcript.text().ends_with("on this machine"));
        assert_eq!(transcript.segments()[4].idx, 4);

        // Empty-text segments never pollute the list (whisper emits them on
        // silence); the revision still moves (the window was ingested).
        let w3 = window(3, 136_000, 160_000, 192_000);
        assert_eq!(transcript.ingest(&w3, &[segment(0, 100, "   ")], 16_000), 0);
        assert_eq!(transcript.segments().len(), 5);
        assert_eq!(transcript.revision(), 4);
    }

    #[test]
    fn the_merge_tolerance_is_fifty_milliseconds() {
        let mut transcript = ProvisionalTranscript::new();
        transcript.ingest(&window(0, 0, 0, 64_000), &[], 16_000); // covers 0–4 s
        assert_eq!(transcript.covered_until_ms(), 4_000);

        // Window 1: 2.5–5 s. A segment ending exactly at 4 050 (covered + 50)
        // is a boundary artifact → dropped; one ending at 4 060 is new.
        let w1 = window(1, 40_000, 64_000, 80_000);
        let decoded = [
            segment(0, 1_550, "boundary artifact"), // session 2 500–4 050
            segment(0, 1_560, "real new words"),    // session 2 500–4 060
        ];
        assert_eq!(transcript.ingest(&w1, &decoded, 16_000), 1);
        assert_eq!(transcript.segments().len(), 1);
        assert_eq!(transcript.segments()[0].text, "real new words");
        assert_eq!(transcript.segments()[0].start_ms, 2_500);
    }

    // ---- the session glue ---------------------------------------------------

    #[test]
    fn a_live_session_grows_and_keeps_the_skipped_count() {
        let mut session = LiveSession::new(LiveConfig::default()).expect("valid config");

        // 9 s at 100 ms ticks; the decoder is busy until 4.5 s, so the 4 s
        // boundary is skipped and the 8 s boundary emits one merged window.
        let mut windows = Vec::new();
        for tick_index in 0..90 {
            windows.extend(session.push_audio(&tick(), tick_index < 45));
        }
        assert_eq!(session.skipped_windows(), 1);
        assert_eq!(session.emitted_windows(), 1);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].new_start_sample, 0);
        assert_eq!(windows[0].end_sample, 128_000);

        // A (fake) decode answer grows the provisional transcript.
        let decoded = [segment(0, 3_900, "hello from the live pass")];
        assert_eq!(session.ingest_decoded(&windows[0], &decoded), 1);
        assert_eq!(session.revision(), 1);
        assert_eq!(session.text(), "hello from the live pass");
        assert_eq!(session.segments()[0].idx, 0);
        assert_eq!(session.segments()[0].end_ms, 3_900);
        assert_eq!(session.config().sample_rate, TARGET_RATE);
    }

    // ---- the decode thread --------------------------------------------------

    #[test]
    fn the_live_decoder_reports_a_missing_model_through_its_results() {
        let settings = LiveDecodeSettings {
            model_path: PathBuf::from("/nonexistent/suramak/ggml-tiny.bin"),
            lang: LangMode::Fixed("en".to_string()),
            initial_prompt: None,
            plan: EngineDecision {
                engine: EngineId::Cpu,
                warning: None,
            },
            n_threads: 2,
        };
        let decoder = LiveDecoder::spawn(settings, 2);
        assert!(!decoder.is_decoding());
        assert!(
            decoder.submit(window(0, 0, 0, 16_000)).is_none(),
            "an empty queue never evicts"
        );

        // The thread answers with the load failure (bounded wait).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut results = Vec::new();
        while std::time::Instant::now() < deadline && results.is_empty() {
            results = decoder.take_results();
            if results.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        assert_eq!(results.len(), 1, "exactly one result");
        let result = &results[0];
        assert_eq!(result.window.seq, 0);
        assert!(result.segments.is_empty());
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("model file not found"),
            "unexpected error: {:?}",
            result.error
        );
        assert!(!decoder.is_decoding(), "the thread is idle again");
        assert_eq!(decoder.queued_windows(), 0);
        assert_eq!(decoder.dropped_windows(), 0);
        drop(decoder); // signals shutdown; the thread exits on its own
    }

    /// The quit path's decode-thread teardown (WORKORDER-HOTFIX-QUIT): an idle
    /// thread exits within the grace, the wait is idempotent, and a stopped
    /// decoder takes no further work ("teardown is a no-op when nothing runs").
    #[test]
    fn the_live_decoder_shutdown_is_quick_idempotent_and_final() {
        let settings = LiveDecodeSettings {
            model_path: PathBuf::from("/nonexistent/suramak/ggml-tiny.bin"),
            lang: LangMode::Fixed("en".to_string()),
            initial_prompt: None,
            plan: EngineDecision {
                engine: EngineId::Cpu,
                warning: None,
            },
            n_threads: 2,
        };
        let grace = std::time::Duration::from_millis(250);
        let mut decoder = LiveDecoder::spawn(settings, 2);
        assert!(
            decoder.shutdown_and_wait(grace),
            "an idle decode thread must exit within the grace"
        );
        assert!(
            decoder.shutdown_and_wait(grace),
            "a second shutdown is a no-op that still reports the exit"
        );
        // Shut down means shut down: further windows are refused outright
        // (reported through `submit`'s drop answer, never decoded).
        assert!(decoder.submit(window(0, 0, 0, 16_000)).is_some());
        assert!(decoder.take_results().is_empty());
        drop(decoder);
    }
}
