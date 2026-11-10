//! The compact transcript editor: read, tap-to-select, inline edit, search,
//! play/pause and the export sheet entry.
//!
//! Semantics are the desktop editor's, at phone scale: one segment is selected,
//! an inline field edits exactly that segment, Save persists the drafted rows
//! through the host (the same `update_segment_text` path), search filters the
//! visible row set, and Export asks for one of the five formats through
//! `ACTION_CREATE_DOCUMENT`. Playback is transport state here; the host owns the
//! audio engine (the phone's M13F/M8 edges).

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Layout, Rect, ScrollArea,
    Sense, Stroke, StrokeKind, Ui,
};

use crate::deck::motion::Motion;
use crate::deck::theme::Tokens;

use super::chrome::{self, GUTTER, MIN_TOUCH};
use super::model::{clock, CompactAction, CompactApp, CompactExportFormat};

/// One segment row's height (read mode).
const SEGMENT_ROW: f32 = 66.0;

pub fn ui(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut CompactApp,
    screen: Rect,
) -> Vec<CompactAction> {
    let mut actions = Vec::new();
    let bands = chrome::bands(
        screen,
        state.insets.top,
        state.insets.bottom,
        Some(chrome::BOTTOM_BAR),
    );
    chrome::background(ui, tokens, screen);
    // Take the editor for the frame: several branches also mutate the app (back,
    // the export sheet, a save acknowledgement), and two mutable borrows of
    // `state` cannot coexist. It is put back before returning unless navigation
    // left the editor.
    let Some(mut editor) = state.editor.take() else {
        return actions;
    };
    let now = ui.input(|input| input.time);
    let (back, _) = chrome::app_bar(
        ui,
        tokens,
        motion,
        bands.app_bar,
        &editor.transcript.title,
        true,
        None,
    );
    if back {
        state.back(now);
    }

    // --- meta line ---------------------------------------------------------
    let meta_rect = Rect::from_min_size(
        pos2(bands.content.left() + GUTTER, bands.content.top() + 6.0),
        vec2(bands.content.width() - GUTTER * 2.0, 22.0),
    );
    let duration = editor
        .transcript
        .segments
        .iter()
        .map(|segment| segment.end_ms)
        .max()
        .unwrap_or(0);
    let language = editor
        .transcript
        .language
        .clone()
        .unwrap_or_else(|| "auto".to_string());
    ui.painter().text(
        pos2(meta_rect.left(), meta_rect.center().y),
        Align2::LEFT_CENTER,
        format!(
            "{} · {} · {}",
            clock(duration),
            super::model::count_noun(editor.transcript.segments.len(), "segment"),
            language.to_uppercase()
        ),
        FontId::proportional(12.5),
        tokens.ink_soft,
    );
    if editor.has_unsaved() {
        ui.painter().text(
            pos2(meta_rect.right(), meta_rect.center().y),
            Align2::RIGHT_CENTER,
            "unsaved",
            FontId::proportional(12.0),
            tokens.record_red_text,
        );
    }

    // --- search / edit strip ----------------------------------------------
    let mut strip_top = meta_rect.bottom() + 6.0;
    if editor.search_open {
        let search_rect = Rect::from_min_size(
            pos2(bands.content.left() + GUTTER, strip_top),
            vec2(bands.content.width() - GUTTER * 2.0 - 56.0, MIN_TOUCH),
        );
        chrome::search_field(
            ui,
            tokens,
            search_rect,
            &mut editor.search,
            "Find in transcript",
        );
        let close = Rect::from_min_size(
            pos2(search_rect.right() + 8.0, search_rect.top()),
            vec2(48.0, MIN_TOUCH),
        );
        if ui
            .interact(close, ui.id().with("editor-search-close"), Sense::click())
            .clicked()
        {
            editor.search_open = false;
            editor.search.clear();
        }
        strip_top = search_rect.bottom() + 8.0;
    }

    // --- segment list ------------------------------------------------------
    let list_rect = Rect::from_min_max(
        pos2(bands.content.left(), strip_top),
        bands.content.right_bottom(),
    );
    let matches = editor.matches();
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(list_rect)
            .layout(Layout::top_down(Align::Min)),
    );
    let mut scroll = ScrollArea::vertical().auto_shrink([false, false]);
    // The row being edited must stay visible when the keyboard opens.
    if let Some((idx, _)) = &editor.edit_buffer {
        let position = matches
            .iter()
            .position(|index| editor.transcript.segments[*index].idx == *idx)
            .unwrap_or(0);
        scroll = scroll.vertical_scroll_offset((position as f32 * SEGMENT_ROW).max(0.0));
    }
    let mut row_actions: Vec<CompactAction> = Vec::new();
    scroll.show(&mut child, |ui| {
        if matches.is_empty() {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("No matching lines")
                        .size(14.0)
                        .color(tokens.ink_soft),
                );
            });
            return;
        }
        for index in matches {
            let segment = editor.transcript.segments[index].clone();
            let selected = editor.selected == Some(segment.idx);
            let editing = editor
                .edit_buffer
                .as_ref()
                .is_some_and(|(idx, _)| *idx == segment.idx);
            let row_h = if editing { 150.0 } else { SEGMENT_ROW };
            let (rect, _) = ui.allocate_exact_size(vec2(list_rect.width(), row_h), Sense::hover());
            let response =
                ui.interact(rect, ui.id().with(("segment", segment.idx)), Sense::click());
            let fill = if selected {
                tokens.accent_tint
            } else if response.hovered() {
                tokens.surface_hover
            } else {
                Color32::TRANSPARENT
            };
            ui.painter()
                .rect_filled(rect, CornerRadius::same(tokens.radius.row), fill);
            // Time gutter.
            ui.painter().text(
                pos2(rect.left() + GUTTER, rect.top() + 18.0),
                Align2::LEFT_CENTER,
                clock(segment.start_ms),
                FontId::new(12.0, FontFamily::Monospace),
                tokens.ink_soft,
            );
            if editing {
                let edit_rect = Rect::from_min_size(
                    pos2(rect.left() + 62.0, rect.top() + 34.0),
                    vec2(rect.width() - 62.0 - GUTTER, 74.0),
                );
                let idx = editor
                    .edit_buffer
                    .as_ref()
                    .map(|(idx, _)| *idx)
                    .expect("editing row");
                {
                    let buffer = &mut editor.edit_buffer.as_mut().expect("editing row").1;
                    let mut field = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(edit_rect)
                            .layout(Layout::top_down(Align::Min)),
                    );
                    field.add(
                        egui::TextEdit::multiline(buffer)
                            .desired_rows(3)
                            .desired_width(edit_rect.width())
                            .frame(false)
                            .font(FontId::proportional(15.0)),
                    );
                }
                let save = Rect::from_min_size(
                    pos2(edit_rect.right() - 152.0, rect.bottom() - 44.0),
                    vec2(72.0, 34.0),
                );
                let cancel = Rect::from_min_size(
                    pos2(edit_rect.right() - 72.0, rect.bottom() - 44.0),
                    vec2(72.0, 34.0),
                );
                if ui
                    .interact(save, ui.id().with(("seg-save", idx)), Sense::click())
                    .clicked()
                {
                    editor.commit_edit_buffer();
                    for edit in editor.pending_edits() {
                        row_actions.push(CompactAction::SaveSegment(edit));
                    }
                }
                if ui
                    .interact(cancel, ui.id().with(("seg-cancel", idx)), Sense::click())
                    .clicked()
                {
                    editor.cancel_edit();
                }
                for (rect, label, primary) in [(save, "Save", true), (cancel, "Cancel", false)] {
                    ui.painter().rect_filled(
                        rect,
                        CornerRadius::same(tokens.radius.control),
                        if primary {
                            tokens.accent
                        } else {
                            tokens.control_fill
                        },
                    );
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        label,
                        FontId::proportional(13.0),
                        if primary {
                            tokens.on_accent
                        } else {
                            tokens.ink
                        },
                    );
                }
            } else {
                let text = editor.text(&segment).to_string();
                let galley = ui.painter().layout(
                    text,
                    FontId::proportional(15.5),
                    tokens.ink,
                    (rect.width() - 62.0 - GUTTER - 24.0).max(60.0),
                );
                ui.painter().galley(
                    pos2(rect.left() + 62.0, rect.top() + 12.0),
                    galley,
                    tokens.ink,
                );
                if editor.dirty.contains(&segment.idx) {
                    ui.painter().circle_filled(
                        pos2(rect.left() + 14.0, rect.top() + 40.0),
                        3.0,
                        tokens.accent,
                    );
                }
                // The speaker mark: seek to this line.
                let speaker = Rect::from_center_size(
                    pos2(rect.right() - GUTTER - 12.0, rect.center().y),
                    vec2(40.0, 40.0),
                );
                ui.painter().add(egui::Shape::convex_polygon(
                    vec![
                        pos2(speaker.center().x - 5.0, speaker.center().y - 8.0),
                        pos2(speaker.center().x - 5.0, speaker.center().y + 8.0),
                        pos2(speaker.center().x + 8.0, speaker.center().y),
                    ],
                    tokens.ink_soft,
                    Stroke::NONE,
                ));
                if response.clicked() {
                    if editor.selected == Some(segment.idx) {
                        editor.begin_edit(segment.idx);
                    } else {
                        editor.select(segment.idx);
                    }
                }
            }
            ui.painter().line_segment(
                [
                    pos2(rect.left() + GUTTER, rect.bottom() - 0.5),
                    pos2(rect.right(), rect.bottom() - 0.5),
                ],
                Stroke::new(1.0_f32, tokens.hairline),
            );
        }
    });
    actions.extend(row_actions);

    // --- bottom transport --------------------------------------------------
    let bar = bands.bottom_bar;
    ui.painter()
        .rect_filled(bar, CornerRadius::ZERO, tokens.panel);
    ui.painter().line_segment(
        [bar.left_top(), pos2(bar.right(), bar.top())],
        Stroke::new(1.0_f32, tokens.hairline),
    );
    // Position, not just numbers: a 3 pt track with the played portion in the
    // accent. It is a read-out (the transcript rows are the seek targets), so it
    // never steals the row taps.
    let track = Rect::from_min_size(
        pos2(bar.left() + GUTTER, bar.top() + 6.0),
        vec2(bar.width() - GUTTER * 2.0, 3.0),
    );
    ui.painter()
        .rect_filled(track, CornerRadius::same(2), tokens.control_fill);
    let played = if duration > 0 {
        (editor.playhead_ms as f32 / duration as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if played > 0.0 {
        ui.painter().rect_filled(
            Rect::from_min_size(track.min, vec2(track.width() * played, 3.0)),
            CornerRadius::same(2),
            tokens.accent,
        );
    }
    let play_rect = Rect::from_center_size(
        pos2(bar.left() + GUTTER + 28.0, bar.center().y),
        vec2(56.0, 56.0),
    );
    let play = ui.interact(play_rect, ui.id().with("editor-play"), Sense::click());
    ui.painter()
        .circle_filled(play_rect.center(), 26.0, tokens.accent);
    if editor.playing {
        let c = play_rect.center();
        ui.painter().rect_filled(
            Rect::from_center_size(c, vec2(13.0, 16.0)),
            CornerRadius::same(2),
            tokens.on_accent,
        );
    } else {
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                pos2(play_rect.center().x - 5.0, play_rect.center().y - 8.0),
                pos2(play_rect.center().x - 5.0, play_rect.center().y + 8.0),
                pos2(play_rect.center().x + 9.0, play_rect.center().y),
            ],
            tokens.on_accent,
            Stroke::NONE,
        ));
    }
    if play.clicked() {
        editor.playing = !editor.playing;
    }
    ui.painter().text(
        pos2(bar.left() + GUTTER + 64.0, bar.center().y),
        Align2::LEFT_CENTER,
        format!("{} / {}", clock(editor.playhead_ms), clock(duration)),
        FontId::new(12.5, FontFamily::Monospace),
        tokens.ink_soft,
    );
    let search_btn = Rect::from_center_size(
        pos2(bar.right() - GUTTER - 104.0, bar.center().y),
        vec2(72.0, 48.0),
    );
    if ui
        .interact(search_btn, ui.id().with("editor-search"), Sense::click())
        .clicked()
    {
        editor.search_open = !editor.search_open;
    }
    ui.painter().text(
        search_btn.center(),
        Align2::CENTER_CENTER,
        "Find",
        FontId::proportional(14.0),
        tokens.ink,
    );
    let export_btn = Rect::from_center_size(
        pos2(bar.right() - GUTTER - 38.0, bar.center().y),
        vec2(76.0, 48.0),
    );
    if chrome::action_button(
        ui,
        tokens,
        motion,
        export_btn,
        "editor-export",
        "Export",
        true,
    )
    .clicked()
    {
        state.sheet = Some(super::model::Sheet::Export);
    }

    if state.nav.current() == super::model::CompactRoute::Editor {
        state.editor = Some(editor);
    }
    actions
}

