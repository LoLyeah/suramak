//! Editor route, rebuilt to comp C — the Deck (M9a).
//!
//! Directly under the titlebar: ONE full-width control-deck band (the panel
//! level of the material ladder, 96 pt): a compact transport capsule on the
//! left, the waveform strip through the middle (the M1 silhouette for now —
//! real peaks are slice 2; this geometry is final), and the `mm:ss.mmm /
//! mm:ss.mmm` readout + one secondary `Export…` button on the right. Nothing
//! else floats in the header; the dead titlebar glyphs are gone.
//!
//! Under the deck: the title line (serif, 20 px) with the single quiet `Polish`
//! action at its far right, then the transcript as clean rows — a muted mono
//! timecode gutter (`hh:mm:ss`), serif body ~16 px at a ~68 % measure, a
//! subtle hover, and the active segment as a 3 pt accent edge bar + faint tint.
//! Inline editing keeps its M1 behavior (per-row `TextEdit` + the debounced
//! autosave), restyled only.
//!
//! The export sheet hangs from the deck's `Export…` button (an anchored card),
//! never a bottom band: the old fixed export panel collided with the floating
//! record pill (critique H8/P1), and the pill's position is an M8.5 contract.

use std::time::Instant;

use egui::{pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Rect, Stroke, StrokeKind, Vec2};

use suramak_playback::segment_index_at;

use crate::app::controls::{self, Tier};
use crate::motion::{self, Motion};
use crate::theme::{self, Tokens};

use super::polish::{PolishState, POLISH_SETUP_NOTE};
use super::views::{pane_caption, PANE_PAD_X};
use super::{export, format_transport, AppState, Effect};

/// The deck band's height (workorder §3: ~96 px).
pub const DECK_HEIGHT: f32 = 96.0;
/// The deck's inner horizontal padding.
pub const DECK_PAD_X: f32 = 18.0;
/// The `Export…` button's designed size (the deck's right anchor).
pub const EXPORT_SIZE: Vec2 = Vec2::new(96.0, 30.0);
/// The time readout's reserved width (`mm:ss.mmm / mm:ss.mmm`, mono 12).
pub const READOUT_WIDTH: f32 = 158.0;
/// The transcript's mono timecode gutter width.
pub const GUTTER_WIDTH: f32 = 72.0;
/// The transcript body's measure as a fraction of the available row width
/// (workorder §3: ~66–70 %).
pub const BODY_MEASURE: f32 = 0.68;

/// The deck's final geometry — band + the three anchors, pure and testable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeckLayout {
    pub band: Rect,
    pub transport: Rect,
    pub waveform: Rect,
    pub readout: Rect,
    pub export: Rect,
}

/// Lays out the deck band inside the main pane: transport left, export right,
/// readout just left of export, waveform spanning what remains between them.
pub fn deck_layout(pane: Rect) -> DeckLayout {
    let band = Rect::from_min_size(pane.min, vec2(pane.width(), DECK_HEIGHT));
    let transport = Rect::from_min_size(
        pos2(
            band.left() + DECK_PAD_X,
            band.center().y - controls::TRANSPORT_HEIGHT / 2.0,
        ),
        vec2(controls::TRANSPORT_WIDTH, controls::TRANSPORT_HEIGHT),
    );
    let export = Rect::from_min_size(
        pos2(
            band.right() - DECK_PAD_X - EXPORT_SIZE.x,
            band.center().y - EXPORT_SIZE.y / 2.0,
        ),
        EXPORT_SIZE,
    );
    let readout = Rect::from_min_size(
        pos2(export.left() - 14.0 - READOUT_WIDTH, band.center().y - 9.0),
        vec2(READOUT_WIDTH, 18.0),
    );
    let wave_left = transport.right() + 18.0;
    let wave_right = (readout.left() - 18.0).max(wave_left + 40.0);
    let waveform = Rect::from_min_max(
        pos2(wave_left, band.top() + 16.0),
        pos2(wave_right, band.bottom() - 16.0),
    );
    DeckLayout {
        band,
        transport,
        waveform,
        readout,
        export,
    }
}

