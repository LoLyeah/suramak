//! Central keyboard map (M9b): one dispatch point, one guard, unit-tested.
//!
//! | key | action | scope |
//! |---|---|---|
//! | Space | play / pause | editor, with a playable source |
//! | ⌘R | start / stop recording | anywhere (the pill's action) |
//! | ⌘E | open / close the export sheet | editor |
//! | Esc | close the sheet, else leave the editor | anywhere the sheet is open, else the editor |
//! | ↑ / ↓ | previous / next segment (seek to its start) | editor, with a playable source |
//! | ⌘⌫ | arm the delete confirm (the existing flow) | editor |
//!
//! **The guard:** no shortcut fires while a text edit has focus — the exception
//! is Esc, which always works (closing the sheet / leaving the editor is the way
//! out). The guard is read from the focus as of the frame's start
//! (`TextEditState::load` on `Memory::focused`), the same information any
//! in-pass check would see.
//!
//! **The seam:** [`dispatch`] runs in `Shell::render` on the raw input *before*
//! egui sees it. A matched shortcut's key event is removed from the event list,
//! so the focused widget, egui's own Tab/arrow focus movement and the keymap can
//! never both act on one press. The decision itself is the pure [`resolve`] +
//! [`segment_target`], so every rule is unit-tested without a window.
//!
//! Play/pause, the sheet and segment navigation go through the same `AppState`
//! methods the pointer controls use, so keyboard-triggered actions reuse the
//! existing transitions (the M9b motion section) by construction.

use std::time::Instant;

use egui::{Event, Key, Modifiers};
use suramak_core::transcript::Segment;

use super::{AppState, Effect, RecordState, Route};

/// Every shortcut the app defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    /// Space — play / pause (editor).
    PlayPause,
    /// ⌘R — start / stop recording (anywhere).
    Record,
    /// ⌘E — toggle the export sheet (editor).
    Export,
    /// Esc — close the sheet, else leave the editor.
    Escape,
    /// ↑ — previous segment (editor).
    SegmentUp,
    /// ↓ — next segment (editor).
    SegmentDown,
    /// ⌘⌫ — arm the delete confirm (editor).
    Delete,
}

/// The app facts a shortcut's guard reads. Built once per frame, from the state
/// as of the frame's start (the focus guard included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyContext {
    pub route: Route,
    /// A text edit (row, path field, destination…) holds keyboard focus.
    pub text_focused: bool,
    pub export_open: bool,
    pub has_editor: bool,
    /// The open transcript's transport can play (source ready / retryable).
    pub can_play: bool,
    pub recording: bool,
    pub can_record: bool,
    /// M9e (P1-1): the quit confirm is up — it is modal, and Esc answers it
    /// (Keep Recording).
    pub quit_prompt: bool,
    /// M9e (P2-2): the transcribe screen has a job in flight — Esc cancels it
    /// (the same path as the card's Cancel control).
    pub job_cancellable: bool,
}

/// What a shortcut resolves to, given the context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    TogglePlayback,
    ToggleRecord,
    ToggleExport,
    CloseExport,
    LeaveEditor,
    SegmentPrev,
    SegmentNext,
    ArmDelete,
    /// M9e: Esc on the quit confirm — Keep Recording (cancel the quit).
    QuitKeep,
    /// M9e: Esc on the transcribe screen — cancel the running job.
    CancelJob,
}

/// The direction of a segment step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Previous,
    Next,
}

