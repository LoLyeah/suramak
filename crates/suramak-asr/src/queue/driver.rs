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
use suramak_core::store::{JobRecord, JobStatus, Store};
use suramak_core::transcript::{Source, Transcript, WordTiming};
use suramak_core::{SuramakError, Uuid};

use crate::cancel::CancelToken;
use crate::engine::EngineId;
use crate::pipeline::model_id_from_path;
use crate::worker::{Job, JobStage, Worker, WorkerEvent};

use super::journal::{self, CheckpointPolicy, CheckpointWriter, DurableJob, Resume, ResumePlan};
use super::{CancelDecision, DecodeAdmission, InferenceTicket, JobQueue, QueueCounts};

/// The per-job options the CLI resolved once for the whole batch.
#[derive(Debug, Clone, PartialEq)]
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
    /// M22A: the job continues an interrupted run at a committed boundary.
    /// The line is informational — the row already exists and the text is in
    /// the library.
    Resumed {
        id: u64,
        seq: usize,
        input: PathBuf,
        from_ms: i64,
        next_idx: u32,
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
            | QueueEvent::Resumed { seq, .. }
            | QueueEvent::Cancelled { seq, .. } => *seq,
        }
    }
}

/// What `resume_pending` did with one journal row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// Enqueued to continue after the committed prefix.
    Resumed { from_ms: i64, next_idx: u32 },
    /// Enqueued from zero (nothing had been committed).
    Fresh,
    /// Left failed in the journal: the source or the model is gone.
    Unavailable { reason: String },
    /// Left untouched in the journal: this launch cannot continue it (for
    /// example it ran with another model). A later launch with the right model
    /// can still resume it.
    Skipped { reason: String },
}

/// One journal row `resume_pending` looked at.
#[derive(Debug, Clone)]
pub struct PendingResume {
    pub job_id: Uuid,
    pub path: PathBuf,
    pub outcome: ResumeOutcome,
}

/// One running job's durable state (M22A).
struct Run {
    durable: DurableJob,
    writer: CheckpointWriter,
    /// A checkpoint that could not be written. The terminal event reports this
    /// instead of a success: a job whose text could not be committed is not a
    /// finished job.
    checkpoint_error: Option<String>,
}