pub fn ui(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    let pane = ui.max_rect();
    let deck = deck_layout(pane);

    deck_band(ui, tokens, motion, state, effects, &deck);

    // The export sheet hangs from the deck's button (anchored card, no band).
    if state.export.open {
        export::sheet(ui.ctx(), tokens, motion, state, effects, deck.export);
    }

    // ---- title line: serif title + the single quiet Polish action ----------
    title_line(ui, tokens, motion, state, effects);

    // ---- meta / save / status captions (quiet, under the title) ------------
    meta_captions(ui, tokens, state);

    // ---- polish actions (only when the state has something to say) ---------
    polish_actions(ui, tokens, motion, state, effects);

    // ---- transcript rows ---------------------------------------------------
    if state.polish.is_running() || state.polish.is_reviewing() {
        polish_panel(ui, tokens, state);
        return;
    }
    transcript_rows(ui, tokens, motion, state, effects);
}

/// The deck band: one panel-level surface, hairline-separated, containing the
/// transport capsule, the waveform strip, the readout and `Export…`.
fn deck_band(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
    deck: &DeckLayout,
) {
    let painter = ui.painter().clone();
    painter.rect_filled(deck.band, CornerRadius::ZERO, tokens.panel);
    painter.hline(
        deck.band.x_range(),
        deck.band.bottom() - 0.5,
        Stroke::new(tokens.stroke.strong, tokens.hairline),
    );

    // ---- transport capsule (left) ------------------------------------------
    let playing = state.playback.is_playing();
    let can_play = state.playback.can_play();
    ui.scope_builder(egui::UiBuilder::new().max_rect(deck.transport), |ui| {
        let transport =
            controls::transport(ui, tokens, motion, "deck-transport", playing, can_play);
        if transport.toggle.clicked() {
            effects.extend(state.toggle_playback());
        }
        if transport.skip_start.clicked() {
            effects.extend(state.seek_playback(0));
        }
    });

    // ---- waveform strip (middle) -------------------------------------------
    let total_ms = state.playback.duration_ms();
    let position_ms = state.playback.position_ms();
    let hint = state.playback.hint().map(str::to_string);
    let mut seek_ms: Option<u32> = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(deck.waveform), |ui| {
        let (rect, response) = ui.allocate_exact_size(deck.waveform.size(), egui::Sense::click());
        waveform_strip(ui, rect, tokens, total_ms, position_ms, hint.as_deref());
        if response.hovered() && hint.is_none() && total_ms > 0 {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() && hint.is_none() && total_ms > 0 {
            if let Some(pointer) = response.interact_pointer_pos() {
                seek_ms = Some(waveform_ms_at(rect, pointer.x, total_ms));
            }
        }
    });
    if let Some(ms) = seek_ms {
        effects.extend(state.seek_playback(ms));
    }

    // ---- time readout (right, before Export…) ------------------------------
    painter.text(
        deck.readout.right_center(),
        Align2::RIGHT_CENTER,
        format!(
            "{} / {}",
            format_transport(position_ms),
            format_transport(total_ms)
        ),
        FontId::new(12.0, FontFamily::Monospace),
        tokens.ink_soft,
    );

    // ---- Export… (the one tier-2 action in the header) ---------------------
    ui.scope_builder(egui::UiBuilder::new().max_rect(deck.export), |ui| {
        let response = controls::button_sized(
            ui,
            tokens,
            motion,
            "deck-export",
            "Export…",
            Tier::Secondary,
            state.editor.is_some(),
            EXPORT_SIZE,
        )
        .on_hover_text("Export the transcript (Markdown, text, JSON, SRT, WebVTT)");
        if response.clicked() {
            state.toggle_export();
        }
    });
}

