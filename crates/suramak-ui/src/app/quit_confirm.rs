//! M9e (P1-1): the quit-mid-recording confirm.
//!
//! A painted modal card — no OS dialog — shown when a quit is requested
//! (window close or Cmd-Q) while a capture session is live. Three actions:
//! **Stop & Save** (recommended), **Discard Recording** and **Keep Recording**
//! (Esc = keep; a click on the scrim also keeps). The card follows the motion
//! brief's dialog grammar: open = base fade + 2 pt rise, close = fast fade,
//! both snap under Reduce Motion (`motion::fade`).
//!
//! The card is modal in fact, not by convention: the scrim sits above the
//! floating pill (later area in the same order), and the pill + the record
//! view's Stop are additionally disabled while the prompt is up (see
//! `record.rs` / `views.rs`), so no control behind the card can fire.
//!
//! Esc resolves through the central keymap ([`super::keymap`]) to
//! [`super::QuitAnswer::Keep`]; the buttons go through
//! [`super::AppState::answer_quit`] and hand the outcome to the frame loop as
//! [`super::Effect::Quit`].

use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Rect};

use crate::motion::{self, Motion};
use crate::theme::Tokens;

use super::controls;
use super::views::paint_card;
use super::{AppState, Effect, QuitAnswer};

/// The card's designed size (fits the 900×600 minimum with room to spare).
pub const CARD_WIDTH: f32 = 460.0;
pub const CARD_HEIGHT: f32 = 196.0;

/// Whether the card must still be drawn at this fade (the fade-out keeps the
/// card alive until the fast token ends).
pub fn card_visible(fade: f32) -> bool {
    fade > 0.01
}

/// Draws the confirm (no-op while closed and settled).
pub fn ui(
    ctx: &egui::Context,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    let open = state.quit_prompt.is_some();
    // Called every frame, also while closed, so the fade id is registered at 0
    // before the first open (the sheet's discipline — a retarget from the
    // current value, never a teleport). Reduce Motion: both directions snap.
    let fade = motion.fade(
        ctx,
        egui::Id::new("suramak-quit-fade"),
        open,
        motion::BASE,
        motion::FAST,
    );
    if !card_visible(fade) {
        return;
    }
    let viewport = ctx.viewport_rect();

    // ---- the scrim (also the safe cancel: a click outside = Keep) -----------
    egui::Area::new(egui::Id::new("suramak-quit-scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(viewport.min)
        .movable(false)
        .constrain(false)
        .fade_in(false)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(viewport.size(), egui::Sense::click());
            let alpha = (96.0 * fade).round().clamp(0.0, 255.0) as u8;
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(alpha));
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
            }
            if open && response.clicked() {
                answer(state, effects, QuitAnswer::Keep);
            }
        });

    // ---- the card -----------------------------------------------------------
    let card = Rect::from_center_size(viewport.center(), vec2(CARD_WIDTH, CARD_HEIGHT));
    egui::Area::new(egui::Id::new("suramak-quit-card"))
        .order(egui::Order::Tooltip)
        .fixed_pos(card.min)
        .movable(false)
        .constrain(false)
        .fade_in(false)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(card.size(), egui::Sense::hover());
            ui.set_opacity(fade);
            let painter = ui.painter().clone();
            paint_card(&painter, rect, tokens);

            let pad = tokens.space.xl - 2.0; // 22
            let title_pos = pos2(
                rect.left() + pad,
                rect.top() + 16.0 + motion::rise(fade, 2.0),
            );
            painter.text(
                title_pos,
                Align2::LEFT_TOP,
                "Still recording",
                FontId::new(22.0, crate::theme::serif_family()),
                tokens.ink,
            );
            painter.text(
                pos2(rect.left() + pad, rect.top() + 56.0),
                Align2::LEFT_TOP,
                "A recording is in progress — choose what happens to it before quitting.",
                FontId::new(12.0, FontFamily::Proportional),
                tokens.ink_soft,
            );
            painter.text(
                pos2(rect.left() + pad, rect.top() + 74.0),
                Align2::LEFT_TOP,
                "Stop & Save keeps the audio and transcribes it on the next launch.",
                FontId::new(12.0, FontFamily::Proportional),
                tokens.ink_soft,
            );

            // The three actions: capsule bodies, per the control system (the
            // title-line cluster's treatment — the house language for in-app
            // confirms since M9c).
            let row = Rect::from_min_size(
                pos2(rect.left() + pad, rect.top() + 112.0),
                vec2(rect.width() - pad * 2.0, controls::CAPSULE_HEIGHT),
            );
            let mut answered: Option<QuitAnswer> = None;
            ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let stop_save = controls::capsule_button(
                        ui,
                        tokens,
                        motion,
                        "quit-stop-save",
                        "Stop & Save",
                        true,
                    )
                    .on_hover_text(
                        "Stop now, keep the recording and transcribe it on the next launch",
                    );
                    if stop_save.clicked() {
                        answered = Some(QuitAnswer::StopAndSave);
                    }
                    let discard = controls::capsule_button(
                        ui,
                        tokens,
                        motion,
                        "quit-discard",
                        "Discard Recording",
                        true,
                    )
                    .on_hover_text("Quit and throw this recording away");
                    if discard.clicked() {
                        answered = Some(QuitAnswer::Discard);
                    }
                    let keep = controls::capsule_button(
                        ui,
                        tokens,
                        motion,
                        "quit-keep",
                        "Keep Recording",
                        true,
                    )
                    .on_hover_text("Cancel the quit — the recording continues (Esc)");
                    if keep.clicked() {
                        answered = Some(QuitAnswer::Keep);
                    }
                });
            });

            painter.text(
                pos2(rect.left() + pad, rect.top() + 150.0),
                Align2::LEFT_TOP,
                "Esc keeps recording",
                FontId::new(11.0, FontFamily::Proportional),
                tokens.ink_soft,
            );

            // One-shot: only a live prompt can be answered; while the card
            // fades out (Keep was chosen) the controls are ghosts.
            if let Some(chosen) = answered {
                if open {
                    answer(state, effects, chosen);
                }
            }
        });
}

