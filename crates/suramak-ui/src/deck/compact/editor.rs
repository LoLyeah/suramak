//! The compact transcript editor: read, tap-to-select, inline edit, search,
//! play/pause and the export sheet.
//!
//! Semantics are the desktop editor's, at phone scale: one segment is selected,
//! an inline field edits exactly that segment, Save persists the drafted rows
//! through the host (the same `update_segment_text` path), search filters the
//! visible row set, and Export opens the sheet that writes through
//! `ACTION_CREATE_DOCUMENT` (or hands the same one file to `ACTION_SEND`).
//! Playback is transport state here; the host owns the audio engine (the
//! phone's M13F/M8 edges).

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Layout, Rect, ScrollArea,
    Sense, Stroke, StrokeKind, Ui, WidgetType,
};

use crate::deck::motion::{self, Motion};
use crate::deck::theme::Tokens;

use super::a11y;
use super::chrome::{self, GUTTER, MIN_TOUCH};
use super::model::{
    clock, CompactAction, CompactApp, CompactExportFormat, ExportPhase, ExportSession,
};

/// One segment row's height (read mode).
const SEGMENT_ROW: f32 = 66.0;
/// The karaoke chips' band under a row (M15.6 compact behaviour).
const WORD_STRIP: f32 = 40.0;

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
    let mut left_editor = false;
    if back {
        state.back(now);
        left_editor = state.nav.current() != super::model::CompactRoute::Editor;
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
        a11y::describe(ui, search_rect, "Find in transcript");
        let close = Rect::from_min_size(
            pos2(search_rect.right() + 8.0, search_rect.top()),
            vec2(48.0, MIN_TOUCH),
        );
        if a11y::touch(
            ui,
            tokens,
            close,
            "editor-search-close",
            WidgetType::Button,
            "Close search",
            true,
        )
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
            // M15.6: the chips ride under the row the reader is on — the
            // selected row, or the row the transport is reading while playing.
            let words = editor.words_for(segment.idx).to_vec();
            let show_words = !editing && !words.is_empty() && (editor.playing || selected);
            let row_h = if editing {
                150.0
            } else if show_words {
                SEGMENT_ROW + WORD_STRIP
            } else {
                SEGMENT_ROW
            };
            let (rect, _) = ui.allocate_exact_size(vec2(list_rect.width(), row_h), Sense::hover());
            let label = format!(
                "Segment {} · {}",
                segment.idx + 1,
                truncate(&editor.text(&segment), 96)
            );
            let response = a11y::touch(
                ui,
                tokens,
                rect,
                ("segment", segment.idx),
                WidgetType::Button,
                &label,
                true,
            );
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
                if a11y::touch(
                    ui,
                    tokens,
                    save,
                    ("seg-save", idx),
                    WidgetType::Button,
                    "Save segment",
                    true,
                )
                .clicked()
                {
                    editor.commit_edit_buffer();
                    for edit in editor.pending_edits() {
                        row_actions.push(CompactAction::SaveSegment(edit));
                    }
                }
                if a11y::touch(
                    ui,
                    tokens,
                    cancel,
                    ("seg-cancel", idx),
                    WidgetType::Button,
                    "Cancel edit",
                    true,
                )
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
                        row_actions.push(CompactAction::Seek {
                            position_ms: editor.playhead_ms,
                        });
                    }
                }
            }
            // M15.6: the word chips under the selected/playing row. Each chip
            // is a seek target; the word the transport is inside carries the
            // accent, its underline springs in (Reduce Motion snaps it in — the
            // state stays). Same rule as the desktop's karaoke strip.
            if show_words {
                word_strip(
                    ui,
                    tokens,
                    motion,
                    &mut editor,
                    &segment,
                    rect,
                    &words,
                    &mut row_actions,
                );
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
    // never steals the row taps. The drawn playhead eases to a seek and follows
    // playback exactly (the Deck's seek rule); a jump asks for frames only for
    // the ease, never as a constant redraw.
    let track = Rect::from_min_size(
        pos2(bar.left() + GUTTER, bar.top() + 6.0),
        vec2(bar.width() - GUTTER * 2.0, 3.0),
    );
    ui.painter()
        .rect_filled(track, CornerRadius::same(2), tokens.control_fill);
    let drawn_ms = motion.seek_ease(
        ui.ctx(),
        ui.id().with("editor-playhead"),
        editor.playhead_ms as f32,
        motion::BASE,
    );
    let played = if duration > 0 {
        (drawn_ms / duration as f32).clamp(0.0, 1.0)
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
    let play = a11y::touch(
        ui,
        tokens,
        play_rect,
        "editor-play",
        WidgetType::Button,
        if editor.playing { "Pause" } else { "Play" },
        true,
    );
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
        let playing = editor.playing;
        actions.push(CompactAction::Playback { playing });
    }
    let time_rect = Rect::from_min_size(
        pos2(bar.left() + GUTTER + 64.0, bar.center().y - 10.0),
        vec2(96.0, 20.0),
    );
    a11y::describe(
        ui,
        time_rect,
        &format!("{} of {}", clock(editor.playhead_ms), clock(duration)),
    );
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
    if a11y::touch(
        ui,
        tokens,
        search_btn,
        "editor-search",
        WidgetType::Button,
        "Find",
        true,
    )
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
        state.open_export_sheet(now);
    }
    a11y::record_button(ui, export_btn, "Export", true);

    if !left_editor {
        state.editor = Some(editor);
    }
    actions
}

