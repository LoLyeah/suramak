//! In-app polish flow (M4) — the configuration gate, the request lifecycle and
//! the review / accept / undo / discard state machine. **Egui-free**: every rule
//! here is unit-tested headless, including a full round trip against a
//! hand-rolled OpenAI-compatible mock endpoint (the pattern of
//! `suramak-core/tests/polish.rs` — no new dependencies).
//!
//! Wiring: the editor's Polish action calls [`AppState::start_polish`], which
//! snapshots the editor and emits [`Effect::PolishTranscript`](super::Effect::PolishTranscript).
//! `crate::runtime` resolves the configuration, runs core's blocking client on a
//! background thread (never the ASR worker) and reports the outcome back through
//! [`AppState::on_polish_done`]. The view lives in `app::editor`.
//!
//! M11: [`polish_audio_request`] resolves the audio-aware toggle against the
//! open transcript (off → text-only; on with a recording → the recording; on
//! without → text-only + a note). Core's graceful ladder's notes ride the
//! outcome and are shown with the review, like every polish message.
//!
//! Failure rule (documented + tested): when the reply does not validate — it
//! drops or merges segments, swaps ids, sends empty text, is not JSON, or the
//! endpoint errors — the **whole run fails**, the transcript is left exactly as
//! it was and the raw backend/reply error is shown. There is no partial apply.

use suramak_core::polish::{AudioRequest, PolishOutcome};
use suramak_core::settings::Settings;
use suramak_core::transcript::Transcript;

use super::{AppState, Effect};

/// Model used when `settings.polish.model` is empty (same default as the CLI).
pub const DEFAULT_POLISH_MODEL: &str = "gpt-4o-mini";

/// Shown next to the disabled Polish button when polish is not configured.
pub const POLISH_SETUP_NOTE: &str = "polish is not configured — set the base URL in Settings \
and provide an API key (OS keychain on macOS/Windows; SURAMAK_OPENAI_API_KEY on Linux)";

/// The resolved polish configuration (base URL + model).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolishConfig {
    pub base_url: String,
    pub model: String,
}

/// Resolves the polish configuration, or the human-readable reason it is
/// unavailable.
///
/// Gate (WORKORDER-M4 §1): a base URL **and** an API key must be present. The
/// key presence is injected (`key_present`) so the rule is testable without
/// touching the environment; the runtime passes
/// `suramak_core::keychain::get_api_key().is_ok()` (keychain on macOS/Windows,
/// `SURAMAK_OPENAI_API_KEY` on Linux, per M3). An empty model falls back to
/// [`DEFAULT_POLISH_MODEL`], exactly like the CLI.
pub fn polish_config(settings: &Settings, key_present: bool) -> Result<PolishConfig, String> {
    let Some(polish) = &settings.polish else {
        return Err(POLISH_SETUP_NOTE.to_string());
    };
    let base_url = polish.base_url.trim();
    if base_url.is_empty() {
        return Err(POLISH_SETUP_NOTE.to_string());
    }
    if !key_present {
        return Err(
            "polish has no API key — save it in Settings (OS keychain) on macOS/Windows, \
or set SURAMAK_OPENAI_API_KEY on Linux"
                .to_string(),
        );
    }
    let model = polish.model.trim();
    Ok(PolishConfig {
        base_url: base_url.to_string(),
        model: if model.is_empty() {
            DEFAULT_POLISH_MODEL.to_string()
        } else {
            model.to_string()
        },
    })
}

/// M11: the note when audio-aware polish is on but the transcript has no audio
/// path at all (e.g. a transcript made by another tool).
pub const AUDIO_NOT_ATTACHED_NOTE: &str =
    "the recording was not sent — no audio is attached to this transcript; the text was polished on its own";

/// M11: the note when audio-aware polish is on but the recording's file is
/// gone (keep-audio off, or a moved/deleted import).
pub const AUDIO_FILE_GONE_NOTE: &str =
    "the recording was not sent — keep audio was off, or the file is gone; the text was polished on its own";

/// M11: what a polish run should do about the recording — the toggle
/// (`settings.polish.send_audio`) resolved against the open transcript.
///
/// `Off` keeps the request byte-identical to the pre-M11 one. The toggle on
/// with a recording that exists sends it; a missing file degrades to
/// text-only + a note, never an error.
pub fn polish_audio_request(settings: &Settings, transcript: &Transcript) -> AudioRequest {
    let send_audio = settings
        .polish
        .as_ref()
        .is_some_and(|polish| polish.send_audio);
    if !send_audio {
        return AudioRequest::Off;
    }
    match transcript.audio_path.as_deref() {
        Some(path) if path.is_file() => AudioRequest::Recording(path.to_path_buf()),
        Some(_) => AudioRequest::Unavailable {
            note: AUDIO_FILE_GONE_NOTE.to_string(),
        },
        None => AudioRequest::Unavailable {
            note: AUDIO_NOT_ATTACHED_NOTE.to_string(),
        },
    }
}

/// Lifecycle of the in-app polish run.
#[derive(Debug, Clone, PartialEq)]
pub enum PolishState {
    /// Nothing in flight, nothing to review.
    Idle,
    /// The HTTP call is running on its background thread.
    Running(PolishRun),
    /// A validated reply is waiting for Accept / Discard.
    Review(PolishReview),
    /// The run failed; the transcript is unchanged and `message` is the raw
    /// backend/reply error (the failed state can be retried or dismissed).
    Failed { message: String },
}

impl PolishState {
    pub fn is_running(&self) -> bool {
        matches!(self, PolishState::Running(_))
    }

    pub fn is_reviewing(&self) -> bool {
        matches!(self, PolishState::Review(_))
    }
}