/// The waveform strip: the M1 silhouette (deterministic bars — real peaks are
/// slice 2; the geometry here is final), the moving accent playhead and
/// click-to-seek. An unavailable source replaces the playhead with the reason.
fn waveform_strip(
    ui: &egui::Ui,
    strip: Rect,
    tokens: &Tokens,
    total_ms: u32,
    position_ms: u32,
    unavailable: Option<&str>,
) {
    let painter = ui.painter();
    let bars = strip.shrink2(vec2(2.0, 2.0));
    let center_y = bars.center().y;
    painter.hline(
        bars.x_range(),
        center_y,
        Stroke::new(tokens.stroke.hairline, tokens.hairline),
    );

    let max_half = (bars.height() * 0.5 - 3.0).max(3.0);
    let bar_width = 2.0;
    let pitch = 5.0;
    let mut x = bars.left();
    let mut index = 0usize;
    while x + bar_width <= bars.right() {
        let half = 2.0 + bar_height(index) * max_half;
        let bar = Rect::from_center_size(
            pos2(x + bar_width * 0.5, center_y),
            vec2(bar_width, half * 2.0),
        );
        let color = if index % 9 == 0 {
            tokens.wave_dim
        } else {
            tokens.wave
        };
        painter.rect_filled(bar, CornerRadius::same(1), color);
        x += pitch;
        index += 1;
    }

    if let Some(hint) = unavailable {
        // M8 unavailable state: no playhead, the reason in words.
        painter.text(
            strip.center(),
            Align2::CENTER_CENTER,
            hint,
            FontId::new(11.5, FontFamily::Proportional),
            tokens.ink_soft,
        );
        return;
    }

    if total_ms > 0 {
        let t = (position_ms as f32 / total_ms as f32).clamp(0.0, 1.0);
        let x = waveform_axis_x(bars, t);
        painter.rect_filled(
            Rect::from_min_max(
                pos2(x - 1.0, bars.top() - 1.0),
                pos2(x + 1.0, bars.bottom() + 1.0),
            ),
            CornerRadius::same(1),
            tokens.accent,
        );
        painter.circle_filled(pos2(x, bars.top() - 1.0), 2.5, tokens.accent);
    }
}

/// The strip's time axis: `t` (0..=1) → x. Shared by the playhead and
/// click-to-seek, so the two can never drift apart.
fn waveform_axis_x(bars: Rect, t: f32) -> f32 {
    bars.left() + bars.width() * t
}

/// A click position on the strip → milliseconds (the inverse of the axis).
fn waveform_ms_at(strip: Rect, x: f32, total_ms: u32) -> u32 {
    let bars = strip.shrink2(vec2(2.0, 2.0));
    let t = ((x - bars.left()) / bars.width().max(1.0)).clamp(0.0, 1.0);
    (t * total_ms as f32).round() as u32
}

/// Deterministic pseudo-random bar silhouette (no RNG dependency; M1 generator;
/// slice 2 replaces this with real peaks).
fn bar_height(index: usize) -> f32 {
    let mut hash = (index as u32).wrapping_mul(2_654_435_761);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2c1b_3c6d);
    hash ^= hash >> 12;
    let noise = (hash & 0xffff) as f32 / 65_535.0;
    let swell = 0.45 + 0.55 * ((index as f32) * 0.045).sin().abs();
    (swell * (0.25 + 0.75 * noise)).clamp(0.0, 1.0)
}

/// The title line: serif title at the left, `Polish` (quiet) at the far right,
/// with `Undo` beside it when a polish undo is pending.
fn title_line(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    let (title, can_polish) = match &state.editor {
        Some(editor) => (editor.transcript.title.clone(), state.can_polish()),
        None => ("No transcript open".to_string(), false),
    };
    ui.add_space(tokens.space.m);
    ui.horizontal(|ui| {
        ui.add_space(PANE_PAD_X - 8.0);
        ui.label(
            egui::RichText::new(title)
                .font(FontId::new(20.0, theme::serif_family()))
                .color(tokens.ink),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(PANE_PAD_X - 8.0);
            let undo_available = state.polish_undo.is_some();
            let polish = controls::button(
                ui,
                tokens,
                motion,
                "editor-polish",
                "Polish",
                Tier::Tertiary,
                can_polish,
            )
            .on_hover_text("Tidy up the transcript text with the configured endpoint");
            if polish.clicked() {
                effects.extend(state.start_polish());
            }
            if undo_available {
                let undo = controls::button(
                    ui,
                    tokens,
                    motion,
                    "editor-polish-undo",
                    "Undo",
                    Tier::Tertiary,
                    true,
                )
                .on_hover_text("Undo the accepted polish");
                if undo.clicked() {
                    effects.extend(state.undo_polish());
                }
            }
            // The delete affordance (M4.5, restyled M9a): a painted trash glyph
            // that turns into an in-place confirm step — acting where the user
            // clicked, no OS dialog. The M4.5 emoji (🗑 ✔ ✖) is gone.
            let confirming = state
                .editor
                .as_ref()
                .is_some_and(|editor| editor.confirm_delete);
            let has_editor = state.editor.is_some();
            if confirming {
                // Right-to-left layout: the first added sits rightmost, so this
                // reads "Delete? ✔ ✖" from left to right.
                let cancel = controls::glyph_button(
                    ui,
                    tokens,
                    motion,
                    "editor-delete-cancel",
                    controls::Glyph::Cross,
                    Tier::Tertiary,
                    true,
                    "Keep this transcript",
                );
                let confirm = controls::glyph_button(
                    ui,
                    tokens,
                    motion,
                    "editor-delete-confirm",
                    controls::Glyph::Check,
                    Tier::Tertiary,
                    true,
                    "Delete this transcript",
                );
                ui.label(
                    egui::RichText::new("Delete?")
                        .size(12.0)
                        .color(tokens.ink_soft),
                );
                if cancel.clicked() {
                    if let Some(editor) = state.editor.as_mut() {
                        editor.confirm_delete = false;
                    }
                }
                if confirm.clicked() {
                    if let Some(editor) = state.editor.as_mut() {
                        editor.confirm_delete = false;
                    }
                    effects.extend(state.delete_open_transcript());
                }
            } else if has_editor {
                let trash = controls::glyph_button(
                    ui,
                    tokens,
                    motion,
                    "editor-delete",
                    controls::Glyph::Trash,
                    Tier::Tertiary,
                    true,
                    "Delete this transcript",
                );
                if trash.clicked() {
                    if let Some(editor) = state.editor.as_mut() {
                        editor.confirm_delete = true;
                    }
                }
            }
        });
    });
}