/// The export sheet's body: five format chips and the write button. Drawn by
/// the parent over the scrim so back dismisses it first.
pub fn export_sheet(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut CompactApp,
    body: Rect,
) -> Option<CompactAction> {
    let editor = state.editor.as_mut()?;
    ui.painter().text(
        pos2(body.center().x, body.top() + 34.0),
        Align2::CENTER_CENTER,
        "Export transcript",
        FontId::new(
            19.0,
            FontFamily::Name(crate::deck::theme::SERIF_FAMILY.into()),
        ),
        tokens.ink,
    );
    ui.painter().text(
        pos2(body.center().x, body.top() + 58.0),
        Align2::CENTER_CENTER,
        "The file is written where you choose.",
        FontId::proportional(12.5),
        tokens.ink_soft,
    );
    let chip_w = (body.width() - GUTTER * 2.0 - 8.0 * 2.0) / 3.0;
    let chip_h = 44.0;
    for (index, format) in CompactExportFormat::ALL.iter().enumerate() {
        let column = index % 3;
        let row = index / 3;
        let rect = Rect::from_min_size(
            pos2(
                body.left() + GUTTER + column as f32 * (chip_w + 8.0),
                body.top() + 84.0 + row as f32 * (chip_h + 8.0),
            ),
            vec2(chip_w, chip_h),
        );
        let response = ui.interact(
            rect,
            ui.id().with(("export-format", format.label())),
            Sense::click(),
        );
        let selected = editor.format == *format;
        let fill = if selected {
            tokens.accent_tint
        } else if response.hovered() {
            tokens.surface_hover
        } else {
            tokens.control_fill
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(tokens.radius.control),
            fill,
            Stroke::new(
                1.0_f32,
                if selected {
                    tokens.accent
                } else {
                    tokens.hairline
                },
            ),
            StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            format.label(),
            FontId::proportional(13.5),
            if selected { tokens.accent } else { tokens.ink },
        );
        if response.clicked() {
            editor.format = *format;
        }
    }
    let submit = Rect::from_min_size(
        pos2(body.left() + GUTTER, body.bottom() - 72.0),
        vec2(body.width() - GUTTER * 2.0, 52.0),
    );
    let mut action = None;
    if chrome::action_button(
        ui,
        tokens,
        motion,
        submit,
        "export-submit",
        &format!("Export {}", editor.format.label()),
        true,
    )
    .clicked()
    {
        action = Some(editor.export_request());
    }
    action
}