/// A polish request in flight: the editor texts that were sent (kept for the
/// side-by-side review and for the one-step undo).
#[derive(Debug, Clone, PartialEq)]
pub struct PolishRun {
    /// `(segment idx, text)` in editor order — exactly what the endpoint received.
    pub originals: Vec<(u32, String)>,
}

/// One review row: what was sent vs. what the endpoint returned.
#[derive(Debug, Clone, PartialEq)]
pub struct PolishRow {
    pub idx: u32,
    pub original: String,
    pub polished: String,
}

/// A validated reply waiting for Accept / Discard (batch only — no streaming).
#[derive(Debug, Clone, PartialEq)]
pub struct PolishReview {
    pub rows: Vec<PolishRow>,
    /// M11: graceful-degradation notes from the run (empty on a plain
    /// success) — shown with the review like every polish message.
    pub notes: Vec<String>,
}

impl PolishReview {
    /// The notes as one caption line (`None` when there are none).
    pub fn notes_line(&self) -> Option<String> {
        if self.notes.is_empty() {
            None
        } else {
            Some(self.notes.join(" · "))
        }
    }
}

/// One-step undo of an accepted polish: the buffer texts from before Accept.
#[derive(Debug, Clone, PartialEq)]
pub struct PolishUndo {
    /// `(segment idx, text before Accept)`.
    pub previous: Vec<(u32, String)>,
    /// Number of segments the accept changed.
    pub segments: usize,
}

impl AppState {
    /// Whether the Polish action is available: configured, not already
    /// polishing/reviewing, and an editor with at least one segment is open.
    pub fn can_polish(&self) -> bool {
        self.polish_configured
            && !self.polish.is_running()
            && !self.polish.is_reviewing()
            && self
                .editor
                .as_ref()
                .is_some_and(|editor| !editor.transcript.segments.is_empty())
    }

    /// Polish action pressed: snapshot the editor (unsaved edits included) and
    /// ask the runtime to run the call on its background thread.
    pub fn start_polish(&mut self) -> Vec<Effect> {
        if !self.can_polish() {
            return Vec::new();
        }
        let Some(editor) = self.editor.as_ref() else {
            return Vec::new();
        };
        let originals = editor
            .transcript
            .segments
            .iter()
            .zip(editor.buffers.iter())
            .map(|(segment, buffer)| (segment.idx, buffer.clone()))
            .collect();
        let snapshot = editor.snapshot();
        self.polish = PolishState::Running(PolishRun { originals });
        vec![Effect::PolishTranscript {
            transcript: Box::new(snapshot),
        }]
    }

    /// The runtime finished the call: a validated reply opens the review, any
    /// failure keeps the transcript untouched and shows the raw error. M11:
    /// the outcome's degradation notes ride into the review.
    pub fn on_polish_done(&mut self, result: Result<PolishOutcome, String>) -> Vec<Effect> {
        if !self.polish.is_running() {
            // A stray result (no run in flight) — nothing to attach it to.
            return Vec::new();
        }
        let PolishState::Running(run) = std::mem::replace(&mut self.polish, PolishState::Idle)
        else {
            return Vec::new();
        };
        match result {
            Ok(outcome) => {
                let PolishOutcome {
                    segments: polished,
                    notes,
                } = outcome;
                // Core's `parse_and_validate` already rejects count/id/empty-text
                // mismatches; re-check the alignment here so a stray caller can
                // never pair polished text with the wrong segment.
                let aligned = polished.len() == run.originals.len()
                    && polished
                        .iter()
                        .zip(run.originals.iter())
                        .all(|(segment, (idx, _))| segment.idx == *idx);
                if !aligned {
                    self.polish = PolishState::Failed {
                        message: format!(
                            "polish reply does not line up with the {} sent: got {}",
                            super::count_noun(run.originals.len(), "segment"),
                            polished.len()
                        ),
                    };
                    return Vec::new();
                }
                let rows = run
                    .originals
                    .iter()
                    .zip(polished.iter())
                    .map(|((idx, original), segment)| PolishRow {
                        idx: *idx,
                        original: original.clone(),
                        polished: segment.text.clone(),
                    })
                    .collect();
                self.polish = PolishState::Review(PolishReview { rows, notes });
            }
            Err(message) => {
                self.polish = PolishState::Failed { message };
            }
        }
        Vec::new()
    }

    /// Accept (batch): write the polished text into the editor buffers, persist
    /// via the store and keep the previous text for the one-step undo.
    pub fn accept_polish(&mut self) -> Vec<Effect> {
        if !self.polish.is_reviewing() {
            return Vec::new();
        }
        let PolishState::Review(review) = std::mem::replace(&mut self.polish, PolishState::Idle)
        else {
            return Vec::new();
        };
        let Some(editor) = self.editor.as_mut() else {
            // No editor (cannot happen while reviewing) — put the review back.
            self.polish = PolishState::Review(review);
            return Vec::new();
        };
        let mut previous = Vec::with_capacity(review.rows.len());
        for row in &review.rows {
            let Some(buffer) = editor.buffers.get_mut(row.idx as usize) else {
                continue;
            };
            previous.push((row.idx, buffer.clone()));
            *buffer = row.polished.clone();
        }
        let updates = editor.pending_updates();
        if updates.is_empty() {
            editor.dirty = false;
            self.set_status("polish accepted — the text was already up to date", false);
            return Vec::new();
        }
        editor.dirty = true;
        editor.last_edit = None; // the save below is immediate, not debounced
        editor.save_error = None;
        let segments = updates.len();
        self.polish_undo = Some(PolishUndo { previous, segments });
        self.set_status(
            format!(
                "polish accepted — {} updated (Undo available)",
                super::count_noun(segments, "segment")
            ),
            false,
        );
        vec![Effect::SaveEdits { updates }]
    }

