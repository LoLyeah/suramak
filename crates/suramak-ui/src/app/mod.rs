//! Application state machine: routes, job lifecycle, autosave debounce, export
//! selection and model resolution — **pure logic with no egui types**, so every
//! rule in here is unit-tested headless (see `tests` at the bottom of this file).
//!
//! The egui views live in the sibling modules (`views`, `home`, `transcribe`,
//! `editor`, `library`, `export`). They only read/mutate this state and push
//! [`Effect`]s; `crate::runtime` executes effects (worker submit, store reads and
//! writes, file writes) and feeds the results back as method calls on [`AppState`].

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use suramak_asr::capture;
use suramak_asr::worker::WorkerEvent;
use suramak_asr::EngineId;
use suramak_core::export as core_export;
use suramak_core::models as core_models;
use suramak_core::settings::EngineSetting;
use suramak_core::transcript::Transcript;
use suramak_playback::{
    resolve_playback_source, transcript_duration_ms, Command as PlaybackCommand, Playback,
    Status as PlaybackStatus,
};
use uuid::Uuid;

use self::model_manager::{ModelManager, ModelRow};
use self::polish::{PolishState, PolishUndo};
use self::settings_form::SettingsForm;

pub mod editor;
pub mod export;
pub mod home;
pub mod library;
pub mod model_manager;
pub mod models;
pub mod polish;
pub mod record;
pub mod settings;
pub mod settings_form;
pub mod transcribe;
pub mod views;

/// Autosave debounce: edits are persisted this long after the *last* keystroke.
pub const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(1000);

/// Model preference order used when the settings model is missing/`auto`
/// (documented in `docs/reports/M2.md`): shipped default first, then the
/// smaller/faster tiers, then the big accuracy tiers.
pub const AUTO_MODEL_PREFERENCE: &[&str] =
    &["turbo-q5-0", "small", "base", "tiny", "turbo-q8-0", "turbo"];

/// Where the main pane is pointing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Drop zone + import affordance + recent recordings.
    Home,
    /// A job is running (progress + engine line) or failed.
    Transcribe,
    /// Editable transcript + export.
    Editor,
    /// Live microphone/system-audio recording (M3).
    Record,
    /// Model manager: manifest table + download/delete/set-default (M3).
    Models,
    /// Settings: engine/model/language/keep_audio/audio source/polish (M3).
    Settings,
}

/// How many meter samples the rolling waveform painter keeps (oldest first).
pub const RECORD_WAVEFORM_BARS: usize = 96;

/// Recording state machine (M3) — **pure**; the runtime owns the cpal session.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordState {
    Idle,
    Recording(Recording),
    Failed { message: String },
}

/// A live recording session.
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    /// Auto title: `Recording <date>` (also the wav file's stem).
    pub title: String,
    pub started_at: Instant,
    /// Elapsed time, refreshed by [`AppState::tick`].
    pub elapsed: Duration,
    /// Live rolling RMS (0..=1) from the capture layer.
    pub rms: f32,
    /// Rolling waveform bars (oldest first), at most [`RECORD_WAVEFORM_BARS`].
    pub waveform: Vec<f32>,
    /// Input device name, reported by the runtime once capture starts.
    pub device: Option<String>,
    /// Set while the runtime is finalising the wav (Stop pressed once).
    pub stopping: bool,
}

/// Filesystem-safe file stem for a recording title:
/// `Recording 2026-10-13 14:05:33` → `Recording 2026-10-13 14-05-33`.
pub fn recording_file_stem(title: &str) -> String {
    title
        .chars()
        .map(|c| match c {
            ':' | '/' | '\\' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            other => other,
        })
        .collect()
}

/// `mm:ss.t` elapsed clock for the record view (`00:07.3`; hours fold into mm).
pub fn format_elapsed(elapsed: Duration) -> String {
    let total_secs = elapsed.as_secs();
    let tenths = elapsed.subsec_millis() / 100;
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    format!("{minutes:02}:{seconds:02}.{tenths}")
}

/// Lifecycle of the in-app transcription job.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    Idle,
    Running(RunningJob),
    Failed { input: PathBuf, message: String },
}

/// State of the running job.
#[derive(Debug, Clone, PartialEq)]
pub struct RunningJob {
    pub input: PathBuf,
    pub pct: u8,
    /// Manifest id of the model the runtime resolved for this job.
    pub model: Option<String>,
    /// `settings.asr.engine` as requested.
    pub requested_engine: Option<String>,
    /// Engine the plan selected (the engaged engine is reported at `Finished`).
    pub planned_engine: Option<String>,
    pub warnings: Vec<String>,
}

/// Home view state.
#[derive(Debug, Clone, Default)]
pub struct HomeState {
    /// Linux-style import affordance: type a path, press Enter.
    pub path_field: String,
    /// Set while a file is dragged over the window.
    pub drop_hover: bool,
    /// Last import error ("file not found", …).
    pub error: Option<String>,
}

/// Library (sidebar + home recents) state, bound to the store.
#[derive(Debug, Clone, Default)]
pub struct LibraryState {
    /// Transcript headers, newest first (as returned by `Store::list_transcripts`).
    pub items: Vec<Transcript>,
    /// Index into `items` of the selected row.
    pub selected: Option<usize>,
    pub error: Option<String>,
}

/// Open transcript in the editor.
#[derive(Debug, Clone)]
pub struct Editor {
    pub transcript: Transcript,
    /// Per-segment edit buffers (same order as `transcript.segments`).
    pub buffers: Vec<String>,
    pub dirty: bool,
    /// Last keystroke; the debounce clock starts here.
    pub last_edit: Option<Instant>,
    /// When the last successful autosave landed.
    pub last_saved: Option<Instant>,
    /// Last autosave failure (the buffer stays dirty and retries).
    pub save_error: Option<String>,
    /// Trash affordance armed in the editor header (M4.5): the next frame draws
    /// the in-place confirm step; cleared on cancel, on delete and on open.
    pub confirm_delete: bool,
}

impl Editor {
    /// Opens a transcript: buffers start as a copy of the stored segment text.
    pub fn open(transcript: Transcript) -> Self {
        let buffers = transcript.segments.iter().map(|s| s.text.clone()).collect();
        Self {
            transcript,
            buffers,
            dirty: false,
            last_edit: None,
            last_saved: None,
            save_error: None,
            confirm_delete: false,
        }
    }

    /// Records an edit (called by the view on every `TextEdit` change).
    ///
    /// The `TextEdit` widget writes into the buffer directly, so dirtiness is
    /// derived by comparing the buffers against the stored segment text — which
    /// also makes reverting an edit clear the dirty flag again.
    pub fn edit(&mut self, idx: u32, text: String, now: Instant) {
        let Some(buffer) = self.buffers.get_mut(idx as usize) else {
            return;
        };
        if *buffer != text {
            *buffer = text;
        }
        let changed = !self.pending_updates().is_empty();
        self.dirty = changed;
        if changed {
            self.last_edit = Some(now);
            self.save_error = None;
        }
    }

    /// Buffers that differ from the stored segment text (what a save must write).
    pub fn pending_updates(&self) -> Vec<(u32, String)> {
        self.transcript
            .segments
            .iter()
            .zip(self.buffers.iter())
            .filter(|(seg, buffer)| &seg.text != *buffer)
            .map(|(seg, buffer)| (seg.idx, buffer.clone()))
            .collect()
    }

    /// Copies the buffers into the transcript segments (after a successful save).
    pub fn apply_buffers(&mut self) {
        for (seg, buffer) in self.transcript.segments.iter_mut().zip(self.buffers.iter()) {
            seg.text = buffer.clone();
        }
    }

    /// A transcript with the current buffers applied (used by export).
    pub fn snapshot(&self) -> Transcript {
        let mut snapshot = self.transcript.clone();
        for (seg, buffer) in snapshot.segments.iter_mut().zip(self.buffers.iter()) {
            seg.text = buffer.clone();
        }
        snapshot
    }

    /// Human-readable save state for the "saved" indicator.
    pub fn save_state(&self) -> &'static str {
        if self.save_error.is_some() {
            "save failed — retrying"
        } else if self.dirty {
            "unsaved changes"
        } else if self.last_saved.is_some() {
            "saved"
        } else {
            ""
        }
    }
}

/// Export formats (core's export module is the renderer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Md,
    Txt,
    Json,
    Srt,
    Vtt,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 5] = [
        ExportFormat::Md,
        ExportFormat::Txt,
        ExportFormat::Json,
        ExportFormat::Srt,
        ExportFormat::Vtt,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Md => "md",
            ExportFormat::Txt => "txt",
            ExportFormat::Json => "json",
            ExportFormat::Srt => "srt",
            ExportFormat::Vtt => "vtt",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Md => "Markdown",
            ExportFormat::Txt => "Text",
            ExportFormat::Json => "JSON",
            ExportFormat::Srt => "SRT",
            ExportFormat::Vtt => "VTT",
        }
    }

    /// Renders a transcript through the core exporters.
    pub fn render(self, transcript: &Transcript) -> Result<String, String> {
        let rendered = match self {
            ExportFormat::Md => core_export::to_markdown(transcript),
            ExportFormat::Txt => core_export::to_txt(transcript),
            ExportFormat::Json => core_export::to_json(transcript),
            ExportFormat::Srt => core_export::to_srt(transcript),
            ExportFormat::Vtt => core_export::to_vtt(transcript),
        };
        rendered.map_err(|err| err.to_string())
    }
}

/// Export panel state.
#[derive(Debug, Clone)]
pub struct ExportState {
    pub format: ExportFormat,
    pub dest: String,
    /// True while `dest` follows the default pattern (so a format change renames it).
    pub follows_default: bool,
    /// Last result line (error or "wrote …").
    pub result: Option<Result<String, String>>,
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            format: ExportFormat::Md,
            dest: String::new(),
            follows_default: true,
            result: None,
        }
    }
}

/// Cross-view status message (engine line, save confirmations, errors).
#[derive(Debug, Clone)]
pub struct Status {
    pub text: String,
    pub error: bool,
}

