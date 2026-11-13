//! M24B: the desktop's Notes flow — the editor action, the sheet's host
//! methods and the store round trip.
//!
//! The sheet's state machine itself is [`crate::notes_sheet::NotesSheet`], the
//! same one the phone drives. This file only wires it to the desktop app: open
//! (read the stored rows), Run (snapshot the editor and emit the request),
//! result (upsert + persist), Export/Copy.

use suramak_core::notes::{NotePreset, NotesOutcome, StoredNote};
use suramak_core::store::NoteRecord;

use super::{AppState, Effect};

impl AppState {
    /// The header's `Notes` action: open the sheet and read this transcript's
    /// stored notes. The gate is the polish gate — notes use the same endpoint.
    pub fn open_notes(&mut self) -> Vec<Effect> {
        let Some(editor) = self.editor.as_ref() else {
            return Vec::new();
        };
        let id = editor.transcript.id;
        self.notes
            .open(self.polish_configured, Vec::new(), &editor.transcript);
        self.notes.dest = self.note_dest();
        // Seed from what we already hold so a reopen never flashes empty while
        // the store read lands.
        vec![Effect::LoadNotes { transcript: id }]
    }

    /// The suggested `Export .md` path in the user's home directory.
    fn note_dest(&self) -> String {
        let Some(editor) = self.editor.as_ref() else {
            return String::new();
        };
        let name = self.notes.export_name(&editor.transcript);
        super::default_dest(&name, super::ExportFormat::Md)
    }

    /// Selects a preset row and refreshes the suggested file name.
    pub fn select_note_preset(&mut self, preset: NotePreset) {
        self.notes.select(preset);
        self.notes.dest = self.note_dest();
    }

    /// The header's `Notes` action toggles the sheet (opening reads the store).
    pub fn toggle_notes(&mut self) -> Vec<Effect> {
        if self.notes.open {
            self.close_notes();
            Vec::new()
        } else {
            self.open_notes()
        }
    }

    /// Closes the sheet (a run in flight still lands in `stored`).
    pub fn close_notes(&mut self) {
        self.notes.close();
    }

    /// The `Run` button: snapshot the editor (unsaved edits included) and ask
    /// the runtime to generate the selected preset.
    pub fn start_notes(&mut self) -> Vec<Effect> {
        if !self.notes.open {
            return Vec::new();
        }
        let preset = match self.notes.start() {
            Ok(preset) => preset,
            Err(message) => {
                // A refusal is a named state, not a silent no-op.
                self.set_status(message, true);
                return Vec::new();
            }
        };
        let Some(editor) = self.editor.as_ref() else {
            return Vec::new();
        };
        let transcript = editor.snapshot();
        let speakers: Vec<(u32, String)> = editor
            .transcript
            .segments
            .iter()
            .zip(self.diarization.per_segment.iter())
            .filter_map(|(segment, speaker)| {
                speaker.map(|speaker| (segment.idx, self.diarization.label(speaker.primary)))
            })
            .collect();
        vec![Effect::GenerateNotes {
            transcript: Box::new(transcript),
            preset,
            speakers,
        }]
    }

    /// The store's answer for the open transcript's stored notes.
    pub fn on_notes_loaded(
        &mut self,
        id: suramak_core::Uuid,
        result: Result<Vec<StoredNote>, String>,
    ) {
        if self.notes.transcript != Some(id) {
            return;
        }
        match result {
            Ok(stored) => {
                self.notes.configured = self.polish_configured;
                self.notes.adopt_stored(stored);
            }
            Err(message) => self.set_status(format!("notes could not be read — {message}"), true),
        }
    }

