//! The compact profile's state machine — the phone's single source of truth.
//!
//! It is deliberately independent of the desktop `AppState`: the desktop state
//! machine owns capture devices, playback engines, job queues and export
//! dialogs that a phone shell does not have. What it **does** share with the
//! desktop is the data (`suramak_core::Transcript`, `Settings`, the model
//! manifest) and the presentation vocabulary (Deck tokens, motion, painted
//! controls). Every action the screens emit is a [`CompactAction`] the host
//! executes against the real store; nothing here writes to disk.

use std::collections::BTreeMap;

use chrono::{DateTime, Local};
use suramak_core::device::{self, DeviceProfile, PhoneTier};
use suramak_core::models::MODELS;
use suramak_core::settings::Appearance;
use suramak_core::transcript::{Segment, Transcript};
use uuid::Uuid;

use crate::deck::nav::{BackOutcome, NavStack};
use crate::deck::theme::AppTheme;

use super::motion::{ListAnim, ProgressAnim, SheetAnim};

/// The three screens of the G1 compact profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactRoute {
    /// The library: every transcript, search, import/record entry.
    Library,
    /// One transcript: read, edit, search, play, export.
    Editor,
    /// Engine/model/language/appearance/storage, plus Quit.
    Settings,
}

/// One row in the compact library.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryRow {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTime<Local>,
    pub segments: usize,
    pub duration_ms: i64,
    pub language: Option<String>,
}

impl LibraryRow {
    pub fn from_transcript(transcript: &Transcript) -> Self {
        let duration_ms = transcript
            .segments
            .iter()
            .map(|segment| segment.end_ms)
            .max()
            .unwrap_or(0);
        Self {
            id: transcript.id,
            title: transcript.title.clone(),
            created_at: transcript.created_at.with_timezone(&Local),
            segments: transcript.segments.len(),
            duration_ms,
            language: transcript.language.clone(),
        }
    }

    /// `"12 Mar · 4:32 · 21 segments"` — the phone row's one meta line.
    pub fn meta(&self) -> String {
        format!(
            "{} · {} · {}",
            self.created_at.format("%-d %b"),
            clock(self.duration_ms),
            crate::deck::compact::count_noun(self.segments, "segment")
        )
    }
}

/// `m:ss` (or `h:mm:ss` past an hour) — the phone's compact time format.
pub fn clock(ms: i64) -> String {
    let total = (ms.max(0) / 1000) as u32;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `1 segment` / `3 segments` (desktop's `count_noun`, phone-sized).
pub fn count_noun(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// A search hit in the library: the transcript and the first matching segment.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryHit {
    pub id: Uuid,
    pub title: String,
    pub snippet: String,
    pub segment_idx: Option<u32>,
    pub start_ms: i64,
}

/// The compact export formats — the desktop export list's set, in the same
/// order (Markdown, Text, JSON, SRT, VTT, TSV), minus ASS: ASS is a desktop
/// extra the phone does not offer. Every format is rendered by the same
/// `suramak_core::export` function the desktop calls, so the phone writes the
/// exact bytes the desktop writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactExportFormat {
    Md,
    Txt,
    Json,
    Srt,
    Vtt,
    Tsv,
}

impl CompactExportFormat {
    pub const ALL: [Self; 6] = [
        Self::Md,
        Self::Txt,
        Self::Json,
        Self::Srt,
        Self::Vtt,
        Self::Tsv,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Md => "Markdown",
            Self::Txt => "Text",
            Self::Json => "JSON",
            Self::Srt => "SRT",
            Self::Vtt => "VTT",
            Self::Tsv => "TSV",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Md => "md",
            Self::Txt => "txt",
            Self::Json => "json",
            Self::Srt => "srt",
            Self::Vtt => "vtt",
            Self::Tsv => "tsv",
        }
    }

    /// The MIME type `ACTION_CREATE_DOCUMENT` advertises. `text/*` where the
    /// platform has a canonical type; the subtitle types are the ones players
    /// register for.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Md => "text/markdown",
            Self::Txt => "text/plain",
            Self::Json => "application/json",
            Self::Srt => "application/x-subrip",
            Self::Vtt => "text/vtt",
            Self::Tsv => "text/tab-separated-values",
        }
    }

    /// Whether the stacked bilingual variant exists (SRT and VTT; ASS is not
    /// offered on the phone).
    pub fn supports_bilingual(self) -> bool {
        matches!(self, Self::Srt | Self::Vtt)
    }
}

/// The export sheet's phase — the "picker result path" is a modelled state, not
/// a platform callback the UI infers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportPhase {
    /// `ACTION_CREATE_DOCUMENT` / the share chooser is on screen.
    Choosing,
    /// The destination is chosen; the host is writing and verifying.
    Writing,
    /// The file is at the chosen frame, bytes verified.
    Written,
    /// Handed to another app through `ACTION_SEND`.
    Shared,
    Failed,
}

/// One export attempt, bound to its transcript and format.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSession {
    pub transcript: Uuid,
    pub format: CompactExportFormat,
    pub bilingual: bool,
    pub phase: ExportPhase,
    /// The suggested file name (also the staged file's name).
    pub name: String,
    /// Bytes written, once known.
    pub bytes: Option<u64>,
    /// The honest failure line, when the phase is [`ExportPhase::Failed`].
    pub error: Option<String>,
}

impl ExportSession {
    pub fn new(
        transcript: Uuid,
        format: CompactExportFormat,
        bilingual: bool,
        name: String,
    ) -> Self {
        Self {
            transcript,
            format,
            bilingual,
            phase: ExportPhase::Choosing,
            name,
            bytes: None,
            error: None,
        }
    }

    /// Whether the sheet should show the working/result state rather than the
    /// format chips.
    pub fn busy_or_done(&self) -> bool {
        !matches!(self.phase, ExportPhase::Choosing)
    }
}

/// A fling observation in progress: one pointer release, then the scroll
/// offset's own decay.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct ActiveFling {
    velocity: f32,
    distance: f32,
    frames: u32,
    last_offset: f32,
}

/// Watches a scroll area's drag release and measures what egui's momentum did
/// with it. This is the "probe" half of the fling decision: on the host it
/// measures nothing useful (no touch), on the owner's phone it records the
/// gesture's actual decay next to the Deck layer's expectation.
#[derive(Debug, Clone, Default)]
pub struct FlingTracker {
    last_offset: f32,
    last_velocity: f32,
    active: Option<ActiveFling>,
}

