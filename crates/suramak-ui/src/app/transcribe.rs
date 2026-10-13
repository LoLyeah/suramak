//! Transcribe route: the running job — file name, progress bar fed by
//! `WorkerEvent::Progress`, the engine line from M1.5's outcome reporting, and
//! the failure state.

use egui::{pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Rect, Stroke, StrokeKind};

use crate::theme::Palette;

use super::views::{pane_caption, pane_title, PANE_PAD_X};
use super::{AppState, Effect, Job};

pub fn ui(ui: &mut egui::Ui, palette: &Palette, state: &mut AppState, effects: &mut Vec<Effect>) {
    let rect = ui.max_rect();
    let left = rect.left() + PANE_PAD_X;
    let right = rect.right() - PANE_PAD_X;

    let (file_name, pct, model, requested, planned, warnings, failure) = match &state.job {
        Job::Running(run) => (
            run.input
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| run.input.display().to_string()),
            run.pct,
            run.model.clone(),
            run.requested_engine.clone(),
            run.planned_engine.clone(),
            run.warnings.clone(),
            None,
        ),
        Job::Failed { input, message } => (
            input
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| input.display().to_string()),
            0,
            None,
            None,
            None,
            Vec::new(),
            Some(message.clone()),
        ),
        Job::Idle => ("—".to_string(), 0, None, None, None, Vec::new(), None),
    };

    ui.horizontal(|ui| {
        ui.add_space(PANE_PAD_X - 8.0);
        ui.vertical(|ui| {
            pane_title(ui, &file_name, palette);
            let caption = match &failure {
                Some(_) => "transcription failed".to_string(),
                None => "transcribing on this machine…".to_string(),
            };
            pane_caption(ui, &caption, palette, failure.is_some());
        });
    });

    // ---- progress card ------------------------------------------------------
    let card = Rect::from_min_size(pos2(left, rect.top() + 96.0), vec2(right - left, 104.0));
    let painter = ui.painter();
    painter.rect_filled(card, CornerRadius::same(10), palette.card);
    painter.rect_stroke(
        card,
        CornerRadius::same(10),
        Stroke::new(1.0_f32, palette.hairline),
        StrokeKind::Inside,
    );

    let bar = Rect::from_min_size(
        pos2(card.left() + 22.0, card.center().y - 7.0),
        vec2(card.width() - 44.0, 14.0),
    );
    painter.rect_filled(bar, CornerRadius::same(7), palette.pane);
    let fraction = f32::from(pct) / 100.0;
    if fraction > 0.0 {
        let filled = Rect::from_min_size(bar.min, vec2(bar.width() * fraction, bar.height()));
        painter.rect_filled(filled, CornerRadius::same(7), palette.accent);
    }
    painter.text(
        pos2(bar.center().x, bar.top() - 14.0),
        Align2::CENTER_CENTER,
        format!("{pct}%"),
        FontId::new(12.0, FontFamily::Monospace),
        palette.text,
    );
    let engine_line = match (&requested, &planned) {
        (Some(requested), Some(planned)) => format!("engine: {requested} → planned {planned}"),
        _ => "engine: resolving…".to_string(),
    };
    painter.text(
        pos2(bar.left(), bar.bottom() + 14.0),
        Align2::LEFT_TOP,
        engine_line,
        FontId::new(11.5, FontFamily::Monospace),
        palette.text_dim,
    );
    if let Some(model) = &model {
        painter.text(
            pos2(bar.right(), bar.bottom() + 14.0),
            Align2::RIGHT_TOP,
            format!("model: {model}"),
            FontId::new(11.5, FontFamily::Monospace),
            palette.text_dim,
        );
    }

    let mut y = card.bottom() + 16.0;
    for warning in &warnings {
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(pos2(left, y), vec2(right - left, 18.0))),
            |ui| {
                ui.horizontal(|ui| {
                    pane_caption(ui, &format!("warning: {warning}"), palette, false);
                });
            },
        );
        y += 18.0;
    }

    if let Some(message) = &failure {
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(pos2(left, y), vec2(right - left, 24.0))),
            |ui| {
                ui.horizontal(|ui| {
                    pane_caption(ui, message, palette, true);
                });
            },
        );
        y += 34.0;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(pos2(left, y), vec2(right - left, 30.0))),
            |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Back").clicked() {
                        effects.extend(state.back_home());
                    }
                });
            },
        );
    }
}