/// Outcome of the most recent library insert (drives the status line and the
/// scripted smoke job).
#[derive(Debug, Clone)]
pub struct SaveOutcome {
    pub transcript: Transcript,
    pub error: Option<String>,
}

/// Everything the UI draws from, plus the effects it wants executed.
#[derive(Debug, Clone)]
pub struct AppState {
    pub route: Route,
    pub home: HomeState,
    pub job: Job,
    pub library: LibraryState,
    pub editor: Option<Editor>,
    pub export: ExportState,
    pub status: Option<Status>,
    /// Engine that actually ran for the most recent job (M1.5 outcome reporting).
    pub last_engine: Option<EngineId>,
    /// Set when the most recent finished job was written to the library.
    pub last_insert: Option<SaveOutcome>,
    /// Microphone/system-audio recording state (M3).
    pub record: RecordState,
    /// Model-manager state (M3): rows, download progress, last action.
    pub model_manager: ModelManager,
    /// Settings-form state (M3).
    pub settings_form: SettingsForm,
    /// Enumerated input device names (for the Audio source dropdown).
    pub audio_devices: Vec<String>,
    /// Enumeration problem (no capture compiled / no host support), shown as a note.
    pub audio_devices_error: Option<String>,
    /// A temp recording to delete once its transcript is saved (`keep_audio` off).
    pub pending_recording_cleanup: Option<PathBuf>,
    /// Whether this build has capture compiled in — the record control's enable
    /// (set from [`capture::CAPTURE_COMPILED`] at startup; a field so the state
    /// machine is testable headless on Linux builds).
    pub capture_available: bool,
    /// In-app polish lifecycle (M4) — see `app::polish`.
    pub polish: PolishState,
    /// One-step undo of an accepted polish (cleared when consumed / on a
    /// transcript switch).
    pub polish_undo: Option<PolishUndo>,
    /// Whether polish is configured (base URL + API key present, per M4) — the
    /// Polish action's enable. Seeded by the runtime at startup and refreshed
    /// after a settings save; a field so the state machine is testable headless.
    pub polish_configured: bool,
    /// In-app playback transport for the open transcript (M8): resolved source,
    /// state, playhead and duration. The runtime owns the cpal engine; this is
    /// the state the strip draws from and the machine that decides what the
    /// engine is told.
    pub playback: Playback,
}