impl FlingTracker {
    /// Feeds one frame. A drag frame records the pointer's velocity; the
    /// release frame starts the probe with the last drag velocity (egui zeroes
    /// the pointer velocity as the finger lifts, so the release frame alone
    /// would measure nothing). The returned probe lands when the offset has
    /// stopped moving (or after 180 frames, so a probe can never run forever).
    pub fn observe(&mut self, offset: f32, velocity: f32, released: bool) -> Option<FlingProbe> {
        if velocity > 0.0 {
            self.last_velocity = velocity;
        }
        if released {
            let start = self.last_velocity.max(velocity);
            self.last_velocity = 0.0;
            if start > 40.0 && self.active.is_none() {
                self.active = Some(ActiveFling {
                    velocity: start,
                    distance: 0.0,
                    frames: 0,
                    last_offset: offset,
                });
            }
            self.last_offset = offset;
            return None;
        }
        let Some(mut active) = self.active.take() else {
            self.last_offset = offset;
            return None;
        };
        let delta = (offset - active.last_offset).abs();
        active.distance += delta;
        active.frames += 1;
        active.last_offset = offset;
        if delta < 0.5 || active.frames >= 180 {
            self.last_offset = offset;
            return Some(FlingProbe {
                release_velocity: active.velocity,
                distance: active.distance,
                frames: active.frames,
            });
        }
        self.active = Some(active);
        self.last_offset = offset;
        None
    }

    /// The offset the tracker last saw (the caller may diff against it).
    pub fn last_offset(&self) -> f32 {
        self.last_offset
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }
}

/// A running transcription job, as the host reports it (M16.1's bounded slot is
/// the producer; this is only the display).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobView {
    pub label: String,
    pub done: u32,
    pub total: u32,
}

impl JobView {
    pub fn percent(&self) -> u8 {
        if self.total == 0 {
            return 0;
        }
        ((self.done.min(self.total) as f32 / self.total as f32) * 100.0).round() as u8
    }

    pub fn line(&self) -> String {
        format!("{} · {}%", self.label, self.percent())
    }
}

/// One fling observation from the compact list (owner-device calibration).
///
/// egui's `ScrollArea` owns the scroll offset and its momentum; this records
/// what the gesture delivered and how far the list travelled afterwards so the
/// owner can compare it with the Deck motion layer's expected curve. Nothing
/// here changes scrolling — the report states why (the probe argument).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FlingProbe {
    /// Pointer velocity at release, points/second (absolute).
    pub release_velocity: f32,
    /// Distance the scroll offset moved after release.
    pub distance: f32,
    /// Frames the list kept moving after release.
    pub frames: u32,
}

impl FlingProbe {
    /// egui's default scroll animation decays over roughly this fraction of a
    /// second; the expected travel is the release velocity times it. The report
    /// cites the measured field, not this estimate.
    pub const EXPECTED_DECAY: f32 = 0.12;

    pub fn expected_distance(&self) -> f32 {
        self.release_velocity * Self::EXPECTED_DECAY
    }

    /// A one-line log/summary the owner checklist can read back.
    pub fn summary(&self) -> String {
        format!(
            "fling {:.0} pt/s → {:.0} pt over {} frames (egui expectation {:.0} pt)",
            self.release_velocity,
            self.distance,
            self.frames,
            self.expected_distance()
        )
    }
}

/// A settings change the host applies to `settings.toml`.
#[derive(Debug, Clone, PartialEq)]
pub enum CompactSetting {
    Model(String),
    Language(String),
    Appearance(Appearance),
    KeepAudio(bool),
}

/// One model row in Settings, derived from the core manifest + device tier.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRow {
    pub id: String,
    pub size_bytes: u64,
    pub role: String,
    pub note: String,
    /// Whether this model runs on the device at all (M13D's refusal rule).
    pub offered: bool,
    /// Whether the host found its file in `files/models`.
    pub installed: bool,
    /// The model the action should select/download.
    pub recommended: bool,
}

/// A transcript edit the host persists.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentEdit {
    pub transcript: Uuid,
    pub idx: u32,
    pub text: String,
}

/// Everything the UI asks the host to do.
#[derive(Debug, Clone, PartialEq)]
pub enum CompactAction {
    /// Load a transcript from the store, then call [`CompactApp::open_editor`].
    /// `segment` is the search hit to park the playhead on (the desktop's
    /// `open transcript and seek` behaviour).
    OpenTranscript {
        id: Uuid,
        segment: Option<u32>,
    },
    /// Re-read the library after a load error.
    ReloadLibrary,
    /// Persist one edited segment (the desktop's update path, one row).
    SaveSegment(SegmentEdit),
    /// Write an export through `ACTION_CREATE_DOCUMENT`.
    Export {
        transcript: Uuid,
        format: CompactExportFormat,
        bilingual: bool,
        suggested_name: String,
    },
    /// The one-file `ACTION_SEND` alternative: stage the same bytes and hand
    /// that one file to the share chooser (disclosed in the sheet).
    ShareExport {
        transcript: Uuid,
        format: CompactExportFormat,
        bilingual: bool,
        suggested_name: String,
    },
    /// The library's Import button (SAF / share).
    Import,
    /// The library's Record button.
    Record,
    /// Apply a setting; the host writes it and refreshes the store cache.
    Setting(CompactSetting),
    DownloadModel(String),
    DeleteModel(String),
    /// Quit: the host runs capture → jobs → services → exit.
    Quit,
    /// Back at the root: the host finishes the Activity.
    Finish,
}

/// Which compact sheet is open above the stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sheet {
    Export,
    Quit,
}

/// Editor state for the open transcript.
#[derive(Debug, Clone)]
pub struct CompactEditor {
    pub transcript: Transcript,
    pub selected: Option<u32>,
    /// The inline editor's buffer while a segment is being edited (set by
    /// [`CompactEditor::begin_edit`], consumed by Save/Cancel).
    pub edit_buffer: Option<(u32, String)>,
    drafts: BTreeMap<u32, String>,
    /// Segment indices whose draft differs from the stored text.
    pub dirty: Vec<u32>,
    pub search: String,
    pub search_open: bool,
    pub format: CompactExportFormat,
    pub playing: bool,
    pub playhead_ms: i64,
    /// The search hit the editor jumped to (a transient highlight).
    pub focus_segment: Option<u32>,
    /// Whether the transcript has at least one stored translation (seeded by
    /// the host from `Store::segment_translations`). The bilingual export
    /// switch is offered only when there is a second line to stack.
    pub has_translation: bool,
}

impl CompactEditor {
    pub fn new(transcript: Transcript) -> Self {
        Self {
            transcript,
            selected: None,
            edit_buffer: None,
            drafts: BTreeMap::new(),
            dirty: Vec::new(),
            search: String::new(),
            search_open: false,
            format: CompactExportFormat::Md,
            playing: false,
            playhead_ms: 0,
            focus_segment: None,
            has_translation: false,
        }
    }