/// The pure dispatch rule: which action (if any) a shortcut fires in `context`.
pub fn resolve(shortcut: Shortcut, context: &KeyContext) -> Option<Action> {
    // M9e (P1-1): the quit confirm is modal — nothing fires while it is up
    // except Esc, which answers it (Keep Recording; cancel the quit).
    if context.quit_prompt {
        return if shortcut == Shortcut::Escape {
            Some(Action::QuitKeep)
        } else {
            None
        };
    }
    let editor = context.route == Route::Editor && context.has_editor;
    match shortcut {
        // Space: the transport, but never while typing.
        Shortcut::PlayPause if !context.text_focused && editor && context.can_play => {
            Some(Action::TogglePlayback)
        }
        // ⌘R: recording is global (it is the floating pill's action).
        Shortcut::Record if !context.text_focused && (context.recording || context.can_record) => {
            Some(Action::ToggleRecord)
        }
        Shortcut::Export if !context.text_focused && editor => Some(Action::ToggleExport),
        // Esc is the guard's exception: the way out always works.
        Shortcut::Escape if context.export_open => Some(Action::CloseExport),
        Shortcut::Escape if editor => Some(Action::LeaveEditor),
        // M9e (P2-2): Esc on the transcribe screen cancels the running job —
        // the same path as the card's Cancel control.
        Shortcut::Escape if context.job_cancellable => Some(Action::CancelJob),
        Shortcut::SegmentUp if !context.text_focused && editor && context.can_play => {
            Some(Action::SegmentPrev)
        }
        Shortcut::SegmentDown if !context.text_focused && editor && context.can_play => {
            Some(Action::SegmentNext)
        }
        Shortcut::Delete if !context.text_focused && editor => Some(Action::ArmDelete),
        _ => None,
    }
}

/// One key binding: the modifiers to match (`Modifiers::COMMAND` is ⌘ on macOS,
/// Ctrl elsewhere), the logical key, and whether key-repeats count (segment
/// navigation wants them — holding ↓ walks the transcript).
struct Binding {
    shortcut: Shortcut,
    modifiers: Modifiers,
    key: Key,
    repeat: bool,
}

/// ⌘-shortcuts first, then the plain keys (the patterns cannot overlap, but the
/// order documents the intent).
const BINDINGS: [Binding; 7] = [
    Binding {
        shortcut: Shortcut::Record,
        modifiers: Modifiers::COMMAND,
        key: Key::R,
        repeat: false,
    },
    Binding {
        shortcut: Shortcut::Export,
        modifiers: Modifiers::COMMAND,
        key: Key::E,
        repeat: false,
    },
    Binding {
        shortcut: Shortcut::Delete,
        modifiers: Modifiers::COMMAND,
        key: Key::Backspace,
        repeat: false,
    },
    Binding {
        shortcut: Shortcut::PlayPause,
        modifiers: Modifiers::NONE,
        key: Key::Space,
        repeat: false,
    },
    Binding {
        shortcut: Shortcut::Escape,
        modifiers: Modifiers::NONE,
        key: Key::Escape,
        repeat: false,
    },
    Binding {
        shortcut: Shortcut::SegmentUp,
        modifiers: Modifiers::NONE,
        key: Key::ArrowUp,
        repeat: true,
    },
    Binding {
        shortcut: Shortcut::SegmentDown,
        modifiers: Modifiers::NONE,
        key: Key::ArrowDown,
        repeat: true,
    },
];

/// Whether a text edit holds keyboard focus right now (the guard's source).
///
/// `Memory::focused` names the focused widget; a focused widget carrying
/// `TextEditState` is a text edit (row, path field, destination field). The
/// check is the same one egui's own widgets would see in this pass.
pub fn text_edit_focused(ctx: &egui::Context) -> bool {
    ctx.memory(|memory| memory.focused())
        .is_some_and(|id| egui::widgets::text_edit::TextEditState::load(ctx, id).is_some())
}

/// The dispatch seam: consumes the frame's raw input events, applies every
/// shortcut that fires and returns the effects they produced. Events are
/// removed **only** when an action fires — a guarded key (Space while typing,
/// arrows outside the editor) is left for egui.
pub fn dispatch(ctx: &egui::Context, state: &mut AppState, events: &mut Vec<Event>) -> Vec<Effect> {
    let context = KeyContext {
        route: state.route,
        text_focused: text_edit_focused(ctx),
        export_open: state.export.open,
        has_editor: state.editor.is_some(),
        can_play: state.playback.can_play(),
        recording: matches!(state.record, RecordState::Recording(_)),
        can_record: state.can_record(),
        quit_prompt: state.quit_prompt.is_some(),
        job_cancellable: state.job_cancellable(),
    };
    let mut effects = Vec::new();
    for binding in BINDINGS {
        if !pressed(events, &binding) {
            continue;
        }
        let Some(action) = resolve(binding.shortcut, &context) else {
            continue;
        };
        consume(events, &binding);
        effects.extend(apply(action, state));
    }
    effects
}

