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
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Rect, ScrollArea, Sense,
    Stroke, StrokeKind, Ui, WidgetType,
};

use suramak_core::diagnostics::{diagnose, Level, SubtitleLimits};

use crate::deck::icons::{self, Icon};
use crate::deck::material::{self, Plane};
use crate::deck::motion::{self, Motion};
use crate::deck::speakers;
use crate::deck::theme::Tokens;
use crate::deck::type_scale;

use super::a11y;
use super::chrome::{self, GUTTER, MIN_TOUCH};
use super::model::{
    clock, CompactAction, CompactApp, CompactExportFormat, ExportPhase, ExportSession,
};

/// One segment's header band (timecode · speaker · advisory) at the row
/// title's 20 pt line height.
const SEGMENT_HEADER: f32 = 20.0;
/// The row's top padding before the header.
const SEGMENT_TOP: f32 = 10.0;
/// The row's bottom padding under the body.
const SEGMENT_BOTTOM: f32 = 10.0;
/// Header → body gap.
const BODY_GAP: f32 = 4.0;
/// The karaoke chips' band under a row (M15.6 compact behaviour).
const WORD_STRIP: f32 = 40.0;
/// The inline editor's fixed body height (field + Save/Cancel).
const EDIT_HEIGHT: f32 = 160.0;
/// M23A: the phone's per-turn rail width. The desktop lane is 8 px; the phone's
/// rail is the slimmer mark beside the row (the plan's 4 px).
const PHONE_RAIL_W: f32 = 4.0;

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
    // The duration is a numeral, so it paints on the tabular grid (a reload
    // from 0:09 to 0:10 cannot shift the line); the words after it are caption.
    let numerals = type_scale::numerals();
    let caption = type_scale::caption();
    let duration_width = type_scale::paint_tabular(
        ui.painter(),
        pos2(meta_rect.left(), meta_rect.center().y),
        Align2::LEFT_CENTER,
        &clock(duration),
        &numerals.font,
        tokens.ink_soft,
    );
    ui.painter().text(
        pos2(meta_rect.left() + duration_width, meta_rect.center().y),
        Align2::LEFT_CENTER,
        format!(
            " · {} · {}",
            super::model::count_noun(editor.transcript.segments.len(), "segment"),
            language.to_uppercase()
        ),
        caption.font.clone(),
        tokens.ink_soft,
    );
    if editor.has_unsaved() {
        ui.painter().text(
            pos2(meta_rect.right(), meta_rect.center().y),
            Align2::RIGHT_CENTER,
            "unsaved",
            caption.font.clone(),
            tokens.record_red_text,
        );
    }

    // --- speaker lane (M16I) ------------------------------------------------
    // ONE 8 px strip above the list, drawn by the same shared renderer the
    // desktop deck uses. It is decorative-but-labelled; the rows carry the
    // badges that name a speaker.
    let mut strip_top = meta_rect.bottom() + 6.0;
    if let Some(speaker_view) = &editor.speakers {
        let lane = Rect::from_min_size(
            pos2(bands.content.left() + GUTTER, meta_rect.bottom() + 2.0),
            vec2(bands.content.width() - GUTTER * 2.0, speakers::LANE_H),
        );
        speakers::lane(
            ui,
            tokens,
            lane,
            &speaker_view.speakers,
            &speaker_view.turns,
            speaker_view.audio_ms,
            speakers::LaneStyle {
                dim: if speaker_view.stale { 0.55 } else { 1.0 },
                fade: 1.0,
            },
        );
        a11y::describe(
            ui,
            lane,
            &format!(
                "Speaker lane: {} turns, {} speakers",
                speaker_view.turns.len(),
                speaker_view.speakers.len()
            ),
        );
        strip_top = lane.bottom() + 4.0;
    }
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
        let close_response = a11y::touch(
            ui,
            tokens,
            close,
            "editor-search-close",
            WidgetType::Button,
            "Close search",
            true,
        );
        icons::paint(
            ui.painter(),
            Rect::from_center_size(close.center(), vec2(16.0, 16.0)),
            Icon::Close,
            16.0,
            tokens.ink_soft,
        );
        if close_response.clicked() {
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
    // M22C: a Library hit lands on its segment (the existing seek path), so
    // the first editor frame scrolls that row into view. The flag is consumed
    // once; later frames leave the reader's own scroll alone.
    if std::mem::take(&mut editor.focus_scroll) {
        if let Some(idx) = editor.focus_segment {
            let position = matches
                .iter()
                .position(|index| editor.transcript.segments[*index].idx == idx)
                .unwrap_or(0);
            scroll = scroll.vertical_scroll_offset(position as f32 * 96.0);
        }
    }
    // The row being edited must stay visible when the keyboard opens. Rows are
    // variable height now (a header band + wrapped body), so this is the
    // designed estimate, not an exact offset.
    if let Some((idx, _)) = &editor.edit_buffer {
        let position = matches
            .iter()
            .position(|index| editor.transcript.segments[*index].idx == *idx)
            .unwrap_or(0);
        scroll = scroll.vertical_scroll_offset(position as f32 * 96.0);
    }
    let mut row_actions: Vec<CompactAction> = Vec::new();
    scroll.show(&mut child, |ui| {
        if matches.is_empty() {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("No matching lines")
                        .size(type_scale::caption().size())
                        .color(tokens.ink_soft),
                );
            });
            return;
        }
        let numerals = type_scale::numerals();
        let body_token = type_scale::body();
        // The speaker header belongs to a turn, not to every row: it is drawn
        // when the visible sequence changes speaker. A filtered list groups by
        // what is on screen, which is what the reader sees.
        let mut previous_speaker: Option<u32> = None;
        for index in matches {
            let segment = editor.transcript.segments[index].clone();
            let speaker_view = editor.speakers.clone();
            let speaker_row = speaker_view.as_ref().and_then(|view| view.row(segment.idx));
            let primary = speaker_row.map(|row| row.primary);
            let show_name = primary.is_some() && primary != previous_speaker;
            previous_speaker = primary;
            let selected = editor.selected == Some(segment.idx);
            let editing = editor
                .edit_buffer
                .as_ref()
                .is_some_and(|(idx, _)| *idx == segment.idx);
            // M15.6: the chips ride under the row the reader is on — the
            // selected row, or the row the transport is reading while playing.
            let words = editor.words_for(segment.idx).to_vec();
            let show_words = !editing
                && !words.is_empty()
                && (selected || (editor.playing && editor.playhead_segment() == Some(segment.idx)));
            let text = editor.text(&segment);
            // The line-length advisory belongs to the segment being worked on,
            // not to every row: the desktop can badge a whole column because
            // its reader is a subtitle editor; on the phone one quiet read-out
            // on the selected row is enough. It is also spoken with the row.
            let advisory = if selected {
                advisory_for(&text, segment.start_ms, segment.end_ms)
            } else {
                None
            };

            // Geometry: the speaker lane keeps its 8 px strip above; inside a
            // row the badge sits in the leading gutter and the header band
            // carries the timecode, the turn's name and the line-length
            // advisory. The body is the type scale's 15.5/24.
            let lead = if speaker_view.is_some() {
                speakers::SLOT_W + 6.0
            } else {
                0.0
            };
            let text_left = list_rect.left() + GUTTER + lead;
            let body_width = (list_rect.right() - GUTTER - 8.0 - text_left).max(80.0);
            let body_galley = {
                let mut job = egui::text::LayoutJob::default();
                job.append(&text, 0.0, body_token.format(tokens.ink));
                job.wrap.max_width = body_width;
                job.wrap.break_anywhere = false;
                job.wrap.overflow_character = Some('…');
                ui.painter().layout_job(job)
            };
            let body_h = body_galley.size().y.max(body_token.line_height);
            let row_h = if editing {
                EDIT_HEIGHT
            } else {
                SEGMENT_TOP
                    + SEGMENT_HEADER
                    + BODY_GAP
                    + body_h
                    + SEGMENT_BOTTOM
                    + if show_words { WORD_STRIP } else { 0.0 }
            };
            let (rect, _) = ui.allocate_exact_size(vec2(list_rect.width(), row_h), Sense::hover());
            let mut label = format!("Segment {} · {}", segment.idx + 1, truncate(&text, 96));
            if let Some(advisory) = &advisory {
                label.push_str(" · ");
                label.push_str(advisory);
            }
            let response = a11y::touch(
                ui,
                tokens,
                rect,
                ("segment", segment.idx),
                WidgetType::Button,
                &label,
                true,
            );
            // The active-segment highlight is a crossfade on the FAST token:
            // the row under the playhead lights as playback enters it and
            // yields when it leaves. A tap's selection is a state, so it is
            // solid on the very first frame; only the playback highlight
            // animates its opacity.
            let playing_here =
                editor.playing && editor.playhead_segment() == Some(segment.idx) && !selected;
            let fill_id = ui.id().with(("segment-fill", segment.idx));
            let highlight =
                motion.factor(ui.ctx(), fill_id.with("active"), playing_here, motion::FAST);
            let hover = motion.factor(
                ui.ctx(),
                fill_id.with("hover"),
                response.hovered() && !selected,
                motion::FAST,
            );
            let fill = if selected {
                tokens.accent_tint
            } else {
                motion::tint_over(
                    tokens.surface_hover.gamma_multiply(hover),
                    tokens.accent_tint,
                    highlight,
                )
            };
            if fill.a() > 0 {
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(tokens.radius.row), fill);
            }
            if selected {
                // One hairline of accent names the row being edited; the tint
                // alone is too quiet at this density.
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::same(tokens.radius.row),
                    Stroke::new(1.0_f32, tokens.accent.gamma_multiply(0.55)),
                    StrokeKind::Inside,
                );
            }

            // M23A: the 4 px turn rail down the row's leading edge. It is the
            // phone's mark for "these lines belong to this speaker" — the
            // desktop keeps its 8 px lane above the list.
            if let (Some(view), Some(row)) = (&speaker_view, speaker_row) {
                let top = if show_name {
                    rect.top() + 6.0
                } else {
                    rect.top()
                };
                let rail = Rect::from_min_max(
                    pos2(rect.left() + 2.0, top),
                    pos2(rect.left() + 2.0 + PHONE_RAIL_W, rect.bottom()),
                );
                ui.painter().rect_filled(
                    rail,
                    CornerRadius::same(2),
                    speakers::color(tokens, view.color_slot(row.primary))
                        .gamma_multiply(if view.stale { 0.55 } else { 0.9 }),
                );
            }

            let header_center = rect.top() + SEGMENT_TOP + SEGMENT_HEADER / 2.0;
            // Speaker badge (M16I): the shared 3 px bar + chip, in the row's
            // leading gutter. Tapping it selects the segment.
            if let (Some(view), Some(row)) = (&speaker_view, speaker_row) {
                let slot = Rect::from_min_size(
                    pos2(rect.left() + GUTTER, header_center - speakers::BADGE / 2.0),
                    vec2(speakers::SLOT_W, speakers::BADGE),
                );
                let label = view.label(row.primary);
                let badge_response = speakers::badge(
                    ui,
                    tokens,
                    segment.idx,
                    slot,
                    speakers::BadgeLook {
                        label: &label,
                        color_slot: view.color_slot(row.primary),
                        secondary: row.secondary.map(|second| view.color_slot(second)),
                        dim: if view.stale { 0.55 } else { 1.0 },
                    },
                );
                if badge_response.clicked() {
                    // M23A: the badge is the segment's speaker mark — tapping
                    // it opens the turn strip for this line's range. The name
                    // beside it opens the speaker's own sheet.
                    state.open_turn_strip(segment.idx, now);
                }
            }

            // Time gutter, on the tabular grid: `0:09 → 0:10` is the same
            // width, so a playing transcript never twitches.
            let time_width = type_scale::paint_tabular(
                ui.painter(),
                pos2(text_left, header_center),
                Align2::LEFT_CENTER,
                &clock(segment.start_ms),
                &numerals.font,
                tokens.ink_soft,
            );

            let play_center = pos2(rect.right() - GUTTER - 12.0, header_center);
            let advisory_right = play_center.x - 28.0;
            if let Some(advisory_text) = &advisory {
                // Advisory, not an alarm: amber at this weight, never the
                // record red (that belongs to failures that block work).
                type_scale::paint_tabular(
                    ui.painter(),
                    pos2(advisory_right, header_center),
                    Align2::RIGHT_CENTER,
                    advisory_text,
                    &numerals.font,
                    tokens.warn,
                );
            }

            // The turn's name, at the type scale's row title (15/20), with the
            // kit's pencil marking the in-place rename affordance. It is not a
            // second control: the badge beside it owns the rename target, and
            // the name is part of the row's spoken label.
            if show_name {
                let name = speaker_view
                    .as_ref()
                    .map(|view| view.label(primary.expect("named turn")))
                    .unwrap_or_default();
                let name_x = text_left + time_width + 10.0;
                let name_right = if advisory.is_some() {
                    advisory_right - 8.0
                } else {
                    play_center.x - 24.0
                };
                let mut job = egui::text::LayoutJob::default();
                job.append(&name, 0.0, type_scale::row_title().format(tokens.ink));
                job.wrap.max_width = (name_right - name_x).max(40.0);
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                job.wrap.overflow_character = Some('…');
                let name_galley = ui.painter().layout_job(job);
                ui.painter().galley(
                    pos2(name_x, header_center - name_galley.size().y / 2.0),
                    name_galley.clone(),
                    tokens.ink,
                );
                let pencil = Rect::from_center_size(
                    pos2(name_x + name_galley.size().x + 9.0, header_center),
                    vec2(12.0, 12.0),
                );
                if pencil.right() < name_right {
                    icons::paint(ui.painter(), pencil, Icon::Pencil, 12.0, tokens.ink_faint);
                }
                // The whole name (with its pencil) is the Speaker sheet's
                // target: rename, colour, merge, remove. It is a second tap
                // zone inside the row, not a second visual control.
                let name_rect = Rect::from_min_size(
                    pos2(name_x, header_center - 10.0),
                    vec2((name_galley.size().x + 18.0).min(name_right - name_x), 20.0),
                );
                let name_response = a11y::touch(
                    ui,
                    tokens,
                    name_rect,
                    ("speaker-name", primary.expect("named turn")),
                    WidgetType::Button,
                    &format!("{name} — rename, colour, merge or remove"),
                    true,
                );
                if name_response.clicked() {
                    state.open_speaker_sheet_named(primary.expect("named turn"), name.clone(), now);
                }
            }

            // M23A: a labelled transcript's unlabelled rows carry the muted
            // `+ Speaker` pill in the header band. One tap opens the turn strip
            // for exactly this segment; a plain (never tagged) transcript keeps
            // today's clean row — the bottom bar's `Tag speakers` is its door.
            if speaker_view.is_some() && speaker_row.is_none() {
                let pill = Rect::from_min_size(
                    pos2(text_left + time_width + 10.0, header_center - 10.0),
                    vec2(78.0, 20.0),
                );
                let response = a11y::touch(
                    ui,
                    tokens,
                    pill,
                    ("segment-pill", segment.idx),
                    WidgetType::Button,
                    "Tag this segment with a speaker",
                    true,
                );
                ui.painter().rect(
                    pill,
                    CornerRadius::same(10),
                    tokens.control_fill,
                    Stroke::new(1.0_f32, tokens.hairline),
                    StrokeKind::Inside,
                );
                ui.painter().text(
                    pill.center(),
                    Align2::CENTER_CENTER,
                    "+ Speaker",
                    type_scale::caption().font,
                    tokens.ink_soft,
                );
                if response.clicked() {
                    state.open_turn_strip(segment.idx, now);
                }
            }

            // The row's seek mark (the whole row is the target; this is the
            // visible affordance).
            ui.painter().add(egui::Shape::convex_polygon(
                vec![
                    pos2(play_center.x - 5.0, play_center.y - 8.0),
                    pos2(play_center.x - 5.0, play_center.y + 8.0),
                    pos2(play_center.x + 8.0, play_center.y),
                ],
                tokens.ink_soft,
                Stroke::NONE,
            ));

            let body_top = rect.top() + SEGMENT_TOP + SEGMENT_HEADER + BODY_GAP;
            if editing {
                let edit_rect =
                    Rect::from_min_size(pos2(text_left, body_top), vec2(body_width, 74.0));
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
                            .font(FontId::proportional(body_token.size())),
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
                        type_scale::row_value().font,
                        if primary {
                            tokens.on_accent
                        } else {
                            tokens.ink
                        },
                    );
                }
            } else {
                ui.painter()
                    .galley(pos2(text_left, body_top), body_galley, tokens.ink);
                if editor.dirty.contains(&segment.idx) {
                    ui.painter().circle_filled(
                        pos2(rect.left() + 8.0, header_center),
                        3.0,
                        tokens.accent,
                    );
                }
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
            // accent, its tint crossfading on the FAST token. Reduce Motion
            // snaps the swap (the state stays).
            if show_words {
                word_strip(
                    ui,
                    tokens,
                    motion,
                    &mut editor,
                    &segment,
                    rect,
                    text_left,
                    body_top + body_h + 2.0,
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
    let play_press = chrome::press_frame(
        ui.ctx(),
        ui.id().with("editor-play-press"),
        play.is_pointer_button_down_on(),
        motion.reduce,
    );
    let radius = 26.0 * play_press.scale;
    ui.painter()
        .circle_filled(play_rect.center(), radius, tokens.accent);
    if editor.playing {
        let c = play_rect.center();
        let side = vec2(13.0, 16.0) * play_press.scale;
        ui.painter().rect_filled(
            Rect::from_center_size(c, side),
            CornerRadius::same(2),
            tokens.on_accent,
        );
    } else {
        let c = play_rect.center();
        let s = play_press.scale;
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                pos2(c.x - 5.0 * s, c.y - 8.0 * s),
                pos2(c.x - 5.0 * s, c.y + 8.0 * s),
                pos2(c.x + 9.0 * s, c.y),
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
    // The live transport read-out is the one numeral a reader watches tick:
    // both runs and the separator sit on fixed cells, so the bar does not
    // twitch at 0:09 → 0:10.
    type_scale::paint_tabular(
        ui.painter(),
        pos2(bar.left() + GUTTER + 64.0, bar.center().y),
        Align2::LEFT_CENTER,
        &format!("{} / {}", clock(editor.playhead_ms), clock(duration)),
        &type_scale::numerals().font,
        tokens.ink_soft,
    );
    // M23A: the Speakers button. It reads `Tag speakers` until the transcript
    // has a label, then the count; both are the plan's literal labels, and the
    // sheet behind it is the only door to Add/Clear.
    let speakers_btn = Rect::from_min_size(
        pos2(bar.left() + GUTTER + 166.0, bar.center().y - 22.0),
        vec2(92.0, 44.0),
    );
    let speakers_label = editor
        .speakers
        .as_ref()
        .map(super::model::CompactSpeakers::button_label)
        .unwrap_or_else(|| "Tag speakers".to_string());
    if chrome::action_button(
        ui,
        tokens,
        motion,
        speakers_btn,
        "editor-speakers",
        &speakers_label,
        false,
    )
    .clicked()
    {
        state.open_speakers_sheet(now);
    }
    a11y::record_button(ui, speakers_btn, &speakers_label, true);

    let search_btn = Rect::from_center_size(
        pos2(bar.right() - GUTTER - 128.0, bar.center().y),
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
    let find_token = type_scale::row_value();
    let find_galley = ui
        .painter()
        .layout_no_wrap("Find".to_owned(), find_token.font, tokens.ink);
    let find_total = 16.0 + 6.0 + find_galley.size().x;
    let find_start = search_btn.center().x - find_total / 2.0;
    icons::paint(
        ui.painter(),
        Rect::from_center_size(
            pos2(find_start + 8.0, search_btn.center().y),
            vec2(16.0, 16.0),
        ),
        Icon::Search,
        16.0,
        tokens.ink_soft,
    );
    ui.painter().galley(
        pos2(
            find_start + 22.0,
            search_btn.center().y - find_galley.size().y / 2.0,
        ),
        find_galley,
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
/// the current word carrying the accent tint and a FAST crossfade underline.
/// A tap parks the playhead at the word's estimated start and emits a Seek.
#[allow(clippy::too_many_arguments)]
fn word_strip(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    editor: &mut super::model::CompactEditor,
    segment: &suramak_core::transcript::Segment,
    rect: Rect,
    text_left: f32,
    top: f32,
    words: &[suramak_core::transcript::WordTiming],
    actions: &mut Vec<CompactAction>,
) {
    let current = editor.current_word(segment.idx);
    let playing = editor.playing;
    let caption = type_scale::caption();
    let mut x = text_left;
    let mut y = top;
    let max_x = rect.right() - GUTTER - 8.0;
    let line_start = x;
    for (word_index, word) in words.iter().enumerate() {
        let galley =
            ui.painter()
                .layout_no_wrap(word.text.clone(), caption.font.clone(), tokens.ink_soft);
        let width = galley.size().x + 14.0;
        if x + width > max_x && x > line_start {
            x = line_start;
            y += 22.0;
        }
        let chip = Rect::from_min_size(pos2(x, y), vec2(width, 18.0));
        let id = egui::Id::new(("compact-word", segment.idx, word_index));
        let highlight = motion.factor(
            ui.ctx(),
            id,
            playing && current == Some(word_index),
            motion::FAST,
        );
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

/// The phone's advisory read for one segment, in human words.
///
/// The desktop editor keeps the trade shorthand (`CPL 44 · CPS 21.3`) because
/// its audience is subtitle editors; the phone says **Line length** and
/// **Pace**. Only crossed dimensions appear, exactly like `Metrics::badge`.
/// Thresholds are the documented defaults (37/42 and 17/20); the phone has no
/// settings surface for them.
fn advisory_for(text: &str, start_ms: i64, end_ms: i64) -> Option<String> {
    let metrics = diagnose(text, start_ms, end_ms, &SubtitleLimits::default());
    let mut parts = Vec::new();
    if metrics.cpl_level != Level::Ok {
        parts.push(format!("Line length {}", metrics.longest_line));
    }
    if metrics.cps_level != Level::Ok {
        parts.push(format!("Pace {:.1}", metrics.cps));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
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
    paint_display_centered(
        ui,
        pos2(body.center().x, body.top() + 30.0),
        "Export transcript",
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
    let has_speakers = editor
        .speakers
        .as_ref()
        .is_some_and(|view| !view.speakers.is_empty());
    let speakers_edited = editor
        .speakers
        .as_ref()
        .is_some_and(|view| view.edited_since_export);

    if let Some(session) = state.export_session.clone() {
        export_result(ui, tokens, motion, state, body, &session, now, &mut actions);
        return actions;
    }

    ui.painter().text(
        pos2(body.center().x, body.top() + 54.0),
        Align2::CENTER_CENTER,
        "Same bytes as the desktop. Choose where it goes.",
        type_scale::caption().font,
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
        // The chip takes the sheet's spring: its body scales, the hit rect does
        // not. Selected keeps the plan's tint + 45 % accent rim.
        let press = chrome::press_frame(
            ui.ctx(),
            ui.id().with(("export-chip-press", format.label())),
            response.is_pointer_button_down_on(),
            motion.reduce,
        );
        let visual = Rect::from_center_size(rect.center(), rect.size() * press.scale);
        let fill = if selected {
            tokens.accent_tint
        } else if response.hovered() {
            tokens.surface_hover
        } else {
            tokens.control_fill
        };
        ui.painter().rect(
            visual,
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
            visual.center(),
            Align2::CENTER_CENTER,
            format.label(),
            type_scale::row_value().font,
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
    let mut next_toggle_y = 176.0;
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
            icons::paint(ui.painter(), box_rect, Icon::Check, 13.0, tokens.on_accent);
        }
        ui.painter().text(
            pos2(box_rect.right() + 10.0, toggle_rect.center().y),
            Align2::LEFT_CENTER,
            if has_translation {
                "Include the translation (stacked)"
            } else {
                "No translation to include"
            },
            type_scale::caption().font,
            if has_translation {
                tokens.ink
            } else {
                tokens.ink_faint
            },
        );
        if response.clicked() && has_translation {
            state.export_bilingual = !state.export_bilingual;
        }
        next_toggle_y = 224.0;
    }

    // M23A: `Include speakers`. Offered for SRT/VTT/Markdown only; a format
    // with no speaker variant says why instead of showing a dead switch.
    let speakers_toggle = Rect::from_min_size(
        pos2(body.left() + GUTTER, body.top() + next_toggle_y),
        vec2(body.width() - GUTTER * 2.0, 40.0),
    );
    let speakers_offered = current_format.supports_speakers();
    let speakers_enabled = speakers_offered && has_speakers;
    let response = a11y::touch(
        ui,
        tokens,
        speakers_toggle,
        "export-speakers",
        WidgetType::Checkbox,
        "Include speakers",
        speakers_enabled,
    );
    let box_rect = Rect::from_min_size(
        pos2(
            speakers_toggle.left() + 2.0,
            speakers_toggle.center().y - 10.0,
        ),
        vec2(20.0, 20.0),
    );
    ui.painter().rect(
        box_rect,
        CornerRadius::same(5),
        if state.export_speakers && speakers_enabled {
            tokens.accent
        } else {
            Color32::TRANSPARENT
        },
        Stroke::new(1.0_f32, tokens.hairline_strong),
        StrokeKind::Inside,
    );
    if state.export_speakers && speakers_enabled {
        icons::paint(ui.painter(), box_rect, Icon::Check, 13.0, tokens.on_accent);
    }
    let speakers_note = if !speakers_offered {
        format!("{} has no speaker variant", current_format.label())
    } else if !has_speakers {
        "No speakers tagged".to_string()
    } else {
        "Include speakers".to_string()
    };
    ui.painter().text(
        pos2(box_rect.right() + 10.0, speakers_toggle.center().y),
        Align2::LEFT_CENTER,
        &speakers_note,
        type_scale::caption().font,
        if speakers_enabled {
            tokens.ink
        } else {
            tokens.ink_faint
        },
    );
    if response.clicked() && speakers_enabled {
        state.export_speakers = !state.export_speakers;
    }
    if speakers_edited && speakers_enabled {
        // The honesty note: an exported file is a snapshot, never rewritten.
        ui.painter().text(
            pos2(body.left() + GUTTER, body.top() + next_toggle_y + 50.0),
            Align2::LEFT_CENTER,
            "Speakers edited since last export — export again to update",
            type_scale::caption().font,
            tokens.warn,
        );
    }

    // Two ways out, both disclosed.
    ui.painter().text(
        pos2(body.left() + GUTTER, body.top() + 300.0),
        Align2::LEFT_CENTER,
        "Picker: the system file chooser writes to the frame you pick.",
        type_scale::caption().font,
        tokens.ink_soft,
    );
    ui.painter().text(
        pos2(body.left() + GUTTER, body.top() + 318.0),
        Align2::LEFT_CENTER,
        "Share: hands this one file to another app (ACTION_SEND).",
        type_scale::caption().font,
        tokens.ink_soft,
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
    material::frosted_panel(ui, tokens, card, tokens.radius.card, Plane::Card);
    let spoken = format!("{line}: {detail}");
    let display = type_scale::editorial_display();
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &line,
        0.0,
        display.format(if error {
            tokens.record_red_text
        } else {
            tokens.ink
        }),
    );
    let line_galley = ui.painter().layout_job(job);
    ui.painter().galley(
        pos2(
            card.left() + 16.0,
            card.top() + 30.0 - line_galley.size().y / 2.0,
        ),
        line_galley,
        tokens.ink,
    );
    ui.painter().text(
        pos2(card.left() + 16.0, card.top() + 60.0),
        Align2::LEFT_CENTER,
        truncate(&detail, 58),
        type_scale::caption().font,
        tokens.ink_soft,
    );
    ui.painter().text(
        pos2(card.left() + 16.0, card.top() + 96.0),
        Align2::LEFT_CENTER,
        format!("{} · {}", session.format.label(), session.format.mime()),
        type_scale::mono().font,
        tokens.ink_soft,
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
    paint_display_centered(
        ui,
        pos2(body.center().x, body.top() + 40.0),
        "Quit Suramak?",
        tokens.ink,
    );
    ui.painter().text(
        pos2(body.center().x, body.top() + 70.0),
        Align2::CENTER_CENTER,
        "Any recording is saved and handed to the inbox first;\nrunning transcription pauses, services stop, then the app exits.",
        type_scale::caption().font,
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

/// A centred serif line at the type scale's **editorial display** (Newsreader
/// 21 / −0.2 / 27) — sheet titles and result states. Laid out as a job so the
/// token's tracking and line height are real, not a bare `FontId`.
fn paint_display_centered(ui: &Ui, center: egui::Pos2, text: &str, ink: Color32) {
    let token = type_scale::editorial_display();
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, token.format(ink));
    let galley = ui.painter().layout_job(job);
    ui.painter()
        .galley(center - galley.size() / 2.0, galley, ink);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The phone names the metric. `CPL` is trade shorthand the review flagged;
    /// the read-out says **Line length** (and Pace for the other dimension).
    #[test]
    fn the_advisory_speaks_line_length_not_cpl() {
        let long = "a".repeat(48);
        let advisory = advisory_for(&long, 0, 4_000).expect("48 graphemes crosses 42");
        assert!(advisory.starts_with("Line length 48"), "{advisory}");
        assert!(!advisory.contains("CPL"), "{advisory}");
        assert!(!advisory.contains("CPS"), "{advisory}");

        // A clean line has no advisory at all.
        assert!(advisory_for("A short line.", 0, 4_000).is_none());

        // A dense segment crosses pace only: each hard line stays under the
        // line-length threshold, so the read-out says Pace and nothing else.
        let dense = "word word word word\n".repeat(8);
        let advisory = advisory_for(&dense, 0, 2_000).expect("40 CPS crosses 17");
        assert!(advisory.starts_with("Pace "), "{advisory}");
    }

    /// Both numerals in the advisory sit on fixed cells: the widening from
    /// `99` to `100` is the only change, never a digit swap.
    #[test]
    fn the_advisory_numerals_are_tabular() {
        let size = type_scale::numerals().size();
        assert_eq!(
            type_scale::tabular_width("Line length 44", size),
            type_scale::tabular_width("Line length 84", size)
        );
    }
}
