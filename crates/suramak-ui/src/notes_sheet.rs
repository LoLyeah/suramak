//! M24B: the Notes sheet's shared state machine — one implementation for the
//! phone and the desktop.
//!
//! The sheet lists the three presets, runs one through the polish client's own
//! endpoint, keeps the stored result per preset, and shows the validator's one
//! honest line (`12 citations checked · 1 removed (no matching audio)`).
//! Everything here is egui-free and unit-tested headless; each surface only
//! paints it and forwards a Run/Export/Copy intent to its host.

use suramak_core::notes::{NotePreset, NotesOutcome, StoredNote};
use suramak_core::transcript::Transcript;
use suramak_core::Uuid;

/// Shown when the Notes action is opened (or Run pressed) with no endpoint.
pub const NOTES_SETUP_NOTE: &str =
    "notes are not configured — set the polish base URL in Settings \
and provide an API key (OS keychain on macOS/Windows; SURAMAK_OPENAI_API_KEY on Linux)";

/// The polite note under a very short transcript. Notes are allowed; they are
/// just short.
pub const NOTES_SHORT_NOTE: &str = "this transcript is under 20 s — the notes will be short";

/// Where the sheet is in its lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotesPhase {
    /// Viewing the presets and any stored result; Run is available.
    Ready,
    /// One request per preset is in flight.
    Running { preset: NotePreset },
    /// The run failed. `message` is the raw backend/reply error (status and
    /// endpoint message included); the stored notes are untouched.
    Failed { preset: NotePreset, message: String },
}

impl NotesPhase {
    pub fn is_running(&self) -> bool {
        matches!(self, NotesPhase::Running { .. })
    }
}

/// M25C: which timeline a notes run reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoteScope {
    /// One part's own segments and clock (M24B).
    ThisPart,
    /// Every part in order on the session-absolute clock (M25C).
    #[default]
    WholeSession,
}

impl NoteScope {
    /// The two toggle labels, in order.
    pub const LABELS: [&'static str; 2] = ["This part", "Whole session"];

    pub fn label(self) -> &'static str {
        match self {
            NoteScope::ThisPart => "This part",
            NoteScope::WholeSession => "Whole session",
        }
    }
}

/// The Notes sheet's whole state.
#[derive(Debug, Clone, PartialEq)]
pub struct NotesSheet {
    /// Whether the sheet is on screen.
    pub open: bool,
    /// The transcript the stored notes belong to.
    pub transcript: Option<Uuid>,
    /// M25C: the session the open part belongs to, when it has one.
    pub session: Option<Uuid>,
    /// M25C: parts that exist / parts the session declares.
    pub transcribed: u32,
    pub part_count: u32,
    /// M25C: `This part` / `Whole session` (defaults to the whole session when
    /// the transcript belongs to one).
    pub scope: NoteScope,
    /// The selected preset row.
    pub preset: NotePreset,
    pub phase: NotesPhase,
    /// The stored notes for the open transcript, in product order.
    pub stored: Vec<StoredNote>,
    /// Whether the polish endpoint is configured (base URL + API key) — the
    /// Run button's enable and the not-configured state.
    pub configured: bool,
    /// The path the most recent run took (`single request` / `N chunks
    /// merged`), shown with the result until the sheet closes.
    pub path: Option<String>,
    /// Whether the open transcript is under 20 s (`NOTES_SHORT_NOTE`).
    pub short: bool,
    /// The `Export .md` destination (the desktop's path field; the phone host
    /// supplies its own SAF name and ignores this).
    pub dest: String,
}

impl Default for NotesSheet {
    fn default() -> Self {
        Self {
            open: false,
            transcript: None,
            session: None,
            transcribed: 0,
            part_count: 0,
            scope: NoteScope::ThisPart,
            preset: NotePreset::Minutes,
            phase: NotesPhase::Ready,
            stored: Vec::new(),
            configured: false,
            path: None,
            short: false,
            dest: String::new(),
        }
    }
}

/// One painted line of a stored note: a heading or body text. Markdown's
/// emphasis markers and list dashes are stripped — the surface renders the
/// words with the existing tokens, never a second typographic language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteLine {
    pub text: String,
    /// `1` for an `#`/`##`/`###` heading, `0` for body text.
    pub level: u8,
}