/// The M15.6 karaoke chips for one row: wrapped word targets under the text,
/// the current word carrying the accent tint and a spring-grown underline.
/// A tap parks the playhead at the word's estimated start and emits a Seek.
#[allow(clippy::too_many_arguments)]
fn word_strip(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    editor: &mut super::model::CompactEditor,
    segment: &suramak_core::transcript::Segment,
    rect: Rect,
    words: &[suramak_core::transcript::WordTiming],
    actions: &mut Vec<CompactAction>,
) {
    let current = editor.current_word(segment.idx);
    let playing = editor.playing;
    let mut x = rect.left() + 62.0;
    let mut y = rect.top() + SEGMENT_ROW + 4.0;
    let max_x = rect.right() - GUTTER - 8.0;
    let line_start = x;
    for (word_index, word) in words.iter().enumerate() {
        let galley = ui.painter().layout_no_wrap(
            word.text.clone(),
            FontId::proportional(13.0),
            tokens.ink_soft,
        );
        let width = galley.size().x + 14.0;
        if x + width > max_x && x > line_start {
            x = line_start;
            y += 22.0;
        }
        let chip = Rect::from_min_size(pos2(x, y), vec2(width, 18.0));
        let id = egui::Id::new(("compact-word", segment.idx, word_index));
        let highlight =
            motion.highlight_spring(ui.ctx(), id, playing && current == Some(word_index));
        let response = a11y::touch(
            ui,
            tokens,
            chip,
            ("word", segment.idx, word_index),
            WidgetType::Button,
            &format!("Word {} · {}", word_index + 1, word.text),
            true,
        );
        if highlight > 0.01 {
            ui.painter().rect_filled(
                chip,
                CornerRadius::same(4),
                motion::tint_over(egui::Color32::TRANSPARENT, tokens.accent_tint, highlight),
            );
            let bar = Rect::from_center_size(
                pos2(chip.center().x, chip.bottom() + 0.5),
                vec2(chip.width() * highlight, 2.0),
            );
            ui.painter().rect_filled(
                bar,
                CornerRadius::same(1),
                tokens.accent.gamma_multiply(highlight),
            );
        }
        ui.painter().galley(
            pos2(chip.left() + 7.0, chip.top() + 1.0),
            galley,
            motion::lerp_color(tokens.ink_soft, tokens.ink, highlight),
        );
        if response.clicked() {
            editor.playhead_ms = word.start_ms;
            editor.selected = Some(segment.idx);
            actions.push(CompactAction::Seek {
                position_ms: word.start_ms,
            });
        }
        x += width + 4.0;
    }
}