    /// The text the row paints: the draft when one exists, else the stored one.
    pub fn text(&self, segment: &Segment) -> String {
        self.drafts
            .get(&segment.idx)
            .cloned()
            .unwrap_or_else(|| segment.text.clone())
    }

    /// Edits the selected segment's draft. Empty text is allowed (the desktop
    /// lets a reader clear a line); Save persists it.
    pub fn edit(&mut self, idx: u32, text: String) {
        if !self.transcript.segments.iter().any(|s| s.idx == idx) {
            return;
        }
        let stored = self
            .transcript
            .segments
            .iter()
            .find(|s| s.idx == idx)
            .map(|s| s.text.as_str())
            .unwrap_or_default();
        if text == stored {
            self.drafts.remove(&idx);
            self.dirty.retain(|dirty| *dirty != idx);
            return;
        }
        self.drafts.insert(idx, text);
        if !self.dirty.contains(&idx) {
            self.dirty.push(idx);
        }
        self.dirty.sort_unstable();
    }

    /// Commits every draft into the in-memory transcript and returns the rows
    /// the host must persist. The dirty list empties only when the caller's
    /// save succeeds — [`CompactApp::on_saved`] is that acknowledgement.
    pub fn pending_edits(&self) -> Vec<SegmentEdit> {
        self.dirty
            .iter()
            .filter_map(|idx| {
                let text = self.drafts.get(idx)?;
                Some(SegmentEdit {
                    transcript: self.transcript.id,
                    idx: *idx,
                    text: text.clone(),
                })
            })
            .collect()
    }

    /// The host confirmed the save: fold the drafts into the transcript.
    pub fn on_saved(&mut self) {
        for segment in &mut self.transcript.segments {
            if let Some(text) = self.drafts.remove(&segment.idx) {
                segment.text = text;
            }
        }
        self.dirty.clear();
        self.transcript.updated_at = chrono::Utc::now();
    }

    /// Segment indices the current search matches (all when the query is empty).
    pub fn matches(&self) -> Vec<usize> {
        let needle = self.search.trim().to_lowercase();
        self.transcript
            .segments
            .iter()
            .enumerate()
            .filter(|(_, segment)| {
                needle.is_empty() || self.text(segment).to_lowercase().contains(&needle)
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// Tap on a segment: select it and park the playhead there. Selecting a
    /// segment also closes the search focus so the keyboard can go away.
    pub fn select(&mut self, idx: u32) {
        self.selected = Some(idx);
        if let Some(segment) = self.transcript.segments.iter().find(|s| s.idx == idx) {
            self.playhead_ms = segment.start_ms;
        }
    }

    /// Starts the inline editor on `idx` with the text the row currently shows.
    pub fn begin_edit(&mut self, idx: u32) {
        let Some(segment) = self.transcript.segments.iter().find(|s| s.idx == idx) else {
            return;
        };
        let text = self.text(segment);
        self.selected = Some(idx);
        self.edit_buffer = Some((idx, text));
    }

    /// Commits the inline editor's buffer into the draft (not yet the store).
    pub fn commit_edit_buffer(&mut self) {
        if let Some((idx, text)) = self.edit_buffer.take() {
            self.edit(idx, text);
        }
    }

    /// Abandons the inline editor without touching the draft.
    pub fn cancel_edit(&mut self) {
        self.edit_buffer = None;
    }

    /// A tap on the speaker glyph: seek to the segment's start.
    pub fn seek(&mut self, idx: u32) {
        if let Some(segment) = self.transcript.segments.iter().find(|s| s.idx == idx) {
            self.playhead_ms = segment.start_ms;
            self.focus_segment = Some(idx);
        }
    }

    /// The export request for the current transcript. `bilingual` adds the
    /// stacked translation (SRT/VTT only; the sheet gates the switch).
    pub fn export_request(&self, bilingual: bool) -> CompactAction {
        CompactAction::Export {
            transcript: self.transcript.id,
            format: self.format,
            bilingual: bilingual && self.format.supports_bilingual(),
            suggested_name: self.suggested_name(),
        }
    }

    /// The one-file share request (same bytes, `ACTION_SEND`).
    pub fn share_request(&self, bilingual: bool) -> CompactAction {
        CompactAction::ShareExport {
            transcript: self.transcript.id,
            format: self.format,
            bilingual: bilingual && self.format.supports_bilingual(),
            suggested_name: self.suggested_name(),
        }
    }

    /// The file name the sheet and the host use for this format.
    pub fn suggested_name(&self) -> String {
        format!(
            "{}.{}",
            sanitise_stem(&self.transcript.title),
            self.format.extension()
        )
    }

    /// Whether anything is unsaved (drives the Save affordance + back guard).
    pub fn has_unsaved(&self) -> bool {
        !self.dirty.is_empty()
    }
}

/// A filesystem-safe stem for `ACTION_CREATE_DOCUMENT`'s suggestion.
pub fn sanitise_stem(title: &str) -> String {
    let mut out: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                ' '
            }
        })
        .collect();
    while out.contains("  ") {
        out = out.replace("  ", " ");
    }
    let trimmed = out.trim();
    if trimmed.is_empty() {
        "transcript".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The compact app: the nav stack, the library, the open editor and Settings.
#[derive(Debug, Clone)]
pub struct CompactApp {
    pub nav: NavStack<CompactRoute>,
    /// The loaded library, in the store's order. The rows are derived from it;
    /// search reads the same values, so a hit can never disagree with a row.
    pub transcripts: Vec<Transcript>,
    pub rows: Vec<LibraryRow>,
    pub library_query: String,
    /// Library load failure, shown as an honest note (never a fake empty list).
    pub library_error: Option<String>,
    pub editor: Option<CompactEditor>,
    pub sheet: Option<Sheet>,
    pub settings: CompactSettings,
    pub models: Vec<ModelRow>,
    /// The export sheet's flow (picker → write → result), when one is open.
    pub export_session: Option<ExportSession>,
    /// The sheet's "include the translation" switch.
    pub export_bilingual: bool,
    /// The sheet's present/dismiss animation.
    pub sheet_anim: SheetAnim,
    /// The library's insert/delete animation.
    pub list_anim: ListAnim,
    /// The running job, as last reported by the host.
    pub job: Option<JobView>,
    /// The job's eased progress fill.
    pub job_progress: ProgressAnim,
    /// A job event landed and the host should wake the surface exactly once.
    frame_request: bool,
    /// The last fling observation (owner-device calibration).
    pub fling_probe: Option<FlingProbe>,
    /// The scroll gesture watcher (the probe's input side).
    pub fling: FlingTracker,
    /// A one-shot Settings scroll offset (the tier screenshot / "scroll to the
    /// model table" affordance).
    pub settings_scroll: Option<f32>,
    /// `None` when the shell has not reported memory: the Settings card says so
    /// instead of pretending a tier.
    pub tier: Option<PhoneTier>,
    pub ram_bytes: Option<u64>,
    pub status: Option<(String, bool)>,
    /// A system-bar inset in points (the Kotlin shim reports pixels; the host
    /// converts with the same pixels_per_point the surface uses).
    pub insets: Insets,
    pub reduce_motion: bool,
    /// The resolved token ladder (the host resolves System against the OS).
    pub theme: AppTheme,
    pub version: String,
    /// Deep link that arrived before the library row could be loaded.
    pub pending_open: Option<Uuid>,
}

/// Window insets, in logical points.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Insets {
    pub top: f32,
    pub bottom: f32,
}

/// The settings the compact surface edits (seeded from `suramak_core::Settings`).
#[derive(Debug, Clone, PartialEq)]
pub struct CompactSettings {
    pub model_id: String,
    pub language: String,
    pub appearance: Appearance,
    pub keep_audio: bool,
}

impl Default for CompactSettings {
    fn default() -> Self {
        let defaults = suramak_core::settings::Settings::default();
        Self {
            model_id: defaults.asr.model_id,
            language: "auto".to_string(),
            appearance: defaults.ui.appearance,
            keep_audio: defaults.storage.keep_audio,
        }
    }
}

impl CompactApp {
    /// A new compact app on the Library root, static on its first frame.
    pub fn new(settings: CompactSettings) -> Self {
        Self {
            nav: NavStack::new(CompactRoute::Library),
            transcripts: Vec::new(),
            rows: Vec::new(),
            library_query: String::new(),
            library_error: None,
            editor: None,
            sheet: None,
            settings,
            models: Vec::new(),
            export_session: None,
            export_bilingual: false,
            sheet_anim: SheetAnim::closed(),
            list_anim: ListAnim::new(),
            job: None,
            job_progress: ProgressAnim::new(),
            frame_request: false,
            fling_probe: None,
            fling: FlingTracker::default(),
            settings_scroll: None,
            tier: None,
            ram_bytes: None,
            status: None,
            insets: Insets::default(),
            reduce_motion: false,
            theme: AppTheme::Dark,
            version: env!("CARGO_PKG_VERSION").to_string(),
            pending_open: None,
        }
    }