/// Splits a stored note into paintable lines. Blank lines are dropped.
pub fn note_lines(body: &str) -> Vec<NoteLine> {
    let mut lines = Vec::new();
    for raw in body.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        let text = if hashes > 0 {
            trimmed[hashes..].trim()
        } else if let Some(item) = trimmed.strip_prefix("- ") {
            item.trim()
        } else {
            trimmed
        };
        let text = text.replace("**", "");
        let level = u8::try_from(hashes.min(3)).unwrap_or(3);
        lines.push(NoteLine { text, level });
    }
    lines
}

impl NotesSheet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the sheet with this transcript's stored notes (freshly read) and
    /// the endpoint gate.
    pub fn open(&mut self, configured: bool, stored: Vec<StoredNote>, transcript: &Transcript) {
        self.open_with_session(configured, stored, transcript, None);
    }

    /// M25C: opens the sheet on a part that belongs to a session.
    ///
    /// The scope defaults to `Whole session` (the meeting is what the user is
    /// usually after). The stored rows are whatever the host read for that
    /// scope; `transcribed` / `part_count` drive the honest `2 of 3` line.
    pub fn open_with_session(
        &mut self,
        configured: bool,
        mut stored: Vec<StoredNote>,
        transcript: &Transcript,
        session: Option<(Uuid, u32, u32)>,
    ) {
        sort_notes(&mut stored);
        self.transcript = Some(transcript.id);
        self.stored = stored;
        self.configured = configured;
        self.open = true;
        self.phase = NotesPhase::Ready;
        self.short = transcript
            .segments
            .iter()
            .map(|segment| segment.end_ms)
            .max()
            .unwrap_or(0)
            < 20_000;
        self.path = None;
        match session {
            Some((session_id, part_count, transcribed)) => {
                self.session = Some(session_id);
                self.part_count = part_count;
                self.transcribed = transcribed;
                self.scope = NoteScope::WholeSession;
            }
            None => {
                self.session = None;
                self.part_count = 0;
                self.transcribed = 0;
                self.scope = NoteScope::ThisPart;
            }
        }
        // Prefer the first preset that has a result; otherwise Minutes.
        if let Some(first) = self.stored.first() {
            self.preset = first.preset;
        }
    }

    /// M25C: switches the scope. Ignored while a run is in flight so a result
    /// can never land under the wrong timeline.
    pub fn set_scope(&mut self, scope: NoteScope) {
        if !self.phase.is_running() {
            self.scope = scope;
        }
    }

    /// M25C: the honest incomplete-session line, shown under the scope toggle.
    pub fn scope_note(&self) -> Option<String> {
        (self.session.is_some() && self.transcribed < self.part_count).then(|| {
            suramak_core::session::session_incomplete_line(
                self.transcribed as usize,
                self.part_count as usize,
            )
        })
    }

    /// Closes the sheet. A run in flight is not cancelled (batch only) — its
    /// result still lands in `stored`.
    pub fn close(&mut self) {
        self.open = false;
        if !self.phase.is_running() {
            self.phase = NotesPhase::Ready;
        }
    }

    /// Selects a preset row. Locked while a run is in flight so the result can
    /// never land on the wrong row.
    pub fn select(&mut self, preset: NotePreset) {
        if !self.phase.is_running() {
            self.preset = preset;
        }
    }

    /// Whether Run is available: configured, no run in flight.
    pub fn can_run(&self) -> bool {
        self.configured && !self.phase.is_running()
    }

    /// The Run action. Returns the preset to run, or the named refusal.
    ///
    /// M25C: a whole-session run over a session with a missing part is refused
    /// before the request, with `session_incomplete` and the real count.
    pub fn start(&mut self) -> Result<NotePreset, String> {
        if self.phase.is_running() {
            return Err("a notes run is already in progress".to_string());
        }
        if !self.configured {
            return Err(NOTES_SETUP_NOTE.to_string());
        }
        if self.scope == NoteScope::WholeSession {
            if let Some(reason) = suramak_core::session::session_incomplete_refusal(
                self.transcribed as usize,
                self.part_count as usize,
            ) {
                return Err(reason);
            }
        }
        self.phase = NotesPhase::Running {
            preset: self.preset,
        };
        self.path = None;
        Ok(self.preset)
    }

    /// The runtime finished the selected preset's run. A valid outcome
    /// replaces that preset's stored row; any failure keeps the old row and
    /// shows the raw message.
    pub fn on_done(
        &mut self,
        result: Result<NotesOutcome, String>,
        created_at: String,
    ) -> Option<StoredNote> {
        let preset = match &self.phase {
            NotesPhase::Running { preset } => *preset,
            // A stray result (no run in flight) — nothing to attach it to.
            _ => return None,
        };
        match result {
            Ok(outcome) => {
                if outcome.preset != preset {
                    self.phase = NotesPhase::Failed {
                        preset,
                        message: format!(
                            "the reply was for {} but {} was requested",
                            outcome.preset.label(),
                            preset.label()
                        ),
                    };
                    return None;
                }
                self.path = Some(NotePreset::path_label(outcome.parts));
                let note = outcome.stored(created_at);
                self.stored.retain(|existing| existing.preset != preset);
                self.stored.push(note.clone());
                sort_notes(&mut self.stored);
                self.phase = NotesPhase::Ready;
                Some(note)
            }
            Err(message) => {
                self.phase = NotesPhase::Failed { preset, message };
                None
            }
        }
    }

    /// Replaces the stored rows with a fresh store read (the sheet's reopen
    /// path). Keeps the current selection when it still has a result.
    pub fn adopt_stored(&mut self, mut stored: Vec<StoredNote>) {
        sort_notes(&mut stored);
        if !stored.iter().any(|note| note.preset == self.preset) {
            if let Some(first) = stored.first() {
                self.preset = first.preset;
            }
        }
        self.stored = stored;
    }

    /// The selected preset's stored result, if any.
    pub fn current(&self) -> Option<&StoredNote> {
        self.stored.iter().find(|note| note.preset == self.preset)
    }

    /// The exact Markdown `Export .md` writes (the stored body, byte-for-byte).
    pub fn export_text(&self) -> Option<&str> {
        self.current().map(|note| note.body_md.as_str())
    }

    /// The exact text `Copy` puts on the clipboard.
    pub fn copy_text(&self) -> Option<&str> {
        self.export_text()
    }

    /// The one honest line for the selected result.
    pub fn result_line(&self) -> Option<String> {
        self.current().map(StoredNote::result_line)
    }

    /// The breakdown line (only when something was removed in this session).
    pub fn breakdown_line(&self) -> Option<String> {
        self.current().and_then(StoredNote::breakdown_line)
    }

    /// The named failure, when the last run failed.
    pub fn failure(&self) -> Option<(&NotePreset, &str)> {
        match &self.phase {
            NotesPhase::Failed { preset, message } => Some((preset, message)),
            _ => None,
        }
    }

    /// Whether every preset has a stored note.
    pub fn all_generated(&self) -> bool {
        NotePreset::ALL
            .iter()
            .all(|preset| self.stored.iter().any(|note| note.preset == *preset))
    }

    /// The sheet's suggested export name for the selected preset.
    pub fn export_name(&self, transcript: &Transcript) -> String {
        let stem = crate::deck::compact::sanitise_stem(&transcript.title);
        format!("{stem} — {}.md", self.preset.label())
    }
}