/// The quiet caption block under the title: the transcript meta, the autosave
/// state and the cross-view status line.
fn meta_captions(ui: &mut egui::Ui, tokens: &Tokens, state: &AppState) {
    let (meta, save_state, failed) = match &state.editor {
        Some(editor) => (
            format!(
                "{} · {} · {} segment(s) · {}",
                editor.transcript.model,
                editor.transcript.language.as_deref().unwrap_or("auto"),
                editor.transcript.segments.len(),
                editor.transcript.created_at.format("%d %b %Y %H:%M")
            ),
            editor.save_state().to_string(),
            editor.save_error.is_some(),
        ),
        None => (String::new(), String::new(), false),
    };
    ui.horizontal(|ui| {
        ui.add_space(PANE_PAD_X - 8.0);
        ui.vertical(|ui| {
            if !meta.is_empty() {
                pane_caption(ui, &meta, tokens, false);
            }
            if !save_state.is_empty() {
                pane_caption(ui, &save_state, tokens, failed);
            }
            if let Some(status) = &state.status {
                pane_caption(ui, &status.text, tokens, status.error);
            }
        });
    });
}

/// The polish action row: only drawn when the polish state has something to
/// say (a run in flight, a review, a failure, or the setup note). The idle
/// `Polish` action lives on the title line.
fn polish_actions(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    enum Bar {
        Idle { show_setup_note: bool },
        Running { segments: usize },
        Review { rows: usize },
        Failed { can: bool, message: String },
    }
    let bar = match &state.polish {
        PolishState::Idle => Bar::Idle {
            show_setup_note: !state.polish_configured && state.editor.is_some(),
        },
        PolishState::Running(run) => Bar::Running {
            segments: run.originals.len(),
        },
        PolishState::Review(review) => Bar::Review {
            rows: review.rows.len(),
        },
        PolishState::Failed { message } => Bar::Failed {
            can: state.can_polish(),
            message: message.clone(),
        },
    };
    match bar {
        Bar::Idle { show_setup_note } => {
            if show_setup_note {
                ui.horizontal(|ui| {
                    ui.add_space(PANE_PAD_X - 8.0);
                    pane_caption(ui, POLISH_SETUP_NOTE, tokens, false);
                });
            }
        }
        Bar::Running { segments } => {
            ui.horizontal(|ui| {
                ui.add_space(PANE_PAD_X - 8.0);
                pane_caption(
                    ui,
                    &format!(
                        "polishing {segments} segment(s) — the review opens here when the endpoint answers"
                    ),
                    tokens,
                    false,
                );
            });
        }
        Bar::Review { rows } => {
            ui.horizontal(|ui| {
                ui.add_space(PANE_PAD_X - 8.0);
                let accept = controls::button(
                    ui,
                    tokens,
                    motion,
                    "polish-accept",
                    "Accept",
                    Tier::Secondary,
                    true,
                );
                if accept.clicked() {
                    effects.extend(state.accept_polish());
                }
                let discard = controls::button(
                    ui,
                    tokens,
                    motion,
                    "polish-discard",
                    "Discard",
                    Tier::Tertiary,
                    true,
                );
                if discard.clicked() {
                    state.discard_polish();
                }
                pane_caption(
                    ui,
                    &format!(
                        "reviewing {rows} segment(s) — original | tidied; Accept replaces the transcript text"
                    ),
                    tokens,
                    false,
                );
            });
        }
        Bar::Failed { can, message } => {
            ui.horizontal(|ui| {
                ui.add_space(PANE_PAD_X - 8.0);
                let retry = controls::button(
                    ui,
                    tokens,
                    motion,
                    "polish-retry",
                    "Polish",
                    Tier::Secondary,
                    can,
                );
                if retry.clicked() {
                    effects.extend(state.start_polish());
                }
                let dismiss = controls::button(
                    ui,
                    tokens,
                    motion,
                    "polish-dismiss",
                    "Dismiss",
                    Tier::Tertiary,
                    true,
                );
                if dismiss.clicked() {
                    state.discard_polish();
                }
                pane_caption(ui, &message, tokens, true);
            });
        }
    }
}

