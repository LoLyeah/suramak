//! Runtime: executes [`Effect`]s on the UI thread (worker submit, store reads
//! and writes, export writes, microphone capture, model downloads) and owns the
//! background worker bridge.
//!
//! The worker itself lives on a dedicated bridge thread (the `suramak-asr`
//! `Worker` handle is not shareable); the bridge moves events into a mutex
//! queue and calls `egui::Context::request_repaint()` — `Context` is Send+Sync —
//! so progress reaches the UI even when it is otherwise idle.
//!
//! M3: the cpal capture session ([`suramak_asr::capture::Recorder`] is `!Send`)
//! lives here on the UI thread; model downloads run on a detached thread whose
//! byte counter the UI polls each tick.
//!
//! M4: the polish call (core's client is blocking HTTP) runs on its own
//! detached `suramak-polish` thread — never the ASR worker, never the UI
//! thread — and reports through the same poll-and-repaint pattern.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use suramak_asr::capture;
use suramak_asr::worker::{Job, Worker, WorkerEvent};
use suramak_core::models as core_models;
use suramak_core::paths;
use suramak_core::polish::{OpenAiCompat, PolishBackend};
use suramak_core::resample;
use suramak_core::settings::{LangMode, PolishSettings, Settings};
use suramak_core::store::Store;
use suramak_core::transcript::{Segment, Transcript};
use uuid::Uuid;

use crate::app::{self, model_manager, polish, settings_form, AppState, Effect, ExportFormat};

/// Subdirectory of the system temp dir used for transient recordings
/// (`keep_audio` off — deleted once the transcript is saved).
const RECORDINGS_TEMP_DIR: &str = "suramak-recordings";

/// Owns the store, the settings snapshot and the worker bridge; executes effects.
pub struct Runtime {
    store: Option<Store>,
    settings: Settings,
    models_dir: PathBuf,
    bridge: Bridge,
    /// Kept for the polish thread's `request_repaint` wake-up (M4).
    ctx: egui::Context,
    next_job_id: u64,
    /// Store/settings problems found at startup (shown as a status line).
    pub startup_error: Option<String>,
    /// Live capture session (`!Send` — stays on the UI thread).
    recorder: Option<capture::Recorder>,
    /// Background model download (one at a time).
    download: Option<DownloadJob>,
    /// Background polish run (one at a time, M4).
    polish: Option<PolishJob>,
}

/// A model download running on a detached thread; the UI polls it.
struct DownloadJob {
    id: String,
    bytes: Arc<Mutex<u64>>,
    result: Arc<Mutex<Option<Result<(), String>>>>,
}

/// A polish run on a detached thread (`suramak-polish`); the UI polls it.
/// Core's client is blocking HTTP, so it must never run on the UI thread or the
/// ASR worker (WORKORDER-M4 §1).
struct PolishJob {
    result: PolishSlot,
}

/// Shared slot for a background polish outcome: `None` while running, `Some`
/// exactly once when the call finished (Ok = validated segments, Err = the raw
/// backend/reply error shown to the user).
type PolishSlot = Arc<Mutex<Option<Result<Vec<Segment>, String>>>>;

