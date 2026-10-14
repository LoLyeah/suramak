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
//!
//! M8: playback. The cpal output engine lives in [`suramak_playback`]'s own
//! thread (the stream is `!Send`); the runtime opens it on the first `Play`,
//! forwards transport commands and exposes the polled position so the frame
//! loop can draw the playhead — [`Runtime::playback_status`], like
//! [`Runtime::capture_rms`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use suramak_asr::capture;
use suramak_asr::live;
use suramak_asr::mix;
use suramak_asr::worker::{Job, Worker, WorkerEvent};
use suramak_core::models as core_models;
use suramak_core::paths;
use suramak_core::polish::{OpenAiCompat, PolishBackend};
use suramak_core::resample;
use suramak_core::settings::{LangMode, PolishSettings, RecordSource, Settings};
use suramak_core::store::Store;
use suramak_core::transcript::{Segment, Transcript};
use suramak_playback::{Command as PlaybackCommand, Engine, Status as PlaybackStatus};
use uuid::Uuid;

use crate::app::{
    self, model_manager, polish, record_source_label, settings_form, AppState, Effect,
    ExportFormat, RecordState,
};

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
    /// Live capture session(s) (`!Send` — stays on the UI thread). M6: one
    /// `MultiRecorder` holding a recorder per selected source.
    session: Option<capture::MultiRecorder>,
    /// Live transcription machinery for the running session (M7): the pure
    /// scheduler session + provisional transcript, the decode thread and the
    /// per-source device-rate buffers waiting to be flushed into the 16 kHz
    /// live stream. `None` while not recording (or when live is unavailable).
    live: Option<LiveCapture>,
    /// Background model download (one at a time).
    download: Option<DownloadJob>,
    /// Background polish run (one at a time, M4).
    polish: Option<PolishJob>,
    /// The live output engine (M8), keyed by the file it was opened for.
    playback: Option<PlaybackSession>,
    /// A playback session that could not even be started (thread spawn);
    /// reported to the UI once through [`Runtime::playback_status`].
    playback_error: Option<String>,
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

