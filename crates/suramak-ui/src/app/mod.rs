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

use suramak_asr::worker::WorkerEvent;
use suramak_asr::EngineId;
use suramak_core::export as core_export;
use suramak_core::models;
use suramak_core::settings::EngineSetting;
use suramak_core::transcript::Transcript;

pub mod editor;
pub mod export;
pub mod home;
pub mod library;
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
        }
    }

    /// Effects to run once at startup (library list from the store).
    pub fn initial_effects(&self) -> Vec<Effect> {
        vec![Effect::RefreshLibrary]
    }

    /// Whether the UI wants the periodic tick (job running / dirty edits).
    pub fn busy(&self) -> bool {
        matches!(self.job, Job::Running(_)) || self.editor.as_ref().is_some_and(|e| e.dirty)
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

    /// Import request from a drop, the dialog or the path field. Validates the
    /// path, switches to the transcribe route and asks the runtime to submit.
    pub fn begin_import(&mut self, input: PathBuf) -> Vec<Effect> {
        if !input.is_file() {
            self.home.error = Some(format!("file not found: {}", input.display()));
            return Vec::new();
        }
        self.home.error = None;
        self.status = None;
        self.last_insert = None;
        self.route = Route::Transcribe;
        self.job = Job::Running(RunningJob {
            input: input.clone(),
            pct: 0,
            model: None,
            requested_engine: None,
            planned_engine: None,
            warnings: Vec::new(),
        });
        vec![Effect::SubmitJob { input }]
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
                let text = match engine_line {
                    Some(line) => format!("{line} — saved to library: {}", transcript.id),
                    None => format!("saved to library: {}", transcript.id),
                };
                self.set_status(text, false);
                self.last_insert = Some(SaveOutcome {
                    transcript,
                    error: None,
                });
                vec![Effect::RefreshLibrary]
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
        let mut effects = self.flush_edits();
        effects.push(Effect::LoadTranscript(Box::new(item)));
        effects
    }

    /// Transcript loaded from the store.
    pub fn on_transcript_loaded(&mut self, transcript: Transcript) -> Vec<Effect> {
        self.open_editor(transcript);
        Vec::new()
    }

    /// Back to the home route (pending edits are flushed first).
    pub fn back_home(&mut self) -> Vec<Effect> {
        self.route = Route::Home;
        self.flush_edits()
    }

    /// The periodic tick: autosave debounce (and nothing else, for now).
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
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
        self.route = Route::Editor;
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
        models::find_model(id)
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
}
