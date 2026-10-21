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
    // M14e: the bar moves between the engine's sparse progress events — from the
    // wall clock against the input's duration, clamped below 97 % until a real
    // event confirms more. A job without a known duration keeps the honest
    // "estimating…" label instead of a bar that pretends.
    let progress = match &state.job {
        Job::Running(run) => Some(run.progress(std::time::Instant::now())),
        _ => None,
    };
    let fraction = progress
        .as_ref()
        .map(|progress| progress.fraction)
        .unwrap_or_else(|| f32::from(pct) / 100.0);
    let label = progress
        .as_ref()
        .map(|progress| progress.label.clone())
        .unwrap_or_else(|| format!("{pct}%"));
    if progress.as_ref().is_some_and(|progress| progress.animating) {
        // Heat budget: the bar is the only thing moving on this screen, and only
        // while it visibly moves (the estimate stops at the ceiling).
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(120));
    }
    // Why a fast engine is idle on this job: `auto` never routes to one. Shown
    // only when the fast engine is on (otherwise there is nothing to explain).
    let auto_note = match (&state.job, state.settings_form.fast_engine) {
        (Job::Running(run), true) => run.auto_language_note(),
        _ => None,
    };

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
    if fraction > 0.0 {
        let filled = Rect::from_min_size(bar.min, vec2(bar.width() * fraction, bar.height()));
        painter.rect_filled(filled, CornerRadius::same(7), tokens.accent);
    }
    painter.text(
        pos2(bar.center().x, bar.top() - 14.0),
        Align2::CENTER_CENTER,
        label,
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
    if let Some(note) = auto_note {
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(pos2(left, y), vec2(right - left, 18.0))),
            |ui| {
                ui.horizontal(|ui| {
                    pane_caption(ui, note, tokens, false);
                });
            },
        );
        y += 18.0;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppState, Job};
    use crate::theme::AppTheme;

    fn frame(ctx: &egui::Context, state: &mut AppState, time: f64) -> egui::FullOutput {
        let tokens = crate::theme::tokens(AppTheme::Light);
        let motion = Motion::new(false);
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 600.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let mut effects = Vec::new();
                    super::ui(ui, &tokens, &motion, state, &mut effects);
                });
        })
    }

    fn labels(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    fn running_job(auto_language: bool, with_duration: bool) -> AppState {
        let mut state = AppState::new();
        let dir = std::env::temp_dir().join(format!("suramak-m14e-jc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("clip.wav");
        std::fs::write(&input, b"x").unwrap();
        state.begin_import(input);
        let language = if auto_language { None } else { Some("en") };
        let duration = if with_duration { Some(9_000) } else { None };
        state.job_started(std::time::Instant::now(), duration, language);
        // The fast engine is on (the note only exists to explain a fast engine).
        state.settings_form.fast_engine = true;
        state
    }

    /// M14e: with the fast engine on and the language on `auto`, the card says
    /// why no fast engine ran — a quiet one-liner, not a warning.
    #[test]
    fn the_card_explains_an_auto_language_when_the_fast_engine_is_on() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = running_job(true, true);
        let _ = frame(&ctx, &mut state, 0.0);
        let output = frame(&ctx, &mut state, 0.3);
        let painted = labels(&output);
        assert!(
            painted
                .iter()
                .any(|label| label == crate::app::fast_engine::AUTO_LANGUAGE_CAPTION),
            "the card must say why the fast engine is idle: {painted:?}"
        );

        // A forced language has nothing to explain…
        let mut forced = running_job(false, true);
        let _ = frame(&ctx, &mut forced, 0.0);
        let output = frame(&ctx, &mut forced, 0.3);
        assert!(
            !labels(&output)
                .iter()
                .any(|label| label == crate::app::fast_engine::AUTO_LANGUAGE_CAPTION),
            "a forced language must not show the auto note"
        );

        // …and neither has a fast-engine-off job.
        let mut off = running_job(true, true);
        off.settings_form.fast_engine = false;
        let _ = frame(&ctx, &mut off, 0.0);
        let output = frame(&ctx, &mut off, 0.3);
        assert!(!labels(&output)
            .iter()
            .any(|label| label == crate::app::fast_engine::AUTO_LANGUAGE_CAPTION));
    }

    /// M14e: between progress events the bar moves on the clock, and its label
    /// says the value is an estimate (never a bare `0%` for a worked job).
    #[test]
    fn the_bar_moves_between_events_and_labels_the_estimate() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = running_job(true, true);
        // Pretend the worker reported nothing yet and one second has passed.
        if let Job::Running(run) = &mut state.job {
            run.model = Some("turbo-q5-0".to_string());
            run.planned_engine = Some("metal".to_string());
            run.started_at =
                Some(std::time::Instant::now() - std::time::Duration::from_millis(700));
        }
        let _ = frame(&ctx, &mut state, 0.0);
        let output = frame(&ctx, &mut state, 0.1);
        let painted = labels(&output);
        assert!(
            painted.iter().any(|label| label.starts_with('~')),
            "an interpolated value is labelled as an estimate: {painted:?}"
        );
        assert!(
            !painted.iter().any(|label| label == "0%"),
            "a worked job must not still read 0%: {painted:?}"
        );
        // The button/cancel chrome is unchanged.
        assert!(painted.iter().any(|label| label == "Cancel"));
    }
}
