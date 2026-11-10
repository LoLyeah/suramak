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
use super::{AppState, Effect, Job, JobStage};

/// The queue card's fixed geometry (Deck density): four rows visible, the rest
/// behind a `+n more` line. Bounded rows are a design rule, not a scroll area —
/// no hidden scrolling, no variable-height card.
pub const QUEUE_VISIBLE_ROWS: usize = 4;
const QUEUE_ROW_HEIGHT: f32 = 26.0;
const QUEUE_HEADER: f32 = 28.0;
const QUEUE_MORE_LINE: f32 = 18.0;
const QUEUE_BUTTONS_WIDTH: f32 = 196.0;

/// End-elides a file name to `max_chars` (the card never lets a long path push
/// the state label or the controls off the row).
pub fn elide(name: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(4);
    if name.chars().count() <= max_chars {
        return name.to_string();
    }
    let keep = max_chars - 1;
    let mut out: String = name.chars().take(keep).collect();
    out.push('…');
    out
}

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

    let (file_name, pct, model, planned, warnings, failure, cancelling, stage) = match &state.job {
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
            run.stage,
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
            None,
        ),
        Job::Idle => (
            "—".to_string(),
            0,
            None,
            None,
            Vec::new(),
            None,
            false,
            None,
        ),
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
                // M16.1: the per-item state machine is visible. `decoding` is
                // the honest label until the engine actually starts.
                (None, false) => match stage {
                    Some(JobStage::Decoding) => "decoding the audio…".to_string(),
                    _ => "transcribing on this machine…".to_string(),
                },
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

    // ---- M16.1: the queue ---------------------------------------------------
    // Compact, bounded, drag-free: rows are painted at fixed geometry and the
    // controls are the house painted buttons. Nothing here animates on its own
    // — a queue row only repaints on input or on the worker events that changed
    // it, so the card never adds an idle frame.
    let rows: Vec<(u64, usize, String, &'static str)> = state
        .queue_rows()
        .iter()
        .map(|item| (item.id, item.seq, item.name(), item.state.label()))
        .collect();
    let mut bottom = y;
    if !rows.is_empty() {
        let top = y + 34.0;
        let visible = rows.len().min(QUEUE_VISIBLE_ROWS);
        let more = rows.len() - visible;
        let height = QUEUE_HEADER
            + visible as f32 * QUEUE_ROW_HEIGHT
            + if more > 0 { QUEUE_MORE_LINE } else { 0.0 }
            + 8.0;
        let card = Rect::from_min_size(pos2(left, top), vec2(right - left, height));
        let painter = ui.painter().clone();
        paint_card(&painter, card, tokens);
        painter.text(
            pos2(card.left() + 18.0, card.top() + 15.0),
            Align2::LEFT_CENTER,
            format!("Up next · {}", super::count_noun(rows.len(), "file")),
            FontId::new(11.0, FontFamily::Proportional),
            tokens.ink_soft,
        );

        let max_chars = (((right - left) - QUEUE_BUTTONS_WIDTH - 96.0) / 7.0).max(12.0) as usize;
        for (index, (id, seq, name, label)) in rows.iter().take(visible).enumerate() {
            let row_top = card.top() + QUEUE_HEADER + index as f32 * QUEUE_ROW_HEIGHT;
            painter.text(
                pos2(card.left() + 18.0, row_top + QUEUE_ROW_HEIGHT / 2.0),
                Align2::LEFT_CENTER,
                format!("{seq}. {}", elide(name, max_chars)),
                FontId::new(12.0, FontFamily::Proportional),
                tokens.ink,
            );
            painter.text(
                pos2(
                    card.right() - 12.0 - QUEUE_BUTTONS_WIDTH - 10.0,
                    row_top + QUEUE_ROW_HEIGHT / 2.0,
                ),
                Align2::RIGHT_CENTER,
                *label,
                FontId::new(10.5, FontFamily::Monospace),
                tokens.ink_soft,
            );
            let controls_rect = Rect::from_min_size(
                pos2(card.right() - 12.0 - QUEUE_BUTTONS_WIDTH, row_top),
                vec2(QUEUE_BUTTONS_WIDTH, QUEUE_ROW_HEIGHT),
            );
            let row_id = *id;
            let row_name = name.clone();
            ui.scope_builder(egui::UiBuilder::new().max_rect(controls_rect), |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let remove = controls::button(
                        ui,
                        tokens,
                        motion,
                        ("queue-remove", row_id),
                        "Remove",
                        Tier::Tertiary,
                        true,
                    )
                    .on_hover_text(format!("Remove {row_name} from the queue"));
                    if remove.clicked() {
                        effects.extend(state.cancel_queued(row_id));
                    }
                    let down = controls::button(
                        ui,
                        tokens,
                        motion,
                        ("queue-down", row_id),
                        "Down",
                        Tier::Tertiary,
                        true,
                    )
                    .on_hover_text("Move this file one place later");
                    if down.clicked() {
                        state.lower_queued(row_id);
                    }
                    let up = controls::button(
                        ui,
                        tokens,
                        motion,
                        ("queue-up", row_id),
                        "Up",
                        Tier::Tertiary,
                        true,
                    )
                    .on_hover_text("Move this file one place earlier");
                    if up.clicked() {
                        state.raise_queued(row_id);
                    }
                });
            });
        }
        if more > 0 {
            painter.text(
                pos2(
                    card.left() + 18.0,
                    card.top() + QUEUE_HEADER + visible as f32 * QUEUE_ROW_HEIGHT + 9.0,
                ),
                Align2::LEFT_CENTER,
                format!("+{more} more"),
                FontId::new(11.0, FontFamily::Proportional),
                tokens.ink_soft,
            );
        }
        bottom = card.bottom();
    }

    // M16.1: a failed item's reason stays visible while the next one runs —
    // the queue never hides why a file stopped. The single failure state above
    // covers the last item (the card's main subject then).
    if let Some(last) = &state.queue_failure {
        if matches!(state.job, Job::Running(_)) {
            let line = format!(
                "failed earlier: {} — {}",
                super::file_label(&last.input),
                last.message
            );
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(Rect::from_min_size(
                    pos2(left, bottom + 10.0),
                    vec2(right - left, 22.0),
                )),
                |ui| {
                    ui.horizontal(|ui| {
                        pane_caption(ui, &line, tokens, true);
                    });
                },
            );
        }
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

    // ---- M16.1: the queue card ----------------------------------------------

    fn queued_state(count: usize) -> AppState {
        let mut state = AppState::new();
        let dir = std::env::temp_dir().join(format!("suramak-m16a-q-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for index in 0..count {
            let path = dir.join(format!("queue-{index}.wav"));
            std::fs::write(&path, b"x").unwrap();
            state.begin_import(path);
        }
        state
    }

    #[test]
    fn the_queue_card_lists_pending_rows_with_controls_and_the_more_line() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        // Seven files: one running, six waiting; four rows + "+2 more".
        let mut state = queued_state(7);
        let _ = frame(&ctx, &mut state, 0.0);
        let output = frame(&ctx, &mut state, 0.2);
        let painted = labels(&output);

        assert!(
            painted.iter().any(|label| label == "Up next · 6 files"),
            "the header counts the waiting rows: {painted:?}"
        );
        // The first waiting row is position 2 (position 1 is running).
        assert!(
            painted.iter().any(|label| label == "2. queue-1.wav"),
            "rows carry their stable position: {painted:?}"
        );
        assert!(
            painted.iter().any(|label| label == "+2 more"),
            "bounded rows leave a '+n more' line: {painted:?}"
        );
        for control in ["Up", "Down", "Remove"] {
            let count = painted.iter().filter(|label| *label == control).count();
            assert_eq!(count, QUEUE_VISIBLE_ROWS, "one {control} per visible row");
        }
        assert!(painted.iter().any(|label| label == "queued"));
    }

    #[test]
    fn the_queue_card_keeps_a_failed_items_reason_visible() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = queued_state(3);
        // The first item fails; the second is now running.
        state.job_failed("decode failed".to_string());
        let _ = frame(&ctx, &mut state, 0.0);
        let output = frame(&ctx, &mut state, 0.2);
        let painted = labels(&output);
        assert!(
            painted
                .iter()
                .any(|label| label.starts_with("failed earlier: queue-0.wav — decode failed")),
            "the failed reason must not vanish when the next item starts: {painted:?}"
        );
    }

    #[test]
    fn the_queue_card_adds_no_idle_repaint() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = queued_state(4);
        // No interpolation clock: the bar is static, the rows are static.
        if let Job::Running(run) = &mut state.job {
            run.started_at = None;
        }
        let _ = frame(&ctx, &mut state, 0.0);
        let output = frame(&ctx, &mut state, 0.2);
        let delay = output
            .viewport_output
            .values()
            .map(|viewport| viewport.repaint_delay)
            .min()
            .unwrap_or(std::time::Duration::MAX);
        assert!(
            delay > std::time::Duration::from_secs(1),
            "the queue must not schedule idle frames (repaint_delay {delay:?})"
        );
    }

    #[test]
    fn long_names_are_end_elided_so_the_controls_stay_on_the_row() {
        assert_eq!(elide("clip.wav", 12), "clip.wav");
        assert_eq!(elide("a-very-long-recording-name.wav", 12), "a-very-long…");
        assert_eq!(elide("abc", 1), "abc", "a tiny budget never panics");
        assert_eq!(elide("", 8), "");
    }
}
