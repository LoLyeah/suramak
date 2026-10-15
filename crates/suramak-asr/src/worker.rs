//! One dedicated worker thread (`suramak-worker`) with std mpsc channels.
//!
//! UI/CLI → [`Worker::submit`] sends a [`Job`]; the worker → consumer channel emits
//! [`WorkerEvent`]s. The whisper context cache lives in (and only in) the worker thread.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use serde::{Deserialize, Serialize};
use suramak_core::settings::{EngineSetting, LangMode};
use suramak_core::transcript::{Segment, Transcript};

use crate::cancel::CancelToken;
use crate::engine::{self, EngineId};
use crate::pipeline;
use crate::whisper::{self, AsrError, ContextCache};

/// Name of the worker thread (visible in debuggers/profilers).
pub const WORKER_THREAD_NAME: &str = "suramak-worker";

/// A transcription job submitted to the worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: u64,
    pub input: PathBuf,
    pub model_path: PathBuf,
    pub lang: LangMode,
    pub initial_prompt: Option<String>,
    /// Requested engine; absent in older callers → `Auto`.
    #[serde(default)]
    pub engine: EngineSetting,
    /// M10A: the "Fast engine (experimental)" setting; absent in older callers →
    /// OFF (today's routing exactly).
    #[serde(default)]
    pub fast_engine: bool,
    /// M9e (P2-2): the cooperative cancel token for this job. The submitter
    /// keeps a clone and sets it; the worker/pipeline observe it at stage
    /// boundaries. Not serialized (a token is a live process handle; the CLI
    /// and UI both construct jobs in-process).
    #[serde(skip)]
    pub cancel: CancelToken,
}

/// Events emitted by the worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerEvent {
    /// Decoding progress, 0–100 (from whisper's progress callback).
    Progress { job_id: u64, pct: u8 },
    /// The final segment list for the job (emitted once, just before `Finished`).
    Segments { job_id: u64, segments: Vec<Segment> },
    /// The completed transcript plus the engine that actually ran.
    Finished {
        job_id: u64,
        transcript: Transcript,
        engine_used: EngineId,
        /// Non-fatal engine notes (downgrades, Core ML fallback).
        warnings: Vec<String>,
    },
    /// The job failed; `message` is human-readable.
    Error { job_id: u64, message: String },
    /// M9e (P2-2): the job was cancelled through its [`CancelToken`] — the
    /// pipeline stopped at a stage boundary, partial results were discarded
    /// and the worker is ready for the next job. Terminal, like `Finished`.
    Cancelled { job_id: u64 },
}

/// Handle to the `suramak-worker` thread.
pub struct Worker {
    jobs: Option<Sender<Job>>,
    events: Receiver<WorkerEvent>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    /// Spawns the worker thread.
    ///
    /// Panics if the OS refuses to spawn a thread, which leaves the process unusable.
    pub fn spawn() -> Worker {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>();
        let handle = thread::Builder::new()
            .name(WORKER_THREAD_NAME.to_string())
            .spawn(move || run_worker(job_rx, event_tx))
            .expect("failed to spawn suramak-worker thread");
        Worker {
            jobs: Some(job_tx),
            events: event_rx,
            handle: Some(handle),
        }
    }

    /// Queues a job. Fails if the worker thread has exited.
    pub fn submit(&self, job: Job) -> Result<(), AsrError> {
        self.jobs
            .as_ref()
            .ok_or(AsrError::WorkerStopped)?
            .send(job)
            .map_err(|_| AsrError::WorkerStopped)
    }

    /// Non-blocking poll for the next event (UI-style usage).
    pub fn try_recv(&self) -> Option<WorkerEvent> {
        self.events.try_recv().ok()
    }

    /// Blocking receive; `None` means the worker is gone.
    pub fn recv(&self) -> Option<WorkerEvent> {
        self.events.recv().ok()
    }

    /// Drops the job sender and joins the worker thread (waits for the current job).
    fn shutdown_impl(&mut self) {
        self.jobs.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown_impl();
    }
}