    /// Undo the accepted polish (one step): restore the previous buffer texts
    /// and persist them through the store again.
    pub fn undo_polish(&mut self) -> Vec<Effect> {
        let Some(undo) = self.polish_undo.take() else {
            return Vec::new();
        };
        let Some(editor) = self.editor.as_mut() else {
            self.polish_undo = Some(undo);
            return Vec::new();
        };
        for (idx, text) in &undo.previous {
            if let Some(buffer) = editor.buffers.get_mut(*idx as usize) {
                *buffer = text.clone();
            }
        }
        let updates = editor.pending_updates();
        editor.dirty = !updates.is_empty();
        editor.last_edit = None;
        editor.save_error = None;
        self.set_status("polish undone — the previous text is back", false);
        if updates.is_empty() {
            return Vec::new();
        }
        vec![Effect::SaveEdits { updates }]
    }

    /// Discard the review (or dismiss a failure): the transcript is unchanged.
    pub fn discard_polish(&mut self) {
        match &self.polish {
            PolishState::Review(_) | PolishState::Failed { .. } => {
                self.polish = PolishState::Idle;
                self.set_status("polish discarded — the transcript is unchanged", false);
            }
            // A run in flight cannot be discarded (batch only, no cancel).
            PolishState::Idle | PolishState::Running(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use base64::Engine as _;

    use suramak_core::paths;
    use suramak_core::settings::{PolishSettings, Settings};
    use suramak_core::store::Store;
    use suramak_core::transcript::{Segment, Source, Transcript};

    use super::*;
    use crate::app::Route;
    use crate::runtime::Runtime;

    // ---- helpers -------------------------------------------------------------

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
            vec![segment(0, "helo wrold"), segment(1, "this iz a tset")],
        )
    }

    fn state_with_editor(configured: bool) -> AppState {
        let mut state = AppState::new();
        state.polish_configured = configured;
        state.on_transcript_loaded(transcript("polish.wav"));
        state
    }

    fn settings_with_polish(base_url: &str, model: &str) -> Settings {
        Settings {
            polish: Some(PolishSettings {
                base_url: base_url.to_string(),
                model: model.to_string(),
                send_audio: false,
            }),
            ..Settings::default()
        }
    }

    // ---- configuration gate (no network) -------------------------------------

    #[test]
    fn polish_config_requires_a_base_url_and_a_key() {
        let err = polish_config(&Settings::default(), true).unwrap_err();
        assert!(err.contains("not configured"), "unexpected: {err}");

        let blank = Settings {
            polish: Some(PolishSettings {
                base_url: "   ".to_string(),
                model: "m".to_string(),
                send_audio: false,
            }),
            ..Settings::default()
        };
        assert!(polish_config(&blank, true).is_err(), "blank base URL");

        let settings = settings_with_polish("http://localhost:9/v1", "m");
        let err = polish_config(&settings, false).unwrap_err();
        assert!(err.contains("API key"), "unexpected: {err}");
    }

    #[test]
    fn polish_config_defaults_the_model_and_trims_the_url() {
        let settings = settings_with_polish("  http://localhost:9/v1  ", "");
        let config = polish_config(&settings, true).expect("configured");
        assert_eq!(config.base_url, "http://localhost:9/v1");
        assert_eq!(config.model, DEFAULT_POLISH_MODEL);

        let settings = settings_with_polish("http://localhost:9/v1", "llama3");
        assert_eq!(polish_config(&settings, true).unwrap().model, "llama3");
    }

    #[test]
    fn start_polish_is_gated_on_configuration() {
        let mut state = state_with_editor(false);
        assert!(!state.can_polish());
        assert!(state.start_polish().is_empty());
        assert_eq!(state.polish, PolishState::Idle);

        // Configured but no editor: still gated.
        let mut state = AppState::new();
        state.polish_configured = true;
        assert!(!state.can_polish());
        assert!(state.start_polish().is_empty());

        // Configured with an empty transcript: gated too.
        let mut state = AppState::new();
        state.polish_configured = true;
        let mut empty = transcript("empty.wav");
        empty.segments.clear();
        state.on_transcript_loaded(empty);
        assert!(!state.can_polish());
    }

    #[test]
    fn start_polish_snapshots_the_editor_including_unsaved_edits() {
        let mut state = state_with_editor(true);
        let t0 = Instant::now();
        state
            .editor
            .as_mut()
            .unwrap()
            .edit(1, "this is a tset".to_string(), t0);

        let effects = state.start_polish();
        match effects.as_slice() {
            [Effect::PolishTranscript { transcript }] => {
                assert_eq!(transcript.segments[0].text, "helo wrold");
                assert_eq!(
                    transcript.segments[1].text, "this is a tset",
                    "unsaved edits are what gets polished"
                );
            }
            other => panic!("expected a polish effect, got {other:?}"),
        }
        match &state.polish {
            PolishState::Running(run) => {
                assert_eq!(
                    run.originals,
                    vec![
                        (0, "helo wrold".to_string()),
                        (1, "this is a tset".to_string())
                    ]
                );
            }
            other => panic!("expected a running polish, got {other:?}"),
        }
        assert!(state.busy(), "a polish run keeps the tick alive");
        assert!(state.start_polish().is_empty(), "no double start");
    }

    #[test]
    fn a_failed_reply_keeps_the_original_segments_and_shows_the_error() {
        let mut state = state_with_editor(true);
        state.start_polish();
        // The merged/dropped-segment rule: the reply failed validation upstream,
        // so the run fails as a whole and nothing is applied.
        let message = "Bad polish response: segment count mismatch: expected 2, got 1";
        assert!(state.on_polish_done(Err(message.to_string())).is_empty());
        match &state.polish {
            PolishState::Failed { message } => {
                assert!(message.contains("count mismatch"), "unexpected: {message}")
            }
            other => panic!("expected a failure, got {other:?}"),
        }
        let editor = state.editor.as_ref().unwrap();
        assert_eq!(editor.buffers[0], "helo wrold");
        assert_eq!(editor.buffers[1], "this iz a tset");
        assert!(!editor.dirty, "a failed polish must not dirty the editor");
        assert!(!state.busy(), "a failed polish releases the tick");

        // The failure can be dismissed or retried.
        state.discard_polish();
        assert_eq!(state.polish, PolishState::Idle);
        assert!(state.can_polish());
    }

    #[test]
    fn a_misaligned_reply_is_rejected_even_if_core_ever_lets_it_through() {
        let mut state = state_with_editor(true);
        state.start_polish();
        let swapped = vec![segment(1, "Hello world."), segment(0, "This is a test.")];
        state.on_polish_done(Ok(PolishOutcome::plain(swapped)));
        match &state.polish {
            PolishState::Failed { message } => {
                assert!(message.contains("line up"), "unexpected: {message}")
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn stray_results_and_accept_without_a_review_are_noops() {
        let mut state = state_with_editor(true);
        assert!(state
            .on_polish_done(Ok(PolishOutcome::plain(vec![])))
            .is_empty());
        assert_eq!(state.polish, PolishState::Idle);
        assert!(state.accept_polish().is_empty());
        assert!(state.undo_polish().is_empty());
        state.discard_polish();
        assert_eq!(state.polish, PolishState::Idle);
    }

    #[test]
    fn accept_applies_the_review_persists_and_offers_undo() {
        let mut state = state_with_editor(true);
        state.start_polish();
        let polished = vec![segment(0, "Hello world."), segment(1, "This is a test.")];
        assert!(state
            .on_polish_done(Ok(PolishOutcome::plain(polished)))
            .is_empty());
        match &state.polish {
            PolishState::Review(review) => {
                assert_eq!(review.rows.len(), 2);
                assert_eq!(review.rows[0].original, "helo wrold");
                assert_eq!(review.rows[0].polished, "Hello world.");
                assert_eq!(review.rows[1].idx, 1);
            }
            other => panic!("expected a review, got {other:?}"),
        }
        assert!(!state.busy(), "a review is not a running job");

        let effects = state.accept_polish();
        match effects.as_slice() {
            [Effect::SaveEdits { updates }] => {
                assert_eq!(
                    updates,
                    &[
                        (0, "Hello world.".to_string()),
                        (1, "This is a test.".to_string())
                    ],
                    "accept persists immediately"
                );
            }
            other => panic!("expected one save, got {other:?}"),
        }
        assert_eq!(state.polish, PolishState::Idle);
        let undo = state.polish_undo.as_ref().expect("one-step undo armed");
        assert_eq!(undo.segments, 2);
        assert_eq!(undo.previous[0], (0, "helo wrold".to_string()));
        assert!(state.status.as_ref().unwrap().text.contains("accepted"));

        // The store confirms the save; the transcript now carries the new text.
        assert!(state.on_edits_saved(Instant::now(), Ok(())).is_empty());
        let editor = state.editor.as_ref().unwrap();
        assert_eq!(editor.transcript.segments[0].text, "Hello world.");
        assert!(!editor.dirty);
        assert_eq!(editor.save_state(), "saved");
    }

    #[test]
    fn undo_restores_the_previous_text_and_persists_it_once() {
        let mut state = state_with_editor(true);
        state.start_polish();
        state.on_polish_done(Ok(PolishOutcome::plain(vec![
            segment(0, "Hello world."),
            segment(1, "This is a test."),
        ])));
        state.accept_polish();
        state.on_edits_saved(Instant::now(), Ok(()));

        let effects = state.undo_polish();
        match effects.as_slice() {
            [Effect::SaveEdits { updates }] => {
                assert_eq!(
                    updates,
                    &[
                        (0, "helo wrold".to_string()),
                        (1, "this iz a tset".to_string())
                    ]
                );
            }
            other => panic!("expected one save, got {other:?}"),
        }
        assert!(state.polish_undo.is_none(), "the undo is one step");
        assert!(state.status.as_ref().unwrap().text.contains("undone"));
        assert!(state.undo_polish().is_empty(), "no second undo");

        assert!(state.on_edits_saved(Instant::now(), Ok(())).is_empty());
        assert_eq!(
            state.editor.as_ref().unwrap().transcript.segments[1].text,
            "this iz a tset"
        );
    }

    #[test]
    fn accept_with_no_text_change_skips_the_save_and_the_undo() {
        let mut state = state_with_editor(true);
        state.start_polish();
        state.on_polish_done(Ok(PolishOutcome::plain(vec![
            segment(0, "helo wrold"),
            segment(1, "this iz a tset"),
        ])));
        assert!(state.accept_polish().is_empty(), "nothing to persist");
        assert!(state.polish_undo.is_none());
        assert!(!state.editor.as_ref().unwrap().dirty);
        assert!(state
            .status
            .as_ref()
            .unwrap()
            .text
            .contains("already up to date"));
    }

    #[test]
    fn discard_keeps_the_transcript_untouched() {
        let mut state = state_with_editor(true);
        state.start_polish();
        state.on_polish_done(Ok(PolishOutcome::plain(vec![
            segment(0, "Hello world."),
            segment(1, "This is a test."),
        ])));
        state.discard_polish();
        assert_eq!(state.polish, PolishState::Idle);
        let editor = state.editor.as_ref().unwrap();
        assert_eq!(editor.buffers[0], "helo wrold");
        assert!(!editor.dirty);
        assert!(state.polish_undo.is_none());
        // Discard while running is a no-op (batch only, no cancel).
        state.start_polish();
        state.discard_polish();
        assert!(state.polish.is_running());
    }

    #[test]
    fn opening_another_transcript_drops_a_pending_review() {
        let mut state = state_with_editor(true);
        state.start_polish();
        state.on_polish_done(Ok(PolishOutcome::plain(vec![
            segment(0, "Hello world."),
            segment(1, "This is a test."),
        ])));
        assert!(state.polish.is_reviewing());

        state.on_transcript_loaded(transcript("other.wav"));
        assert_eq!(state.polish, PolishState::Idle);
        assert!(state.polish_undo.is_none());
        assert_eq!(state.route, Route::Editor);
    }

    // ---- full round trip against a mock OpenAI-compatible endpoint -----------
    //
    // These tests drive the real runtime (background thread + core's blocking
    // client + the sqlite store) against a hand-rolled `TcpListener` mock, the
    // same pattern as `suramak-core/tests/polish.rs`. The environment is
    // process-global, so every test that sets it holds this lock.

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn read_request(stream: &mut TcpStream) -> String {
        // Reads the full request — headers + the Content-Length body (M11's
        // audio requests carry megabytes of base64).
        let mut data: Vec<u8> = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        let mut expected: Option<(usize, usize)> = None;
        loop {
            if expected.is_none() {
                if let Some(pos) = find_bytes(&data, b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..pos]).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    expected = Some((pos + 4, length));
                }
            }
            if let Some((body_at, length)) = expected {
                if data.len() >= body_at + length {
                    break;
                }
            }
            let n = stream.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&data).to_string()
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    fn respond(stream: &mut TcpStream, status: &str, body: &str) {
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(resp.as_bytes()).ok();
    }

    /// Spawns a mock endpoint that serves up to 4 connections with `handler`.
    fn spawn_mock<F>(handler: F) -> String
    where
        F: Fn(&mut TcpStream, &str) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for _ in 0..4 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let request = read_request(&mut stream);
                handler(&mut stream, &request);
            }
        });
        format!("http://{addr}/v1")
    }

    /// Escapes `s` as a JSON string literal (tiny, so the mock needs no deps).
    fn json_escape(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// OpenAI-style success body carrying `content` as the assistant message.
    fn chat_body(content: &str) -> String {
        format!(
            "{{\"choices\":[{{\"message\":{{\"role\":\"assistant\",\"content\":{}}}}}]}}",
            json_escape(content)
        )
    }

    /// Spawns a mock endpoint that serves `expected` connections with a
    /// scripted handler (request index + raw request → (status, body)) and
    /// records every raw request it saw (M11: the audio tests need the index
    /// and the full body).
    fn spawn_scripted_mock<F>(expected: usize, handler: F) -> (String, Arc<Mutex<Vec<String>>>)
    where
        F: Fn(usize, &str) -> (String, String) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let addr = listener.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        thread::spawn(move || {
            for index in 0..expected {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let request = read_request(&mut stream);
                let (status, body) = handler(index, &request);
                record.lock().unwrap().push(request);
                respond(&mut stream, &status, &body);
            }
        });
        (format!("http://{addr}/v1"), seen)
    }

    /// The raw request body (after the header block) as JSON.
    fn request_body(request: &str) -> serde_json::Value {
        let at = find_bytes(request.as_bytes(), b"\r\n\r\n").expect("headers") + 4;
        serde_json::from_str(request[at..].trim_end()).expect("JSON body")
    }

    /// The ids of the request payload — works for the text-only string content
    /// and the M11 content-part array alike.
    fn payload_ids(body: &serde_json::Value) -> Vec<u32> {
        let content = &body["messages"][1]["content"];
        let text = match content {
            serde_json::Value::String(text) => text.clone(),
            serde_json::Value::Array(parts) => parts
                .iter()
                .find_map(|part| {
                    if part["type"] == "text" {
                        part["text"].as_str()
                    } else {
                        None
                    }
                })
                .expect("a text part")
                .to_string(),
            other => panic!("unexpected user content: {other}"),
        };
        let payload: serde_json::Value = serde_json::from_str(&text).expect("payload JSON");
        payload["segments"]
            .as_array()
            .expect("segments array")
            .iter()
            .map(|segment| segment["id"].as_u64().expect("id") as u32)
            .collect()
    }

    /// The `input_audio` part of the user message, if any: (base64, format).
    fn audio_part(content: &serde_json::Value) -> Option<(String, String)> {
        let parts = content.as_array()?;
        let part = parts.iter().find(|part| part["type"] == "input_audio")?;
        Some((
            part["input_audio"]["data"].as_str()?.to_string(),
            part["input_audio"]["format"].as_str()?.to_string(),
        ))
    }

    /// A valid reply for the given segment ids.
    fn reply_for(ids: &[u32]) -> String {
        let segments: Vec<serde_json::Value> = ids
            .iter()
            .map(|id| serde_json::json!({ "id": id, "text": format!("Polished {id}.") }))
            .collect();
        chat_body(&serde_json::json!({ "segments": segments }).to_string())
    }

    /// A 16 kHz mono PCM16 WAV of `frames` silent samples — a recording, as
    /// the app writes them (`decode::write_wav_i16`).
    fn wav16k(tag: &str, frames: usize) -> (PathBuf, Vec<u8>) {
        let path = std::env::temp_dir().join(format!(
            "suramak-ui-polish-{tag}-{}-{frames}.wav",
            std::process::id()
        ));
        suramak_core::decode::write_wav_i16(&path, &vec![0.0f32; frames], 16_000, 1)
            .expect("write wav");
        let bytes = std::fs::read(&path).expect("read wav");
        (path, bytes)
    }

    /// Runs `f` with a fresh `XDG_DATA_HOME` and a test API key, then restores
    /// the environment. Serialized on `ENV_LOCK` (all mock tests take it).
    fn with_polish_env<T>(tag: &str, f: impl FnOnce(&Path) -> T) -> T {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root =
            std::env::temp_dir().join(format!("suramak-ui-polish-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("temp root");
        let previous_xdg = std::env::var_os("XDG_DATA_HOME");
        let previous_key = std::env::var_os("SURAMAK_OPENAI_API_KEY");
        std::env::set_var("XDG_DATA_HOME", &root);
        std::env::set_var("SURAMAK_OPENAI_API_KEY", "test-key");
        let out = f(&root);
        match previous_xdg {
            Some(value) => std::env::set_var("XDG_DATA_HOME", value),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        match previous_key {
            Some(value) => std::env::set_var("SURAMAK_OPENAI_API_KEY", value),
            None => std::env::remove_var("SURAMAK_OPENAI_API_KEY"),
        }
        std::fs::remove_dir_all(&root).ok();
        out
    }

    /// Writes `settings.toml` (polish pointed at `base_url`) under the temp
    /// data dir and returns the loaded settings path.
    fn write_settings(base_url: &str) {
        write_settings_send_audio(base_url, false);
    }

    /// The M11 variant: the audio-aware toggle persisted as given.
    fn write_settings_send_audio(base_url: &str, send_audio: bool) {
        let path = paths::settings_path().expect("settings path");
        std::fs::create_dir_all(path.parent().unwrap()).expect("data dir");
        let mut settings = settings_with_polish(base_url, "mock-model");
        settings.polish.as_mut().unwrap().send_audio = send_audio;
        settings.save(&path).expect("save settings");
    }

    /// Polls the runtime until the polish run reports its terminal outcome.
    fn wait_for_polish(runtime: &mut Runtime) -> Result<PolishOutcome, String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = runtime.polish_status() {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "the polish run did not finish in time"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn polish_round_trip_against_a_mock_endpoint() {
        let addr = spawn_mock(|stream, request| {
            assert!(
                request.contains("POST /v1/chat/completions"),
                "unexpected request: {request}"
            );
            respond(
                stream,
                "200 OK",
                &chat_body(
                    r#"{"segments":[{"id":0,"text":"Hello world."},{"id":1,"text":"This is a test."}]}"#,
                ),
            );
        });
        with_polish_env("round-trip", |_root| {
            write_settings(&addr);
            let mut runtime = Runtime::new(egui::Context::default());
            assert!(
                runtime.polish_configured(),
                "base URL + key must configure polish"
            );

            let mut state = AppState::new();
            state.polish_configured = runtime.polish_configured();
            let transcript = transcript("polish.wav");
            let id = transcript.id;
            Store::open(&paths::db_path().unwrap())
                .unwrap()
                .insert_transcript(&transcript)
                .unwrap();
            state.on_transcript_loaded(transcript);

            let effects = state.start_polish();
            assert!(matches!(
                effects.as_slice(),
                [Effect::PolishTranscript { .. }]
            ));
            runtime.apply(effects, &mut state);

            let result = wait_for_polish(&mut runtime);
            assert!(result.is_ok(), "expected success, got {result:?}");
            assert!(state.on_polish_done(result).is_empty());
            match &state.polish {
                PolishState::Review(review) => {
                    assert_eq!(review.rows.len(), 2);
                    assert_eq!(review.rows[0].original, "helo wrold");
                    assert_eq!(review.rows[0].polished, "Hello world.");
                    assert_eq!(review.rows[1].polished, "This is a test.");
                }
                other => panic!("expected a review, got {other:?}"),
            }

            // Accept → persist → the store (read back through a fresh connection).
            let effects = state.accept_polish();
            runtime.apply(effects, &mut state);
            let stored = Store::open(&paths::db_path().unwrap())
                .unwrap()
                .get_transcript(id)
                .unwrap();
            assert_eq!(stored.segments[0].text, "Hello world.");
            assert_eq!(stored.segments[1].text, "This is a test.");
            assert!(!state.editor.as_ref().unwrap().dirty);

            // One-step undo → the store carries the previous text again.
            let effects = state.undo_polish();
            runtime.apply(effects, &mut state);
            let stored = Store::open(&paths::db_path().unwrap())
                .unwrap()
                .get_transcript(id)
                .unwrap();
            assert_eq!(stored.segments[0].text, "helo wrold");
            assert_eq!(stored.segments[1].text, "this iz a tset");
        });
    }

    #[test]
    fn a_merged_reply_against_the_mock_fails_and_keeps_the_originals() {
        // The endpoint answers with 1 segment for a 2-segment request (a merge):
        // the defined rule is to fail the whole run and show the raw error.
        let addr = spawn_mock(|stream, _request| {
            respond(
                stream,
                "200 OK",
                &chat_body(r#"{"segments":[{"id":0,"text":"Hello world. This is a test."}]}"#),
            );
        });
        with_polish_env("merged", |_root| {
            write_settings(&addr);
            let mut runtime = Runtime::new(egui::Context::default());

            let mut state = AppState::new();
            state.polish_configured = true;
            let transcript = transcript("merged.wav");
            let id = transcript.id;
            Store::open(&paths::db_path().unwrap())
                .unwrap()
                .insert_transcript(&transcript)
                .unwrap();
            state.on_transcript_loaded(transcript);

            runtime.apply(state.start_polish(), &mut state);
            let result = wait_for_polish(&mut runtime);
            let message = result.expect_err("a merged reply must fail");
            assert!(
                message.contains("count mismatch"),
                "the raw reply error must surface: {message}"
            );

            state.on_polish_done(Err(message));
            match &state.polish {
                PolishState::Failed { message } => {
                    assert!(message.contains("count mismatch"), "unexpected: {message}")
                }
                other => panic!("expected a failure, got {other:?}"),
            }
            let editor = state.editor.as_ref().unwrap();
            assert_eq!(editor.buffers[0], "helo wrold", "originals kept");
            assert!(!editor.dirty);
            // The store is untouched too.
            let stored = Store::open(&paths::db_path().unwrap())
                .unwrap()
                .get_transcript(id)
                .unwrap();
            assert_eq!(stored.segments[0].text, "helo wrold");
        });
    }

    #[test]
    fn an_endpoint_error_against_the_mock_fails_with_the_raw_status() {
        let addr = spawn_mock(|stream, _request| {
            respond(
                stream,
                "500 Internal Server Error",
                r#"{"error":"server fault"}"#,
            );
        });
        with_polish_env("http-500", |_root| {
            write_settings(&addr);
            let mut runtime = Runtime::new(egui::Context::default());

            let mut state = AppState::new();
            state.polish_configured = true;
            state.on_transcript_loaded(transcript("http500.wav"));
            runtime.apply(state.start_polish(), &mut state);

            let result = wait_for_polish(&mut runtime);
            let message = result.expect_err("HTTP 500 must fail");
            assert!(message.contains("HTTP 500"), "unexpected: {message}");
            state.on_polish_done(Err(message));
            match &state.polish {
                PolishState::Failed { message } => {
                    assert!(message.contains("HTTP 500"), "unexpected: {message}")
                }
                other => panic!("expected a failure, got {other:?}"),
            }
            assert_eq!(state.editor.as_ref().unwrap().buffers[0], "helo wrold");
        });
    }

    #[test]
    fn a_missing_api_key_fails_the_run_before_any_request() {
        let addr = spawn_mock(|stream, _request| {
            respond(stream, "200 OK", &chat_body(r#"{"segments":[]}"#));
        });
        with_polish_env("no-key", |_root| {
            write_settings(&addr);
            // The key comes from the environment on Linux; drop it again.
            std::env::remove_var("SURAMAK_OPENAI_API_KEY");
            let mut runtime = Runtime::new(egui::Context::default());
            assert!(!runtime.polish_configured());

            let mut state = AppState::new();
            state.polish_configured = true; // stale UI flag: the runtime re-checks
            state.on_transcript_loaded(transcript("nokey.wav"));
            runtime.apply(state.start_polish(), &mut state);

            // The effect fails synchronously — no background thread, no request.
            match &state.polish {
                PolishState::Failed { message } => {
                    assert!(message.contains("API key"), "unexpected: {message}")
                }
                other => panic!("expected a failure, got {other:?}"),
            }
        });
    }

    // ---- M11: audio-aware polish --------------------------------------------

    #[test]
    fn polish_audio_request_resolves_the_toggle_against_the_transcript() {
        let dir = std::env::temp_dir().join(format!("suramak-ui-audio-req-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (wav, _bytes) = wav16k("resolve", 1_000);
        let mut transcript = transcript("audio-aware.wav");
        transcript.audio_path = Some(wav.clone());

        // Off (the default) — even with a recording present — and no polish
        // section at all.
        let settings = settings_with_polish("http://localhost:9/v1", "m");
        assert_eq!(
            polish_audio_request(&settings, &transcript),
            AudioRequest::Off
        );
        assert_eq!(
            polish_audio_request(&Settings::default(), &transcript),
            AudioRequest::Off
        );

        // On + the recording exists → the recording itself.
        let mut on = settings.clone();
        on.polish.as_mut().unwrap().send_audio = true;
        assert_eq!(
            polish_audio_request(&on, &transcript),
            AudioRequest::Recording(wav.clone())
        );

        // On + the file is gone → text-only + the keep-audio/file-gone note.
        transcript.audio_path = Some(dir.join("gone.wav"));
        match polish_audio_request(&on, &transcript) {
            AudioRequest::Unavailable { note } => assert_eq!(note, AUDIO_FILE_GONE_NOTE),
            other => panic!("expected Unavailable, got {other:?}"),
        }

        // On + no audio attached at all → text-only + the no-audio note.
        transcript.audio_path = None;
        match polish_audio_request(&on, &transcript) {
            AudioRequest::Unavailable { note } => assert_eq!(note, AUDIO_NOT_ATTACHED_NOTE),
            other => panic!("expected Unavailable, got {other:?}"),
        }

        std::fs::remove_file(&wav).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn degradation_notes_surface_with_the_review() {
        let mut state = state_with_editor(true);
        state.start_polish();
        let notes = vec![
            "the endpoint rejected the recording (HTTP 400: nope) — retried without it".to_string(),
        ];
        state.on_polish_done(Ok(PolishOutcome {
            segments: vec![segment(0, "Hello world."), segment(1, "This is a test.")],
            notes: notes.clone(),
        }));
        match &state.polish {
            PolishState::Review(review) => {
                assert_eq!(review.notes, notes, "the notes ride the review");
                assert_eq!(review.notes_line().as_deref(), Some(notes[0].as_str()));
            }
            other => panic!("expected a review, got {other:?}"),
        }
        // Accept still works with notes attached.
        assert!(matches!(
            state.accept_polish().as_slice(),
            [Effect::SaveEdits { .. }]
        ));

        // A plain outcome carries no notes.
        let mut state = state_with_editor(true);
        state.start_polish();
        state.on_polish_done(Ok(PolishOutcome::plain(vec![
            segment(0, "Hello world."),
            segment(1, "This is a test."),
        ])));
        match &state.polish {
            PolishState::Review(review) => assert!(review.notes_line().is_none()),
            other => panic!("expected a review, got {other:?}"),
        }
    }

    #[test]
    fn audio_aware_polish_sends_the_recording_through_the_runtime() {
        let (url, seen) = spawn_scripted_mock(1, |_index, request| {
            let ids = payload_ids(&request_body(request));
            ("200 OK".to_string(), reply_for(&ids))
        });
        with_polish_env("audio-aware", |_root| {
            write_settings_send_audio(&url, true);
            let mut runtime = Runtime::new(egui::Context::default());
            assert!(runtime.polish_configured());

            let (wav, bytes) = wav16k("runtime", 32_000);
            let mut state = AppState::new();
            state.polish_configured = true;
            let mut transcript = transcript("audio-aware.wav");
            transcript.audio_path = Some(wav.clone());
            Store::open(&paths::db_path().unwrap())
                .unwrap()
                .insert_transcript(&transcript)
                .unwrap();
            state.on_transcript_loaded(transcript);

            runtime.apply(state.start_polish(), &mut state);
            let outcome = wait_for_polish(&mut runtime).expect("the run succeeds");
            assert!(outcome.notes.is_empty(), "a clean audio run has no notes");
            state.on_polish_done(Ok(outcome));
            match &state.polish {
                PolishState::Review(review) => {
                    assert_eq!(review.rows[0].polished, "Polished 0.");
                    assert_eq!(review.rows[1].polished, "Polished 1.");
                    assert!(review.notes_line().is_none());
                }
                other => panic!("expected a review, got {other:?}"),
            }

            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 1, "one request for a single-chunk recording");
            let body = request_body(&seen[0]);
            let (data, format) =
                audio_part(&body["messages"][1]["content"]).expect("the recording is on the wire");
            assert_eq!(format, "wav");
            // The payload is the recording's own WAV bytes, base64'd as-is.
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .expect("base64"),
                bytes,
                "the chunk is the recording's exact bytes"
            );
            assert_eq!(payload_ids(&body), vec![0, 1]);

            std::fs::remove_file(&wav).ok();
        });
    }

    #[test]
    fn a_rejected_recording_retries_text_only_and_notes_it_in_the_review() {
        let (url, seen) = spawn_scripted_mock(2, |index, request| {
            if index == 0 {
                (
                    "400 Bad Request".to_string(),
                    r#"{"error":{"message":"audio unsupported"}}"#.to_string(),
                )
            } else {
                let ids = payload_ids(&request_body(request));
                ("200 OK".to_string(), reply_for(&ids))
            }
        });
        with_polish_env("audio-400", |_root| {
            write_settings_send_audio(&url, true);
            let mut runtime = Runtime::new(egui::Context::default());

            let (wav, _bytes) = wav16k("rejected", 32_000);
            let mut state = AppState::new();
            state.polish_configured = true;
            let mut transcript = transcript("rejected.wav");
            transcript.audio_path = Some(wav.clone());
            Store::open(&paths::db_path().unwrap())
                .unwrap()
                .insert_transcript(&transcript)
                .unwrap();
            state.on_transcript_loaded(transcript);

            runtime.apply(state.start_polish(), &mut state);
            let outcome = wait_for_polish(&mut runtime).expect("the ladder retries text-only");
            assert_eq!(outcome.notes.len(), 1);
            let note = &outcome.notes[0];
            assert!(note.contains("HTTP 400"), "names the status: {note}");
            assert!(note.contains("retried without"), "names the retry: {note}");
            assert_eq!(outcome.segments[1].text, "Polished 1.");

            state.on_polish_done(Ok(outcome));
            match &state.polish {
                PolishState::Review(review) => {
                    assert!(review
                        .notes_line()
                        .expect("the note is shown")
                        .contains("HTTP 400"));
                    assert_eq!(review.rows[1].polished, "Polished 1.");
                }
                other => panic!("expected a review, got {other:?}"),
            }

            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 2, "the audio attempt + the text-only retry");
            assert!(
                audio_part(&request_body(&seen[0])["messages"][1]["content"]).is_some(),
                "the first attempt carries the recording"
            );
            assert!(
                audio_part(&request_body(&seen[1])["messages"][1]["content"]).is_none(),
                "the retry is text-only"
            );

            std::fs::remove_file(&wav).ok();
        });
    }

    #[test]
    fn a_gone_recording_degrades_to_text_only_with_the_note_in_the_review() {
        let (url, seen) = spawn_scripted_mock(1, |_index, request| {
            let ids = payload_ids(&request_body(request));
            ("200 OK".to_string(), reply_for(&ids))
        });
        with_polish_env("audio-gone", |root| {
            write_settings_send_audio(&url, true);
            let mut runtime = Runtime::new(egui::Context::default());

            let mut state = AppState::new();
            state.polish_configured = true;
            let mut transcript = transcript("gone.wav");
            transcript.audio_path = Some(root.join("recordings").join("gone.wav"));
            Store::open(&paths::db_path().unwrap())
                .unwrap()
                .insert_transcript(&transcript)
                .unwrap();
            state.on_transcript_loaded(transcript);

            runtime.apply(state.start_polish(), &mut state);
            let outcome = wait_for_polish(&mut runtime).expect("the run succeeds text-only");
            assert_eq!(outcome.notes, vec![AUDIO_FILE_GONE_NOTE.to_string()]);
            state.on_polish_done(Ok(outcome));
            match &state.polish {
                PolishState::Review(review) => {
                    let line = review.notes_line().expect("the note is shown");
                    assert!(line.contains("keep audio was off"), "unexpected: {line}");
                }
                other => panic!("expected a review, got {other:?}"),
            }

            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert!(
                audio_part(&request_body(&seen[0])["messages"][1]["content"]).is_none(),
                "no recording was sent"
            );
        });
    }
}