/// One poll of the model download (see [`Runtime::download_status`]).
pub struct DownloadStatus {
    pub id: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// `Some` exactly once, when the download finished.
    pub finished: Option<Result<(), String>>,
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
            bridge: Bridge::spawn(ctx.clone()),
            ctx,
            next_job_id: 0,
            startup_error,
            recorder: None,
            download: None,
            polish: None,
        }
    }

    /// Whether the Polish action may run: a configured base URL and an API key
    /// (keychain on macOS/Windows, `SURAMAK_OPENAI_API_KEY` on Linux).
    pub fn polish_configured(&self) -> bool {
        polish::polish_config(&self.settings, key_present()).is_ok()
    }

    /// Outcome of the background polish run, if one finished since the last
    /// poll. Terminal-once, like [`Runtime::download_status`].
    pub fn polish_status(&mut self) -> Option<Result<Vec<Segment>, String>> {
        let job = self.polish.as_ref()?;
        let result = job.result.lock().ok().and_then(|guard| guard.clone());
        if result.is_some() {
            self.polish = None;
        }
        result
    }

    /// Drains worker events queued by the bridge thread (non-blocking).
    pub fn drain_worker(&self) -> Vec<WorkerEvent> {
        self.bridge.drain()
    }

    /// A settings form seeded from the persisted settings (startup sync).
    pub fn settings_form(&self) -> settings_form::SettingsForm {
        settings_form::SettingsForm::from_settings(&self.settings)
    }

    /// Live rolling RMS of the capture session (`None` when not recording).
    pub fn capture_rms(&self) -> Option<f32> {
        self.recorder.as_ref().map(|recorder| recorder.rms())
    }

    /// Progress of the model download, if one is running.
    ///
    /// A finished download is reported **once** (the job is cleared), so callers
    /// must treat `finished: Some(..)` as terminal.
    pub fn download_status(&mut self) -> Option<DownloadStatus> {
        let job = self.download.as_ref()?;
        let bytes_done = job.bytes.lock().map(|guard| *guard).unwrap_or(0);
        let bytes_total = core_models::find_model(&job.id)
            .map(|info| info.size_bytes)
            .unwrap_or(0);
        let finished = job.result.lock().ok().and_then(|guard| guard.clone());
        if finished.is_some() {
            let id = job.id.clone();
            self.download = None;
            return Some(DownloadStatus {
                id,
                bytes_done,
                bytes_total,
                finished,
            });
        }
        Some(DownloadStatus {
            id: job.id.clone(),
            bytes_done,
            bytes_total,
            finished: None,
        })
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
                Effect::StartCapture => self.start_capture(state),
                Effect::StopCapture { title } => {
                    let follow_ups = self.stop_capture(&title, state);
                    queue.extend(follow_ups);
                }
                Effect::DiscardRecording { path } => {
                    if let Err(err) = std::fs::remove_file(&path) {
                        tracing::warn!("could not remove {}: {err}", path.display());
                    } else {
                        tracing::info!("removed temp recording {}", path.display());
                    }
                }
                Effect::RefreshModels => {
                    let rows = model_manager::rows(&self.models_dir, &self.settings.asr.model_id);
                    state.on_models_loaded(rows);
                }
                Effect::DownloadModel { id } => self.start_download(&id, state),
                Effect::DeleteModel { id } => {
                    let result = model_manager::delete_plan(&self.models_dir, &id)
                        .and_then(|plan| model_manager::apply_delete(&plan))
                        .map(|removed| {
                            removed
                                .iter()
                                .map(|path| path.display().to_string())
                                .collect::<Vec<_>>()
                        });
                    queue.extend(state.on_model_deleted(&id, result));
                }
                Effect::DeleteTranscript { id } => {
                    let result = self.delete_transcript(id);
                    queue.extend(state.on_transcript_deleted(id, result));
                }
                Effect::SetDefaultModel { id } => {
                    let result = self.set_default(&id);
                    queue.extend(state.on_default_set(&id, result));
                }
                Effect::RefreshAudioDevices => {
                    let (devices, error) = match capture::list_input_devices() {
                        Ok(devices) => (devices, None),
                        Err(suramak_asr::AsrError::CaptureUnsupported) => (
                            Vec::new(),
                            Some("not compiled into this build (feature `capture`)".to_string()),
                        ),
                        Err(err) => (Vec::new(), Some(err.to_string())),
                    };
                    state.on_audio_devices(devices, error);
                }
                Effect::SaveSettings { form } => {
                    form.apply(&mut self.settings);
                    let result = self.persist_settings(&form);
                    // The form may have changed the polish base URL/key note —
                    // refresh the action's enable from the new snapshot.
                    state.polish_configured = self.polish_configured();
                    state.on_settings_saved(result);
                }
                Effect::PolishTranscript { transcript } => {
                    if let Err(err) = self.start_polish(*transcript) {
                        queue.extend(state.on_polish_done(Err(err)));
                    }
                }
            }
        }
    }

    /// Runs the polish call for `transcript` on a detached `suramak-polish`
    /// thread: core's client is blocking HTTP (never the ASR worker, never the
    /// UI thread). The result lands in a mutex slot that [`Runtime::polish_status`]
    /// polls; the thread also requests a repaint when it finishes, like the
    /// bridge does for worker events.
    fn start_polish(&mut self, transcript: Transcript) -> Result<(), String> {
        if self.polish.is_some() {
            return Err("a polish run is already in progress".to_string());
        }
        let config = polish::polish_config(&self.settings, key_present())?;
        let segments = transcript.segments;
        let result: PolishSlot = Arc::new(Mutex::new(None));
        let outcome = Arc::clone(&result);
        let ctx = self.ctx.clone();
        tracing::info!(
            endpoint = %config.base_url,
            model = %config.model,
            segments = segments.len(),
            "polish: starting the background run"
        );
        let spawned = thread::Builder::new()
            .name("suramak-polish".to_string())
            .spawn(move || {
                let run = (|| -> Result<Vec<Segment>, String> {
                    let backend = OpenAiCompat::new().map_err(|err| err.to_string())?;
                    let cfg = PolishSettings {
                        base_url: config.base_url,
                        model: config.model,
                    };
                    backend
                        .polish(&segments, &cfg)
                        .map_err(|err| error_chain(&err))
                })();
                match &run {
                    Ok(segments) => {
                        tracing::info!(segments = segments.len(), "polish: reply validated")
                    }
                    Err(err) => tracing::warn!(error = %err, "polish: run failed"),
                }
                if let Ok(mut guard) = outcome.lock() {
                    *guard = Some(run);
                }
                ctx.request_repaint();
            });
        match spawned {
            Ok(_handle) => {
                self.polish = Some(PolishJob { result });
                Ok(())
            }
            Err(err) => Err(format!("cannot start the polish thread: {err}")),
        }
    }

    /// Opens the microphone / system-audio tap on this thread (cpal streams are
    /// `!Send`), reporting the device + any resolution warning.
    fn start_capture(&mut self, state: &mut AppState) {
        match capture::start_recording_with(&self.settings.recording.audio_source) {
            Ok(recorder) => {
                state.record_device(recorder.device_name().to_string());
                if let Some(warning) = recorder.warning() {
                    state.set_status(warning.to_string(), false);
                }
                self.recorder = Some(recorder);
            }
            Err(err) => state.record_failed(err.to_string()),
        }
    }

    /// Stops the session, resamples to 16 kHz and writes the recording wav —
    /// to `<data>/recordings/` when `keep_audio` is on, else to a temp dir
    /// (deleted once the transcript is saved; see `AppState::on_transcript_saved`).
    fn stop_capture(&mut self, title: &str, state: &mut AppState) -> Vec<Effect> {
        let Some(recorder) = self.recorder.take() else {
            state.record_failed("no recording in progress".to_string());
            return Vec::new();
        };
        let buffer = recorder.stop();
        let samples = match capture::resample_16k(&buffer.samples, buffer.rate) {
            Ok(samples) => samples,
            Err(err) => {
                state.record_failed(format!("could not resample the recording: {err}"));
                return Vec::new();
            }
        };
        let keep = self.settings.storage.keep_audio;
        let dir = if keep {
            match paths::recordings_dir() {
                Ok(dir) => dir,
                Err(err) => {
                    state.record_failed(err.to_string());
                    return Vec::new();
                }
            }
        } else {
            std::env::temp_dir().join(RECORDINGS_TEMP_DIR)
        };
        let stem = app::recording_file_stem(title);
        let path = dir.join(format!("{stem}.wav"));
        match capture::write_recording_wav(&path, &samples, resample::TARGET_RATE) {
            Ok(()) => state.on_recording_stopped(path, keep),
            Err(err) => {
                state.record_failed(format!("could not write the recording: {err}"));
                Vec::new()
            }
        }
    }

    /// Starts the background download for `id` (one at a time).
    fn start_download(&mut self, id: &str, state: &mut AppState) {
        if self.download.is_some() {
            state.on_download_done(id, Err("a download is already running".to_string()));
            return;
        }
        let Some(info) = core_models::find_model(id) else {
            state.on_download_done(id, Err(format!("unknown model '{id}'")));
            return;
        };
        let dir = self.models_dir.clone();
        let dest = dir.join(info.hf_file);
        let url = core_models::model_url(info);
        let sha256 = info.sha256.to_string();
        let bytes = Arc::new(Mutex::new(0u64));
        let result: Arc<Mutex<Option<Result<(), String>>>> = Arc::new(Mutex::new(None));
        let progress = Arc::clone(&bytes);
        let outcome = Arc::clone(&result);

        let spawned = thread::Builder::new()
            .name("suramak-model-download".to_string())
            .spawn(move || {
                let run = (|| -> Result<(), String> {
                    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
                    suramak_core::download::download_with_progress(
                        &url,
                        &dest,
                        &sha256,
                        |written| {
                            if let Ok(mut guard) = progress.lock() {
                                *guard = written;
                            }
                        },
                    )
                    .map_err(|err| err.to_string())
                })();
                if let Ok(mut guard) = outcome.lock() {
                    *guard = Some(run);
                }
            });
        match spawned {
            Ok(_handle) => {
                self.download = Some(DownloadJob {
                    id: id.to_string(),
                    bytes,
                    result,
                });
            }
            Err(err) => {
                state.on_download_done(id, Err(format!("cannot start the download thread: {err}")));
            }
        }
    }

    /// Writes `settings.model_id = id` and persists settings.toml.
    fn set_default(&mut self, id: &str) -> Result<(), String> {
        model_manager::apply_default(&mut self.settings, id)?;
        let path = paths::settings_path().map_err(|err| err.to_string())?;
        self.settings
            .save(&path)
            .map_err(|err| format!("could not write {}: {err}", path.display()))
    }

    /// Persists the settings form (toml) and, on macOS/Windows, the keychain key.
    fn persist_settings(&mut self, form: &settings_form::SettingsForm) -> Result<String, String> {
        let path = paths::settings_path().map_err(|err| err.to_string())?;
        self.settings
            .save(&path)
            .map_err(|err| format!("could not write {}: {err}", path.display()))?;
        let mut note = format!("settings saved to {}", path.display());
        let key = form.api_key.trim();
        if !key.is_empty() {
            if settings_form::api_key_supported() {
                suramak_core::keychain::set_api_key(key).map_err(|err| {
                    format!("settings saved, but the keychain refused the key: {err}")
                })?;
                note.push_str(" — API key stored in the OS keychain");
            } else {
                note.push_str(
                    " — API key ignored (the keychain is macOS/Windows only; \
                     set SURAMAK_OPENAI_API_KEY here)",
                );
            }
        }
        Ok(note)
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

    /// Deletes a transcript from the store (its segments cascade).
    fn delete_transcript(&self, id: Uuid) -> Result<(), String> {
        self.store
            .as_ref()
            .ok_or_else(|| "library database unavailable".to_string())?
            .delete_transcript(id)
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

/// Whether an API key is available for polish: the OS keychain on
/// macOS/Windows, `SURAMAK_OPENAI_API_KEY` on Linux (M3/M4 key policy).
fn key_present() -> bool {
    suramak_core::keychain::get_api_key().is_ok()
}

/// Formats an error together with its source chain.
///
/// `reqwest`'s own `Display` for a transport failure is only
/// "error sending request for url (…)" — the actual cause (connection refused,
/// connection closed before message completed, …) lives in `source()`. The
/// polish flow surfaces this string to the user, so walk the chain.
fn error_chain(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        text.push_str(" — caused by: ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
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