/// Whether the event list holds a matching press (peek — nothing is consumed).
fn pressed(events: &[Event], binding: &Binding) -> bool {
    events.iter().any(|event| matches_binding(event, binding))
}

/// Removes the matching press from the event list (the key never reaches egui).
fn consume(events: &mut Vec<Event>, binding: &Binding) {
    events.retain(|event| !matches_binding(event, binding));
}

fn matches_binding(event: &Event, binding: &Binding) -> bool {
    matches!(
        event,
        Event::Key {
            key,
            modifiers,
            pressed: true,
            repeat,
            ..
        } if *key == binding.key
            && modifiers.matches_logically(binding.modifiers)
            && (binding.repeat || !*repeat)
    )
}

/// Executes an action against the state, returning the effects it wants run.
fn apply(action: Action, state: &mut AppState) -> Vec<Effect> {
    match action {
        Action::TogglePlayback => state.toggle_playback(),
        Action::ToggleRecord => {
            let date = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            state.toggle_record(Instant::now(), &date)
        }
        Action::ToggleExport => {
            state.toggle_export();
            Vec::new()
        }
        Action::CloseExport => {
            state.export.open = false;
            Vec::new()
        }
        Action::LeaveEditor => state.back_home(),
        Action::SegmentPrev => segment_nav(state, Direction::Previous),
        Action::SegmentNext => segment_nav(state, Direction::Next),
        Action::ArmDelete => {
            if let Some(editor) = state.editor.as_mut() {
                editor.confirm_delete = true;
            }
            Vec::new()
        }
        // M9e: Esc answers the quit confirm — Keep Recording; the outcome
        // (Stay) travels to the frame loop like a button's.
        Action::QuitKeep => match state.answer_quit(super::QuitAnswer::Keep) {
            Some(outcome) => vec![Effect::Quit(outcome)],
            None => Vec::new(),
        },
        // M9e: Esc on the transcribe screen — the card's Cancel path.
        Action::CancelJob => state.cancel_job(),
    }
}

/// Seeks the playhead to the previous / next segment's start — the active-row
/// tint crossfade follows the playhead, so navigation reuses it as-is.
fn segment_nav(state: &mut AppState, direction: Direction) -> Vec<Effect> {
    let Some(editor) = state.editor.as_ref() else {
        return Vec::new();
    };
    let Some(target) = segment_target(
        &editor.transcript.segments,
        state.playback.position_ms(),
        direction,
    ) else {
        return Vec::new();
    };
    state.seek_playback(target)
}