    /// Seeds the device facts and derives the model rows through M13D's rules.
    pub fn seed_device(&mut self, profile: DeviceProfile, installed: &[String]) {
        self.tier = Some(device::tier_for(profile));
        self.ram_bytes = Some(profile.total_ram_bytes);
        self.models = model_rows(profile, &self.settings.language, installed);
    }

    /// Replaces the library list (a fresh `Store::list_transcripts`).
    pub fn set_library(&mut self, transcripts: Vec<Transcript>) {
        self.rows = transcripts
            .iter()
            .map(LibraryRow::from_transcript)
            .collect();
        self.transcripts = transcripts;
        self.library_error = None;
    }

    pub fn set_library_error(&mut self, error: impl Into<String>) {
        self.library_error = Some(error.into());
        self.rows.clear();
        self.transcripts.clear();
    }

    /// Applies a library query (the field's text).
    pub fn set_library_query(&mut self, query: impl Into<String>) {
        self.library_query = query.into();
    }

    /// Ranked-in-order search hits over the loaded transcripts. The query is
    /// matched case-insensitively against the title (a title hit ranks first)
    /// and then each segment; the first matching segment is the snippet. This
    /// is the phone's search: the same data as the desktop's FTS panel, with a
    /// deterministic in-memory rule (the library is already loaded for the
    /// list, and a phone library is small enough that this never needs FTS).
    pub fn hits(&self) -> Vec<LibraryHit> {
        let needle = self.library_query.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        for transcript in &self.transcripts {
            let title_matches = transcript.title.to_lowercase().contains(&needle);
            let segment = transcript
                .segments
                .iter()
                .find(|segment| segment.text.to_lowercase().contains(&needle));
            if !title_matches && segment.is_none() {
                continue;
            }
            let (idx, start, snippet) = match segment {
                Some(segment) => (
                    Some(segment.idx),
                    segment.start_ms,
                    snippet(&segment.text, &needle),
                ),
                None => (None, 0, transcript.title.clone()),
            };
            hits.push(LibraryHit {
                id: transcript.id,
                title: transcript.title.clone(),
                snippet,
                segment_idx: idx,
                start_ms: start,
            });
        }
        // Title hits first, then by start time — stable and explainable.
        hits.sort_by_key(|hit| (hit.segment_idx.is_some(), hit.start_ms));
        hits
    }

    /// Opens the editor on a loaded transcript (Library stays below it).
    pub fn open_editor(&mut self, transcript: Transcript, now: f64) {
        self.editor = Some(CompactEditor::new(transcript));
        self.nav.push(CompactRoute::Editor, now);
    }

    /// The notification's landing: Library, then the finished transcript's
    /// editor, in one motion. This is the path M13E's `ACTION_OPEN_TRANSCRIPT`
    /// intent feeds through the platform event.
    pub fn deep_link(&mut self, transcript: Transcript, now: f64) {
        self.sheet = None;
        self.pending_open = None;
        self.editor = Some(CompactEditor::new(transcript));
        self.nav
            .open_deep(CompactRoute::Library, CompactRoute::Editor, now);
    }

    /// The notification named a transcript that is no longer in the library:
    /// land on the Library with an honest line, never an empty editor.
    pub fn deep_link_missing(&mut self, id: Uuid, _now: f64) {
        self.pending_open = Some(id);
        self.editor = None;
        self.sheet = None;
        self.nav = NavStack::new(CompactRoute::Library);
        self.status = Some((
            "That transcript is no longer in the library.".to_string(),
            false,
        ));
    }

    /// Opens Settings (a push; back returns to the Library).
    pub fn open_settings(&mut self, now: f64) {
        self.sheet = None;
        self.nav.push(CompactRoute::Settings, now);
    }