/// Side effects the runtime executes after a frame (or an event).
#[derive(Debug, Clone)]
pub enum Effect {
    /// Submit the current import to the background worker.
    SubmitJob { input: PathBuf },
    /// Persist a finished transcript to the library (in-app jobs always save).
    SaveTranscript(Box<Transcript>),
    /// Persist edited segment text (autosave / flush).
    SaveEdits { updates: Vec<(u32, String)> },
    /// Load a transcript from the store into the editor.
    LoadTranscript(Box<Transcript>),
    /// Reload the library list from the store.
    RefreshLibrary,
    /// Render + write one export file.
    Export {
        transcript: Box<Transcript>,
        format: ExportFormat,
        dest: PathBuf,
    },
    /// Open the microphone / system-audio tap (runtime owns the cpal session).
    StartCapture,
    /// Close the session, write the recording wav (`keep_audio` decides where) and
    /// continue into the job flow.
    StopCapture { title: String },
    /// Delete a temp recording after its transcript was saved.
    DiscardRecording { path: PathBuf },
    /// Refresh the model-manager table from the manifest + filesystem.
    RefreshModels,
    /// Download a model in the background (progress polled by the runtime).
    DownloadModel { id: String },
    /// Delete a model (decoder + unshared encoder, M1.5 rule).
    DeleteModel { id: String },
    /// Delete a transcript from the library (its segments cascade in the store).
    DeleteTranscript { id: Uuid },
    /// Write `settings.model_id = id`.
    SetDefaultModel { id: String },
    /// Enumerate input devices for the Audio source dropdown.
    RefreshAudioDevices,
    /// Persist the settings form (toml; keychain on macOS/Windows).
    SaveSettings { form: Box<SettingsForm> },
    /// Run the polish call for this transcript on a background thread
    /// (core's client is blocking HTTP; never the ASR worker). M4.
    PolishTranscript { transcript: Box<Transcript> },
    /// Execute a playback transport command (M8). `source` is set on `Play` so
    /// the runtime opens (or replaces) the engine for exactly this file;
    /// pause/resume/seek/stop go to the engine it already owns.
    Playback {
        source: Option<PathBuf>,
        command: PlaybackCommand,
    },
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            route: Route::Home,
            home: HomeState::default(),
            job: Job::Idle,
            library: LibraryState::default(),
            editor: None,
            export: ExportState::default(),
            status: None,
            last_engine: None,
            last_insert: None,
            record: RecordState::Idle,
            model_manager: ModelManager::default(),
            settings_form: SettingsForm::from_settings(&suramak_core::settings::Settings::default()),
            audio_devices: Vec::new(),
            audio_devices_error: None,
            pending_recording_cleanup: None,
            capture_available: capture::CAPTURE_COMPILED,
            polish: PolishState::Idle,
            polish_undo: None,
            // Seeded by the runtime at startup (`Runtime::polish_configured`);
            // false until then so the action can never fire half-configured.
            polish_configured: false,
            playback: Playback::none(),
        }
    }

    /// Effects to run once at startup (library list from the store).
    pub fn initial_effects(&self) -> Vec<Effect> {
        vec![Effect::RefreshLibrary]
    }

    /// Whether the UI wants the periodic tick (job running / dirty edits /
    /// recording / a polish run polling for its result / the playhead moving).
    pub fn busy(&self) -> bool {
        matches!(self.job, Job::Running(_))
            || self.editor.as_ref().is_some_and(|e| e.dirty)
            || matches!(self.record, RecordState::Recording(_))
            || self.polish.is_running()
            || self.playback.is_playing()
    }

    pub fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some(Status {
            text: text.into(),
            error,
        });
    }

    pub fn set_drop_hover(&mut self, hover: bool) {
        self.home.drop_hover = hover;
    }

    // ---- recording (M3) -----------------------------------------------------

    /// Whether the chrome-strip record dot can start a session: capture compiled in,
    /// no job running, not already recording.
    pub fn can_record(&self) -> bool {
        self.capture_available
            && !matches!(self.job, Job::Running(_))
            && !matches!(self.record, RecordState::Recording(_))
    }

    /// Record dot pressed: opens the recording view and asks the runtime to start.
    pub fn start_recording(&mut self, now: Instant, date: &str) -> Vec<Effect> {
        if !self.can_record() {
            return Vec::new();
        }
        // M8: the capture session owns the audio — playback stops first.
        let mut effects = self.stop_playback();
        self.status = None;
        self.home.error = None;
        self.last_insert = None;
        self.record = RecordState::Recording(Recording {
            title: format!("Recording {date}"),
            started_at: now,
            elapsed: Duration::ZERO,
            rms: 0.0,
            waveform: Vec::new(),
            device: None,
            stopping: false,
        });
        self.route = Route::Record;
        effects.push(Effect::StartCapture);
        effects
    }

    /// Runtime: capture is live on this device.
    pub fn record_device(&mut self, device: String) {
        if let RecordState::Recording(recording) = &mut self.record {
            recording.device = Some(device);
        }
    }

    /// Meter sample (one per tick while recording).
    pub fn record_sample(&mut self, rms: f32) {
        if let RecordState::Recording(recording) = &mut self.record {
            let level = rms.clamp(0.0, 1.0);
            recording.rms = level;
            recording.waveform.push(level);
            let overflow = recording
                .waveform
                .len()
                .saturating_sub(RECORD_WAVEFORM_BARS);
            if overflow > 0 {
                recording.waveform.drain(..overflow);
            }
        }
    }

    /// Stop pressed in the record view (idempotent until the runtime answers).
    pub fn stop_recording(&mut self) -> Vec<Effect> {
        match &mut self.record {
            RecordState::Recording(recording) if !recording.stopping => {
                recording.stopping = true;
                vec![Effect::StopCapture {
                    title: recording.title.clone(),
                }]
            }
            _ => Vec::new(),
        }
    }

    /// Runtime: the recording wav is written; continue into the existing job flow.
    ///
    /// `keep == false` → the temp file is remembered and deleted once the transcript
    /// was saved (see [`AppState::on_transcript_saved`]); on a failed save it stays
    /// for a retry.
    pub fn on_recording_stopped(&mut self, wav: PathBuf, keep: bool) -> Vec<Effect> {
        self.record = RecordState::Idle;
        if !wav.is_file() {
            self.record_failed(format!("recording file missing: {}", wav.display()));
            return Vec::new();
        }
        self.pending_recording_cleanup = if keep { None } else { Some(wav.clone()) };
        self.begin_import(wav)
    }

    /// Capture could not start (or the wav could not be written): show the runtime's
    /// message in the record view.
    pub fn record_failed(&mut self, message: String) {
        self.record = RecordState::Failed {
            message: message.clone(),
        };
        self.set_status(message, true);
        self.route = Route::Record;
    }

    /// Back from the failed record view.
    pub fn leave_record(&mut self) -> Vec<Effect> {
        self.record = RecordState::Idle;
        self.route = Route::Home;
        Vec::new()
    }

    // ---- playback (M8) ------------------------------------------------------

    /// The waveform strip's play/pause control.
    pub fn toggle_playback(&mut self) -> Vec<Effect> {
        let Some(command) = self.playback.toggle() else {
            return Vec::new();
        };
        vec![self.playback_effect(command)]
    }

    /// Click-to-seek: the waveform strip or a segment's timestamp.
    pub fn seek_playback(&mut self, ms: u32) -> Vec<Effect> {
        let Some(command) = self.playback.seek(ms) else {
            return Vec::new();
        };
        vec![self.playback_effect(command)]
    }

    /// The runtime's per-frame poll of the playback engine: the playhead (and,
    /// once, the reason playback cannot start).
    pub fn on_playback_status(&mut self, status: PlaybackStatus) -> Vec<Effect> {
        if let Some(error) = status.error {
            self.playback.on_failure(error.message, error.retryable);
            return Vec::new();
        }
        self.playback
            .on_status(status.position_ms, status.duration_ms, status.finished);
        Vec::new()
    }

    /// Stops and resets the transport — leaving the editor, switching
    /// transcripts, deleting one, importing, starting a recording. Emits a
    /// `Stop` only when a session was live.
    fn stop_playback(&mut self) -> Vec<Effect> {
        let Some(command) = self.playback.stop() else {
            return Vec::new();
        };
        vec![self.playback_effect(command)]
    }

    /// The effect for a transport command: a `Play` carries the file the engine
    /// must open; the rest are addressed to the engine the runtime owns.
    fn playback_effect(&self, command: PlaybackCommand) -> Effect {
        let source = match command {
            PlaybackCommand::Play { .. } => self.playback.ready_path().map(Path::to_path_buf),
            _ => None,
        };
        Effect::Playback { source, command }
    }

    /// Rebuilds the transport for the open transcript (source resolution +
    /// duration estimate), or clears it when no editor is open.
    fn reset_playback(&mut self) {
        self.playback = match &self.editor {
            Some(editor) => Playback::new(
                resolve_playback_source(&editor.transcript),
                transcript_duration_ms(&editor.transcript),
            ),
            None => Playback::none(),
        };
    }

    // ---- model manager (M3) -------------------------------------------------

    /// Home → Models (the runtime rebuilds the table).
    pub fn open_models(&mut self) -> Vec<Effect> {
        self.route = Route::Models;
        vec![Effect::RefreshModels]
    }

    /// Runtime: fresh table rows.
    pub fn on_models_loaded(&mut self, rows: Vec<ModelRow>) {
        self.model_manager.rows = rows;
    }

    /// Download pressed for `id`.
    pub fn start_model_download(&mut self, id: &str) -> Vec<Effect> {
        if !self.model_manager.begin_download(id) {
            return Vec::new();
        }
        vec![Effect::DownloadModel { id: id.to_string() }]
    }

    /// Download progress polled by the runtime.
    pub fn on_download_progress(&mut self, bytes_done: u64, bytes_total: u64) {
        self.model_manager.on_progress(bytes_done, bytes_total);
    }

    /// Download finished (Ok/Err) → refresh the table (the row's state changed).
    pub fn on_download_done(&mut self, id: &str, result: Result<(), String>) -> Vec<Effect> {
        self.model_manager.on_download_done(id, result);
        vec![Effect::RefreshModels]
    }

    /// Delete pressed for `id`.
    pub fn delete_model(&mut self, id: &str) -> Vec<Effect> {
        vec![Effect::DeleteModel { id: id.to_string() }]
    }

    /// Delete executed → status + refresh.
    pub fn on_model_deleted(
        &mut self,
        id: &str,
        result: Result<Vec<String>, String>,
    ) -> Vec<Effect> {
        self.model_manager.status = Some(match result {
            Ok(removed) if removed.is_empty() => {
                Ok(format!("{id} is not installed — nothing to remove"))
            }
            Ok(removed) => Ok(format!("{id}: removed {} file(s)", removed.len())),
            Err(err) => Err(err),
        });
        vec![Effect::RefreshModels]
    }

    /// Set-default pressed for `id`.
    pub fn set_default_model(&mut self, id: &str) -> Vec<Effect> {
        vec![Effect::SetDefaultModel { id: id.to_string() }]
    }

    /// Set-default executed → status + refresh (the default marker follows).
    pub fn on_default_set(&mut self, id: &str, result: Result<(), String>) -> Vec<Effect> {
        self.model_manager.status = Some(match result {
            Ok(()) => Ok(format!("{id} is now the default model")),
            Err(err) => Err(err),
        });
        vec![Effect::RefreshModels]
    }

    // ---- settings (M3) ------------------------------------------------------

    /// Home → Settings (also re-enumerates the audio devices for the dropdown).
    pub fn open_settings(&mut self) -> Vec<Effect> {
        self.route = Route::Settings;
        vec![Effect::RefreshAudioDevices]
    }

    /// Runtime: enumerated input devices for the Audio source dropdown.
    pub fn on_audio_devices(&mut self, devices: Vec<String>, error: Option<String>) {
        self.audio_devices = devices;
        self.audio_devices_error = error;
    }

    /// Save pressed: the runtime applies the form (toml write + keychain).
    pub fn save_settings(&mut self) -> Vec<Effect> {
        vec![Effect::SaveSettings {
            form: Box::new(self.settings_form.clone()),
        }]
    }

    /// Save result from the runtime.
    pub fn on_settings_saved(&mut self, result: Result<String, String>) {
        self.settings_form.dirty = result.is_err();
        self.settings_form.status = Some(result);
    }

    /// Reset-to-defaults (form-only until Save).
    pub fn reset_settings_form(&mut self) {
        self.settings_form.reset();
    }

    // ---- import (M2) --------------------------------------------------------

    /// Import request from a drop, the dialog or the path field. Validates the
    /// path, switches to the transcribe route and asks the runtime to submit.
    pub fn begin_import(&mut self, input: PathBuf) -> Vec<Effect> {
        // A live recording owns the audio session: imports are ignored until it stops.
        if matches!(self.record, RecordState::Recording(_)) {
            return Vec::new();
        }
        if !input.is_file() {
            self.home.error = Some(format!("file not found: {}", input.display()));
            return Vec::new();
        }
        self.home.error = None;
        self.status = None;
        self.last_insert = None;
        // M8: the editor view is being left — playback stops and resets.
        let mut effects = self.stop_playback();
        self.route = Route::Transcribe;
        self.job = Job::Running(RunningJob {
            input: input.clone(),
            pct: 0,
            model: None,
            requested_engine: None,
            planned_engine: None,
            warnings: Vec::new(),
        });
        effects.push(Effect::SubmitJob { input });
        effects
    }

    /// Import from the home path field (Enter / Import button).
    pub fn import_from_field(&mut self) -> Vec<Effect> {
        let raw = self.home.path_field.trim().to_string();
        if raw.is_empty() {
            self.home.error = Some("enter a file path first".to_string());
            return Vec::new();
        }
        self.begin_import(PathBuf::from(raw))
    }

    /// The runtime reports the resolved model + engine plan for the running job.
    pub fn job_planned(
        &mut self,
        model: &str,
        requested: EngineSetting,
        planned: EngineId,
        warning: Option<String>,
    ) {
        if let Job::Running(run) = &mut self.job {
            run.model = Some(model.to_string());
            run.requested_engine = Some(requested.as_str().to_string());
            run.planned_engine = Some(planned.as_str().to_string());
            if let Some(warning) = warning {
                run.warnings.push(warning);
            }
        }
    }

    /// The runtime could not start the job (no model, worker gone, …).
    pub fn job_failed(&mut self, message: String) {
        let input = match &self.job {
            Job::Running(run) => run.input.clone(),
            Job::Failed { input, .. } => input.clone(),
            Job::Idle => PathBuf::new(),
        };
        self.set_status(message.clone(), true);
        self.job = Job::Failed { input, message };
    }

    /// Worker event → state transitions (+ effects).
    pub fn on_worker_event(&mut self, event: WorkerEvent) -> Vec<Effect> {
        match event {
            WorkerEvent::Progress { pct, .. } => {
                if let Job::Running(run) = &mut self.job {
                    run.pct = pct.min(100);
                }
                Vec::new()
            }
            // The segment list arrives just before `Finished`; the editor opens
            // from the full transcript, so this event carries no extra state.
            WorkerEvent::Segments { .. } => Vec::new(),
            WorkerEvent::Finished {
                transcript,
                engine_used,
                warnings,
                ..
            } => {
                if !matches!(self.job, Job::Running(_)) {
                    // Stray event (no job in flight) — nothing to attach it to.
                    return Vec::new();
                }
                self.job = Job::Idle;
                self.last_engine = Some(engine_used);
                let mut status = format!("engine: {}", engine_used.as_str());
                if !warnings.is_empty() {
                    status.push_str(&format!(" — {}", warnings.join("; ")));
                }
                self.set_status(status, false);
                self.open_editor(transcript.clone());
                vec![Effect::SaveTranscript(Box::new(transcript))]
            }
            WorkerEvent::Error { message, .. } => {
                self.job_failed(message);
                Vec::new()
            }
        }
    }

    /// Result of the library insert for a finished job.
    pub fn on_transcript_saved(
        &mut self,
        transcript: Transcript,
        result: Result<(), String>,
    ) -> Vec<Effect> {
        match result {
            Ok(()) => {
                // Keep the engaged-engine line from `Finished` visible next to
                // the save confirmation (M1.5 outcome reporting).
                let engine_line = self
                    .status
                    .as_ref()
                    .filter(|status| !status.error && status.text.starts_with("engine:"))
                    .map(|status| status.text.clone());
                let title = transcript.title.trim();
                let saved_line = if title.is_empty() {
                    "saved to library".to_string()
                } else {
                    format!("saved to library: {title}")
                };
                let text = match engine_line {
                    Some(line) => format!("{line} — {saved_line}"),
                    None => saved_line,
                };
                self.set_status(text, false);
                self.last_insert = Some(SaveOutcome {
                    transcript: transcript.clone(),
                    error: None,
                });
                let mut effects = vec![Effect::RefreshLibrary];
                // keep_audio off: the temp recording goes now that its transcript
                // is safely in the library (a failed save keeps it for a retry).
                if let Some(path) = self.pending_recording_cleanup.clone() {
                    if transcript.audio_path.as_deref() == Some(path.as_path()) {
                        self.pending_recording_cleanup = None;
                        effects.push(Effect::DiscardRecording { path });
                    }
                }
                effects
            }
            Err(err) => {
                self.set_status(format!("could not save to library: {err}"), true);
                self.last_insert = Some(SaveOutcome {
                    transcript,
                    error: Some(err),
                });
                Vec::new()
            }
        }
    }

    /// Library list refreshed from the store.
    pub fn on_library_loaded(&mut self, items: Vec<Transcript>, error: Option<String>) {
        let mut selected = self
            .library
            .selected
            .and_then(|index| self.library.items.get(index))
            .map(|t| t.id);
        // A fresh list (e.g. right after a save) keeps the open transcript marked.
        if selected.is_none() {
            selected = self.editor.as_ref().map(|editor| editor.transcript.id);
        }
        self.library.items = items;
        self.library.error = error;
        self.library.selected =
            selected.and_then(|id| self.library.items.iter().position(|t| t.id == id));
    }

    /// Library row clicked: load into the editor (or return to it when it is
    /// already open, keeping the edit buffers).
    pub fn select_library(&mut self, index: usize) -> Vec<Effect> {
        // A live recording owns the route (its Stop button is the only exit).
        if matches!(self.record, RecordState::Recording(_)) {
            return Vec::new();
        }
        let Some(item) = self.library.items.get(index).cloned() else {
            return Vec::new();
        };
        self.library.selected = Some(index);
        if self
            .editor
            .as_ref()
            .is_some_and(|e| e.transcript.id == item.id)
        {
            self.route = Route::Editor;
            return Vec::new();
        }
        // M8: switching transcripts stops the old one's playback.
        let mut effects = self.stop_playback();
        effects.extend(self.flush_edits());
        effects.push(Effect::LoadTranscript(Box::new(item)));
        effects
    }

    /// Transcript loaded from the store.
    pub fn on_transcript_loaded(&mut self, transcript: Transcript) -> Vec<Effect> {
        self.open_editor(transcript);
        Vec::new()
    }

    /// Trash confirmed in the editor header (M4.5): delete the open transcript.
    pub fn delete_open_transcript(&mut self) -> Vec<Effect> {
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        vec![Effect::DeleteTranscript {
            id: editor.transcript.id,
        }]
    }

    /// Delete executed → status + library refresh; the editor closes and the
    /// route goes Home when the transcript it was showing is the one that was
    /// deleted. Errors surface through the status line and change nothing.
    pub fn on_transcript_deleted(&mut self, id: Uuid, result: Result<(), String>) -> Vec<Effect> {
        // The title for the status line: the open editor first, else the
        // library row (both are read before any state changes).
        let title = self
            .editor
            .as_ref()
            .filter(|editor| editor.transcript.id == id)
            .map(|editor| editor.transcript.title.clone())
            .or_else(|| {
                self.library
                    .items
                    .iter()
                    .find(|item| item.id == id)
                    .map(|item| item.title.clone())
            })
            .unwrap_or_else(|| id.to_string());
        match result {
            Ok(()) => {
                let mut effects = Vec::new();
                if self
                    .editor
                    .as_ref()
                    .is_some_and(|editor| editor.transcript.id == id)
                {
                    // M8: the open transcript is going away — stop and reset.
                    effects.extend(self.stop_playback());
                    self.playback = Playback::none();
                    self.editor = None;
                    // A pending polish review / undo belongs to the transcript
                    // that just went away (same rule as `open_editor`).
                    self.polish = PolishState::Idle;
                    self.polish_undo = None;
                    self.route = Route::Home;
                }
                self.set_status(format!("deleted {title}"), false);
                effects.push(Effect::RefreshLibrary);
                effects
            }
            Err(err) => {
                self.set_status(format!("could not delete {title}: {err}"), true);
                Vec::new()
            }
        }
    }

    /// Back to the home route (pending edits are flushed first; M8: playback
    /// stops and the playhead resets).
    pub fn back_home(&mut self) -> Vec<Effect> {
        self.route = Route::Home;
        let mut effects = self.stop_playback();
        effects.extend(self.flush_edits());
        effects
    }

    /// The periodic tick: recording clock + autosave debounce.
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        if let RecordState::Recording(recording) = &mut self.record {
            recording.elapsed = now.saturating_duration_since(recording.started_at);
        }
        let Some(editor) = &mut self.editor else {
            return Vec::new();
        };
        if !editor.dirty {
            return Vec::new();
        }
        let Some(last_edit) = editor.last_edit else {
            return Vec::new();
        };
        if now.duration_since(last_edit) < AUTOSAVE_DEBOUNCE {
            return Vec::new();
        }
        let updates = editor.pending_updates();
        if updates.is_empty() {
            editor.dirty = false;
            return Vec::new();
        }
        // Throttle retries: the debounce clock restarts even on failure.
        editor.last_edit = Some(now);
        vec![Effect::SaveEdits { updates }]
    }

    /// Result of an autosave/flush.
    pub fn on_edits_saved(&mut self, at: Instant, result: Result<(), String>) -> Vec<Effect> {
        let Some(editor) = &mut self.editor else {
            return Vec::new();
        };
        match result {
            Ok(()) => {
                editor.apply_buffers();
                editor.dirty = false;
                editor.last_saved = Some(at);
                editor.save_error = None;
            }
            Err(err) => {
                editor.save_error = Some(err);
                editor.last_edit = Some(at);
            }
        }
        Vec::new()
    }

    /// Format picked in the export panel; the default destination follows.
    pub fn set_export_format(&mut self, format: ExportFormat) {
        if self.export.follows_default {
            if let Some(editor) = &self.editor {
                self.export.dest = default_dest(&editor.transcript.title, format);
            }
        }
        self.export.format = format;
        self.export.result = None;
    }

    /// Destination typed/edited by the user (or set by the save dialog).
    pub fn set_export_dest(&mut self, dest: String) {
        self.export.dest = dest;
        self.export.follows_default = false;
        self.export.result = None;
    }

    /// Export button: render + write (pending edits are flushed alongside).
    pub fn start_export(&mut self) -> Vec<Effect> {
        let Some(editor) = &self.editor else {
            self.export.result = Some(Err("no transcript open".to_string()));
            return Vec::new();
        };
        let dest = self.export.dest.trim().to_string();
        if dest.is_empty() {
            self.export.result = Some(Err("choose a destination first".to_string()));
            return Vec::new();
        }
        let transcript = editor.snapshot();
        self.export.result = None;
        let mut effects = self.flush_edits();
        effects.push(Effect::Export {
            transcript: Box::new(transcript),
            format: self.export.format,
            dest: PathBuf::from(dest),
        });
        effects
    }

    /// Result of an export write.
    pub fn on_export_done(&mut self, dest: &Path, result: Result<(), String>) {
        self.export.result = Some(match result {
            Ok(()) => Ok(format!("wrote {}", dest.display())),
            Err(err) => Err(format!("export failed: {err}")),
        });
    }

    /// Flushes dirty buffers immediately (leaving the editor, switching
    /// transcripts, exporting).
    fn flush_edits(&mut self) -> Vec<Effect> {
        let Some(editor) = &mut self.editor else {
            return Vec::new();
        };
        if !editor.dirty {
            return Vec::new();
        }
        let updates = editor.pending_updates();
        if updates.is_empty() {
            editor.dirty = false;
            return Vec::new();
        }
        editor.last_edit = Some(Instant::now());
        vec![Effect::SaveEdits { updates }]
    }

    /// Opens a transcript in the editor and switches the route.
    fn open_editor(&mut self, transcript: Transcript) {
        let id = transcript.id;
        self.export.dest = default_dest(&transcript.title, self.export.format);
        self.export.follows_default = true;
        self.export.result = None;
        self.library.selected = self.library.items.iter().position(|t| t.id == id);
        self.editor = Some(Editor::open(transcript));
        // A pending polish review (or one-step undo) belongs to the transcript
        // that was open — it applies by segment index, so a different
        // transcript must never inherit it (M4).
        self.polish = PolishState::Idle;
        self.polish_undo = None;
        self.route = Route::Editor;
        // M8: the transport is rebuilt for this transcript's source (a previous
        // transcript's playback never leaks across).
        self.reset_playback();
    }
}

