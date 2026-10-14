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
pub fn sheet(
    ctx: &egui::Context,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
    anchor: Rect,
) {
    let pos = pos2(
        (anchor.right() - SHEET_WIDTH).max(anchor.left()),
        anchor.bottom() + 8.0,
    );
    let area = egui::Area::new(egui::Id::new("suramak-export-sheet"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .movable(false)
        .constrain(true)
        .fade_in(false)
        .show(ctx, |ui| {
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

    // Click outside the sheet (and outside the deck button) dismisses it.
    if ctx.input(|i| i.pointer.any_click()) {
        if let Some(pointer) = ctx.input(|i| i.pointer.interact_pos()) {
            if !area.response.rect.contains(pointer) && !anchor.contains(pointer) {
                state.export.open = false;
            }
        }
    }
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