    /// The back gesture. A sheet dismisses (with its exit animation), then the
    /// stack pops; at the root the host finishes the Activity (v2.4's
    /// `nativeBackPressed` funnel).
    ///
    /// **Predictive back mid-transition.** The gesture is answered from this
    /// one place at the moment it arrives, never queued: if a push/pop spring
    /// is in flight, the gesture's target is the screen the stack says is on
    /// top now, so a back during a push pops that screen immediately (the
    /// in-flight push is retargeted, not completed first). The Android shell
    /// reports `finish:true` only from the root, so the system's predictive
    /// animation is never asked to commit a screen this stack did not pop.
    pub fn back(&mut self, now: f64) -> BackOutcome {
        if self.sheet.is_some() {
            if !self.sheet_anim.is_present() && !self.sheet_anim.is_done() {
                // Already dismissing: keep that exit, do not restart it.
                return BackOutcome::Dismissed;
            }
            self.sheet_anim.begin(false, now);
            self.status = None;
            return BackOutcome::Dismissed;
        }
        self.nav.back(false, now)
    }

    /// Advances every compact animation and returns whether a repaint must be
    /// requested. This is the whole frame budget: a settled screen (even one
    /// with a job running) returns false and the surface goes idle.
    pub fn advance(&mut self, now: f64) -> bool {
        let mut animating = self.nav.advance(now, self.reduce_motion);
        animating |= self.sheet_anim.advance(now, self.reduce_motion);
        animating |= self.list_anim.advance(now, self.reduce_motion).animating;
        animating |= self.job_progress.advance(now);
        if self.sheet.is_some() && !self.sheet_anim.is_present() && self.sheet_anim.is_done() {
            self.sheet = None;
            self.export_session = None;
        }
        // The editor's data outlives a pop until the transition has played: the
        // outgoing screen is painted while it slides away.
        let settled = match self.nav.motion() {
            Some(motion) => motion.is_done(),
            None => true,
        };
        if settled && self.nav.current() == CompactRoute::Library {
            self.editor = None;
        }
        animating
    }

    /// Whether the surface must draw another frame for an animation.
    pub fn needs_frame(&self) -> bool {
        self.nav.motion().is_some_and(|motion| !motion.is_done())
            || !self.sheet_anim.is_done()
            || self.list_anim.is_animating()
            || !self.job_progress.is_done()
    }

    /// A worker event (job progress, completion) asks the host to wake the
    /// surface exactly once. The flag is taken, never polled: `false` after the
    /// first wake means "stay idle".
    pub fn take_frame_request(&mut self) -> bool {
        std::mem::take(&mut self.frame_request)
    }

    /// Reports job progress from the M16.1 worker. Returns whether the value
    /// changed — a repeated poll with the same percent asks for no frame, which
    /// is what keeps a 10-minute job from redrawing at 60 Hz.
    pub fn on_job(&mut self, label: impl Into<String>, done: u32, total: u32, now: f64) -> bool {
        let next = JobView {
            label: label.into(),
            done,
            total,
        };
        if self.job.as_ref() == Some(&next) {
            return false;
        }
        let percent = if next.total == 0 {
            0.0
        } else {
            next.done.min(next.total) as f32 / next.total as f32
        };
        self.job = Some(next);
        self.job_progress.set(percent, now);
        self.frame_request = true;
        true
    }

    /// The job ended (success, failure or cancellation): drop the strip and
    /// wake the surface once.
    pub fn job_finished(&mut self) {
        if self.job.take().is_some() {
            self.frame_request = true;
        }
    }

    /// The painter's progress factor for the job strip (settles at its target).
    pub fn job_fraction(&self) -> f32 {
        self.job_progress.value()
    }

    /// Records a fling observation from a compact list (see [`FlingProbe`]).
    pub fn note_fling(&mut self, probe: FlingProbe) {
        self.fling_probe = Some(probe);
        self.frame_request = true;
    }

    /// The library's Back is the system finish.
    pub fn finish_requested(&self) -> bool {
        self.nav.depth() == 1 && self.sheet.is_none()
    }

    /// Opens the export sheet for the open transcript.
    pub fn open_export_sheet(&mut self, now: f64) {
        if self.editor.is_none() {
            return;
        }
        self.sheet = Some(Sheet::Export);
        self.export_session = None;
        self.sheet_anim.begin(true, now);
    }

    /// The sheet's Export button: the host opens `ACTION_CREATE_DOCUMENT`.
    /// Returns the action the host must run, or `None` when no editor is open.
    pub fn begin_export(&mut self, now: f64) -> Option<CompactAction> {
        let editor = self.editor.as_ref()?;
        let action = editor.export_request(self.export_bilingual);
        let (transcript, format) = match &action {
            CompactAction::Export {
                transcript, format, ..
            } => (*transcript, *format),
            _ => return None,
        };
        self.export_session = Some(ExportSession::new(
            transcript,
            format,
            self.export_bilingual,
            editor.suggested_name(),
        ));
        self.sheet_anim.begin(true, now);
        Some(action)
    }

    /// The sheet's Share button: the same bytes, one file, `ACTION_SEND`.
    pub fn share_export(&mut self, now: f64) -> Option<CompactAction> {
        let editor = self.editor.as_ref()?;
        let action = editor.share_request(self.export_bilingual);
        let (transcript, format) = match &action {
            CompactAction::ShareExport {
                transcript, format, ..
            } => (*transcript, *format),
            _ => return None,
        };
        let mut session = ExportSession::new(
            transcript,
            format,
            self.export_bilingual,
            editor.suggested_name(),
        );
        session.phase = ExportPhase::Writing;
        self.export_session = Some(session);
        self.sheet_anim.begin(true, now);
        Some(action)
    }

    /// The picker returned a destination; the host is writing.
    pub fn on_export_picked(&mut self) {
        if let Some(session) = self.export_session.as_mut() {
            session.phase = ExportPhase::Writing;
        }
    }

    /// The picker was dismissed (back or Cancel). The sheet stays open on the
    /// chips — an abandoned export is not an error.
    pub fn on_export_cancelled(&mut self) {
        self.export_session = None;
        self.status = Some(("Export cancelled".to_string(), false));
    }

    /// The write landed and was verified.
    pub fn on_export_written(&mut self, name: impl Into<String>, bytes: u64) {
        let name = name.into();
        if let Some(session) = self.export_session.as_mut() {
            session.phase = ExportPhase::Written;
            session.bytes = Some(bytes);
            session.name = name.clone();
        }
        self.status = Some((format!("Saved {name}"), false));
    }

    /// The file went to another app (ACTION_SEND).
    pub fn on_export_shared(&mut self, name: impl Into<String>) {
        let name = name.into();
        if let Some(session) = self.export_session.as_mut() {
            session.phase = ExportPhase::Shared;
            session.name = name.clone();
        }
        self.status = Some((format!("Shared {name}"), false));
    }