/// The export sheet's body: the format chips, the bilingual switch and the two
/// ways out (picker or one-file share), then the picker/write result.
///
/// The sheet is the disclosure: the picker line names `ACTION_CREATE_DOCUMENT`,
/// the secondary action names `ACTION_SEND`, and the note says both write the
/// same bytes the desktop writes. No action happens on open.
pub fn export_sheet(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut CompactApp,
    body: Rect,
) -> Vec<CompactAction> {
    let now = ui.input(|input| input.time);
    let mut actions = Vec::new();
    ui.painter().text(
        pos2(body.center().x, body.top() + 30.0),
        Align2::CENTER_CENTER,
        "Export transcript",
        FontId::new(
            19.0,
            FontFamily::Name(crate::deck::theme::SERIF_FAMILY.into()),
        ),
        tokens.ink,
    );
    let Some(editor) = state.editor.as_ref() else {
        return actions;
    };
    // Copy what the sheet needs before any branch mutates the app: the format
    // chips and the bilingual gate are decisions about the editor, not a live
    // borrow of it.
    let current_format = editor.format;
    let has_translation = editor.has_translation;

    if let Some(session) = state.export_session.clone() {
        export_result(ui, tokens, motion, state, body, &session, now, &mut actions);
        return actions;
    }

    ui.painter().text(
        pos2(body.center().x, body.top() + 54.0),
        Align2::CENTER_CENTER,
        "Same bytes as the desktop. Choose where it goes.",
        FontId::proportional(12.5),
        tokens.ink_soft,
    );
    let chip_w = (body.width() - GUTTER * 2.0 - 8.0 * 2.0) / 3.0;
    let chip_h = 42.0;
    for (index, format) in CompactExportFormat::ALL.iter().enumerate() {
        let column = index % 3;
        let row = index / 3;
        let rect = Rect::from_min_size(
            pos2(
                body.left() + GUTTER + column as f32 * (chip_w + 8.0),
                body.top() + 76.0 + row as f32 * (chip_h + 8.0),
            ),
            vec2(chip_w, chip_h),
        );
        let selected = current_format == *format;
        let response = a11y::touch(
            ui,
            tokens,
            rect,
            ("export-format", format.label()),
            WidgetType::RadioButton,
            &format!("Export format: {}", format.label()),
            true,
        );
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
            if let Some(editor) = state.editor.as_mut() {
                editor.format = *format;
            }
        }
    }

    // The bilingual switch: only rendered for the stacked formats and only when
    // the transcript actually has a translation to stack.
    if current_format.supports_bilingual() {
        let toggle_rect = Rect::from_min_size(
            pos2(body.left() + GUTTER, body.top() + 176.0),
            vec2(body.width() - GUTTER * 2.0, 40.0),
        );
        let response = a11y::touch(
            ui,
            tokens,
            toggle_rect,
            "export-bilingual",
            WidgetType::Checkbox,
            "Include the translation",
            has_translation,
        );
        let box_rect = Rect::from_min_size(
            pos2(toggle_rect.left() + 2.0, toggle_rect.center().y - 10.0),
            vec2(20.0, 20.0),
        );
        ui.painter().rect(
            box_rect,
            CornerRadius::same(5),
            if state.export_bilingual && has_translation {
                tokens.accent
            } else {
                Color32::TRANSPARENT
            },
            Stroke::new(1.0_f32, tokens.hairline_strong),
            StrokeKind::Inside,
        );
        if state.export_bilingual && has_translation {
            ui.painter().add(egui::Shape::line(
                vec![
                    pos2(box_rect.left() + 5.0, box_rect.center().y),
                    pos2(box_rect.center().x, box_rect.bottom() - 5.0),
                    pos2(box_rect.right() - 4.0, box_rect.top() + 5.0),
                ],
                Stroke::new(2.0_f32, tokens.on_accent),
            ));
        }
        ui.painter().text(
            pos2(box_rect.right() + 10.0, toggle_rect.center().y),
            Align2::LEFT_CENTER,
            if has_translation {
                "Include the translation (stacked)"
            } else {
                "No translation to include"
            },
            FontId::proportional(13.0),
            if has_translation {
                tokens.ink
            } else {
                tokens.ink_faint
            },
        );
        if response.clicked() && has_translation {
            state.export_bilingual = !state.export_bilingual;
        }
    }

    // Two ways out, both disclosed.
    ui.painter().text(
        pos2(body.left() + GUTTER, body.top() + 232.0),
        Align2::LEFT_CENTER,
        "Picker: the system file chooser writes to the frame you pick.",
        FontId::proportional(11.5),
        tokens.ink_faint,
    );
    ui.painter().text(
        pos2(body.left() + GUTTER, body.top() + 250.0),
        Align2::LEFT_CENTER,
        "Share: hands this one file to another app (ACTION_SEND).",
        FontId::proportional(11.5),
        tokens.ink_faint,
    );
    let share = Rect::from_min_size(
        pos2(body.left() + GUTTER, body.bottom() - 128.0),
        vec2(body.width() - GUTTER * 2.0, 46.0),
    );
    if chrome::action_button(
        ui,
        tokens,
        motion,
        share,
        "export-share",
        "Share instead",
        false,
    )
    .clicked()
    {
        if let Some(action) = state.share_export(now) {
            actions.push(action);
        }
    }
    a11y::record_button(ui, share, "Share instead", true);
    let submit = Rect::from_min_size(
        pos2(body.left() + GUTTER, body.bottom() - 72.0),
        vec2(body.width() - GUTTER * 2.0, 52.0),
    );
    if chrome::action_button(
        ui,
        tokens,
        motion,
        submit,
        "export-submit",
        &format!("Export {}", current_format.label()),
        true,
    )
    .clicked()
    {
        if let Some(action) = state.begin_export(now) {
            actions.push(action);
        }
    }
    a11y::record_button(ui, submit, "Export", true);
    actions
}

