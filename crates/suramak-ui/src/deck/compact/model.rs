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
use suramak_core::models::{ModelFileState, MODELS};
use suramak_core::notes::{NotePreset, NotesOutcome};
use suramak_core::settings::Appearance;
use suramak_core::store::WordAlignment;
use suramak_core::transcript::{Segment, Transcript, WordTiming};
use uuid::Uuid;

use crate::deck::nav::{BackOutcome, NavStack};
use crate::deck::theme::AppTheme;
use crate::notes_sheet::NotesSheet;

use super::motion::{ListAnim, ProgressAnim, SheetAnim};

/// M24B: the phone's notes request — the editor snapshot plus the preset and
/// the speaker labels the host's generation call needs. A plain value so the
/// action stays `PartialEq` (a `Transcript` is not).
#[derive(Debug, Clone, PartialEq)]
pub struct NoteRequest {
    pub transcript: Uuid,
    pub title: String,
    pub language: Option<String>,
    pub preset: NotePreset,
    pub segments: Vec<Segment>,
    pub speakers: Vec<(u32, String)>,
    /// M25C: the session this part belongs to, when it has one.
    pub session: Option<Uuid>,
    /// M25C: `This part` or `Whole session`.
    pub scope: crate::notes_sheet::NoteScope,
}

/// The three screens of the G1 compact profile, plus G3's Diagnostics card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactRoute {
    /// The library: every transcript, search, import/record entry.
    Library,
    /// One transcript: read, edit, search, play, export.
    Editor,
    /// Engine/model/language/appearance/storage, plus Quit.
    Settings,
    /// The M13b four checks + the crash banner, kept behind Settings (G3).
    Diagnostics,
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
    /// M22C: how many speaker labels the transcript has (0 = none). The row
    /// shows up to four pips, then `+N`.
    pub speakers: usize,
    /// M22C: down-sampled RMS bars (`0..=1`) for the 28 px mini-waveform.
    /// Empty means no stored peaks: the row draws its flat baseline.
    pub waveform: Vec<f32>,
    /// M25B: `Part 2 of 3` when this row is one part of a split session.
    pub part: Option<suramak_core::session::PartInfo>,
    /// M25C: `Some` when this one row stands for a whole multi-part session.
    pub session: Option<SessionSummary>,
    /// M25C: the small chip (`3 parts`) a collapsed row draws; `None` on an
    /// ordinary row.
    pub part_chip: Option<String>,
}

/// M25C: a multi-part session as one library row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: Uuid,
    pub part_count: u32,
    /// Parts that actually have a transcript (a failed part is a hole, not a
    /// shorter meeting).
    pub transcribed: u32,
    pub total_ms: i64,
}

/// M25C: one part inside the session view.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionPartRow {
    pub id: Uuid,
    pub part_index: u32,
    pub title: String,
    pub duration_ms: i64,
    /// The first line of the part, one visual line, elided by the surface.
    pub opening: String,
}

/// M25C: the session view behind a collapsed library row.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionView {
    pub session_id: Uuid,
    pub title: String,
    pub part_count: u32,
    pub transcribed: u32,
    pub total_ms: i64,
    /// Ordered by `part_index`.
    pub parts: Vec<SessionPartRow>,
}

impl SessionView {
    /// `2 of 3 parts transcribed` — `None` when every part is present.
    pub fn incomplete_line(&self) -> Option<String> {
        (self.transcribed < self.part_count).then(|| {
            suramak_core::session::session_incomplete_line(
                self.transcribed as usize,
                self.part_count as usize,
            )
        })
    }

    /// `3 parts · 2 h 10 m`.
    pub fn meta(&self) -> String {
        suramak_core::session::session_meta_label(self.part_count as usize, self.total_ms)
    }
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
            speakers: 0,
            waveform: Vec::new(),
            part: transcript
                .audio_path
                .as_deref()
                .and_then(|path| path.file_name().and_then(|name| name.to_str()))
                .and_then(suramak_core::session::PartInfo::from_name)
                .or_else(|| suramak_core::session::PartInfo::from_name(&transcript.title)),
            session: None,
            part_chip: None,
        }
    }

    /// M25C: the one collapsed row for a whole session. `parts` is already
    /// ordered by `part_index`; `transcribed` counts the rows that exist.
    pub fn from_session(session_id: Uuid, parts: &[&Transcript], summary: SessionSummary) -> Self {
        let first = parts.first().copied();
        let newest = parts
            .iter()
            .map(|part| part.created_at)
            .max()
            .unwrap_or_else(chrono::Utc::now);
        Self {
            id: first.map(|part| part.id).unwrap_or(Uuid::nil()),
            title: first
                .map(|part| suramak_core::session::base_name(&part.title))
                .unwrap_or_else(|| "Session".to_string()),
            created_at: newest.with_timezone(&Local),
            segments: parts.iter().map(|part| part.segments.len()).sum(),
            duration_ms: summary.total_ms,
            language: first.and_then(|part| part.language.clone()),
            speakers: 0,
            waveform: Vec::new(),
            part: None,
            session: Some(summary),
            part_chip: Some(format!(
                "{} part{}",
                summary.part_count,
                if summary.part_count == 1 { "" } else { "s" }
            )),
        }
    }

    /// `"12 Mar · 21 segments"` — the phone row's meta line. The duration
    /// lives in the row's tabular right-hand column (M22C), so it is not
    /// repeated here.
    pub fn meta(&self) -> String {
        format!(
            "{} · {}",
            self.created_at.format("%-d %b"),
            crate::deck::compact::count_noun(self.segments, "segment")
        )
    }
}

/// The per-transcript facets a library load attaches to a row (M22C): the
/// speaker count for the pips and the stored waveform bars. Kept separate from
/// [`LibraryRow::from_transcript`] because the compact host reads them in one
/// batched query rather than one per row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LibraryFacets {
    pub speakers: usize,
    pub waveform: Vec<f32>,
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
    /// The ±60-char window with the match wrapped in `[`/`]` (the store's wire
    /// shape; the row paints the inner span in the accent ink, without the
    /// brackets).
    pub snippet: String,
    pub segment_idx: Option<u32>,
    pub start_ms: i64,
    /// The segment's speaker label, when the transcript is diarized.
    pub speaker: Option<String>,
    /// FTS5 `bm25`; lower is better. Kept for the phone JSON and stable order.
    pub rank: f64,
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

    /// Whether this format has a speaker-aware variant (M23A): SRT, VTT and
    /// Markdown prefix a line; txt/json/tsv are machine rows and stay plain.
    pub fn supports_speakers(self) -> bool {
        matches!(self, Self::Srt | Self::Vtt | Self::Md)
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
    /// M23A: whether the speaker prefixes were included in this write.
    pub speakers: bool,
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
            speakers: false,
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
    /// M13g3: the one-time intro has been shown (additive `first_run_seen`).
    FirstRunSeen(bool),
    /// M24A: the watched inbox switch (`settings.watch.enabled`). Turning it
    /// on without a folder opens the tree picker instead (the host only
    /// persists `true` once a folder exists).
    WatchEnabled(bool),
}

/// M24A: the Library's watch status row (the host fills it from the ledger).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchView {
    /// The folder's display name (never a raw SAF URI).
    pub folder: String,
    /// Ledger rows — how many files the inbox has imported.
    pub imports: u64,
    /// The named failure when the folder or its grant is gone.
    pub failure: Option<String>,
}

impl WatchView {
    /// `Watching · 3 imports`, or the named failure. The folder is the row's
    /// second text, not part of this line.
    pub fn line(&self) -> String {
        match &self.failure {
            Some(failure) => format!("Watching paused — {failure}"),
            None => {
                let noun = if self.imports == 1 {
                    "import"
                } else {
                    "imports"
                };
                format!("Watching · {} {noun}", self.imports)
            }
        }
    }
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
    /// Whether the file is **verified** on disk (pinned size, no `.part`) —
    /// M19B: not merely present. `false` for an absent file, a `.part` or a
    /// wrong-sized file.
    pub installed: bool,
    /// The disk truth behind `installed` (M19B).
    pub disk: ModelFileState,
    /// The model the action should select/download.
    pub recommended: bool,
}

/// The five states a model row can be in (M19B).
///
/// Only [`ModelState::InUse`] may paint the accent border and the "In use"
/// pill, and it is reachable only through a **verified** file — the row state
/// is derived from disk truth, never from the configured id alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelState {
    /// Nothing on disk: `Get` (or the card's `Download`).
    NotDownloaded,
    /// A transfer is running or a `.part` is waiting: progress + `Cancel`
    /// (`Resume` when there is no live job).
    Downloading,
    /// A file exists at the wrong size: `Re-download` (the bad file is
    /// unlinked before the transfer starts).
    StaleCorrupt,
    /// Verified, not the configured model: `Use`.
    Installed,
    /// Verified **and** configured: the only "In use" state.
    InUse,
}

impl ModelState {
    pub const ALL: [Self; 5] = [
        Self::NotDownloaded,
        Self::Downloading,
        Self::StaleCorrupt,
        Self::Installed,
        Self::InUse,
    ];

    /// The row's state word (the second line's suffix and the log marker).
    pub fn label(self) -> &'static str {
        match self {
            Self::NotDownloaded => "not downloaded",
            Self::Downloading => "downloading",
            Self::StaleCorrupt => "stale file",
            Self::Installed => "installed",
            Self::InUse => "in use",
        }
    }

    /// The row's action word.
    pub fn action(self) -> &'static str {
        match self {
            Self::NotDownloaded => "Get",
            Self::Downloading => "Cancel",
            Self::StaleCorrupt => "Re-download",
            Self::Installed => "Use",
            Self::InUse => "In use",
        }
    }

    /// The machine-readable token (the shell's log marker, the host snapshot).
    pub fn slug(self) -> &'static str {
        match self {
            Self::NotDownloaded => "not_downloaded",
            Self::Downloading => "downloading",
            Self::StaleCorrupt => "stale_corrupt",
            Self::Installed => "installed",
            Self::InUse => "in_use",
        }
    }

    pub fn is_in_use(self) -> bool {
        matches!(self, Self::InUse)
    }

    /// Whether the row's action button does anything.
    pub fn is_actionable(self) -> bool {
        !self.is_in_use()
    }
}

impl ModelRow {
    /// The phone's **human tier name** for this model (M20D).
    ///
    /// The manifest ids (`tiny`, `base`, `small`, `turbo-q5-0`) are the stable
    /// machine vocabulary; they are not what a person should read first. The
    /// Settings card leads with these names and keeps the id on a mono second
    /// line (and in Diagnostics) so support can still name the file exactly.
    pub fn tier_name(&self) -> &'static str {
        match self.id.as_str() {
            "tiny" => "Quick test",
            "base" => "Compact",
            "small" => "Balanced",
            "turbo-q5-0" => "Highly accurate",
            "turbo-q8-0" => "High accuracy",
            "turbo" => "Full accuracy",
            _ => "Model",
        }
    }

    /// The size a person compares against: `"78 MB"`, `"148 MB"`, `"488 MB"`.
    pub fn tier_line(&self) -> String {
        super::model::bytes_label(self.size_bytes)
    }

    /// One human sentence about what the tier is for (M20D).
    ///
    /// The manifest's `phone.note` is the refusal/guidance string and speaks
    /// in ids and RAM bands ("the smaller pinned model; the phone default on
    /// 4–6 GB devices"); this is the sentence the grouped card shows, in the
    /// product's own words. Unknown ids fall back to the manifest note.
    pub fn phone_note(&self) -> String {
        match self.id.as_str() {
            "tiny" => "For quick tests — the least accurate, especially in Indonesian.".to_string(),
            "base" => "The lightest model for everyday notes.".to_string(),
            "small" => "Better accuracy for longer recordings.".to_string(),
            "turbo-q5-0" => "Slower, but the most accurate model that fits a phone.".to_string(),
            _ => self.note.clone(),
        }
    }

    /// The phone's size label (`"78 MB"`).
    ///
    /// The manifest's `role` is build vocabulary (`CI + tests`, `optional/
    /// legacy`, `fast`): it is never spoken or painted on the phone. The size
    /// is the one fact a person compares tiers by; the human name comes from
    /// [`tier_name`](Self::tier_name).
    pub fn size_role(&self) -> String {
        super::model::bytes_label(self.size_bytes)
    }

    /// The five-state machine for this row (M19B).
    ///
    /// `selected` is the configured model id; `download` is the live M13D job
    /// view for this row, when one exists. A verified file is a precondition
    /// for `InUse` — nothing else can produce it.
    pub fn state(&self, selected: bool, download: Option<&DownloadView>) -> ModelState {
        if self.installed {
            return if selected {
                ModelState::InUse
            } else {
                ModelState::Installed
            };
        }
        if download.is_some_and(|download| {
            matches!(
                download.state,
                DownloadState::Running | DownloadState::Paused
            )
        }) {
            return ModelState::Downloading;
        }
        match self.disk {
            ModelFileState::Partial => ModelState::Downloading,
            ModelFileState::WrongSize => ModelState::StaleCorrupt,
            ModelFileState::Absent | ModelFileState::Verified => ModelState::NotDownloaded,
        }
    }
}