/// The quit confirm sheet (Settings' Quit action). Returns `Some(true)` when
/// Quit is confirmed, `Some(false)` when cancelled.
pub fn quit_sheet(ui: &mut Ui, tokens: &Tokens, motion: &Motion, body: Rect) -> Option<bool> {
    ui.painter().text(
        pos2(body.center().x, body.top() + 40.0),
        Align2::CENTER_CENTER,
        "Quit Suramak?",
        FontId::new(
            21.0,
            FontFamily::Name(crate::deck::theme::SERIF_FAMILY.into()),
        ),
        tokens.ink,
    );
    ui.painter().text(
        pos2(body.center().x, body.top() + 70.0),
        Align2::CENTER_CENTER,
        "Any recording is saved and handed to the inbox first;\nrunning transcription pauses, services stop, then the app exits.",
        FontId::proportional(13.0),
        tokens.ink_soft,
    );
    let width = body.width() - GUTTER * 2.0;
    let quit = Rect::from_min_size(
        pos2(body.left() + GUTTER, body.bottom() - 72.0),
        vec2(width, 52.0),
    );
    let cancel = Rect::from_min_size(
        pos2(body.left() + GUTTER, body.bottom() - 132.0),
        vec2(width, 52.0),
    );
    let mut answer = None;
    if chrome::action_button(ui, tokens, motion, quit, "quit-confirm", "Quit", true).clicked() {
        answer = Some(true);
    }
    if chrome::action_button(ui, tokens, motion, cancel, "quit-cancel", "Cancel", false).clicked() {
        answer = Some(false);
    }
    answer
}
