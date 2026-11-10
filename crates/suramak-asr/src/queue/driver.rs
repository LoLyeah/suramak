//! The production pump for [`JobQueue`]: one inference backend, one bounded
//! decode-ahead pool, and the queue's per-item events.
//!
//! [`QueueDriver`] is deliberately synchronous: [`QueueDriver::run_once`] moves
//! the machine as far as it can without blocking (drain decode results, drain
//! backend events, fill the single inference slot, request up to two decodes).
//! The CLI spins it with a short sleep; a future capture slice can call the same
//! method from its frame loop. The actual blocking work lives on two threads —
//! the ASR worker and the prefetch thread — never on the caller's.
//!
//! The driver is generic over [`InferenceBackend`] and [`Prefetcher`] so the
//! concurrency rules are pinned by deterministic tests: a test backend counts
//! active runs (asserting the 1-slot invariant) and a manual prefetcher hands
//! out decode results on command (asserting the decode-ahead bound and the
//! mid-decode cancellation path) without a model or a wall clock.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use suramak_core::decode;
use suramak_core::resample;
use suramak_core::settings::{EngineSetting, LangMode};
use suramak_core::transcript::{Transcript, WordTiming};
use suramak_core::SuramakError;

use crate::cancel::CancelToken;
use crate::engine::EngineId;
use crate::worker::{Job, JobStage, Worker, WorkerEvent};

use super::{CancelDecision, DecodeAdmission, JobQueue, QueueCounts};

/// The per-job options the CLI resolved once for the whole batch.
#[derive(Debug, Clone)]
pub struct JobOptions {
    pub model_path: PathBuf,
    pub lang: LangMode,
    pub initial_prompt: Option<String>,
    pub engine: EngineSetting,
    pub fast_engine: bool,
}

/// What the driver needs from a transcription backend.
pub trait InferenceBackend {
    /// Submits one job. Called only when no other job is active.
    fn submit(&mut self, job: Job) -> Result<(), String>;
    /// Non-blocking poll for the next event of the active job.
    fn try_recv(&mut self) -> Option<WorkerEvent>;
}

impl InferenceBackend for Worker {
    fn submit(&mut self, job: Job) -> Result<(), String> {
        Worker::submit(self, job).map_err(|err| err.to_string())
    }

    fn try_recv(&mut self) -> Option<WorkerEvent> {
        Worker::try_recv(self)
    }
}

/// A decode failure, kept distinct from a user cancellation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeFailure {
    Cancelled,
    Failed(String),
}

/// The decode-ahead supplier.
pub trait AudioDecoder: Send + 'static {
    fn decode(&self, input: &Path, cancel: &CancelToken) -> Result<Vec<f32>, DecodeFailure>;
}

/// The production decoder: core's file decode + 16 kHz mono resample, with the
/// same cooperative cancellation the pipeline uses (checked per demuxed packet,
/// so a cancel mid-file drops the partial buffer).
#[derive(Debug, Default, Clone, Copy)]
pub struct FileDecoder;

impl AudioDecoder for FileDecoder {
    fn decode(&self, input: &Path, cancel: &CancelToken) -> Result<Vec<f32>, DecodeFailure> {
        let buffer =
            decode::decode_file_cancellable(input, &|| cancel.is_cancelled()).map_err(|err| {
                match err {
                    SuramakError::Cancelled => DecodeFailure::Cancelled,
                    other => DecodeFailure::Failed(other.to_string()),
                }
            })?;
        if cancel.is_cancelled() {
            return Err(DecodeFailure::Cancelled);
        }
        resample::to_16khz_mono(&buffer).map_err(|err| DecodeFailure::Failed(err.to_string()))
    }
}

/// One decode-ahead request.
#[derive(Debug, Clone)]
pub struct DecodeTask {
    pub id: u64,
    pub input: PathBuf,
    pub cancel: CancelToken,
}

/// One completed decode-ahead request.
#[derive(Debug)]
pub struct DecodeDone {
    pub id: u64,
    pub result: Result<Vec<f32>, DecodeFailure>,
}