/// The model download's phase (M13D's `ModelHost` state, mirrored for the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    /// A `.part` left by an interrupted run: the next start resumes it.
    Paused,
    Running,
    Done,
    Failed,
}

impl DownloadState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Paused => "Paused",
            Self::Running => "Downloading",
            Self::Done => "Downloaded",
            Self::Failed => "Download failed",
        }
    }
}

/// One download, as the model job reports it. `percent` is the **job's own**
/// integer percent (`bytes_done * 100 / bytes_total`), never a UI estimate:
/// the card and the dataSync notification show one number because they read
/// one number.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadView {
    pub model: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub percent: u8,
    pub state: DownloadState,
    /// The host's own sentence (a refusal, a pause reason, a failure).
    pub message: String,
    /// This view was reconstructed from a `.part` at launch (the resume offer).
    pub resumed: bool,
}

impl DownloadView {
    pub fn line(&self) -> String {
        match self.state {
            DownloadState::Paused => format!(
                "Paused at {}% — {} of {}",
                self.percent,
                super::model::bytes_label(self.bytes_done),
                super::model::bytes_label(self.bytes_total)
            ),
            DownloadState::Running => format!(
                "Downloading {} · {}% · {} of {}",
                self.model,
                self.percent,
                super::model::bytes_label(self.bytes_done),
                super::model::bytes_label(self.bytes_total)
            ),
            DownloadState::Done => format!("{} downloaded", self.model),
            DownloadState::Failed => format!("{} failed", self.model),
        }
    }

    pub fn fraction(&self) -> f32 {
        if self.bytes_total == 0 {
            0.0
        } else {
            (self.bytes_done.min(self.bytes_total) as f32 / self.bytes_total as f32).clamp(0.0, 1.0)
        }
    }
}

/// The v2.4 crash banner, as the diagnostics card shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct CrashBanner {
    /// `true` = recorded by this process; `false` = read from the crash file.
    pub live: bool,
    pub text: String,
}

impl CrashBanner {
    pub fn title(&self) -> &'static str {
        if self.live {
            "CRASH — THIS SESSION (caught, app still running)"
        } else {
            "CRASH — LAST LAUNCH (read from the crash file)"
        }
    }
}

/// The four-check counters + crash banner behind Settings → Diagnostics.
///
/// This is the M13b spike's evidence, kept at the same coverage (nothing is
/// dropped) but aggregated into four groups instead of a tab farm. The Android
/// shell fills it from `SpikeState`/`diag` every frame; the host suite fills it
/// with fixtures for the shot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiagnosticsView {
    // 1 · text
    pub text_chars: u64,
    pub ime_updates: u64,
    pub deletions: u64,
    // 2 · lifecycle
    pub resumes: u32,
    pub surface_recreates: u32,
    pub frames_gated: u64,
    pub frames_after_destroy: u64,
    pub probe_saves: u32,
    pub probe_restores: u32,
    // 3 · motion
    pub avg_fps: Option<f32>,
    pub frames: u64,
    // 4 · insets + back
    pub inset_updates: u32,
    pub inset_top: f32,
    pub inset_bottom: f32,
    pub back_presses: u32,
    /// The last crash (this session's or the previous launch's).
    pub crash: Option<CrashBanner>,
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
    /// M25C: a collapsed multi-part row was tapped — open its session view.
    OpenSession(Uuid),
    /// M22C: the Library field changed — run the store's segment search and
    /// hand the hits back through [`CompactApp::set_search_hits`].
    SearchLibrary {
        query: String,
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
        /// M23A: prefix each line with its speaker (`Include speakers`).
        speakers: bool,
        suggested_name: String,
    },
    /// The one-file `ACTION_SEND` alternative: stage the same bytes and hand
    /// that one file to the share chooser (disclosed in the sheet).
    ShareExport {
        transcript: Uuid,
        format: CompactExportFormat,
        bilingual: bool,
        speakers: bool,
        suggested_name: String,
    },
    /// The library's Import button (SAF / share).
    Import,
    /// M24A: open `ACTION_OPEN_DOCUMENT_TREE` and persist the chosen tree's
    /// read grant (the host stores the URI, never a raw path).
    PickWatchFolder,
    /// The library's Record button.
    Record,
    /// The library's Stop button while the mic host is recording.
    StopRecording,
    /// A row's play affordance: open the transcript and start the transport.
    PlayTranscript(Uuid),
    /// The transport started or stopped (the host owns audio; this is state).
    Playback {
        playing: bool,
    },
    /// A word chip or a segment seek: park the transport at this position.
    Seek {
        position_ms: i64,
    },
    /// Apply a setting; the host writes it and refreshes the store cache.
    Setting(CompactSetting),
    DownloadModel(String),
    DeleteModel(String),
    /// The model job's Cancel action (the `.part` stays for a resume).
    CancelDownload,
    /// The one-time intro was dismissed (`skipped` distinguishes the paths).
    OnboardingSeen {
        skipped: bool,
    },
    /// Diagnostics: delete `spike-last-crash.txt` + the history log.
    ClearCrash,
    /// Diagnostics: hand the crash log to another app (the v2.1 share route).
    ShareCrashLog,
    /// M23A: add the next speaker (ordinal + first unused colour).
    AddSpeaker {
        transcript: Uuid,
    },
    /// M23A: assign one segment's range (or from it to the end) to a speaker.
    AssignSpeaker {
        transcript: Uuid,
        speaker_id: u32,
        segment: u32,
        from_here_down: bool,
    },
    /// M23A: `+ New` in the turn strip — add a speaker and assign the range
    /// to it in one host transaction pair (two undo entries, both undoable).
    NewSpeakerAssign {
        transcript: Uuid,
        segment: u32,
        from_here_down: bool,
    },
    /// M23A: rename one speaker (the store owns uniqueness and the 60-char cap).
    RenameSpeaker {
        transcript: Uuid,
        speaker_id: u32,
        label: String,
    },
    /// M23A: one palette slot (0..6).
    SetSpeakerColor {
        transcript: Uuid,
        speaker_id: u32,
        slot: u8,
    },
    /// M23A: merge the source speaker into the target (the target's colour and
    /// name survive).
    MergeSpeakers {
        transcript: Uuid,
        from: u32,
        into: u32,
    },
    /// M23A: remove one speaker's label and turns (the rows go unlabelled).
    RemoveSpeakerLabel {
        transcript: Uuid,
        speaker_id: u32,
    },
    /// M23A: remove every speaker (confirmed in the Speakers sheet).
    ClearSpeakers {
        transcript: Uuid,
    },
    /// M23A: undo the newest speaker transaction (the host's SpeakerHistory).
    UndoSpeakers {
        transcript: Uuid,
    },
    /// M22B: start the shared voice note with this language (`auto` allowed).
    VoiceNoteAccept {
        language: String,
    },
    /// M22B: the offer was dismissed without starting anything.
    VoiceNoteDismiss,
    /// M24B: read the open transcript's stored notes.
    LoadNotes {
        transcript: Uuid,
        /// M25C: read the whole-session rows instead of the part's own.
        session: Option<Uuid>,
    },
    /// M24B: generate one preset from this snapshot (unsaved edits included).
    GenerateNote {
        request: Box<NoteRequest>,
    },
    /// M24B: persist one citation-checked note (one row per preset).
    SaveNote {
        note: Box<suramak_core::store::NoteRecord>,
    },
    /// M24B: write the exact stored Markdown through ACTION_CREATE_DOCUMENT.
    ExportNote {
        name: String,
        markdown: String,
    },
    /// M25C: one Markdown file for a whole session, part dividers included.
    ExportSession {
        name: String,
        markdown: String,
    },
    /// M24B: copy the exact stored Markdown to the clipboard.
    CopyNote {
        text: String,
    },
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
    /// M13g3: the one-time intro (three paths, skippable).
    Onboarding,
    /// M22B: the 1.2 s voice-note offer a share opens.
    VoiceNote,
    /// M23A: the speaker list (bottom-bar `Tag speakers` / `N speakers`).
    Speakers,
    /// M23A: one speaker's properties (rename, colour, merge, remove).
    Speaker,
    /// M23A: the per-segment turn strip (assign this turn / from here down).
    TurnStrip,
    /// M24B: the citation-verified notes sheet.
    Notes,
    /// M25C: the parts of one multi-part session, behind a collapsed row.
    Session,
}

impl Sheet {
    /// Whether this sheet is one of M23A's speaker surfaces.
    pub fn is_speaker_sheet(self) -> bool {
        matches!(self, Sheet::Speakers | Sheet::Speaker | Sheet::TurnStrip)
    }

    /// The height the speaker surfaces ask the sheet plane for.
    pub fn speaker_height(self) -> f32 {
        match self {
            Sheet::TurnStrip => 260.0,
            _ => 430.0,
        }
    }
}

/// M23A: the turn strip's scope — one segment, or the rest of the recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TurnScope {
    /// Only the tapped segment's time range.
    #[default]
    ThisTurn,
    /// From the tapped segment's start to the end of the transcript.
    FromHereDown,
}

impl TurnScope {
    pub fn label(self) -> &'static str {
        match self {
            TurnScope::ThisTurn => "This turn",
            TurnScope::FromHereDown => "From here down",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            TurnScope::ThisTurn => TurnScope::FromHereDown,
            TurnScope::FromHereDown => TurnScope::ThisTurn,
        }
    }
}

/// M23A: the turn strip's target and scope (the segment it was opened on).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurnStripState {
    pub segment: u32,
    pub scope: TurnScope,
}

/// M23A: the Speaker sheet's drafts. The rename field is a draft until Save;
/// `merge_open` swaps the body to the "merge into…" target list.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerSheetState {
    pub speaker_id: u32,
    pub rename: String,
    pub merge_open: bool,
}

/// M23A: the 6 s Undo toast after a speaker edit.
#[derive(Debug, Clone, PartialEq)]
pub struct UndoToast {
    pub label: String,
    pub opened_at: f64,
}

/// How long the Undo toast stays on screen (the plan's 6 s).
pub const UNDO_TOAST_SECS: f64 = 6.0;

impl UndoToast {
    pub fn is_expired(&self, now: f64) -> bool {
        now - self.opened_at >= UNDO_TOAST_SECS
    }

    /// 0 → 1 opacity over the last 400 ms, so the toast leaves rather than
    /// vanishing mid-read (Reduce Motion keeps its state, as everywhere).
    pub fn alpha(&self, now: f64, reduce_motion: bool) -> f32 {
        if reduce_motion {
            return 1.0;
        }
        let remaining = UNDO_TOAST_SECS - (now - self.opened_at);
        (remaining / 0.4).clamp(0.0, 1.0) as f32
    }
}

/// M22B: the state the voice-note micro-sheet draws. The host builds it from
/// the share intent; the UI owns only the clock and the press.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceNoteView {
    /// The shared file's display name (or "Voice note" when it had none).
    pub title: String,
    /// The sender from the intent's own title extra — never derived.
    pub sender: Option<String>,
    /// The preselected pill; `None` means Auto (and no auto-start).
    pub pill: Option<super::voice_note::Pill>,
    /// Whether a verified model is installed (`Transcribe` is enabled).
    pub model_ok: bool,
    /// Whether the sheet may start the job and dismiss itself.
    pub auto_start: bool,
    /// The honest line under the pill when it cannot start by itself.
    pub reason: String,
    /// The egui clock reading when the sheet first drew (the 1.2 s origin).
    pub opened_at: f64,
}

/// M16I: what the compact editor needs from one diarization run — the shared
/// [`crate::deck::speakers`] renderer takes exactly this shape.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactSpeakers {
    pub speakers: Vec<suramak_core::diarization::Speaker>,
    pub turns: Vec<suramak_core::diarization::SpeakerTurn>,
    /// One badge per segment row (the read-time projection).
    pub per_segment: Vec<Option<suramak_core::diarization::SegmentSpeaker>>,
    pub stale: bool,
    pub audio_ms: i64,
    /// M23A: whether a speaker edit landed after the last speaker-inclusive
    /// export (the export sheet's honesty note).
    pub edited_since_export: bool,
}

impl CompactSpeakers {
    pub fn label(&self, speaker_id: u32) -> String {
        self.speakers
            .iter()
            .find(|speaker| speaker.speaker_id == speaker_id)
            .map(|speaker| speaker.label.clone())
            .unwrap_or_else(|| suramak_core::diarization::default_label(speaker_id))
    }

    pub fn color_slot(&self, speaker_id: u32) -> u8 {
        self.speakers
            .iter()
            .find(|speaker| speaker.speaker_id == speaker_id)
            .map(|speaker| speaker.color_slot)
            .unwrap_or(0)
    }

    pub fn row(&self, idx: u32) -> Option<suramak_core::diarization::SegmentSpeaker> {
        self.per_segment.get(idx as usize).copied().flatten()
    }

    /// How many turns a speaker owns in the active run (the Speaker sheet's
    /// subtitle and the list row's count).
    pub fn turn_count(&self, speaker_id: u32) -> usize {
        self.turns
            .iter()
            .filter(|turn| turn.speaker_id == speaker_id)
            .count()
    }