/// The sheet's working/result body. Drawn instead of the chips once the picker
/// has been asked for a destination — the "picker result path" the screenshot
/// suite fixes in place.
#[allow(clippy::too_many_arguments)]
fn export_result(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut CompactApp,
    body: Rect,
    session: &ExportSession,
    now: f64,
    actions: &mut Vec<CompactAction>,
) {
    let (line, detail, error) = match &session.phase {
        ExportPhase::Choosing => (
            "Waiting for the picker".to_string(),
            "If the system chooser is not in front, press Back to return.".to_string(),
            false,
        ),
        ExportPhase::Writing => (
            "Writing".to_string(),
            format!(
                "{} · the file is verified before this screen says saved",
                session.name
            ),
            false,
        ),
        ExportPhase::Written => (
            "Saved".to_string(),
            match session.bytes {
                Some(bytes) => format!("{} · {}", session.name, file_size_label(bytes)),
                None => session.name.clone(),
            },
            false,
        ),
        ExportPhase::Shared => ("Shared".to_string(), session.name.clone(), false),
        ExportPhase::Failed => (
            "Export failed".to_string(),
            session
                .error
                .clone()
                .unwrap_or_else(|| "the host did not say why".to_string()),
            true,
        ),
    };
    let card = Rect::from_min_size(
        pos2(body.left() + GUTTER, body.top() + 76.0),
        vec2(body.width() - GUTTER * 2.0, 132.0),
    );
    chrome::card(ui, tokens, card);
    let spoken = format!("{line}: {detail}");
    ui.painter().text(
        pos2(card.left() + 16.0, card.top() + 30.0),
        Align2::LEFT_CENTER,
        line,
        FontId::new(
            20.0,
            FontFamily::Name(crate::deck::theme::SERIF_FAMILY.into()),
        ),
        if error {
            tokens.record_red_text
        } else {
            tokens.ink
        },
    );
    ui.painter().text(
        pos2(card.left() + 16.0, card.top() + 60.0),
        Align2::LEFT_CENTER,
        truncate(&detail, 58),
        FontId::proportional(12.5),
        tokens.ink_soft,
    );
    ui.painter().text(
        pos2(card.left() + 16.0, card.top() + 96.0),
        Align2::LEFT_CENTER,
        format!("{} · {}", session.format.label(), session.format.mime()),
        FontId::new(11.5, FontFamily::Monospace),
        tokens.ink_faint,
    );
    a11y::describe(ui, card, &spoken);

    let done = Rect::from_min_size(
        pos2(body.left() + GUTTER, body.bottom() - 72.0),
        vec2(body.width() - GUTTER * 2.0, 52.0),
    );
    if chrome::action_button(ui, tokens, motion, done, "export-done", "Done", true).clicked() {
        state.close_export_sheet(now);
    }
    a11y::record_button(ui, done, "Done", true);
    if matches!(session.phase, ExportPhase::Failed) {
        let retry = Rect::from_min_size(
            pos2(body.left() + GUTTER, body.bottom() - 128.0),
            vec2(body.width() - GUTTER * 2.0, 46.0),
        );
        if chrome::action_button(
            ui,
            tokens,
            motion,
            retry,
            "export-retry",
            "Try again",
            false,
        )
        .clicked()
        {
            if let Some(action) = state.begin_export(now) {
                actions.push(action);
            }
        }
        a11y::record_button(ui, retry, "Try again", true);
    }
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
    a11y::record_button(ui, quit, "Quit", true);
    a11y::record_button(ui, cancel, "Cancel", true);
    answer
}

/// A file size for the result line: bytes/KB below a megabyte (an SRT export
/// is a few kilobytes; "0 MB" would read as an empty file).
fn file_size_label(bytes: u64) -> String {
    if bytes < 1_000 {
        format!("{bytes} B")
    } else if bytes < 1_000_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    }
}

/// Shortens a string to `chars` characters with an ellipsis (labels only).
fn truncate(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(chars.saturating_sub(1)).collect();
    out.push('…');
    out
}