fn run_worker(jobs: Receiver<Job>, events: Sender<WorkerEvent>) {
    // Route whisper.cpp/GGML C-level logs into whisper-rs's hooks. With neither the
    // `log_backend` nor `tracing_backend` feature enabled this silences the C-level
    // model-load chatter; the CLI/UI owns all user-facing output. `suramak-asr` then
    // layers its Core ML engagement probe on top (see `whisper::install_log_hooks`).
    whisper_rs::install_logging_hooks();

    let mut cache = ContextCache::new();
    // M10A: the Parakeet recognizer cache (empty and unused without the feature).
    let mut parakeet_cache = crate::parakeet::ParakeetCache::new();
    while let Ok(job) = jobs.recv() {
        let job_id = job.id;
        let _ = events.send(WorkerEvent::Progress { job_id, pct: 0 });

        // M9e (P2-2): the first cooperative check point — a cancel that landed
        // before this job started (the token can be set while the job sits in
        // the channel) exits here, before the model is even loaded.
        if job.cancel.is_cancelled() {
            tracing::info!(job_id, "job cancelled before it started");
            let _ = events.send(WorkerEvent::Cancelled { job_id });
            continue;
        }

        // Engine selection is a pure function of
        // (setting, fast engine, language, availability).
        let language = match &job.lang {
            LangMode::Auto => None,
            LangMode::Fixed(code) => Some(code.as_str()),
        };
        let plan = engine::resolve_for(&job.model_path, job.engine, job.fast_engine, language);

        let progress_events = events.clone();
        let mut on_progress = move |pct| {
            let _ = progress_events.send(WorkerEvent::Progress { job_id, pct });
        };

        // M10A: the fast engine runs through sherpa-onnx — no whisper context.
        // The whisper tiers keep the exact flow below.
        let result = if plan.engine == EngineId::Parakeet {
            let models_dir = job.model_path.parent().unwrap_or(std::path::Path::new("."));
            let model_dir = suramak_core::models::parakeet_dir(models_dir);
            match parakeet_cache.get_or_load(
                &model_dir,
                whisper::default_threads(),
                crate::parakeet::provider(),
            ) {
                Ok(recognizer) => pipeline::transcribe_file_parakeet(
                    recognizer,
                    &job.input,
                    &job.lang,
                    &plan,
                    &job.cancel,
                    &mut on_progress,
                ),
                Err(err) => Err(err),
            }
        } else {
            match cache.get_or_load(&job.model_path, plan.engine.use_gpu()) {
                Ok(ctx) => pipeline::transcribe_file(
                    ctx,
                    &job.input,
                    &job.model_path,
                    &job.lang,
                    job.initial_prompt.as_deref(),
                    whisper::default_threads(),
                    &plan,
                    &job.cancel,
                    &mut on_progress,
                ),
                Err(err) => Err(err),
            }
        };

        match result {
            Ok(outcome) => {
                let _ = events.send(WorkerEvent::Segments {
                    job_id,
                    segments: outcome.transcript.segments.clone(),
                });
                let _ = events.send(WorkerEvent::Finished {
                    job_id,
                    transcript: outcome.transcript,
                    engine_used: outcome.engine_used,
                    warnings: outcome.warnings,
                });
            }
            // M9e (P2-2): a cancelled job is terminal but is not a failure —
            // the partial results were dropped by the pipeline and the worker
            // (and its context cache) stays healthy for the next job.
            Err(AsrError::Cancelled) => {
                tracing::info!(job_id, "job cancelled at a stage boundary");
                let _ = events.send(WorkerEvent::Cancelled { job_id });
            }
            Err(err) => {
                let _ = events.send(WorkerEvent::Error {
                    job_id,
                    message: err.to_string(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: u64, cancel: CancelToken) -> Job {
        Job {
            id,
            input: PathBuf::from("/nonexistent/suramak/input.wav"),
            model_path: PathBuf::from("/nonexistent/suramak/ggml-tiny.bin"),
            lang: LangMode::Auto,
            initial_prompt: None,
            engine: EngineSetting::Auto,
            fast_engine: false,
            cancel,
        }
    }

    /// M9e (P2-2): a job whose token is set before the worker picks it up
    /// exits at the first check point — before the model load — as a terminal
    /// `Cancelled`, and the worker stays alive: the next job runs normally
    /// (here it fails on the missing model, which proves the thread and its
    /// loop survived the cancellation cleanly).
    #[test]
    fn a_cancelled_job_exits_cleanly_and_the_worker_keeps_going() {
        let worker = Worker::spawn();
        let token = CancelToken::new();
        token.cancel();
        worker.submit(job(1, token)).expect("submit");
        assert!(matches!(
            worker.recv(),
            Some(WorkerEvent::Progress { job_id: 1, pct: 0 })
        ));
        match worker.recv() {
            Some(WorkerEvent::Cancelled { job_id }) => assert_eq!(job_id, 1),
            other => panic!("expected Cancelled, got {other:?}"),
        }

        // The worker is still running: a fresh (uncancelled) job reaches the
        // model load and fails there — an Error, not a hang, not a dead thread.
        worker
            .submit(job(2, CancelToken::new()))
            .expect("submit after a cancellation");
        assert!(matches!(
            worker.recv(),
            Some(WorkerEvent::Progress { job_id: 2, pct: 0 })
        ));
        match worker.recv() {
            Some(WorkerEvent::Error { job_id, message }) => {
                assert_eq!(job_id, 2);
                assert!(
                    message.contains("model file not found"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// A cancellation that lands while the job waits in the channel (before
    /// the worker picks it up) is observed at the same first check point.
    #[test]
    fn a_cancel_queued_with_the_job_is_observed_before_the_model_load() {
        let worker = Worker::spawn();
        let token = CancelToken::new();
        worker.submit(job(7, token.clone())).expect("submit");
        // The token is set right after submission; the worker may or may not
        // have started — either way the answer is Cancelled (never an Error).
        token.cancel();
        let mut cancelled = false;
        for _ in 0..4 {
            match worker.recv() {
                Some(WorkerEvent::Cancelled { job_id }) => {
                    assert_eq!(job_id, 7);
                    cancelled = true;
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
        assert!(cancelled, "the queued cancellation must be observed");
    }
}