/// The transcript as clean rows: mono gutter + serif body at the ~68 % measure,
/// hover wash, active segment = 3 pt accent edge bar + faint tint (the tint
/// crossfades with the fast token), and the row `TextEdit` editing as before.
fn transcript_rows(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    let rows: Vec<(u32, i64)> = state
        .editor
        .as_ref()
        .map(|editor| {
            editor
                .transcript
                .segments
                .iter()
                .map(|seg| (seg.idx, seg.start_ms))
                .collect()
        })
        .unwrap_or_default();

    // M8: the active segment is the one containing the playhead, and the list
    // only follows it **while playing** — a paused playhead never yanks the
    // scroll out from under a reader.
    let position_ms = i64::from(state.playback.position_ms());
    let following = state.playback.is_playing();
    let active = state
        .editor
        .as_ref()
        .and_then(|editor| segment_index_at(&editor.transcript.segments, position_ms));

    let mut seek_to: Option<u32> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(tokens.space.s);
            if rows.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(PANE_PAD_X - 8.0);
                    pane_caption(
                        ui,
                        "No speech segments were detected in this recording.",
                        tokens,
                        false,
                    );
                });
                return;
            }
            let mut edited: Option<(u32, String)> = None;
            let Some(editor) = &mut state.editor else {
                return;
            };
            for (index, (idx, start_ms)) in rows.iter().enumerate() {
                let is_active = active == Some(index);
                let active_f = motion.factor(
                    ui.ctx(),
                    egui::Id::new(("suramak-active-segment", *idx)),
                    is_active,
                    motion::FAST,
                );

                // Reserve the row's background + edge bar + focus ring, drawn
                // BEHIND the content once its rect is known.
                let bg_idx = ui.painter().add(egui::Shape::Noop);
                let bar_idx = ui.painter().add(egui::Shape::Noop);
                let focus_idx = ui.painter().add(egui::Shape::Noop);

                let gutter_w = GUTTER_WIDTH;
                let row_max = ui.max_rect();
                let text_w =
                    ((row_max.width() - PANE_PAD_X - gutter_w - 24.0) * BODY_MEASURE).max(220.0);
                let mut content_rect = Rect::NOTHING;
                let mut row_focused = false;
                ui.horizontal_top(|ui| {
                    ui.add_space(PANE_PAD_X - 8.0);
                    // M8: the timestamp seeks to the segment's start.
                    let stamp_rect =
                        Rect::from_min_size(ui.cursor().min, vec2(gutter_w - 10.0, 20.0));
                    let stamp = ui.allocate_rect(stamp_rect, egui::Sense::click());
                    let stamp_hover = motion.factor(
                        ui.ctx(),
                        egui::Id::new(("suramak-gutter-hover", *idx)),
                        stamp.hovered(),
                        motion::FAST,
                    );
                    ui.painter().text(
                        stamp_rect.left_center(),
                        Align2::LEFT_CENTER,
                        suramak_core::time::fmt_plain(*start_ms),
                        FontId::new(11.5, FontFamily::Monospace),
                        motion::lerp_color(tokens.ink_soft, tokens.ink, stamp_hover),
                    );
                    if stamp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if stamp.clicked() {
                        seek_to = Some((*start_ms).clamp(0, i64::from(u32::MAX)) as u32);
                    }

                    let response = ui.add(
                        egui::TextEdit::multiline(&mut editor.buffers[index])
                            .id_salt(("segment", *idx))
                            .desired_width(text_w)
                            .desired_rows(1)
                            .frame(false)
                            .font(FontId::new(16.0, crate::theme::serif_family()))
                            .text_color(tokens.ink),
                    );
                    if response.changed() {
                        edited = Some((*idx, editor.buffers[index].clone()));
                    }
                    content_rect = response.rect;
                    row_focused = response.has_focus();
                });

                // The row surface: full-width, padded around the content.
                let row_rect = Rect::from_min_max(
                    pos2(row_max.left() + PANE_PAD_X - 10.0, content_rect.top() - 4.0),
                    pos2(row_max.right() - 10.0, content_rect.bottom() + 4.0),
                );
                let hover_resp = ui.interact(
                    row_rect,
                    egui::Id::new(("suramak-row", *idx)),
                    egui::Sense::hover(),
                );
                let hover_f = motion.factor(
                    ui.ctx(),
                    egui::Id::new(("suramak-row-hover", *idx)),
                    hover_resp.hovered() && !row_focused,
                    motion::FAST,
                );
                let fill = motion::tint_over(
                    motion::tint_over(egui::Color32::TRANSPARENT, tokens.surface_hover, hover_f),
                    tokens.accent_tint,
                    active_f,
                );
                let radius = CornerRadius::same(tokens.radius.row);
                if fill.a() > 0 {
                    ui.painter()
                        .set(bg_idx, egui::Shape::rect_filled(row_rect, radius, fill));
                }
                if active_f > 0.01 {
                    let bar = Rect::from_min_size(
                        pos2(row_rect.left() + 2.0, row_rect.top() + 3.0),
                        vec2(3.0, (row_rect.height() - 6.0).max(1.0)),
                    );
                    ui.painter().set(
                        bar_idx,
                        egui::Shape::rect_filled(
                            bar,
                            CornerRadius::same(2),
                            tokens.accent.gamma_multiply(active_f),
                        ),
                    );
                }
                if row_focused {
                    ui.painter().set(
                        focus_idx,
                        egui::Shape::rect_stroke(
                            row_rect,
                            radius,
                            Stroke::new(tokens.stroke.hairline, tokens.accent),
                            StrokeKind::Inside,
                        ),
                    );
                }
                if is_active && following {
                    ui.scroll_to_rect(row_rect, Some(egui::Align::Center));
                }
                ui.add_space(tokens.space.xs);
            }
            if let Some((idx, text)) = edited {
                editor.edit(idx, text, Instant::now());
            }
        });
    if let Some(ms) = seek_to {
        effects.extend(state.seek_playback(ms));
    }
}