/// Resolves the model file the app transcribes with.
///
/// Order (documented in the M2 report): an explicit path from settings when it
/// exists → the settings manifest id when its file is installed → "auto": the
/// first installed model in [`AUTO_MODEL_PREFERENCE`] order.
pub fn resolve_model_path(settings_model_id: &str, models_dir: &Path) -> Result<PathBuf, String> {
    let settings_model_id = settings_model_id.trim();
    if settings_model_id != "auto" {
        let as_path = Path::new(settings_model_id);
        if as_path.is_file() {
            return Ok(as_path.to_path_buf());
        }
    }
    let installed = |id: &str| {
        core_models::find_model(id)
            .map(|info| models_dir.join(info.hf_file))
            .filter(|path| path.is_file())
    };
    if !settings_model_id.is_empty() && settings_model_id != "auto" {
        if let Some(path) = installed(settings_model_id) {
            return Ok(path);
        }
    }
    for id in AUTO_MODEL_PREFERENCE {
        if let Some(path) = installed(id) {
            return Ok(path);
        }
    }
    Err(format!(
        "no model installed in {} — run `suramak-cli models download <id>`",
        models_dir.display()
    ))
}

/// Default export destination for a transcript title and format
/// (`en.wav` + `md` → `$HOME/en.md`).
pub fn default_dest(title: &str, format: ExportFormat) -> String {
    let stem = Path::new(title)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("transcript");
    let dir = home_dir().unwrap_or_else(|| PathBuf::from("."));
    dir.join(format!("{stem}.{}", format.extension()))
        .to_string_lossy()
        .into_owned()
}

