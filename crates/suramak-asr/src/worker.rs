//! One dedicated worker thread (`suramak-worker`) with std mpsc channels.
//!
//! UI/CLI → [`Worker::submit`] sends a [`Job`]; the worker → consumer channel emits
//! [`WorkerEvent`]s. The whisper context cache lives in (and only in) the worker thread.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use serde::{Deserialize, Serialize};
use suramak_core::settings::LangMode;
use suramak_core::transcript::{Segment, Transcript};

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
}

/// Events emitted by the worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerEvent {
    /// Decoding progress, 0–100 (from whisper's progress callback).
    Progress { job_id: u64, pct: u8 },
    /// The final segment list for the job (emitted once, just before `Finished`).
    Segments { job_id: u64, segments: Vec<Segment> },
    /// The completed transcript.
    Finished { job_id: u64, transcript: Transcript },
    /// The job failed; `message` is human-readable.
    Error { job_id: u64, message: String },
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
    // model-load chatter; the CLI/UI owns all user-facing output.
    whisper_rs::install_logging_hooks();

    let mut cache = ContextCache::new();
    while let Ok(job) = jobs.recv() {
        let job_id = job.id;
        let _ = events.send(WorkerEvent::Progress { job_id, pct: 0 });

        let ctx = match cache.get_or_load(&job.model_path) {
            Ok(ctx) => ctx,
            Err(err) => {
                let _ = events.send(WorkerEvent::Error {
                    job_id,
                    message: err.to_string(),
                });
                continue;
            }
        };

        let progress_events = events.clone();
        let result = pipeline::transcribe_file(
            ctx,
            &job.input,
            &job.model_path,
            &job.lang,
            job.initial_prompt.as_deref(),
            whisper::default_threads(),
            move |pct| {
                let _ = progress_events.send(WorkerEvent::Progress { job_id, pct });
            },
        );

        match result {
            Ok(transcript) => {
                let _ = events.send(WorkerEvent::Segments {
                    job_id,
                    segments: transcript.segments.clone(),
                });
                let _ = events.send(WorkerEvent::Finished { job_id, transcript });
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