    /// The speaker's label for the badge/pill (the sheet reads through
    /// [`Self::label`] too; this is the borrow-friendly form).
    pub fn speaker(&self, speaker_id: u32) -> Option<&suramak_core::diarization::Speaker> {
        self.speakers
            .iter()
            .find(|speaker| speaker.speaker_id == speaker_id)
    }

    /// The bottom bar's label: `Tag speakers` when nothing is labelled yet,
    /// else the count. Never a nounless number.
    pub fn button_label(&self) -> String {
        match self.speakers.len() {
            0 => "Tag speakers".to_string(),
            1 => "1 speaker".to_string(),
            n => format!("{n} speakers"),
        }
    }
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
    /// M22C: the next frame should scroll the focused segment into view. Set
    /// by [`CompactEditor::reveal_segment`], consumed by the editor's list.
    pub focus_scroll: bool,
    /// Whether the transcript has at least one stored translation (seeded by
    /// the host from `Store::segment_translations`). The bilingual export
    /// switch is offered only when there is a second line to stack.
    pub has_translation: bool,
    /// M15.6 estimated word timing, as the store reports it. Only segments
    /// whose rows are current drive the compact chips; the rest fall back to
    /// segment seeking (there is no row-level badge for it on the phone).
    pub words: WordAlignment,
    /// M16I: the speaker annotation, when the host seeded one. `None` keeps the
    /// plain phone row (no badge, no lane) — the same shared Deck components
    /// the desktop draws, not a second implementation.
    pub speakers: Option<CompactSpeakers>,
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
            focus_scroll: false,
            has_translation: false,
            words: WordAlignment::default(),
            speakers: None,
        }
    }

    /// The transcript's whole duration (the last segment's end).
    pub fn duration_ms(&self) -> i64 {
        self.transcript
            .segments
            .iter()
            .map(|segment| segment.end_ms)
            .max()
            .unwrap_or(0)
    }

    /// Adopts the store's usable word timing (after a load or a fresh job).
    pub fn set_words(&mut self, words: WordAlignment) {
        self.words = words;
    }

    /// The current words of one segment, or an empty slice (the fallback).
    pub fn words_for(&self, idx: u32) -> &[WordTiming] {
        self.words
            .current
            .get(&idx)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The index of the word the playhead is in, for the segment the playhead
    /// is inside (M15.6's `word_at`, the same rule the desktop uses).
    pub fn current_word(&self, idx: u32) -> Option<usize> {
        word_at(self.words_for(idx), self.playhead_ms)
    }

    /// The segment containing the playhead, if any.
    pub fn playhead_segment(&self) -> Option<u32> {
        self.transcript
            .segments
            .iter()
            .find(|segment| {
                self.playhead_ms >= segment.start_ms && self.playhead_ms < segment.end_ms
            })
            .map(|segment| segment.idx)
            .or_else(|| {
                self.transcript
                    .segments
                    .last()
                    .filter(|segment| self.playhead_ms >= segment.end_ms)
                    .map(|segment| segment.idx)
            })
    }

    /// Starts the transport from the current playhead (no-op at the end).
    pub fn start_playback(&mut self) {
        if self.playhead_ms >= self.duration_ms() {
            self.playhead_ms = 0;
        }
        self.playing = self.duration_ms() > 0;
    }

    pub fn stop_playback(&mut self) {
        self.playing = false;
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

    /// M22C: the Library hit's landing. Selects the row (the editor's own
    /// highlight), parks the playhead on its start and asks the next frame to
    /// scroll it into view — the phone's existing seek path, one call.
    pub fn reveal_segment(&mut self, idx: u32) {
        if let Some(segment) = self.transcript.segments.iter().find(|s| s.idx == idx) {
            self.selected = Some(idx);
            self.playhead_ms = segment.start_ms;
            self.focus_segment = Some(idx);
            self.focus_scroll = true;
        }
    }

    /// The export request the host stages. `bilingual` is gated on the
    /// format; `speakers` on a labelled transcript and a speaker-capable
    /// format (txt/json/tsv stay machine rows).
    pub fn export_request(&self, bilingual: bool, speakers: bool) -> CompactAction {
        CompactAction::Export {
            transcript: self.transcript.id,
            format: self.format,
            bilingual: bilingual && self.format.supports_bilingual(),
            speakers: speakers
                && self.format.supports_speakers()
                && self
                    .speakers
                    .as_ref()
                    .is_some_and(|view| !view.speakers.is_empty()),
            suggested_name: self.suggested_name(),
        }
    }

    /// The one-file share request (same bytes, `ACTION_SEND`).
    pub fn share_request(&self, bilingual: bool, speakers: bool) -> CompactAction {
        CompactAction::ShareExport {
            transcript: self.transcript.id,
            format: self.format,
            bilingual: bilingual && self.format.supports_bilingual(),
            speakers: speakers
                && self.format.supports_speakers()
                && self
                    .speakers
                    .as_ref()
                    .is_some_and(|view| !view.speakers.is_empty()),
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

/// The index of the word the playhead is in (M15.6's rule, shared with the
/// desktop editor): the last word whose estimate starts at or before
/// `position_ms`, or `None` before the first word. Karaoke holds the word it
/// just read through the pause that follows it.
pub fn word_at(words: &[WordTiming], position_ms: i64) -> Option<usize> {
    let after = words.partition_point(|word| word.start_ms <= position_ms);
    after.checked_sub(1)
}

/// M25C: the one-line opening of a part for the session view: its first
/// segment, collapsed to one visual line (`…` when it runs long). An empty
/// part says so rather than inventing a sentence.
pub fn opening_line(transcript: &Transcript) -> String {
    let Some(first) = transcript.segments.first() else {
        return "no speech in this part".to_string();
    };
    let text = first.text.trim();
    let mut out: String = text.chars().take(96).collect();
    if text.chars().count() > 96 {
        out.push('…');
    }
    if out.is_empty() {
        "no speech in this part".to_string()
    } else {
        out
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
    /// M25C: the session identity of every part, as the host read it from the
    /// store. Empty means an older host / no session rows — the library then
    /// renders exactly as before.
    pub session_identities: std::collections::HashMap<Uuid, suramak_core::store::SessionIdentity>,
    /// M25C: the session view behind a collapsed row (`Sheet::Session`).
    pub session_view: Option<SessionView>,
    pub library_query: String,
    /// M22C: the store's spoken-text hits for the current query, already ranked
    /// and limited. The library draws them under its "in transcripts" heading;
    /// title matches stay in `rows` and come first.
    pub search_hits: Vec<LibraryHit>,
    /// Library load failure, shown as an honest note (never a fake empty list).
    pub library_error: Option<String>,
    pub editor: Option<CompactEditor>,
    pub sheet: Option<Sheet>,
    pub settings: CompactSettings,
    /// M24A: the watched inbox switch + folder display name, seeded by the
    /// host from `settings.watch` (the folder is a SAF tree URI there; this is
    /// only its display name).
    pub watch_enabled: bool,
    pub watch_folder: String,
    /// M24A: the status row, filled by the host while watching is on.
    pub watch: Option<WatchView>,
    pub models: Vec<ModelRow>,
    /// The profile the model rows were derived from (`None` until the shell
    /// reports memory); a language change re-derives the rows through it.
    pub profile: Option<DeviceProfile>,
    /// Free space on the models volume, in bytes (the M13D preflight's number).
    pub free_bytes: Option<u64>,
    /// The model download as the host reports it (the launch card's state).
    pub download: Option<DownloadView>,
    /// M13g3: the one-time intro has been shown. Seeded from settings.
    pub first_run_seen: bool,
    /// The last four-check snapshot the shell pushed (Settings → Diagnostics).
    pub diagnostics: DiagnosticsView,
    /// Whether the mic host is recording (the Library's Record becomes Stop).
    pub recording: bool,
    /// The transport's last tick (the preview clock; `None` when stopped).
    playback_last: Option<f64>,
    /// A back gesture dismissed the intro; the host persists the flag after
    /// `back()` returns (the gesture has no action vector to return one).
    onboarding_seen_pending: bool,
    /// The export sheet's flow (picker → write → result), when one is open.
    pub export_session: Option<ExportSession>,
    /// The sheet's "include the translation" switch.
    pub export_bilingual: bool,
    /// M23A: the sheet's "include speakers" switch.
    pub export_speakers: bool,
    /// M23A: the Speaker sheet's draft (open while `Sheet::Speaker` is).
    pub speaker_sheet: Option<SpeakerSheetState>,
    /// M23A: the turn strip's target/scope (open while `Sheet::TurnStrip`).
    pub turn_strip: Option<TurnStripState>,
    /// M23A: the 6 s Undo toast after a speaker edit.
    pub undo_toast: Option<UndoToast>,
    /// M23A: `Clear all speakers` is destructive; the first tap only arms it.
    pub speakers_confirm_clear: bool,
    /// M24B: the citation-verified notes sheet (the shared state machine).
    pub notes: NotesSheet,
    /// M24B: whether the polish endpoint is configured, seeded by the host.
    pub notes_configured: bool,
    /// M23A: whether the host has a speaker snapshot to undo (the turn strip's
    /// and the toast's Undo state; the host owns the log).
    pub speakers_undo_available: bool,
    /// M22B: the voice-note offer being drawn (or about to be).
    pub voice_note: Option<VoiceNoteView>,
    /// Where the Library's gear was last drawn (points, window coordinates).
    /// The Android host turns it into device pixels for the emulator smoke, so
    /// a tap on Settings never depends on a hard-coded dp guess.
    pub settings_anchor: Option<egui::Pos2>,
    /// M22B: an offer that arrived before the next frame could timestamp it.
    voice_note_armed: Option<VoiceNoteView>,
    /// M22B: the offer was dismissed by BACK; the shell tells the Activity.
    voice_note_cancel_pending: bool,
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
    /// The smallest bottom clearance the content leaves under its last
    /// control, even when the platform reports no inset (M19A2). 0 on desktop;
    /// the Android shell sets [`super::settings::ANDROID_MIN_CLEARANCE`].
    pub clearance_floor: f32,
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
            session_identities: std::collections::HashMap::new(),
            session_view: None,
            library_query: String::new(),
            search_hits: Vec::new(),
            library_error: None,
            editor: None,
            sheet: None,
            settings,
            watch_enabled: false,
            watch_folder: String::new(),
            watch: None,
            models: Vec::new(),
            profile: None,
            free_bytes: None,
            download: None,
            first_run_seen: false,
            diagnostics: DiagnosticsView::default(),
            recording: false,
            playback_last: None,
            onboarding_seen_pending: false,
            export_session: None,
            export_bilingual: false,
            export_speakers: false,
            speaker_sheet: None,
            turn_strip: None,
            undo_toast: None,
            speakers_confirm_clear: false,
            notes: NotesSheet::new(),
            notes_configured: false,
            speakers_undo_available: false,
            voice_note: None,
            settings_anchor: None,
            voice_note_armed: None,
            voice_note_cancel_pending: false,
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
            clearance_floor: 0.0,
            reduce_motion: false,
            theme: AppTheme::Dark,
            version: env!("CARGO_PKG_VERSION").to_string(),
            pending_open: None,
        }
    }

    /// Seeds the device facts and derives the model rows through M13D's rules.
    ///
    /// `installed` is the **verified** set (M19B): callers that only have ids
    /// (tests, fixtures) get the same treatment as a host that scanned disk.
    pub fn seed_device(&mut self, profile: DeviceProfile, installed: &[String]) {
        let disk = disk_from_ids(installed);
        self.seed_device_states(profile, &disk);
    }

    /// Seeds the device facts with the full disk truth per model (M19B). The
    /// Android host scans `files/models` and passes every row's state, so a
    /// `.part` or a truncated file can never read as installed.
    pub fn seed_device_states(
        &mut self,
        profile: DeviceProfile,
        disk: &[(&'static str, ModelFileState)],
    ) {
        self.tier = Some(device::tier_for(profile));
        self.ram_bytes = Some(profile.total_ram_bytes);
        self.profile = Some(profile);
        self.models = model_rows_from_states(profile, &self.settings.language, disk);
    }

    /// Re-derives the model rows after a language change (Indonesian's `tiny`
    /// steering is a property of the recommendation, not of the tier).
    pub fn refresh_models(&mut self, installed: &[String]) {
        let disk = disk_from_ids(installed);
        self.refresh_model_states(&disk);
    }

    /// Re-derives the rows from a fresh disk scan (M19B).
    pub fn refresh_model_states(&mut self, disk: &[(&'static str, ModelFileState)]) {
        if let Some(profile) = self.profile {
            self.models = model_rows_from_states(profile, &self.settings.language, disk);
        }
    }

    /// The row for the configured model, when the manifest carries it.
    pub fn configured_model(&self) -> Option<&ModelRow> {
        self.models
            .iter()
            .find(|row| row.id == self.settings.model_id)
    }

    /// The TRANSCRIPTION → Model row's value and whether it needs the warning
    /// tint (M19B): `small` when the file is verified, `small · not downloaded`
    /// when it is not. The configured model is shown even when another model is
    /// installed — the header must name what the app will actually try to load.
    pub fn model_label(&self) -> (String, bool) {
        let id = self.settings.model_id.clone();
        match self.models.iter().find(|row| row.id == id) {
            Some(row) if row.installed => (id, false),
            Some(_) => (format!("{id} · not downloaded"), true),
            // No device report yet (or an unknown id): nothing to qualify.
            None => (id, false),
        }
    }

    /// Whether a recording has the configured model to run (M19B).
    ///
    /// The job loads `settings.model_id`, so "some model is installed" is not
    /// enough: the configured one must be verified. Before the shell reports
    /// memory the model table is empty and the answer is "unknown, do not
    /// block". A phone with a verified file that is not the configured model
    /// gets [`Self::record_block_reason`]'s "choose" line instead.
    pub fn can_record(&self) -> bool {
        self.models.is_empty() || self.configured_model().is_some_and(|row| row.installed)
    }

    /// The inline reason the Record button is disabled, if it is (M19B).
    pub fn record_block_reason(&self) -> Option<&'static str> {
        if self.can_record() {
            return None;
        }
        if self.tier == Some(PhoneTier::Unsupported) {
            return Some("No model can run on this phone.");
        }
        if self.models.iter().any(|row| row.installed && row.offered) {
            Some("Choose a model that is on this phone.")
        } else {
            Some("Download a model before recording.")
        }
    }

    /// Whether the Library must lead with the M13D download card: the shell
    /// knows the manifest but no offered model is on disk.
    pub fn model_card_needed(&self) -> bool {
        !self.models.is_empty() && !self.models.iter().any(|row| row.installed && row.offered)
    }

    /// The recommended row for this phone + language.
    pub fn recommended_model(&self) -> Option<&ModelRow> {
        self.models
            .iter()
            .find(|row| row.recommended && row.offered)
            .or_else(|| self.models.iter().find(|row| row.offered && !row.installed))
            .or_else(|| self.models.iter().find(|row| row.offered))
    }

    /// The offered rows in card order: recommended first, then cheapest first.
    pub fn offered_models(&self) -> Vec<&ModelRow> {
        let mut rows: Vec<&ModelRow> = self.models.iter().filter(|row| row.offered).collect();
        rows.sort_by_key(|row| (!row.recommended, row.size_bytes));
        rows
    }

    /// Whether the models volume can hold `bytes`, if the shell reported space.
    pub fn has_space_for(&self, bytes: u64) -> Option<bool> {
        self.free_bytes.map(|free| free >= bytes)
    }

    /// The M13D language sentence (Indonesian's `tiny` warning), when set.
    pub fn language_note(&self) -> Option<String> {
        self.profile
            .map(|profile| device::recommend(profile, &self.settings.language))
            .and_then(|recommendation| recommendation.language_note)
    }

    /// The recommendation's headline for the card's top line.
    pub fn model_headline(&self) -> String {
        match self.profile {
            Some(profile) => device::recommend(profile, &self.settings.language)
                .headline
                .to_string(),
            None => "Device memory not reported by the shell".to_string(),
        }
    }

    /// Records the job's progress. `percent` is the **job's own** number; the
    /// UI never recomputes it (the notification shows the same integer).
    #[allow(clippy::too_many_arguments)]
    pub fn on_download(
        &mut self,
        model: impl Into<String>,
        bytes_done: u64,
        bytes_total: u64,
        percent: u8,
        state: DownloadState,
        message: impl Into<String>,
        now: f64,
    ) -> bool {
        let next = DownloadView {
            model: model.into(),
            bytes_done,
            bytes_total,
            percent: percent.min(100),
            state,
            message: message.into(),
            resumed: false,
        };
        if self.download.as_ref() == Some(&next) {
            return false;
        }
        self.download = Some(next);
        self.frame_request = true;
        if state == DownloadState::Running {
            // The card's fill rides the same job-progress machinery as the
            // library strip: one ease per changed percent, then idle.
            self.job_progress.set(percent as f32 / 100.0, now);
        }
        true
    }

    /// A `.part` found at launch: the resume offer, at the same percent the
    /// next job run will start from.
    pub fn on_download_resume(
        &mut self,
        model: impl Into<String>,
        bytes_done: u64,
        bytes_total: u64,
        percent: u8,
    ) {
        self.download = Some(DownloadView {
            model: model.into(),
            bytes_done,
            bytes_total,
            percent: percent.min(100),
            state: DownloadState::Paused,
            message: "A partial download is on this phone; it resumes where it stopped."
                .to_string(),
            resumed: true,
        });
    }

    pub fn clear_download(&mut self) {
        self.download = None;
    }

    pub fn set_free_space(&mut self, bytes: Option<u64>) {
        self.free_bytes = bytes;
    }

    /// Opens the one-time intro when the settings say it has not been seen.
    pub fn maybe_begin_onboarding(&mut self, now: f64) -> bool {
        if self.first_run_seen || self.sheet.is_some() {
            return false;
        }
        self.sheet = Some(Sheet::Onboarding);
        self.sheet_anim.begin_spring(true, now);
        true
    }

    // ---- M22B: the voice-note offer ---------------------------------------

    /// Arms the offer. The first frame after this call stamps `opened_at`, so
    /// the 1.2 s window is measured on the same clock that animates it.
    pub fn offer_voice_note(&mut self, view: VoiceNoteView) {
        self.voice_note = None;
        self.voice_note_armed = Some(view);
    }

    /// Called once per frame by [`super::ui`]: turns an armed offer into the
    /// open sheet. The intro yields to a share (the share is the user's act;
    /// the intro returns next launch because `first_run_seen` is untouched).
    pub fn begin_voice_note(&mut self, now: f64) {
        let Some(mut view) = self.voice_note_armed.take() else {
            return;
        };
        view.opened_at = now;
        self.voice_note = Some(view);
        self.sheet = Some(Sheet::VoiceNote);
        self.sheet_anim.begin_spring(true, now);
    }

    /// When the auto-dismiss fires, if it is armed.
    pub fn voice_note_deadline(&self) -> Option<f64> {
        let view = self.voice_note.as_ref()?;
        view.auto_start
            .then_some(view.opened_at + super::voice_note::AUTO_DISMISS_SECONDS)
    }

    /// The auto-dismiss: starts the job and closes the sheet in one step. Only
    /// armed when the language was unambiguous and a model is verified.
    pub fn take_voice_note_auto(&mut self, now: f64) -> Option<CompactAction> {
        let deadline = self.voice_note_deadline()?;
        if now < deadline {
            return None;
        }
        let language = self
            .voice_note
            .as_ref()
            .and_then(|view| view.pill)
            .map(|pill| pill.tag())
            .unwrap_or("auto")
            .to_string();
        if let Some(view) = self.voice_note.as_mut() {
            // The deadline has fired; never schedule it again while the exit
            // animation plays (a zero-duration repaint would busy-loop).
            view.auto_start = false;
        }
        self.dismiss_voice_note(now);
        Some(CompactAction::VoiceNoteAccept { language })
    }

    /// Transcribe was tapped. `pill` is the sheet's current choice (or `None`
    /// for Auto); the sheet closes and the host schedules the job.
    pub fn accept_voice_note(
        &mut self,
        pill: Option<super::voice_note::Pill>,
        now: f64,
    ) -> CompactAction {
        if let Some(view) = self.voice_note.as_mut() {
            view.pill = pill;
            view.auto_start = false;
        }
        self.dismiss_voice_note(now);
        CompactAction::VoiceNoteAccept {
            language: pill.map(|pill| pill.tag()).unwrap_or("auto").to_string(),
        }
    }

    /// Closes the offer (Start, Cancel, a tap outside, or BACK).
    pub fn dismiss_voice_note(&mut self, now: f64) {
        self.sheet_anim.begin(false, now);
    }

    /// BACK dismissed the offer: the shell tells the Activity to drop the
    /// pending share (no job was scheduled).
    pub fn take_voice_note_cancel(&mut self) -> bool {
        std::mem::take(&mut self.voice_note_cancel_pending)
    }

    /// Start/Skip on the intro: dismiss it (spring back down) and ask the host
    /// to persist `first_run_seen = true`. Returns the host action.
    pub fn finish_onboarding(&mut self, now: f64, skipped: bool) -> CompactAction {
        self.first_run_seen = true;
        self.sheet_anim.begin(false, now);
        CompactAction::OnboardingSeen { skipped }
    }

    /// Opens Diagnostics (a push; back returns to Settings).
    pub fn open_diagnostics(&mut self, now: f64) {
        self.sheet = None;
        self.nav.push(CompactRoute::Diagnostics, now);
    }

    /// The shell's four-check snapshot (drawn by the Diagnostics route).
    pub fn set_diagnostics(&mut self, view: DiagnosticsView) {
        if self.diagnostics != view {
            self.diagnostics = view;
        }
    }

    /// The mic host's recording state (the Library's Record/Stop toggle).
    pub fn set_recording(&mut self, recording: bool) {
        if self.recording != recording {
            self.recording = recording;
            self.frame_request = true;
        }
    }

    /// Starts the transport for the open transcript (the row's play affordance).
    pub fn start_playback(&mut self) {
        if let Some(editor) = self.editor.as_mut() {
            editor.start_playback();
            self.playback_last = None;
            self.frame_request = true;
        }
    }

    /// The host's playhead while it owns the audio clock (M8/M13 media host).
    pub fn on_playhead(&mut self, position_ms: i64) {
        if let Some(editor) = self.editor.as_mut() {
            editor.playhead_ms = position_ms.clamp(0, editor.duration_ms());
            self.frame_request = true;
        }
    }

    /// Replaces the library list (a fresh `Store::list_transcripts`).
    ///
    /// M25C: with session identities attached, a session's parts collapse into
    /// one row. Without them (an older host, or an ordinary library) every row
    /// is exactly the pre-M25C row.
    pub fn set_library(&mut self, transcripts: Vec<Transcript>) {
        self.set_library_with_sessions(transcripts, std::collections::HashMap::new());
    }

    /// M25C: the library plus the store's `session_id`/`part_index` map.
    ///
    /// A session collapses when at least two of its parts are present. The
    /// collapsed row takes the position of the session's newest part, so the
    /// list stays newest-first. A session with one present part keeps today's
    /// single row (and its `Part N of M` label) — a lone part is not a session
    /// view until a second part exists.
    pub fn set_library_with_sessions(
        &mut self,
        transcripts: Vec<Transcript>,
        identities: std::collections::HashMap<Uuid, suramak_core::store::SessionIdentity>,
    ) {
        let mut groups: std::collections::HashMap<Uuid, Vec<&Transcript>> =
            std::collections::HashMap::new();
        for transcript in &transcripts {
            if let Some(identity) = identities.get(&transcript.id) {
                if identity.part_count >= 2 {
                    groups
                        .entry(identity.session_id)
                        .or_default()
                        .push(transcript);
                }
            }
        }
        let mut emitted: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        let mut rows = Vec::with_capacity(transcripts.len());
        for transcript in &transcripts {
            let identity = identities.get(&transcript.id);
            if let Some(identity) = identity {
                if identity.part_count >= 2 && groups.contains_key(&identity.session_id) {
                    if !emitted.insert(identity.session_id) {
                        continue;
                    }
                    let mut parts = groups[&identity.session_id].clone();
                    parts.sort_by_key(|part| {
                        identities
                            .get(&part.id)
                            .map(|identity| identity.part_index)
                            .unwrap_or(u32::MAX)
                    });
                    let present = parts.len();
                    if present < 2 {
                        // One present part: keep the ordinary row (with its
                        // honest `Part N of M` label).
                        rows.push(LibraryRow::from_transcript(transcript));
                        continue;
                    }
                    let total_ms = parts
                        .iter()
                        .map(|part| {
                            part.segments
                                .iter()
                                .map(|segment| segment.end_ms)
                                .max()
                                .unwrap_or(0)
                        })
                        .sum();
                    let summary = SessionSummary {
                        session_id: identity.session_id,
                        part_count: identity.part_count,
                        transcribed: present as u32,
                        total_ms,
                    };
                    rows.push(LibraryRow::from_session(
                        identity.session_id,
                        &parts,
                        summary,
                    ));
                    continue;
                }
            }
            rows.push(LibraryRow::from_transcript(transcript));
        }
        self.rows = rows;
        self.session_identities = identities;
        self.transcripts = transcripts;
        self.library_error = None;
    }

    /// M25C: opens the session view for a collapsed row.
    pub fn open_session(&mut self, session_id: Uuid, now: f64) {
        let mut parts: Vec<&Transcript> = self
            .transcripts
            .iter()
            .filter(|transcript| {
                self.session_identities
                    .get(&transcript.id)
                    .is_some_and(|identity| identity.session_id == session_id)
            })
            .collect();
        parts.sort_by_key(|part| {
            self.session_identities
                .get(&part.id)
                .map(|identity| identity.part_index)
                .unwrap_or(u32::MAX)
        });
        let part_count = self
            .session_identities
            .values()
            .find(|identity| identity.session_id == session_id)
            .map(|identity| identity.part_count)
            .unwrap_or(parts.len() as u32);
        let total_ms = parts
            .iter()
            .map(|part| {
                part.segments
                    .iter()
                    .map(|segment| segment.end_ms)
                    .max()
                    .unwrap_or(0)
            })
            .sum();
        let rows = parts
            .iter()
            .map(|part| {
                let identity = self.session_identities.get(&part.id);
                SessionPartRow {
                    id: part.id,
                    part_index: identity.map(|identity| identity.part_index).unwrap_or(0),
                    title: suramak_core::session::base_name(&part.title),
                    duration_ms: part
                        .segments
                        .iter()
                        .map(|segment| segment.end_ms)
                        .max()
                        .unwrap_or(0),
                    opening: opening_line(part),
                }
            })
            .collect::<Vec<_>>();
        self.session_view = Some(SessionView {
            session_id,
            title: rows
                .first()
                .map(|row| row.title.clone())
                .unwrap_or_else(|| "Session".to_string()),
            part_count,
            transcribed: rows.len() as u32,
            total_ms,
            parts: rows,
        });
        self.sheet = Some(Sheet::Session);
        self.sheet_anim.begin(true, now);
    }

    /// M25C: one Markdown file for the open session view.
    ///
    /// The parts are put on the same session-absolute timeline the notes use;
    /// the body's times are absolute and each part opens with a
    /// `## Part N — mm:ss` divider. A part's own export is untouched.
    pub fn session_export_action(&self) -> Option<CompactAction> {
        let view = self.session_view.as_ref()?;
        let mut parts: Vec<&Transcript> = self
            .transcripts
            .iter()
            .filter(|transcript| {
                self.session_identities
                    .get(&transcript.id)
                    .is_some_and(|identity| identity.session_id == view.session_id)
            })
            .collect();
        parts.sort_by_key(|part| {
            self.session_identities
                .get(&part.id)
                .map(|identity| identity.part_index)
                .unwrap_or(u32::MAX)
        });
        let mut inputs = Vec::with_capacity(parts.len());
        for part in &parts {
            let identity = self.session_identities.get(&part.id)?;
            inputs.push(suramak_core::session::SessionPartInput {
                transcript_id: part.id,
                title: suramak_core::session::base_name(&part.title),
                part_index: identity.part_index,
                part_count: identity.part_count,
                duration_ms: part
                    .segments
                    .iter()
                    .map(|segment| segment.end_ms)
                    .max()
                    .unwrap_or(0),
                segments: part.segments.clone(),
            });
        }
        let timeline = suramak_core::session::session_timeline(&inputs);
        let first = parts.first()?;
        let markdown = suramak_core::export::to_session_markdown(
            &view.title,
            first.created_at,
            &first.source,
            &first.model,
            first.language.as_deref(),
            &timeline,
        );
        Some(CompactAction::ExportSession {
            name: format!("{}.md", sanitise_stem(&view.title)),
            markdown,
        })
    }

    /// Attaches the batched per-row facets (M22C): speaker counts for the pips
    /// and stored waveform bars. Rows without an entry keep the honest empty
    /// facets (no pips, flat baseline).
    pub fn set_library_facets(&mut self, facets: BTreeMap<Uuid, LibraryFacets>) {
        for row in &mut self.rows {
            if let Some(facet) = facets.get(&row.id) {
                row.speakers = facet.speakers;
                row.waveform = facet.waveform.clone();
            }
        }
    }

    pub fn set_library_error(&mut self, error: impl Into<String>) {
        self.library_error = Some(error.into());
        self.rows.clear();
        self.transcripts.clear();
        self.search_hits.clear();
    }

    /// Applies a library query (the field's text).
    ///
    /// Changing the query drops the previous spoken-text hits immediately: a
    /// row must never show a snippet for a question the reader has moved past.
    /// Clearing the field also restores the plain list.
    pub fn set_library_query(&mut self, query: impl Into<String>) {
        let next = query.into();
        if next.trim() != self.library_query.trim() {
            self.search_hits.clear();
        }
        self.library_query = next;
        if self.library_query.trim().is_empty() {
            self.search_hits.clear();
        }
    }

    /// The store's ranked spoken-text hits for the current query (M22C).
    pub fn set_search_hits(&mut self, hits: Vec<LibraryHit>) {
        self.search_hits = hits;
    }

    /// Whether the current query has anything to show in either section.
    pub fn search_has_results(&self) -> bool {
        !self.title_matches().is_empty() || !self.search_hits.is_empty()
    }

    /// Title matches for the current field text, in the library's own order
    /// (`rows` is already newest-first). These render first, as plain rows.
    pub fn title_matches(&self) -> Vec<&LibraryRow> {
        let needle = self.library_query.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        self.rows
            .iter()
            .filter(|row| row.title.to_lowercase().contains(&needle))
            .collect()
    }

    /// The "in transcripts" section: the store's segment hits, best first.
    pub fn transcript_hits(&self) -> &[LibraryHit] {
        &self.search_hits
    }

    /// Ranked-in-order search hits over the loaded transcripts. The query is
    /// matched case-insensitively against the title (a title hit ranks first)
    /// and then each segment; the first matching segment is the snippet. This
    /// is the pre-M22C in-memory helper kept for callers that hold full
    /// transcripts (tests and previews); the product's Library uses
    /// [`CompactApp::transcript_hits`], which the host fills from the store's
    /// FTS5 segment index.
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
                speaker: None,
                rank: 0.0,
            });
        }
        // Title hits first, then by start time — stable and explainable.
        hits.sort_by_key(|hit| (hit.segment_idx.is_some(), hit.start_ms));
        hits
    }

    /// Opens the editor on a loaded transcript (Library stays below it).
    pub fn open_editor(&mut self, transcript: Transcript, now: f64) {
        self.open_editor_words(transcript, WordAlignment::default(), now);
    }

    /// Opens the editor with the store's usable M15.6 word timing (the compact
    /// chips). The host reads `Store::word_alignment` on the same open.
    pub fn open_editor_words(&mut self, transcript: Transcript, words: WordAlignment, now: f64) {
        let mut editor = CompactEditor::new(transcript);
        editor.set_words(words);
        self.editor = Some(editor);
        self.nav.push(CompactRoute::Editor, now);
    }

    /// The notification's landing: Library, then the finished transcript's
    /// editor, in one motion. This is the path M13E's `ACTION_OPEN_TRANSCRIPT`
    /// intent feeds through the platform event.
    pub fn deep_link(&mut self, transcript: Transcript, now: f64) {
        self.deep_link_words(transcript, WordAlignment::default(), now);
    }

    /// The deep link with the transcript's stored word timing.
    pub fn deep_link_words(&mut self, transcript: Transcript, words: WordAlignment, now: f64) {
        self.sheet = None;
        self.pending_open = None;
        let mut editor = CompactEditor::new(transcript);
        editor.set_words(words);
        self.editor = Some(editor);
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
            if self.sheet == Some(Sheet::Onboarding) {
                // Back on the intro is Skip: it is a one-time sheet, so the
                // gesture must not bring it back on the next launch.
                self.first_run_seen = true;
                self.onboarding_seen_pending = true;
            }
            if self.sheet == Some(Sheet::VoiceNote) {
                // A dismissed share starts nothing; the Activity drops the
                // pending URI so a later stale tap cannot revive it.
                self.voice_note_cancel_pending = true;
            }
            self.sheet_anim.begin(false, now);
            self.status = None;
            return BackOutcome::Dismissed;
        }
        self.nav.back(false, now)
    }

    /// Whether a back gesture dismissed the intro (the host persists it).
    pub fn take_onboarding_seen(&mut self) -> bool {
        std::mem::take(&mut self.onboarding_seen_pending)
    }

    /// Advances every compact animation and returns whether a repaint must be
    /// requested. This is the whole frame budget: a settled screen (even one
    /// with a job running) returns false and the surface goes idle.
    pub fn advance(&mut self, now: f64) -> bool {
        let mut animating = self.nav.advance(now, self.reduce_motion);
        animating |= self.sheet_anim.advance(now, self.reduce_motion);
        animating |= self.list_anim.advance(now, self.reduce_motion).animating;
        animating |= self.job_progress.advance(now);
        // The transport is a clock, not a spring: while it plays it owns the
        // playhead (and therefore the karaoke word). It asks for frames only
        // while playing; stopping at the transcript's end returns the surface
        // to on-demand repaints.
        animating |= self.tick_playback(now);
        if self.sheet.is_some() && !self.sheet_anim.is_present() && self.sheet_anim.is_done() {
            self.sheet = None;
            self.export_session = None;
            self.voice_note = None;
            self.speaker_sheet = None;
            self.turn_strip = None;
            self.speakers_confirm_clear = false;
        }
        if self
            .undo_toast
            .as_ref()
            .is_some_and(|toast| toast.is_expired(now))
        {
            self.undo_toast = None;
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

    /// The transport's clock (see [`CompactEditor::playing`]). It follows the
    /// host's egui clock exactly; the first tick after a start only anchors
    /// (no jump), and the last tick stops at the transcript's duration. Reduce
    /// Motion changes nothing here: this is position, not decoration.
    fn tick_playback(&mut self, now: f64) -> bool {
        let Some(editor) = self.editor.as_mut() else {
            self.playback_last = None;
            return false;
        };
        if !editor.playing {
            self.playback_last = None;
            return false;
        }
        let duration = editor.duration_ms();
        if duration <= 0 {
            editor.playing = false;
            self.playback_last = None;
            return false;
        }
        let previous = self.playback_last.replace(now);
        if let Some(previous) = previous {
            let dt_ms = ((now - previous).max(0.0) * 1000.0).round() as i64;
            if dt_ms > 0 {
                editor.playhead_ms = (editor.playhead_ms + dt_ms).min(duration);
            }
        }
        if editor.playhead_ms >= duration {
            editor.playhead_ms = duration;
            editor.playing = false;
            self.playback_last = None;
            return false;
        }
        true
    }

    /// Whether the surface must draw another frame for an animation.
    pub fn needs_frame(&self) -> bool {
        self.nav.motion().is_some_and(|motion| !motion.is_done())
            || !self.sheet_anim.is_done()
            || self.list_anim.is_animating()
            || !self.job_progress.is_done()
            || self.editor.as_ref().is_some_and(|editor| editor.playing)
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
        let action = editor.export_request(self.export_bilingual, self.export_speakers);
        let (transcript, format, speakers) = match &action {
            CompactAction::Export {
                transcript,
                format,
                speakers,
                ..
            } => (*transcript, *format, *speakers),
            _ => return None,
        };
        let mut session = ExportSession::new(
            transcript,
            format,
            self.export_bilingual,
            editor.suggested_name(),
        );
        session.speakers = speakers;
        self.export_session = Some(session);
        self.sheet_anim.begin(true, now);
        Some(action)
    }

    /// The sheet's Share button: the same bytes, one file, `ACTION_SEND`.
    pub fn share_export(&mut self, now: f64) -> Option<CompactAction> {
        let editor = self.editor.as_ref()?;
        let action = editor.share_request(self.export_bilingual, self.export_speakers);
        let (transcript, format, speakers) = match &action {
            CompactAction::ShareExport {
                transcript,
                format,
                speakers,
                ..
            } => (*transcript, *format, *speakers),
            _ => return None,
        };
        let mut session = ExportSession::new(
            transcript,
            format,
            self.export_bilingual,
            editor.suggested_name(),
        );
        session.speakers = speakers;
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

    // ---- M24B: the notes sheet -------------------------------------------

    /// Opens the Notes sheet and asks the host for the stored rows.
    pub fn open_notes_sheet(&mut self, now: f64) -> Option<CompactAction> {
        let editor = self.editor.as_ref()?;
        let id = editor.transcript.id;
        let session = self.session_identities.get(&id).copied().map(|identity| {
            let transcribed = self
                .transcripts
                .iter()
                .filter(|transcript| {
                    self.session_identities
                        .get(&transcript.id)
                        .is_some_and(|other| other.session_id == identity.session_id)
                })
                .count() as u32;
            (identity.session_id, identity.part_count, transcribed)
        });
        self.notes.open_with_session(
            self.notes_configured,
            Vec::new(),
            &editor.transcript,
            session,
        );
        self.sheet = Some(Sheet::Notes);
        self.sheet_anim.begin(true, now);
        Some(CompactAction::LoadNotes {
            transcript: id,
            session: session.map(|(session, _, _)| session),
        })
    }

    /// M25C: the sheet's `This part` / `Whole session` toggle.
    pub fn set_notes_scope(&mut self, scope: crate::notes_sheet::NoteScope) {
        self.notes.set_scope(scope);
    }

    pub fn close_notes_sheet(&mut self, now: f64) {
        self.sheet_anim.begin(false, now);
    }

    pub fn select_note_preset(&mut self, preset: NotePreset) {
        self.notes.select(preset);
    }

    /// The sheet's Run button: one request for the selected preset, from the
    /// editor's current text (drafts included) and speaker labels.
    pub fn start_note(&mut self) -> Option<CompactAction> {
        let preset = match self.notes.start() {
            Ok(preset) => preset,
            Err(message) => {
                self.status = Some((message, true));
                return None;
            }
        };
        let editor = self.editor.as_ref()?;
        let mut transcript = editor.transcript.clone();
        for segment in &mut transcript.segments {
            if let Some(text) = editor.drafts.get(&segment.idx) {
                segment.text = text.clone();
            }
        }
        let speakers = editor
            .speakers
            .as_ref()
            .map(|view| {
                transcript
                    .segments
                    .iter()
                    .zip(view.per_segment.iter())
                    .filter_map(|(segment, speaker)| {
                        speaker.map(|speaker| (segment.idx, view.label(speaker.primary)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(CompactAction::GenerateNote {
            request: Box::new(NoteRequest {
                transcript: transcript.id,
                title: transcript.title,
                language: transcript.language,
                preset,
                segments: transcript.segments,
                speakers,
                session: self.notes.session,
                scope: self.notes.scope,
            }),
        })
    }

    /// The host's store read for the open transcript's notes.
    pub fn on_notes_loaded(
        &mut self,
        id: Uuid,
        result: Result<Vec<suramak_core::notes::StoredNote>, String>,
    ) {
        if self.notes.transcript != Some(id) {
            return;
        }
        match result {
            Ok(stored) => self.notes.adopt_stored(stored),
            Err(message) => {
                self.status = Some((format!("notes could not be read — {message}"), true))
            }
        }
    }

    /// The host finished a notes run: store it in memory and return the row the
    /// host must persist.
    pub fn on_notes_done(&mut self, result: Result<NotesOutcome, String>) -> Option<CompactAction> {
        let id = self.notes.transcript?;
        let stored = self
            .notes
            .on_done(result, suramak_core::Utc::now().to_rfc3339())?;
        let created = DateTime::parse_from_rfc3339(&stored.created_at)
            .map(|at| at.with_timezone(&suramak_core::Utc))
            .unwrap_or_else(|_| suramak_core::Utc::now());
        self.status = Some((format!("{} ready", stored.preset.label()), false));
        let session_id = (self.notes.scope == crate::notes_sheet::NoteScope::WholeSession)
            .then_some(self.notes.session)
            .flatten();
        Some(CompactAction::SaveNote {
            note: Box::new(suramak_core::store::NoteRecord {
                transcript_id: id,
                preset: stored.preset,
                body_md: stored.body_md,
                citations_checked: stored.citations_checked,
                citations_removed: stored.citations_removed,
                model: stored.model,
                created_at: created,
                session_id,
            }),
        })
    }

    /// `Export .md`: the exact stored Markdown and the suggested file name.
    pub fn note_export_action(&self) -> Option<CompactAction> {
        let editor = self.editor.as_ref()?;
        let markdown = self.notes.export_text()?.to_string();
        Some(CompactAction::ExportNote {
            name: self.notes.export_name(&editor.transcript),
            markdown,
        })
    }

    /// `Copy`: the exact stored Markdown.
    pub fn note_copy_action(&self) -> Option<CompactAction> {
        self.notes.copy_text().map(|text| CompactAction::CopyNote {
            text: text.to_string(),
        })
    }

    // ---- M23A: the speaker surfaces --------------------------------------

    /// The open transcript's speaker view, when the host seeded one.
    pub fn speakers_view(&self) -> Option<&CompactSpeakers> {
        self.editor
            .as_ref()
            .and_then(|editor| editor.speakers.as_ref())
    }

    /// The bottom bar's label: `Tag speakers` or `N speakers`.
    pub fn speakers_button_label(&self) -> String {
        self.speakers_view()
            .map(CompactSpeakers::button_label)
            .unwrap_or_else(|| "Tag speakers".to_string())
    }

    /// Opens the Speakers sheet (the bottom bar's button). No editor guard:
    /// the editor calls this while it holds the frame's `CompactEditor` (the
    /// app field is temporarily taken), and it only opens from that screen.
    pub fn open_speakers_sheet(&mut self, now: f64) {
        self.speakers_confirm_clear = false;
        self.sheet = Some(Sheet::Speakers);
        self.sheet_anim.begin(true, now);
    }

    /// Opens one speaker's sheet (tapping the turn header). The rename field
    /// starts at the current label — a rename is an edit, not a blank box.
    pub fn open_speaker_sheet(&mut self, speaker_id: u32, now: f64) {
        let label = self
            .speakers_view()
            .map(|view| view.label(speaker_id))
            .unwrap_or_else(|| suramak_core::diarization::default_label(speaker_id));
        self.open_speaker_sheet_named(speaker_id, label, now);
    }

    /// The same sheet with the label the editor already has in hand (the app's
    /// editor field is taken while a screen draws, so the row passes its own).
    pub fn open_speaker_sheet_named(&mut self, speaker_id: u32, label: String, now: f64) {
        self.speaker_sheet = Some(SpeakerSheetState {
            speaker_id,
            rename: label,
            merge_open: false,
        });
        self.sheet = Some(Sheet::Speaker);
        self.sheet_anim.begin(true, now);
    }

    /// Opens the turn strip for a segment (tapping `[+ Speaker]`, a badge, or
    /// a segment's speaker affordance). No editor guard: the caller is the
    /// editor drawing the frame (see [`Self::open_speakers_sheet`]).
    pub fn open_turn_strip(&mut self, segment: u32, now: f64) {
        self.turn_strip = Some(TurnStripState {
            segment,
            scope: TurnScope::ThisTurn,
        });
        self.sheet = Some(Sheet::TurnStrip);
        self.sheet_anim.begin(true, now);
    }

    /// Flips `This turn | From here down` in place (the strip stays open).
    pub fn toggle_turn_scope(&mut self) {
        if let Some(strip) = self.turn_strip.as_mut() {
            strip.scope = strip.scope.toggled();
        }
    }

    /// The transcript the speaker surfaces act on.
    fn speaker_transcript(&self) -> Option<Uuid> {
        self.editor.as_ref().map(|editor| editor.transcript.id)
    }

    /// `Add speaker` — the host writes the row, then calls
    /// [`Self::on_speaker_applied`] with the summary (`Speaker 2 added`).
    pub fn request_add_speaker(&mut self) -> Option<CompactAction> {
        Some(CompactAction::AddSpeaker {
            transcript: self.speaker_transcript()?,
        })
    }

    /// `+ New` inside the turn strip: the host adds the next speaker and
    /// immediately assigns the strip's range to it.
    pub fn request_new_speaker(&mut self) -> Option<CompactAction> {
        let strip = self.turn_strip?;
        let transcript = self.speaker_transcript()?;
        Some(CompactAction::NewSpeakerAssign {
            transcript,
            segment: strip.segment,
            from_here_down: strip.scope == TurnScope::FromHereDown,
        })
    }

    /// Assigns the strip's range to a speaker and closes the strip; the host
    /// answers with a summary for the Undo toast.
    pub fn request_assign(&mut self, speaker_id: u32, now: f64) -> Option<CompactAction> {
        let strip = self.turn_strip?;
        let transcript = self.speaker_transcript()?;
        self.sheet_anim.begin(false, now);
        Some(CompactAction::AssignSpeaker {
            transcript,
            speaker_id,
            segment: strip.segment,
            from_here_down: strip.scope == TurnScope::FromHereDown,
        })
    }

    /// The Speaker sheet's Save: a rename (trimmed; empty is refused in the UI
    /// and again by the store).
    pub fn request_rename(&mut self) -> Option<CompactAction> {
        let sheet = self.speaker_sheet.clone()?;
        let label = sheet.rename.trim();
        if label.is_empty() {
            return None;
        }
        Some(CompactAction::RenameSpeaker {
            transcript: self.speaker_transcript()?,
            speaker_id: sheet.speaker_id,
            label: label.to_string(),
        })
    }

    /// A colour dot: the store's `set_speaker_color`.
    pub fn request_speaker_color(&mut self, slot: u8) -> Option<CompactAction> {
        let sheet = self.speaker_sheet.as_ref()?;
        Some(CompactAction::SetSpeakerColor {
            transcript: self.speaker_transcript()?,
            speaker_id: sheet.speaker_id,
            slot,
        })
    }

    /// `Merge into…` reveals the target list; the second tap merges.
    pub fn open_merge_targets(&mut self) {
        if let Some(sheet) = self.speaker_sheet.as_mut() {
            sheet.merge_open = true;
        }
    }

    pub fn request_merge(&mut self, into: u32) -> Option<CompactAction> {
        let sheet = self.speaker_sheet.as_ref()?;
        Some(CompactAction::MergeSpeakers {
            transcript: self.speaker_transcript()?,
            from: sheet.speaker_id,
            into,
        })
    }

    /// `Remove label` — reverts the speaker's turns to unlabelled.
    pub fn request_remove_label(&mut self) -> Option<CompactAction> {
        let sheet = self.speaker_sheet.as_ref()?;
        Some(CompactAction::RemoveSpeakerLabel {
            transcript: self.speaker_transcript()?,
            speaker_id: sheet.speaker_id,
        })
    }

    /// `Clear all speakers`: the first tap arms the confirmation, the second
    /// emits the action. Returns `None` on the arming tap.
    pub fn request_clear_speakers(&mut self) -> Option<CompactAction> {
        if !self.speakers_confirm_clear {
            self.speakers_confirm_clear = true;
            return None;
        }
        self.speakers_confirm_clear = false;
        Some(CompactAction::ClearSpeakers {
            transcript: self.speaker_transcript()?,
        })
    }

    /// The Undo toast's action (and the turn strip's Undo when one is open).
    pub fn request_undo_speakers(&mut self, now: f64) -> Option<CompactAction> {
        let transcript = self.speaker_transcript()?;
        self.undo_toast = None;
        self.sheet_anim.begin(false, now);
        Some(CompactAction::UndoSpeakers { transcript })
    }

    /// Shows the 6 s Undo toast without closing the sheet (Add speaker and
    /// the colour dots keep their surface open; a colour is worth trying).
    pub fn show_speaker_toast(&mut self, summary: impl Into<String>, now: f64) {
        self.undo_toast = Some(UndoToast {
            label: summary.into(),
            opened_at: now,
        });
        self.speakers_undo_available = true;
    }

    /// The host applied one speaker verb: close the speaker surfaces and show
    /// the 6 s Undo toast with the verb's own summary.
    pub fn on_speaker_applied(&mut self, summary: impl Into<String>, now: f64) {
        self.show_speaker_toast(summary, now);
        self.speaker_sheet = None;
        self.turn_strip = None;
        self.speakers_confirm_clear = false;
        self.sheet_anim.begin(false, now);
    }

    /// The host undid the newest speaker transaction. The sheet dismisses with
    /// its own animation (advance clears it when the slide lands).
    pub fn on_speaker_undone(&mut self, now: f64) {
        self.undo_toast = None;
        self.speakers_undo_available = false;
        self.speaker_sheet = None;
        self.turn_strip = None;
        self.sheet_anim.begin(false, now);
    }

    /// The host re-read the speaker view (after any verb or on open). `None`
    /// is the plain, never-tagged view.
    pub fn on_speakers_refreshed(&mut self, view: Option<CompactSpeakers>) {
        if let Some(editor) = self.editor.as_mut() {
            editor.speakers = view;
        }
    }

    /// The frame deadline that clears the toast, if one is showing.
    pub fn undo_deadline(&self) -> Option<f64> {
        self.undo_toast
            .as_ref()
            .map(|toast| toast.opened_at + UNDO_TOAST_SECS)
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
            CompactSetting::Language(lang) => {
                self.settings.language = lang;
                // The phone's model guidance is language-aware (Indonesian is
                // steered off tiny), so the rows are re-derived here rather
                // than only on the next launch.
                let installed: Vec<String> = self
                    .models
                    .iter()
                    .filter(|row| row.installed)
                    .map(|row| row.id.clone())
                    .collect();
                self.refresh_models(&installed);
            }
            CompactSetting::Appearance(appearance) => self.settings.appearance = appearance,
            CompactSetting::KeepAudio(keep) => self.settings.keep_audio = keep,
            CompactSetting::FirstRunSeen(seen) => self.first_run_seen = seen,
            CompactSetting::WatchEnabled(enabled) => {
                self.watch_enabled = enabled;
                if !enabled {
                    // The row goes away with the switch; the ledger keeps its
                    // rows (re-enabling the same folder must not re-import).
                    self.watch = None;
                }
            }
        }
    }

    /// M24A: the host seeded (or refreshed) the watch state from the store
    /// settings. `folder` is the display name; `status` is `None` while the
    /// watcher has not polled yet (the Settings row still shows the folder).
    pub fn set_watch(
        &mut self,
        enabled: bool,
        folder: impl Into<String>,
        status: Option<WatchView>,
    ) {
        self.watch_enabled = enabled;
        self.watch_folder = folder.into();
        self.watch = if enabled { status } else { None };
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
///
/// `installed` names the models whose files are **verified** (M19B); every
/// other row is `Absent`. The host uses [`model_rows_from_states`] with its
/// real disk scan so `.part` and wrong-sized files stay honest.
pub fn model_rows(profile: DeviceProfile, language: &str, installed: &[String]) -> Vec<ModelRow> {
    let disk = disk_from_ids(installed);
    model_rows_from_states(profile, language, &disk)
}

/// The model rows for a device and a full disk scan (M19B).
pub fn model_rows_from_states(
    profile: DeviceProfile,
    language: &str,
    disk: &[(&'static str, ModelFileState)],
) -> Vec<ModelRow> {
    let recommended = device::recommend(profile, language).default_model_id;
    MODELS
        .iter()
        .map(|model| {
            let state = disk
                .iter()
                .find(|(id, _)| *id == model.id)
                .map(|(_, state)| *state)
                .unwrap_or(ModelFileState::Absent);
            ModelRow {
                id: model.id.to_string(),
                size_bytes: model.size_bytes,
                // A role that only names the desktop is not phone copy.
                role: if model.role == "desktop default" {
                    String::new()
                } else {
                    model.role.to_string()
                },
                note: model.phone.note.to_string(),
                offered: device::refusal_for(profile, language, model.id).is_none(),
                installed: state.is_verified(),
                disk: state,
                recommended: Some(model.id) == recommended,
            }
        })
        .collect()
}

/// `["base"]` → every manifest id with `Verified` for the named ones and
/// `Absent` for the rest (the fixture form of a disk scan).
fn disk_from_ids(installed: &[String]) -> Vec<(&'static str, ModelFileState)> {
    MODELS
        .iter()
        .map(|model| {
            let state = if installed.iter().any(|id| id == model.id) {
                ModelFileState::Verified
            } else {
                ModelFileState::Absent
            };
            (model.id, state)
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
    fn the_two_search_sections_are_separate_and_clear_together() {
        let mut app = app();
        // Capture the fixture's size rather than a magic number: the plain
        // list is what must survive a search round-trip, whatever it holds.
        let plain_rows = app.rows.len();
        app.set_library_query("rapat");
        // Title matches come from the loaded rows; spoken hits come from the
        // store (the host fills them). One of each here.
        assert_eq!(app.title_matches().len(), 1);
        assert_eq!(app.title_matches()[0].title, "Rapat tim");
        let id = app.title_matches()[0].id;
        app.set_search_hits(vec![LibraryHit {
            id,
            title: "Rapat tim".to_string(),
            snippet: "Kita membahas [rapat] anggaran".to_string(),
            segment_idx: Some(0),
            start_ms: 12_000,
            speaker: Some("Budi".to_string()),
            rank: -1.5,
        }]);
        assert_eq!(app.transcript_hits().len(), 1);
        assert_eq!(app.transcript_hits()[0].segment_idx, Some(0));
        assert!(app.search_has_results());

        // A new query drops the old hits immediately; clearing restores the
        // plain list. The title section is a live filter, not stale state.
        app.set_library_query("zzz");
        assert!(app.search_hits.is_empty());
        assert_eq!(app.title_matches().len(), 0, "wrong query, no title hit");
        app.set_library_query("");
        assert!(app.search_hits.is_empty());
        assert!(app.title_matches().is_empty());
        assert_eq!(app.rows.len(), plain_rows, "the plain list is intact");
    }

    #[test]
    fn a_search_hit_reveals_its_segment_on_the_existing_seek_path() {
        let mut app = app();
        let first = app.transcripts[0].clone();
        app.open_editor(first, 0.0);
        let editor = app.editor.as_mut().expect("editor");
        editor.reveal_segment(1);
        assert_eq!(editor.selected, Some(1));
        assert_eq!(editor.focus_segment, Some(1));
        assert!(editor.focus_scroll);
        assert_eq!(
            editor.playhead_ms, 4_000,
            "the playhead parks on the hit's start"
        );
    }

    #[test]
    fn library_facets_attach_by_transcript_id() {
        let mut app = app();
        let id = app.rows[1].id;
        app.set_library_facets(
            [(
                id,
                LibraryFacets {
                    speakers: 5,
                    waveform: vec![0.1, 0.9],
                },
            )]
            .into_iter()
            .collect(),
        );
        assert_eq!(app.rows[1].speakers, 5);
        assert_eq!(app.rows[1].waveform, [0.1, 0.9]);
        assert_eq!(app.rows[0].speakers, 0, "absent facets stay honest");
        assert!(app.rows[0].waveform.is_empty());
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
        match editor.export_request(false, false) {
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
        match editor.export_request(true, false) {
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

    // ---- M19B: the model registry tells the truth --------------------------

    fn profile_at(ram_gb: f64) -> DeviceProfile {
        DeviceProfile::new(
            (ram_gb * 1_000_000_000.0) as u64,
            1_000_000_000,
            false,
            true,
        )
    }

    fn rows_at(ram_gb: f64, disk: &[(&'static str, ModelFileState)]) -> Vec<ModelRow> {
        model_rows_from_states(profile_at(ram_gb), "en", disk)
    }

    fn find_row<'a>(rows: &'a [ModelRow], id: &str) -> &'a ModelRow {
        rows.iter().find(|row| row.id == id).expect(id)
    }

    fn live(model: &str, state: DownloadState) -> DownloadView {
        DownloadView {
            model: model.to_string(),
            bytes_done: 42,
            bytes_total: 100,
            percent: 42,
            state,
            message: String::new(),
            resumed: false,
        }
    }

    #[test]
    fn the_tier_default_is_the_recommended_row_at_4_6_8_12_16_gb() {
        let cases = [
            (4.0, "base"),
            (6.0, "base"),
            (8.0, "small"),
            (12.0, "small"),
            (16.0, "small"),
        ];
        for (ram_gb, expected) in cases {
            let rows = rows_at(ram_gb, &[]);
            let recommended: Vec<&str> = rows
                .iter()
                .filter(|row| row.recommended)
                .map(|row| row.id.as_str())
                .collect();
            assert_eq!(recommended, vec![expected], "{ram_gb} GB");
            assert!(
                find_row(&rows, expected).offered,
                "{ram_gb} GB must offer it"
            );
        }
    }

    #[test]
    fn the_five_row_states_are_reachable_from_disk_truth() {
        // Nothing on disk: the configured model is honestly "not downloaded".
        let rows = rows_at(8.0, &[("small", ModelFileState::Absent)]);
        let row = find_row(&rows, "small");
        assert_eq!(row.state(true, None), ModelState::NotDownloaded);
        assert_eq!(row.state(true, None).action(), "Get");
        assert_eq!(row.state(true, None).label(), "not downloaded");
        assert!(!row.state(true, None).is_in_use(), "no file, no 'In use'");

        // A `.part` — or a live job — is the Downloading state.
        let rows = rows_at(8.0, &[("small", ModelFileState::Partial)]);
        let row = find_row(&rows, "small");
        assert_eq!(row.state(false, None), ModelState::Downloading);
        let running = live("small", DownloadState::Running);
        assert_eq!(row.state(false, Some(&running)), ModelState::Downloading);
        assert_eq!(
            row.state(false, Some(&running)).action(),
            "Cancel",
            "a live transfer offers Cancel"
        );
        let paused = live("small", DownloadState::Paused);
        assert_eq!(row.state(false, Some(&paused)), ModelState::Downloading);

        // A wrong-sized file is stale: re-download, never install/use.
        let rows = rows_at(8.0, &[("small", ModelFileState::WrongSize)]);
        let row = find_row(&rows, "small");
        assert_eq!(row.state(true, None), ModelState::StaleCorrupt);
        assert_eq!(row.state(true, None).action(), "Re-download");

        // Verified but not selected → Use; verified + selected → In use.
        let rows = rows_at(8.0, &[("small", ModelFileState::Verified)]);
        let row = find_row(&rows, "small");
        assert!(row.installed);
        assert_eq!(row.state(false, None), ModelState::Installed);
        assert_eq!(row.state(false, None).action(), "Use");
        assert_eq!(row.state(true, None), ModelState::InUse);
        assert_eq!(row.state(true, None).action(), "In use");
        assert!(row.state(true, None).is_in_use());

        // The five labels/actions are distinct and exhaustive.
        let mut slugs: Vec<&str> = ModelState::ALL.iter().map(|state| state.slug()).collect();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), 5);
        assert_eq!(
            ModelState::ALL.map(|state| state.label()),
            [
                "not downloaded",
                "downloading",
                "stale file",
                "installed",
                "in use"
            ]
        );
    }

    #[test]
    fn in_use_is_impossible_without_a_verified_file() {
        let states = [
            ModelFileState::Absent,
            ModelFileState::Partial,
            ModelFileState::WrongSize,
            ModelFileState::Verified,
        ];
        let views = [
            None,
            Some(live("base", DownloadState::Running)),
            Some(live("base", DownloadState::Paused)),
            Some(live("base", DownloadState::Failed)),
        ];
        for disk in states {
            for selected in [false, true] {
                for view in &views {
                    let rows = rows_at(8.0, &[("base", disk)]);
                    let row = find_row(&rows, "base");
                    let state = row.state(selected, view.as_ref());
                    let expected = disk == ModelFileState::Verified && selected;
                    assert_eq!(
                        state == ModelState::InUse,
                        expected,
                        "disk={disk:?} selected={selected} view={:?}",
                        view.as_ref().map(|view| view.state)
                    );
                    // The row's `installed` is exactly the verified flag, so a
                    // UI that keys "In use" off it cannot lie either.
                    assert_eq!(row.installed, disk == ModelFileState::Verified);
                }
            }
        }
    }

    #[test]
    fn the_header_row_names_the_configured_model_and_its_disk_truth() {
        // Before the shell reports memory there is nothing to qualify.
        let mut app = CompactApp::new(CompactSettings::default());
        assert_eq!(app.model_label(), ("turbo-q5-0".to_string(), false));
        assert!(app.can_record(), "no device report yet: do not block");

        // An 8 GB phone whose configured model is the tier default, with no
        // file: the header says so, in warning ink.
        app.settings.model_id = "small".to_string();
        app.seed_device(profile_at(8.0), &[]);
        assert_eq!(
            app.model_label(),
            ("small · not downloaded".to_string(), true)
        );
        assert!(!app.can_record());
        assert_eq!(
            app.record_block_reason(),
            Some("Download a model before recording.")
        );

        // The file lands and verifies: the suffix goes away.
        app.refresh_models(&["small".to_string()]);
        assert_eq!(app.model_label(), ("small".to_string(), false));
        assert!(app.can_record());
        assert_eq!(app.record_block_reason(), None);
        assert_eq!(
            app.configured_model().map(|row| row.state(true, None)),
            Some(ModelState::InUse)
        );

        // A configured model cannot borrow another installed model's file.
        app.settings.model_id = "turbo-q5-0".to_string();
        assert_eq!(
            app.model_label(),
            ("turbo-q5-0 · not downloaded".to_string(), true),
            "base/small being installed does not make turbo usable"
        );
        assert!(!app.can_record(), "the job loads the configured model");
        assert_eq!(
            app.record_block_reason(),
            Some("Choose a model that is on this phone.")
        );
    }

    #[test]
    fn a_partial_or_stale_file_does_not_unlock_recording() {
        let mut app = CompactApp::new(CompactSettings {
            model_id: "small".to_string(),
            ..CompactSettings::default()
        });
        for disk in [
            ModelFileState::Partial,
            ModelFileState::WrongSize,
            ModelFileState::Absent,
        ] {
            app.seed_device_states(profile_at(8.0), &[("small", disk)]);
            assert!(!app.can_record(), "{disk:?} must not unlock Record");
            assert!(app.model_card_needed(), "{disk:?} still needs a model");
            assert!(!app.models.iter().any(|row| row.installed));
        }
        app.seed_device_states(profile_at(8.0), &[("small", ModelFileState::Verified)]);
        assert!(app.can_record());
        assert!(!app.model_card_needed());
    }

    #[test]
    fn an_unsupported_cpu_says_why_recording_is_blocked() {
        let mut app = CompactApp::new(CompactSettings::default());
        app.seed_device(
            DeviceProfile::new(8_000_000_000, 4_000_000_000, false, false),
            &[],
        );
        assert!(!app.can_record());
        assert_eq!(
            app.record_block_reason(),
            Some("No model can run on this phone.")
        );
    }

    #[test]
    fn the_desktop_role_is_not_phone_copy() {
        let rows = rows_at(8.0, &[("turbo-q5-0", ModelFileState::Verified)]);
        let turbo = find_row(&rows, "turbo-q5-0");
        assert_eq!(turbo.role, "", "the catalog role is desktop-only copy");
        assert_eq!(turbo.size_role(), "574 MB");
        for row in &rows {
            assert!(!row.role.contains("macOS"), "{}: {}", row.id, row.role);
            assert!(!row.note.contains("macOS"), "{}: {}", row.id, row.note);
        }
        // The size is the whole phone subtitle — the manifest role never
        // reaches the screen, even when it is a seemingly harmless word.
        let small = find_row(&rows, "small");
        assert_eq!(small.role, "fast");
        assert_eq!(small.size_role(), "488 MB");
    }

    #[test]
    fn the_phone_tiers_are_human_and_the_ids_stay_machine_readable() {
        let rows = rows_at(12.0, &[]);
        let expected = [
            ("tiny", "Quick test", "78 MB"),
            ("base", "Compact", "148 MB"),
            ("small", "Balanced", "488 MB"),
            ("turbo-q5-0", "Highly accurate", "574 MB"),
        ];
        for (id, name, size) in expected {
            let row = find_row(&rows, id);
            assert_eq!(row.tier_name(), name, "{id}");
            assert_eq!(row.tier_line(), size, "{id}");
            assert!(
                !row.phone_note().contains(id),
                "{}: the note must not name the manifest id: {}",
                id,
                row.phone_note()
            );
        }
        // The hidden manifest entries still answer with a human name (no id
        // leakage if a future phone offers them).
        for row in &rows {
            assert_ne!(row.tier_name(), "Model", "{}", row.id);
        }
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
            ..
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

    // ---- M13g3: first-run, download, onboarding, diagnostics, words --------

    #[test]
    fn a_phone_with_no_installed_model_needs_the_download_card() {
        let mut app = CompactApp::new(CompactSettings::default());
        assert!(!app.model_card_needed(), "no device report yet: no card");
        app.seed_device(
            DeviceProfile::new(6_000_000_000, 2_000_000_000, false, true),
            &[],
        );
        assert!(app.model_card_needed());
        assert_eq!(
            app.recommended_model().map(|row| row.id.as_str()),
            Some("base"),
            "M13D: 6 GB defaults to base"
        );
        let offered: Vec<&str> = app
            .offered_models()
            .iter()
            .map(|row| row.id.as_str())
            .collect();
        assert_eq!(offered.first().copied(), Some("base"), "recommended first");
        assert!(
            app.language_note().is_none(),
            "English has no steering line"
        );

        // Once a model is installed the card is gone; the Library lists rows.
        app.seed_device(
            DeviceProfile::new(6_000_000_000, 2_000_000_000, false, true),
            &["base".to_string()],
        );
        assert!(!app.model_card_needed());
    }

    #[test]
    fn indonesian_is_steered_off_tiny_by_the_card() {
        let settings = CompactSettings {
            language: "id".to_string(),
            ..CompactSettings::default()
        };
        let mut app = CompactApp::new(settings);
        app.seed_device(
            DeviceProfile::new(6_000_000_000, 2_000_000_000, false, true),
            &[],
        );
        assert_eq!(
            app.recommended_model().map(|row| row.id.as_str()),
            Some("base"),
            "Indonesian still defaults to base, never tiny"
        );
        let note = app.language_note().expect("Indonesian steering");
        assert!(note.contains("tiny"), "{note}");
        assert!(note.contains("base or small"), "{note}");

        // A language change re-derives the rows through the stored profile.
        let settings = CompactSettings {
            language: "en".to_string(),
            ..CompactSettings::default()
        };
        let mut app = CompactApp::new(settings);
        app.seed_device(
            DeviceProfile::new(6_000_000_000, 2_000_000_000, false, true),
            &[],
        );
        app.apply_setting(CompactSetting::Language("id".to_string()));
        assert!(app.language_note().is_some(), "the note follows the change");
    }

    #[test]
    fn the_download_card_uses_the_jobs_percent_and_offers_the_resume() {
        let mut app = CompactApp::new(CompactSettings::default());
        app.seed_device(
            DeviceProfile::new(6_000_000_000, 2_000_000_000, false, true),
            &[],
        );
        assert!(app.on_download(
            "base",
            61_000_000,
            143_000_000,
            42,
            DownloadState::Running,
            "starting",
            1.0
        ));
        let view = app.download.clone().expect("download");
        assert_eq!(view.percent, 42, "the job's own integer, never recomputed");
        assert!(view.line().contains("42%"));
        assert!(!app.on_download(
            "base",
            61_000_000,
            143_000_000,
            42,
            DownloadState::Running,
            "starting",
            2.0
        ));

        app.on_download_resume("base", 61_000_000, 143_000_000, 42);
        let view = app.download.clone().expect("resume");
        assert_eq!(view.state, DownloadState::Paused);
        assert!(view.resumed);
        assert!(view.line().contains("Paused at 42%"));
    }

    #[test]
    fn the_free_space_line_refuses_a_download_that_cannot_fit() {
        let mut app = CompactApp::new(CompactSettings::default());
        app.seed_device(
            DeviceProfile::new(6_000_000_000, 2_000_000_000, false, true),
            &[],
        );
        app.set_free_space(Some(50_000_000));
        let base = app.recommended_model().expect("base").clone();
        assert_eq!(app.has_space_for(base.size_bytes), Some(false));
        app.set_free_space(Some(9_000_000_000));
        assert_eq!(app.has_space_for(base.size_bytes), Some(true));
    }

    #[test]
    fn the_intro_is_one_time_and_back_counts_as_skip() {
        let mut first_app = app();
        assert!(first_app.maybe_begin_onboarding(0.0));
        assert_eq!(first_app.sheet, Some(Sheet::Onboarding));
        assert_eq!(
            first_app.sheet_anim.curve(),
            super::super::motion::SheetCurve::Spring
        );

        // A second call while the sheet is open does nothing.
        assert!(!first_app.maybe_begin_onboarding(0.1));

        // Back is Skip: the sheet dismisses and the flag is stashed for the
        // host to persist (the gesture has no action vector).
        assert_eq!(first_app.back(0.2), BackOutcome::Dismissed);
        assert!(first_app.take_onboarding_seen());
        assert!(first_app.first_run_seen);
        assert!(!first_app.take_onboarding_seen(), "one-shot");
        first_app.advance(0.5);
        assert!(first_app.sheet.is_none());
        assert!(!first_app.maybe_begin_onboarding(1.0), "never again");

        // Start emits the host action and marks the sheet seen.
        let mut second = app();
        second.maybe_begin_onboarding(0.0);
        assert_eq!(
            second.finish_onboarding(0.0, false),
            CompactAction::OnboardingSeen { skipped: false }
        );
        assert!(second.first_run_seen);
    }

    #[test]
    fn the_editor_highlights_the_word_the_playhead_is_in() {
        let mut app = app();
        let first = app.transcripts[0].clone();
        app.open_editor(first, 0.0);
        let mut alignment = WordAlignment::default();
        alignment.current.insert(
            0,
            vec![
                WordTiming {
                    segment_idx: 0,
                    word_idx: 0,
                    start_ms: 0,
                    end_ms: 400,
                    text: "the".to_string(),
                },
                WordTiming {
                    segment_idx: 0,
                    word_idx: 1,
                    start_ms: 400,
                    end_ms: 900,
                    text: "quick".to_string(),
                },
                WordTiming {
                    segment_idx: 0,
                    word_idx: 2,
                    start_ms: 900,
                    end_ms: 1_500,
                    text: "brown".to_string(),
                },
            ],
        );
        app.editor.as_mut().expect("editor").set_words(alignment);
        let editor = app.editor.as_ref().expect("editor");
        assert_eq!(word_at(editor.words_for(0), 0), Some(0));
        assert_eq!(word_at(editor.words_for(0), 399), Some(0));
        assert_eq!(word_at(editor.words_for(0), 400), Some(1));
        assert_eq!(word_at(editor.words_for(0), 1_400), Some(2));
        assert_eq!(editor.current_word(0), Some(0), "the playhead starts at 0");

        // The transport clock moves the playhead and stops at the duration.
        app.editor.as_mut().expect("editor").start_playback();
        assert!(app.advance(1.0));
        assert!(app.advance(1.45));
        let editor = app.editor.as_ref().expect("editor");
        assert!(
            editor.playhead_ms >= 400,
            "playhead advanced: {}",
            editor.playhead_ms
        );
        assert_eq!(editor.current_word(0), Some(1));
        assert!(app.advance(1.5));
        assert!(!app.advance(20.0));
        assert!(!app.editor.as_ref().expect("editor").playing);
        assert_eq!(
            app.editor.as_ref().expect("editor").playhead_ms,
            app.editor.as_ref().expect("editor").duration_ms()
        );
    }

    #[test]
    fn a_host_playhead_is_clamped_and_wakes_the_surface_once() {
        let mut app = app();
        let first = app.transcripts[0].clone();
        app.open_editor(first, 0.0);
        app.on_playhead(1_000);
        assert_eq!(app.editor.as_ref().expect("editor").playhead_ms, 1_000);
        assert!(app.take_frame_request());
        app.on_playhead(99_999_999);
        assert_eq!(
            app.editor.as_ref().expect("editor").playhead_ms,
            app.editor.as_ref().expect("editor").duration_ms()
        );
    }

    #[test]
    fn diagnostics_lives_behind_settings_and_keeps_its_counters() {
        let mut app = app();
        app.open_settings(0.0);
        app.advance(1.0);
        app.open_diagnostics(1.0);
        assert_eq!(app.nav.current(), CompactRoute::Diagnostics);
        assert_eq!(
            app.nav.screens(),
            &[
                CompactRoute::Library,
                CompactRoute::Settings,
                CompactRoute::Diagnostics
            ]
        );
        app.set_diagnostics(DiagnosticsView {
            text_chars: 42,
            ime_updates: 7,
            deletions: 2,
            resumes: 3,
            surface_recreates: 1,
            frames_gated: 9,
            frames_after_destroy: 4,
            probe_saves: 2,
            probe_restores: 2,
            avg_fps: Some(59.4),
            frames: 900,
            inset_updates: 5,
            inset_top: 28.0,
            inset_bottom: 20.0,
            back_presses: 1,
            crash: Some(CrashBanner {
                live: false,
                text: "panic: boom".to_string(),
            }),
        });
        assert_eq!(app.diagnostics.text_chars, 42);
        assert_eq!(
            app.diagnostics.crash.as_ref().unwrap().title(),
            "CRASH — LAST LAUNCH (read from the crash file)"
        );
        assert_eq!(app.back(2.0), BackOutcome::Popped);
        assert_eq!(app.nav.current(), CompactRoute::Settings);
    }

    #[test]
    fn the_notes_sheet_runs_the_shared_state_machine() {
        let mut app = app();
        app.notes_configured = true;
        let first = app.transcripts[0].clone();
        app.open_editor(first, 0.0);

        let action = app.open_notes_sheet(1.0).expect("the sheet asks for rows");
        assert!(matches!(action, CompactAction::LoadNotes { .. }));
        assert_eq!(app.sheet, Some(Sheet::Notes));
        assert_eq!(app.notes.stored.len(), 0);

        app.select_note_preset(NotePreset::ActionItems);
        let action = app.start_note().expect("a request");
        match action {
            CompactAction::GenerateNote { request } => {
                assert_eq!(request.preset, NotePreset::ActionItems);
                assert!(!request.segments.is_empty());
            }
            other => panic!("expected a notes request, got {other:?}"),
        }
        assert!(app.notes.phase.is_running());
        app.select_note_preset(NotePreset::Minutes);
        assert_eq!(
            app.notes.preset,
            NotePreset::ActionItems,
            "a selection cannot move mid-run"
        );

        let outcome = NotesOutcome {
            preset: NotePreset::ActionItems,
            body_md: "# Action items\n\n- ship [00:04]\n".to_string(),
            citations_checked: 1,
            citations_removed: 0,
            breakdown: Default::default(),
            parts: 1,
            model: "mock".to_string(),
        };
        let action = app.on_notes_done(Ok(outcome)).expect("a save");
        assert!(matches!(action, CompactAction::SaveNote { .. }));
        assert_eq!(
            app.notes.result_line().as_deref(),
            Some("1 citations checked · 0 removed")
        );
        match app.note_export_action().expect("export") {
            CompactAction::ExportNote { name, markdown } => {
                assert!(name.ends_with("— Action items.md"), "unexpected: {name}");
                assert_eq!(markdown, "# Action items\n\n- ship [00:04]\n");
            }
            other => panic!("expected an export, got {other:?}"),
        }
        match app.note_copy_action().expect("copy") {
            CompactAction::CopyNote { text } => {
                assert_eq!(text, "# Action items\n\n- ship [00:04]\n")
            }
            other => panic!("expected a copy, got {other:?}"),
        }

        // The stored rows come back from the host in product order.
        app.on_notes_loaded(
            app.notes.transcript.unwrap(),
            Ok(vec![
                suramak_core::notes::StoredNote {
                    preset: NotePreset::BilingualBrief,
                    body_md: "# Brief (EN↔ID)\n".to_string(),
                    citations_checked: 0,
                    citations_removed: 0,
                    breakdown: Default::default(),
                    model: "mock".to_string(),
                    created_at: "2026-01-01T00:00:00+00:00".to_string(),
                },
                suramak_core::notes::StoredNote {
                    preset: NotePreset::Minutes,
                    body_md: "# Minutes\n".to_string(),
                    citations_checked: 2,
                    citations_removed: 1,
                    breakdown: Default::default(),
                    model: "mock".to_string(),
                    created_at: "2026-01-01T00:00:00+00:00".to_string(),
                },
            ]),
        );
        assert_eq!(
            app.notes
                .stored
                .iter()
                .map(|note| note.preset)
                .collect::<Vec<_>>(),
            vec![NotePreset::Minutes, NotePreset::BilingualBrief],
            "the host's rows are sorted by preset"
        );
        assert!(app.back(2.0) == BackOutcome::Dismissed);
    }

    #[test]
    fn the_phone_notes_sheet_refuses_run_when_not_configured() {
        let mut app = app();
        app.notes_configured = false;
        let first = app.transcripts[0].clone();
        app.open_editor(first, 0.0);
        app.open_notes_sheet(1.0);
        assert!(!app.notes.can_run());
        assert!(app.start_note().is_none());
        let (text, error) = app.status.as_ref().expect("the refusal is named");
        assert!(text.contains("not configured"));
        assert!(error);
    }
}
