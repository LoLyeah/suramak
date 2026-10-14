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
use suramak_core::settings::{Appearance, EngineSetting, RecordSource};
use suramak_core::transcript::{Segment, Transcript};
use suramak_playback::{
    resolve_playback_source, transcript_duration_ms, Command as PlaybackCommand, Peaks, Playback,
    Status as PlaybackStatus,
};
use uuid::Uuid;

use crate::theme::AppTheme;

use self::model_manager::{ModelManager, ModelRow};
use self::polish::{PolishState, PolishUndo};
use self::settings_form::SettingsForm;

pub mod bounds;
pub mod controls;
pub mod editor;
pub mod export;
pub mod home;
pub mod keymap;
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
    /// M6: one row per live source (label + gain/mute). Empty until the
    /// runtime reports the session's sources; the record view draws the rows
    /// and the runtime reads the gains at stop.
    pub sources: Vec<RecordingSource>,
    /// M7: the live provisional transcript (session timeline), grown by the
    /// runtime's live decoder while the recording runs. **Always cleared on
    /// Stop** — the final pass replaces it, and only the final transcript is
    /// ever saved.
    pub provisional: Vec<Segment>,
    /// M7: windows skipped by the live backlog guard (skip-and-merge; the
    /// decoder could not keep up). Surfaced subtly in the record view; reset
    /// with the provisional list.
    pub provisional_skipped: u64,
    /// M9a: first-seen instant per provisional segment (same order as
    /// [`Recording::provisional`]), so the record view can fade+rise each line
    /// as it arrives. Internal bookkeeping — not shown anywhere.
    pub provisional_arrivals: Vec<Instant>,
    /// Set while the runtime is finalising the wav (Stop pressed once).
    pub stopping: bool,
}

/// One live recording source shown in the record view (M6).
///
/// `gain`/`muted` are applied by the mixing engine at finalize — the mix is
/// computed from the captured per-source buffers, so adjusting a control while
/// recording applies to the whole recording (documented in the M6 report).
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingSource {
    pub label: String,
    /// Per-source gain applied before the sum (1.0 = unity).
    pub gain: f32,
    pub muted: bool,
}

impl RecordingSource {
    /// Gain slider range (0× = silent … 2× = +6 dB).
    pub const MIN_GAIN: f32 = 0.0;
    pub const MAX_GAIN: f32 = 2.0;
}

