//! Export sheet (M9a): the deck's `Export…` button opens an anchored card —
//! format chips, destination, `Save as…` (macOS/Windows rfd dialog) and the
//! primary `Export` action — instead of the old fixed 148 px bottom band that
//! collided with the floating record pill (critique H8/P1).
//!
//! Formats are rendered by `suramak-core`'s export module; Linux uses the
//! output path field (rfd is not a dependency there).

use std::path::PathBuf;

use egui::{pos2, Rect};

use crate::app::controls::{self, Tier};
use crate::motion::Motion;
use crate::theme::Tokens;

use super::{AppState, Effect, ExportFormat};

/// The sheet's designed width.
pub const SHEET_WIDTH: f32 = 460.0;

/// macOS/Windows: native save dialog (rfd). Linux: `None` — the path field is
/// the destination affordance there.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn pick_save_dest(suggested: &str) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title("Export transcript");
    let path = PathBuf::from(suggested);
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        dialog = dialog.set_file_name(name);
    }
    if let Some(parent) = path.parent().filter(|p| p.is_dir()) {
        dialog = dialog.set_directory(parent);
    }
    dialog.save_file()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn pick_save_dest(_suggested: &str) -> Option<PathBuf> {
    None
}

/// Whether this build offers the native save dialog.
const HAS_NATIVE_DIALOG: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// The export sheet: an anchored card under the deck's `Export…` button.
/// Closes on a click outside (or when the editor is left).
///
/// M9c motion: **open = base fade + 2 pt rise; close = fast fade** (the brief's
/// "exits faster than enters"). The fade id is computed on every editor frame —
/// including while the sheet is closed — so the animation is registered at 0
/// before the first open, and a rapid open/close flip retargets from the
/// current value (never a snap-back, never a queue). Under Reduce Motion both
/// directions snap.
pub fn sheet(
    ctx: &egui::Context,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
    anchor: Rect,
) {
    let fade = sheet_fade(motion, ctx, state.export.open);
    if !sheet_visible(fade) {
        return;
    }
    let pos = pos2(
        (anchor.right() - SHEET_WIDTH).max(anchor.left()),
        anchor.bottom() + 8.0 + crate::motion::rise(fade, 2.0),
    );
    let area = egui::Area::new(egui::Id::new("suramak-export-sheet"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .movable(false)
        .constrain(true)
        // The sheet does its own fade; a closing sheet takes no input.
        .interactable(state.export.open)
        .fade_in(false)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            egui::Frame::NONE
                .fill(tokens.card)
                .stroke(egui::Stroke::new(tokens.stroke.hairline, tokens.hairline))
                .corner_radius(egui::CornerRadius::same(tokens.radius.card))
                .shadow(tokens.card_shadow)
                .inner_margin(egui::Margin::same(tokens.space.l as i8))
                .show(ui, |ui| {
                    ui.set_min_width(SHEET_WIDTH - tokens.space.l * 2.0);
                    ui.set_max_width(SHEET_WIDTH - tokens.space.l * 2.0);
                    ui.vertical(|ui| {
                        contents(ui, tokens, motion, state, effects);
                    });
                });
        });

    // Click outside the sheet (and outside the deck button) dismisses it —
    // only while it is open (the closing fade takes no input).
    if state.export.open && ctx.input(|i| i.pointer.any_click()) {
        if let Some(pointer) = ctx.input(|i| i.pointer.interact_pos()) {
            if !area.response.rect.contains(pointer) && !anchor.contains(pointer) {
                state.export.open = false;
            }
        }
    }
}

/// The sheet's open/close fade factor: enter base, exit fast. Called every
/// editor frame (also while closed) so the animation id is registered before
/// the first open — the first open animates instead of teleporting.
pub fn sheet_fade(motion: &Motion, ctx: &egui::Context, open: bool) -> f32 {
    motion.fade(
        ctx,
        egui::Id::new("suramak-export-sheet-fade"),
        open,
        crate::motion::BASE,
        crate::motion::FAST,
    )
}

/// Whether the sheet must be drawn at this fade value (it stays drawn while
/// closing, until the fast fade has fully ended).
pub fn sheet_visible(fade: f32) -> bool {
    fade > 0.002
}