/// Answers the confirm and hands the outcome to the frame loop.
fn answer(state: &mut AppState, effects: &mut Vec<Effect>, answer: QuitAnswer) {
    if let Some(outcome) = state.answer_quit(answer) {
        effects.push(Effect::Quit(outcome));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{QuitSource, RecordState};
    use crate::theme::AppTheme;

    #[test]
    fn the_card_draws_while_fading_and_vanishes_at_zero() {
        assert!(!card_visible(0.0), "a settled-closed card draws nothing");
        assert!(card_visible(0.02), "the opening card is drawn");
        assert!(card_visible(1.0));
        assert!(card_visible(0.5), "the closing card keeps drawing");
    }

    /// Frame-level: with the prompt up, the card paints its title and the
    /// three actions land in the accesskit tree with their labels (a11y
    /// labels, M9b floor).
    #[test]
    fn the_confirm_renders_its_title_and_three_labelled_actions() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        crate::theme::install_fonts(&ctx);
        let tokens = crate::theme::tokens(AppTheme::Light);
        let motion = Motion::new(false);
        let mut state = AppState::new();
        // A live session (seeded, as the smoke harness does) + the prompt.
        state.seed_demo_recording();
        assert_eq!(
            state.request_quit(QuitSource::WindowClose),
            super::super::QuitAction::ShowConfirm
        );
        let mut effects = Vec::new();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0));
        let run = |state: &mut AppState, effects: &mut Vec<Effect>, time: f64| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..Default::default()
            };
            ctx.run(input, |ctx| {
                ui(ctx, &tokens, &motion, state, effects);
            })
        };
        // Two frames: the fade registers at 0 first, then animates in.
        let _ = run(&mut state, &mut effects, 0.0);
        let output = run(&mut state, &mut effects, 0.05);
        let has_title = output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text() == "Still recording")
        });
        assert!(has_title, "the card's title must paint while open");

        let update = output
            .platform_output
            .accesskit_update
            .expect("an accesskit tree update");
        let labels: Vec<String> = update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label().map(str::to_string))
            .collect();
        for expected in ["Stop & Save", "Discard Recording", "Keep Recording"] {
            assert!(
                labels.iter().any(|label| label == expected),
                "missing a11y label {expected:?}; got {labels:?}"
            );
        }
        assert!(matches!(state.record, RecordState::Recording(_)));
    }
}