/// The host-supplied display identity for one queue item (M22A2).
#[derive(Debug, Clone)]
struct ItemMeta {
    title: String,
    source_name: String,
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
    /// M22A: the durable journal. The driver is its **single writer** (one
    /// SQLite connection): job rows at enqueue, the checkpoint transaction on
    /// the worker's stream, the terminal status. `None` keeps the M16.1
    /// in-memory behavior exactly.
    journal: Option<Store>,
    /// Queue item id → durable job id (M22A). Absent when there is no journal.
    durable_ids: HashMap<u64, Uuid>,
    /// Running item id → its durable state.
    runs: HashMap<u64, Run>,
    /// M22A2: per-item display metadata a host supplies (the Android job's
    /// human title and the original shared file name). The CLI and desktop
    /// derive both from the input path; the phone cannot — its copied media is
    /// named by the opaque job key.
    meta: HashMap<u64, ItemMeta>,
    /// The checkpoint cadence (constant in production; tests may tighten it).
    policy: CheckpointPolicy,
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
            journal: None,
            durable_ids: HashMap::new(),
            runs: HashMap::new(),
            meta: HashMap::new(),
            policy: CheckpointPolicy::default(),
        }
    }

    /// Attaches the durable journal (M22A). Additive: every rule of [`Self::new`]
    /// stays; the driver additionally writes job rows, checkpoints and resume
    /// markers into `store`, and is the journal's only writer.
    pub fn with_journal(mut self, store: Store) -> Self {
        self.journal = Some(store);
        self
    }

    /// The journal, when this driver owns one (tests read it back through this).
    pub fn journal(&self) -> Option<&Store> {
        self.journal.as_ref()
    }

    /// M22A2: updates the per-batch options between jobs (the desktop app
    /// resolves the model/language at submit time; a settings change must not
    /// bind the next row to the previous model). Only legal while the driver
    /// has no live item — the host serializes its submits.
    pub fn set_options(&mut self, options: JobOptions) {
        debug_assert!(
            self.is_idle(),
            "options change while the queue still owns work"
        );
        self.options = options;
    }

    /// Changes the checkpoint cadence (test seam; production uses the defaults).
    #[cfg(test)]
    fn with_policy(mut self, policy: CheckpointPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Accepts a file at the end of the batch.
    ///
    /// With a journal (M22A) an item whose exact path+model already has a
    /// resumable row is not enqueued twice: the pending item keeps the job, and
    /// the caller gets its queue id back. Without a journal nothing changes.
    pub fn enqueue(&mut self, input: PathBuf) -> u64 {
        if let Some(existing) = self.live_item_for(&input) {
            return existing;
        }
        let id = self.queue.enqueue(input.clone());
        self.tokens.insert(id, CancelToken::new());
        self.attach_job_row(id, &input);
        id
    }

    /// The live queue item already bound to the resumable journal row for this
    /// path+model, if any.
    fn live_item_for(&self, input: &Path) -> Option<u64> {
        let store = self.journal.as_ref()?;
        let record = store
            .resumable_job_for(
                &input.display().to_string(),
                &self.options.model_path.display().to_string(),
            )
            .ok()??;
        self.durable_ids
            .iter()
            .find(|(queue_id, job_id)| {
                **job_id == record.id
                    && self
                        .queue
                        .item(**queue_id)
                        .is_some_and(|item| item.is_live())
            })
            .map(|(queue_id, _)| *queue_id)
    }

    /// Creates the `queued` journal row for a new item — or binds the item to
    /// the resumable row that already exists for the same path+model.
    fn attach_job_row(&mut self, id: u64, input: &Path) {
        let Some(store) = self.journal.as_ref() else {
            return;
        };
        let path = input.display().to_string();
        let model = self.options.model_path.display().to_string();
        let reuse = match store.pending_job_for(&path, &model) {
            Ok(record) => record.filter(|record| {
                // A row another live item already owns is not reusable: two
                // inputs with the same path in one batch are two jobs, never one
                // shared transcript.
                !self.durable_ids.values().any(|bound| *bound == record.id)
            }),
            Err(err) => {
                tracing::error!(queue_id = id, "cannot read the journal: {err}");
                None
            }
        };
        match reuse {
            Some(record) => {
                self.durable_ids.insert(id, record.id);
            }
            None => {
                let now = suramak_core::Utc::now();
                let record = JobRecord {
                    id: Uuid::new_v4(),
                    path,
                    model,
                    status: JobStatus::Queued,
                    created_at: now,
                    updated_at: now,
                    transcript_id: None,
                    last_committed_segment: 0,
                    last_committed_ms: 0,
                    resumed: false,
                    failure: None,
                };
                match store.insert_job(&record) {
                    Ok(()) => {
                        self.durable_ids.insert(id, record.id);
                    }
                    Err(err) => {
                        tracing::error!(queue_id = id, "cannot journal the queued job: {err}")
                    }
                }
            }
        }
    }

    pub fn enqueue_all(&mut self, inputs: impl IntoIterator<Item = PathBuf>) -> Vec<u64> {
        inputs
            .into_iter()
            .map(|input| self.enqueue(input))
            .collect()
    }

    /// M25B: accepts a user import, splitting it at pauses when it is over the
    /// platform cap. Every part is a normal queue item (and a normal journal
    /// row), so the batch runs on the existing one-slot machinery.
    ///
    /// Returns one id per queued item. `Err` only when the file cannot be
    /// decoded to find its cuts; the caller shows that message verbatim.
    pub fn enqueue_import(&mut self, input: PathBuf, parts_dir: &Path) -> Result<Vec<u64>, String> {
        self.enqueue_import_with_cap(input, parts_dir, suramak_core::session::session_cap_ms())
    }

    /// [`Self::enqueue_import`] with an explicit cap (the test seam; production
    /// always reads the platform cap).
    pub fn enqueue_import_with_cap(
        &mut self,
        input: PathBuf,
        parts_dir: &Path,
        cap_ms: i64,
    ) -> Result<Vec<u64>, String> {
        let split = suramak_core::session::split_audio_file(&input, parts_dir, cap_ms)
            .map_err(|err| format!("could not split the import: {err}"))?;
        match split {
            None => Ok(vec![self.enqueue(input)]),
            Some(output) => Ok(output
                .parts
                .iter()
                .map(|part| self.enqueue(part.path.clone()))
                .collect()),
        }
    }

    /// Accepts a file with the host's display identity (M22A2).
    ///
    /// The journal row cannot carry a title (its schema is M22A's and does not
    /// change); a resumed row therefore gets its title from the host after
    /// `resume_pending` through [`Self::set_item_meta`]. The driver uses these
    /// strings when it builds the durable transcript, so a phone share keeps the
    /// name the user saw instead of the copied media's opaque key.
    pub fn enqueue_named(&mut self, input: PathBuf, title: String, source_name: String) -> u64 {
        let id = self.enqueue(input);
        self.set_item_meta(id, title, source_name);
        id
    }

    /// Sets (or replaces) one queue item's display identity.
    pub fn set_item_meta(&mut self, id: u64, title: String, source_name: String) {
        self.meta.insert(id, ItemMeta { title, source_name });
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
                if let (Some(store), Some(job_id)) =
                    (self.journal.as_ref(), self.durable_ids.get(&id).copied())
                {
                    let _ = store.set_job_status(job_id, JobStatus::Cancelled, None);
                }
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

    /// Reads the journal and enqueues every job a previous process left in
    /// `decoding`/`transcribing` (M22A "resume on launch").
    ///
    /// A row whose source or model is gone is **not** dropped: it becomes
    /// `failed(resume_unavailable: …)` in the journal, with the reason, and the
    /// outcome says so. A row that still has its inputs is enqueued and will
    /// continue at `last_committed_segment`. A job that ran with a different
    /// model is left pending for a launch that has that model — never spliced.
    pub fn resume_pending(&mut self) -> Vec<PendingResume> {
        let Some(store) = self.journal.as_ref() else {
            return Vec::new();
        };
        let jobs = match store.resumable_jobs() {
            Ok(jobs) => jobs,
            Err(err) => {
                tracing::error!("cannot read the job journal: {err}");
                return Vec::new();
            }
        };
        let mut outcomes = Vec::new();
        let model = self.options.model_path.display().to_string();
        for record in jobs {
            let source = PathBuf::from(&record.path);
            if record.model != model {
                // Not this launch's job to fail: the row stays resumable for a
                // run with the model it was accepted with.
                let reason = format!("the job ran with a different model ({})", record.model);
                tracing::info!(job = %record.id, "{reason}");
                outcomes.push(PendingResume {
                    job_id: record.id,
                    path: source,
                    outcome: ResumeOutcome::Skipped { reason },
                });
                continue;
            }
            match journal::plan_resume(store, &record, &self.options.model_path) {
                Ok(ResumePlan::Unavailable { reason }) => {
                    let named = format!("resume_unavailable: {reason}");
                    if let Err(err) = store.fail_job_unavailable(record.id, &reason) {
                        tracing::error!(job = %record.id, "cannot record the refusal: {err}");
                    }
                    tracing::warn!(job = %record.id, path = %source.display(), "{named}");
                    outcomes.push(PendingResume {
                        job_id: record.id,
                        path: source,
                        outcome: ResumeOutcome::Unavailable { reason: named },
                    });
                }
                Ok(plan) => {
                    // M22A2: the queue row carries the marker the hosts surface.
                    let id = self.queue.enqueue_resumed(source.clone());
                    self.tokens.insert(id, CancelToken::new());
                    self.durable_ids.insert(id, record.id);
                    match plan {
                        ResumePlan::Resume {
                            from_ms, next_idx, ..
                        } => {
                            if let Err(err) = store.mark_job_resumed(record.id) {
                                tracing::error!(job = %record.id, "cannot mark resume: {err}");
                            }
                            tracing::info!(
                                job = %record.id,
                                from_ms,
                                next_idx,
                                "resuming interrupted job at its last committed segment"
                            );
                            outcomes.push(PendingResume {
                                job_id: record.id,
                                path: source,
                                outcome: ResumeOutcome::Resumed { from_ms, next_idx },
                            });
                        }
                        _ => outcomes.push(PendingResume {
                            job_id: record.id,
                            path: source,
                            outcome: ResumeOutcome::Fresh,
                        }),
                    }
                }
                Err(err) => {
                    tracing::error!(job = %record.id, "cannot plan the resume: {err}");
                    outcomes.push(PendingResume {
                        job_id: record.id,
                        path: source,
                        outcome: ResumeOutcome::Unavailable {
                            reason: format!("resume_unavailable: journal: {err}"),
                        },
                    });
                }
            }
        }
        outcomes
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
                        if let (Some(store), Some(job_id)) = (
                            self.journal.as_ref(),
                            self.durable_ids.get(&done.id).copied(),
                        ) {
                            let _ = store.set_job_status(job_id, JobStatus::Failed, Some(&reason));
                        }
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
                if stage == JobStage::Transcribing {
                    if let (Some(store), Some(run)) =
                        (self.journal.as_ref(), self.runs.get(&job_id))
                    {
                        if let Err(err) =
                            store.set_job_status(run.durable.job_id, JobStatus::Transcribing, None)
                        {
                            tracing::error!(job_id, "cannot mark transcribing: {err}");
                        }
                    }
                }
                if let Some(running) = self.current.as_ref().filter(|run| run.id == job_id) {
                    self.events.push(QueueEvent::Stage {
                        id: job_id,
                        seq: running.seq,
                        stage,
                    });
                }
            }
            // M22A: a segment the engine finalized mid-run. The writer batches
            // it; a due checkpoint commits text + journal in one transaction.
            WorkerEvent::Checkpoint { job_id, segments } => {
                let Some(run) = self.runs.get_mut(&job_id) else {
                    return;
                };
                let mut due = false;
                for segment in segments {
                    due |= run.writer.push(segment);
                }
                if !due {
                    return;
                }
                let Some(store) = self.journal.as_ref() else {
                    return;
                };
                if let Err(err) = run.writer.flush(store) {
                    // A checkpoint that cannot be written is not a finished job.
                    // Stop the run and report the failure at its terminal event.
                    tracing::error!(job_id, "checkpoint failed: {err}");
                    run.checkpoint_error = Some(format!("checkpoint: {err}"));
                    if let Some(token) = self.tokens.get(&job_id) {
                        token.cancel();
                    }
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
                self.tokens.remove(&job_id);
                self.samples.remove(&job_id);
                let mut transcript = transcript;
                let run = self.runs.remove(&job_id);
                // M22A2: the host's display identity wins over the copied media
                // name (the phone's `<key>.opus`). The CLI/desktop meta is the
                // file name itself, so their rows are unchanged.
                if let Some(run) = &run {
                    transcript.title = run.durable.title.clone();
                    transcript.source = run.durable.source.clone();
                }
                if let Some(run) = run {
                    // A checkpoint that failed earlier means the committed text
                    // is not the text this run produced: report the failure.
                    if let Some(reason) = run.checkpoint_error {
                        if let Some(store) = self.journal.as_ref() {
                            let _ = store.set_job_status(
                                run.durable.job_id,
                                JobStatus::Failed,
                                Some(&reason),
                            );
                        }
                        self.queue.on_failed(job_id, reason.clone());
                        self.events.push(QueueEvent::Failed {
                            id: job_id,
                            seq: running.seq,
                            input: running.input,
                            reason,
                        });
                        return;
                    }
                    if let Some(store) = self.journal.as_ref() {
                        if let Err(err) = store.finish_job(run.durable.job_id, &transcript, &words)
                        {
                            let reason = format!("journal: {err}");
                            tracing::error!(job_id, "{reason}");
                            let _ = store.set_job_status(
                                run.durable.job_id,
                                JobStatus::Failed,
                                Some(&reason),
                            );
                            self.queue.on_failed(job_id, reason.clone());
                            self.events.push(QueueEvent::Failed {
                                id: job_id,
                                seq: running.seq,
                                input: running.input,
                                reason,
                            });
                            return;
                        }
                    }
                }
                self.queue
                    .on_finished(job_id, engine_used, warnings.clone());
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
                if let Some(run) = self.runs.remove(&job_id) {
                    if let Some(store) = self.journal.as_ref() {
                        let _ = store.set_job_status(
                            run.durable.job_id,
                            JobStatus::Failed,
                            Some(&message),
                        );
                    }
                }
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
                // M22A: the segments produced before the cancel are real. Flush
                // the last partial batch, then close the row as cancelled — the
                // text stays in the library.
                if let Some(mut run) = self.runs.remove(&job_id) {
                    if let Some(store) = self.journal.as_ref() {
                        if let Err(err) = run.writer.flush(store) {
                            tracing::error!(job_id, "cannot flush the cancelled batch: {err}");
                        }
                        if let Err(err) =
                            store.set_job_status(run.durable.job_id, JobStatus::Cancelled, None)
                        {
                            tracing::error!(job_id, "cannot mark cancelled: {err}");
                        }
                    }
                }
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
        // M25B: the session cap is checked **before** a job starts. A file over
        // the platform cap is refused by name (`session_too_long`) with the
        // measured budget — never fed to the in-memory path where the phone's
        // LMKD would kill the app. Imports over the cap are split by the host
        // before they reach this queue; this is the safety net for every other
        // submitter (and the honest refusal for a recording that somehow grew
        // past its part).
        if let Some(duration) = decode::probe_duration_ms(&ticket.input) {
            let duration = duration as i64;
            // A deliberate `Part N of M` may run up to the cap **plus the
            // rotation grace** (the recorder keeps writing until the next real
            // pause). Any other file has the hard cap — 1 ms over is refused.
            let is_part = ticket
                .input
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(suramak_core::session::PartInfo::from_name)
                .is_some();
            let limit = if is_part {
                suramak_core::session::session_cap_ms() + suramak_core::session::SPLIT_GRACE_MS
            } else {
                suramak_core::session::session_cap_ms()
            };
            if let Some(refusal) = suramak_core::session::cap_refusal_for(
                duration,
                suramak_core::session::SessionPlatform::host(),
            )
            .filter(|_| duration > limit)
            {
                let reason = refusal.reason();
                if let (Some(store), Some(job_id)) = (
                    self.journal.as_ref(),
                    self.durable_ids.get(&ticket.id).copied(),
                ) {
                    let _ = store.set_job_status(job_id, JobStatus::Failed, Some(&reason));
                }
                self.queue.on_failed(ticket.id, reason.clone());
                self.tokens.remove(&ticket.id);
                self.samples.remove(&ticket.id);
                self.events.push(QueueEvent::Failed {
                    id: ticket.id,
                    seq: ticket.seq,
                    input: ticket.input,
                    reason,
                });
                return true;
            }
        }
        // M22A: build (or resume) the durable run before submitting. A job whose
        // inputs are gone fails here by name — the slot never opens for it.
        let durable = match self.begin_durable(&ticket) {
            Ok(durable) => durable,
            Err(reason) => {
                self.queue.on_failed(ticket.id, reason.clone());
                self.tokens.remove(&ticket.id);
                self.samples.remove(&ticket.id);
                self.events.push(QueueEvent::Failed {
                    id: ticket.id,
                    seq: ticket.seq,
                    input: ticket.input,
                    reason,
                });
                return true;
            }
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
            durable: durable.clone(),
        };
        match self.backend.submit(job) {
            Ok(()) => {
                if let Some(durable) = durable {
                    self.runs.insert(
                        ticket.id,
                        Run {
                            writer: CheckpointWriter::new(durable.clone(), self.policy),
                            durable: durable.clone(),
                            checkpoint_error: None,
                        },
                    );
                    if let Some(store) = self.journal.as_ref() {
                        if let Err(err) =
                            store.set_job_status(durable.job_id, JobStatus::Decoding, None)
                        {
                            tracing::error!(job_id = ticket.id, "cannot mark decoding: {err}");
                        }
                    }
                }
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
                if let Some(durable) = durable {
                    if let Some(store) = self.journal.as_ref() {
                        let _ =
                            store.set_job_status(durable.job_id, JobStatus::Failed, Some(&reason));
                    }
                }
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

    /// Resolves the journal row for this ticket into a runnable [`DurableJob`],
    /// or a named refusal when the job cannot be resumed.
    fn begin_durable(&mut self, ticket: &InferenceTicket) -> Result<Option<DurableJob>, String> {
        let Some(store) = self.journal.as_ref() else {
            return Ok(None);
        };
        let Some(job_id) = self.durable_ids.get(&ticket.id).copied() else {
            return Ok(None);
        };
        let record = match store.job(job_id) {
            Ok(Some(record)) => record,
            Ok(None) => return Ok(None),
            Err(err) => return Err(format!("journal: {err}")),
        };
        let plan = match journal::plan_resume(store, &record, &self.options.model_path) {
            Ok(plan) => plan,
            Err(err) => return Err(format!("journal: {err}")),
        };
        let (committed, resume) = match plan {
            ResumePlan::Unavailable { reason } => {
                let named = format!("resume_unavailable: {reason}");
                if let Err(err) = store.fail_job_unavailable(record.id, &reason) {
                    tracing::error!(job = %record.id, "cannot record the refusal: {err}");
                }
                tracing::warn!(job = %record.id, "{named}");
                return Err(named);
            }
            ResumePlan::Fresh => (Vec::new(), None),
            ResumePlan::Resume {
                committed,
                from_ms,
                next_idx,
            } => (committed, Some(Resume { from_ms, next_idx })),
        };
        if let Some(resume) = resume {
            if let Err(err) = store.mark_job_resumed(record.id) {
                tracing::error!(job = %record.id, "cannot mark resume: {err}");
            }
            tracing::info!(
                job = %record.id,
                from_ms = resume.from_ms,
                next_idx = resume.next_idx,
                "resuming interrupted job at its last committed segment"
            );
            self.events.push(QueueEvent::Resumed {
                id: ticket.id,
                seq: ticket.seq,
                input: ticket.input.clone(),
                from_ms: resume.from_ms,
                next_idx: resume.next_idx,
            });
        }
        let (title, source_name) = match self.meta.get(&ticket.id) {
            Some(meta) => (meta.title.clone(), meta.source_name.clone()),
            None => {
                let name = ticket
                    .input
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "transcript".to_string());
                (name.clone(), name)
            }
        };
        let language = match &self.options.lang {
            LangMode::Auto => None,
            LangMode::Fixed(code) => Some(code.clone()),
        };
        Ok(Some(DurableJob {
            job_id: record.id,
            transcript_id: record.transcript_id.unwrap_or_else(Uuid::new_v4),
            created_at: record.created_at,
            title: title.clone(),
            source: Source::File { name: source_name },
            model: model_id_from_path(&self.options.model_path),
            language,
            audio_path: Some(ticket.input.clone()),
            committed,
            resume,
        }))
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
        /// The full jobs, for the durability tests (M22A).
        jobs: Vec<Job>,
        events: VecDeque<WorkerEvent>,
    }

    impl InferenceBackend for SilentBackend {
        fn submit(&mut self, job: Job) -> Result<(), String> {
            self.submitted.push(job.id);
            self.jobs.push(job);
            Ok(())
        }

        fn try_recv(&mut self) -> Option<WorkerEvent> {
            self.events.pop_front()
        }
    }

    // ---- M22A: the driver as the journal's single writer ------------------

    fn durable_store(tag: &str) -> (PathBuf, Store) {
        let dir = std::env::temp_dir().join(format!(
            "suramak-driver-journal-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("library.db");
        let store = Store::open(&path).unwrap();
        (dir, store)
    }

    fn journal_job_record(path: &str) -> suramak_core::store::JobRecord {
        let now = suramak_core::Utc::now();
        suramak_core::store::JobRecord {
            id: Uuid::new_v4(),
            path: path.to_string(),
            model: options().model_path.display().to_string(),
            status: JobStatus::Transcribing,
            created_at: now,
            updated_at: now,
            transcript_id: None,
            last_committed_segment: 0,
            last_committed_ms: 0,
            resumed: false,
            failure: None,
        }
    }

    fn checkpoint_transcript(count: u32) -> Transcript {
        let mut transcript = Transcript::new(
            "lecture.wav".to_string(),
            Source::File {
                name: "lecture.wav".to_string(),
            },
            "tiny".to_string(),
            Some("en".to_string()),
            Some(PathBuf::from("/audio/lecture.wav")),
            Vec::new(),
        );
        transcript.segments = (0..count)
            .map(|idx| Segment {
                idx,
                start_ms: i64::from(idx) * 1_000,
                end_ms: (i64::from(idx) + 1) * 1_000,
                text: format!("segment {idx}"),
            })
            .collect();
        transcript
    }

    /// A real file plus a real model in the scratch dir (the journal checks
    /// both still exist before it lets a job start).
    fn durable_options(dir: &Path) -> (JobOptions, PathBuf) {
        let model = dir.join("ggml-tiny.bin");
        std::fs::write(&model, b"model").unwrap();
        let input = dir.join("lecture.wav");
        std::fs::write(&input, b"audio").unwrap();
        let mut options = options();
        options.model_path = model;
        (options, input)
    }

    /// A completed job's whole text and its `done` row land in the journal.
    #[test]
    fn a_finished_job_is_written_done_in_the_journal() {
        let (dir, store) = durable_store("done");
        let (options, input) = durable_options(&dir);
        let mut driver = QueueDriver::new(
            InstantBackend::new(&[]),
            InstantPrefetch::default(),
            options,
        )
        .with_journal(store);
        let queue_id = driver.enqueue(input);
        drain_until_idle(&mut driver);

        let journal = driver.journal().unwrap();
        let job_id = *driver.durable_ids.get(&queue_id).unwrap();
        let row = journal.job(job_id).unwrap().unwrap();
        assert_eq!(row.status, JobStatus::Done);
        assert_eq!(row.last_committed_segment, 1);
        let transcript_id = row.transcript_id.unwrap();
        assert_eq!(
            journal
                .get_transcript(transcript_id)
                .unwrap()
                .segments
                .len(),
            1
        );
        drop(driver);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cancel flushes the segments the engine already produced and keeps
    /// them; the row closes as `cancelled`, not `done`.
    #[test]
    fn a_cancelled_job_keeps_its_partial_segments_in_the_journal() {
        let (dir, store) = durable_store("cancel");
        let (options, input) = durable_options(&dir);
        let mut driver = QueueDriver::new(
            SilentBackend::default(),
            InstantPrefetch::default(),
            options,
        )
        .with_journal(store)
        .with_policy(CheckpointPolicy {
            every_segments: 1,
            every_ms: 5_000,
        });
        let queue_id = driver.enqueue(input);
        driver.run_once();
        let job_id = *driver.durable_ids.get(&queue_id).unwrap();

        // Two finalized segments, then the worker answers the cancel.
        driver.backend.events.push_back(WorkerEvent::Checkpoint {
            job_id: queue_id,
            segments: vec![
                Segment {
                    idx: 0,
                    start_ms: 0,
                    end_ms: 1_000,
                    text: "first".to_string(),
                },
                Segment {
                    idx: 1,
                    start_ms: 1_000,
                    end_ms: 2_000,
                    text: "second".to_string(),
                },
            ],
        });
        driver.run_once();
        driver
            .backend
            .events
            .push_back(WorkerEvent::Cancelled { job_id: queue_id });
        driver.run_once();

        let journal = driver.journal().unwrap();
        let row = journal.job(job_id).unwrap().unwrap();
        assert_eq!(row.status, JobStatus::Cancelled);
        assert_eq!(row.last_committed_segment, 2);
        let stored = journal
            .get_transcript(row.transcript_id.unwrap())
            .unwrap()
            .segments;
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[1].text, "second");
        assert!(journal.resumable_jobs().unwrap().is_empty());
        drop(driver);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Launch-time resume: a journal row left `transcribing` is enqueued, the
    /// row is marked resumed, and the worker receives the exact boundary.
    #[test]
    fn resume_pending_enqueues_an_interrupted_job_with_its_boundary() {
        let (dir, store) = durable_store("resume");
        let source = dir.join("lecture.wav");
        let model = dir.join("ggml-tiny.bin");
        std::fs::write(&source, b"audio").unwrap();
        std::fs::write(&model, b"model").unwrap();
        let mut record = journal_job_record(&source.display().to_string());
        record.model = model.display().to_string();
        store.insert_job(&record).unwrap();
        let transcript = checkpoint_transcript(3);
        store.commit_job_checkpoint(record.id, &transcript).unwrap();
        drop(store);

        let store = Store::open(&dir.join("library.db")).unwrap();
        let mut job_options = options();
        job_options.model_path = model;
        let mut driver = QueueDriver::new(
            SilentBackend::default(),
            InstantPrefetch::default(),
            job_options,
        )
        .with_journal(store);
        let outcomes = driver.resume_pending();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0].outcome,
            ResumeOutcome::Resumed {
                from_ms: 3_000,
                next_idx: 3
            }
        );

        driver.run_once();
        assert_eq!(driver.backend.jobs.len(), 1);
        let durable = driver.backend.jobs[0]
            .durable
            .as_ref()
            .expect("the resumed job is durable");
        assert_eq!(
            durable.resume,
            Some(Resume {
                from_ms: 3_000,
                next_idx: 3
            })
        );
        assert_eq!(durable.committed.len(), 3);
        assert_eq!(durable.job_id, record.id);
        let row = driver.journal().unwrap().job(record.id).unwrap().unwrap();
        assert!(row.resumed, "the resume marker lands on the row");
        assert_eq!(row.status, JobStatus::Decoding);
        drop(driver);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A launch whose source disappeared is a named failure — the row stays,
    /// nothing is enqueued, and no work is silently lost.
    #[test]
    fn resume_pending_fails_a_missing_source_with_the_named_reason() {
        let (dir, store) = durable_store("missing");
        let model = dir.join("ggml-tiny.bin");
        std::fs::write(&model, b"model").unwrap();
        let mut record = journal_job_record("/audio/definitely-gone.wav");
        record.model = model.display().to_string();
        store.insert_job(&record).unwrap();
        drop(store);

        let store = Store::open(&dir.join("library.db")).unwrap();
        let mut job_options = options();
        job_options.model_path = model;
        let mut driver = QueueDriver::new(
            SilentBackend::default(),
            InstantPrefetch::default(),
            job_options,
        )
        .with_journal(store);
        let outcomes = driver.resume_pending();
        assert_eq!(outcomes.len(), 1);
        let ResumeOutcome::Unavailable { reason } = &outcomes[0].outcome else {
            panic!("expected Unavailable, got {:?}", outcomes[0].outcome);
        };
        assert!(reason.starts_with("resume_unavailable:"), "{reason}");
        assert!(reason.contains("source file is gone"), "{reason}");
        assert!(driver.queue().items().is_empty());
        let row = driver.journal().unwrap().job(record.id).unwrap().unwrap();
        assert_eq!(row.status, JobStatus::Failed);
        assert!(row
            .failure
            .as_deref()
            .unwrap()
            .starts_with("resume_unavailable:"));
        drop(driver);
        let _ = std::fs::remove_dir_all(&dir);
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

    /// Writes a WAV of exactly `ms` at `rate` (silence) for the cap probes.
    fn wav_of_ms(path: &Path, ms: i64, rate: u32) {
        let n = (ms as i128 * i128::from(rate) / 1000) as usize;
        suramak_core::decode::write_wav_i16(path, &vec![0.0; n], rate, 1).expect("write fixture");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "suramak-driver-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// M25B: a job 1 ms over the platform cap is refused by name before the
    /// backend ever sees it; the reason carries the measured budget.
    #[test]
    fn a_session_one_millisecond_over_the_cap_is_refused_before_it_starts() {
        let dir = scratch("cap");
        let over = dir.join("over.wav");
        // 8 kHz keeps the fixture small; the probe reads the header only.
        wav_of_ms(&over, suramak_core::session::session_cap_ms() + 1, 8_000);
        let mut driver = QueueDriver::new(
            InstantBackend::new(&[]),
            ManualPrefetch::default(),
            options(),
        );
        driver.enqueue(over);
        let events = drain_until_idle(&mut driver);
        let reason = events
            .iter()
            .find_map(|event| match event {
                QueueEvent::Failed { reason, .. } => Some(reason.clone()),
                _ => None,
            })
            .expect("the refusal event");
        assert!(reason.starts_with("session_too_long:"), "{reason}");
        assert!(reason.contains("split it into shorter parts"), "{reason}");
        assert_eq!(
            driver.backend.submitted.len(),
            0,
            "the cap is checked before the inference slot opens"
        );
        assert!(
            driver.prefetch.tasks.is_empty(),
            "nothing was decoded ahead"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M25B: a file exactly at the cap is not refused (the queue accepts it and
    /// the pipeline runs).
    #[test]
    fn a_session_exactly_at_the_cap_is_accepted() {
        let dir = scratch("at-cap");
        let exact = dir.join("exact.wav");
        wav_of_ms(&exact, suramak_core::session::session_cap_ms(), 8_000);
        let mut driver = QueueDriver::new(
            InstantBackend::new(&[]),
            InstantPrefetch::default(),
            options(),
        );
        driver.enqueue(exact);
        let events = drain_until_idle(&mut driver);
        assert!(events
            .iter()
            .any(|event| matches!(event, QueueEvent::Started { .. })));
        assert_eq!(driver.backend.submitted.len(), 1);
        assert!(!events
            .iter()
            .any(|event| matches!(event, QueueEvent::Failed { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M25B: a deliberate part may run past the cap into the rotation grace
    /// (the recorder kept writing to the next pause) — it is not refused.
    #[test]
    fn a_part_inside_the_rotation_grace_is_accepted() {
        let dir = scratch("part-grace");
        let part = dir.join("Recording — Part 2 of 2.wav");
        wav_of_ms(
            &part,
            suramak_core::session::session_cap_ms() + suramak_core::session::SPLIT_GRACE_MS / 2,
            8_000,
        );
        let mut driver = QueueDriver::new(
            InstantBackend::new(&[]),
            InstantPrefetch::default(),
            options(),
        );
        driver.enqueue(part);
        let events = drain_until_idle(&mut driver);
        assert!(events
            .iter()
            .any(|event| matches!(event, QueueEvent::Finished { .. })));
        assert!(!events
            .iter()
            .any(|event| matches!(event, QueueEvent::Failed { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M25B: an import over the cap becomes two normal queue rows, named
    /// `Part 1 of 2` / `Part 2 of 2`, both under the cap.
    #[test]
    fn an_over_cap_import_splits_into_two_queued_parts() {
        let dir = scratch("import2");
        let input = dir.join("Interview.wav");
        // 9.7 s speech, a 0.7 s pause straddling 10 s, then 5 s more.
        let rate = 16_000;
        let mut samples = vec![0.0f32; (9_700 * rate / 1000) as usize];
        for (index, sample) in samples.iter_mut().enumerate() {
            *sample = 0.4 * (index as f32 * 0.08).sin();
        }
        samples.extend(std::iter::repeat_n(0.0, (700 * rate / 1000) as usize));
        let tail_start = samples.len();
        samples.resize(tail_start + (5_000 * rate / 1000) as usize, 0.0);
        for (index, sample) in samples[tail_start..].iter_mut().enumerate() {
            *sample = 0.4 * (index as f32 * 0.08).sin();
        }
        suramak_core::decode::write_wav_i16(&input, &samples, rate, 1).expect("write");

        let mut driver = QueueDriver::new(
            InstantBackend::new(&[]),
            ManualPrefetch::default(),
            options(),
        );
        let ids = driver
            .enqueue_import_with_cap(input, &dir.join("parts"), 10_000)
            .expect("split");
        assert_eq!(ids.len(), 2);
        let items: Vec<_> = ids
            .iter()
            .map(|id| driver.queue().item(*id).expect("queued"))
            .collect();
        assert_eq!(
            suramak_core::session::PartInfo::from_name(&items[0].name()),
            Some(suramak_core::session::PartInfo::new(1, 2))
        );
        assert_eq!(
            suramak_core::session::PartInfo::from_name(&items[1].name()),
            Some(suramak_core::session::PartInfo::new(2, 2))
        );
        for (index, item) in items.iter().enumerate() {
            assert!(item.input.is_file(), "part {index} exists");
            assert!(
                suramak_core::decode::probe_duration_ms(&item.input).unwrap() <= 10_000,
                "part {index} is under the cap"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M25B: a far-over-cap import becomes three queued parts with dense
    /// `Part N of 3` metadata.
    #[test]
    fn an_over_cap_import_splits_into_three_queued_parts() {
        let dir = scratch("import3");
        let input = dir.join("All hands.wav");
        let rate = 16_000;
        let mut samples = Vec::new();
        for (speech_ms, silence_ms) in [(9_800, 600), (9_200, 1_000), (7_400, 0)] {
            let start = samples.len();
            samples.resize(start + (speech_ms * rate / 1000) as usize, 0.0);
            for (index, sample) in samples[start..].iter_mut().enumerate() {
                *sample = 0.4 * (index as f32 * 0.08).sin();
            }
            samples.extend(std::iter::repeat_n(
                0.0,
                (silence_ms * rate / 1000) as usize,
            ));
        }
        suramak_core::decode::write_wav_i16(&input, &samples, rate, 1).expect("write");

        let mut driver = QueueDriver::new(
            InstantBackend::new(&[]),
            ManualPrefetch::default(),
            options(),
        );
        let ids = driver
            .enqueue_import_with_cap(input, &dir.join("parts"), 10_000)
            .expect("split");
        assert_eq!(ids.len(), 3);
        for (index, id) in ids.iter().enumerate() {
            let item = driver.queue().item(*id).expect("queued");
            assert_eq!(
                suramak_core::session::PartInfo::from_name(&item.name()),
                Some(suramak_core::session::PartInfo::new(index as u32 + 1, 3))
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
