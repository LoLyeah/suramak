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
use suramak_playback::Peaks;

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
/// The deck's one bottom separator sits this far above the band's bottom edge.
pub const DECK_SEPARATOR_OFFSET: f32 = 0.5;

// ---- the header's explicit vertical rhythm (owner-directed fix 2) ----------
//
// The editor header below the deck is positioned ENTIRELY from these pixel
// constants — never from a measured galley height and never from an
// `add_space` after measured content. Root cause of the owner's macOS defect:
// the block used to flow from the measured heights of the serif title and the
// proportional captions, so under the real system fonts (New York / SF Pro)
// the rows shifted a few pixels while the deck's separator sat at a fixed
// offset — and the caption zone collided with the rule. Explicit rows make
// that class impossible on every platform, whatever the fonts measure.

/// The deck separator → title row gap.
pub const HEADER_TITLE_GAP: f32 = 6.0;
/// The title row's constant height (holds the 20 pt serif title and the
/// capsule action cluster; nothing below depends on what it measures).
pub const HEADER_TITLE_ROW: f32 = 32.0;
/// The title row → caption block gap.
pub const HEADER_CAPTION_GAP: f32 = 2.0;
/// One caption row's constant height (meta / save state / status).
pub const HEADER_CAPTION_ROW: f32 = 18.0;
/// How many caption rows the block reserves (meta, save, status).
pub const HEADER_CAPTION_ROWS: usize = 3;
/// The title's right reserve for the action cluster (trash + Polish + Undo +
/// the confirm step); a longer title truncates with an ellipsis instead of
/// pushing the cluster or growing the row.
pub const HEADER_CLUSTER_RESERVE: f32 = 280.0;

/// The header zone's explicit geometry: the one rule's y, the title row and
/// the caption block. Pure, unit-tested (the macOS-collision fix).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeaderLayout {
    /// The deck's bottom separator (the ONLY horizontal rule in the header).
    pub separator_y: f32,
    /// The title row (serif title + the capsule cluster).
    pub title: Rect,
    /// The caption block (3 constant rows: meta / save state / status).
    pub captions: Rect,
}

/// Lays out the header zone below the deck from the constants above.
pub fn header_layout(pane: Rect) -> HeaderLayout {
    let separator_y = pane.top() + DECK_HEIGHT - DECK_SEPARATOR_OFFSET;
    let title = Rect::from_min_size(
        pos2(pane.left(), pane.top() + DECK_HEIGHT + HEADER_TITLE_GAP),
        vec2(pane.width(), HEADER_TITLE_ROW),
    );
    let captions = Rect::from_min_size(
        pos2(pane.left(), title.bottom() + HEADER_CAPTION_GAP),
        vec2(
            pane.width(),
            HEADER_CAPTION_ROW * HEADER_CAPTION_ROWS as f32,
        ),
    );
    HeaderLayout {
        separator_y,
        title,
        captions,
    }
}