/// The user's home directory without pulling in a new dependency.
fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let var = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let var = std::env::var_os("HOME");
    var.map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_core::transcript::{Segment, Source};

    fn segment(idx: u32, text: &str) -> Segment {
        Segment {
            idx,
            start_ms: i64::from(idx) * 1000,
            end_ms: i64::from(idx) * 1000 + 900,
            text: text.to_string(),
        }
    }

    fn transcript(title: &str) -> Transcript {
        Transcript::new(
            title.to_string(),
            Source::File {
                name: title.to_string(),
            },
            "tiny".to_string(),
            Some("en".to_string()),
            None,
            vec![
                segment(0, "the quick brown fox"),
                segment(1, "suramak keeps it local"),
            ],
        )
    }

    fn temp_wav(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("suramak-ui-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!("{tag}.wav"));
        std::fs::write(&path, b"RIFF....").expect("temp wav");
        path
    }

    fn finished_event(transcript: Transcript) -> WorkerEvent {
        WorkerEvent::Finished {
            job_id: 1,
            transcript,
            engine_used: EngineId::Cpu,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn job_lifecycle_happy_path_reaches_the_editor_and_library() {
        let input = temp_wav("happy");
        let mut state = AppState::new();

        let effects = state.begin_import(input.clone());
        assert_eq!(effects.len(), 1);
        assert!(matches!(effects[0], Effect::SubmitJob { .. }));
        assert_eq!(state.route, Route::Transcribe);
        assert!(state.busy(), "a running job must keep the tick alive");

        state.job_planned("tiny", EngineSetting::Auto, EngineId::Cpu, None);
        let effects = state.on_worker_event(WorkerEvent::Progress { job_id: 1, pct: 42 });
        assert!(effects.is_empty());
        match &state.job {
            Job::Running(run) => {
                assert_eq!(run.pct, 42);
                assert_eq!(run.model.as_deref(), Some("tiny"));
                assert_eq!(run.planned_engine.as_deref(), Some("cpu"));
            }
            other => panic!("expected a running job, got {other:?}"),
        }

        let done = transcript("happy.wav");
        let id = done.id;
        let effects = state.on_worker_event(finished_event(done.clone()));
        assert!(matches!(effects.as_slice(), [Effect::SaveTranscript(_)]));
        assert_eq!(state.route, Route::Editor);
        assert!(!state.busy(), "a finished job must release the tick");
        assert_eq!(state.last_engine, Some(EngineId::Cpu));
        let editor = state.editor.as_ref().expect("editor open");
        assert_eq!(editor.buffers.len(), 2);
        assert!(!editor.dirty);

        // The library insert lands and refreshes the sidebar.
        let effects = state.on_transcript_saved(done.clone(), Ok(()));
        assert!(matches!(effects.as_slice(), [Effect::RefreshLibrary]));
        assert_eq!(state.last_insert.as_ref().unwrap().error, None);
        let status = state.status.as_ref().unwrap();
        assert!(
            status.text.starts_with("engine: cpu — saved to library:"),
            "unexpected status: {}",
            status.text
        );
        assert!(
            status.text.contains("happy.wav") && !status.text.contains(&id.to_string()),
            "the saved title, not the raw id: {}",
            status.text
        );

        state.on_library_loaded(vec![done], None);
        assert_eq!(state.library.items.len(), 1);
        assert_eq!(
            state.library.selected,
            Some(0),
            "the open transcript is marked"
        );
        // The loaded transcript has the same id the save reported.
        assert_eq!(state.editor.as_ref().unwrap().transcript.id, id);
    }

    #[test]
    fn a_second_job_runs_after_the_first_completed() {
        let mut state = AppState::new();
        state.begin_import(temp_wav("first"));
        state.on_worker_event(finished_event(transcript("first.wav")));
        state.on_transcript_saved(transcript("first.wav"), Ok(()));
        assert!(state.last_insert.is_some());

        // Import again from the editor route.
        let effects = state.begin_import(temp_wav("second"));
        assert!(matches!(effects.as_slice(), [Effect::SubmitJob { .. }]));
        assert_eq!(state.route, Route::Transcribe);
        assert!(matches!(state.job, Job::Running(_)));
        assert!(state.busy());
        assert!(
            state.last_insert.is_none(),
            "a stale save outcome must not leak into the next job"
        );
    }

    #[test]
    fn job_failure_is_reported_and_recoverable() {
        let input = temp_wav("failure");
        let mut state = AppState::new();
        state.begin_import(input);
        state.job_failed("model file not found".to_string());
        assert!(matches!(state.job, Job::Failed { .. }));
        assert!(state.status.as_ref().unwrap().error);
        assert!(!state.busy());
        assert_eq!(state.route, Route::Transcribe, "the failure stays visible");

        let effects = state.back_home();
        assert!(effects.is_empty());
        assert_eq!(state.route, Route::Home);
    }

    #[test]
    fn worker_error_event_fails_the_job() {
        let mut state = AppState::new();
        state.begin_import(temp_wav("worker-error"));
        state.on_worker_event(WorkerEvent::Error {
            job_id: 1,
            message: "decode failed".to_string(),
        });
        match &state.job {
            Job::Failed { message, .. } => assert_eq!(message, "decode failed"),
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[test]
    fn import_rejects_a_missing_file() {
        let mut state = AppState::new();
        let effects = state.begin_import(PathBuf::from("/definitely/not/here.wav"));
        assert!(effects.is_empty());
        assert_eq!(state.route, Route::Home);
        assert!(state.home.error.as_deref().unwrap().contains("not found"));
    }

    #[test]
    fn import_from_field_validates_and_trims() {
        let mut state = AppState::new();
        assert!(state.import_from_field().is_empty());
        assert_eq!(state.home.error.as_deref(), Some("enter a file path first"));

        let path = temp_wav("field");
        state.home.path_field = format!("  {}  ", path.display());
        let effects = state.import_from_field();
        assert!(matches!(effects.as_slice(), [Effect::SubmitJob { .. }]));
        assert_eq!(state.home.error, None);
    }

    #[test]
    fn autosave_debounce_waits_for_the_quiet_period() {
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript("debounce.wav"));
        let t0 = Instant::now();

        state
            .editor
            .as_mut()
            .unwrap()
            .edit(0, "edited".to_string(), t0);
        assert!(state.busy());
        assert!(
            state.tick(t0 + Duration::from_millis(500)).is_empty(),
            "too early"
        );
        let effects = state.tick(t0 + AUTOSAVE_DEBOUNCE);
        match effects.as_slice() {
            [Effect::SaveEdits { updates }] => {
                assert_eq!(updates, &[(0, "edited".to_string())]);
            }
            other => panic!("expected one save, got {other:?}"),
        }

        // A second edit after the save marks dirty again and debounces anew.
        let t1 = t0 + AUTOSAVE_DEBOUNCE;
        state
            .editor
            .as_mut()
            .unwrap()
            .edit(1, "second".to_string(), t1);
        assert!(state.tick(t1 + Duration::from_millis(999)).is_empty());
        assert_eq!(state.tick(t1 + AUTOSAVE_DEBOUNCE).len(), 1);
    }

    #[test]
    fn autosave_coalesces_edits_and_clears_dirty_on_success() {
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript("coalesce.wav"));
        let t0 = Instant::now();
        {
            let editor = state.editor.as_mut().unwrap();
            editor.edit(0, "one".to_string(), t0);
            editor.edit(1, "two".to_string(), t0 + Duration::from_millis(300));
            editor.edit(0, "one-b".to_string(), t0 + Duration::from_millis(600));
        }
        // The debounce restarts at the last keystroke.
        assert!(state.tick(t0 + Duration::from_millis(1000)).is_empty());
        let effects = state.tick(t0 + Duration::from_millis(1600));
        match effects.as_slice() {
            [Effect::SaveEdits { updates }] => {
                assert_eq!(updates, &[(0, "one-b".to_string()), (1, "two".to_string())]);
            }
            other => panic!("expected one coalesced save, got {other:?}"),
        }

        let at = t0 + Duration::from_millis(1600);
        assert!(state.on_edits_saved(at, Ok(())).is_empty());
        let editor = state.editor.as_ref().unwrap();
        assert!(!editor.dirty);
        assert_eq!(editor.last_saved, Some(at));
        assert_eq!(editor.transcript.segments[0].text, "one-b");
        assert_eq!(editor.save_state(), "saved");
        assert!(!state.busy());
    }

    #[test]
    fn autosave_retries_after_a_failed_save() {
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript("retry.wav"));
        let t0 = Instant::now();
        state
            .editor
            .as_mut()
            .unwrap()
            .edit(0, "kept".to_string(), t0);
        let save_at = t0 + AUTOSAVE_DEBOUNCE;
        assert_eq!(state.tick(save_at).len(), 1);

        state.on_edits_saved(save_at, Err("database is locked".to_string()));
        let editor = state.editor.as_ref().unwrap();
        assert!(editor.dirty, "a failed save must keep the buffer dirty");
        assert_eq!(editor.save_state(), "save failed — retrying");
        // …and the retry waits for a fresh debounce window.
        assert!(state.tick(save_at + Duration::from_millis(500)).is_empty());
        assert_eq!(state.tick(save_at + AUTOSAVE_DEBOUNCE).len(), 1);
    }

    #[test]
    fn reverting_an_edit_clears_the_dirty_flag() {
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript("revert.wav"));
        let t0 = Instant::now();
        state
            .editor
            .as_mut()
            .unwrap()
            .edit(0, "changed".to_string(), t0);
        assert!(state.busy());
        // The TextEdit wrote the original text back: no save is needed.
        state
            .editor
            .as_mut()
            .unwrap()
            .edit(0, "the quick brown fox".to_string(), t0);
        assert!(!state.editor.as_ref().unwrap().dirty);
        assert!(state.tick(t0 + AUTOSAVE_DEBOUNCE).is_empty());
    }

    #[test]
    fn leaving_the_editor_flushes_pending_edits() {
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript("leave.wav"));
        let t0 = Instant::now();
        state
            .editor
            .as_mut()
            .unwrap()
            .edit(0, "pending".to_string(), t0);

        let effects = state.back_home();
        assert!(matches!(effects.as_slice(), [Effect::SaveEdits { .. }]));
        assert_eq!(state.route, Route::Home);
    }

    #[test]
    fn selecting_another_recording_flushes_then_loads() {
        let mut state = AppState::new();
        let open = transcript("open.wav");
        let other = transcript("other.wav");
        state.on_transcript_loaded(open.clone());
        state.on_library_loaded(vec![open, other.clone()], None);
        let t0 = Instant::now();
        state
            .editor
            .as_mut()
            .unwrap()
            .edit(0, "unsaved".to_string(), t0);

        let effects = state.select_library(1);
        assert_eq!(effects.len(), 2);
        assert!(
            matches!(effects[0], Effect::SaveEdits { .. }),
            "flush first"
        );
        assert!(matches!(effects[1], Effect::LoadTranscript(_)), "then load");

        state.on_transcript_loaded(other.clone());
        assert_eq!(state.route, Route::Editor);
        assert_eq!(state.editor.as_ref().unwrap().transcript.id, other.id);
        assert_eq!(state.library.selected, Some(1));
    }

    #[test]
    fn reselecting_the_open_recording_keeps_the_edit_buffers() {
        let mut state = AppState::new();
        let open = transcript("same.wav");
        state.on_transcript_loaded(open.clone());
        state.on_library_loaded(vec![open], None);
        let t0 = Instant::now();
        state
            .editor
            .as_mut()
            .unwrap()
            .edit(0, "in progress".to_string(), t0);
        state.route = Route::Home;

        let effects = state.select_library(0);
        assert!(
            effects.is_empty(),
            "no reload for the already-open transcript"
        );
        assert_eq!(state.route, Route::Editor);
        assert_eq!(state.editor.as_ref().unwrap().buffers[0], "in progress");
    }

    #[test]
    fn export_default_destination_follows_the_format() {
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript("en.wav"));
        assert!(state.export.follows_default);
        assert!(state.export.dest.ends_with("en.md"));

        state.set_export_format(ExportFormat::Vtt);
        assert!(state.export.dest.ends_with("en.vtt"));

        // A hand-typed destination is sticky across format changes.
        state.set_export_dest("/tmp/custom.srt".to_string());
        state.set_export_format(ExportFormat::Json);
        assert_eq!(state.export.dest, "/tmp/custom.srt");
        assert!(!state.export.follows_default);
    }

    #[test]
    fn export_requires_a_destination() {
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript("empty.wav"));
        state.export.dest = "   ".to_string();
        let effects = state.start_export();
        assert!(effects.is_empty());
        assert!(state.export.result.as_ref().unwrap().is_err());
    }

    #[test]
    fn export_effect_carries_format_destination_and_edited_text() {
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript("carry.wav"));
        let t0 = Instant::now();
        state
            .editor
            .as_mut()
            .unwrap()
            .edit(0, "edited text".to_string(), t0);
        state.set_export_format(ExportFormat::Srt);
        state.set_export_dest("/tmp/carry.srt".to_string());

        let effects = state.start_export();
        assert_eq!(effects.len(), 2, "flush + export");
        assert!(matches!(effects[0], Effect::SaveEdits { .. }));
        match &effects[1] {
            Effect::Export {
                transcript,
                format,
                dest,
            } => {
                assert_eq!(*format, ExportFormat::Srt);
                assert_eq!(dest, &PathBuf::from("/tmp/carry.srt"));
                assert_eq!(transcript.segments[0].text, "edited text");
            }
            other => panic!("expected an export effect, got {other:?}"),
        }

        state.on_export_done(Path::new("/tmp/carry.srt"), Ok(()));
        assert!(state.export.result.as_ref().unwrap().is_ok());
        state.on_export_done(Path::new("/tmp/carry.srt"), Err("disk full".to_string()));
        assert!(state.export.result.as_ref().unwrap().is_err());
    }

    #[test]
    fn model_resolution_prefers_an_installed_settings_model() {
        let dir = temp_wav("models-dir");
        let dir = dir
            .parent()
            .unwrap()
            .join(format!("models-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ggml-tiny.bin"), b"x").unwrap();
        std::fs::write(dir.join("ggml-small.bin"), b"x").unwrap();

        let path = resolve_model_path("small", &dir).expect("settings model installed");
        assert_eq!(path.file_name().unwrap(), "ggml-small.bin");

        // "auto" (or an uninstalled settings id) falls back to the preference order.
        let path = resolve_model_path("auto", &dir).expect("auto");
        assert_eq!(
            path.file_name().unwrap(),
            "ggml-small.bin",
            "small before tiny"
        );
        let path = resolve_model_path("turbo-q5-0", &dir).expect("fallback");
        assert_eq!(path.file_name().unwrap(), "ggml-small.bin");

        // A path in settings is honoured when it exists.
        let direct = resolve_model_path(dir.join("ggml-tiny.bin").to_str().unwrap(), &dir).unwrap();
        assert_eq!(direct.file_name().unwrap(), "ggml-tiny.bin");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn model_resolution_reports_when_nothing_is_installed() {
        let dir =
            std::env::temp_dir().join(format!("suramak-ui-empty-models-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = resolve_model_path("turbo-q5-0", &dir).unwrap_err();
        assert!(err.contains("no model installed"), "unexpected: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- M3: recording state machine ----------------------------------------

    /// An AppState with capture "compiled in" (the Linux default build has it off;
    /// the state machine is exercised here regardless — that is the point).
    fn recording_state() -> AppState {
        let mut state = AppState::new();
        state.capture_available = true;
        state
    }

    fn transcript_with_audio(title: &str, path: &Path) -> Transcript {
        let mut t = transcript(title);
        t.audio_path = Some(path.to_path_buf());
        t
    }

    #[test]
    fn recording_start_stop_runs_the_existing_job_flow() {
        let mut state = recording_state();
        let t0 = Instant::now();
        let effects = state.start_recording(t0, "2026-10-13 14:05:33");
        assert!(matches!(effects.as_slice(), [Effect::StartCapture]));
        assert_eq!(state.route, Route::Record);
        assert!(state.busy(), "recording keeps the tick alive");
        match &state.record {
            RecordState::Recording(rec) => {
                assert_eq!(rec.title, "Recording 2026-10-13 14:05:33");
                assert_eq!(rec.elapsed, Duration::ZERO);
                assert!(rec.device.is_none());
                assert!(!rec.stopping);
            }
            other => panic!("expected a recording, got {other:?}"),
        }

        // Meter samples + the timer.
        state.record_device("MacBook Microphone".to_string());
        state.record_sample(0.4);
        state.record_sample(0.6);
        state.tick(t0 + Duration::from_millis(7_300));
        match &state.record {
            RecordState::Recording(rec) => {
                assert_eq!(rec.device.as_deref(), Some("MacBook Microphone"));
                assert_eq!(rec.waveform, vec![0.4, 0.6]);
                assert!((rec.rms - 0.6).abs() < 1e-6);
                assert_eq!(rec.elapsed, Duration::from_millis(7_300));
                assert_eq!(format_elapsed(rec.elapsed), "00:07.3");
            }
            other => panic!("expected a recording, got {other:?}"),
        }

        // Stop is idempotent until the runtime answers.
        let effects = state.stop_recording();
        let title = match effects.as_slice() {
            [Effect::StopCapture { title }] => title.clone(),
            other => panic!("expected a stop effect, got {other:?}"),
        };
        assert_eq!(title, "Recording 2026-10-13 14:05:33");
        assert!(state.stop_recording().is_empty(), "no double stop");
        assert!(matches!(state.record, RecordState::Recording(_)));

        // The runtime wrote the wav (temp dir, keep_audio off) → job flow.
        let wav = temp_wav("recorded");
        let effects = state.on_recording_stopped(wav.clone(), false);
        assert!(matches!(effects.as_slice(), [Effect::SubmitJob { .. }]));
        assert_eq!(state.route, Route::Transcribe);
        assert!(matches!(state.job, Job::Running(_)));
        assert_eq!(
            state.pending_recording_cleanup.as_deref(),
            Some(wav.as_path())
        );
        assert!(matches!(state.record, RecordState::Idle));
        // The file stem is the title (the transcript title comes from it).
        assert_eq!(
            recording_file_stem("Recording 2026-10-13 14:05:33"),
            "Recording 2026-10-13 14-05-33"
        );
        assert_eq!(format_elapsed(Duration::from_millis(62_040)), "01:02.0");
    }

    #[test]
    fn recording_failure_is_shown_and_recoverable() {
        let mut state = recording_state();
        state.start_recording(Instant::now(), "2026-10-13 14:05:33");
        state.record_failed("no default input device".to_string());
        match &state.record {
            RecordState::Failed { message } => assert_eq!(message, "no default input device"),
            other => panic!("expected a failure, got {other:?}"),
        }
        assert_eq!(state.route, Route::Record, "the error stays visible");
        assert!(state.status.as_ref().unwrap().error);
        assert!(!state.busy(), "a failed recording releases the tick");

        let effects = state.leave_record();
        assert!(effects.is_empty());
        assert!(matches!(state.record, RecordState::Idle));
        assert_eq!(state.route, Route::Home);
    }

    #[test]
    fn recording_is_disabled_while_a_job_runs_and_without_capture() {
        let mut state = recording_state();
        state.begin_import(temp_wav("busy"));
        assert!(!state.can_record(), "a running job locks the record dot");
        assert!(state.start_recording(Instant::now(), "d").is_empty());

        // Without capture compiled the dot is dead too (the Linux default build).
        let mut plain = AppState::new();
        assert_eq!(plain.capture_available, capture::CAPTURE_COMPILED);
        assert_eq!(
            plain.can_record(),
            capture::CAPTURE_COMPILED,
            "the probe is the build's capture feature"
        );
        plain.capture_available = false;
        assert!(!plain.can_record());
        assert!(plain.start_recording(Instant::now(), "d").is_empty());
        assert!(matches!(plain.record, RecordState::Idle));
    }

    #[test]
    fn a_recording_owns_the_audio_session() {
        let mut state = recording_state();
        state.on_library_loaded(vec![transcript("open.wav")], None);
        state.start_recording(Instant::now(), "2026-10-13 14:05:33");

        // Imports and library clicks are ignored while recording.
        assert!(state.begin_import(temp_wav("import")).is_empty());
        assert!(state.select_library(0).is_empty());
        assert_eq!(state.route, Route::Record);

        // The waveform is a rolling window of RECORD_WAVEFORM_BARS samples.
        for i in 0..(RECORD_WAVEFORM_BARS + 10) {
            state.record_sample(i as f32 / 200.0);
        }
        match &state.record {
            RecordState::Recording(rec) => {
                assert_eq!(rec.waveform.len(), RECORD_WAVEFORM_BARS);
                assert!((rec.rms - (RECORD_WAVEFORM_BARS as f32 + 9.0) / 200.0).abs() < 1e-6);
                assert_eq!(rec.waveform[0], 10.0 / 200.0, "oldest samples dropped");
            }
            other => panic!("expected a recording, got {other:?}"),
        }
    }

    #[test]
    fn temp_recordings_are_discarded_only_after_a_successful_save() {
        let wav = temp_wav("temp-record");
        let mut state = recording_state();
        state.start_recording(Instant::now(), "2026-10-13 14:05:33");
        state.stop_recording();
        state.on_recording_stopped(wav.clone(), false);

        // Job completes; the library save FAILS → the temp file stays for a retry.
        state.on_worker_event(finished_event(transcript_with_audio(
            "Recording x.wav",
            &wav,
        )));
        let effects = state.on_transcript_saved(
            transcript_with_audio("Recording x.wav", &wav),
            Err("database is locked".to_string()),
        );
        assert!(effects.is_empty());
        assert_eq!(
            state.pending_recording_cleanup.as_deref(),
            Some(wav.as_path())
        );

        // A successful save discards it.
        let effects =
            state.on_transcript_saved(transcript_with_audio("Recording x.wav", &wav), Ok(()));
        assert!(matches!(effects[0], Effect::RefreshLibrary));
        match &effects[1] {
            Effect::DiscardRecording { path } => assert_eq!(path, &wav),
            other => panic!("expected a discard effect, got {other:?}"),
        }
        assert!(state.pending_recording_cleanup.is_none());

        // keep_audio on → nothing is discarded.
        let kept = temp_wav("kept-record");
        let mut state = recording_state();
        state.start_recording(Instant::now(), "2026-10-13 14:05:33");
        state.stop_recording();
        state.on_recording_stopped(kept.clone(), true);
        assert!(state.pending_recording_cleanup.is_none());
        let effects =
            state.on_transcript_saved(transcript_with_audio("Recording y.wav", &kept), Ok(()));
        assert_eq!(effects.len(), 1, "only the library refresh");
    }

    // ---- M3: model manager ---------------------------------------------------

    #[test]
    fn model_manager_actions_flow_through_effects() {
        let mut state = AppState::new();
        let effects = state.open_models();
        assert!(matches!(effects.as_slice(), [Effect::RefreshModels]));
        assert_eq!(state.route, Route::Models);

        state.on_models_loaded(vec![ModelRow {
            id: "tiny",
            hf_file: "ggml-tiny.bin",
            size_bytes: 77_691_713,
            role: "CI + tests",
            install: model_manager::InstallState::Missing,
            encoder: model_manager::EncoderState::Missing,
            is_default: false,
        }]);
        assert_eq!(state.model_manager.rows.len(), 1);

        // Download start → effect + running state; a second start is ignored.
        let effects = state.start_model_download("tiny");
        assert!(matches!(effects.as_slice(), [Effect::DownloadModel { .. }]));
        assert!(state.start_model_download("base").is_empty());
        state.on_download_progress(38_845_856, 77_691_713);
        assert_eq!(state.model_manager.download.percent(), Some(49));
        let effects = state.on_download_done("tiny", Ok(()));
        assert!(matches!(effects.as_slice(), [Effect::RefreshModels]));
        assert!(state.model_manager.status.as_ref().unwrap().is_ok());

        // Delete → effect; the result line follows the runtime's answer.
        let effects = state.delete_model("small");
        assert!(matches!(effects.as_slice(), [Effect::DeleteModel { .. }]));
        let effects = state.on_model_deleted("small", Ok(vec!["/m/ggml-small.bin".to_string()]));
        assert!(matches!(effects.as_slice(), [Effect::RefreshModels]));
        assert!(state
            .model_manager
            .status
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .contains("1 file(s)"));

        // Set default → effect; the marker refresh follows.
        let effects = state.set_default_model("tiny");
        assert!(matches!(
            effects.as_slice(),
            [Effect::SetDefaultModel { .. }]
        ));
        let effects = state.on_default_set("tiny", Ok(()));
        assert!(matches!(effects.as_slice(), [Effect::RefreshModels]));
        assert!(state
            .model_manager
            .status
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .contains("default"));
    }

    // ---- M4.5: transcript delete --------------------------------------------

    #[test]
    fn transcript_delete_flows_through_effects_and_closes_the_editor() {
        let mut state = AppState::new();
        let open = transcript("trash.wav");
        let id = open.id;
        state.on_transcript_loaded(open.clone());
        state.on_library_loaded(vec![open], None);
        assert_eq!(state.route, Route::Editor);
        assert!(
            !state.editor.as_ref().unwrap().confirm_delete,
            "a freshly opened editor is not armed"
        );

        // Trash confirmed → the delete effect carries the open transcript's id.
        let effects = state.delete_open_transcript();
        match effects.as_slice() {
            [Effect::DeleteTranscript { id: effect_id }] => assert_eq!(*effect_id, id),
            other => panic!("expected a delete effect, got {other:?}"),
        }

        // The runtime answers Ok: Home + status + a library refresh.
        let effects = state.on_transcript_deleted(id, Ok(()));
        assert!(matches!(effects.as_slice(), [Effect::RefreshLibrary]));
        assert!(state.editor.is_none());
        assert_eq!(state.route, Route::Home);
        let status = state.status.as_ref().unwrap();
        assert!(!status.error);
        assert_eq!(status.text, "deleted trash.wav");

        // The refreshed list no longer holds the row (the sidebar row is gone).
        state.on_library_loaded(Vec::new(), None);
        assert!(state.library.items.is_empty());
        assert_eq!(state.library.selected, None);
    }

    #[test]
    fn deleting_another_transcript_keeps_the_open_editor() {
        let mut state = AppState::new();
        let open = transcript("open.wav");
        let other = transcript("other.wav");
        state.on_transcript_loaded(open.clone());
        state.on_library_loaded(vec![open.clone(), other.clone()], None);

        let effects = state.on_transcript_deleted(other.id, Ok(()));
        assert!(matches!(effects.as_slice(), [Effect::RefreshLibrary]));
        assert_eq!(state.route, Route::Editor, "the open transcript stays open");
        assert_eq!(state.editor.as_ref().unwrap().transcript.id, open.id);
        assert_eq!(state.status.as_ref().unwrap().text, "deleted other.wav");
    }

    #[test]
    fn transcript_delete_failure_surfaces_and_changes_nothing() {
        let mut state = AppState::new();
        let open = transcript("locked.wav");
        state.on_transcript_loaded(open.clone());

        let effects = state.on_transcript_deleted(open.id, Err("database is locked".to_string()));
        assert!(effects.is_empty());
        assert_eq!(state.route, Route::Editor, "no navigation on failure");
        assert!(state.editor.is_some());
        let status = state.status.as_ref().unwrap();
        assert!(status.error);
        assert!(status.text.contains("could not delete"), "{}", status.text);
        assert!(
            status.text.contains("database is locked"),
            "{}",
            status.text
        );
    }

    #[test]
    fn delete_without_an_open_transcript_emits_nothing() {
        let mut state = AppState::new();
        assert!(state.delete_open_transcript().is_empty());
    }

    // ---- M3: settings --------------------------------------------------------

    #[test]
    fn settings_save_and_reset_flow() {
        let mut state = AppState::new();
        let effects = state.open_settings();
        assert!(matches!(effects.as_slice(), [Effect::RefreshAudioDevices]));
        assert_eq!(state.route, Route::Settings);

        state.on_audio_devices(
            vec![
                "MacBook Microphone".to_string(),
                "BlackHole 2ch".to_string(),
            ],
            None,
        );
        assert_eq!(state.audio_devices.len(), 2);

        // Save carries the current form values.
        state.settings_form.keep_audio = true;
        state.settings_form.engine = EngineSetting::Cpu;
        state.settings_form.dirty = true;
        let effects = state.save_settings();
        match effects.as_slice() {
            [Effect::SaveSettings { form }] => {
                assert!(form.keep_audio);
                assert_eq!(form.engine, EngineSetting::Cpu);
            }
            other => panic!("expected a save effect, got {other:?}"),
        }

        state.on_settings_saved(Ok("settings saved".to_string()));
        assert!(!state.settings_form.dirty);
        assert!(state.settings_form.status.as_ref().unwrap().is_ok());

        // A failed save keeps the dirty marker for a retry.
        state.on_settings_saved(Err("disk full".to_string()));
        assert!(state.settings_form.dirty);
        assert!(state.settings_form.status.as_ref().unwrap().is_err());

        // Reset is form-only: defaults appear, nothing is persisted here.
        state.reset_settings_form();
        assert_eq!(state.settings_form.model_id, "turbo-q5-0");
        assert!(!state.settings_form.keep_audio);
        assert!(state.settings_form.dirty);
    }

    #[test]
    fn home_error_note_when_capture_is_unavailable() {
        // The home view reads `capture_available` to show the disabled-dot note;
        // the probe is the asr crate's compile-time flag (`capture` feature).
        let state = AppState::new();
        assert_eq!(state.capture_available, capture::CAPTURE_COMPILED);

        // On a build without capture the dot is dead by default; with the flag
        // cleared it is dead even on a capture-capable build.
        let mut plain = AppState::new();
        if !capture::CAPTURE_COMPILED {
            assert!(!plain.can_record());
        }
        plain.capture_available = false;
        assert!(!plain.can_record());
    }

    // ---- M8: playback --------------------------------------------------------

    fn playback_status(position_ms: u32, duration_ms: u32, finished: bool) -> PlaybackStatus {
        PlaybackStatus {
            position_ms,
            duration_ms,
            finished,
            error: None,
        }
    }

    #[test]
    fn opening_a_transcript_resolves_its_playback_source() {
        let wav = temp_wav("playback-open");
        let mut state = AppState::new();
        assert!(
            state.playback.hint().is_some(),
            "no transcript open → nothing to play"
        );

        state.on_transcript_loaded(transcript_with_audio("open.wav", &wav));
        assert_eq!(state.playback.ready_path(), Some(wav.as_path()));
        assert!(state.playback.can_play());
        assert_eq!(
            state.playback.duration_ms(),
            1_900,
            "the last segment end is the pre-engine estimate"
        );
        assert!(!state.busy(), "a parked transport keeps no tick");
        assert_eq!(state.playback.position_ms(), 0);
    }

    #[test]
    fn playback_idle_playing_paused_then_stop_on_leave() {
        let wav = temp_wav("playback-leave");
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript_with_audio("leave.wav", &wav));

        // idle → playing: the Play effect carries the file the engine must open.
        let effects = state.toggle_playback();
        match effects.as_slice() {
            [Effect::Playback { source, command }] => {
                assert_eq!(source.as_deref(), Some(wav.as_path()));
                assert_eq!(*command, PlaybackCommand::Play { from_ms: 0 });
            }
            other => panic!("expected a play effect, got {other:?}"),
        }
        assert!(state.playback.is_playing());
        assert!(state.busy(), "the moving playhead must keep frames coming");

        // The polled position drives the playhead.
        assert!(state
            .on_playback_status(playback_status(900, 1_900, false))
            .is_empty());
        assert_eq!(state.playback.position_ms(), 900);

        // playing → paused.
        let effects = state.toggle_playback();
        assert!(matches!(
            effects.as_slice(),
            [Effect::Playback {
                source: None,
                command: PlaybackCommand::Pause
            }]
        ));
        assert!(state.playback.is_paused());
        assert!(
            !state.busy(),
            "a paused transport releases the tick (no yanking while paused)"
        );

        // paused → playing (resume, no file needed).
        let effects = state.toggle_playback();
        assert!(matches!(
            effects.as_slice(),
            [Effect::Playback {
                command: PlaybackCommand::Resume,
                ..
            }]
        ));

        // stop-on-leave: Home stops the engine and resets the playhead.
        let effects = state.back_home();
        assert!(
            matches!(
                effects.first(),
                Some(Effect::Playback {
                    command: PlaybackCommand::Stop,
                    ..
                })
            ),
            "expected a stop first, got {effects:?}"
        );
        assert!(state.playback.is_idle());
        assert_eq!(state.playback.position_ms(), 0);
        assert!(!state.busy());
        assert_eq!(
            state.playback.state(),
            &suramak_playback::PlaybackState::Idle,
            "the source stays resolved (ready) after a stop"
        );
    }

    #[test]
    fn an_unavailable_source_shows_the_hint_and_never_plays() {
        let mut state = AppState::new();
        let mut gone = transcript("gone.wav");
        gone.audio_path = Some(PathBuf::from("/definitely/not/here.wav"));
        state.on_transcript_loaded(gone);

        let hint = state.playback.hint().expect("unavailable").to_string();
        assert!(hint.contains("import"), "unexpected hint: {hint}");
        assert!(!state.playback.can_play());
        assert!(
            state.toggle_playback().is_empty(),
            "the control emits nothing"
        );
        assert!(
            state.seek_playback(1_200).is_empty(),
            "seeking is a no-op too"
        );
        assert!(!state.busy());
    }

    #[test]
    fn a_recording_without_kept_audio_hints_at_the_setting() {
        let mut state = AppState::new();
        let mut recorded = transcript("Recording x.wav");
        recorded.source = Source::Recording {
            started_at: chrono::Utc::now(),
            duration_ms: 1_900,
        };
        // `keep_audio` off: no path survived the save.
        state.on_transcript_loaded(recorded);
        let hint = state.playback.hint().expect("unavailable").to_string();
        assert!(hint.contains("keep audio"), "unexpected hint: {hint}");
        assert!(!state.playback.can_play());
    }

    #[test]
    fn a_device_failure_lands_in_the_strip_and_play_retries() {
        let wav = temp_wav("playback-device");
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript_with_audio("device.wav", &wav));
        state.toggle_playback();
        assert!(state.playback.is_playing());

        state.on_playback_status(PlaybackStatus::failure(
            "no audio output device on this machine",
            true,
        ));
        let hint = state.playback.hint().expect("unavailable").to_string();
        assert!(hint.contains("output device"), "unexpected hint: {hint}");
        assert!(
            state.playback.can_play(),
            "a device error is retryable — plug in and press play again"
        );
        assert!(!state.busy(), "nothing is moving while it cannot play");

        let effects = state.toggle_playback();
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::Playback {
                    command: PlaybackCommand::Play { .. },
                    ..
                }]
            ),
            "the retry opens a session again: {effects:?}"
        );
        assert!(state.playback.is_playing());
    }

    #[test]
    fn seeking_moves_the_playhead_and_tells_a_live_engine() {
        let wav = temp_wav("playback-seek");
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript_with_audio("seek.wav", &wav));

        // Idle: the position is remembered for the next Play (no engine yet).
        assert!(state.seek_playback(1_000).is_empty());
        assert_eq!(state.playback.position_ms(), 1_000);

        // Playing: the engine is told; the playhead clamps to the duration.
        state.toggle_playback();
        let effects = state.seek_playback(99_000);
        match effects.as_slice() {
            [Effect::Playback { source, command }] => {
                assert_eq!(*source, None, "a seek addresses the open engine");
                assert_eq!(*command, PlaybackCommand::Seek { ms: 1_900 });
            }
            other => panic!("expected a seek effect, got {other:?}"),
        }
        assert_eq!(state.playback.position_ms(), 1_900);
    }

    #[test]
    fn starting_a_recording_stops_playback_first() {
        let mut state = recording_state();
        let wav = temp_wav("playback-record");
        state.on_transcript_loaded(transcript_with_audio("rec.wav", &wav));
        state.toggle_playback();
        assert!(state.playback.is_playing());

        let effects = state.start_recording(Instant::now(), "2026-10-13 14:05:33");
        match effects.as_slice() {
            [Effect::Playback {
                command: PlaybackCommand::Stop,
                ..
            }, Effect::StartCapture] => {}
            other => panic!("expected stop-then-capture, got {other:?}"),
        }
        assert!(state.playback.is_idle());
        assert_eq!(state.route, Route::Record);
    }

    #[test]
    fn deleting_the_open_transcript_stops_playback_and_clears_it() {
        let wav = temp_wav("playback-delete");
        let open = transcript_with_audio("bye.wav", &wav);
        let id = open.id;
        let mut state = AppState::new();
        state.on_transcript_loaded(open);
        state.toggle_playback();

        let effects = state.on_transcript_deleted(id, Ok(()));
        match effects.as_slice() {
            [Effect::Playback {
                command: PlaybackCommand::Stop,
                ..
            }, Effect::RefreshLibrary] => {}
            other => panic!("expected stop + refresh, got {other:?}"),
        }
        assert_eq!(state.route, Route::Home);
        assert!(state.playback.hint().is_some());
        assert!(!state.playback.can_play(), "the transport was cleared");
    }

    #[test]
    fn switching_transcripts_stops_the_old_playback() {
        let first = temp_wav("playback-switch-first");
        let second = temp_wav("playback-switch-second");
        let mut state = AppState::new();
        let open = transcript_with_audio("first.wav", &first);
        let other = transcript_with_audio("second.wav", &second);
        state.on_transcript_loaded(open.clone());
        state.on_library_loaded(vec![open, other.clone()], None);
        state.toggle_playback();
        assert!(state.playback.is_playing());

        let effects = state.select_library(1);
        assert!(
            matches!(
                effects.first(),
                Some(Effect::Playback {
                    command: PlaybackCommand::Stop,
                    ..
                })
            ),
            "the old session stops before the load: {effects:?}"
        );
        state.on_transcript_loaded(other.clone());
        assert!(state.playback.is_idle());
        assert_eq!(state.playback.ready_path(), Some(second.as_path()));
        assert_eq!(state.playback.position_ms(), 0);
    }

    #[test]
    fn the_engine_status_parks_the_playhead_at_the_end() {
        let wav = temp_wav("playback-end");
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript_with_audio("end.wav", &wav));
        state.toggle_playback();

        state.on_playback_status(playback_status(1_900, 4_000, true));
        assert!(state.playback.is_idle(), "finished → idle");
        assert_eq!(state.playback.position_ms(), 4_000, "parked at the end");
        assert_eq!(
            state.playback.duration_ms(),
            4_000,
            "the engine's real duration replaced the estimate"
        );
        assert!(!state.busy());

        // Play again starts from the top.
        let effects = state.toggle_playback();
        assert!(matches!(
            effects.as_slice(),
            [Effect::Playback {
                command: PlaybackCommand::Play { from_ms: 0 },
                ..
            }]
        ));
    }

    #[test]
    fn importing_stops_playback_before_the_job_starts() {
        let wav = temp_wav("playback-import");
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript_with_audio("open.wav", &wav));
        state.toggle_playback();

        let input = temp_wav("playback-import-input");
        let effects = state.begin_import(input);
        match effects.as_slice() {
            [Effect::Playback {
                command: PlaybackCommand::Stop,
                ..
            }, Effect::SubmitJob { .. }] => {}
            other => panic!("expected stop-then-submit, got {other:?}"),
        }
        assert!(state.playback.is_idle());
        assert_eq!(state.route, Route::Transcribe);
    }

    #[test]
    fn the_editor_paints_the_active_segment_from_the_playhead() {
        // The editor's highlight is `segment_index_at(segments, position)` on the
        // transcript rows; this pins the mapping the view uses (start 0/1000,
        // ends 900/1900 from the test fixture).
        let wav = temp_wav("playback-active");
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript_with_audio("active.wav", &wav));
        let segments = &state.editor.as_ref().unwrap().transcript.segments;
        assert_eq!(suramak_playback::segment_index_at(segments, 0), Some(0));
        assert_eq!(suramak_playback::segment_index_at(segments, 599), Some(0));
        assert_eq!(suramak_playback::segment_index_at(segments, 900), None);
        assert_eq!(suramak_playback::segment_index_at(segments, 1_000), Some(1));
        assert_eq!(suramak_playback::segment_index_at(segments, 5_000), None);
    }
}