/// User-facing label for one settings source (the record view's rows and the
/// runtime's mix reports).
pub fn record_source_label(source: &RecordSource) -> String {
    match source {
        RecordSource::Default => "Default microphone".to_string(),
        RecordSource::Device { name } => name.clone(),
        RecordSource::SystemAudio => "System audio (all processes except Suramak)".to_string(),
        RecordSource::AppAudio { apps } => format!("App audio ({})", apps.join(", ")),
    }
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

/// The **elapsed hh:mm:ss** clock for recordings (M9c copy sweep: the one
/// sanctioned recording time format — the record view's timer and the floating
/// pill both use it; the old `mm:ss.t` / `mm:ss` variants are gone). Hours
/// never fold into minutes here: a 70-minute recording reads `01:10:00`.
pub fn format_elapsed(elapsed: Duration) -> String {
    let total_secs = elapsed.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// `mm:ss.mmm` transport readout for the deck (`01:02.480`; hours fold into
/// minutes). The brief's one-time-format system: recordings use `hh:mm:ss`
/// elapsed, the transport uses `mm:ss.mmm`, and nothing else shows a clock.
pub fn format_transport(ms: u32) -> String {
    let minutes = ms / 60_000;
    let seconds = (ms % 60_000) / 1000;
    let millis = ms % 1000;
    format!("{minutes:02}:{seconds:02}.{millis:03}")
}

/// `1 segment` / `5 segments` — the pluralization rule for every count the UI
/// shows (M9c copy sweep: no `N item(s)` strings).
pub fn count_noun(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// A human label for an engine id (`cpu` → `CPU`, `coreml` → `Core ML`) —
/// the plain-words rule for the transcribe card and the outcome line. Unknown
/// strings pass through unchanged.
pub fn engine_display(name: &str) -> String {
    match name {
        "cpu" => "CPU".to_string(),
        "metal" => "Metal".to_string(),
        "coreml" => "Core ML".to_string(),
        other => other.to_string(),
    }
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
    /// M9c list motion: set once the first list has been drawn. Insert
    /// arrivals are stamped only for rows that appear *after* it, so the
    /// first frame's list renders statically (only changes animate).
    pub loaded_once: bool,
    /// Insert ease (base token): `(transcript id, first-seen instant)` for rows
    /// that appeared after the first load — the row fades + rises once.
    pub arrivals: Vec<(Uuid, Instant)>,
    /// Remove ease (base token): rows that just left the list (delete or a
    /// refresh that dropped them), kept with their old index so the list can
    /// shrink their height and fade them out where they were.
    pub leaving: Vec<LeavingRow>,
}

/// One row that is leaving a list (M9c remove ease).
#[derive(Debug, Clone)]
pub struct LeavingRow {
    pub item: Transcript,
    /// The row's index in the list *before* it left (its draw position).
    pub index: usize,
    /// When it left; the fade + height collapse runs for the base token.
    pub at: Instant,
}

impl LibraryState {
    /// The insert ease's progress for a row (0..=1), when it is still arriving.
    /// `None` → draw at 1 (no arrival). The caller passes the base token
    /// duration ([`crate::motion::arrival_progress`] semantics).
    pub fn row_arrival(&self, id: Uuid, now: Instant, token: f32) -> Option<f32> {
        let at = self
            .arrivals
            .iter()
            .find(|(arrival_id, _)| *arrival_id == id)
            .map(|(_, at)| *at)?;
        Some(crate::motion::arrival_progress(
            now.saturating_duration_since(at),
            token,
        ))
    }

    /// Prunes settled insert arrivals and finished remove-ease rows. Called by
    /// the list views each frame (and on refresh); both animations END — once
    /// the token elapsed, the bookkeeping is dropped and nothing repaints.
    pub fn prune_motion(&mut self, now: Instant, token: Duration) {
        self.arrivals
            .retain(|(_, at)| now.saturating_duration_since(*at) < token);
        self.leaving
            .retain(|row| now.saturating_duration_since(row.at) < token);
    }
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
    /// M9a: the export sheet (anchored under the deck's `Export…` button) is
    /// open. The old fixed bottom band is gone — it collided with the floating
    /// record pill (critique P1/H8).
    pub open: bool,
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            format: ExportFormat::Md,
            dest: String::new(),
            follows_default: true,
            result: None,
            open: false,
        }
    }
}

/// Cross-view status message (engine line, save confirmations, errors).
#[derive(Debug, Clone)]
pub struct Status {
    pub text: String,
    pub error: bool,
}

/// The open transcript's waveform envelope (M9b).
///
/// Peaks are computed once per source load on a background thread (the runtime's
/// peaks job) and land here by transcript id. `transcript` names the editor the
/// envelope belongs to — a late result for a transcript that was left (or
/// switched away from) is discarded, never drawn over another one. While
/// `peaks` is `None` the strip draws the honest flat baseline (no audio, or the
/// decode still in flight) — never fake data.
#[derive(Debug, Clone, Default)]
pub struct EditorPeaks {
    /// The transcript the envelope belongs to (`None` until a job is started).
    pub transcript: Option<Uuid>,
    /// The decoded envelope, once the background job answered.
    pub peaks: Option<Peaks>,
    /// Why the envelope is missing (a decode failure) — logged, not shown.
    pub error: Option<String>,
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
    /// Enumerated input device names (for the Audio sources checkbox list).
    pub audio_devices: Vec<String>,
    /// Enumeration problem (no capture compiled / no host support), shown as a note.
    pub audio_devices_error: Option<String>,
    /// M6: running processes with audio (the "Only an app…" picker).
    pub audio_processes: Vec<capture::AudioProcessInfo>,
    /// Process-listing problem (macOS-only; shown as a note in the picker).
    pub audio_processes_error: Option<String>,
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
    /// Whether the floating record control has a real system material behind it
    /// (macOS 26+: the `NSGlassEffectView` subview via `suramak-glass`, M8.5).
    /// The painter then draws the glyph/label/time only; `false` paints the
    /// fallback pill. Seeded by the shell at startup; a field so the state is
    /// testable headless.
    pub float_glass: bool,
    /// M9a: the effective appearance the token ladder is drawn with (System has
    /// been resolved against the OS theme). Seeded + kept current by the shell
    /// (`Shell::sync_appearance`).
    pub theme: AppTheme,
    /// M9a: the appearance *setting* (System | Light | Dark) — what the
    /// Settings row edits and the shell resolves.
    pub appearance: Appearance,
    /// M9a: macOS Reduce Motion (or the platform default `false`). Kept current
    /// by the shell at startup and whenever settings apply; every motion
    /// helper reads it through [`crate::motion::Motion`].
    pub reduce_motion: bool,
    /// M9b: the open transcript's waveform envelope (background decode).
    pub peaks: EditorPeaks,
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
    /// Enumerate input devices for the Audio sources checkbox list.
    RefreshAudioDevices,
    /// M6: list the running processes with audio (the per-app picker).
    RefreshAudioProcesses,
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
    /// Compute the open transcript's waveform envelope off the UI thread
    /// (M9b): the runtime decodes `path` once and answers through
    /// [`Runtime::peaks_status`]; the result lands on [`AppState::on_peaks_loaded`]
    /// tagged with `transcript`, so a stale decode is discarded.
    LoadPeaks { transcript: Uuid, path: PathBuf },
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
            audio_processes: Vec::new(),
            audio_processes_error: None,
            pending_recording_cleanup: None,
            capture_available: capture::CAPTURE_COMPILED,
            polish: PolishState::Idle,
            polish_undo: None,
            // Seeded by the runtime at startup (`Runtime::polish_configured`);
            // false until then so the action can never fire half-configured.
            polish_configured: false,
            playback: Playback::none(),
            // Seeded by the shell at startup (`GlassHandle::float_glass_available`);
            // false until then — headless/non-macOS builds paint the pill.
            float_glass: false,
            // M9a: seeded + kept current by the shell (`Shell::sync_appearance`);
            // the defaults keep headless tests deterministic (dark ladder,
            // System setting, no reduced motion).
            theme: AppTheme::Dark,
            appearance: Appearance::System,
            reduce_motion: false,
            peaks: EditorPeaks::default(),
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

    /// Whether the floating record control can start a session: capture compiled
    /// in, no job running, not already recording (M8.5: the floating control is
    /// the record entry point).
    pub fn can_record(&self) -> bool {
        self.capture_available
            && !matches!(self.job, Job::Running(_))
            && !matches!(self.record, RecordState::Recording(_))
    }

    /// The floating record control pressed (M8.5) — the single record entry
    /// point. Idle → start (the effect the titlebar dot used to emit);
    /// recording → stop (the same `StopCapture` effect the record view's Stop
    /// button emits, final pass → transcript); unavailable → the same status
    /// lines the old titlebar dot produced.
    pub fn toggle_record(&mut self, now: Instant, date: &str) -> Vec<Effect> {
        let recording = matches!(self.record, RecordState::Recording(_));
        if recording {
            let stopping =
                matches!(&self.record, RecordState::Recording(recording) if recording.stopping);
            if stopping {
                // The runtime is finalising; a second stop is a no-op.
                return Vec::new();
            }
            return self.stop_recording();
        }
        if self.can_record() {
            return self.start_recording(now, date);
        }
        if matches!(self.job, Job::Running(_)) {
            self.set_status("recording is disabled while a job is running", false);
        } else {
            self.set_status(
                "recording is not compiled into this build (feature `capture`)",
                true,
            );
        }
        Vec::new()
    }

    /// Record pressed: opens the recording view and asks the runtime to start.
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
            sources: Vec::new(),
            provisional: Vec::new(),
            provisional_skipped: 0,
            provisional_arrivals: Vec::new(),
            stopping: false,
        });
        self.route = Route::Record;
        effects.push(Effect::StartCapture);
        effects
    }

    /// Runtime: capture is live on these sources (M6) — the record view shows
    /// one row per source; gains start at unity, nothing muted.
    ///
    /// The labels' order is the runtime's recorder order; the runtime reads the
    /// gains back from here at stop.
    pub fn record_sources(&mut self, labels: Vec<String>) {
        if let RecordState::Recording(recording) = &mut self.record {
            if !labels.is_empty() {
                recording.device = Some(labels.join(" + "));
            }
            recording.sources = labels
                .into_iter()
                .map(|label| RecordingSource {
                    label,
                    gain: 1.0,
                    muted: false,
                })
                .collect();
        }
    }

    /// Record view: per-source gain (clamped to the slider range).
    pub fn set_source_gain(&mut self, index: usize, gain: f32) {
        if let RecordState::Recording(recording) = &mut self.record {
            if let Some(source) = recording.sources.get_mut(index) {
                source.gain = gain.clamp(RecordingSource::MIN_GAIN, RecordingSource::MAX_GAIN);
            }
        }
    }

    /// Record view: per-source mute toggle.
    pub fn set_source_muted(&mut self, index: usize, muted: bool) {
        if let RecordState::Recording(recording) = &mut self.record {
            if let Some(source) = recording.sources.get_mut(index) {
                source.muted = muted;
            }
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

    /// Runtime: the live provisional transcript changed (M7) — the decoder
    /// merged new segments (or the skipped-window count moved).
    ///
    /// Ignored once Stop was pressed: provisional content is **always**
    /// discarded on Stop and the final pass takes over. M9a: segments seen for
    /// the first time get an arrival instant (matched by `idx`), so the record
    /// view can fade+rise each new line.
    pub fn record_provisional(&mut self, segments: Vec<Segment>, skipped_windows: u64) {
        if let RecordState::Recording(recording) = &mut self.record {
            if !recording.stopping {
                let now = Instant::now();
                let previous: Vec<(u32, Instant)> = recording
                    .provisional
                    .iter()
                    .zip(recording.provisional_arrivals.iter())
                    .map(|(segment, at)| (segment.idx, *at))
                    .collect();
                recording.provisional_arrivals = segments
                    .iter()
                    .map(|segment| {
                        previous
                            .iter()
                            .find(|(idx, _)| *idx == segment.idx)
                            .map(|(_, at)| *at)
                            .unwrap_or(now)
                    })
                    .collect();
                recording.provisional = segments;
                recording.provisional_skipped = skipped_windows;
            }
        }
    }

    /// Stop pressed in the record view (idempotent until the runtime answers).
    ///
    /// The provisional transcript is discarded right here — the final pass
    /// (running after the wav is written) is the only source of the saved
    /// transcript, so no provisional text can leak into the library.
    pub fn stop_recording(&mut self) -> Vec<Effect> {
        match &mut self.record {
            RecordState::Recording(recording) if !recording.stopping => {
                recording.stopping = true;
                recording.provisional.clear();
                recording.provisional_arrivals.clear();
                recording.provisional_skipped = 0;
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

    /// Headless smoke harness (M9a): seeds a frozen-but-ticking demo recording
    /// so the Record view's composition can be captured on builds without
    /// capture (the Linux VPS). Only `main.rs` calls this, only when
    /// `SURAMAK_SMOKE_RECORD` is set AND a smoke mode is active; it starts no
    /// capture session and touches no pipeline — the view merely draws this
    /// state. Everything in it is shaped exactly like a real session's state.
    pub fn seed_demo_recording(&mut self) {
        let now = Instant::now();
        let waveform: Vec<f32> = (0..RECORD_WAVEFORM_BARS)
            .map(|index| {
                let phase = index as f32 * 0.35;
                (0.24 + 0.6 * (phase.sin().abs() * 0.75 + (index % 7) as f32 * 0.018))
                    .clamp(0.05, 0.95)
            })
            .collect();
        let provisional = vec![
            Segment {
                idx: 0,
                start_ms: 0,
                end_ms: 4200,
                text: "The quick brown fox jumps over the lazy dog.".to_string(),
            },
            Segment {
                idx: 1,
                start_ms: 4200,
                end_ms: 12000,
                text: "Suramak keeps every recording on this machine and never sends audio to the cloud."
                    .to_string(),
            },
            Segment {
                idx: 2,
                start_ms: 12000,
                end_ms: 18600,
                text: "Halo, halo! Apa yang ini?".to_string(),
            },
        ];
        let provisional_arrivals = vec![
            now - Duration::from_millis(9_000),
            now - Duration::from_millis(4_000),
            now - Duration::from_millis(1_200),
        ];
        self.record = RecordState::Recording(Recording {
            title: "Recording 2026-10-14 09:41:02".to_string(),
            started_at: now - Duration::from_millis(42_600),
            elapsed: Duration::from_millis(42_600),
            rms: 0.62,
            waveform,
            device: Some(
                "Default microphone + System audio (all processes except Suramak)".to_string(),
            ),
            sources: vec![
                RecordingSource {
                    label: "Default microphone".to_string(),
                    gain: 1.0,
                    muted: false,
                },
                RecordingSource {
                    label: "System audio (all processes except Suramak)".to_string(),
                    gain: 0.8,
                    muted: false,
                },
            ],
            provisional,
            provisional_skipped: 0,
            provisional_arrivals,
            stopping: false,
        });
        self.route = Route::Record;
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

    // ---- waveform peaks (M9b) ----------------------------------------------

    /// The envelope the deck strip must draw for the open transcript, when it
    /// exists. `None` → the honest flat baseline (no audio, or still decoding).
    pub fn strip_peaks(&self) -> Option<&Peaks> {
        let editor = self.editor.as_ref()?;
        if self.peaks.transcript != Some(editor.transcript.id) {
            return None;
        }
        self.peaks.peaks.as_ref()
    }

    /// The runtime's answer to [`Effect::LoadPeaks`]: stored only for the
    /// transcript it was requested for — a late decode for another (or a
    /// closed) editor is discarded, never drawn over the wrong strip.
    pub fn on_peaks_loaded(&mut self, transcript: Uuid, result: Result<Peaks, String>) {
        if self.peaks.transcript != Some(transcript) {
            return;
        }
        match result {
            Ok(peaks) => {
                self.peaks.peaks = Some(peaks);
                self.peaks.error = None;
            }
            Err(err) => {
                tracing::warn!(transcript = %transcript, error = %err, "peaks: decode failed");
                self.peaks.error = Some(err);
            }
        }
    }

    // ---- model manager (M3) -------------------------------------------------

    /// Home → Models (the runtime rebuilds the table).
    pub fn open_models(&mut self) -> Vec<Effect> {
        self.route = Route::Models;
        vec![Effect::RefreshModels]
    }

    /// Runtime: fresh table rows.
    pub fn on_models_loaded(&mut self, rows: Vec<ModelRow>) {
        // M9c: the refresh bumps the table's epoch, so every row plays its
        // base-token arrival once (the re-check/delete state transition).
        self.model_manager.on_rows_loaded(rows);
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
        let name = model_manager::display_name(id);
        self.model_manager.status = Some(match result {
            Ok(removed) if removed.is_empty() => {
                Ok(format!("{name} is not installed — nothing to remove"))
            }
            Ok(removed) => Ok(format!(
                "{name}: removed {}",
                count_noun(removed.len(), "file")
            )),
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
            Ok(()) => Ok(format!(
                "{} is now the default model",
                model_manager::display_name(id)
            )),
            Err(err) => Err(err),
        });
        vec![Effect::RefreshModels]
    }

    // ---- settings (M3) ------------------------------------------------------

    /// Home → Settings (also re-enumerates the audio devices and, M6, the
    /// running audio processes for the per-app picker).
    pub fn open_settings(&mut self) -> Vec<Effect> {
        self.route = Route::Settings;
        vec![Effect::RefreshAudioDevices, Effect::RefreshAudioProcesses]
    }

    /// Runtime: enumerated input devices for the Audio sources checkbox list.
    pub fn on_audio_devices(&mut self, devices: Vec<String>, error: Option<String>) {
        self.audio_devices = devices;
        self.audio_devices_error = error;
    }

    /// Runtime: running audio processes for the "Only an app…" picker.
    pub fn on_audio_processes(
        &mut self,
        processes: Vec<capture::AudioProcessInfo>,
        error: Option<String>,
    ) {
        self.audio_processes = processes;
        self.audio_processes_error = error;
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
        if result.is_ok() {
            // M9c: a successful save arms the one-shot "applied" confirm pulse
            // (drawn by the settings view around Save; it ends and clears).
            self.settings_form.applied_at = Some(Instant::now());
        }
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
                let mut status = format!("engine: {}", engine_display(engine_used.as_str()));
                if !warnings.is_empty() {
                    status.push_str(&format!(" — {}", warnings.join("; ")));
                }
                self.set_status(status, false);
                let mut effects = self.open_editor(transcript.clone());
                effects.push(Effect::SaveTranscript(Box::new(transcript)));
                effects
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
    ///
    /// M9c list motion: rows that appear *after* the first load get an arrival
    /// (insert ease — the row fades + rises once, base token); rows a
    /// successful refresh drops are kept as [`LeavingRow`]s (remove ease — the
    /// list shrinks their height and fades them out where they were). The
    /// first load renders statically; a refresh that errored animates nothing.
    pub fn on_library_loaded(&mut self, items: Vec<Transcript>, error: Option<String>) {
        let now = Instant::now();
        if self.library.loaded_once && error.is_none() {
            let previous: Vec<Uuid> = self.library.items.iter().map(|item| item.id).collect();
            let live: Vec<Uuid> = items.iter().map(|item| item.id).collect();
            for (index, item) in self.library.items.iter().enumerate() {
                if !live.contains(&item.id) {
                    self.library.leaving.push(LeavingRow {
                        item: item.clone(),
                        index,
                        at: now,
                    });
                }
            }
            for item in &items {
                if !previous.contains(&item.id) {
                    self.library.arrivals.push((item.id, now));
                }
            }
            // A row that came back (a refresh raced a delete) is not leaving.
            self.library
                .leaving
                .retain(|row| !live.contains(&row.item.id));
        }
        self.library.loaded_once = true;
        self.library
            .prune_motion(now, crate::motion::token_duration(crate::motion::BASE));

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
        self.open_editor(transcript)
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
    /// stops and the playhead resets; M9a: the export sheet closes).
    pub fn back_home(&mut self) -> Vec<Effect> {
        self.route = Route::Home;
        self.export.open = false;
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

    /// M9a: the deck's `Export…` button toggles the export sheet (anchored
    /// under the button — never a bottom band over the floating pill).
    pub fn toggle_export(&mut self) {
        self.export.open = !self.export.open;
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

    /// Opens a transcript in the editor and switches the route. Returns the
    /// effects the open produced (M9b: the waveform peaks job).
    fn open_editor(&mut self, transcript: Transcript) -> Vec<Effect> {
        let id = transcript.id;
        self.export.dest = default_dest(&transcript.title, self.export.format);
        self.export.follows_default = true;
        self.export.result = None;
        self.export.open = false;
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
        // M9b: the strip's envelope is decoded once per source load, off the UI
        // thread. A source with no audio starts no job — the strip draws the
        // flat baseline instead (never fake data).
        self.peaks = EditorPeaks::default();
        let mut effects = Vec::new();
        if let Some(path) = self.playback.ready_path().map(Path::to_path_buf) {
            self.peaks.transcript = Some(id);
            effects.push(Effect::LoadPeaks {
                transcript: id,
                path,
            });
        }
        effects
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
            status.text.starts_with("engine: CPU — saved to library:"),
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
        state.record_sources(vec!["MacBook Microphone".to_string()]);
        state.record_sample(0.4);
        state.record_sample(0.6);
        state.tick(t0 + Duration::from_millis(7_300));
        match &state.record {
            RecordState::Recording(rec) => {
                assert_eq!(rec.device.as_deref(), Some("MacBook Microphone"));
                assert_eq!(rec.waveform, vec![0.4, 0.6]);
                assert!((rec.rms - 0.6).abs() < 1e-6);
                assert_eq!(rec.elapsed, Duration::from_millis(7_300));
                assert_eq!(format_elapsed(rec.elapsed), "00:00:07");
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
        assert_eq!(format_elapsed(Duration::from_millis(62_040)), "00:01:02");
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

    // ---- M6: multi-source record state --------------------------------------

    #[test]
    fn recording_sources_flow_into_the_record_view_with_gain_and_mute() {
        let mut state = recording_state();
        state.start_recording(Instant::now(), "2026-10-14 09:00:00");
        state.record_sources(vec![
            "Default microphone".to_string(),
            "System audio (all processes except Suramak)".to_string(),
        ]);
        match &state.record {
            RecordState::Recording(rec) => {
                assert_eq!(rec.sources.len(), 2, "one row per source");
                assert_eq!(rec.sources[0].label, "Default microphone");
                assert_eq!(rec.sources[0].gain, 1.0);
                assert!(!rec.sources[0].muted);
                assert_eq!(
                    rec.device.as_deref(),
                    Some("Default microphone + System audio (all processes except Suramak)"),
                    "the summary line lists every source"
                );
            }
            other => panic!("expected a recording, got {other:?}"),
        }

        // The record view's controls (gain slider / mute checkbox).
        state.set_source_gain(0, 0.5);
        state.set_source_muted(1, true);
        match &state.record {
            RecordState::Recording(rec) => {
                assert_eq!(rec.sources[0].gain, 0.5);
                assert!(rec.sources[1].muted);
            }
            other => panic!("expected a recording, got {other:?}"),
        }
        // Gain is clamped to the slider range; out-of-range indices are no-ops.
        state.set_source_gain(0, 9.0);
        state.set_source_gain(7, 0.5);
        match &state.record {
            RecordState::Recording(rec) => assert_eq!(rec.sources[0].gain, 2.0),
            other => panic!("expected a recording, got {other:?}"),
        }

        // Stop keeps the state (the runtime reads the gains back from here).
        let effects = state.stop_recording();
        assert!(matches!(effects.as_slice(), [Effect::StopCapture { .. }]));
        match &state.record {
            RecordState::Recording(rec) => {
                assert!(rec.stopping);
                assert_eq!(rec.sources.len(), 2, "gains survive the stop request");
                assert_eq!(rec.sources[0].gain, 2.0);
            }
            other => panic!("expected a recording, got {other:?}"),
        }
    }

    #[test]
    fn record_source_labels_cover_every_kind() {
        assert_eq!(
            record_source_label(&RecordSource::Default),
            "Default microphone"
        );
        assert_eq!(
            record_source_label(&RecordSource::Device {
                name: "BlackHole 2ch".to_string()
            }),
            "BlackHole 2ch"
        );
        assert_eq!(
            record_source_label(&RecordSource::SystemAudio),
            "System audio (all processes except Suramak)"
        );
        assert_eq!(
            record_source_label(&RecordSource::AppAudio {
                apps: vec!["us.zoom.xos".to_string(), "com.google.Chrome".to_string()]
            }),
            "App audio (us.zoom.xos, com.google.Chrome)"
        );
    }

    #[test]
    fn a_recording_without_reported_sources_keeps_the_old_shape() {
        // The runtime always reports sources on a successful start; before that
        // (or on an old scripted flow) the rows are empty and nothing breaks.
        let mut state = recording_state();
        state.start_recording(Instant::now(), "2026-10-14 09:00:00");
        match &state.record {
            RecordState::Recording(rec) => {
                assert!(rec.sources.is_empty());
                assert!(rec.device.is_none(), "starting capture…");
            }
            other => panic!("expected a recording, got {other:?}"),
        }
        state.record_sources(Vec::new());
        match &state.record {
            RecordState::Recording(rec) => {
                assert!(rec.sources.is_empty());
                assert!(
                    rec.device.is_none(),
                    "an empty report never invents a source line"
                );
            }
            other => panic!("expected a recording, got {other:?}"),
        }
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

    // ---- M8.5: the floating record control -----------------------------------

    #[test]
    fn the_floating_control_toggles_record_and_stop_through_the_same_effects() {
        let mut state = recording_state();
        assert!(
            !state.float_glass,
            "headless builds paint the fallback pill"
        );
        let t0 = Instant::now();

        // Idle → recording: the exact effect the titlebar dot used to emit.
        let effects = state.toggle_record(t0, "2026-10-14 09:00:00");
        assert!(matches!(effects.as_slice(), [Effect::StartCapture]));
        assert_eq!(state.route, Route::Record);
        match &state.record {
            RecordState::Recording(recording) => {
                assert_eq!(recording.title, "Recording 2026-10-14 09:00:00");
                assert!(!recording.stopping);
            }
            other => panic!("expected a recording, got {other:?}"),
        }

        // The elapsed clock ticks on the app tick while recording; the pill
        // shows the same sanctioned hh:mm:ss format as the record view (M9c).
        state.tick(t0 + Duration::from_millis(7_300));
        match &state.record {
            RecordState::Recording(recording) => {
                assert_eq!(
                    format_elapsed(recording.elapsed),
                    "00:00:07",
                    "the pill shows the live elapsed clock"
                );
            }
            other => panic!("expected a recording, got {other:?}"),
        }

        // Recording → stop: the same StopCapture the record view's button emits.
        let effects = state.toggle_record(t0 + Duration::from_millis(8_000), "ignored");
        match effects.as_slice() {
            [Effect::StopCapture { title }] => {
                assert_eq!(title, "Recording 2026-10-14 09:00:00");
            }
            other => panic!("expected a stop effect, got {other:?}"),
        }
        // A second press while the runtime finalises is a no-op.
        assert!(state.toggle_record(t0, "ignored").is_empty());
        assert!(
            state.busy(),
            "recording keeps the tick alive until it stops"
        );

        // The runtime wrote the wav → the existing job flow takes over.
        let wav = temp_wav("float-record");
        let effects = state.on_recording_stopped(wav, false);
        assert!(matches!(effects.as_slice(), [Effect::SubmitJob { .. }]));
        assert_eq!(state.route, Route::Transcribe);
    }

    #[test]
    fn the_floating_control_explains_when_recording_is_unavailable() {
        // A running job locks the control, with the old dot's status line.
        let mut state = recording_state();
        state.begin_import(temp_wav("float-busy"));
        assert!(state.toggle_record(Instant::now(), "d").is_empty());
        let status = state.status.as_ref().unwrap();
        assert!(!status.error);
        assert_eq!(status.text, "recording is disabled while a job is running");
        assert!(matches!(state.record, RecordState::Idle));

        // Without capture compiled in the control is dead too (Linux default).
        let mut plain = AppState::new();
        plain.capture_available = false;
        assert!(plain.toggle_record(Instant::now(), "d").is_empty());
        let status = plain.status.as_ref().unwrap();
        assert!(status.error);
        assert!(
            status.text.contains("not compiled"),
            "unexpected: {}",
            status.text
        );
        assert!(matches!(plain.record, RecordState::Idle));
    }

    #[test]
    fn the_recording_clock_keeps_the_event_loop_pumping() {
        // The pill's elapsed clock is driven by the frame tick, so the busy
        // predicate must hold from the moment the control starts a session
        // until the runtime has taken the recording over (the M8 playback rule).
        let mut state = recording_state();
        assert!(!state.busy(), "a parked app keeps no tick");
        state.toggle_record(Instant::now(), "2026-10-14 09:00:00");
        assert!(state.busy(), "recording must keep frames coming");

        state.stop_recording();
        assert!(
            state.busy(),
            "the finalising pass still counts as recording"
        );
        state.on_recording_stopped(temp_wav("float-clock"), false);
        assert!(state.busy(), "…and the job it hands over to is running");
    }

    // ---- M7: the live provisional transcript ---------------------------------

    #[test]
    fn provisional_transcript_grows_then_is_discarded_on_stop() {
        let mut state = recording_state();
        state.start_recording(Instant::now(), "2026-10-14 09:00:00");
        match &state.record {
            RecordState::Recording(recording) => {
                assert!(recording.provisional.is_empty());
                assert_eq!(recording.provisional_skipped, 0);
            }
            other => panic!("expected a recording, got {other:?}"),
        }

        // The live decoder answers window by window: the list grows.
        state.record_provisional(vec![segment(0, "The quick brown fox")], 0);
        state.record_provisional(
            vec![
                segment(0, "The quick brown fox"),
                segment(1, "jumps over the lazy dog"),
            ],
            1,
        );
        match &state.record {
            RecordState::Recording(recording) => {
                assert_eq!(recording.provisional.len(), 2);
                assert_eq!(recording.provisional[1].text, "jumps over the lazy dog");
                assert_eq!(
                    recording.provisional_skipped, 1,
                    "the backlog note is surfaced"
                );
            }
            other => panic!("expected a recording, got {other:?}"),
        }

        // Stop: the provisional content is ALWAYS discarded — the effect is the
        // unchanged StopCapture the job flow already knows.
        let effects = state.stop_recording();
        match effects.as_slice() {
            [Effect::StopCapture { title }] => assert_eq!(title, "Recording 2026-10-14 09:00:00"),
            other => panic!("expected a stop effect, got {other:?}"),
        }
        match &state.record {
            RecordState::Recording(recording) => {
                assert!(recording.stopping);
                assert!(
                    recording.provisional.is_empty(),
                    "provisional content is discarded on Stop"
                );
                assert_eq!(recording.provisional_skipped, 0);
            }
            other => panic!("expected a recording, got {other:?}"),
        }

        // A late live update cannot resurrect it.
        state.record_provisional(vec![segment(0, "late")], 3);
        match &state.record {
            RecordState::Recording(recording) => assert!(recording.provisional.is_empty()),
            other => panic!("expected a recording, got {other:?}"),
        }

        // The runtime wrote the wav → the existing job flow takes over.
        let wav = temp_wav("live-record");
        let effects = state.on_recording_stopped(wav, false);
        assert!(matches!(effects.as_slice(), [Effect::SubmitJob { .. }]));
        assert!(matches!(state.record, RecordState::Idle));
        assert_eq!(state.route, Route::Transcribe);
        assert!(matches!(state.job, Job::Running(_)));

        // The final transcript — and only the final transcript — reaches the
        // editor; no provisional text survives anywhere.
        let final_transcript = transcript("Recording 2026-10-14 09-00-00.wav");
        state.on_worker_event(finished_event(final_transcript.clone()));
        let editor = state
            .editor
            .as_ref()
            .expect("the editor opens from the final");
        assert_eq!(
            editor.transcript.segments.len(),
            final_transcript.segments.len()
        );
        assert!(editor
            .transcript
            .segments
            .iter()
            .all(|segment| segment.text != "late" && segment.text != "jumps over the lazy dog"));
    }

    #[test]
    fn provisional_updates_are_ignored_outside_a_recording() {
        // No recording at all (and a failed one): the runtime's live poll must
        // never invent a provisional transcript.
        let mut state = AppState::new();
        state.record_provisional(vec![segment(0, "nope")], 1);
        assert!(matches!(state.record, RecordState::Idle));

        let mut failed = recording_state();
        failed.start_recording(Instant::now(), "2026-10-14 09:00:00");
        failed.record_failed("no default input device".to_string());
        failed.record_provisional(vec![segment(0, "nope")], 1);
        assert!(matches!(failed.record, RecordState::Failed { .. }));
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
            .contains("removed 1 file"));

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
        assert!(matches!(
            effects.as_slice(),
            [Effect::RefreshAudioDevices, Effect::RefreshAudioProcesses]
        ));
        assert_eq!(state.route, Route::Settings);

        state.on_audio_devices(
            vec![
                "MacBook Microphone".to_string(),
                "BlackHole 2ch".to_string(),
            ],
            None,
        );
        assert_eq!(state.audio_devices.len(), 2);
        state.on_audio_processes(
            vec![capture::AudioProcessInfo {
                pid: 700,
                bundle_id: "us.zoom.xos".to_string(),
                is_running_output: true,
                is_running_input: true,
            }],
            None,
        );
        assert_eq!(state.audio_processes.len(), 1);
        assert_eq!(state.audio_processes[0].settings_key(), "us.zoom.xos");

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

    // ---- M9b: waveform peaks -------------------------------------------------

    #[test]
    fn opening_a_transcript_with_audio_requests_its_envelope() {
        let wav = temp_wav("peaks-open");
        let mut state = AppState::new();
        let effects = state.on_transcript_loaded(transcript_with_audio("open.wav", &wav));
        match effects.as_slice() {
            [Effect::LoadPeaks { transcript, path }] => {
                assert_eq!(*transcript, state.editor.as_ref().unwrap().transcript.id);
                assert_eq!(path, &wav);
            }
            other => panic!("expected a peaks load, got {other:?}"),
        }
        // Until the job answers, the strip draws the baseline — no fake data.
        assert!(state.strip_peaks().is_none());
        // The answer lands on the strip.
        let peaks = Peaks::from_samples(&[0.5, -0.5, 0.25, -0.25], 2);
        state.on_peaks_loaded(
            state.editor.as_ref().unwrap().transcript.id,
            Ok(peaks.clone()),
        );
        assert_eq!(state.strip_peaks(), Some(&peaks));
    }

    #[test]
    fn a_source_without_audio_draws_the_baseline_and_never_starts_a_job() {
        // keep_audio off: the recording's file is gone (the M8 unavailable path).
        let mut state = AppState::new();
        let mut recorded = transcript("Recording x.wav");
        recorded.source = Source::Recording {
            started_at: chrono::Utc::now(),
            duration_ms: 1_900,
        };
        let effects = state.on_transcript_loaded(recorded);
        assert!(
            effects.is_empty(),
            "no decode job for a source with no audio"
        );
        assert!(
            state.strip_peaks().is_none(),
            "the strip draws the flat baseline"
        );
        assert!(state.peaks.transcript.is_none());
    }

    #[test]
    fn a_stale_peaks_answer_is_discarded() {
        let first = temp_wav("peaks-stale-first");
        let second = temp_wav("peaks-stale-second");
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript_with_audio("first.wav", &first));
        let first_id = state.editor.as_ref().unwrap().transcript.id;
        state.on_transcript_loaded(transcript_with_audio("second.wav", &second));
        let second_id = state.editor.as_ref().unwrap().transcript.id;
        assert_ne!(first_id, second_id);
        let peaks = Peaks::from_samples(&[1.0], 1);
        state.on_peaks_loaded(first_id, Ok(peaks.clone()));
        assert!(
            state.strip_peaks().is_none(),
            "the stale envelope never lands"
        );
        assert!(state.peaks.peaks.is_none());
        state.on_peaks_loaded(second_id, Ok(peaks.clone()));
        assert_eq!(state.strip_peaks(), Some(&peaks));
    }

    #[test]
    fn a_failed_decode_leaves_the_baseline_and_records_the_reason() {
        let wav = temp_wav("peaks-failed");
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript_with_audio("bad.wav", &wav));
        let id = state.editor.as_ref().unwrap().transcript.id;
        state.on_peaks_loaded(id, Err("could not read the audio".to_string()));
        assert!(state.strip_peaks().is_none());
        assert_eq!(
            state.peaks.error.as_deref(),
            Some("could not read the audio")
        );
    }

    // ---- M9a: deck readout / export sheet / arrival tracking -----------------

    #[test]
    fn the_transport_readout_is_mm_ss_mmm() {
        assert_eq!(format_transport(0), "00:00.000");
        assert_eq!(format_transport(1_234), "00:01.234");
        assert_eq!(format_transport(62_480), "01:02.480");
        // Hours fold into minutes — one format, never a third clock style.
        assert_eq!(format_transport(3_661_007), "61:01.007");
    }

    // ---- M9c: copy sweep — the two time formats, counts, engine names --------

    /// Exactly TWO sanctioned time formats exist (brief §3): recordings show
    /// elapsed hh:mm:ss, the transport mm:ss.mmm — and the old variants
    /// (`mm:ss.t` tenths, bare `mm:ss`) are rejected by shape.
    #[test]
    fn the_two_sanctioned_time_formats_are_the_only_clocks() {
        // Elapsed: hh:mm:ss — two colons, no dot, hours never fold into minutes.
        assert_eq!(format_elapsed(Duration::ZERO), "00:00:00");
        assert_eq!(format_elapsed(Duration::from_millis(7_300)), "00:00:07");
        assert_eq!(format_elapsed(Duration::from_millis(62_040)), "00:01:02");
        assert_eq!(format_elapsed(Duration::from_millis(3_723_456)), "01:02:03");
        assert_eq!(format_elapsed(Duration::from_millis(4_200_000)), "01:10:00");
        for elapsed in [
            Duration::ZERO,
            Duration::from_millis(7_300),
            Duration::from_millis(62_040),
            Duration::from_millis(3_723_456),
        ] {
            let text = format_elapsed(elapsed);
            assert!(
                !text.contains('.') && text.matches(':').count() == 2,
                "elapsed {text:?} must be hh:mm:ss (the mm:ss.t variant is gone)"
            );
        }
        // Transport: mm:ss.mmm — one colon, exactly three fractional digits,
        // never an hours field (the old `mm:ss` readout is rejected by shape).
        for ms in [0u32, 1_234, 62_480, 3_661_007] {
            let text = format_transport(ms);
            let (whole, frac) = text.split_once('.').expect("a millisecond dot");
            assert_eq!(frac.len(), 3, "transport {text:?} has three millis digits");
            assert_eq!(whole.matches(':').count(), 1, "transport {text:?} is mm:ss");
        }
        // The old shapes do not come out of either function.
        assert_ne!(format_elapsed(Duration::from_millis(7_300)), "00:07.3");
        assert_ne!(format_transport(7_000), "00:07");
    }

    #[test]
    fn counts_are_pluralized_never_bracketed() {
        assert_eq!(count_noun(0, "segment"), "0 segments");
        assert_eq!(count_noun(1, "segment"), "1 segment");
        assert_eq!(count_noun(5, "segment"), "5 segments");
        assert_eq!(count_noun(1, "file"), "1 file");
        assert_eq!(count_noun(2, "window"), "2 windows");
        assert_eq!(count_noun(1, "window"), "1 window");
        assert_eq!(count_noun(3, "device"), "3 devices");
        // The bracketed shape is gone everywhere this helper is used.
        for count in [0usize, 1, 2, 96] {
            for noun in ["segment", "file", "window", "device"] {
                let text = count_noun(count, noun);
                assert!(!text.contains("(s)"), "no (s) suffix in {text:?}");
            }
        }
    }

    #[test]
    fn engine_ids_render_as_human_names() {
        assert_eq!(engine_display("cpu"), "CPU");
        assert_eq!(engine_display("metal"), "Metal");
        assert_eq!(engine_display("coreml"), "Core ML");
        // Unknown strings pass through untouched.
        assert_eq!(engine_display("auto"), "auto");
        assert_eq!(engine_display(""), "");
    }

    // ---- M9c: list motion (insert / remove ease) -----------------------------

    #[test]
    fn the_first_library_load_is_static_then_inserts_and_removals_animate() {
        let mut state = AppState::new();
        let first = transcript("first.wav");
        let second = transcript("second.wav");

        // The first load renders statically: no arrivals, nothing leaving.
        state.on_library_loaded(vec![first.clone(), second.clone()], None);
        assert!(state.library.loaded_once);
        assert!(
            state.library.arrivals.is_empty(),
            "no arrival on first load"
        );
        assert!(state.library.leaving.is_empty());

        // An insert (a saved job / a new recording) gets exactly one arrival.
        let third = transcript("third.wav");
        state.on_library_loaded(vec![third.clone(), first.clone(), second.clone()], None);
        assert_eq!(state.library.arrivals.len(), 1, "only the new row arrives");
        assert_eq!(state.library.arrivals[0].0, third.id);
        let progress = state
            .library
            .row_arrival(third.id, Instant::now(), crate::motion::BASE);
        assert!(progress.is_some(), "the new row is mid-arrival");
        assert!(
            state
                .library
                .row_arrival(first.id, Instant::now(), crate::motion::BASE)
                .is_none(),
            "an existing row never replays its arrival"
        );

        // A removal (delete) keeps the row as a leaving ghost at its old index.
        // (third's insert arrival from the previous load may still be in
        // flight — the token has not elapsed — but no NEW arrival is stamped.)
        state.on_library_loaded(vec![third.clone(), first.clone()], None);
        assert_eq!(state.library.leaving.len(), 1);
        assert_eq!(state.library.leaving[0].item.id, second.id);
        assert_eq!(state.library.leaving[0].index, 2, "its old draw position");
        assert_eq!(
            state.library.arrivals.len(),
            1,
            "still only third's insert arrival"
        );
        assert_eq!(state.library.arrivals[0].0, third.id);

        // Once the token elapsed, the bookkeeping is pruned (animations end).
        let later = Instant::now() + Duration::from_millis(500);
        state
            .library
            .prune_motion(later, Duration::from_millis(180));
        assert!(state.library.leaving.is_empty());
        assert!(state.library.arrivals.is_empty());
    }

    #[test]
    fn a_refresh_that_errored_animates_nothing() {
        let mut state = AppState::new();
        state.on_library_loaded(vec![transcript("kept.wav")], None);
        state.on_library_loaded(Vec::new(), Some("database is locked".into()));
        assert!(
            state.library.leaving.is_empty(),
            "an errored refresh must not animate rows away"
        );
        assert!(state.library.arrivals.is_empty());
    }

    #[test]
    fn list_slots_merge_leaving_rows_at_their_old_positions() {
        use crate::app::library::{list_slots, ListSlot};
        let mut state = AppState::new();
        // The same transcript objects across loads (same ids — a refresh that
        // returns new ids would be an insert + a removal, which is correct).
        let a = transcript("a.wav");
        let b = transcript("b.wav");
        let c = transcript("c.wav");
        state.on_library_loaded(vec![a.clone(), b.clone(), c.clone()], None);
        // Delete b: the new list is [a, c]; b leaves at index 1.
        state.on_library_loaded(vec![a.clone(), c.clone()], None);
        assert_eq!(state.library.leaving.len(), 1);
        assert!(state.library.arrivals.is_empty(), "no new rows");
        let slots = list_slots(&state.library, None);
        // a(0) → b-leaving(1) → c(1): the ghost keeps its slot while the live
        // row that took the index follows it, so c slides up as b collapses.
        assert_eq!(slots.len(), 3);
        assert_eq!(slots[0], ListSlot::Live(0));
        assert_eq!(slots[1], ListSlot::Leaving(0));
        assert_eq!(slots[2], ListSlot::Live(1));
        // The home recents cap applies to live rows and to ghosts that were
        // visible within it: cap 2 keeps the ghost (old index 1 < 2), cap 1
        // drops it (it lived beyond the visible rows).
        assert_eq!(list_slots(&state.library, Some(2)).len(), 3);
        assert_eq!(list_slots(&state.library, Some(1)).len(), 1);
    }

    // ---- M9c: settings confirm ----------------------------------------------

    #[test]
    fn a_successful_settings_save_arms_the_applied_confirm() {
        let mut state = AppState::new();
        assert!(state.settings_form.applied_at.is_none());
        state.on_settings_saved(Ok("settings saved".to_string()));
        assert!(
            state.settings_form.applied_at.is_some(),
            "Ok arms the one-shot confirm pulse"
        );
        // A failed save never arms it (the error line is the message).
        let mut failed = AppState::new();
        failed.on_settings_saved(Err("disk full".to_string()));
        assert!(failed.settings_form.applied_at.is_none());
    }

    #[test]
    fn the_export_sheet_toggles_and_closes_when_the_editor_is_left() {
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript("sheet.wav"));
        assert!(!state.export.open, "closed when a transcript opens");
        state.toggle_export();
        assert!(state.export.open);
        state.toggle_export();
        assert!(!state.export.open);
        state.toggle_export();
        let _ = state.back_home();
        assert!(!state.export.open, "leaving the editor closes the sheet");
    }

    #[test]
    fn provisional_arrivals_track_first_seen_per_segment() {
        let mut state = recording_state();
        state.start_recording(Instant::now(), "2026-10-14 09:41:02");
        state.record_provisional(vec![segment(0, "one")], 0);
        let first = match &state.record {
            RecordState::Recording(rec) => rec.provisional_arrivals.clone(),
            other => panic!("expected a recording, got {other:?}"),
        };
        assert_eq!(first.len(), 1);

        // A later update keeps segment 0's arrival and stamps the new segment.
        std::thread::sleep(std::time::Duration::from_millis(5));
        state.record_provisional(vec![segment(0, "one"), segment(1, "two")], 0);
        match &state.record {
            RecordState::Recording(rec) => {
                assert_eq!(rec.provisional_arrivals.len(), 2);
                assert_eq!(
                    rec.provisional_arrivals[0], first[0],
                    "an existing segment keeps its arrival instant"
                );
                assert!(
                    rec.provisional_arrivals[1] >= first[0],
                    "a new segment is stamped now"
                );
            }
            other => panic!("expected a recording, got {other:?}"),
        }

        // Stop clears the arrival bookkeeping with the provisional list.
        state.stop_recording();
        match &state.record {
            RecordState::Recording(rec) => {
                assert!(rec.provisional_arrivals.is_empty());
                assert!(rec.provisional.is_empty());
            }
            other => panic!("expected a recording, got {other:?}"),
        }
    }

    #[test]
    fn the_demo_recording_seed_shapes_a_real_session_state() {
        let mut state = AppState::new();
        state.seed_demo_recording();
        assert_eq!(state.route, Route::Record);
        assert!(state.busy(), "a live recording keeps the tick alive");
        match &state.record {
            RecordState::Recording(rec) => {
                assert_eq!(rec.waveform.len(), RECORD_WAVEFORM_BARS);
                assert_eq!(rec.sources.len(), 2);
                assert_eq!(rec.provisional.len(), 3);
                assert_eq!(rec.provisional_arrivals.len(), 3);
                assert!(!rec.stopping);
                assert!(rec.device.as_deref().unwrap().contains("System audio"));
            }
            other => panic!("expected a recording, got {other:?}"),
        }
    }
}