    /// The write failed. The message is the host's own words — never invented.
    pub fn on_export_failed(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        if let Some(session) = self.export_session.as_mut() {
            session.phase = ExportPhase::Failed;
            session.error = Some(reason.clone());
        }
        self.status = Some((reason, true));
    }

    /// Closes the export sheet (Done, or a tap outside).
    pub fn close_export_sheet(&mut self, now: f64) {
        self.sheet_anim.begin(false, now);
    }

    /// The keys the library list is painted with (stable per transcript id).
    pub fn library_keys(&self) -> Vec<u64> {
        self.rows.iter().map(|row| row_key(row.id)).collect()
    }

    /// The one-shot Settings scroll offset (taken by the screen).
    pub fn take_settings_scroll(&mut self) -> Option<f32> {
        self.settings_scroll.take()
    }

    /// Applies a settings change to the in-memory form (the host persists it).
    pub fn apply_setting(&mut self, change: CompactSetting) {
        match change {
            CompactSetting::Model(id) => self.settings.model_id = id,
            CompactSetting::Language(lang) => self.settings.language = lang,
            CompactSetting::Appearance(appearance) => self.settings.appearance = appearance,
            CompactSetting::KeepAudio(keep) => self.settings.keep_audio = keep,
        }
    }

    /// The host acknowledged a segment save.
    pub fn on_saved(&mut self) -> Option<CompactAction> {
        let editor = self.editor.as_mut()?;
        editor.on_saved();
        self.status = Some(("Saved".to_string(), false));
        None
    }

    /// The host reports an export result in one step (the non-picker paths):
    /// `Ok` names the written file, `Err` is the host's own message.
    pub fn on_export(&mut self, result: Result<String, String>) {
        match result {
            Ok(name) => self.on_export_written(name, 0),
            Err(reason) => self.on_export_failed(reason),
        }
    }

    /// The host rejected a save: the drafts stay dirty so nothing is lost.
    pub fn on_save_failed(&mut self, reason: impl Into<String>) {
        self.status = Some((reason.into(), true));
    }

    /// The Settings Quit button's confirm.
    pub fn request_quit(&mut self) {
        self.sheet = Some(Sheet::Quit);
    }

    /// Confirms Quit: the host runs capture → jobs → services → exit.
    pub fn confirm_quit(&mut self) -> CompactAction {
        self.sheet = None;
        CompactAction::Quit
    }
}

/// A stable 64-bit key for a transcript row (the list animator's identity).
pub fn row_key(id: Uuid) -> u64 {
    let bytes = id.as_bytes();
    let mut key = 0xcbf2_9ce4_8422_2325_u64;
    for byte in &bytes[..8] {
        key ^= u64::from(*byte);
        key = key.wrapping_mul(0x0000_0100_0000_01b3);
    }
    key
}

/// The model rows for a device, from the M13D manifest rules.
pub fn model_rows(profile: DeviceProfile, language: &str, installed: &[String]) -> Vec<ModelRow> {
    let recommended = device::recommend(profile, language).default_model_id;
    MODELS
        .iter()
        .map(|model| ModelRow {
            id: model.id.to_string(),
            size_bytes: model.size_bytes,
            role: model.role.to_string(),
            note: model.phone.note.to_string(),
            offered: device::refusal_for(profile, language, model.id).is_none(),
            installed: installed.iter().any(|id| id == model.id),
            recommended: Some(model.id) == recommended,
        })
        .collect()
}

/// A short snippet around the first match, with an ellipsis where trimmed.
pub fn snippet(text: &str, needle: &str) -> String {
    const AROUND: usize = 34;
    let lower = text.to_lowercase();
    let Some(byte_pos) = lower.find(needle) else {
        return text.to_string();
    };
    let chars: Vec<char> = text.chars().collect();
    let char_pos = text[..byte_pos].chars().count();
    let start = char_pos.saturating_sub(AROUND);
    let end = (char_pos + needle.chars().count() + AROUND).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// A human byte size (the model rows' subtitle).
pub fn bytes_label(bytes: u64) -> String {
    const MB: f64 = 1_000_000.0;
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
    } else {
        format!("{:.0} MB", bytes as f64 / MB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_core::device::{DeviceProfile, PhoneTier};

    fn transcript(title: &str, texts: &[&str]) -> Transcript {
        let segments = texts
            .iter()
            .enumerate()
            .map(|(index, text)| Segment {
                idx: index as u32,
                start_ms: index as i64 * 4_000,
                end_ms: index as i64 * 4_000 + 3_500,
                text: (*text).to_string(),
            })
            .collect();
        Transcript::new(
            title.to_string(),
            suramak_core::transcript::Source::File {
                name: "clip.m4a".to_string(),
            },
            "base".to_string(),
            Some("en".to_string()),
            None,
            segments,
        )
    }

    fn app() -> CompactApp {
        let mut app = CompactApp::new(CompactSettings::default());
        app.set_library(vec![
            transcript(
                "Morning notes",
                &["the quick brown fox", "jumps over the dog"],
            ),
            transcript("Rapat tim", &["kita membahas anggaran", "dan jadwal"]),
        ]);
        app
    }

    #[test]
    fn a_row_carries_the_same_facts_the_desktop_sidebar_shows() {
        let row = LibraryRow::from_transcript(&transcript("Notes", &["one", "two"]));
        assert_eq!(row.title, "Notes");
        assert_eq!(row.segments, 2);
        assert_eq!(row.duration_ms, 7_500);
        assert!(row.meta().contains("2 segments"));
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(65_000), "1:05");
        assert_eq!(clock(3_725_000), "1:02:05");
    }

    #[test]
    fn library_search_matches_titles_and_segments_case_insensitively() {
        let mut app = app();
        app.set_library_query("FOX");
        let hits = app.hits();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Morning notes");
        assert_eq!(hits[0].segment_idx, Some(0));
        assert!(hits[0].snippet.contains("quick brown fox"));

        app.set_library_query("rapat");
        let hits = app.hits();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].segment_idx, None, "a title hit has no segment");
        assert_eq!(hits[0].start_ms, 0);
    }

