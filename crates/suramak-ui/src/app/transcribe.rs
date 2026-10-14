//! Transcribe route: the running job — file name, progress bar fed by
//! `WorkerEvent::Progress`, the engine line from M1.5's outcome reporting, and
//! the failure state.
//!
//! M9a: tokens, the unified control system and the one navigation pattern
//! (Home, top-right) only — the composition is unchanged.

use egui::{pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Rect};

use crate::app::controls::{self, Tier};
use crate::motion::Motion;
use crate::theme::Tokens;

use super::views::{paint_card, pane_caption, pane_title, PANE_PAD_X};
use super::{AppState, Effect, Job};

pub fn ui(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    let rect = ui.max_rect();
    let left = rect.left() + PANE_PAD_X;
    let right = rect.right() - PANE_PAD_X;

    let (file_name, pct, model, planned, warnings, failure, cancelling) = match &state.job {
        Job::Running(run) => (
            run.input
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| run.input.display().to_string()),
            run.pct,
            run.model.clone(),
            run.planned_engine.clone(),
            run.warnings.clone(),
            None,
            run.cancelling,
        ),
        Job::Failed { input, message } => (
            input
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| input.display().to_string()),
            0,
            None,
            None,
            Vec::new(),
            Some(message.clone()),
            false,
        ),
        Job::Idle => ("—".to_string(), 0, None, None, Vec::new(), None, false),
    };
    let running = matches!(state.job, Job::Running(_));

    ui.horizontal(|ui| {
        ui.add_space(PANE_PAD_X - 8.0);
        ui.vertical(|ui| {
            pane_title(ui, &file_name, tokens);
            let caption = match (&failure, cancelling) {
                (Some(_), _) => "transcription failed".to_string(),
                // M9e (P2-2): the cancel was pressed — the worker answers at
                // its next stage boundary; the card says what is happening.
                (None, true) => "cancelling…".to_string(),
                (None, false) => "transcribing on this machine…".to_string(),
            };
            pane_caption(ui, &caption, tokens, failure.is_some());
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            ui.add_space(PANE_PAD_X - 8.0);
            if controls::nav_home(ui, tokens, motion, "transcribe-nav").clicked() {
                effects.extend(state.back_home());
            }
        });
    });

    // ---- progress card ------------------------------------------------------
    let card = Rect::from_min_size(pos2(left, rect.top() + 96.0), vec2(right - left, 104.0));
    let painter = ui.painter();
    paint_card(painter, card, tokens);

    let bar = Rect::from_min_size(
        pos2(card.left() + 22.0, card.center().y - 7.0),
        vec2(card.width() - 44.0, 14.0),
    );
    painter.rect_filled(bar, CornerRadius::same(7), tokens.panel_faint);
    let fraction = f32::from(pct) / 100.0;
    if fraction > 0.0 {
        let filled = Rect::from_min_size(bar.min, vec2(bar.width() * fraction, bar.height()));
        painter.rect_filled(filled, CornerRadius::same(7), tokens.accent);
    }
    painter.text(
        pos2(bar.center().x, bar.top() - 14.0),
        Align2::CENTER_CENTER,
        format!("{pct}%"),
        FontId::new(12.0, FontFamily::Monospace),
        tokens.ink,
    );
    // The engine line in plain words (M9c copy sweep: no `planned`, human
    // engine names). While the plan resolves it says so.
    let engine_line = match &planned {
        Some(planned) => format!("engine: {}", super::engine_display(planned)),
        _ => "engine: choosing…".to_string(),
    };
    painter.text(
        pos2(bar.left(), bar.bottom() + 14.0),
        Align2::LEFT_TOP,
        engine_line,
        FontId::new(11.5, FontFamily::Monospace),
        tokens.ink_soft,
    );
    if let Some(model) = &model {
        painter.text(
            pos2(bar.right(), bar.bottom() + 14.0),
            Align2::RIGHT_TOP,
            format!("model: {}", super::model_manager::display_name(model)),
            FontId::new(11.5, FontFamily::Monospace),
            tokens.ink_soft,
        );
    }

    // M9e (P2-2): the quiet tertiary Cancel, in the card's top-right corner —
    // enabled while the job runs, disabled once the cancel is in flight (the
    // worker answers at its next stage boundary and the caption says
    // `cancelling…`). Esc on this screen is the same path (the central keymap).
    if running {
        let cancel_row = Rect::from_min_size(
            pos2(card.left() + 22.0, card.top() + 6.0),
            vec2(card.width() - 44.0, 24.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(cancel_row), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let cancel = controls::button(
                    ui,
                    tokens,
                    motion,
                    "transcribe-cancel",
                    "Cancel",
                    Tier::Tertiary,
                    !cancelling,
                )
                .on_hover_text("Cancel this transcription");
                if cancel.clicked() {
                    effects.extend(state.cancel_job());
                }
            });
        });
    }

    let mut y = card.bottom() + 16.0;
    for warning in &warnings {
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(pos2(left, y), vec2(right - left, 18.0))),
            |ui| {
                ui.horizontal(|ui| {
                    pane_caption(ui, &format!("warning: {warning}"), tokens, false);
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
                    // The failure line fades in base / out fast (M9c status
                    // discipline) — called every frame so a retry clears it.
                    super::views::status_caption(
                        ui,
                        tokens,
                        motion,
                        "transcribe-failure",
                        Some((message.as_str(), true)),
                    );
                });
            },
        );
    } else {
        // Keep the fade id registered while the job runs (so a failure that
        // lands later animates in, and a cleared one animates out).
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(pos2(left, y), vec2(right - left, 24.0))),
            |ui| {
                ui.horizontal(|ui| {
                    super::views::status_caption(ui, tokens, motion, "transcribe-failure", None);
                });
            },
        );
    }
}
