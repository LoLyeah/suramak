//! M23A — the phone's manual speaker tagging surfaces.
//!
//! Three sheets and one toast, all painted in the Deck's own materials:
//!
//! * [`speakers_sheet`] — the list behind the bottom bar's `Tag speakers` /
//!   `N speakers` button: every speaker with its colour and turn count,
//!   `Add speaker`, and the destructive `Clear all speakers` (armed on the
//!   first tap, executed on the second).
//! * [`speaker_sheet`] — one speaker: rename, colour dots, `Merge into…`,
//!   `Remove label`.
//! * [`turn_strip`] — the per-segment chips (`Speaker 1` … `+ New`) and the
//!   `This turn | From here down` scope toggle.
//! * [`undo_toast`] — the 6 s `Assigned Speaker 2 [Undo]` strip.
//!
//! The UI mutates nothing: every tap returns a [`CompactAction`] the host
//! applies through the store, then re-reads the view. That keeps the badges,
//! the export stamp and the undo log answering to one writer.

use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Stroke, StrokeKind, Ui, WidgetType,
};

use suramak_core::diarization::{COLOR_SLOTS, MAX_SPEAKERS};

use crate::deck::icons::{self, Icon};
use crate::deck::material::{self, Plane};
use crate::deck::speakers;
use crate::deck::theme::Tokens;
use crate::deck::type_scale;

use super::a11y;
use super::chrome::{self, GUTTER, MIN_TOUCH};
use super::model::{CompactAction, CompactApp, TurnScope, UNDO_TOAST_SECS};

/// The list row's height in the Speakers sheet.
const SPEAKER_ROW: f32 = 46.0;
/// The colour dot diameter.
const DOT: f32 = 18.0;

/// One centred line at the editorial display token (the sheets' titles).
fn title(ui: &Ui, center: egui::Pos2, text: &str, ink: Color32) {
    let token = type_scale::editorial_display();
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, token.format(ink));
    let galley = ui.painter().layout_job(job);
    ui.painter()
        .galley(center - galley.size() / 2.0, galley, ink);
}

/// A left-aligned line at the editorial display token (the Speaker sheet's
/// title is the speaker's name, starting at the gutter).
fn title_left(ui: &Ui, pos: egui::Pos2, text: &str, ink: Color32) {
    let token = type_scale::editorial_display();
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, token.format(ink));
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(pos, galley, ink);
}

fn caption_at(ui: &Ui, pos: egui::Pos2, align: Align2, text: &str, ink: Color32) {
    ui.painter()
        .text(pos, align, text, type_scale::caption().font, ink);
}

/// A small filled colour dot with an optional selection ring.
fn colour_dot(ui: &Ui, tokens: &Tokens, center: egui::Pos2, slot: u8, selected: bool) {
    ui.painter()
        .circle_filled(center, DOT / 2.0, speakers::color(tokens, slot));
    if selected {
        ui.painter()
            .circle_stroke(center, DOT / 2.0 + 3.0, Stroke::new(2.0_f32, tokens.ink));
    }
}