    #[test]
    fn search_ranks_a_title_hit_before_a_segment_hit() {
        let mut app = CompactApp::new(CompactSettings::default());
        app.set_library(vec![
            transcript("budget review", &["nothing here"]),
            transcript("Other", &["the budget line"]),
        ]);
        app.set_library_query("budget");
        let hits = app.hits();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "budget review");
        assert!(hits[0].segment_idx.is_none());
    }

    #[test]
    fn snippets_trim_around_the_match_with_ellipses() {
        let text = "one two three four five six seven eight nine ten eleven twelve gggg hhhh iiii jjjj kkkk llll mmmm nnnn oooo pppp qqqq rrrr ssss tttt uuuu";
        let out = snippet(text, "gggg");
        assert!(out.starts_with('…'), "{out}");
        assert!(out.ends_with('…'), "{out}");
        assert!(out.contains("gggg"));
    }

    #[test]
    fn an_edit_is_a_draft_until_the_host_acknowledges_it() {
        let mut app = app();
        let first = app.transcripts[0].clone();
        app.open_editor(first, 0.0);
        let editor = app.editor.as_mut().expect("editor");
        editor.begin_edit(0);
        editor.edit_buffer.as_mut().expect("buffer").1 = "the quick red fox".to_string();
        editor.commit_edit_buffer();
        let edits = editor.pending_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].idx, 0);
        assert_eq!(edits[0].text, "the quick red fox");
        assert!(editor.has_unsaved());
        assert_eq!(
            editor.transcript.segments[0].text, "the quick brown fox",
            "the store text is untouched until on_saved"
        );
        editor.on_saved();
        assert_eq!(editor.transcript.segments[0].text, "the quick red fox");
        assert!(!editor.has_unsaved());
        assert!(editor.pending_edits().is_empty());
    }

    #[test]
    fn editing_back_to_the_stored_text_clears_the_dirty_mark() {
        let mut app = app();
        let first = app.transcripts[0].clone();
        app.open_editor(first, 0.0);
        let editor = app.editor.as_mut().expect("editor");
        editor.edit(0, "changed".to_string());
        assert_eq!(editor.dirty, vec![0]);
        editor.edit(0, "the quick brown fox".to_string());
        assert!(editor.dirty.is_empty());
    }

    #[test]
    fn the_editor_search_filters_rows_and_keeps_the_stored_order() {
        let mut app = app();
        let first = app.transcripts[0].clone();
        app.open_editor(first, 0.0);
        let editor = app.editor.as_mut().expect("editor");
        editor.search = "dog".to_string();
        assert_eq!(editor.matches(), vec![1]);
        editor.search = String::new();
        assert_eq!(editor.matches(), vec![0, 1]);
    }

    #[test]
    fn the_export_request_names_the_chosen_format_and_a_safe_file() {
        let mut app = app();
        app.open_editor(transcript("Rapat: 12/03", &["halo"]), 0.0);
        let editor = app.editor.as_mut().expect("editor");
        editor.format = CompactExportFormat::Srt;
        match editor.export_request(false) {
            CompactAction::Export {
                format,
                suggested_name,
                bilingual,
                ..
            } => {
                assert_eq!(format, CompactExportFormat::Srt);
                assert!(!bilingual);
                assert!(suggested_name.ends_with(".srt"));
                assert!(!suggested_name.contains('/'));
                assert!(!suggested_name.contains(':'));
            }
            other => panic!("unexpected {other:?}"),
        }
        // The sheet gates translation to the stacked formats.
        editor.format = CompactExportFormat::Md;
        match editor.export_request(true) {
            CompactAction::Export { bilingual, .. } => assert!(!bilingual),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_notification_deep_link_lands_on_library_then_editor() {
        let mut app = app();
        let target = app.transcripts[1].clone();
        app.open_settings(0.0);
        app.deep_link(target.clone(), 1.0);
        assert_eq!(
            app.nav.screens(),
            &[CompactRoute::Library, CompactRoute::Editor]
        );
        assert_eq!(
            app.editor.as_ref().map(|editor| editor.transcript.id),
            Some(target.id)
        );
        assert!(app.nav.motion().is_some(), "the landing animates");
    }

    #[test]
    fn a_deep_link_to_a_deleted_row_lands_on_the_library_with_a_note() {
        let mut app = app();
        app.deep_link_missing(Uuid::new_v4(), 0.0);
        assert_eq!(app.nav.current(), CompactRoute::Library);
        assert_eq!(app.nav.depth(), 1);
        assert!(app.editor.is_none());
        assert!(app
            .status
            .as_ref()
            .is_some_and(|(text, _)| text.contains("no longer")));
    }

    #[test]
    fn back_dismisses_a_sheet_then_pops_then_finishes() {
        let mut app = app();
        app.open_editor(transcript("x", &["y"]), 0.0);
        app.advance(1.0);
        app.open_export_sheet(1.0);
        assert_eq!(app.sheet, Some(Sheet::Export));
        assert_eq!(app.back(1.0), BackOutcome::Dismissed);
        // The dismissal animates out before the sheet is gone.
        assert_eq!(app.sheet, Some(Sheet::Export));
        assert!(app.advance(1.05), "the exiting sheet asks for frames");
        assert!(!app.advance(1.2));
        assert!(app.sheet.is_none());
        assert_eq!(app.nav.current(), CompactRoute::Editor);
        assert_eq!(app.back(2.0), BackOutcome::Popped);
        assert_eq!(app.nav.current(), CompactRoute::Library);
        assert_eq!(app.back(3.0), BackOutcome::Finish);
    }

    #[test]
    fn quit_needs_the_confirm_sheet() {
        let mut app = app();
        app.request_quit();
        assert_eq!(app.sheet, Some(Sheet::Quit));
        assert_eq!(app.confirm_quit(), CompactAction::Quit);
        assert!(app.sheet.is_none());
    }

    #[test]
    fn models_follow_the_m13d_tiers_and_refusals() {
        let profile = DeviceProfile::new(6_000_000_000, 2_000_000_000, false, true);
        let rows = model_rows(profile, "en", &["base".to_string()]);
        let base = rows.iter().find(|row| row.id == "base").expect("base");
        assert!(base.offered && base.installed && base.recommended);
        let turbo = rows.iter().find(|row| row.id == "turbo-q8-0").expect("q8");
        assert!(!turbo.offered, "874 MB model is refused on phones");
        let tier = device::tier_for(profile);
        assert_eq!(tier, PhoneTier::Mid);
    }

    #[test]
    fn the_settings_defaults_match_the_desktop_defaults() {
        let defaults = CompactSettings::default();
        let desktop = suramak_core::settings::Settings::default();
        assert_eq!(defaults.model_id, desktop.asr.model_id);
        assert_eq!(defaults.appearance, desktop.ui.appearance);
        assert_eq!(defaults.keep_audio, desktop.storage.keep_audio);
        assert_eq!(defaults.language, "auto");
    }

    #[test]
    fn the_export_sheet_walks_the_picker_to_a_verified_write() {
        let mut app = app();
        let first = app.transcripts[0].clone();
        app.open_editor(first, 0.0);
        app.advance(1.0);
        app.open_export_sheet(1.0);
        assert_eq!(app.sheet, Some(Sheet::Export));
        app.editor.as_mut().expect("editor").format = CompactExportFormat::Srt;

        let action = app.begin_export(1.0).expect("an export request");
        let CompactAction::Export {
            transcript,
            format,
            bilingual,
            suggested_name,
        } = action
        else {
            panic!("expected an Export action");
        };
        assert_eq!(transcript, app.transcripts[0].id);
        assert_eq!(format, CompactExportFormat::Srt);
        assert!(!bilingual);
        assert!(suggested_name.ends_with(".srt"));
        let session = app.export_session.as_ref().expect("session");
        assert_eq!(session.phase, ExportPhase::Choosing);
        assert_eq!(session.name, suggested_name);

        app.on_export_picked();
        assert_eq!(
            app.export_session.as_ref().expect("session").phase,
            ExportPhase::Writing
        );
        app.on_export_written("Morning notes.srt", 1_234);
        let session = app.export_session.as_ref().expect("session");
        assert_eq!(session.phase, ExportPhase::Written);
        assert_eq!(session.bytes, Some(1_234));
        assert!(app
            .status
            .as_ref()
            .is_some_and(|(text, error)| text.contains("Saved") && !error));

        app.close_export_sheet(2.0);
        app.advance(3.0);
        assert_eq!(app.sheet, None);
        assert!(app.export_session.is_none());
    }

    #[test]
    fn a_cancelled_picker_returns_to_the_format_chips() {
        let mut app = app();
        let first = app.transcripts[0].clone();
        app.open_editor(first, 0.0);
        app.advance(1.0);
        app.open_export_sheet(1.0);
        assert!(app.begin_export(1.0).is_some());
        app.on_export_cancelled();
        assert!(app.export_session.is_none(), "back to the chips");
        assert_eq!(app.sheet, Some(Sheet::Export), "the sheet stays open");
    }

    #[test]
    fn the_share_alternative_is_one_file_of_the_same_format() {
        let mut app = app();
        let first = app.transcripts[0].clone();
        app.open_editor(first, 0.0);
        app.advance(1.0);
        app.open_export_sheet(1.0);
        app.editor.as_mut().expect("editor").format = CompactExportFormat::Vtt;
        let action = app.share_export(1.0).expect("a share request");
        let CompactAction::ShareExport {
            format,
            suggested_name,
            ..
        } = action
        else {
            panic!("expected a ShareExport action");
        };
        assert_eq!(format, CompactExportFormat::Vtt);
        assert!(suggested_name.ends_with(".vtt"));
        assert_eq!(
            app.export_session.as_ref().expect("session").phase,
            ExportPhase::Writing
        );
        app.on_export_shared("Morning notes.vtt");
        assert_eq!(
            app.export_session.as_ref().expect("session").phase,
            ExportPhase::Shared
        );
    }

    #[test]
    fn the_six_formats_match_the_desktop_list_in_order() {
        let labels: Vec<&str> = CompactExportFormat::ALL
            .iter()
            .map(|format| format.label())
            .collect();
        assert_eq!(
            labels,
            vec!["Markdown", "Text", "JSON", "SRT", "VTT", "TSV"]
        );
        let extensions: Vec<&str> = CompactExportFormat::ALL
            .iter()
            .map(|format| format.extension())
            .collect();
        assert_eq!(extensions, vec!["md", "txt", "json", "srt", "vtt", "tsv"]);
        for format in CompactExportFormat::ALL {
            assert!(!format.mime().is_empty());
            assert_eq!(
                format.supports_bilingual(),
                matches!(format, CompactExportFormat::Srt | CompactExportFormat::Vtt)
            );
        }
    }

    #[test]
    fn a_job_event_asks_the_host_to_wake_exactly_once() {
        let mut app = app();
        assert!(!app.take_frame_request());
        assert!(app.on_job("Transcribing", 1, 10, 0.0));
        assert!(app.take_frame_request());
        assert!(!app.take_frame_request(), "one wake per change");
        assert!(
            !app.on_job("Transcribing", 1, 10, 0.1),
            "a repeated poll with the same value is not an event"
        );
        assert!(!app.take_frame_request());
        assert!(app.on_job("Transcribing", 5, 10, 0.2));
        assert_eq!(app.job.as_ref().expect("job").percent(), 50);
    }

    #[test]
    fn a_settled_screen_with_a_live_job_needs_no_frames() {
        let mut app = app();
        app.on_job("Transcribing", 4, 10, 0.0);
        app.advance(1.0);
        assert!(!app.needs_frame(), "the job strip is static between events");
        assert!(!app.advance(2.0));
        app.job_finished();
        assert!(app.take_frame_request());
        assert!(!app.needs_frame());
    }

    #[test]
    fn the_fling_tracker_uses_the_drag_velocity_at_release() {
        let mut tracker = FlingTracker::default();
        // Drag frames: the offset moves and the pointer has velocity.
        assert!(tracker.observe(0.0, 900.0, false).is_none());
        assert!(tracker.observe(40.0, 700.0, false).is_none());
        // Release with the pointer's velocity already zeroed: the probe starts
        // from the last drag velocity.
        assert!(tracker.observe(80.0, 0.0, true).is_none());
        assert!(tracker.is_active());
        // The momentum moves the offset for a few frames, then stops.
        assert!(tracker.observe(110.0, 0.0, false).is_none());
        assert!(tracker.observe(140.0, 0.0, false).is_none());
        let probe = tracker
            .observe(140.2, 0.0, false)
            .expect("the probe lands when the offset stops");
        assert_eq!(probe.release_velocity, 700.0, "the last drag velocity");
        assert!((probe.distance - 60.2).abs() < 0.01, "{probe:?}");
        assert!(probe.summary().contains("700 pt/s"));
    }

    #[test]
    fn the_fling_probe_names_its_expected_travel() {
        let mut app = app();
        app.note_fling(FlingProbe {
            release_velocity: 1_000.0,
            distance: 90.0,
            frames: 7,
        });
        let probe = app.fling_probe.expect("probe");
        assert_eq!(probe.expected_distance(), 120.0);
        assert!(probe.summary().contains("1000 pt/s"));
        assert!(app.take_frame_request());
    }

    #[test]
    fn sanitise_never_produces_an_empty_or_path_like_stem() {
        assert_eq!(sanitise_stem("Rapat: 12/03"), "Rapat 12 03");
        assert_eq!(sanitise_stem("///"), "transcript");
        assert_eq!(sanitise_stem(""), "transcript");
    }
}
