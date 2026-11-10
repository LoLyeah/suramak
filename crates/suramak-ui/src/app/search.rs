//! M15.7 — the library search state machine.
//!
//! The search box types into [`SearchState::query`]; after [`SEARCH_DEBOUNCE`]
//! the state emits one [`Effect::Search`], the runtime runs it against the
//! store's FTS5 index, and [`AppState::on_search_results`] stores the hits. A
//! response for a query the user has already moved past is dropped, so a slow
//! search never overwrites a newer one.
//!
//! Clicking a segment hit is the "seek" half of the workorder: the transcript
//! is opened through the ordinary load path, and once it lands the existing
//! playback transport ([`suramak_playback::Playback`]) is seeked to the
//! segment's start. The pending seek is keyed by transcript id, so a load that
//! fails, a different row, or a transcript switch can never seek the wrong
//! recording. A hit whose audio is gone still opens — [`SearchState::seek_note`]
//! says the playhead could not move and why (the documented flag), and the
//! result row stays.
//!
//! This module is egui-free: every rule below is unit-tested headless.

use std::time::{Duration, Instant};

use suramak_core::search::{SearchHit, SearchResults};
use uuid::Uuid;

use super::{AppState, Effect, Route};

/// How long after the last keystroke the query runs: long enough that typing a
/// word is one query, short enough that the list feels live.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(220);

/// The library search's state.
#[derive(Debug, Clone, Default)]
pub struct SearchState {
    /// What the box currently says.
    pub query: String,
    /// The query whose results are on screen (`None` before the first answer).
    pub applied: Option<String>,
    /// The ranked hits, best first (already limited by the store).
    pub results: Vec<SearchHit>,
    /// All matches before the limit, for the "showing N of M" caption.
    pub total: usize,
    /// Why the last search failed, if it did (shown instead of the list).
    pub error: Option<String>,
    /// Last keystroke; the debounce clock starts here.
    pub last_edit: Option<Instant>,
    /// The query currently being run (one at a time, no queue).
    pub in_flight: Option<String>,
    /// A seek waiting for its transcript to load: `(transcript, start_ms)`.
    pub pending_seek: Option<(Uuid, u32)>,
    /// The documented "seek could not move" flag (no audio / missing file),
    /// also used for the quiet "playhead at …" confirmation.
    pub seek_note: Option<String>,
}

impl SearchState {
    /// Whether the search surface is up (the box has a non-blank query).
    pub fn is_active(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// The debounce clock restarted: the user typed, so the previous results
    /// stay visible until the new answer lands.
    pub fn note_edit(&mut self, now: Instant) {
        self.last_edit = Some(now);
        // A note about the previous hit no longer describes what the reader is
        // looking at.
        self.seek_note = None;
    }

    /// Clears everything (Esc, or leaving the library).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The store changed under an active search: forget that the current results
    /// are settled, so the next tick re-runs the query (the debounce still
    /// applies). A no-op when the search surface is closed.
    pub fn invalidate(&mut self) {
        if self.is_active() {
            self.applied = None;
        }
    }
}

impl AppState {
    /// The search box changed (the view calls this on `response.changed()`).
    pub fn on_search_edited(&mut self, now: Instant) {
        self.search.note_edit(now);
    }

    /// The debounce poll, called from [`AppState::tick`] before its editor
    /// early-returns (search must run with no editor open).
    pub(crate) fn poll_search(&mut self, now: Instant) -> Vec<Effect> {
        let query = self.search.query.trim().to_string();
        if query.is_empty() {
            return Vec::new();
        }
        if self.search.applied.as_deref() == Some(query.as_str()) {
            return Vec::new();
        }
        if self.search.in_flight.as_deref() == Some(query.as_str()) {
            return Vec::new();
        }
        // Punctuation with no word in it is not a query: settling it here (as
        // an empty answer) keeps the surface on "No matches" instead of a
        // forever-"Searching…" and spares the store a pointless round trip.
        if suramak_core::search::match_query(&query).is_none() {
            self.search.results.clear();
            self.search.total = 0;
            self.search.error = None;
            self.search.applied = Some(query);
            return Vec::new();
        }
        let Some(last_edit) = self.search.last_edit else {
            // A query restored from outside the box (tests, a future "search
            // for this word" action) runs immediately.
            self.search.in_flight = Some(query.clone());
            return vec![Effect::Search { query }];
        };
        if now.saturating_duration_since(last_edit) < SEARCH_DEBOUNCE {
            return Vec::new();
        }
        self.search.in_flight = Some(query.clone());
        vec![Effect::Search { query }]
    }