/// The sheet's contents: format chips, destination + actions, result line.
fn contents(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Export")
                .font(egui::FontId::new(17.0, crate::theme::serif_family()))
                .color(tokens.ink),
        );
    });
    ui.add_space(tokens.space.s);

    // ---- format: one segmented control (secondary tier) ---------------------
    let mut format = state.export.format;
    let options: Vec<(ExportFormat, &str)> = ExportFormat::ALL
        .iter()
        .map(|format| (*format, format.label()))
        .collect();
    if controls::segmented(ui, tokens, motion, "export-format", &mut format, &options) {
        state.set_export_format(format);
    }

    ui.add_space(tokens.space.m);

    // ---- destination --------------------------------------------------------
    ui.label(
        egui::RichText::new("Destination")
            .size(12.0)
            .color(tokens.ink_soft),
    );
    ui.add_space(tokens.space.xs);
    let width = ui.available_width();
    let response = ui.add_sized(
        [width, 24.0],
        egui::TextEdit::singleline(&mut state.export.dest)
            .hint_text("output path")
            .text_color(tokens.ink),
    );
    if response.changed() {
        let value = state.export.dest.clone();
        state.set_export_dest(value);
    }

    ui.add_space(tokens.space.m);
    ui.horizontal(|ui| {
        if HAS_NATIVE_DIALOG {
            let save_as = controls::button(
                ui,
                tokens,
                motion,
                "export-save-as",
                "Save as…",
                Tier::Secondary,
                true,
            );
            if save_as.clicked() {
                let suggested = state.export.dest.clone();
                if let Some(path) = pick_save_dest(&suggested) {
                    state.set_export_dest(path.to_string_lossy().into_owned());
                }
            }
            ui.add_space(tokens.space.s);
        }
        let export = controls::button(
            ui,
            tokens,
            motion,
            "export-run",
            "Export",
            Tier::Primary,
            state.editor.is_some(),
        );
        if export.clicked() {
            effects.extend(state.start_export());
        }
    });

    ui.add_space(tokens.space.s);
    match &state.export.result {
        Some(Ok(message)) => {
            ui.label(
                egui::RichText::new(message)
                    .size(11.5)
                    .color(tokens.ink_soft),
            );
        }
        Some(Err(message)) => {
            ui.label(
                egui::RichText::new(message)
                    .size(11.5)
                    .color(tokens.record_red),
            );
        }
        None => {
            ui.label(
                egui::RichText::new("Markdown, plain text, JSON, SRT or WebVTT — written locally.")
                    .size(11.5)
                    .color(tokens.ink_soft),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::Motion;

    fn frame(ctx: &egui::Context, time: f64, open: bool) -> f32 {
        let motion = Motion::new(false);
        let mut value = f32::NAN;
        let input = egui::RawInput {
            time: Some(time),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            value = sheet_fade(&motion, ctx, open);
        });
        value
    }

    /// The named M9c leftover: the sheet's open/close is no longer a teleport.
    /// Open = base fade (+2 pt rise), close = fast fade; the sheet keeps being
    /// drawn while it closes, then stops.
    #[test]
    fn the_sheet_fades_in_over_base_and_out_over_fast() {
        let ctx = egui::Context::default();
        // Registered closed (the editor draws the fade id every frame).
        assert_eq!(frame(&ctx, 0.0, false), 0.0);
        assert!(!sheet_visible(0.0), "a closed sheet draws nothing");
        // Opening: mid-flight during the base token, drawn while it fades.
        let opening = frame(&ctx, 0.050, true);
        assert!(opening > 0.0 && opening < 1.0, "opening: {opening}");
        assert!(sheet_visible(opening), "the opening sheet is drawn");
        // Settled open.
        assert_eq!(frame(&ctx, 1.0, true), 1.0);
        // Closing: still drawn mid-flight (the fast token)…
        let closing = frame(&ctx, 1.050, false);
        assert!(closing > 0.0 && closing < 1.0, "closing: {closing}");
        assert!(sheet_visible(closing), "the closing sheet keeps drawing");
        // …and gone once the fast token elapsed.
        assert_eq!(frame(&ctx, 1.2, false), 0.0);
        assert!(!sheet_visible(0.0));
    }

    /// Reduce Motion: both directions snap (never a fade, never a rise).
    #[test]
    fn the_sheet_fade_snaps_under_reduce_motion() {
        let ctx = egui::Context::default();
        let motion = Motion::new(true);
        let run = |time: f64, open: bool| -> f32 {
            let mut value = f32::NAN;
            let input = egui::RawInput {
                time: Some(time),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                value = sheet_fade(&motion, ctx, open);
            });
            value
        };
        assert_eq!(run(0.0, false), 0.0);
        assert_eq!(run(0.01, true), 1.0, "the open is a snap");
        assert_eq!(run(0.02, false), 0.0, "the close is a snap");
    }

    /// A rapid open → close → open retargets from the current value: the
    /// re-open never snaps back to 0 and the close never queues.
    #[test]
    fn a_rapid_toggle_retargets_from_the_current_value() {
        let ctx = egui::Context::default();
        assert_eq!(frame(&ctx, 0.0, false), 0.0);
        let climbing = frame(&ctx, 0.030, true);
        assert!(climbing > 0.0 && climbing < 1.0, "in flight: {climbing}");
        // Flip closed: the value moves down from where it was…
        let closing = frame(&ctx, 0.045, false);
        assert!(
            closing < climbing,
            "retargets down: {climbing} -> {closing}"
        );
        // …then re-open mid-close: it climbs again from the current value
        // (above the trough, never a snap to 0).
        let reopening = frame(&ctx, 0.060, true);
        assert!(
            reopening > 0.0 && reopening >= closing,
            "the re-open retargets from the current value: {closing} -> {reopening}"
        );
        // And it settles exactly at 1.
        assert_eq!(frame(&ctx, 5.0, true), 1.0);
    }
}