/// The polish panel replacing the transcript rows while a run is in flight or a
/// review is open (M4): a calm placeholder for the former, the side-by-side
/// original | tidied review for the latter (batch only — no streaming).
fn polish_panel(ui: &mut egui::Ui, tokens: &Tokens, state: &AppState) {
    let PolishState::Review(review) = &state.polish else {
        if let PolishState::Running(run) = &state.polish {
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                ui.add_space(PANE_PAD_X - 8.0);
                ui.label(
                    egui::RichText::new(format!(
                        "polishing {} segment(s) with the configured endpoint…",
                        run.originals.len()
                    ))
                    .size(13.0)
                    .color(tokens.ink_soft),
                );
            });
        }
        return;
    };

    ui.add_space(8.0);
    // Column captions over the two text columns (timestamp column left blank).
    ui.horizontal(|ui| {
        ui.add_space(PANE_PAD_X - 8.0);
        ui.add_sized([GUTTER_WIDTH, 16.0], egui::Label::new(""));
        ui.columns(2, |columns| {
            columns[0].label(
                egui::RichText::new("original")
                    .size(11.0)
                    .color(tokens.ink_soft),
            );
            columns[1].label(
                egui::RichText::new("tidied")
                    .size(11.0)
                    .color(tokens.ink_soft),
            );
        });
    });

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for row in &review.rows {
                let start_ms = state
                    .editor
                    .as_ref()
                    .and_then(|editor| {
                        editor
                            .transcript
                            .segments
                            .iter()
                            .find(|segment| segment.idx == row.idx)
                    })
                    .map(|segment| segment.start_ms)
                    .unwrap_or(0);
                ui.horizontal_top(|ui| {
                    ui.add_space(PANE_PAD_X - 8.0);
                    ui.add_sized(
                        [GUTTER_WIDTH, 18.0],
                        egui::Label::new(
                            egui::RichText::new(suramak_core::time::fmt_plain(start_ms))
                                .monospace()
                                .size(11.5)
                                .color(tokens.ink_soft),
                        ),
                    );
                    ui.columns(2, |columns| {
                        columns[0].label(
                            egui::RichText::new(&row.original)
                                .size(12.5)
                                .color(tokens.ink_soft),
                        );
                        columns[1].label(
                            egui::RichText::new(&row.polished)
                                .size(14.0)
                                .family(crate::theme::serif_family())
                                .color(tokens.ink),
                        );
                    });
                });
                ui.add_space(6.0);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::controls;

    fn demo_pane() -> Rect {
        // A 1280x800 window: sidebar 23 % (294.4), titlebar 46.
        Rect::from_min_size(pos2(294.4, 46.0), vec2(985.6, 754.0))
    }

    #[test]
    fn deck_geometry_anchors_transport_readout_and_export() {
        let pane = demo_pane();
        let deck = deck_layout(pane);

        // The band: full pane width, the designed height, directly under the
        // titlebar (the pane's top edge).
        assert_eq!(deck.band.top(), pane.top());
        assert_eq!(deck.band.width(), pane.width());
        assert_eq!(deck.band.height(), DECK_HEIGHT);

        // Transport: left-anchored at the deck pad, the capsule size.
        assert_eq!(deck.transport.left(), pane.left() + DECK_PAD_X);
        assert_eq!(
            deck.transport.size(),
            vec2(controls::TRANSPORT_WIDTH, controls::TRANSPORT_HEIGHT)
        );
        assert_eq!(deck.transport.center().y, deck.band.center().y);

        // Export: right-anchored at the deck pad, the designed button size.
        assert_eq!(deck.export.right(), pane.right() - DECK_PAD_X);
        assert_eq!(deck.export.size(), EXPORT_SIZE);
        assert_eq!(deck.export.center().y, deck.band.center().y);

        // Readout: sits between the waveform and the export button.
        assert!(deck.readout.right() <= deck.export.left());
        assert!(deck.readout.left() > deck.transport.right());
        assert_eq!(deck.readout.width(), READOUT_WIDTH);

        // Waveform: the middle band, after the capsule, before the readout.
        assert!(deck.waveform.left() > deck.transport.right());
        assert!(deck.waveform.right() < deck.readout.left());
        assert!(deck.waveform.width() > 200.0);
        assert!(deck.waveform.height() > 20.0);

        // Every anchor lives inside the band.
        for anchor in [deck.transport, deck.waveform, deck.readout, deck.export] {
            assert!(
                deck.band.contains_rect(anchor),
                "{anchor:?} must sit inside the band"
            );
        }
    }

    #[test]
    fn deck_geometry_survives_the_minimum_window() {
        // The 900x600 minimum: sidebar 23 % (207), titlebar 46.
        let pane = Rect::from_min_size(pos2(207.0, 46.0), vec2(693.0, 554.0));
        let deck = deck_layout(pane);
        assert!(deck.waveform.width() >= 40.0, "the strip never collapses");
        assert!(deck.waveform.right() < deck.readout.left());
        assert!(deck.transport.right() < deck.waveform.left());
    }

    #[test]
    fn waveform_axis_round_trips_through_click_to_seek() {
        let strip = Rect::from_min_size(pos2(406.0, 62.0), vec2(570.0, 64.0));
        let bars = strip.shrink2(vec2(2.0, 2.0));
        assert_eq!(waveform_axis_x(bars, 0.0), bars.left());
        assert_eq!(waveform_axis_x(bars, 1.0), bars.right());
        // A click at the axis midpoint seeks to the middle of the duration.
        let x = waveform_axis_x(bars, 0.5);
        assert_eq!(waveform_ms_at(strip, x, 18_000), 9_000);
        // Clamped outside the axis.
        assert_eq!(waveform_ms_at(strip, bars.left() - 50.0, 18_000), 0);
        assert_eq!(waveform_ms_at(strip, bars.right() + 50.0, 18_000), 18_000);
    }
}
