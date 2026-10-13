//! Runtime: executes [`Effect`]s on the UI thread (worker submit, store reads
//! and writes, export writes) and owns the background worker bridge.
//!
//! The worker itself lives on a dedicated bridge thread (the `suramak-asr`
//! `Worker` handle is not shareable); the bridge moves events into a mutex
//! queue and calls `egui::Context::request_repaint()` — `Context` is Send+Sync —
//! so progress reaches the UI even when it is otherwise idle.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use suramak_asr::worker::{Job, Worker, WorkerEvent};
use suramak_core::paths;
use suramak_core::settings::{LangMode, Settings};
use suramak_core::store::Store;
use suramak_core::transcript::Transcript;

use crate::app::{self, AppState, Effect, ExportFormat};

/// Owns the store, the settings snapshot and the worker bridge; executes effects.
pub struct Runtime {
    store: Option<Store>,
    settings: Settings,
    models_dir: PathBuf,
    bridge: Bridge,
    next_job_id: u64,
    /// Store/settings problems found at startup (shown as a status line).
    pub startup_error: Option<String>,
}

impl Runtime {
    pub fn new(ctx: egui::Context) -> Self {
        let settings = paths::settings_path()
            .ok()
            .and_then(|path| Settings::load(&path).ok())
            .unwrap_or_default();
        let models_dir = paths::models_dir().unwrap_or_else(|_| PathBuf::from("models"));
        let (store, startup_error) = match open_store() {
            Ok(store) => (Some(store), None),
            Err(err) => (None, Some(err)),
        };
        Self {
            store,
            settings,
            models_dir,
            bridge: Bridge::spawn(ctx),
            next_job_id: 0,
            startup_error,
        }
    }

    /// Drains worker events queued by the bridge thread (non-blocking).
    pub fn drain_worker(&self) -> Vec<WorkerEvent> {
        self.bridge.drain()
    }

    /// Executes effects (and any effects they produce) in order.
    pub fn apply(&mut self, effects: Vec<Effect>, state: &mut AppState) {
        let mut queue: VecDeque<Effect> = effects.into();
        while let Some(effect) = queue.pop_front() {
            match effect {
                Effect::SubmitJob { input } => self.submit_job(input, state),
                Effect::SaveTranscript(transcript) => {
                    let result = self.insert(&transcript);
                    queue.extend(state.on_transcript_saved(*transcript, result));
                }
                Effect::SaveEdits { updates } => {
                    let result = self.save_edits(state, &updates);
                    queue.extend(state.on_edits_saved(Instant::now(), result));
                }
                Effect::LoadTranscript(header) => match self.load(&header) {
                    Ok(transcript) => queue.extend(state.on_transcript_loaded(transcript)),
                    Err(err) => state.set_status(err, true),
                },
                Effect::RefreshLibrary => {
                    let (items, error) = self.list();
                    state.on_library_loaded(items, error);
                }
                Effect::Export {
                    transcript,
                    format,
                    dest,
                } => {
                    let result = write_export(&transcript, format, &dest);
                    state.on_export_done(&dest, result);
                }
            }
        }
    }

    /// Resolves model + engine plan, then hands the job to the bridge thread.
    fn submit_job(&mut self, input: PathBuf, state: &mut AppState) {
        let model_path =
            match app::resolve_model_path(&self.settings.asr.model_id, &self.models_dir) {
                Ok(path) => path,
                Err(err) => {
                    state.job_failed(err);
                    return;
                }
            };
        let lang = self.settings.asr.language.clone();
        let language = match &lang {
            LangMode::Auto => None,
            LangMode::Fixed(code) => Some(code.as_str()),
        };
        let plan = suramak_asr::resolve_for(&model_path, self.settings.asr.engine, language);
        let model_id = suramak_asr::model_id_from_path(&model_path);
        state.job_planned(
            &model_id,
            self.settings.asr.engine,
            plan.engine,
            plan.warning.clone(),
        );

        self.next_job_id += 1;
        let job = Job {
            id: self.next_job_id,
            input,
            model_path,
            lang,
            initial_prompt: self.settings.asr.initial_prompt.clone(),
            engine: self.settings.asr.engine,
        };
        if let Err(err) = self.bridge.submit(job) {
            state.job_failed(err);
        }
    }

    fn insert(&self, transcript: &Transcript) -> Result<(), String> {
        self.store
            .as_ref()
            .ok_or_else(|| "library database unavailable".to_string())?
            .insert_transcript(transcript)
            .map_err(|err| err.to_string())
    }