/// One caption row's rect: stacked from the explicit row height, zero overlap
/// (row `i + 1` starts exactly where row `i` ends).
pub fn header_caption_rect(header: &HeaderLayout, row: usize) -> Rect {
    let row = row.min(HEADER_CAPTION_ROWS - 1);
    Rect::from_min_size(
        pos2(
            header.captions.left(),
            header.captions.top() + HEADER_CAPTION_ROW * row as f32,
        ),
        vec2(header.captions.width(), HEADER_CAPTION_ROW),
    )
}

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
    // The header's rhythm comes from the constants — never from the flow
    // cursor's measured content (owner-directed fix 2; see `header_layout`).
    let header = header_layout(pane);

    deck_band(ui, tokens, motion, state, effects, &deck);

    // The export sheet hangs from the deck's button (anchored card, no band).
    // It draws itself only while open or mid-close (M9c: base enter / fast exit).
    export::sheet(ui.ctx(), tokens, motion, state, effects, deck.export);

    // ---- title line: serif title + the capsule action cluster --------------
    title_line(ui, tokens, motion, state, effects, header.title);

    // ---- meta / save / status captions (constant rows under the title) -----
    meta_captions(ui, tokens, motion, state, &header);

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
    // The header's ONE horizontal rule: the deck's bottom separator, full pane
    // width, at the constant offset (`header_layout` uses the same constant).
    painter.hline(
        deck.band.x_range(),
        deck.band.bottom() - DECK_SEPARATOR_OFFSET,
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
    // M9d item 3: what the strip **draws** eases to a seek (base token) — the
    // data/seek math above is untouched (the readout and the transport keep
    // reading the true `position_ms`; `waveform_ms_at` is unchanged).
    let drawn_ms = motion.seek_ease(
        ui.ctx(),
        egui::Id::new("suramak-playhead-ease"),
        position_ms as f32,
        motion::BASE,
    );
    let hint = state.playback.hint().map(str::to_string);
    let peaks = state.strip_peaks();
    let transcript_id = state.editor.as_ref().map(|editor| editor.transcript.id);
    let mut seek_ms: Option<u32> = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(deck.waveform), |ui| {
        let (rect, response) = ui.allocate_exact_size(deck.waveform.size(), egui::Sense::click());
        let can_seek = hint.is_none() && total_ms > 0;
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Other,
                can_seek,
                "Waveform — click to seek",
            )
        });
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.expand(1.5),
                CornerRadius::same(tokens.radius.control.saturating_add(2)),
                Stroke::new(tokens.stroke.focus, tokens.accent),
                StrokeKind::Outside,
            );
        }
        // M9b motion: the real peaks fade in once, on their first paint (base
        // token; a snap under Reduce Motion). No growth animation.
        let peaks_fade = match (transcript_id, peaks) {
            (Some(id), Some(_)) => motion.arrive(
                ui.ctx(),
                egui::Id::new(("suramak-peaks-fade", id)),
                motion::BASE,
            ),
            _ => 1.0,
        };
        waveform_strip(
            ui,
            rect,
            tokens,
            total_ms,
            drawn_ms,
            hint.as_deref(),
            peaks,
            peaks_fade,
        );
        if response.hovered() && can_seek {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() && can_seek {
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

/// The waveform strip: the **real** min/max envelope (M9b) in the frozen M9a
/// geometry — same band, same bar pitch, same playhead and click-to-seek. A
/// source with no envelope (no audio, or the decode still in flight) draws a
/// quiet flat baseline — never the old deterministic silhouette.
///
/// `drawn_ms` is the **eased** position ([`Motion::seek_ease`], M9d item 3);
/// the data position lives on `Playback` and drives the readout.
#[allow(clippy::too_many_arguments)]
fn waveform_strip(
    ui: &egui::Ui,
    strip: Rect,
    tokens: &Tokens,
    total_ms: u32,
    drawn_ms: f32,
    unavailable: Option<&str>,
    peaks: Option<&Peaks>,
    peaks_fade: f32,
) {
    let painter = ui.painter();
    let bars = strip.shrink2(vec2(2.0, 2.0));
    let center_y = bars.center().y;

    let max_half = (bars.height() * 0.5 - 3.0).max(3.0);
    let bar_width = 2.0;
    let pitch = 5.0;

    match peaks {
        Some(peaks) => {
            painter.hline(
                bars.x_range(),
                center_y,
                Stroke::new(tokens.stroke.hairline, tokens.hairline),
            );
            let mut x = bars.left();
            let mut index = 0usize;
            while x + bar_width <= bars.right() {
                // Each bar draws the envelope over the time span it covers (the
                // normalized min/max of every bucket it touches).
                let t0 = (x - bars.left()) / bars.width().max(1.0);
                let t1 = (x + bar_width - bars.left()) / bars.width().max(1.0);
                let (min, max) = peaks.range(t0, t1);
                if min != 0.0 || max != 0.0 {
                    let top = center_y - max.clamp(0.0, 1.0) * max_half;
                    let bottom = center_y - min.clamp(-1.0, 0.0) * max_half;
                    // A non-silent span never draws thinner than one pixel
                    // (legibility, not invented data).
                    let (top, bottom) = if bottom - top < 1.0 {
                        let mid = (top + bottom) * 0.5;
                        (mid - 0.5, mid + 0.5)
                    } else {
                        (top, bottom)
                    };
                    let bar = Rect::from_min_max(pos2(x, top), pos2(x + bar_width, bottom));
                    let color = if index % 9 == 0 {
                        tokens.wave_dim
                    } else {
                        tokens.wave
                    };
                    painter.rect_filled(
                        bar,
                        CornerRadius::same(1),
                        color.gamma_multiply(peaks_fade),
                    );
                }
                x += pitch;
                index += 1;
            }
        }
        None => {
            // The honest flat baseline: a quiet line at zero amplitude. No
            // silhouette, no stubs — a source without audio has no waveform.
            painter.hline(
                bars.x_range(),
                center_y,
                Stroke::new(1.5_f32, tokens.wave_dim.gamma_multiply(0.7)),
            );
        }
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
        let t = (drawn_ms / total_ms as f32).clamp(0.0, 1.0);
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

/// The follow-scroll's scroll animation (M9d item 3): the transcript's
/// active-row follow runs over the **base token** and ends exactly at the
/// target; Reduce Motion is a snap (`ScrollAnimation::none()`). egui's scroll
/// machinery carries the interruption discipline for free: a retarget
/// mid-flight updates the target and continues from the current offset, and a
/// user drag cancels it — nothing queues, nothing snaps back.
fn follow_scroll_animation(motion: &Motion) -> egui::style::ScrollAnimation {
    if motion.reduce {
        egui::style::ScrollAnimation::none()
    } else {
        egui::style::ScrollAnimation::duration(motion::BASE)
    }
}

/// The title line: the serif title painted into a **constant-height row**
/// (one-row ellipsis truncation — the row never measures the text) and, right
/// aligned in the same row, the action cluster as capsule buttons
/// (owner-directed fix 1, 2026-10-14: trash / Polish / Undo read as real
/// buttons — translucent capsule bodies, ≥ space.s padding, ≥ space.s gaps).
fn title_line(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
    row: Rect,
) {
    let (title, can_polish) = match &state.editor {
        Some(editor) => (editor.transcript.title.clone(), state.can_polish()),
        None => ("No transcript open".to_string(), false),
    };

    // The title: a single truncated row, centred in the constant row rect.
    let title_left = row.left() + PANE_PAD_X - 8.0;
    let max_width = (row.width() - (PANE_PAD_X - 8.0) * 2.0 - HEADER_CLUSTER_RESERVE).max(120.0);
    let galley = title_galley(ui, tokens, &title, max_width);
    ui.painter().galley(
        pos2(title_left, row.center().y - galley.size().y / 2.0),
        galley,
        tokens.ink,
    );

    // The action cluster: right-aligned, vertically centred, gaps at space.s.
    ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = tokens.space.s;
            ui.add_space(PANE_PAD_X - 8.0);
            let undo_available = state.polish_undo.is_some();
            let polish =
                controls::capsule_button(ui, tokens, motion, "editor-polish", "Polish", can_polish)
                    .on_hover_text("Tidy up the transcript text with the configured endpoint");
            if polish.clicked() {
                effects.extend(state.start_polish());
            }
            if undo_available {
                let undo = controls::capsule_button(
                    ui,
                    tokens,
                    motion,
                    "editor-polish-undo",
                    "Undo",
                    true,
                )
                .on_hover_text("Undo the accepted polish");
                if undo.clicked() {
                    effects.extend(state.undo_polish());
                }
            }
            // The delete affordance (M4.5, M9c capsule treatment): a painted
            // trash glyph that turns into an in-place confirm step — acting
            // where the user clicked, no OS dialog. The confirm step wears the
            // same capsule body (a trivially consistent tweak of the owner's
            // direction; the M4.5 emoji set is long gone).
            let confirming = state
                .editor
                .as_ref()
                .is_some_and(|editor| editor.confirm_delete);
            let has_editor = state.editor.is_some();
            if confirming {
                // Right-to-left layout: the first added sits rightmost, so this
                // reads "Delete? ✔ ✖" from left to right.
                let cancel = controls::capsule_glyph_button(
                    ui,
                    tokens,
                    motion,
                    "editor-delete-cancel",
                    controls::Glyph::Cross,
                    true,
                    "Keep this transcript",
                );
                let confirm = controls::capsule_glyph_button(
                    ui,
                    tokens,
                    motion,
                    "editor-delete-confirm",
                    controls::Glyph::Check,
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
                let trash = controls::capsule_glyph_button(
                    ui,
                    tokens,
                    motion,
                    "editor-delete",
                    controls::Glyph::Trash,
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

/// A one-row, ellipsis-truncated serif galley for the title — the title row's
/// height is a constant, so a long title can never grow it or push the
/// cluster (owner-directed fix 2).
fn title_galley(
    ui: &egui::Ui,
    tokens: &Tokens,
    text: &str,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat {
            font_id: FontId::new(20.0, theme::serif_family()),
            color: tokens.ink,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping {
        max_width,
        max_rows: 1,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    ui.ctx().fonts_mut(|fonts| fonts.layout_job(job))
}

/// The quiet captions under the title — the transcript meta, the autosave
/// state and the cross-view status line — each in its own **constant row**
/// (explicit heights; a row never moves another). The status line fades in
/// over base / out over fast (M9c).
fn meta_captions(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &AppState,
    header: &HeaderLayout,
) {
    let (meta, save_state, failed) = match &state.editor {
        Some(editor) => (
            format!(
                "{} · {} · {} · {}",
                super::model_manager::display_name(&editor.transcript.model),
                editor.transcript.language.as_deref().unwrap_or("auto"),
                super::count_noun(editor.transcript.segments.len(), "segment"),
                editor.transcript.created_at.format("%d %b %Y %H:%M")
            ),
            editor.save_state().to_string(),
            editor.save_error.is_some(),
        ),
        None => (String::new(), String::new(), false),
    };
    let rows = [
        (meta, false),
        (save_state, failed),
        (
            state
                .status
                .as_ref()
                .map(|s| s.text.clone())
                .unwrap_or_default(),
            state.status.as_ref().is_some_and(|s| s.error),
        ),
    ];
    for (index, (text, error)) in rows.into_iter().enumerate() {
        let rect = header_caption_rect(header, index);
        if index == 2 {
            // The cross-view status line: fades in base / out fast, and the
            // stale text lingers while it fades (views::status_caption).
            let message = if text.is_empty() {
                None
            } else {
                Some((text.as_str(), error))
            };
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(PANE_PAD_X - 8.0);
                    super::views::status_caption(ui, tokens, motion, "editor-status", message);
                });
            });
            continue;
        }
        if text.is_empty() {
            continue;
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.horizontal(|ui| {
                ui.add_space(PANE_PAD_X - 8.0);
                pane_caption(ui, &text, tokens, error);
            });
        });
    }
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
        Review { rows: usize, notes: Option<String> },
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
            notes: review.notes_line(),
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
                        "polishing {} — the review opens here when the endpoint answers",
                        super::count_noun(segments, "segment")
                    ),
                    tokens,
                    false,
                );
            });
        }
        Bar::Review { rows, notes } => {
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
                // M11: the run's degradation notes (missing recording,
                // rejected audio) ride the same caption as every polish
                // message.
                let mut caption = format!(
                    "reviewing {} — original | tidied; Accept replaces the transcript text",
                    super::count_noun(rows, "segment")
                );
                if let Some(notes) = &notes {
                    caption.push_str(" · ");
                    caption.push_str(notes);
                }
                pane_caption(ui, &caption, tokens, false);
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
                    // M9b a11y: the painted timestamp carries its semantic label.
                    stamp.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            format!("Seek to {}", suramak_core::time::fmt_plain(*start_ms)),
                        )
                    });
                    if stamp.has_focus() {
                        ui.painter().rect_stroke(
                            stamp_rect.expand(1.0),
                            CornerRadius::same(3),
                            Stroke::new(tokens.stroke.focus, tokens.accent),
                            StrokeKind::Outside,
                        );
                    }
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
                    // M9d item 3: the active-row follow animates — an eased
                    // scroll offset over the base token that ENDS, instead of
                    // an instant `scroll_to_rect` snap; Reduce Motion snaps.
                    ui.scroll_to_rect_animation(
                        row_rect,
                        Some(egui::Align::Center),
                        follow_scroll_animation(motion),
                    );
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
                        "polishing {} with the configured endpoint…",
                        super::count_noun(run.originals.len(), "segment")
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

    // ---- M9d item 3: the follow scroll -------------------------------------

    /// The animation seam: the base token and a fixed duration normally, a
    /// zero-duration snap under Reduce Motion.
    #[test]
    fn follow_scroll_animation_is_the_base_token_or_a_snap() {
        let full = follow_scroll_animation(&crate::motion::Motion::new(false));
        assert_eq!(
            full.duration,
            egui::Rangef::new(crate::motion::BASE, crate::motion::BASE)
        );
        let reduced = follow_scroll_animation(&crate::motion::Motion::new(true));
        assert_eq!(reduced.duration, egui::Rangef::new(0.0, 0.0));
        assert!(
            reduced.points_per_second.is_infinite(),
            "a snap, not a speed"
        );
    }

    /// Frame-level: a real ScrollArea whose follow targets a far row moves the
    /// offset gradually (several intermediate frames — an ease, not a snap) and
    /// ENDS exactly at the target; under Reduce Motion the offset is already at
    /// the target on the first frame that applies it.
    #[test]
    fn the_follow_scroll_animates_and_ends_or_snaps_under_reduce_motion() {
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 240.0));
        let run =
            |ctx: &egui::Context, time: f64, motion: &crate::motion::Motion, follow: bool| -> f32 {
                let mut offset = f32::NAN;
                let input = egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(time),
                    ..Default::default()
                };
                let _ = ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let out = egui::ScrollArea::vertical().show(ui, |ui| {
                            for index in 0..30 {
                                let (rect, _) =
                                    ui.allocate_exact_size(vec2(200.0, 60.0), egui::Sense::hover());
                                if follow && index == 29 {
                                    ui.scroll_to_rect_animation(
                                        rect,
                                        Some(egui::Align::Center),
                                        follow_scroll_animation(motion),
                                    );
                                }
                            }
                        });
                        offset = out.state.offset.y;
                    });
                });
                offset
            };

        // Settle at the top, then follow the last row.
        let ctx = egui::Context::default();
        let motion = crate::motion::Motion::new(false);
        assert_eq!(run(&ctx, 0.0, &motion, false), 0.0);
        // The first follow frame only *requests* the scroll; movement starts on
        // the next frame and eases across several.
        let mut time = 0.02;
        let mut moving_frames = 0;
        let mut previous = run(&ctx, time, &motion, true);
        for _ in 0..20 {
            time += 0.016;
            let offset = run(&ctx, time, &motion, true);
            if offset > previous + 0.5 {
                moving_frames += 1;
            }
            previous = offset;
        }
        assert!(
            moving_frames >= 3,
            "the follow eases over several frames, not a snap: {moving_frames}"
        );
        // It ENDS: the offset is stable across late frames…
        let end = run(&ctx, 5.0, &motion, true);
        assert_eq!(run(&ctx, 5.1, &motion, true), end, "the follow ends");
        // …and the last row is actually in view (the offset really moved).
        assert!(end > 500.0, "the list scrolled to the last row: {end}");

        // Reduce Motion: the very first frame that applies the request is
        // already at the target — a snap.
        let reduced_ctx = egui::Context::default();
        let reduced = crate::motion::Motion::new(true);
        assert_eq!(run(&reduced_ctx, 0.0, &reduced, false), 0.0);
        let _request = run(&reduced_ctx, 0.016, &reduced, true);
        let snapped = run(&reduced_ctx, 0.032, &reduced, true);
        assert!(
            (snapped - end).abs() < 0.5,
            "Reduce Motion snaps to the target: {snapped} vs {end}"
        );
    }

    // ---- M9c: the header's explicit rhythm (owner-directed fix 2) -----------

    /// The deck separator never renders within
    /// `[title_row_top − 2, caption_block_bottom + 2]` — the exact band the
    /// owner's macOS screenshot showed a rule inside.
    #[test]
    fn the_header_separator_never_enters_the_title_or_caption_zone() {
        for pane in [
            demo_pane(),
            Rect::from_min_size(pos2(207.0, 46.0), vec2(693.0, 554.0)),
            Rect::from_min_size(pos2(10.0, 200.0), vec2(900.0, 700.0)),
        ] {
            let header = header_layout(pane);
            let forbidden_top = header.title.top() - 2.0;
            let forbidden_bottom = header.captions.bottom() + 2.0;
            assert!(
                header.separator_y <= forbidden_top,
                "the separator ({}) must sit above the title row ({}): forbidden band {forbidden_top}..{forbidden_bottom}",
                header.separator_y,
                header.title.top()
            );
            assert!(
                !(header.separator_y > forbidden_top && header.separator_y < forbidden_bottom),
                "the separator must never render inside the title/caption zone"
            );
        }
    }

    /// Caption rows stack from the explicit row height: contiguous, zero
    /// overlap, and the block is exactly `ROWS × ROW_HEIGHT` tall.
    #[test]
    fn caption_rows_stack_from_explicit_heights_with_zero_overlap() {
        let header = header_layout(demo_pane());
        for row in 0..HEADER_CAPTION_ROWS {
            let rect = header_caption_rect(&header, row);
            assert_eq!(rect.height(), HEADER_CAPTION_ROW, "row {row} height");
            assert_eq!(rect.left(), header.captions.left());
            assert_eq!(rect.width(), header.captions.width());
            if row > 0 {
                let previous = header_caption_rect(&header, row - 1);
                assert_eq!(rect.top(), previous.bottom(), "row {row} is contiguous");
                assert_eq!(
                    rect.intersect(previous).area(),
                    0.0,
                    "row {row} must not overlap (shared edges are contiguous, not overlap)"
                );
            }
        }
        assert_eq!(
            header.captions.height(),
            HEADER_CAPTION_ROW * HEADER_CAPTION_ROWS as f32
        );
        // The block sits directly under the title row at the constant gap.
        assert_eq!(
            header.captions.top() - header.title.bottom(),
            HEADER_CAPTION_GAP
        );
    }

    /// The rhythm is pure geometry: identical offsets from the pane's top for
    /// every pane position and width (nothing here measures text).
    #[test]
    fn the_header_rhythm_is_constant_across_panes() {
        let a = header_layout(demo_pane());
        let b = header_layout(Rect::from_min_size(pos2(10.0, 200.0), vec2(800.0, 600.0)));
        assert_eq!(a.title.top() - demo_pane().top(), b.title.top() - 200.0);
        assert_eq!(a.title.height(), HEADER_TITLE_ROW);
        assert_eq!(
            a.captions.top() - a.title.bottom(),
            b.captions.top() - b.title.bottom()
        );
        assert_eq!(a.separator_y - demo_pane().top(), b.separator_y - 200.0);
    }

    /// The header-zone audit (owner-directed fix 2b): a real editor frame
    /// paints exactly ONE full-pane-width horizontal rule in the header zone —
    /// the deck's bottom separator, at its constant y.
    #[test]
    fn the_header_zone_has_exactly_one_full_width_rule() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let tokens = crate::theme::tokens(crate::theme::AppTheme::Light);
        let motion = crate::motion::Motion::new(false);
        let mut state = demo_state("header-rule");
        let mut effects = Vec::new();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    super::ui(ui, &tokens, &motion, &mut state, &mut effects);
                });
        });

        let header = header_layout(screen);
        let zone_bottom = header.captions.bottom() + 2.0;
        let mut rules: Vec<Rect> = Vec::new();
        for clipped in &output.shapes {
            let bounds = clipped.shape.visual_bounding_rect();
            if !bounds.is_finite() || bounds.is_negative() {
                continue;
            }
            // Line-like (≤ 2 pt tall) and full-content-width (≥ 80 % pane).
            let line_like = bounds.height() <= 2.0 && bounds.width() >= screen.width() * 0.8;
            if line_like && bounds.center().y >= screen.top() && bounds.center().y <= zone_bottom {
                rules.push(bounds);
            }
        }
        assert_eq!(
            rules.len(),
            1,
            "exactly one full-width rule may exist in the header zone: {rules:?}"
        );
        let rule = rules[0];
        assert!(
            (rule.center().y - header.separator_y).abs() <= 1.0,
            "the one rule is the deck separator at its constant y ({} vs {})",
            rule.center().y,
            header.separator_y
        );
        assert!(
            rule.center().y <= header.title.top() - 2.0,
            "the one rule never enters the title/caption zone"
        );
    }

    /// Owner-directed fix 1: the title-line cluster's painted geometry — the
    /// trash and Polish capsules are separate bodies with ≥ space.s between
    /// them, no two cluster controls overlap, nothing paints across the gap,
    /// and every shape stays inside its control's rect (ring allowance only).
    #[test]
    fn the_title_line_cluster_has_a_real_gap_and_no_overlap() {
        crate::app::bounds::set_enabled(true);
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let tokens = crate::theme::tokens(crate::theme::AppTheme::Light);
        let motion = crate::motion::Motion::new(false);
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0));
        let row = header_layout(screen).title;
        let allow = controls::FOCUS_RING_EXPAND + tokens.stroke.focus;

        let run = |state: &mut crate::app::AppState| -> Vec<crate::app::bounds::ControlRect> {
            let mut effects = Vec::new();
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let output = ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        super::title_line(ui, &tokens, &motion, state, &mut effects, row);
                    });
            });
            let records = crate::app::bounds::take(&ctx);
            // Every painted shape either belongs to a cluster control (inside
            // its rect + the ring allowance) or is the title (left of the
            // cluster); nothing may span the trash ↔ Polish gap.
            let mut cluster: Vec<Rect> = records.iter().map(|record| record.rect).collect();
            cluster.sort_by(|a, b| a.left().partial_cmp(&b.left()).unwrap());
            let trash = *cluster.first().expect("the trash capsule");
            let polish = *cluster.last().expect("the Polish capsule");
            for clipped in &output.shapes {
                let bounds = clipped.shape.visual_bounding_rect();
                if !bounds.is_finite() || bounds.is_negative() {
                    continue;
                }
                let in_trash = bounds.intersects(trash.expand(0.01));
                let in_polish = bounds.intersects(polish.expand(0.01));
                assert!(
                    !(in_trash && in_polish),
                    "a shape spans the trash ↔ Polish gap: {bounds:?}"
                );
                if in_trash {
                    assert!(
                        trash.expand(allow).contains_rect(bounds),
                        "trash shape escapes its body: {bounds:?}"
                    );
                }
                if in_polish {
                    assert!(
                        polish.expand(allow).contains_rect(bounds),
                        "Polish shape escapes its body: {bounds:?}"
                    );
                }
                assert!(
                    row.expand(allow).contains_rect(bounds),
                    "title-line shape escapes the row: {bounds:?}"
                );
            }
            records
        };

        // No undo: exactly the trash + Polish capsules.
        let mut state = demo_state("titleline");
        let records = run(&mut state);
        let mut cluster: Vec<Rect> = records.iter().map(|record| record.rect).collect();
        cluster.sort_by(|a, b| a.left().partial_cmp(&b.left()).unwrap());
        assert_eq!(cluster.len(), 2, "trash + Polish: {cluster:?}");
        let trash = cluster[0];
        let polish = cluster[1];
        let gap = polish.left() - trash.right();
        assert!(
            gap >= tokens.space.s - 0.01,
            "the painted trash ↔ Polish gap ({gap} pt) must be ≥ space.s ({})",
            tokens.space.s
        );
        assert!(!trash.intersects(polish), "the two bodies never overlap");
        // Both capsules are real bodies at the designed height.
        for rect in [trash, polish] {
            assert!(
                (28.0..=30.0).contains(&rect.height()),
                "capsule height: {rect:?}"
            );
        }

        // With a pending polish undo the cluster is trash + Undo + Polish —
        // still pairwise non-overlapping, every gap ≥ space.s.
        let mut undo_state = demo_state("titleline-undo");
        undo_state.polish_undo = Some(super::super::polish::PolishUndo {
            previous: vec![(0, "before".to_string())],
            segments: 1,
        });
        let records = run(&mut undo_state);
        let mut cluster: Vec<Rect> = records.iter().map(|record| record.rect).collect();
        cluster.sort_by(|a, b| a.left().partial_cmp(&b.left()).unwrap());
        assert_eq!(cluster.len(), 3, "trash + Undo + Polish: {cluster:?}");
        for pair in cluster.windows(2) {
            assert!(
                !pair[0].intersects(pair[1]),
                "cluster controls never overlap: {pair:?}"
            );
            let gap = pair[1].left() - pair[0].right();
            assert!(
                gap >= tokens.space.s - 0.01,
                "cluster gap {gap} must be ≥ space.s"
            );
        }
        crate::app::bounds::set_enabled(false);
    }

    /// The editor's a11y tree (M9b) — kept as the shared-state regression.
    fn demo_state(tag: &str) -> crate::app::AppState {
        let dir = std::env::temp_dir().join(format!("suramak-a11y-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let wav = dir.join(format!("{tag}.wav"));
        std::fs::write(&wav, b"RIFF....").expect("temp wav");
        let mut state = crate::app::AppState::new();
        state.on_transcript_loaded(suramak_core::transcript::Transcript::new(
            "a11y.wav".to_string(),
            suramak_core::transcript::Source::File {
                name: "a11y.wav".to_string(),
            },
            "tiny".to_string(),
            Some("en".to_string()),
            Some(wav),
            vec![suramak_core::transcript::Segment {
                idx: 0,
                start_ms: 0,
                end_ms: 1_900,
                text: "hello".to_string(),
            }],
        ));
        state
    }

    /// M9b a11y floor: a full editor frame with accesskit enabled must expose
    /// every painted control as a node carrying its semantic label — this is
    /// the tree VoiceOver / AT-SPI reads.
    #[test]
    fn the_editor_exposes_labelled_accesskit_nodes() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        // The editor draws in the serif family (registered by `install_fonts`);
        // a bare test context has no font families at all.
        crate::theme::install_fonts(&ctx);
        let tokens = crate::theme::tokens(crate::theme::AppTheme::Light);
        let motion = crate::motion::Motion::new(false);
        let mut state = demo_state("a11y-tree");

        let mut effects = Vec::new();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0))),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                super::ui(ui, &tokens, &motion, &mut state, &mut effects);
            });
        });

        let update = output
            .platform_output
            .accesskit_update
            .expect("an accesskit tree update");
        let labels: Vec<String> = update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label().map(str::to_string))
            .collect();
        for expected in [
            "Waveform — click to seek",
            "Skip to start",
            "Play",
            "Export…",
            "Polish",
            "Delete this transcript",
            "Seek to 00:00:00",
        ] {
            assert!(
                labels.iter().any(|label| label == expected),
                "the editor's a11y tree must carry {expected:?}; got {labels:?}"
            );
        }
    }
}