    /// The runtime finished: a validated note lands in the sheet and is
    /// persisted; a failure keeps the old row and names the endpoint error.
    pub fn on_notes_done(&mut self, result: Result<NotesOutcome, String>) -> Vec<Effect> {
        let id = self.notes.transcript;
        let created_at = suramak_core::Utc::now().to_rfc3339();
        let stored = self.notes.on_done(result, created_at);
        let Some(note) = stored else {
            if let Some((_, message)) = self.notes.failure() {
                self.set_status(format!("notes failed — {message}"), true);
            }
            return Vec::new();
        };
        let Some(id) = id else {
            return Vec::new();
        };
        let created = suramak_core::DateTime::parse_from_rfc3339(&note.created_at)
            .map(|at| at.with_timezone(&suramak_core::Utc))
            .unwrap_or_else(|_| suramak_core::Utc::now());
        self.set_status(format!("{} ready", note.preset.label()), false);
        vec![Effect::SaveNote {
            note: Box::new(NoteRecord {
                transcript_id: id,
                preset: note.preset,
                body_md: note.body_md,
                citations_checked: note.citations_checked,
                citations_removed: note.citations_removed,
                model: note.model,
                created_at: created,
                session_id: None,
            }),
        }]
    }

    /// The `Export .md`: the exact stored Markdown and the sheet's destination,
    /// or `None` when the selected preset has no result yet.
    pub fn note_export_request(&self) -> Option<(String, String)> {
        // Only the open editor can export its note.
        self.editor.as_ref()?;
        let markdown = self.notes.export_text()?.to_string();
        let dest = if self.notes.dest.trim().is_empty() {
            self.note_dest()
        } else {
            self.notes.dest.clone()
        };
        Some((dest, markdown))
    }

    /// `Copy`: the exact stored Markdown.
    pub fn note_copy_text(&self) -> Option<String> {
        self.notes.copy_text().map(str::to_string)
    }

    /// The write's outcome (the sheet's status line).
    pub fn on_note_exported(&mut self, name: &str, result: Result<(), String>) {
        match result {
            Ok(()) => self.set_status(format!("Saved {name}"), false),
            Err(message) => self.set_status(format!("notes export failed — {message}"), true),
        }
    }

    /// The gate's hint for the sheet (the same sentence the Polish action
    /// shows when nothing is configured).
    pub fn notes_gate_note(&self) -> Option<&'static str> {
        (!self.polish_configured).then_some(crate::notes_sheet::NOTES_SETUP_NOTE)
    }

    /// Whether the selected preset has a result (`Export`/`Copy` enabled).
    pub fn notes_has_result(&self) -> bool {
        self.notes.current().is_some()
    }
}

/// The Notes sheet: an anchored card under the deck's `Notes` button. One
/// surface, three preset rows, the honest result line, `Export .md` and `Copy`.
pub fn sheet(
    ctx: &egui::Context,
    tokens: &crate::theme::Tokens,
    motion: &crate::motion::Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
    anchor: egui::Rect,
) {
    const WIDTH: f32 = 520.0;
    let fade = motion.fade(
        ctx,
        egui::Id::new("suramak-notes-sheet-fade"),
        state.notes.open,
        crate::motion::BASE,
        crate::motion::FAST,
    );
    if fade <= 0.002 {
        return;
    }
    let pos = egui::pos2(
        (anchor.right() - WIDTH).max(anchor.left()),
        anchor.bottom() + 8.0 + crate::motion::rise(fade, 2.0),
    );
    let area = egui::Area::new(egui::Id::new("suramak-notes-sheet"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .movable(false)
        .constrain(true)
        .interactable(state.notes.open)
        .fade_in(false)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            egui::Frame::NONE
                .fill(tokens.card)
                .stroke(egui::Stroke::new(tokens.stroke.hairline, tokens.hairline))
                .corner_radius(egui::CornerRadius::same(tokens.radius.card))
                .shadow(tokens.card_shadow)
                .inner_margin(egui::Margin::same(tokens.space.l as i8))
                .show(ui, |ui| {
                    ui.set_min_width(WIDTH - tokens.space.l * 2.0);
                    ui.set_max_width(WIDTH - tokens.space.l * 2.0);
                    contents(ui, tokens, motion, state, effects);
                });
        });

    if state.notes.open && ctx.input(|input| input.pointer.any_click()) {
        if let Some(pointer) = ctx.input(|input| input.pointer.interact_pos()) {
            if !area.response.rect.contains(pointer) && !anchor.contains(pointer) {
                state.close_notes();
            }
        }
    }
}