    fn save_edits(&self, state: &AppState, updates: &[(u32, String)]) -> Result<(), String> {
        let editor = state
            .editor
            .as_ref()
            .ok_or_else(|| "no transcript open".to_string())?;
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| "library database unavailable".to_string())?;
        for (idx, text) in updates {
            store
                .update_segment_text(editor.transcript.id, *idx, text)
                .map_err(|err| err.to_string())?;
        }
        Ok(())
    }

    fn load(&self, header: &Transcript) -> Result<Transcript, String> {
        self.store
            .as_ref()
            .ok_or_else(|| "library database unavailable".to_string())?
            .get_transcript(header.id)
            .map_err(|err| err.to_string())
    }

    fn list(&self) -> (Vec<Transcript>, Option<String>) {
        match &self.store {
            Some(store) => match store.list_transcripts() {
                Ok(items) => (items, None),
                Err(err) => (Vec::new(), Some(err.to_string())),
            },
            None => (Vec::new(), Some("library database unavailable".to_string())),
        }
    }
}

fn open_store() -> Result<Store, String> {
    paths::ensure_dirs().map_err(|err| err.to_string())?;
    let db = paths::db_path().map_err(|err| err.to_string())?;
    Store::open(&db).map_err(|err| err.to_string())
}

fn write_export(transcript: &Transcript, format: ExportFormat, dest: &Path) -> Result<(), String> {
    let text = format.render(transcript)?;
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
    }
    std::fs::write(dest, text).map_err(|err| err.to_string())
}

/// The bridge thread: owns the `suramak-asr` worker, forwards events to the UI.
pub struct Bridge {
    jobs: Option<Sender<Job>>,
    events: Arc<Mutex<Vec<WorkerEvent>>>,
    handle: Option<JoinHandle<()>>,
}

impl Bridge {
    pub fn spawn(ctx: egui::Context) -> Bridge {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let events = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::clone(&events);
        let handle = thread::Builder::new()
            .name("suramak-ui-bridge".to_string())
            .spawn(move || run_bridge(job_rx, queue, ctx))
            .expect("failed to spawn the suramak-ui bridge thread");
        Bridge {
            jobs: Some(job_tx),
            events,
            handle: Some(handle),
        }
    }

    /// Queues a job on the bridge thread. Fails when it has shut down.
    pub fn submit(&self, job: Job) -> Result<(), String> {
        self.jobs
            .as_ref()
            .ok_or_else(|| "worker bridge is shut down".to_string())?
            .send(job)
            .map_err(|_| "suramak-worker exited unexpectedly".to_string())
    }

    /// Non-blocking: takes everything queued since the last drain.
    pub fn drain(&self) -> Vec<WorkerEvent> {
        let mut guard = self
            .events
            .lock()
            .expect("the bridge event queue was poisoned");
        std::mem::take(&mut *guard)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // Closing the job channel lets the bridge thread finish its current job
        // and exit on its own; we deliberately do not join (an exit while a job
        // is running must not block).
        self.jobs.take();
        self.handle.take();
    }
}

fn run_bridge(
    job_rx: mpsc::Receiver<Job>,
    queue: Arc<Mutex<Vec<WorkerEvent>>>,
    ctx: egui::Context,
) {
    let worker = Worker::spawn();
    while let Ok(job) = job_rx.recv() {
        let job_id = job.id;
        if worker.submit(job).is_err() {
            push(&queue, worker_gone(job_id));
            ctx.request_repaint();
            continue;
        }
        loop {
            match worker.recv() {
                Some(event) => {
                    let terminal = matches!(
                        event,
                        WorkerEvent::Finished { .. } | WorkerEvent::Error { .. }
                    );
                    push(&queue, event);
                    ctx.request_repaint();
                    if terminal {
                        break;
                    }
                }
                None => {
                    push(&queue, worker_gone(job_id));
                    ctx.request_repaint();
                    break;
                }
            }
        }
    }
}

fn worker_gone(job_id: u64) -> WorkerEvent {
    WorkerEvent::Error {
        job_id,
        message: "suramak-worker exited unexpectedly".to_string(),
    }
}

fn push(queue: &Arc<Mutex<Vec<WorkerEvent>>>, event: WorkerEvent) {
    if let Ok(mut guard) = queue.lock() {
        guard.push(event);
    }
}