/// The decode-ahead pool, abstracted so tests control its timing.
pub trait Prefetcher: Send {
    fn request(&mut self, task: DecodeTask);
    fn try_recv(&mut self) -> Option<DecodeDone>;
}

/// The production prefetcher: one named thread, FIFO, no unbounded backlog (the
/// queue never asks for more than [`super::MAX_DECODE_AHEAD`] at a time).
pub struct ThreadedPrefetch {
    tasks: Option<Sender<DecodeTask>>,
    done: Receiver<DecodeDone>,
    handle: Option<JoinHandle<()>>,
}

impl ThreadedPrefetch {
    pub fn new(decoder: Box<dyn AudioDecoder>) -> Self {
        let (task_tx, task_rx) = mpsc::channel::<DecodeTask>();
        let (done_tx, done_rx) = mpsc::channel::<DecodeDone>();
        let handle = thread::Builder::new()
            .name("suramak-decode-ahead".to_string())
            .spawn(move || {
                while let Ok(task) = task_rx.recv() {
                    let result = decoder.decode(&task.input, &task.cancel);
                    if done_tx
                        .send(DecodeDone {
                            id: task.id,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("failed to spawn the suramak-decode-ahead thread");
        Self {
            tasks: Some(task_tx),
            done: done_rx,
            handle: Some(handle),
        }
    }
}

impl Prefetcher for ThreadedPrefetch {
    fn request(&mut self, task: DecodeTask) {
        if let Some(tasks) = &self.tasks {
            let _ = tasks.send(task);
        }
    }

    fn try_recv(&mut self) -> Option<DecodeDone> {
        self.done.try_recv().ok()
    }
}

impl Drop for ThreadedPrefetch {
    fn drop(&mut self) {
        // Closing the task channel ends the thread once its current decode
        // returns; the join is deliberately skipped (a quit must never wait on
        // a long decode — the thread owns no engine context and dies with the
        // process).
        self.tasks.take();
        self.handle.take();
    }
}

/// Per-item events for the CLI's lines (and any future surface).
#[derive(Debug)]
pub enum QueueEvent {
    Started {
        id: u64,
        seq: usize,
        input: PathBuf,
        prefetched: bool,
    },
    Progress {
        id: u64,
        seq: usize,
        pct: u8,
    },
    Stage {
        id: u64,
        seq: usize,
        stage: JobStage,
    },
    Finished {
        id: u64,
        seq: usize,
        input: PathBuf,
        transcript: Box<Transcript>,
        words: Vec<WordTiming>,
        engine: EngineId,
        warnings: Vec<String>,
    },
    Failed {
        id: u64,
        seq: usize,
        input: PathBuf,
        reason: String,
    },
    Cancelled {
        id: u64,
        seq: usize,
        input: PathBuf,
    },
}

impl QueueEvent {
    /// The stable 1-based position this event belongs to.
    pub fn seq(&self) -> usize {
        match self {
            QueueEvent::Started { seq, .. }
            | QueueEvent::Progress { seq, .. }
            | QueueEvent::Stage { seq, .. }
            | QueueEvent::Finished { seq, .. }
            | QueueEvent::Failed { seq, .. }
            | QueueEvent::Cancelled { seq, .. } => *seq,
        }
    }
}

/// The active job's handles kept beside its cancel token.
struct Running {
    id: u64,
    seq: usize,
    input: PathBuf,
}

/// One inference slot, bounded decode-ahead, deterministic pump.
pub struct QueueDriver<B: InferenceBackend, P: Prefetcher> {
    queue: JobQueue,
    backend: B,
    prefetch: P,
    options: JobOptions,
    tokens: HashMap<u64, CancelToken>,
    samples: HashMap<u64, Vec<f32>>,
    current: Option<Running>,
    events: Vec<QueueEvent>,
    /// Tasks handed to the prefetch pool whose completion has not arrived yet
    /// (M13F found the hole: the queue's row may have been cancelled — removed
    /// from the waiting count — while the pool still reads the file; `is_idle`
    /// would otherwise go true too early and the CLI batch loop could exit
    /// mid-batch). Incremented in `start_decode`, decremented on every
    /// `DecodeDone` arrival.
    prefetch_in_flight: usize,
}

impl<B: InferenceBackend, P: Prefetcher> QueueDriver<B, P> {
    pub fn new(backend: B, prefetch: P, options: JobOptions) -> Self {
        Self {
            queue: JobQueue::new(),
            backend,
            prefetch,
            options,
            tokens: HashMap::new(),
            samples: HashMap::new(),
            current: None,
            events: Vec::new(),
            prefetch_in_flight: 0,
        }
    }

    /// Accepts a file at the end of the batch.
    pub fn enqueue(&mut self, input: PathBuf) -> u64 {
        let id = self.queue.enqueue(input);
        self.tokens.insert(id, CancelToken::new());
        id
    }

    pub fn enqueue_all(&mut self, inputs: impl IntoIterator<Item = PathBuf>) -> Vec<u64> {
        inputs
            .into_iter()
            .map(|input| self.enqueue(input))
            .collect()
    }

    pub fn queue(&self) -> &JobQueue {
        &self.queue
    }

    /// Updates the queue's owned snapshot of the counts at the time of an event.
    pub fn counts(&self) -> QueueCounts {
        self.queue.counts()
    }

    /// Whether any work — running, waiting, decoded-ahead, or an in-flight
    /// prefetch decode — remains (the CLI batch loop's exit condition).
    pub fn is_idle(&self) -> bool {
        self.queue.is_idle() && self.current.is_none() && self.prefetch_in_flight == 0
    }

    /// Takes the events produced since the last drain.
    pub fn drain_events(&mut self) -> Vec<QueueEvent> {
        std::mem::take(&mut self.events)
    }

    /// Cancels the item at a stable 1-based enqueue position (`--cancel`).
    pub fn cancel_seq(&mut self, seq: usize) -> CancelDecision {
        let Some(id) = self
            .queue
            .items()
            .iter()
            .find(|item| item.seq == seq)
            .map(|item| item.id)
        else {
            return CancelDecision::Inert;
        };
        self.cancel(id)
    }

    /// Cancels one item: a waiting item is removed; the running item's
    /// cooperative token is set and the worker answers at its next stage
    /// boundary. The slot stays occupied until that answer (never a second
    /// inference on top of a half-stopped one).
    pub fn cancel(&mut self, id: u64) -> CancelDecision {
        let before = self.queue.item(id).map(|item| item.seq);
        let decision = self.queue.cancel(id);
        match decision {
            CancelDecision::Removed => {
                if let Some(token) = self.tokens.get(&id) {
                    token.cancel();
                }
                self.samples.remove(&id);
                if let Some(seq) = before {
                    let input = self
                        .queue
                        .item(id)
                        .map(|item| item.input.clone())
                        .unwrap_or_default();
                    self.events.push(QueueEvent::Cancelled { id, seq, input });
                }
            }
            CancelDecision::Running => {
                if let Some(token) = self.tokens.get(&id) {
                    token.cancel();
                }
            }
            _ => {}
        }
        decision
    }

    /// Moves the machine as far as it can without blocking. Returns whether any
    /// state changed.
    pub fn run_once(&mut self) -> bool {
        let mut progressed = false;
        loop {
            let mut step = false;
            while let Some(done) = self.prefetch.try_recv() {
                step = true;
                self.on_decoded(done);
            }
            while let Some(event) = self.backend.try_recv() {
                step = true;
                self.on_backend(event);
            }
            if self.start_inference() {
                step = true;
            }
            if self.start_decode() {
                step = true;
            }
            if !step {
                break;
            }
            progressed = true;
        }
        progressed
    }

    fn on_decoded(&mut self, done: DecodeDone) {
        // The completion arrived, whatever happens to the item next — the
        // in-flight counter is the drain discipline of `is_idle`.
        self.prefetch_in_flight = self.prefetch_in_flight.saturating_sub(1);
        match done.result {
            Ok(samples) => {
                let bytes = samples.len() * std::mem::size_of::<f32>();
                match self.queue.mark_decoded(done.id, bytes) {
                    DecodeAdmission::Kept => {
                        self.samples.insert(done.id, samples);
                    }
                    // Over budget: the item decodes inline. Stale: it was
                    // cancelled while this buffer was in flight — dropped.
                    DecodeAdmission::OverBudget | DecodeAdmission::Stale => {}
                }
            }
            Err(DecodeFailure::Cancelled) => {
                // The user's cancel already removed the item; nothing to report.
            }
            Err(DecodeFailure::Failed(reason)) => {
                if let Some((seq, input)) = self.position(done.id) {
                    if self.queue.decode_failed(done.id, reason.clone()) {
                        self.events.push(QueueEvent::Failed {
                            id: done.id,
                            seq,
                            input,
                            reason,
                        });
                    }
                }
            }
        }
    }

    fn on_backend(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::Progress { job_id, pct } => {
                self.queue.on_progress(job_id, pct);
                if let Some(running) = self.current.as_ref().filter(|run| run.id == job_id) {
                    self.events.push(QueueEvent::Progress {
                        id: job_id,
                        seq: running.seq,
                        pct,
                    });
                }
            }
            WorkerEvent::Stage { job_id, stage } => {
                self.queue.on_stage(job_id, stage);
                if let Some(running) = self.current.as_ref().filter(|run| run.id == job_id) {
                    self.events.push(QueueEvent::Stage {
                        id: job_id,
                        seq: running.seq,
                        stage,
                    });
                }
            }
            // Emitted just before Finished; the transcript rides Finished.
            WorkerEvent::Segments { .. } => {}
            WorkerEvent::Finished {
                job_id,
                transcript,
                words,
                engine_used,
                warnings,
            } => {
                let Some(running) = self.current.take().filter(|run| run.id == job_id) else {
                    return;
                };
                self.queue
                    .on_finished(job_id, engine_used, warnings.clone());
                self.tokens.remove(&job_id);
                self.samples.remove(&job_id);
                self.events.push(QueueEvent::Finished {
                    id: job_id,
                    seq: running.seq,
                    input: running.input,
                    transcript,
                    words,
                    engine: engine_used,
                    warnings,
                });
            }
            WorkerEvent::Error { job_id, message } => {
                let Some(running) = self.current.take().filter(|run| run.id == job_id) else {
                    return;
                };
                self.queue.on_failed(job_id, message.clone());
                self.tokens.remove(&job_id);
                self.samples.remove(&job_id);
                self.events.push(QueueEvent::Failed {
                    id: job_id,
                    seq: running.seq,
                    input: running.input,
                    reason: message,
                });
            }
            WorkerEvent::Cancelled { job_id } => {
                let Some(running) = self.current.take().filter(|run| run.id == job_id) else {
                    return;
                };
                self.queue.on_cancelled(job_id);
                self.tokens.remove(&job_id);
                self.samples.remove(&job_id);
                self.events.push(QueueEvent::Cancelled {
                    id: job_id,
                    seq: running.seq,
                    input: running.input,
                });
            }
        }
    }

    /// Fills the inference slot when the queue allows it.
    fn start_inference(&mut self) -> bool {
        let Some(ticket) = self.queue.next_inference() else {
            return false;
        };
        let token = self.tokens.entry(ticket.id).or_default().clone();
        let samples = if ticket.prefetched {
            self.samples.remove(&ticket.id)
        } else {
            None
        };
        let job = Job {
            id: ticket.id,
            input: ticket.input.clone(),
            model_path: self.options.model_path.clone(),
            lang: self.options.lang.clone(),
            initial_prompt: self.options.initial_prompt.clone(),
            engine: self.options.engine,
            fast_engine: self.options.fast_engine,
            cancel: token.clone(),
            samples,
        };
        match self.backend.submit(job) {
            Ok(()) => {
                self.events.push(QueueEvent::Started {
                    id: ticket.id,
                    seq: ticket.seq,
                    input: ticket.input.clone(),
                    prefetched: ticket.prefetched,
                });
                self.current = Some(Running {
                    id: ticket.id,
                    seq: ticket.seq,
                    input: ticket.input,
                });
                true
            }
            Err(reason) => {
                self.queue.on_failed(ticket.id, reason.clone());
                self.samples.remove(&ticket.id);
                self.tokens.remove(&ticket.id);
                self.events.push(QueueEvent::Failed {
                    id: ticket.id,
                    seq: ticket.seq,
                    input: ticket.input,
                    reason,
                });
                // The slot is free again; the caller's loop starts the next item.
                true
            }
        }
    }

    /// Requests decode-ahead work while the pool has room.
    fn start_decode(&mut self) -> bool {
        let Some(ticket) = self.queue.next_decode() else {
            return false;
        };
        let token = self.tokens.entry(ticket.id).or_default().clone();
        self.prefetch.request(DecodeTask {
            id: ticket.id,
            input: ticket.input,
            cancel: token,
        });
        self.prefetch_in_flight += 1;
        true
    }

    fn position(&self, id: u64) -> Option<(usize, PathBuf)> {
        self.queue
            .item(id)
            .map(|item| (item.seq, item.input.clone()))
    }
}

impl<B: InferenceBackend, P: Prefetcher> std::fmt::Debug for QueueDriver<B, P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QueueDriver")
            .field("counts", &self.queue.counts())
            .field("current", &self.current.as_ref().map(|run| run.seq))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    use suramak_core::transcript::{Segment, Source};

    use super::super::ItemState;

    fn path(name: &str) -> PathBuf {
        PathBuf::from("/tmp/suramak-queue-driver-tests").join(name)
    }

    fn options() -> JobOptions {
        JobOptions {
            model_path: PathBuf::from("/tmp/suramak-model.bin"),
            lang: LangMode::Auto,
            initial_prompt: None,
            engine: EngineSetting::Auto,
            fast_engine: false,
        }
    }

    fn transcript(title: &str) -> Transcript {
        Transcript::new(
            title.to_string(),
            Source::File {
                name: title.to_string(),
            },
            "tiny".to_string(),
            None,
            None,
            vec![Segment {
                idx: 0,
                start_ms: 0,
                end_ms: 1_000,
                text: "hello".to_string(),
            }],
        )
    }

    /// A backend that finishes every job immediately (or fails the ids in its
    /// script) and asserts the 1-slot invariant on every submit.
    struct InstantBackend {
        events: VecDeque<WorkerEvent>,
        fail: Vec<u64>,
        active: usize,
        max_active: usize,
        submitted: Vec<u64>,
    }

    impl InstantBackend {
        fn new(fail: &[u64]) -> Self {
            Self {
                events: VecDeque::new(),
                fail: fail.to_vec(),
                active: 0,
                max_active: 0,
                submitted: Vec::new(),
            }
        }
    }

    impl InferenceBackend for InstantBackend {
        fn submit(&mut self, job: Job) -> Result<(), String> {
            assert_eq!(self.active, 0, "two inference runs overlapped");
            self.active = 1;
            self.max_active = self.max_active.max(self.active);
            self.submitted.push(job.id);
            let id = job.id;
            self.events
                .push_back(WorkerEvent::Progress { job_id: id, pct: 0 });
            self.events.push_back(WorkerEvent::Stage {
                job_id: id,
                stage: JobStage::Transcribing,
            });
            if self.fail.contains(&id) {
                self.events.push_back(WorkerEvent::Error {
                    job_id: id,
                    message: format!("model file not found (job {id})"),
                });
            } else {
                self.events.push_back(WorkerEvent::Finished {
                    job_id: id,
                    transcript: Box::new(transcript(&format!("{id}.wav"))),
                    words: Vec::new(),
                    engine_used: EngineId::Cpu,
                    warnings: Vec::new(),
                });
            }
            Ok(())
        }

        fn try_recv(&mut self) -> Option<WorkerEvent> {
            let event = self.events.pop_front()?;
            if matches!(
                event,
                WorkerEvent::Finished { .. }
                    | WorkerEvent::Error { .. }
                    | WorkerEvent::Cancelled { .. }
            ) {
                self.active -= 1;
            }
            Some(event)
        }
    }

    /// A prefetcher that hands decode results to the test on command.
    #[derive(Default)]
    struct ManualPrefetch {
        tasks: Vec<DecodeTask>,
        done: VecDeque<DecodeDone>,
    }

    impl Prefetcher for ManualPrefetch {
        fn request(&mut self, task: DecodeTask) {
            self.tasks.push(task);
        }

        fn try_recv(&mut self) -> Option<DecodeDone> {
            self.done.pop_front()
        }
    }

    /// A prefetcher that answers every request immediately with a valid (tiny)
    /// buffer — the inline-decode path's deterministic stand-in.
    #[derive(Default)]
    struct InstantPrefetch {
        done: VecDeque<DecodeDone>,
    }

    impl Prefetcher for InstantPrefetch {
        fn request(&mut self, task: DecodeTask) {
            self.done.push_back(DecodeDone {
                id: task.id,
                result: Ok(vec![0.0; 16_000]),
            });
        }

        fn try_recv(&mut self) -> Option<DecodeDone> {
            self.done.pop_front()
        }
    }

    fn drain_until_idle<B: InferenceBackend, P: Prefetcher>(
        driver: &mut QueueDriver<B, P>,
    ) -> Vec<QueueEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while !driver.is_idle() && Instant::now() < deadline {
            driver.run_once();
            events.extend(driver.drain_events());
            if !driver.is_idle() {
                thread::sleep(Duration::from_millis(2));
            }
        }
        events.extend(driver.drain_events());
        assert!(
            driver.is_idle(),
            "the driver must drain within the deadline"
        );
        events
    }

    /// Three items, one failure in the middle: every item runs in queue order,
    /// one at a time, and the failure does not stop the batch.
    #[test]
    fn mixed_outcomes_run_in_order_on_one_slot() {
        let mut driver = QueueDriver::new(
            InstantBackend::new(&[2]),
            InstantPrefetch::default(),
            options(),
        );
        driver.enqueue_all([path("a.wav"), path("b.wav"), path("c.wav")]);
        let events = drain_until_idle(&mut driver);

        let finished: Vec<usize> = events
            .iter()
            .filter_map(|event| match event {
                QueueEvent::Finished { seq, .. } => Some(*seq),
                _ => None,
            })
            .collect();
        let failed: Vec<usize> = events
            .iter()
            .filter_map(|event| match event {
                QueueEvent::Failed { seq, .. } => Some(*seq),
                _ => None,
            })
            .collect();
        assert_eq!(finished, vec![1, 3]);
        assert_eq!(failed, vec![2]);
        assert_eq!(driver.counts().done, 2);
        assert_eq!(driver.counts().failed, 1);
        assert_eq!(driver.backend.max_active, 1, "the 1-slot invariant");
        assert_eq!(driver.backend.submitted, vec![1, 2, 3]);
    }

    /// Decode-ahead never exceeds the documented bound, and each completed
    /// buffer immediately makes room for exactly one more.
    #[test]
    fn decode_ahead_never_exceeds_the_bound() {
        let mut driver = QueueDriver::new(
            InstantBackend::new(&[]),
            ManualPrefetch::default(),
            options(),
        );
        driver.enqueue_all([path("a.wav"), path("b.wav"), path("c.wav"), path("d.wav")]);
        driver.run_once();
        assert_eq!(driver.prefetch.tasks.len(), super::super::MAX_DECODE_AHEAD);

        // Complete both in-flight decodes: the first takes the slot and exactly
        // one more task is requested — the request bound, not a growing backlog.
        for task in driver.prefetch.tasks.clone() {
            driver.prefetch.done.push_back(DecodeDone {
                id: task.id,
                result: Ok(vec![0.0; 16_000]),
            });
        }
        driver.run_once();
        assert_eq!(
            driver.prefetch.tasks.len(),
            super::super::MAX_DECODE_AHEAD + 1
        );
        assert!(driver.queue().decoding_count() <= super::super::MAX_DECODE_AHEAD);
        assert!(
            driver.queue().prefetched_bytes() <= super::super::MAX_PREFETCH_BYTES,
            "the pool never grows past its documented cap"
        );
    }

    /// A queued item can be cancelled by its stable position; it never runs.
    #[test]
    fn cancel_by_position_removes_a_queued_item() {
        let mut driver = QueueDriver::new(
            SilentBackend::default(),
            InstantPrefetch::default(),
            options(),
        );
        driver.enqueue_all([path("a.wav"), path("b.wav"), path("c.wav")]);
        driver.run_once();
        assert_eq!(driver.cancel_seq(2), CancelDecision::Removed);
        let events = driver.drain_events();
        assert!(events
            .iter()
            .any(|event| matches!(event, QueueEvent::Cancelled { seq: 2, .. })));
        // The running file keeps the slot; the batch does not skip ahead until
        // the worker answers.
        assert_eq!(driver.backend.submitted, vec![1]);
        driver.backend.events.push_back(WorkerEvent::Finished {
            job_id: 1,
            transcript: Box::new(transcript("a.wav")),
            words: Vec::new(),
            engine_used: EngineId::Cpu,
            warnings: Vec::new(),
        });
        driver.run_once();
        assert_eq!(driver.backend.submitted, vec![1, 3]);
    }

    /// A backend that accepts a job and answers only when the test says so.
    #[derive(Default)]
    struct SilentBackend {
        submitted: Vec<u64>,
        events: VecDeque<WorkerEvent>,
    }

    impl InferenceBackend for SilentBackend {
        fn submit(&mut self, job: Job) -> Result<(), String> {
            self.submitted.push(job.id);
            Ok(())
        }

        fn try_recv(&mut self) -> Option<WorkerEvent> {
            self.events.pop_front()
        }
    }

    /// Cancelling the running item sets its token (the existing seam) and the
    /// slot frees only when the worker confirms.
    #[test]
    fn cancel_by_position_sets_the_running_tokens_seam() {
        let mut driver = QueueDriver::new(
            SilentBackend::default(),
            InstantPrefetch::default(),
            options(),
        );
        driver.enqueue_all([path("a.wav"), path("b.wav")]);
        driver.run_once();
        let running = driver.queue().running_id().expect("a holds the slot");
        let token = driver.tokens.get(&running).cloned().expect("its token");
        assert!(!token.is_cancelled());

        assert_eq!(driver.cancel_seq(1), CancelDecision::Running);
        assert!(token.is_cancelled(), "the running token is set");
        assert!(driver.current.is_some(), "the slot stays occupied");
        assert!(
            driver.queue().item(2).unwrap().is_waiting(),
            "no second item can start on top of a half-stopped one"
        );
        assert!(driver.queue().item(1).unwrap().cancelling);

        // The worker's answer is what frees the slot.
        driver
            .backend
            .events
            .push_back(WorkerEvent::Cancelled { job_id: running });
        driver.run_once();
        assert_eq!(driver.queue().running_id(), Some(2));
    }

    /// A cancellation that lands between the decode request and its result drops
    /// the buffer and the item never reaches the backend.
    #[test]
    fn cancel_mid_decode_drops_the_buffer() {
        let mut driver = QueueDriver::new(
            SilentBackend::default(),
            ManualPrefetch::default(),
            options(),
        );
        driver.enqueue_all([path("a.wav"), path("b.wav"), path("c.wav")]);
        driver.run_once();
        // The first file runs inline; the decode-ahead tasks are for 2 and 3.
        let task = driver
            .prefetch
            .tasks
            .iter()
            .find(|task| {
                driver
                    .queue()
                    .item(task.id)
                    .is_some_and(|item| item.seq == 2)
            })
            .expect("a decode-ahead task for the second file")
            .clone();
        assert_eq!(driver.cancel(task.id), CancelDecision::Removed);
        // The late buffer arrives after the cancel: it is dropped, never run.
        driver.prefetch.done.push_back(DecodeDone {
            id: task.id,
            result: Ok(vec![0.0; 16_000]),
        });
        for other in driver.prefetch.tasks.clone() {
            if other.id != task.id {
                driver.prefetch.done.push_back(DecodeDone {
                    id: other.id,
                    result: Ok(vec![0.0; 16_000]),
                });
            }
        }
        driver.run_once();
        assert_eq!(
            driver.queue().item(task.id).unwrap().state,
            ItemState::Cancelled
        );
        assert!(
            !driver.samples.contains_key(&task.id),
            "the late buffer for a cancelled item is dropped, never kept"
        );
        driver.backend.events.push_back(WorkerEvent::Finished {
            job_id: 1,
            transcript: Box::new(transcript("a.wav")),
            words: Vec::new(),
            engine_used: EngineId::Cpu,
            warnings: Vec::new(),
        });
        driver.run_once();
        let events = driver.drain_events();
        assert!(!events
            .iter()
            .any(|event| matches!(event, QueueEvent::Started { seq: 2, .. })));
        assert!(events
            .iter()
            .any(|event| matches!(event, QueueEvent::Started { seq: 3, .. })));
        assert_eq!(driver.backend.submitted, vec![1, 3]);
    }

    /// The queue's pre-decode failure path names the file and continues.
    #[test]
    fn a_decode_failure_is_reported_and_the_batch_continues() {
        let mut driver = QueueDriver::new(
            SilentBackend::default(),
            ManualPrefetch::default(),
            options(),
        );
        driver.enqueue_all([path("good.wav"), path("bad.wav")]);
        driver.run_once();
        // Item 1 runs inline; the head's decode-ahead task is item 2.
        let task = driver
            .prefetch
            .tasks
            .iter()
            .find(|task| {
                driver
                    .queue()
                    .item(task.id)
                    .is_some_and(|item| item.seq == 2)
            })
            .expect("a decode-ahead task for the second file")
            .clone();
        driver.prefetch.done.push_back(DecodeDone {
            id: task.id,
            result: Err(DecodeFailure::Failed("decode: bad container".to_string())),
        });
        driver.run_once();
        driver.backend.events.push_back(WorkerEvent::Finished {
            job_id: 1,
            transcript: Box::new(transcript("good.wav")),
            words: Vec::new(),
            engine_used: EngineId::Cpu,
            warnings: Vec::new(),
        });
        driver.run_once();
        let events = driver.drain_events();
        assert!(events.iter().any(|event| matches!(
            event,
            QueueEvent::Failed { seq: 2, reason, .. } if reason.contains("bad container")
        )));
        assert!(events
            .iter()
            .any(|event| matches!(event, QueueEvent::Finished { seq: 1, .. })));
    }

    /// The queue's state survives a failure: the item is terminal, the batch is
    /// still well-formed and nothing is left running.
    #[test]
    fn the_queue_continues_after_a_failure() {
        let mut driver = QueueDriver::new(
            InstantBackend::new(&[1]),
            InstantPrefetch::default(),
            options(),
        );
        driver.enqueue_all([path("a.wav"), path("b.wav")]);
        drain_until_idle(&mut driver);
        assert!(driver.queue().item(1).unwrap().state.is_terminal());
        assert_eq!(driver.queue().item(2).unwrap().state, ItemState::Done);
        assert!(driver.queue().running().is_none());
    }
}