    /// The runtime's answer. A response for a query the box no longer holds is
    /// discarded (the newer query's answer is the one that will land).
    pub fn on_search_results(&mut self, query: String, result: Result<SearchResults, String>) {
        if self.search.query.trim() != query {
            // The box moved on: drop the answer, but release the in-flight
            // marker it owned — otherwise retyping that exact query later
            // would be suppressed forever.
            if self.search.in_flight.as_deref() == Some(query.as_str()) {
                self.search.in_flight = None;
            }
            return;
        }
        if self.search.in_flight.as_deref() == Some(query.as_str()) {
            self.search.in_flight = None;
        }
        match result {
            Ok(results) => {
                self.search.total = results.total;
                self.search.results = results.hits;
                self.search.error = None;
            }
            Err(error) => {
                self.search.results.clear();
                self.search.total = 0;
                self.search.error = Some(error);
            }
        }
        self.search.applied = Some(query);
    }

    /// A result row was clicked: open its transcript and park the playhead at
    /// the segment's start.
    ///
    /// Title hits open without a seek; glossary/dictionary hits are not tied to
    /// a recording, so they open the Text rules section of Settings instead.
    pub fn open_search_hit(&mut self, hit: &SearchHit) -> Vec<Effect> {
        self.search.seek_note = None;
        let Some(id) = hit.transcript_id else {
            return self.open_settings();
        };
        self.search.pending_seek = hit
            .start_ms
            .map(|ms| (id, ms.clamp(0, u32::MAX as i64) as u32));

        // The transcript is already open: seek in place, no reload (the edit
        // buffers must not be thrown away for a click in the sidebar).
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.transcript.id == id)
        {
            self.route = Route::Editor;
            let mut effects = Vec::new();
            if let Some((_, ms)) = self.search.pending_seek.take() {
                effects.extend(self.seek_playback(ms));
                self.note_search_seek(ms);
            }
            return effects;
        }

        let Some(index) = self
            .library
            .items
            .iter()
            .position(|transcript| transcript.id == id)
        else {
            // The hit is real but the sidebar list is stale (a save landed
            // between the search and the click): refresh and drop the seek
            // rather than opening a row that is not there.
            self.search.pending_seek = None;
            return vec![Effect::RefreshLibrary];
        };
        let effects = self.select_library(index);
        if effects.is_empty() && self.route != Route::Editor {
            // The click was refused (a live recording owns the route).
            self.search.pending_seek = None;
        }
        effects
    }

    /// Records what the seek did: the playhead moved, or it could not (the
    /// documented missing-media flag — the result row itself is never removed).
    pub(super) fn note_search_seek(&mut self, ms: u32) {
        let time = fmt_timestamp(ms as i64);
        self.search.seek_note = Some(match self.playback.hint() {
            Some(hint) => format!("Opened at {time} — the playhead cannot move: {hint}"),
            None => format!("Playhead at {time}"),
        });
    }
}

