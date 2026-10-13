//! Record route (M3): the live recording view — auto title, elapsed clock, RMS
//! meter and the rolling waveform painter, all in the calm M1 style.
//!
//! The runtime owns the cpal session; this view reads [`AppState::record`] (fed by
//! the runtime's meter samples) and only ever asks for [`Effect::StopCapture`].

use egui::{
    pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Painter, Rect, Stroke, StrokeKind,
};

use crate::theme::Palette;

use super::views::{pane_caption, pane_title, PANE_PAD_X};
use super::{format_elapsed, AppState, Effect, RecordState, RECORD_WAVEFORM_BARS};

pub fn ui(ui: &mut egui::Ui, palette: &Palette, state: &mut AppState, effects: &mut Vec<Effect>) {
    let rect = ui.max_rect();
    let left = rect.left() + PANE_PAD_X;
    let right = rect.right() - PANE_PAD_X;

    match &state.record {
        RecordState::Recording(recording) => {
            let title = recording.title.clone();
            let elapsed = format_elapsed(recording.elapsed);
            let rms = recording.rms;
            let waveform = recording.waveform.clone();
            let device = recording.device.clone();
            let stopping = recording.stopping;
            recording_body(
                ui,
                palette,
                state,
                effects,
                rect,
                left,
                right,
                &title,
                &elapsed,
                rms,
                &waveform,
                device.as_deref(),
                stopping,
            );
        }
        RecordState::Failed { message } => {
            let message = message.clone();
            pane_title(ui, "Recording failed", palette);
            pane_caption(ui, &message, palette, true);
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(PANE_PAD_X - 8.0);
                if ui.button("Back").clicked() {
                    effects.extend(state.leave_record());
                }
            });
        }
        RecordState::Idle => {
            // Reached only through a route change without a session; stay calm and
            // offer the way back rather than showing a dead screen.
            pane_title(ui, "Not recording", palette);
            pane_caption(
                ui,
                "Press the floating Record control (bottom center) to record.",
                palette,
                false,
            );
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(PANE_PAD_X - 8.0);
                if ui.button("Back").clicked() {
                    effects.extend(state.back_home());
                }
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn recording_body(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
    rect: Rect,
    left: f32,
    right: f32,
    title: &str,
    elapsed: &str,
    rms: f32,
    waveform: &[f32],
    device: Option<&str>,
    stopping: bool,
) {
    let painter = ui.painter().clone();

    ui.horizontal(|ui| {
        ui.add_space(PANE_PAD_X - 8.0);
        ui.vertical(|ui| {
            pane_title(ui, title, palette);
            let caption = match device {
                Some(device) => {
                    format!("recording from {device} — transcribed on this machine, never uploaded")
                }
                None => "starting capture…".to_string(),
            };
            pane_caption(ui, &caption, palette, false);
        });
    });

    // ---- metering card ------------------------------------------------------
    let card = Rect::from_min_size(pos2(left, rect.top() + 96.0), vec2(right - left, 176.0));
    painter.rect_filled(card, CornerRadius::same(10), palette.card);
    painter.rect_stroke(
        card,
        CornerRadius::same(10),
        Stroke::new(1.0_f32, palette.hairline),
        StrokeKind::Inside,
    );

    // Elapsed clock (mono, prominent but calm).
    painter.text(
        pos2(card.left() + 22.0, card.top() + 20.0),
        Align2::LEFT_TOP,
        elapsed,
        FontId::new(30.0, FontFamily::Monospace),
        palette.text,
    );
    painter.text(
        pos2(card.right() - 22.0, card.top() + 30.0),
        Align2::RIGHT_TOP,
        if stopping {
            "finishing…"
        } else {
            "recording"
        },
        FontId::new(12.0, FontFamily::Monospace),
        palette.text_dim,
    );

    // RMS meter.
    let meter_label = pos2(card.left() + 22.0, card.top() + 66.0);
    painter.text(
        meter_label,
        Align2::LEFT_TOP,
        "level",
        FontId::new(11.0, FontFamily::Proportional),
        palette.text_dim,
    );
    let meter = Rect::from_min_size(
        pos2(card.left() + 22.0, card.top() + 82.0),
        vec2(card.width() - 44.0, 8.0),
    );
    painter.rect_filled(meter, CornerRadius::same(4), palette.pane);
    let fraction = rms.clamp(0.0, 1.0);
    if fraction > 0.0 {
        painter.rect_filled(
            Rect::from_min_size(meter.min, vec2(meter.width() * fraction, meter.height())),
            CornerRadius::same(4),
            palette.accent,
        );
    }

    // Rolling waveform.
    let wave = Rect::from_min_size(
        pos2(card.left() + 22.0, card.top() + 108.0),
        vec2(card.width() - 44.0, 46.0),
    );
    draw_waveform(&painter, wave, waveform, palette);

    // ---- controls -----------------------------------------------------------
    let controls = Rect::from_min_size(pos2(left, card.bottom() + 18.0), vec2(right - left, 30.0));
    let mut stop_clicked = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(controls), |ui| {
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!stopping, |ui| {
                if ui.button("Stop").clicked() {
                    stop_clicked = true;
                }
            });
            ui.add_space(10.0);
            pane_caption(
                ui,
                &format!(
                    "current recording: {} of the last {RECORD_WAVEFORM_BARS} meter samples",
                    waveform.len()
                ),
                palette,
                false,
            );
        });
    });
    if stop_clicked {
        effects.extend(state.stop_recording());
    }

    // Source-resolution warnings ("device not found — using the default") must be
    // visible while recording (Addendum A: fall back with a *visible* warning).
    if let Some(status) = &state.status {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(Rect::from_min_size(
                pos2(left, card.bottom() + 56.0),
                vec2(right - left, 20.0),
            )),
            |ui| {
                ui.horizontal(|ui| {
                    pane_caption(ui, &status.text, palette, status.error);
                });
            },
        );
    }
}

/// Rolling waveform painter: one centered bar per meter sample (oldest first,
/// newest at the right edge), in the M1 waveform greys.
fn draw_waveform(painter: &Painter, rect: Rect, values: &[f32], palette: &Palette) {
    painter.hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0_f32, palette.hairline),
    );
    if values.is_empty() {
        return;
    }
    let step = rect.width() / RECORD_WAVEFORM_BARS as f32;
    let bar_width = (step - 2.0).max(1.0);
    for (index, value) in values.iter().enumerate() {
        let offset_from_right = (values.len() - index) as f32;
        let x = rect.right() - step * offset_from_right + step / 2.0;
        let half = (value.clamp(0.0, 1.0) * rect.height() * 0.5).max(1.0);
        let bar = Rect::from_center_size(pos2(x, rect.center().y), vec2(bar_width, half * 2.0));
        painter.rect_filled(bar, CornerRadius::same(1), palette.wave);
    }
}