fn sort_notes(notes: &mut [StoredNote]) {
    notes.sort_by_key(|note| {
        NotePreset::ALL
            .iter()
            .position(|preset| *preset == note.preset)
            .unwrap_or(usize::MAX)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_core::notes::CitationBreakdown;
    use suramak_core::transcript::{Segment, Source};

    fn transcript(seconds: i64) -> Transcript {
        Transcript::new(
            "Weekly sync".to_string(),
            Source::File {
                name: "Weekly sync.wav".to_string(),
            },
            "tiny".to_string(),
            Some("en".to_string()),
            None,
            vec![Segment {
                idx: 0,
                start_ms: 0,
                end_ms: seconds * 1000,
                text: "We decided to ship on Friday.".to_string(),
            }],
        )
    }

    fn stored(preset: NotePreset, checked: u32, removed: u32) -> StoredNote {
        StoredNote {
            preset,
            body_md: format!("# {}\n\n- shipped [00:00]\n", preset.heading()),
            citations_checked: checked,
            citations_removed: removed,
            breakdown: CitationBreakdown {
                removed,
                ..Default::default()
            },
            model: "mock-model".to_string(),
            created_at: "2026-01-01T00:00:00+00:00".to_string(),
        }
    }

    fn outcome(preset: NotePreset, checked: u32, removed: u32) -> NotesOutcome {
        NotesOutcome {
            preset,
            body_md: format!("# {}\n\n- shipped [00:00]\n", preset.heading()),
            citations_checked: checked,
            citations_removed: removed,
            breakdown: CitationBreakdown {
                removed,
                ..Default::default()
            },
            parts: 1,
            model: "mock-model".to_string(),
        }
    }

    #[test]
    fn the_sheet_lists_the_three_presets_in_product_order() {
        assert_eq!(
            NotePreset::ALL.map(NotePreset::label),
            ["Minutes", "Action items", "Bilingual brief (EN↔ID)"]
        );
        let mut sheet = NotesSheet::new();
        sheet.open(true, Vec::new(), &transcript(60));
        for preset in NotePreset::ALL {
            sheet.select(preset);
            assert_eq!(sheet.preset, preset);
        }
    }

    #[test]
    fn not_configured_is_a_named_refusal_and_run_stays_off() {
        let mut sheet = NotesSheet::new();
        sheet.open(false, Vec::new(), &transcript(60));
        assert!(!sheet.can_run());
        assert_eq!(sheet.start().unwrap_err(), NOTES_SETUP_NOTE);
        assert_eq!(sheet.phase, NotesPhase::Ready, "no run was started");
        assert_eq!(sheet.result_line(), None);

        sheet.configured = true;
        assert!(sheet.can_run());
        assert_eq!(sheet.start(), Ok(NotePreset::Minutes));
        assert_eq!(
            sheet.phase,
            NotesPhase::Running {
                preset: NotePreset::Minutes
            }
        );
        // A second start is refused while one is in flight.
        assert!(sheet.start().is_err());
    }

    #[test]
    fn the_result_line_reads_zero_removed_and_n_removed() {
        let mut sheet = NotesSheet::new();
        sheet.open(
            true,
            vec![
                stored(NotePreset::Minutes, 12, 0),
                stored(NotePreset::ActionItems, 5, 2),
            ],
            &transcript(60),
        );
        sheet.select(NotePreset::Minutes);
        assert_eq!(
            sheet.result_line().as_deref(),
            Some("12 citations checked · 0 removed")
        );
        assert_eq!(sheet.breakdown_line(), None);
        sheet.select(NotePreset::ActionItems);
        assert_eq!(
            sheet.result_line().as_deref(),
            Some("5 citations checked · 2 removed (no matching audio)")
        );
        assert_eq!(sheet.breakdown_line().as_deref(), Some("2 no match"));

        sheet.select(NotePreset::BilingualBrief);
        assert_eq!(sheet.result_line(), None);
    }

    #[test]
    fn no_citations_returned_is_said_plainly() {
        let mut sheet = NotesSheet::new();
        sheet.open(
            true,
            vec![stored(NotePreset::Minutes, 0, 0)],
            &transcript(60),
        );
        assert_eq!(
            sheet.result_line().as_deref(),
            Some("no citations returned")
        );
    }

    #[test]
    fn a_finished_run_stores_the_result_and_a_regenerate_replaces_it() {
        let mut sheet = NotesSheet::new();
        sheet.open(true, Vec::new(), &transcript(60));
        sheet.select(NotePreset::ActionItems);
        sheet.start().unwrap();
        sheet.on_done(
            Ok(outcome(NotePreset::ActionItems, 4, 1)),
            "2026-01-02T00:00:00+00:00".to_string(),
        );
        assert_eq!(sheet.phase, NotesPhase::Ready);
        assert_eq!(sheet.stored.len(), 1);
        assert_eq!(sheet.path.as_deref(), Some("single request"));
        assert_eq!(
            sheet.result_line().as_deref(),
            Some("4 citations checked · 1 removed (no matching audio)")
        );

        // Regenerate the same preset: one row, the new body.
        sheet.start().unwrap();
        sheet.on_done(
            Ok(NotesOutcome {
                body_md: "# Action items\n\n- newer [00:00]\n".to_string(),
                citations_checked: 9,
                citations_removed: 0,
                breakdown: CitationBreakdown::default(),
                parts: 3,
                ..outcome(NotePreset::ActionItems, 9, 0)
            }),
            "2026-01-03T00:00:00+00:00".to_string(),
        );
        assert_eq!(sheet.stored.len(), 1, "one row per preset");
        assert_eq!(
            sheet.stored[0].body_md,
            "# Action items\n\n- newer [00:00]\n"
        );
        assert_eq!(sheet.path.as_deref(), Some("3 chunks merged"));
        assert_eq!(
            sheet.result_line().as_deref(),
            Some("9 citations checked · 0 removed")
        );
    }

    #[test]
    fn a_failed_run_keeps_the_old_result_and_names_the_endpoint_error() {
        let mut sheet = NotesSheet::new();
        sheet.open(
            true,
            vec![stored(NotePreset::Minutes, 12, 1)],
            &transcript(60),
        );
        sheet.select(NotePreset::Minutes);
        sheet.start().unwrap();
        sheet.on_done(
            Err("Polish backend error: HTTP 503: model overloaded".to_string()),
            "2026-01-02T00:00:00+00:00".to_string(),
        );
        assert_eq!(
            sheet.phase,
            NotesPhase::Failed {
                preset: NotePreset::Minutes,
                message: "Polish backend error: HTTP 503: model overloaded".to_string()
            }
        );
        assert_eq!(sheet.stored.len(), 1, "the old result is untouched");
        assert_eq!(
            sheet.result_line().as_deref(),
            Some("12 citations checked · 1 removed (no matching audio)")
        );
        let (preset, message) = sheet.failure().expect("the failure is named");
        assert_eq!(*preset, NotePreset::Minutes);
        assert!(message.contains("HTTP 503"));
        assert!(message.contains("model overloaded"));
        // Retry is possible.
        assert!(sheet.can_run());
        assert_eq!(sheet.start(), Ok(NotePreset::Minutes));
    }

    #[test]
    fn a_reply_for_another_preset_is_refused() {
        let mut sheet = NotesSheet::new();
        sheet.open(true, Vec::new(), &transcript(60));
        sheet.select(NotePreset::Minutes);
        sheet.start().unwrap();
        sheet.on_done(
            Ok(outcome(NotePreset::ActionItems, 1, 0)),
            "2026-01-02T00:00:00+00:00".to_string(),
        );
        assert!(sheet.failure().is_some());
        assert!(sheet.stored.is_empty());
    }

    #[test]
    fn export_and_copy_are_the_exact_stored_markdown() {
        let mut sheet = NotesSheet::new();
        sheet.open(
            true,
            vec![stored(NotePreset::Minutes, 3, 0)],
            &transcript(60),
        );
        let exact = "# Minutes\n\n- shipped [00:00]\n".to_string();
        assert_eq!(sheet.export_text(), Some(exact.as_str()));
        assert_eq!(sheet.copy_text(), Some(exact.as_str()));
        sheet.select(NotePreset::BilingualBrief);
        assert_eq!(sheet.export_text(), None);
        assert_eq!(
            sheet.export_name(&transcript(60)),
            "Weekly sync — Bilingual brief (EN↔ID).md"
        );
    }

    #[test]
    fn a_short_transcript_is_allowed_and_says_so() {
        let mut sheet = NotesSheet::new();
        sheet.open(true, Vec::new(), &transcript(12));
        assert!(sheet.short);
        assert!(sheet.can_run(), "short is allowed, never blocked");
        let mut long = NotesSheet::new();
        long.open(true, Vec::new(), &transcript(21));
        assert!(!long.short);
    }

    #[test]
    fn stored_notes_open_in_product_order_and_the_first_is_selected() {
        let mut sheet = NotesSheet::new();
        sheet.open(
            true,
            vec![
                stored(NotePreset::BilingualBrief, 1, 0),
                stored(NotePreset::Minutes, 2, 0),
                stored(NotePreset::ActionItems, 3, 0),
            ],
            &transcript(60),
        );
        let order: Vec<NotePreset> = sheet.stored.iter().map(|note| note.preset).collect();
        assert_eq!(order, NotePreset::ALL.to_vec());
        assert_eq!(sheet.preset, NotePreset::Minutes);
        assert!(sheet.all_generated());
    }

    #[test]
    fn note_lines_strip_markdown_markers_without_a_second_type_language() {
        let lines = note_lines("# Minutes\n\n## Decisions\n\n- shipped **Friday**\n");
        assert_eq!(
            lines,
            vec![
                NoteLine {
                    text: "Minutes".to_string(),
                    level: 1
                },
                NoteLine {
                    text: "Decisions".to_string(),
                    level: 2
                },
                NoteLine {
                    text: "shipped Friday".to_string(),
                    level: 0
                }
            ]
        );
    }
}