/// `m:ss` under an hour, `h:mm:ss` above it — the same shape the editor's
/// gutter uses, without a leading zero hour.
pub fn fmt_timestamp(ms: i64) -> String {
    let total = ms.max(0) / 1_000;
    let seconds = total % 60;
    let minutes = (total / 60) % 60;
    let hours = total / 3_600;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_core::search::HitKind;
    use suramak_core::transcript::{Segment, Source, Transcript};

    fn transcript(title: &str) -> Transcript {
        Transcript::new(
            title.to_string(),
            Source::File {
                name: title.to_string(),
            },
            "tiny".to_string(),
            Some("id".to_string()),
            None,
            vec![Segment {
                idx: 0,
                start_ms: 1_500,
                end_ms: 2_400,
                text: "kita bahas anggaran".to_string(),
            }],
        )
    }

    fn hit(id: Uuid, start_ms: i64) -> SearchHit {
        SearchHit {
            kind: HitKind::Segment,
            transcript_id: Some(id),
            title: Some("rapat.wav".to_string()),
            segment_idx: Some(0),
            start_ms: Some(start_ms),
            end_ms: Some(start_ms + 900),
            snippet: "kita bahas anggaran".to_string(),
            highlight: Some((10, 9)),
            from_source_text: false,
        }
    }

    fn results(total: usize, hits: Vec<SearchHit>) -> SearchResults {
        SearchResults { hits, total }
    }

    #[test]
    fn typing_debounces_into_one_search() {
        let mut state = AppState::new();
        let start = Instant::now();
        state.search.query = "rapat".to_string();
        state.on_search_edited(start);

        assert!(state.tick(start + Duration::from_millis(50)).is_empty());
        let effects = state.tick(start + SEARCH_DEBOUNCE + Duration::from_millis(1));
        match effects.as_slice() {
            [Effect::Search { query }] => assert_eq!(query, "rapat"),
            other => panic!("expected one search effect, got {other:?}"),
        }
        // The same query is not asked again while it is in flight.
        assert!(state
            .tick(start + SEARCH_DEBOUNCE + Duration::from_millis(20))
            .is_empty());
    }

    #[test]
    fn a_blank_or_punctuation_only_query_never_runs() {
        let mut state = AppState::new();
        state.search.query = "   ".to_string();
        state.on_search_edited(Instant::now());
        assert!(state
            .tick(Instant::now() + Duration::from_secs(1))
            .is_empty());
        state.search.query = "-".to_string();
        state.on_search_edited(Instant::now());
        assert!(state
            .tick(Instant::now() + Duration::from_secs(1))
            .is_empty());
    }

    #[test]
    fn a_stale_answer_never_overwrites_a_newer_query() {
        let mut state = AppState::new();
        let t = transcript("rapat.wav");
        state.library.items = vec![t.clone()];
        state.search.query = "lama".to_string();
        state.search.in_flight = Some("lama".to_string());
        state.search.query = "baru".to_string();
        state.on_search_results("lama".to_string(), Ok(results(1, vec![hit(t.id, 0)])));
        assert!(state.search.results.is_empty(), "the old answer is dropped");
        assert_eq!(state.search.applied, None);

        state.search.in_flight = Some("baru".to_string());
        state.on_search_results("baru".to_string(), Ok(results(1, vec![hit(t.id, 1_500)])));
        assert_eq!(state.search.results.len(), 1);
        assert_eq!(state.search.applied.as_deref(), Some("baru"));
    }

    #[test]
    fn a_failed_search_shows_the_error_instead_of_stale_rows() {
        let mut state = AppState::new();
        state.search.query = "rapat".to_string();
        state.search.in_flight = Some("rapat".to_string());
        state.on_search_results("rapat".to_string(), Ok(results(1, vec![])));
        state.search.query = "rusak".to_string();
        state.search.in_flight = Some("rusak".to_string());
        state.on_search_results("rusak".to_string(), Err("database is locked".to_string()));
        assert!(state.search.results.is_empty());
        assert_eq!(state.search.error.as_deref(), Some("database is locked"));
    }

    #[test]
    fn clicking_a_segment_hit_seeks_the_existing_playback_state() {
        let mut state = AppState::new();
        let t = transcript("rapat.wav");
        // A real file so the transport can be ready and remember the position.
        let dir = std::env::temp_dir().join(format!("suramak-search-seek-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let wav = dir.join("seek.wav");
        std::fs::write(&wav, b"not really audio").unwrap();
        let mut t = t;
        t.audio_path = Some(wav.clone());
        state.library.items = vec![t.clone()];
        state.on_transcript_loaded(t.clone());
        assert_eq!(state.route, Route::Editor);

        let effects = state.open_search_hit(&hit(t.id, 1_500));
        assert!(
            effects.is_empty(),
            "an idle transport parks the playhead without a command"
        );
        assert_eq!(state.playback.position_ms(), 1_500);
        assert_eq!(state.search.seek_note.as_deref(), Some("Playhead at 0:01"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hit_for_another_transcript_loads_it_and_then_seeks() {
        let mut state = AppState::new();
        let dir = std::env::temp_dir().join(format!("suramak-search-load-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut a = transcript("a.wav");
        let wav_a = dir.join("a.wav");
        std::fs::write(&wav_a, b"x").unwrap();
        a.audio_path = Some(wav_a);
        let mut b = transcript("b.wav");
        let wav_b = dir.join("b.wav");
        std::fs::write(&wav_b, b"x").unwrap();
        b.audio_path = Some(wav_b);
        state.library.items = vec![a.clone(), b.clone()];
        state.on_transcript_loaded(a);

        let effects = state.open_search_hit(&hit(b.id, 1_500));
        match effects.as_slice() {
            [Effect::LoadTranscript(header)] => assert_eq!(header.id, b.id),
            other => panic!("expected a load effect, got {other:?}"),
        }
        assert_eq!(state.search.pending_seek, Some((b.id, 1_500)));

        // The load lands: the seek is applied once, to the right transcript.
        let after = state.on_transcript_loaded(b.clone());
        assert!(
            after
                .iter()
                .all(|effect| matches!(effect, Effect::LoadPeaks { .. })),
            "only the waveform decode follows a load: {after:?}"
        );
        assert_eq!(state.playback.position_ms(), 1_500);
        assert_eq!(state.search.pending_seek, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_load_for_the_wrong_transcript_never_seeks_it() {
        let mut state = AppState::new();
        let a = transcript("a.wav");
        let b = transcript("b.wav");
        state.library.items = vec![a.clone(), b.clone()];
        state.search.pending_seek = Some((a.id, 4_000));
        state.on_transcript_loaded(b);
        assert_eq!(state.playback.position_ms(), 0);
        assert_eq!(
            state.search.pending_seek, None,
            "a seek for a transcript that never loaded is stale, not a time bomb"
        );
    }

    #[test]
    fn a_missing_media_hit_still_opens_and_flags_the_no_op_seek() {
        let mut state = AppState::new();
        let mut t = transcript("hilang.wav");
        t.audio_path = Some(std::path::PathBuf::from("/nonexistent/suramak/gone.wav"));
        state.library.items = vec![t.clone()];
        state.on_transcript_loaded(t.clone());
        assert!(state.playback.hint().is_some());

        let effects = state.open_search_hit(&hit(t.id, 2_000));
        assert!(effects.is_empty(), "no engine command for missing audio");
        assert_eq!(state.playback.position_ms(), 0);
        let note = state.search.seek_note.as_deref().expect("a flag");
        assert!(note.contains("playhead cannot move"), "{note}");
        assert!(note.contains("0:02"), "{note}");
        // The result row is untouched by the failed seek.
        assert_eq!(state.search.query, "");
    }

    #[test]
    fn a_glossary_hit_opens_the_text_rules_page() {
        let mut state = AppState::new();
        let mut glossary = hit(Uuid::new_v4(), 0);
        glossary.transcript_id = None;
        glossary.kind = HitKind::Glossary;
        glossary.start_ms = None;
        let effects = state.open_search_hit(&glossary);
        assert_eq!(state.route, Route::Settings);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::RefreshAudioDevices)),
            "open_settings' effects run"
        );
        assert!(state.search.pending_seek.is_none());
    }

    #[test]
    fn a_hit_whose_row_left_the_sidebar_refreshes_instead_of_opening_nothing() {
        let mut state = AppState::new();
        let gone = Uuid::new_v4();
        let effects = state.open_search_hit(&hit(gone, 0));
        assert!(matches!(effects.as_slice(), [Effect::RefreshLibrary]));
        assert!(state.search.pending_seek.is_none());
    }

    #[test]
    fn clearing_the_search_drops_every_result_and_seek() {
        let mut state = AppState::new();
        let t = transcript("rapat.wav");
        state.search.query = "rapat".to_string();
        state.search.results = vec![hit(t.id, 0)];
        state.search.pending_seek = Some((t.id, 10));
        state.search.seek_note = Some("Playhead at 0:00".to_string());
        state.search.clear();
        assert!(!state.search.is_active());
        assert!(state.search.results.is_empty());
        assert!(state.search.pending_seek.is_none());
        assert!(state.search.seek_note.is_none());
    }

    #[test]
    fn an_answer_for_an_abandoned_query_does_not_block_retyping_it() {
        let mut state = AppState::new();
        let now = Instant::now();
        state.search.query = "rapat".to_string();
        state.search.in_flight = Some("rapat".to_string());
        // The user replaces the query with punctuation the moment the request
        // is out: it settles with no request of its own.
        state.search.query = "-".to_string();
        assert!(state.tick(now).is_empty());
        assert_eq!(state.search.applied.as_deref(), Some("-"));

        // The abandoned answer lands: dropped, and its marker released.
        state.on_search_results("rapat".to_string(), Ok(results(3, vec![])));
        assert!(state.search.in_flight.is_none());
        assert!(state.search.results.is_empty());
        assert_eq!(state.search.total, 0);

        // Retyping the same word searches again instead of hanging on that
        // stale marker.
        state.search.query = "rapat".to_string();
        state.search.last_edit = Some(now - Duration::from_secs(1));
        let effects = state.tick(now);
        assert!(
            matches!(effects.as_slice(), [Effect::Search { .. }]),
            "the retype must issue a new query: {effects:?}"
        );
    }

    #[test]
    fn an_edit_under_an_open_search_reruns_the_query() {
        let mut state = AppState::new();
        let t = transcript("rapat.wav");
        state.search.query = "rapat".to_string();
        state.search.applied = Some("rapat".to_string());
        state.search.results = vec![hit(t.id, 0)];
        let now = Instant::now();
        state.search.last_edit = Some(now - Duration::from_secs(2));

        // No editor open: the save still invalidates the settled results.
        let effects = state.on_edits_saved(now, Ok(()));
        assert!(effects.is_empty());
        let effects = state.tick(now);
        assert!(
            matches!(effects.as_slice(), [Effect::Search { query }] if query == "rapat"),
            "the stale snippet list must be re-queried: {effects:?}"
        );
    }

    #[test]
    fn timestamps_are_compact_and_never_negative() {
        assert_eq!(fmt_timestamp(0), "0:00");
        assert_eq!(fmt_timestamp(1_500), "0:01");
        assert_eq!(fmt_timestamp(61_000), "1:01");
        assert_eq!(fmt_timestamp(3_600_000), "1:00:00");
        assert_eq!(fmt_timestamp(-500), "0:00");
    }
}
