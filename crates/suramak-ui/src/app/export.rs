//! Export panel: format picker (md / txt / json / srt / vtt — rendered by
//! `suramak-core`'s export module) + destination. macOS/Windows get the rfd save
//! dialog; Linux uses the output path field (rfd is not a dependency there).

use std::path::PathBuf;

use egui::FontFamily;

use crate::theme::Palette;

use super::views::section_title;
use super::{AppState, Effect, ExportFormat};

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

pub fn ui(ui: &mut egui::Ui, palette: &Palette, state: &mut AppState, effects: &mut Vec<Effect>) {
    section_title(ui, "Export", palette);
    ui.add_space(6.0);

    // ---- format picker ------------------------------------------------------
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Format")
                .size(12.0)
                .color(palette.text_dim),
        );
        ui.add_space(6.0);
        for format in ExportFormat::ALL {
            let selected = state.export.format == format;
            if ui
                .selectable_label(
                    selected,
                    egui::RichText::new(format.label())
                        .size(12.0)
                        .family(FontFamily::Monospace),
                )
                .clicked()
                && !selected
            {
                state.set_export_format(format);
            }
        }
    });

    ui.add_space(6.0);

    // ---- destination + action ----------------------------------------------
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Destination")
                .size(12.0)
                .color(palette.text_dim),
        );
        let reserved = if HAS_NATIVE_DIALOG { 84.0 } else { 0.0 };
        let width = (ui.available_width() - reserved - 78.0).max(120.0);
        let response = ui.add_sized(
            [width, 24.0],
            egui::TextEdit::singleline(&mut state.export.dest).hint_text("output path"),
        );
        if response.changed() {
            let value = state.export.dest.clone();
            state.set_export_dest(value);
        }
        if HAS_NATIVE_DIALOG && ui.button("Save as…").clicked() {
            let suggested = state.export.dest.clone();
            if let Some(path) = pick_save_dest(&suggested) {
                state.set_export_dest(path.to_string_lossy().into_owned());
            }
        }
        if ui.button("Export").clicked() {
            effects.extend(state.start_export());
        }
    });

    // ---- result line --------------------------------------------------------
    ui.add_space(4.0);
    match &state.export.result {
        Some(Ok(message)) => {
            ui.label(
                egui::RichText::new(message)
                    .size(11.5)
                    .color(palette.text_dim),
            );
        }
        Some(Err(message)) => {
            ui.label(
                egui::RichText::new(message)
                    .size(11.5)
                    .color(palette.record_red),
            );
        }
        None => {
            ui.label(
                egui::RichText::new("Markdown, plain text, JSON, SRT or WebVTT — written locally.")
                    .size(11.5)
                    .color(palette.text_dim),
            );
        }
    }
}