/// The seek target for a segment step, pure: `Next` is the first segment start
/// after the playhead (clamped to the last start at the end), `Previous` the
/// last start before it (clamped to the first start at the top).
pub fn segment_target(segments: &[Segment], position_ms: u32, direction: Direction) -> Option<u32> {
    if segments.is_empty() {
        return None;
    }
    let position = i64::from(position_ms);
    let target = match direction {
        Direction::Next => segments
            .iter()
            .map(|segment| segment.start_ms)
            .find(|start| *start > position),
        Direction::Previous => segments
            .iter()
            .map(|segment| segment.start_ms)
            .rfind(|start| *start < position),
    };
    let fallback = match direction {
        Direction::Next => segments.last(),
        Direction::Previous => segments.first(),
    };
    let target = target.or_else(|| fallback.map(|segment| segment.start_ms))?;
    Some(target.clamp(0, i64::from(u32::MAX)) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_core::transcript::{Source, Transcript};

    fn segment(idx: u32, start_ms: i64, end_ms: i64) -> Segment {
        Segment {
            idx,
            start_ms,
            end_ms,
            text: format!("segment {idx}"),
        }
    }

    fn editor_state() -> AppState {
        let dir = std::env::temp_dir().join(format!("suramak-keymap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let wav = dir.join("keymap.wav");
        std::fs::write(&wav, b"RIFF....").expect("temp wav");
        let transcript = Transcript::new(
            "keymap.wav".to_string(),
            Source::File {
                name: "keymap.wav".to_string(),
            },
            "tiny".to_string(),
            Some("en".to_string()),
            Some(wav),
            vec![
                segment(0, 0, 900),
                segment(1, 1_000, 1_900),
                segment(2, 2_000, 2_900),
            ],
        );
        let mut state = AppState::new();
        state.on_transcript_loaded(transcript);
        state
    }

    fn context(state: &AppState, text_focused: bool) -> KeyContext {
        KeyContext {
            route: state.route,
            text_focused,
            export_open: state.export.open,
            has_editor: state.editor.is_some(),
            can_play: state.playback.can_play(),
            recording: matches!(state.record, RecordState::Recording(_)),
            can_record: state.can_record(),
            quit_prompt: state.quit_prompt.is_some(),
            job_cancellable: state.job_cancellable(),
        }
    }

    fn key_event(key: Key, modifiers: Modifiers, repeat: bool) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        }
    }

    // ---- the pure rules ------------------------------------------------------

    #[test]
    fn space_toggles_playback_only_in_the_editor_with_a_source() {
        let state = editor_state();
        let open = context(&state, false);
        assert_eq!(
            resolve(Shortcut::PlayPause, &open),
            Some(Action::TogglePlayback)
        );
        // Outside the editor: no transport to toggle.
        let home = KeyContext {
            route: Route::Home,
            ..open
        };
        assert_eq!(resolve(Shortcut::PlayPause, &home), None);
        // No transcript open: nothing to play.
        let empty = KeyContext {
            has_editor: false,
            ..open
        };
        assert_eq!(resolve(Shortcut::PlayPause, &empty), None);
        // A source that cannot play: the shortcut is dead (the control dims too).
        let unplayable = KeyContext {
            can_play: false,
            ..open
        };
        assert_eq!(resolve(Shortcut::PlayPause, &unplayable), None);
    }

    #[test]
    fn the_text_edit_focus_guard_blocks_every_shortcut_except_escape() {
        let state = editor_state();
        let typing = context(&state, true);
        for shortcut in [
            Shortcut::PlayPause,
            Shortcut::Record,
            Shortcut::Export,
            Shortcut::SegmentUp,
            Shortcut::SegmentDown,
            Shortcut::Delete,
        ] {
            assert_eq!(
                resolve(shortcut, &typing),
                None,
                "{shortcut:?} must not fire while a text edit has focus"
            );
        }
        // …while Esc is the documented exception and still leaves the editor.
        assert_eq!(
            resolve(Shortcut::Escape, &typing),
            Some(Action::LeaveEditor)
        );
    }

    #[test]
    fn command_r_toggles_recording_and_stops_a_live_session() {
        let mut state = AppState::new();
        state.capture_available = true;
        // Idle: ⌘R starts a recording.
        let idle = context(&state, false);
        assert_eq!(resolve(Shortcut::Record, &idle), Some(Action::ToggleRecord));
        state.start_recording(Instant::now(), "2026-10-14 09:00:00");
        // Recording: ⌘R stops it (from the record route too).
        let live = context(&state, false);
        assert_eq!(live.route, Route::Record);
        assert_eq!(resolve(Shortcut::Record, &live), Some(Action::ToggleRecord));
        // A running job locks recording, so the shortcut is inert.
        let mut busy = AppState::new();
        busy.capture_available = true;
        let input = std::env::temp_dir().join("suramak-keymap-busy.wav");
        std::fs::write(&input, b"RIFF....").expect("temp wav");
        busy.begin_import(input);
        let busy = context(&busy, false);
        assert_eq!(resolve(Shortcut::Record, &busy), None);
    }

    #[test]
    fn command_e_toggles_the_sheet_only_in_the_editor() {
        let mut state = editor_state();
        let open = context(&state, false);
        assert_eq!(resolve(Shortcut::Export, &open), Some(Action::ToggleExport));
        state.export.open = true;
        let sheet_open = context(&state, false);
        assert_eq!(
            resolve(Shortcut::Export, &sheet_open),
            Some(Action::ToggleExport),
            "⌘E closes it again"
        );
        let home = KeyContext {
            route: Route::Home,
            ..sheet_open
        };
        assert_eq!(resolve(Shortcut::Export, &home), None);
    }

    #[test]
    fn escape_closes_the_sheet_before_leaving_the_editor() {
        let mut state = editor_state();
        let editor = context(&state, false);
        assert_eq!(
            resolve(Shortcut::Escape, &editor),
            Some(Action::LeaveEditor)
        );
        // The sheet wins: Esc closes it first, the editor stays.
        state.export.open = true;
        let sheet = context(&state, false);
        assert_eq!(resolve(Shortcut::Escape, &sheet), Some(Action::CloseExport));
        // Somewhere with nothing to leave: inert.
        let home = KeyContext {
            route: Route::Home,
            export_open: false,
            has_editor: false,
            ..sheet
        };
        assert_eq!(resolve(Shortcut::Escape, &home), None);
    }

    #[test]
    fn arrow_keys_move_between_segment_starts() {
        let state = editor_state();
        let open = context(&state, false);
        assert_eq!(
            resolve(Shortcut::SegmentDown, &open),
            Some(Action::SegmentNext)
        );
        assert_eq!(
            resolve(Shortcut::SegmentUp, &open),
            Some(Action::SegmentPrev)
        );

        let segments = &state.editor.as_ref().unwrap().transcript.segments;
        // From the top: ↓ lands on the first segment start, ↑ clamps there too.
        assert_eq!(segment_target(segments, 0, Direction::Next), Some(1_000));
        assert_eq!(segment_target(segments, 0, Direction::Previous), Some(0));
        // Inside segment 1: ↓ goes to segment 2's start, ↑ back to segment 1's.
        assert_eq!(
            segment_target(segments, 1_500, Direction::Next),
            Some(2_000)
        );
        assert_eq!(
            segment_target(segments, 1_500, Direction::Previous),
            Some(1_000)
        );
        // At/after the last start: ↓ clamps to it (no teleport past the end).
        assert_eq!(
            segment_target(segments, 2_000, Direction::Next),
            Some(2_000)
        );
        assert_eq!(
            segment_target(segments, 9_000, Direction::Next),
            Some(2_000)
        );
        assert_eq!(
            segment_target(segments, 9_000, Direction::Previous),
            Some(2_000)
        );
        // An empty transcript has nowhere to go.
        assert_eq!(segment_target(&[], 0, Direction::Next), None);
    }

    #[test]
    fn command_backspace_arms_the_existing_confirm_flow() {
        let mut state = editor_state();
        let open = context(&state, false);
        assert_eq!(resolve(Shortcut::Delete, &open), Some(Action::ArmDelete));
        // Arming never deletes: the trash confirm step is the only path to the
        // delete effect, and this shortcut only arms it.
        assert!(!state.editor.as_ref().unwrap().confirm_delete);
        let effects = apply(Action::ArmDelete, &mut state);
        assert!(effects.is_empty(), "arming emits no delete effect");
        assert!(state.editor.as_ref().unwrap().confirm_delete);
    }

    // ---- M9e: the quit confirm + the job cancel ------------------------------

    /// The quit confirm is modal: nothing fires while it is up except Esc,
    /// which answers it with Keep Recording (the quit is cancelled).
    #[test]
    fn the_quit_confirm_is_modal_and_escape_keeps_recording() {
        let mut state = AppState::new();
        state.capture_available = true;
        state.start_recording(Instant::now(), "2026-10-14 09:00:00");
        assert_eq!(
            state.request_quit(crate::app::QuitSource::WindowClose),
            crate::app::QuitAction::ShowConfirm
        );
        let ctx = context(&state, false);
        assert!(ctx.quit_prompt);
        for shortcut in [
            Shortcut::PlayPause,
            Shortcut::Record,
            Shortcut::Export,
            Shortcut::SegmentUp,
            Shortcut::SegmentDown,
            Shortcut::Delete,
        ] {
            assert_eq!(
                resolve(shortcut, &ctx),
                None,
                "{shortcut:?} must not fire while the confirm is up"
            );
        }
        // Esc answers it — and only Esc.
        assert_eq!(resolve(Shortcut::Escape, &ctx), Some(Action::QuitKeep));
        let effects = apply(Action::QuitKeep, &mut state);
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::Quit(crate::app::QuitOutcome::Stay)]
            ),
            "Esc keeps recording: {effects:?}"
        );
        assert!(state.quit_prompt.is_none());
        assert!(matches!(state.record, RecordState::Recording(_)));
    }

    /// Esc on the transcribe screen with a job in flight resolves to the same
    /// cancel path the card's control uses; without a job it stays inert.
    #[test]
    fn escape_cancels_a_running_job_on_the_transcribe_screen() {
        let mut state = AppState::new();
        let input = std::env::temp_dir().join("suramak-keymap-cancel.wav");
        std::fs::write(&input, b"RIFF....").expect("temp wav");
        state.begin_import(input);
        assert_eq!(state.route, Route::Transcribe);
        let ctx = context(&state, false);
        assert_eq!(resolve(Shortcut::Escape, &ctx), Some(Action::CancelJob));
        let effects = apply(Action::CancelJob, &mut state);
        assert!(
            matches!(effects.as_slice(), [Effect::CancelJob]),
            "Esc sets the cancel token: {effects:?}"
        );
        // Idempotent: a second Esc (or a second Cancel click) is a no-op.
        assert!(apply(Action::CancelJob, &mut state).is_empty());

        // The failure state has no job: Esc is inert there (unchanged).
        state.job_failed("boom".to_string());
        let ctx = context(&state, false);
        assert_eq!(resolve(Shortcut::Escape, &ctx), None);
    }

    // ---- the dispatch seam ---------------------------------------------------

    /// Runs two frames that draw a focused `TextEdit`, so the guard has a real
    /// focused text edit to see (the same information a pass would).
    fn focus_a_text_edit(ctx: &egui::Context) -> egui::Id {
        let id = egui::Id::new("suramak-test-text-edit");
        let mut text = String::from("hello");
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.add(egui::TextEdit::singleline(&mut text).id(id));
                    response.request_focus();
                });
            });
        }
        id
    }

    #[test]
    fn dispatch_applies_the_action_and_consumes_the_key() {
        let ctx = egui::Context::default();
        let mut state = editor_state();
        let mut events = vec![key_event(Key::Space, Modifiers::NONE, false)];
        let effects = dispatch(&ctx, &mut state, &mut events);
        assert!(
            events.is_empty(),
            "the key is consumed so egui never sees it too"
        );
        assert!(state.playback.is_playing(), "Space played");
        assert!(matches!(
            effects.as_slice(),
            [Effect::Playback {
                command: suramak_playback::Command::Play { .. },
                ..
            }]
        ));
    }

    #[test]
    fn dispatch_never_fires_while_a_text_edit_is_focused() {
        let ctx = egui::Context::default();
        let id = focus_a_text_edit(&ctx);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));
        assert!(text_edit_focused(&ctx));

        let mut state = editor_state();
        let mut events = vec![key_event(Key::Space, Modifiers::NONE, false)];
        let effects = dispatch(&ctx, &mut state, &mut events);
        assert!(effects.is_empty(), "no action while typing");
        assert!(!state.playback.is_playing());
        assert_eq!(events.len(), 1, "the Space is left for the text edit");
        // The arrows and ⌘⌫ stay with the text edit too.
        let mut events = vec![
            key_event(Key::ArrowDown, Modifiers::NONE, false),
            key_event(Key::Backspace, Modifiers::COMMAND, false),
        ];
        assert!(dispatch(&ctx, &mut state, &mut events).is_empty());
        assert_eq!(events.len(), 2);
        assert!(!state.editor.as_ref().unwrap().confirm_delete);

        // Esc is the exception: it still closes the sheet / leaves the editor.
        state.export.open = true;
        let mut events = vec![key_event(Key::Escape, Modifiers::NONE, false)];
        assert!(dispatch(&ctx, &mut state, &mut events).is_empty());
        assert!(events.is_empty(), "Esc is consumed");
        assert!(!state.export.open, "the sheet closed while typing");
    }

    #[test]
    fn dispatch_handles_the_command_shortcuts() {
        let ctx = egui::Context::default();
        // ⌘E opens the sheet (on macOS ⌘ = cmd; the pattern is COMMAND).
        let mut state = editor_state();
        let mut events = vec![key_event(Key::E, Modifiers::COMMAND, false)];
        assert!(dispatch(&ctx, &mut state, &mut events).is_empty());
        assert!(state.export.open);
        // ⌘E again closes it.
        let mut events = vec![key_event(Key::E, Modifiers::COMMAND, false)];
        dispatch(&ctx, &mut state, &mut events);
        assert!(!state.export.open);
        // Esc leaves the editor (flushing nothing — no edits).
        let mut events = vec![key_event(Key::Escape, Modifiers::NONE, false)];
        assert!(dispatch(&ctx, &mut state, &mut events).is_empty());
        assert_eq!(state.route, Route::Home);
        assert!(events.is_empty());

        // ⌘R starts a recording (capture compiled in for the state machine).
        let mut state = editor_state();
        state.capture_available = true;
        let mut events = vec![key_event(Key::R, Modifiers::COMMAND, false)];
        let effects = dispatch(&ctx, &mut state, &mut events);
        assert!(matches!(effects.as_slice(), [Effect::StartCapture]));
        assert_eq!(state.route, Route::Record);
        // …and ⌘R again stops it.
        let mut events = vec![key_event(Key::R, Modifiers::COMMAND, false)];
        let effects = dispatch(&ctx, &mut state, &mut events);
        assert!(matches!(effects.as_slice(), [Effect::StopCapture { .. }]));
    }

    #[test]
    fn dispatch_leaves_unhandled_and_guarded_keys_alone() {
        let ctx = egui::Context::default();
        // A plain arrow on the Home route is not ours: egui keeps it.
        let mut state = AppState::new();
        let mut events = vec![key_event(Key::ArrowDown, Modifiers::NONE, false)];
        assert!(dispatch(&ctx, &mut state, &mut events).is_empty());
        assert_eq!(events.len(), 1);
        // A ⌘-modified key never matches the plain bindings.
        let mut events = vec![key_event(Key::Space, Modifiers::COMMAND, false)];
        assert!(dispatch(&ctx, &mut state, &mut events).is_empty());
        assert_eq!(events.len(), 1);
        // Key repeats only count where the binding asks for them.
        let mut state = editor_state();
        let mut events = vec![key_event(Key::Space, Modifiers::NONE, true)];
        assert!(dispatch(&ctx, &mut state, &mut events).is_empty());
        assert_eq!(events.len(), 1, "a held Space does not toggle playback");
        assert!(!state.playback.is_playing());
        let mut events = vec![key_event(Key::ArrowDown, Modifiers::NONE, true)];
        dispatch(&ctx, &mut state, &mut events);
        assert!(events.is_empty(), "held ↓ walks the transcript");
        assert_eq!(state.playback.position_ms(), 1_000);
    }
}