/// A live playback engine and the file it was opened for (M8). The engine's
/// thread owns the cpal stream; the runtime only sends it commands.
struct PlaybackSession {
    path: PathBuf,
    engine: Engine,
}

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
            session: None,
            live: None,
            download: None,
            polish: None,
            playback: None,
            playback_error: None,
        }
    }

    /// Whether the Polish action may run: a configured base URL and an API key
    /// (keychain on macOS/Windows, `SURAMAK_OPENAI_API_KEY` on Linux).
    pub fn polish_configured(&self) -> bool {
        polish::polish_config(&self.settings, key_present()).is_ok()
    }

    /// M9a: the persisted appearance setting (`settings.ui.appearance`), which
    /// the shell resolves against the OS theme and applies live.
    pub fn appearance(&self) -> suramak_core::settings::Appearance {
        self.settings.ui.appearance
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

    /// Live rolling RMS of the capture session (`None` when not recording);
    /// M6: the loudest of the live sources (see `MultiRecorder::rms`).
    pub fn capture_rms(&self) -> Option<f32> {
        self.session.as_ref().map(|session| session.rms())
    }

    /// Per-frame poll of the playback engine (M8): position, duration, the
    /// finished flag and a terminal-once error (`None` while no engine exists).
    pub fn playback_status(&mut self) -> Option<PlaybackStatus> {
        if let Some(message) = self.playback_error.take() {
            return Some(PlaybackStatus::failure(message, true));
        }
        let session = self.playback.as_ref()?;
        Some(session.engine.poll())
    }

    /// Executes a playback transport command (M8).
    ///
    /// `Play` opens (or replaces) the engine for the file it carries — the
    /// engine thread decodes and opens the output device on that first command,
    /// so a headless machine lands in a clean error state instead of failing to
    /// start. `Stop` also releases the session: leaving the editor must not keep
    /// a decoded buffer parked.
    fn playback_command(&mut self, source: Option<PathBuf>, command: PlaybackCommand) {
        if matches!(command, PlaybackCommand::Stop) {
            if let Some(session) = self.playback.take() {
                if let Err(err) = session.engine.send(PlaybackCommand::Stop) {
                    tracing::warn!("playback: {err}");
                }
            }
            return;
        }
        if let Some(path) = source {
            let stale = self
                .playback
                .as_ref()
                .is_none_or(|session| session.path != path);
            if stale {
                self.playback = None;
                match Engine::start(path.clone()) {
                    Ok(engine) => {
                        self.playback = Some(PlaybackSession { path, engine });
                    }
                    Err(err) => {
                        tracing::warn!("playback: {err}");
                        self.playback_error = Some(err);
                        return;
                    }
                }
            }
        }
        let Some(session) = self.playback.as_ref() else {
            // Pause/resume/seek without a session (it was never opened, or the
            // load failed): nothing to do — the UI's own position stands.
            return;
        };
        if let Err(err) = session.engine.send(command) {
            self.playback_error = Some(err);
        }
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
                Effect::RefreshAudioProcesses => {
                    let (processes, error) = match capture::list_audio_processes() {
                        Ok(processes) => (processes, None),
                        Err(suramak_asr::AsrError::CaptureUnsupported) => (
                            Vec::new(),
                            Some(
                                "needs a macOS build with capture (CoreAudio process taps, \
                                 macOS 14.2+)"
                                    .to_string(),
                            ),
                        ),
                        Err(err) => (Vec::new(), Some(err.to_string())),
                    };
                    state.on_audio_processes(processes, error);
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
                Effect::Playback { source, command } => self.playback_command(source, command),
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

    /// Opens the capture session(s) on this thread (cpal streams are `!Send`):
    /// one recorder per selected source, per-app keys resolved to live pids,
    /// reporting the source labels + any resolution warnings.
    fn start_capture(&mut self, state: &mut AppState) {
        let sources = self.settings.recording.effective_sources();
        let inputs = match resolve_capture_inputs(&sources) {
            Ok(inputs) => inputs,
            Err(message) => {
                state.record_failed(message);
                return;
            }
        };
        match capture::start_recording_multi(&inputs) {
            Ok(session) => {
                state.record_sources(sources.iter().map(record_source_label).collect());
                let warnings = session.warnings();
                if !warnings.is_empty() {
                    state.set_status(warnings.join(" · "), false);
                }
                self.session = Some(session);
                // M7: live partials ride the session just opened (best-effort —
                // a recording without them is still a working recording).
                self.start_live();
            }
            Err(err) => state.record_failed(err.to_string()),
        }
    }

    /// Starts the live transcription machinery for the session just opened (M7).
    ///
    /// Best-effort by design: a recording without live partials is still a
    /// working recording, and the final pass at Stop is unaffected either way —
    /// problems are logged, never fatal. The decode thread loads the model
    /// lazily on the first window (~`step_ms` into the recording).
    fn start_live(&mut self) {
        let config = live::LiveConfig::default();
        let session = match live::LiveSession::new(config.clone()) {
            Ok(session) => session,
            Err(err) => {
                tracing::warn!("live transcription unavailable: {err}");
                return;
            }
        };
        let model_path =
            match app::resolve_model_path(&self.settings.asr.model_id, &self.models_dir) {
                Ok(path) => path,
                Err(err) => {
                    tracing::info!("live transcription unavailable: {err}");
                    return;
                }
            };
        let lang = self.settings.asr.language.clone();
        let language = match &lang {
            LangMode::Auto => None,
            LangMode::Fixed(code) => Some(code.as_str()),
        };
        let plan = suramak_asr::resolve_for(&model_path, self.settings.asr.engine, language);
        let decoder = live::LiveDecoder::spawn(
            live::LiveDecodeSettings {
                model_path,
                lang,
                initial_prompt: self.settings.asr.initial_prompt.clone(),
                plan,
                n_threads: suramak_asr::whisper::default_threads(),
            },
            config.max_queued,
        );
        tracing::info!(
            step_ms = config.step_ms,
            overlap_ms = config.overlap_ms,
            max_queued = config.max_queued,
            "live transcription armed for this recording"
        );
        self.live = Some(LiveCapture {
            session,
            decoder,
            pending: Vec::new(),
            published_revision: 0,
            published_skipped: 0,
        });
    }

    /// One frame's worth of the live machinery (M7): drains finished decodes
    /// into the provisional transcript, feeds new capture audio through the
    /// same mix the final pass uses, and hands windows to the decode thread.
    ///
    /// Called from the frame pump while a recording runs; a no-op otherwise.
    /// Stop tears the machinery down here and in [`Runtime::stop_capture`] —
    /// provisional content never outlives the recording.
    pub fn live_tick(&mut self, state: &mut AppState) {
        let stopping = matches!(&state.record, RecordState::Recording(rec) if rec.stopping);
        if stopping || self.session.is_none() {
            self.live = None;
            return;
        }
        let Some(mut live) = self.live.take() else {
            return;
        };
        match self.live_step(&mut live, state) {
            LiveStep::Keep => self.live = Some(live),
            LiveStep::Stop(reason) => {
                tracing::warn!("live transcription stopped for this recording: {reason}");
            }
        }
    }

    /// The body of [`Runtime::live_tick`], factored out so the machinery can be
    /// taken out of `self` (and dropped on failure) without borrow conflicts.
    fn live_step(&mut self, live: &mut LiveCapture, state: &mut AppState) -> LiveStep {
        // 1. Finished decodes → the provisional transcript.
        for result in live.decoder.take_results() {
            if let Some(error) = result.error {
                return LiveStep::Stop(error);
            }
            live.session
                .ingest_decoded(&result.window, &result.segments);
        }

        // 2. New capture audio → per-source device-rate buffers.
        let Some(session) = self.session.as_mut() else {
            return LiveStep::Stop("capture session gone".to_string());
        };
        let chunks = session.take_live_chunk();
        if !chunks.is_empty() {
            if live.pending.len() != chunks.len() {
                live.pending = chunks
                    .iter()
                    .map(|(rate, _)| PendingSource {
                        rate: *rate,
                        samples: Vec::new(),
                    })
                    .collect();
            }
            for (slot, (_, samples)) in live.pending.iter_mut().zip(chunks.iter()) {
                slot.samples.extend_from_slice(samples);
            }
        }

        // 3. Flush the accumulated chunk through the one mixing engine.
        if live.should_flush() {
            live.flush(state);
        }

        // 4. Publish to the record view when the provisional content or the
        //    backlog count changed (a skip can move before the next decode
        //    answers).
        if live.session.revision() != live.published_revision
            || live.session.skipped_windows() != live.published_skipped
        {
            live.published_revision = live.session.revision();
            live.published_skipped = live.session.skipped_windows();
            state.record_provisional(
                live.session.segments().to_vec(),
                live.session.skipped_windows(),
            );
        }
        LiveStep::Keep
    }

    /// Stops every session, mixes the sources into the one 16 kHz mono stream
    /// (per-source gain/mute from the record view, clock-drift correction from
    /// the session span) and writes the recording wav — to
    /// `<data>/recordings/` when `keep_audio` is on, else to a temp dir
    /// (deleted once the transcript is saved; see `AppState::on_transcript_saved`).
    ///
    /// M7: the live machinery is torn down first — the decode thread is
    /// signalled and detached (an in-flight window finishes on its own), so the
    /// final pass runs against exactly the pre-M7 pipeline: the captured
    /// buffers, the mix, the wav write and the job flow are untouched.
    fn stop_capture(&mut self, title: &str, state: &mut AppState) -> Vec<Effect> {
        if self.live.take().is_some() {
            tracing::info!("live transcription: recording stopped — provisional content discarded");
        }
        let Some(session) = self.session.take() else {
            state.record_failed("no recording in progress".to_string());
            return Vec::new();
        };
        let (buffers, duration) = session.stop();
        // The record view's per-source gain/mute (unity when it holds none or a
        // different source count — e.g. a scripted stop).
        let (labels, gains, mutes) = match &state.record {
            RecordState::Recording(recording) if recording.sources.len() == buffers.len() => (
                recording
                    .sources
                    .iter()
                    .map(|source| source.label.clone())
                    .collect::<Vec<_>>(),
                recording
                    .sources
                    .iter()
                    .map(|source| source.gain)
                    .collect::<Vec<_>>(),
                recording
                    .sources
                    .iter()
                    .map(|source| source.muted)
                    .collect::<Vec<_>>(),
            ),
            _ => (
                (1..=buffers.len())
                    .map(|index| format!("source {index}"))
                    .collect(),
                vec![1.0; buffers.len()],
                vec![false; buffers.len()],
            ),
        };
        let tracks: Vec<mix::MixTrack<'_>> = buffers
            .iter()
            .zip(labels.iter())
            .zip(gains.iter().zip(mutes.iter()))
            .map(|((buffer, label), (gain, muted))| mix::MixTrack {
                label: label.clone(),
                rate: buffer.rate,
                samples: &buffer.samples,
                gain: *gain,
                muted: *muted,
            })
            .collect();
        let outcome = match mix::mix(&tracks, Some(duration)) {
            Ok(outcome) => outcome,
            Err(err) => {
                state.record_failed(format!("could not mix the recording: {err}"));
                return Vec::new();
            }
        };
        // The mix's honesty line: per-source measured clocks + saturation.
        for report in &outcome.reports {
            tracing::info!(
                source = %report.label,
                rate = report.rate,
                samples = report.captured_samples,
                seconds = report.captured_seconds,
                drift_ppm = report.drift_ppm,
                gain = report.applied_gain,
                muted = report.muted,
                clamped = report.clamped,
                "capture source mixed"
            );
        }
        if outcome.clipped_samples > 0 {
            tracing::warn!(
                clipped_samples = outcome.clipped_samples,
                peak = outcome.peak,
                "the mix exceeded full scale and saturated at the PCM boundary"
            );
        }
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
        match capture::write_recording_wav(&path, &outcome.samples, resample::TARGET_RATE) {
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

    /// Best-effort shutdown of everything that owns a whisper context or a
    /// device — the quit path's teardown (WORKORDER-HOTFIX-QUIT).
    ///
    /// Reuses the existing teardown paths and stays bounded by `grace` in
    /// total, never blocking longer: the live decode thread is signalled and
    /// waited for (an idle thread exits and drops its whisper context — and
    /// the Metal residency sets behind it — right away), the capture session is
    /// stopped through [`capture::MultiRecorder::stop`] (streams first, then
    /// any tap; the recorded buffers are discarded — the user quit, nothing is
    /// saved), playback gets its existing `Stop`, and the worker bridge is
    /// closed and waited for (the bridge thread joins the worker and drops its
    /// context cache as it exits).
    ///
    /// A job in flight is **not** interrupted — a full join would block the
    /// quit; `shutdown::hard_exit` covers that case. Download/polish threads
    /// own no whisper contexts and are left detached. Never panics; idempotent
    /// (every step tolerates having nothing to stop).
    ///
    /// Returns whether every context-owning thread exited within the grace.
    pub fn shutdown(&mut self, grace: Duration) -> bool {
        let deadline = Instant::now() + grace;

        // 1. Live transcription (M7): signal the decode thread and give an idle
        //    one the moment it needs to drop its context.
        if let Some(mut live) = self.live.take() {
            live.decoder.shutdown_and_wait(remaining(deadline));
        }

        // 2. Capture: the existing stop path (each stream, then its tap).
        if let Some(session) = self.session.take() {
            let _ = session.stop();
        }

        // 3. Playback: the existing transport teardown (`Stop` releases the
        //    engine; it owns no whisper context).
        self.playback_command(None, PlaybackCommand::Stop);

        // 4. The worker bridge: close the job channel, then wait for the thread.
        self.bridge.shutdown_and_wait(remaining(deadline))
    }
}

/// Time left before `deadline` (zero once it passed) — the quit teardown's
/// budget is shared across its steps, so the whole teardown stays within one
/// grace.
fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// One live step's outcome (see [`Runtime::live_step`]).
enum LiveStep {
    /// Keep the machinery running.
    Keep,
    /// Tear the machinery down; the reason is logged.
    Stop(String),
}

/// The live transcription machinery for one recording (M7).
struct LiveCapture {
    /// The pure scheduler session + provisional transcript.
    session: live::LiveSession,
    /// The decode thread (windows in, results out; its own model context).
    decoder: live::LiveDecoder,
    /// Per-source device-rate samples not yet flushed into the 16 kHz stream,
    /// in recorder order.
    pending: Vec<PendingSource>,
    /// The provisional revision the record view has already seen.
    published_revision: u64,
    /// The skipped-window count the record view has already seen.
    published_skipped: u64,
}

/// One source's pending device-rate audio (M7).
struct PendingSource {
    rate: u32,
    samples: Vec<f32>,
}

/// Seconds of new audio per source that trigger a live-stream flush.
const LIVE_FLUSH_SECS: usize = 1;

impl LiveCapture {
    /// Whether any source has accumulated enough new audio to flush (all
    /// sources are flushed together so the live mix stays time-aligned).
    fn should_flush(&self) -> bool {
        self.pending
            .iter()
            .any(|slot| slot.rate > 0 && slot.samples.len() >= slot.rate as usize * LIVE_FLUSH_SECS)
    }

    /// Mixes the pending per-source chunks into the one 16 kHz stream (the
    /// same [`mix`] engine the final pass uses — chunk-wise and without the
    /// session, i.e. no clock-drift correction: provisional only) and hands the
    /// scheduler's windows to the decoder.
    ///
    /// Per-source gain/mute come from the record view's rows, exactly like the
    /// final mix; a source with nothing pending contributes nothing (a few ms
    /// of sub-tick skew at most — the final mix stays the authoritative one).
    fn flush(&mut self, state: &AppState) {
        let (gains, mutes) = live_gains(state, self.pending.len());
        let tracks: Vec<mix::MixTrack<'_>> = self
            .pending
            .iter()
            .enumerate()
            .map(|(index, slot)| mix::MixTrack {
                label: format!("source {}", index + 1),
                rate: slot.rate,
                samples: &slot.samples,
                gain: gains[index],
                muted: mutes[index],
            })
            .collect();
        let mixed = match mix::mix(&tracks, None) {
            Ok(outcome) => outcome.samples,
            Err(err) => {
                tracing::warn!("live mix failed: {err}");
                for slot in &mut self.pending {
                    slot.samples.clear();
                }
                return;
            }
        };
        for slot in &mut self.pending {
            slot.samples.clear();
        }

        let skipped_before = self.session.skipped_windows();
        let busy = self.decoder.is_decoding();
        for window in self.session.push_audio(&mixed, busy) {
            if let Some(evicted) = self.decoder.submit(window) {
                tracing::warn!(
                    seq = evicted.seq,
                    dropped = self.decoder.dropped_windows(),
                    "live: dropped the oldest window (the decoder queue is full)"
                );
            }
        }
        let skipped = self.session.skipped_windows();
        if skipped != skipped_before {
            tracing::info!(
                skipped,
                "live: backlog — the previous decode was still running; window(s) skipped and merged into the next"
            );
        }
    }
}

/// The record view's per-source gain/mute for the live mix — unity when the
/// view holds none or a different source count (the same rule the final mix
/// uses at Stop).
fn live_gains(state: &AppState, count: usize) -> (Vec<f32>, Vec<bool>) {
    match &state.record {
        RecordState::Recording(recording) if recording.sources.len() == count => (
            recording.sources.iter().map(|source| source.gain).collect(),
            recording
                .sources
                .iter()
                .map(|source| source.muted)
                .collect(),
        ),
        _ => (vec![1.0; count], vec![false; count]),
    }
}

fn open_store() -> Result<Store, String> {
    paths::ensure_dirs().map_err(|err| err.to_string())?;
    let db = paths::db_path().map_err(|err| err.to_string())?;
    Store::open(&db).map_err(|err| err.to_string())
}

/// Maps the settings source list to capture inputs (M6).
///
/// Per-app entries resolve their stored keys (bundle ids / `pid:<pid>`) against
/// the live process list — a key with no running process is a clear error,
/// never a silent widening to full system audio.
fn resolve_capture_inputs(sources: &[RecordSource]) -> Result<Vec<capture::CaptureInput>, String> {
    let mut inputs = Vec::with_capacity(sources.len());
    for source in sources {
        match source {
            RecordSource::Default => inputs.push(capture::CaptureInput::Default),
            RecordSource::Device { name } => {
                inputs.push(capture::CaptureInput::Device(name.clone()));
            }
            RecordSource::SystemAudio => inputs.push(capture::CaptureInput::SystemAudio),
            RecordSource::AppAudio { apps } => {
                let processes = capture::list_audio_processes().map_err(|err| match err {
                    suramak_asr::AsrError::CaptureUnsupported => {
                        "per-app capture needs a macOS build with capture (CoreAudio process taps, \
                         macOS 14.2+)"
                            .to_string()
                    }
                    other => other.to_string(),
                })?;
                let pids = capture::resolve_app_processes(apps, &processes)?;
                inputs.push(capture::CaptureInput::AppAudio(pids));
            }
        }
    }
    Ok(inputs)
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

    /// Closes the job channel and waits at most `grace` for the bridge thread —
    /// which joins the worker and drops its whisper context cache (and with it
    /// the Metal residency sets) as it exits. The quit path's worker teardown
    /// (WORKORDER-HOTFIX-QUIT).
    ///
    /// Reuses the existing teardown: dropping the sender ends the bridge's
    /// `recv()` loop as soon as the current job — if any — is done; a job in
    /// flight is never interrupted. Returns whether the thread exited in time;
    /// on timeout it is left detached exactly like [`Drop`] leaves it.
    /// Idempotent.
    pub fn shutdown_and_wait(&mut self, grace: Duration) -> bool {
        self.jobs.take();
        let Some(_) = self.handle.as_ref() else {
            return true; // already shut down
        };
        let deadline = Instant::now() + grace;
        let mut finished = self.handle.as_ref().is_none_or(|h| h.is_finished());
        while !finished && Instant::now() < deadline {
            thread::sleep(crate::shutdown::TEARDOWN_POLL);
            finished = self.handle.as_ref().is_none_or(|h| h.is_finished());
        }
        if finished {
            // Reap it: the join is instant (the thread already returned) and it
            // proves the worker — and the context cache it owns — is gone.
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
        finished
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

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_core::settings::{EngineSetting, LangMode};

    fn job() -> Job {
        Job {
            id: 1,
            input: PathBuf::from("/nonexistent/suramak/input.wav"),
            model_path: PathBuf::from("/nonexistent/suramak/ggml-tiny.bin"),
            lang: LangMode::Auto,
            initial_prompt: None,
            engine: EngineSetting::Auto,
        }
    }

    /// The quit teardown's worker step (WORKORDER-HOTFIX-QUIT): an idle bridge
    /// exits within the grace, a second shutdown is a no-op, and the closed
    /// channel refuses further jobs — the teardown is idempotent and does not
    /// leave the bridge usable.
    #[test]
    fn bridge_shutdown_is_quick_idempotent_and_closes_the_channel() {
        let mut bridge = Bridge::spawn(egui::Context::default());
        assert!(
            bridge.shutdown_and_wait(crate::shutdown::TEARDOWN_GRACE),
            "an idle bridge must exit within the grace"
        );
        assert!(
            bridge.shutdown_and_wait(crate::shutdown::TEARDOWN_GRACE),
            "a second shutdown is a no-op that still reports the exit"
        );
        assert!(
            bridge.submit(job()).is_err(),
            "a shut-down bridge refuses further jobs"
        );
    }

    /// The teardown budget helper: steps share one grace, and an expired
    /// deadline means "no more waiting" (never a negative duration).
    #[test]
    fn the_remaining_budget_shrinks_and_clamps_at_zero() {
        let deadline = Instant::now() + Duration::from_millis(50);
        let left = remaining(deadline);
        assert!(left > Duration::ZERO && left <= Duration::from_millis(50));
        assert_eq!(remaining(Instant::now()), Duration::ZERO);
        assert_eq!(
            remaining(Instant::now() - Duration::from_secs(1)),
            Duration::ZERO,
            "an expired deadline never yields a negative wait"
        );
    }
}