fn truncate(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ---------------------------------------------------------------------------
// The Speakers sheet
// ---------------------------------------------------------------------------

pub fn speakers_sheet(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &crate::deck::motion::Motion,
    state: &mut CompactApp,
    body: Rect,
) -> Vec<CompactAction> {
    let mut actions = Vec::new();
    let view = state.speakers_view().cloned();
    title(
        ui,
        pos2(body.center().x, body.top() + 30.0),
        "Speakers",
        tokens.ink,
    );
    caption_at(
        ui,
        pos2(body.center().x, body.top() + 52.0),
        Align2::CENTER_CENTER,
        "Per recording, never identity. No voiceprints are stored.",
        tokens.ink_soft,
    );

    let count = view.as_ref().map(|view| view.speakers.len()).unwrap_or(0);
    let list_top = body.top() + 68.0;
    match view.as_ref() {
        Some(view) if !view.speakers.is_empty() => {
            for (index, speaker) in view.speakers.iter().enumerate() {
                let top = list_top + index as f32 * SPEAKER_ROW;
                let row = Rect::from_min_size(
                    pos2(body.left() + GUTTER, top),
                    vec2(body.width() - GUTTER * 2.0, SPEAKER_ROW - 2.0),
                );
                let response = a11y::touch(
                    ui,
                    tokens,
                    row,
                    ("speaker-row", speaker.speaker_id),
                    WidgetType::Button,
                    &format!(
                        "{} · {} turns",
                        speaker.label,
                        view.turn_count(speaker.speaker_id)
                    ),
                    true,
                );
                if response.hovered() {
                    ui.painter().rect_filled(
                        row,
                        CornerRadius::same(tokens.radius.control),
                        tokens.surface_hover,
                    );
                }
                colour_dot(
                    ui,
                    tokens,
                    pos2(row.left() + DOT / 2.0 + 2.0, row.center().y),
                    speaker.color_slot,
                    false,
                );
                ui.painter().text(
                    pos2(row.left() + DOT + 14.0, row.center().y),
                    Align2::LEFT_CENTER,
                    truncate(&speaker.label, 28),
                    type_scale::row_title().font,
                    tokens.ink,
                );
                let turns = view.turn_count(speaker.speaker_id);
                caption_at(
                    ui,
                    pos2(row.right() - 4.0, row.center().y),
                    Align2::RIGHT_CENTER,
                    &format!("{turns} {}", if turns == 1 { "turn" } else { "turns" }),
                    if turns == 0 {
                        tokens.ink_faint
                    } else {
                        tokens.ink_soft
                    },
                );
                icons::paint(
                    ui.painter(),
                    Rect::from_center_size(
                        pos2(row.right() - 72.0, row.center().y),
                        vec2(12.0, 12.0),
                    ),
                    Icon::Chevron,
                    12.0,
                    tokens.ink_faint,
                );
                if response.clicked() {
                    state.open_speaker_sheet(speaker.speaker_id, response.ctx.input(|i| i.time));
                }
            }
        }
        _ => {
            ui.painter().text(
                pos2(body.center().x, list_top + 54.0),
                Align2::CENTER_CENTER,
                "No speakers yet.",
                type_scale::row_title().font,
                tokens.ink,
            );
            caption_at(
                ui,
                pos2(body.center().x, list_top + 78.0),
                Align2::CENTER_CENTER,
                "Add one here, or tap + Speaker on any line.",
                tokens.ink_soft,
            );
        }
    }

    let buttons_top = body.bottom() - 92.0;
    let half = (body.width() - GUTTER * 2.0 - 8.0) / 2.0;
    let add = Rect::from_min_size(pos2(body.left() + GUTTER, buttons_top), vec2(half, 44.0));
    let can_add = count < MAX_SPEAKERS as usize;
    if chrome::action_button(
        ui,
        tokens,
        motion,
        add,
        "speakers-add",
        "Add speaker",
        false,
    )
    .clicked()
        && can_add
    {
        if let Some(action) = state.request_add_speaker() {
            actions.push(action);
        }
    }
    if !can_add {
        caption_at(
            ui,
            pos2(add.center().x, add.bottom() + 10.0),
            Align2::CENTER_CENTER,
            "Six is the palette's limit",
            tokens.ink_faint,
        );
    }
    a11y::record_button(ui, add, "Add speaker", can_add);

    let clear = Rect::from_min_size(
        pos2(body.right() - GUTTER - half, buttons_top),
        vec2(half, 44.0),
    );
    let armed = state.speakers_confirm_clear;
    let clear_response = a11y::touch(
        ui,
        tokens,
        clear,
        "speakers-clear",
        WidgetType::Button,
        if armed {
            "Tap again to clear all speakers"
        } else {
            "Clear all speakers"
        },
        count > 0,
    );
    ui.painter().rect_stroke(
        clear,
        CornerRadius::same(tokens.radius.control),
        Stroke::new(
            1.0_f32,
            if armed {
                tokens.record_red_text
            } else {
                tokens.hairline
            },
        ),
        StrokeKind::Inside,
    );
    ui.painter().text(
        clear.center(),
        Align2::CENTER_CENTER,
        if armed {
            "Tap again to clear"
        } else {
            "Clear all"
        },
        type_scale::row_value().font,
        if count == 0 {
            tokens.ink_faint
        } else if armed {
            tokens.record_red_text
        } else {
            tokens.ink_soft
        },
    );
    a11y::record_button(ui, clear, "Clear all speakers", count > 0);
    if clear_response.clicked() && count > 0 {
        if let Some(action) = state.request_clear_speakers() {
            actions.push(action);
        }
    }
    caption_at(
        ui,
        pos2(body.center().x, body.bottom() - 28.0),
        Align2::CENTER_CENTER,
        "Clear all removes the labels; the transcript text is untouched.",
        tokens.ink_faint,
    );
    actions
}

// ---------------------------------------------------------------------------
// One speaker
// ---------------------------------------------------------------------------

pub fn speaker_sheet(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &crate::deck::motion::Motion,
    state: &mut CompactApp,
    body: Rect,
) -> Vec<CompactAction> {
    let mut actions = Vec::new();
    let Some(mut sheet) = state.speaker_sheet.clone() else {
        return actions;
    };
    let Some(view) = state.speakers_view().cloned() else {
        return actions;
    };
    let slot = view.color_slot(sheet.speaker_id);
    let turns = view.turn_count(sheet.speaker_id);

    title_left(
        ui,
        pos2(body.left() + GUTTER, body.top() + 18.0),
        &truncate(&sheet.rename, 22),
        tokens.ink,
    );
    colour_dot(
        ui,
        tokens,
        pos2(body.right() - GUTTER - DOT / 2.0, body.top() + 30.0),
        slot,
        false,
    );
    caption_at(
        ui,
        pos2(body.left() + GUTTER, body.top() + 50.0),
        Align2::LEFT_CENTER,
        &format!("{turns} {}", if turns == 1 { "turn" } else { "turns" }),
        tokens.ink_soft,
    );

    // --- rename ------------------------------------------------------------
    let field = Rect::from_min_size(
        pos2(body.left() + GUTTER, body.top() + 68.0),
        vec2(body.width() - GUTTER * 2.0, 48.0),
    );
    ui.painter().rect(
        field,
        CornerRadius::same(tokens.radius.control),
        tokens.control_fill,
        Stroke::new(1.0_f32, tokens.hairline),
        StrokeKind::Inside,
    );
    {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(field.shrink2(vec2(12.0, 8.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.add(
            egui::TextEdit::singleline(&mut sheet.rename)
                .frame(false)
                .hint_text("Name this speaker")
                .font(FontId::proportional(type_scale::row_value().size()))
                .desired_width(field.width() - 24.0),
        );
    }
    a11y::describe(ui, field, "Speaker name");
    state.speaker_sheet = Some(sheet.clone());

    let save = Rect::from_min_size(
        pos2(body.left() + GUTTER, body.top() + 124.0),
        vec2(body.width() - GUTTER * 2.0, 44.0),
    );
    if chrome::action_button(
        ui,
        tokens,
        motion,
        save,
        "speaker-rename",
        "Save name",
        true,
    )
    .clicked()
    {
        if let Some(action) = state.request_rename() {
            actions.push(action);
        }
    }
    a11y::record_button(ui, save, "Save name", !sheet.rename.trim().is_empty());

    // --- colour dots -------------------------------------------------------
    caption_at(
        ui,
        pos2(body.left() + GUTTER, body.top() + 188.0),
        Align2::LEFT_CENTER,
        "Colour",
        tokens.ink_soft,
    );
    let dots_left = body.left() + GUTTER + 56.0;
    for index in 0..COLOR_SLOTS {
        let center = pos2(
            dots_left + index as f32 * 36.0 + DOT / 2.0,
            body.top() + 188.0,
        );
        let hit = Rect::from_center_size(center, vec2(MIN_TOUCH, MIN_TOUCH));
        let response = a11y::touch(
            ui,
            tokens,
            hit,
            ("speaker-colour", index),
            WidgetType::RadioButton,
            &format!("Colour {}", index + 1),
            true,
        );
        colour_dot(ui, tokens, center, index, slot == index);
        if response.clicked() {
            if let Some(action) = state.request_speaker_color(index) {
                actions.push(action);
            }
        }
    }

    // --- merge / remove ----------------------------------------------------
    if sheet.merge_open {
        caption_at(
            ui,
            pos2(body.left() + GUTTER, body.top() + 228.0),
            Align2::LEFT_CENTER,
            "Merge this speaker into:",
            tokens.ink_soft,
        );
        let mut x = body.left() + GUTTER;
        let mut y = body.top() + 242.0;
        let max_x = body.right() - GUTTER;
        for speaker in view
            .speakers
            .iter()
            .filter(|speaker| speaker.speaker_id != sheet.speaker_id)
        {
            let galley = ui.painter().layout_no_wrap(
                truncate(&speaker.label, 18),
                type_scale::row_value().font,
                tokens.ink,
            );
            let width = galley.size().x + 40.0;
            if x + width > max_x {
                x = body.left() + GUTTER;
                y += 40.0;
            }
            let chip = Rect::from_min_size(pos2(x, y), vec2(width, 34.0));
            let response = a11y::touch(
                ui,
                tokens,
                chip,
                ("speaker-merge-target", speaker.speaker_id),
                WidgetType::Button,
                &format!("Merge into {}", speaker.label),
                true,
            );
            ui.painter().rect(
                chip,
                CornerRadius::same(tokens.radius.control),
                tokens.control_fill,
                Stroke::new(1.0_f32, tokens.hairline),
                StrokeKind::Inside,
            );
            colour_dot(
                ui,
                tokens,
                pos2(chip.left() + 16.0, chip.center().y),
                speaker.color_slot,
                false,
            );
            ui.painter().galley(
                pos2(chip.left() + 30.0, chip.center().y - galley.size().y / 2.0),
                galley,
                tokens.ink,
            );
            if response.clicked() {
                if let Some(action) = state.request_merge(speaker.speaker_id) {
                    actions.push(action);
                }
            }
            x += width + 8.0;
        }
    } else {
        let merge = Rect::from_min_size(
            pos2(body.left() + GUTTER, body.top() + 246.0),
            vec2(body.width() - GUTTER * 2.0, 44.0),
        );
        let merge_enabled = view.speakers.len() > 1;
        if chrome::action_button(
            ui,
            tokens,
            motion,
            merge,
            "speaker-merge",
            "Merge into…",
            false,
        )
        .clicked()
            && merge_enabled
        {
            state.open_merge_targets();
        }
        a11y::record_button(ui, merge, "Merge into another speaker", merge_enabled);
    }

    let remove = Rect::from_min_size(
        pos2(body.left() + GUTTER, body.bottom() - 74.0),
        vec2(body.width() - GUTTER * 2.0, 44.0),
    );
    let remove_response = a11y::touch(
        ui,
        tokens,
        remove,
        "speaker-remove",
        WidgetType::Button,
        "Remove label",
        true,
    );
    ui.painter().text(
        remove.center(),
        Align2::CENTER_CENTER,
        "Remove label",
        type_scale::row_value().font,
        tokens.record_red_text,
    );
    a11y::record_button(ui, remove, "Remove label", true);
    if remove_response.clicked() {
        if let Some(action) = state.request_remove_label() {
            actions.push(action);
        }
    }
    caption_at(
        ui,
        pos2(body.center().x, body.bottom() - 18.0),
        Align2::CENTER_CENTER,
        "The lines become unlabelled; the text stays.",
        tokens.ink_faint,
    );
    actions
}

// ---------------------------------------------------------------------------
// The turn strip
// ---------------------------------------------------------------------------

pub fn turn_strip(
    ui: &mut Ui,
    tokens: &Tokens,
    state: &mut CompactApp,
    body: Rect,
) -> Vec<CompactAction> {
    let mut actions = Vec::new();
    let Some(strip) = state.turn_strip else {
        return actions;
    };
    let now = ui.input(|input| input.time);
    let Some(view) = state.speakers_view().cloned() else {
        return actions;
    };
    let segment_label = state
        .editor
        .as_ref()
        .and_then(|editor| editor.transcript.segments.get(strip.segment as usize))
        .map(|segment| {
            format!(
                "Segment {} · {}",
                segment.idx + 1,
                super::model::clock(segment.start_ms)
            )
        })
        .unwrap_or_else(|| format!("Segment {}", strip.segment + 1));
    title_left(
        ui,
        pos2(body.left() + GUTTER, body.top() + 18.0),
        "Tag speakers",
        tokens.ink,
    );
    caption_at(
        ui,
        pos2(body.left() + GUTTER, body.top() + 52.0),
        Align2::LEFT_CENTER,
        &segment_label,
        tokens.ink_soft,
    );

    // --- chips -------------------------------------------------------------
    let mut x = body.left() + GUTTER;
    let mut y = body.top() + 68.0;
    let max_x = body.right() - GUTTER;
    for speaker in &view.speakers {
        let galley = ui.painter().layout_no_wrap(
            truncate(&speaker.label, 16),
            type_scale::row_value().font,
            tokens.ink,
        );
        let width = (galley.size().x + 50.0).max(84.0);
        if x + width > max_x {
            x = body.left() + GUTTER;
            y += 42.0;
        }
        let chip = Rect::from_min_size(pos2(x, y), vec2(width, 36.0));
        let response = a11y::touch(
            ui,
            tokens,
            chip,
            ("turn-chip", speaker.speaker_id),
            WidgetType::Button,
            &format!("Assign to {}", speaker.label),
            true,
        );
        ui.painter().rect(
            chip,
            CornerRadius::same(18),
            speakers::chip_fill(tokens, speaker.color_slot),
            Stroke::new(1.0_f32, speakers::color(tokens, speaker.color_slot)),
            StrokeKind::Inside,
        );
        ui.painter().circle_filled(
            pos2(chip.left() + 15.0, chip.center().y),
            5.0,
            speakers::color(tokens, speaker.color_slot),
        );
        ui.painter().galley(
            pos2(chip.left() + 28.0, chip.center().y - galley.size().y / 2.0),
            galley,
            tokens.ink,
        );
        if response.clicked() {
            if let Some(action) = state.request_assign(speaker.speaker_id, now) {
                actions.push(action);
            }
        }
        x += width + 8.0;
    }
    // `+ New` last.
    let width = 84.0;
    if x + width > max_x {
        x = body.left() + GUTTER;
        y += 42.0;
    }
    let chip = Rect::from_min_size(pos2(x, y), vec2(width, 36.0));
    let response = a11y::touch(
        ui,
        tokens,
        chip,
        "turn-new",
        WidgetType::Button,
        "New speaker",
        view.speakers.len() < MAX_SPEAKERS as usize,
    );
    ui.painter().rect(
        chip,
        CornerRadius::same(18),
        Color32::TRANSPARENT,
        Stroke::new(1.0_f32, tokens.hairline_strong),
        StrokeKind::Inside,
    );
    ui.painter().text(
        chip.center(),
        Align2::CENTER_CENTER,
        "+ New",
        type_scale::row_value().font,
        tokens.ink_soft,
    );
    if response.clicked() && view.speakers.len() < MAX_SPEAKERS as usize {
        if let Some(action) = state.request_new_speaker() {
            actions.push(action);
        }
    }

    // --- scope -------------------------------------------------------------
    let toggle_top = body.top() + 158.0;
    caption_at(
        ui,
        pos2(body.left() + GUTTER, toggle_top - 12.0),
        Align2::LEFT_CENTER,
        "Apply to",
        tokens.ink_soft,
    );
    let segment_w = (body.width() - GUTTER * 2.0 - 6.0) / 2.0;
    for (index, scope) in [TurnScope::ThisTurn, TurnScope::FromHereDown]
        .into_iter()
        .enumerate()
    {
        let rect = Rect::from_min_size(
            pos2(
                body.left() + GUTTER + index as f32 * (segment_w + 6.0),
                toggle_top,
            ),
            vec2(segment_w, 40.0),
        );
        let selected = strip.scope == scope;
        let response = a11y::touch(
            ui,
            tokens,
            rect,
            ("turn-scope", index),
            WidgetType::RadioButton,
            scope.label(),
            true,
        );
        ui.painter().rect(
            rect,
            CornerRadius::same(tokens.radius.control),
            if selected {
                tokens.accent_tint
            } else {
                tokens.control_fill
            },
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
            scope.label(),
            type_scale::row_value().font,
            if selected { tokens.accent } else { tokens.ink },
        );
        if response.clicked() && !selected {
            state.toggle_turn_scope();
        }
    }

    // --- undo --------------------------------------------------------------
    let undo = Rect::from_min_size(
        pos2(body.left() + GUTTER, body.bottom() - 56.0),
        vec2(body.width() - GUTTER * 2.0, 40.0),
    );
    let enabled = state.speakers_undo_available;
    let response = a11y::touch(
        ui,
        tokens,
        undo,
        "turn-undo",
        WidgetType::Button,
        "Undo the last speaker edit",
        enabled,
    );
    ui.painter().text(
        undo.center(),
        Align2::CENTER_CENTER,
        "Undo",
        type_scale::row_value().font,
        if enabled {
            tokens.accent
        } else {
            tokens.ink_faint
        },
    );
    a11y::record_button(ui, undo, "Undo", enabled);
    if response.clicked() && enabled {
        if let Some(action) = state.request_undo_speakers(now) {
            actions.push(action);
        }
    }
    caption_at(
        ui,
        pos2(body.center().x, body.bottom() - 12.0),
        Align2::CENTER_CENTER,
        "Two taps: pick the range, then the speaker.",
        tokens.ink_faint,
    );
    actions
}

// ---------------------------------------------------------------------------
// The Undo toast
// ---------------------------------------------------------------------------

/// Draws `Assigned Speaker 2 · Undo` above the bottom bar for six seconds.
/// Returns the action when Undo is tapped.
pub fn undo_toast(
    ui: &mut Ui,
    tokens: &Tokens,
    state: &mut CompactApp,
    bar: Rect,
) -> Vec<CompactAction> {
    let mut actions = Vec::new();
    let Some(toast) = state.undo_toast.clone() else {
        return actions;
    };
    let now = ui.input(|input| input.time);
    let alpha = toast.alpha(now, state.reduce_motion);
    if alpha <= 0.01 {
        return actions;
    }
    let width = (bar.width() - GUTTER * 2.0).min(420.0);
    let card = Rect::from_min_size(
        pos2(bar.center().x - width / 2.0, bar.top() - 58.0),
        vec2(width, 44.0),
    );
    material::frosted_panel(ui, tokens, card, tokens.radius.card, Plane::Card);
    let undo = Rect::from_min_size(
        pos2(card.right() - 78.0, card.top() + 4.0),
        vec2(70.0, 36.0),
    );
    let response = a11y::touch(
        ui,
        tokens,
        undo,
        "toast-undo",
        WidgetType::Button,
        "Undo",
        true,
    );
    ui.painter().text(
        pos2(card.left() + 14.0, card.center().y),
        Align2::LEFT_CENTER,
        truncate(&toast.label, 30),
        type_scale::row_value().font,
        tokens.ink.gamma_multiply(alpha),
    );
    ui.painter().text(
        undo.center(),
        Align2::CENTER_CENTER,
        "Undo",
        type_scale::row_value().font,
        tokens.accent.gamma_multiply(alpha),
    );
    if response.clicked() {
        if let Some(action) = state.request_undo_speakers(now) {
            actions.push(action);
        }
    }
    // A quiet countdown hairline: the toast is leaving, not stuck.
    let remaining = (UNDO_TOAST_SECS - (now - toast.opened_at)).clamp(0.0, UNDO_TOAST_SECS);
    let fraction = (remaining / UNDO_TOAST_SECS) as f32;
    ui.painter().rect_filled(
        Rect::from_min_size(
            pos2(card.left(), card.bottom() - 2.0),
            vec2(card.width() * fraction, 2.0),
        ),
        CornerRadius::same(1),
        tokens.accent.gamma_multiply(alpha * 0.6),
    );
    actions
}