fn contents(
    ui: &mut egui::Ui,
    tokens: &crate::theme::Tokens,
    motion: &crate::motion::Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    use crate::app::controls::{self, Tier};

    ui.label(
        egui::RichText::new("Notes")
            .font(egui::FontId::new(17.0, crate::theme::serif_family()))
            .color(tokens.ink),
    );
    ui.add_space(tokens.space.s);

    if let Some(gate) = state.notes_gate_note() {
        ui.label(egui::RichText::new(gate).size(12.0).color(tokens.ink_soft));
        ui.add_space(tokens.space.m);
    }

    // ---- the three presets --------------------------------------------------
    for preset in NotePreset::ALL {
        let selected = state.notes.preset == preset;
        let stored = state.notes.stored.iter().any(|note| note.preset == preset);
        let label = if stored {
            format!("{}  ✓", preset.label())
        } else {
            preset.label().to_string()
        };
        let enabled = !state.notes.phase.is_running();
        let response = controls::button(
            ui,
            tokens,
            motion,
            ("notes-preset", preset.key()),
            &label,
            if selected {
                Tier::Primary
            } else {
                Tier::Secondary
            },
            enabled,
        );
        if response.clicked() {
            state.select_note_preset(preset);
        }
        ui.add_space(tokens.space.xs);
    }

    if state.notes.short {
        ui.label(
            egui::RichText::new(crate::notes_sheet::NOTES_SHORT_NOTE)
                .size(11.5)
                .color(tokens.ink_soft),
        );
    }
    ui.add_space(tokens.space.s);

    // ---- run ----------------------------------------------------------------
    ui.horizontal(|ui| {
        let run = controls::button(
            ui,
            tokens,
            motion,
            "notes-run",
            if state.notes.current().is_some() {
                "Regenerate"
            } else {
                "Run"
            },
            Tier::Primary,
            state.notes.can_run(),
        );
        if run.clicked() {
            effects.extend(state.start_notes());
        }
        if state.notes.phase.is_running() {
            ui.label(
                egui::RichText::new("Writing notes…")
                    .size(12.0)
                    .color(tokens.ink_soft),
            );
        }
    });

    if let Some((preset, message)) = state.notes.failure() {
        ui.add_space(tokens.space.s);
        ui.label(
            egui::RichText::new(format!("{} failed — {message}", preset.label()))
                .size(11.5)
                .color(tokens.record_red_text),
        );
    }

    // ---- the stored result --------------------------------------------------
    let Some(note) = state.notes.current().cloned() else {
        ui.add_space(tokens.space.m);
        return;
    };
    ui.add_space(tokens.space.m);
    ui.label(
        egui::RichText::new(note.result_line())
            .size(12.0)
            .color(tokens.ink),
    );
    if let Some(breakdown) = state.notes.breakdown_line() {
        ui.label(
            egui::RichText::new(breakdown)
                .size(11.0)
                .color(tokens.ink_soft),
        );
    }
    if let Some(path) = &state.notes.path {
        ui.label(
            egui::RichText::new(format!("{path} · {}", note.model))
                .size(11.0)
                .color(tokens.ink_faint),
        );
    }
    ui.add_space(tokens.space.s);
    egui::ScrollArea::vertical()
        .id_salt("notes-body")
        .max_height(190.0)
        .show(ui, |ui| {
            for line in crate::notes_sheet::note_lines(&note.body_md) {
                let mut text = egui::RichText::new(line.text).color(tokens.ink);
                text = match line.level {
                    3 => text.size(14.5).strong(),
                    2 => text.size(14.5).strong(),
                    1 => text.size(16.0).strong(),
                    _ => text.size(12.5),
                };
                ui.label(text);
            }
        });
    ui.add_space(tokens.space.s);

    // ---- export + copy ------------------------------------------------------
    let width = ui.available_width();
    ui.add_sized(
        [width, 24.0],
        egui::TextEdit::singleline(&mut state.notes.dest)
            .hint_text("output path (.md)")
            .text_color(tokens.ink),
    );
    ui.add_space(tokens.space.s);
    ui.horizontal(|ui| {
        let export = controls::button(
            ui,
            tokens,
            motion,
            "notes-export",
            "Export .md",
            Tier::Secondary,
            state.notes_has_result(),
        );
        if export.clicked() {
            if let Some((dest, markdown)) = state.note_export_request() {
                effects.push(Effect::ExportNote {
                    dest: std::path::PathBuf::from(dest),
                    markdown,
                });
            }
        }
        ui.add_space(tokens.space.s);
        let copy = controls::button(
            ui,
            tokens,
            motion,
            "notes-copy",
            "Copy",
            Tier::Secondary,
            state.notes_has_result(),
        );
        if copy.clicked() {
            if let Some(text) = state.note_copy_text() {
                ui.ctx().copy_text(text);
                state.set_status("note copied to the clipboard", false);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use suramak_core::paths;
    use suramak_core::settings::{PolishSettings, Settings};
    use suramak_core::store::Store;
    use suramak_core::transcript::{Segment, Source, Transcript};

    use super::*;
    use crate::app::TEST_ENV_LOCK as ENV_LOCK;

    fn transcript() -> Transcript {
        Transcript::new(
            "Weekly sync".to_string(),
            Source::File {
                name: "Weekly sync.m4a".to_string(),
            },
            "tiny".to_string(),
            Some("en".to_string()),
            None,
            vec![
                Segment {
                    idx: 0,
                    start_ms: 0,
                    end_ms: 8_000,
                    text: "Good morning, everyone.".to_string(),
                },
                Segment {
                    idx: 1,
                    start_ms: 12_000,
                    end_ms: 20_000,
                    text: "We decided to ship on Friday.".to_string(),
                },
                Segment {
                    idx: 2,
                    start_ms: 65_000,
                    end_ms: 78_000,
                    text: "Budi will own the migration.".to_string(),
                },
            ],
        )
    }

    fn state_with_editor(configured: bool) -> AppState {
        let mut state = AppState::new();
        state.polish_configured = configured;
        state.on_transcript_loaded(transcript());
        state
    }

    #[test]
    fn the_notes_action_opens_the_sheet_with_the_three_presets() {
        let mut state = state_with_editor(true);
        let effects = state.open_notes();
        assert!(state.notes.open);
        assert!(matches!(effects.as_slice(), [Effect::LoadNotes { .. }]));
        assert_eq!(state.notes.stored.len(), 0);
        for preset in NotePreset::ALL {
            state.notes.select(preset);
            assert_eq!(state.notes.preset, preset);
        }
        state.close_notes();
        assert!(!state.notes.open);
    }

    #[test]
    fn the_not_configured_state_refuses_run_with_the_setup_sentence() {
        let mut state = state_with_editor(false);
        state.open_notes();
        assert_eq!(
            state.notes_gate_note(),
            Some(crate::notes_sheet::NOTES_SETUP_NOTE)
        );
        assert!(!state.notes.can_run());
        assert!(state.start_notes().is_empty(), "no request is emitted");
        let status = state.status.as_ref().expect("the refusal is visible");
        assert!(status.text.contains("not configured"), "{}", status.text);
    }

    #[test]
    fn run_emits_the_snapshot_and_a_result_asks_for_a_save() {
        let mut state = state_with_editor(true);
        state.open_notes();
        // An unsaved edit rides the request, like polish.
        state.editor.as_mut().unwrap().buffers[1] = "We decided to ship on Monday.".to_string();
        let effects = state.start_notes();
        match effects.as_slice() {
            [Effect::GenerateNotes {
                transcript, preset, ..
            }] => {
                assert_eq!(*preset, NotePreset::Minutes);
                assert_eq!(
                    transcript.segments[1].text, "We decided to ship on Monday.",
                    "unsaved edits are summarized"
                );
            }
            other => panic!("expected a notes request, got {other:?}"),
        }

        let outcome = NotesOutcome {
            preset: NotePreset::Minutes,
            body_md: "# Minutes\n\n- Ship Friday [00:12]\n".to_string(),
            citations_checked: 1,
            citations_removed: 0,
            breakdown: Default::default(),
            parts: 1,
            model: "mock-model".to_string(),
        };
        let effects = state.on_notes_done(Ok(outcome));
        match effects.as_slice() {
            [Effect::SaveNote { note }] => {
                assert_eq!(note.preset, NotePreset::Minutes);
                assert!(note.body_md.contains("[00:12]"));
            }
            other => panic!("expected one save, got {other:?}"),
        }
        assert_eq!(
            state.notes.result_line().as_deref(),
            Some("1 citations checked · 0 removed")
        );
        assert_eq!(
            state.note_copy_text().as_deref(),
            Some("# Minutes\n\n- Ship Friday [00:12]\n")
        );
        let (name, markdown) = state.note_export_request().expect("a result to export");
        assert!(name.ends_with("— Minutes.md"), "unexpected name: {name}");
        assert_eq!(markdown, "# Minutes\n\n- Ship Friday [00:12]\n");
    }

    #[test]
    fn a_failed_run_keeps_the_stored_result_and_names_the_error() {
        let mut state = state_with_editor(true);
        state.open_notes();
        state.notes.adopt_stored(vec![StoredNote {
            preset: NotePreset::Minutes,
            body_md: "# Minutes\n\n- old [00:00]\n".to_string(),
            citations_checked: 4,
            citations_removed: 0,
            breakdown: Default::default(),
            model: "old".to_string(),
            created_at: "2026-01-01T00:00:00+00:00".to_string(),
        }]);
        state.start_notes();
        let effects = state.on_notes_done(Err(
            "Polish backend error: HTTP 500: model exploded".to_string()
        ));
        assert!(effects.is_empty());
        assert_eq!(
            state.notes.result_line().as_deref(),
            Some("4 citations checked · 0 removed")
        );
        let (_, message) = state.notes.failure().expect("named");
        assert!(message.contains("HTTP 500"));
        assert!(message.contains("model exploded"));
        let status = state.status.as_ref().unwrap();
        assert!(status.error);
        assert!(status.text.contains("HTTP 500"));
    }

    // ---- full round trip through the runtime and the store -------------------

    fn read_request(stream: &mut TcpStream) -> String {
        let mut data = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        let mut expected: Option<(usize, usize)> = None;
        loop {
            if expected.is_none() {
                if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
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

    fn respond(stream: &mut TcpStream, status: &str, body: &str) {
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(resp.as_bytes()).ok();
    }

    fn chat_body(content: &str) -> String {
        serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": content } }]
        })
        .to_string()
    }

    fn spawn_mock<F>(expected: usize, handler: F) -> (String, Arc<Mutex<Vec<String>>>)
    where
        F: Fn(usize, &str) -> String + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        thread::spawn(move || {
            for index in 0..expected {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let request = read_request(&mut stream);
                let content = handler(index, &request);
                record.lock().unwrap().push(request);
                respond(&mut stream, "200 OK", &chat_body(&content));
            }
        });
        (format!("http://{addr}/v1"), seen)
    }

    fn with_notes_env<T>(tag: &str, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root =
            std::env::temp_dir().join(format!("suramak-ui-notes-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("temp root");
        let previous_xdg = std::env::var_os("XDG_DATA_HOME");
        let previous_key = std::env::var_os("SURAMAK_OPENAI_API_KEY");
        std::env::set_var("XDG_DATA_HOME", &root);
        std::env::set_var("SURAMAK_OPENAI_API_KEY", "test-key");
        let out = f();
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

    fn write_settings(base_url: &str) {
        let path = paths::settings_path().expect("settings path");
        std::fs::create_dir_all(path.parent().unwrap()).expect("data dir");
        Settings {
            polish: Some(PolishSettings {
                base_url: base_url.to_string(),
                model: "mock-model".to_string(),
                send_audio: false,
            }),
            ..Settings::default()
        }
        .save(&path)
        .expect("save settings");
    }

    fn wait_for_notes(runtime: &mut crate::runtime::Runtime) -> Result<NotesOutcome, String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = runtime.notes_status() {
                return result;
            }
            assert!(Instant::now() < deadline, "the notes run did not finish");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn run_one_preset(preset: NotePreset, reply: &'static str) {
        let (url, _seen) = spawn_mock(1, |_index, _request| reply.to_string());
        with_notes_env("round-trip", || {
            write_settings(&url);
            let mut runtime = crate::runtime::Runtime::new(egui::Context::default());
            let mut state = AppState::new();
            state.polish_configured = true;
            let transcript = transcript();
            let id = transcript.id;
            state.on_transcript_loaded(transcript.clone());
            state.open_notes();
            state.notes.select(preset);
            Store::open(&paths::db_path().unwrap())
                .unwrap()
                .insert_transcript(&transcript)
                .unwrap();

            let effects = state.start_notes();
            runtime.apply(effects, &mut state);
            let outcome = wait_for_notes(&mut runtime).expect("the mock answers");
            let effects = state.on_notes_done(Ok(outcome));
            runtime.apply(effects, &mut state);

            let stored = Store::open(&paths::db_path().unwrap())
                .unwrap()
                .note(id, preset)
                .unwrap()
                .expect("the note is stored");
            assert_eq!(stored.preset, preset);
            assert!(
                !stored.body_md.contains("[13:00]"),
                "fabricated time stripped"
            );
            assert!(stored.body_md.contains("[00:12]"));
            assert_eq!(
                state.notes.current().map(|note| note.body_md.clone()),
                Some(stored.body_md)
            );
        });
    }

    #[test]
    fn a_minutes_run_round_trips_through_the_runtime_and_store() {
        run_one_preset(
            NotePreset::Minutes,
            "## Decisions\n\n- Ship Friday [00:12]\n- Budget approved [13:00]\n",
        );
    }

    #[test]
    fn an_action_items_run_round_trips_through_the_runtime_and_store() {
        run_one_preset(
            NotePreset::ActionItems,
            "## Actions\n\n- **Budi** — own the migration [01:05]\n- **Unassigned** — book the room [00:12]\n- **Someone** — invented [13:00]\n",
        );
    }

    #[test]
    fn a_bilingual_run_round_trips_through_the_runtime_and_store() {
        run_one_preset(
            NotePreset::BilingualBrief,
            "## Ringkasan\n\n- Kirim Jumat [00:12]\n- Anggaran [13:00]\n",
        );
    }

    #[test]
    fn export_note_writes_the_exact_stored_markdown_to_disk() {
        with_notes_env("export", || {
            let mut runtime = crate::runtime::Runtime::new(egui::Context::default());
            let mut state = AppState::new();
            state.polish_configured = true;
            state.on_transcript_loaded(transcript());
            state.open_notes();
            state.notes.adopt_stored(vec![StoredNote {
                preset: NotePreset::Minutes,
                body_md: "# Minutes\n\n- exact [00:12]\n".to_string(),
                citations_checked: 1,
                citations_removed: 0,
                breakdown: Default::default(),
                model: "mock-model".to_string(),
                created_at: "2026-01-01T00:00:00+00:00".to_string(),
            }]);
            state.notes.select(NotePreset::Minutes);
            let (suggested, markdown) = state.note_export_request().expect("a result");
            let dest = std::env::temp_dir().join(format!(
                "suramak-note-export-{}-{}.md",
                std::process::id(),
                suggested.len()
            ));
            runtime.apply(
                vec![Effect::ExportNote {
                    dest: dest.clone(),
                    markdown: markdown.clone(),
                }],
                &mut state,
            );
            assert_eq!(std::fs::read_to_string(&dest).unwrap(), markdown);
            assert_eq!(markdown, "# Minutes\n\n- exact [00:12]\n");
            assert!(state
                .status
                .as_ref()
                .is_some_and(|status| status.text.starts_with("Saved ")));
            std::fs::remove_file(&dest).ok();
        });
    }
}
