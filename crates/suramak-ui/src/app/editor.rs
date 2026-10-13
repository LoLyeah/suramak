//! Editor route: the editable transcript — per-segment row = mono timestamp
//! label + `TextEdit`, autosave via the debounced dirty tracking in
//! `super::Editor`, the "saved" indicator, the timeline strip (M1 composition,
//! static — playback lands in M3) and the export panel.
//!
//! Playback does not exist in the app yet, so selection-follows-playback is
//! deliberately skipped (documented in the M2 report).

use std::time::Instant;

use egui::{pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Rect, Stroke, StrokeKind};

use crate::theme::Palette;

use super::views::{pane_caption, pane_title, PANE_PAD_X};
use super::{export, AppState, Effect};

pub fn ui(ui: &mut egui::Ui, palette: &Palette, state: &mut AppState, effects: &mut Vec<Effect>) {
    let (title, meta, duration_ms) = match &state.editor {
        Some(editor) => (
            editor.transcript.title.clone(),
            format!(
                "{} · {} · {} segment(s) · {}",
                editor.transcript.model,
                editor.transcript.language.as_deref().unwrap_or("auto"),
                editor.transcript.segments.len(),
                editor.transcript.created_at.format("%d %b %Y %H:%M")
            ),
            editor
                .transcript
                .segments
                .last()
                .map(|seg| seg.end_ms)
                .unwrap_or(0),
        ),
        None => ("No transcript open".to_string(), String::new(), 0),
    };

    ui.horizontal(|ui| {
        ui.add_space(PANE_PAD_X - 8.0);
        ui.vertical(|ui| {
            pane_title(ui, &title, palette);
            pane_caption(ui, &meta, palette, false);
            let save_state = state
                .editor
                .as_ref()
                .map(|editor| editor.save_state())
                .unwrap_or("");
            if !save_state.is_empty() {
                let failed = state
                    .editor
                    .as_ref()
                    .is_some_and(|e| e.save_error.is_some());
                pane_caption(ui, save_state, palette, failed);
            }
            if let Some(status) = &state.status {
                pane_caption(ui, &status.text, palette, status.error);
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            ui.add_space(PANE_PAD_X - 8.0);
            let response = ui.add(
                egui::Label::new(
                    egui::RichText::new("Home")
                        .size(12.0)
                        .color(palette.text_dim),
                )
                .sense(egui::Sense::click()),
            );
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                effects.extend(state.back_home());
            }
        });
    });

    // ---- timeline strip (M1 scrubber composition, static) --------------------
    let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), 66.0), egui::Sense::hover());
    let strip = Rect::from_min_max(
        pos2(outer.left() + PANE_PAD_X, outer.top()),
        pos2(outer.right() - PANE_PAD_X, outer.bottom() - 6.0),
    );
    timeline_strip(ui, strip, palette, duration_ms);

    // ---- export panel (bottom card) ----------------------------------------
    egui::TopBottomPanel::bottom("suramak-export")
        .resizable(false)
        .exact_height(148.0)
        .frame(
            egui::Frame::NONE
                .fill(palette.card)
                .stroke(Stroke::new(1.0_f32, palette.hairline))
                .inner_margin(egui::Margin::same(14)),
        )
        .show_inside(ui, |ui| export::ui(ui, palette, state, effects));

    // ---- transcript rows ----------------------------------------------------
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

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(6.0);
            if rows.is_empty() {
                let painter = ui.painter();
                painter.text(
                    pos2(ui.cursor().left() + PANE_PAD_X - 8.0, ui.cursor().top()),
                    Align2::LEFT_TOP,
                    "No speech segments were detected in this recording.",
                    FontId::new(13.0, FontFamily::Proportional),
                    palette.text_dim,
                );
                return;
            }
            let mut edited: Option<(u32, String)> = None;
            let Some(editor) = &mut state.editor else {
                return;
            };
            for (index, (idx, start_ms)) in rows.iter().enumerate() {
                ui.horizontal_top(|ui| {
                    ui.add_space(PANE_PAD_X - 8.0);
                    ui.add_sized(
                        [86.0, 18.0],
                        egui::Label::new(
                            egui::RichText::new(suramak_core::time::fmt_ms(*start_ms))
                                .monospace()
                                .size(11.0)
                                .color(palette.text_dim),
                        ),
                    );
                    let width = (ui.available_width() - 16.0).max(120.0);
                    let response = ui.add(
                        egui::TextEdit::multiline(&mut editor.buffers[index])
                            .id_salt(("segment", *idx))
                            .desired_width(width)
                            .desired_rows(1)
                            .font(FontId::new(15.0, crate::theme::serif_family())),
                    );
                    if response.changed() {
                        edited = Some((*idx, editor.buffers[index].clone()));
                    }
                });
                ui.add_space(2.0);
            }
            if let Some((idx, text)) = edited {
                editor.edit(idx, text, Instant::now());
            }
        });
}

/// The M1 scrubber strip: deterministic bar silhouette + linear time ticks from
/// the real transcript duration + the playhead parked at the start.
fn timeline_strip(ui: &egui::Ui, strip: Rect, palette: &Palette, total_ms: i64) {
    let painter = ui.painter();
    painter.rect_filled(strip, CornerRadius::same(10), palette.card);
    painter.rect_stroke(
        strip,
        CornerRadius::same(10),
        Stroke::new(1.0_f32, palette.hairline),
        StrokeKind::Inside,
    );

    let bars = strip.shrink2(vec2(16.0, 8.0));
    let tick_row = 14.0;
    let center_y = bars.center().y - tick_row * 0.5;
    let max_half = (bars.height() - tick_row) * 0.5 - 4.0;

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
            palette.wave_dim
        } else {
            palette.wave
        };
        painter.rect_filled(bar, CornerRadius::same(1), color);
        x += pitch;
        index += 1;
    }

    if total_ms > 0 {
        let tick_y = bars.bottom() + tick_row * 0.5;
        let ticks = [
            (
                0.0_f32,
                suramak_core::time::fmt_plain(0),
                Align2::LEFT_CENTER,
            ),
            (
                0.5,
                suramak_core::time::fmt_plain(total_ms / 2),
                Align2::CENTER_CENTER,
            ),
            (
                1.0,
                suramak_core::time::fmt_plain(total_ms),
                Align2::RIGHT_CENTER,
            ),
        ];
        for (t, label, anchor) in ticks {
            let tick_x = bars.left() + (bars.width() - 22.0) * t;
            painter.text(
                pos2(tick_x, tick_y),
                anchor,
                label,
                FontId::new(10.5, FontFamily::Monospace),
                palette.text_dim,
            );
        }
    }

    // No playback yet (M3): the playhead stays parked at the start.
    let playhead = Rect::from_min_max(
        pos2(bars.left() + 1.0, bars.top() - 2.0),
        pos2(bars.left() + 3.0, bars.bottom() - 2.0),
    );
    painter.rect_filled(playhead, CornerRadius::same(1), palette.accent);
}

/// Deterministic pseudo-random bar silhouette (no RNG dependency; M1 generator).
fn bar_height(index: usize) -> f32 {
    let mut hash = (index as u32).wrapping_mul(2_654_435_761);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2c1b_3c6d);
    hash ^= hash >> 12;
    let noise = (hash & 0xffff) as f32 / 65_535.0;
    let swell = 0.45 + 0.55 * ((index as f32) * 0.045).sin().abs();
    (swell * (0.25 + 0.75 * noise)).clamp(0.0, 1.0)
}
