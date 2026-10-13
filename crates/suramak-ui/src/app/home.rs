//! Home route: the drop zone (idle state), the import affordance and the recent
//! recordings list bound to the store.
//!
//! Import paths: winit `WindowEvent::DroppedFile` (all platforms, handled in
//! `main.rs`) plus — on macOS/Windows — an rfd open dialog; on Linux (rfd is
//! deliberately not a dependency there) the path field + Enter.

use std::path::PathBuf;

use egui::{pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Rect, Stroke, StrokeKind};

use crate::theme::Palette;

use super::views::{pane_caption, pane_title, PANE_PAD_X};
use super::{AppState, Effect};

/// macOS/Windows: native open dialog (rfd). Linux: `None` — the path field is
/// the affordance there (avoids the zbus/portal dependency chain).
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn pick_audio_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Import audio")
        .add_filter("Audio", &["wav", "mp3", "m4a", "flac", "ogg", "aiff"])
        .pick_file()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn pick_audio_file() -> Option<PathBuf> {
    None
}

/// Whether this build offers the native "Choose file…" dialog.
const HAS_NATIVE_DIALOG: bool = cfg!(any(target_os = "macos", target_os = "windows"));

pub fn ui(ui: &mut egui::Ui, palette: &Palette, state: &mut AppState, effects: &mut Vec<Effect>) {
    let rect = ui.max_rect();
    let left = rect.left() + PANE_PAD_X;
    let right = rect.right() - PANE_PAD_X;

    ui.horizontal(|ui| {
        ui.add_space(PANE_PAD_X - 8.0);
        ui.vertical(|ui| {
            pane_title(ui, "New transcription", palette);
            pane_caption(
                ui,
                "Drop an audio file anywhere in this window, or import one below.",
                palette,
                false,
            );
        });
    });

    // ---- drop zone card -----------------------------------------------------
    let zone_top = rect.top() + 84.0;
    let zone = Rect::from_min_size(pos2(left, zone_top), vec2(right - left, 168.0));
    let painter = ui.painter();
    painter.rect_filled(zone, CornerRadius::same(10), palette.card);
    let border = if state.home.drop_hover {
        Stroke::new(1.6_f32, palette.accent)
    } else {
        Stroke::new(1.0_f32, palette.hairline)
    };
    painter.rect_stroke(zone, CornerRadius::same(10), border, StrokeKind::Inside);
    painter.text(
        pos2(zone.center().x, zone.center().y - 12.0),
        Align2::CENTER_CENTER,
        if state.home.drop_hover {
            "Release to transcribe"
        } else {
            "Drop a file here"
        },
        FontId::new(24.0, crate::theme::serif_family()),
        palette.text,
    );
    painter.text(
        pos2(zone.center().x, zone.center().y + 20.0),
        Align2::CENTER_CENTER,
        "wav · mp3 · m4a · flac · ogg — transcribed on this machine, never uploaded",
        FontId::new(12.0, FontFamily::Proportional),
        palette.text_dim,
    );

    // ---- import affordance --------------------------------------------------
    let row_top = zone.bottom() + 18.0;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(
            pos2(left, row_top),
            vec2(right - left, 30.0),
        )),
        |ui| {
            ui.horizontal(|ui| {
                if HAS_NATIVE_DIALOG && ui.button("Choose file…").clicked() {
                    if let Some(path) = pick_audio_file() {
                        effects.extend(state.begin_import(path));
                    }
                }
                let field_width =
                    ui.available_width() - if HAS_NATIVE_DIALOG { 84.0 } else { 96.0 };
                let response = ui.add_sized(
                    [field_width.max(120.0), 24.0],
                    egui::TextEdit::singleline(&mut state.home.path_field)
                        .hint_text("path to an audio file — press Enter to import"),
                );
                let submitted =
                    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let clicked = ui.button("Import").clicked();
                if submitted || clicked {
                    effects.extend(state.import_from_field());
                }
            });
        },
    );

    if let Some(error) = &state.home.error {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(PANE_PAD_X - 8.0);
            pane_caption(ui, error, palette, true);
        });
    }

    // ---- recent recordings --------------------------------------------------
    let list_top = row_top + 46.0;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(
            pos2(left, list_top),
            vec2(right - left, rect.bottom() - list_top - 8.0),
        )),
        |ui| {
            ui.vertical(|ui| {
                super::views::section_title(ui, "Recent recordings", palette);
                ui.add_space(6.0);
                if let Some(error) = &state.library.error {
                    pane_caption(ui, error, palette, true);
                } else if state.library.items.is_empty() {
                    pane_caption(
                        ui,
                        "Nothing yet — your transcriptions will appear here.",
                        palette,
                        false,
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for index in 0..state.library.items.len().min(6) {
                                let (title, meta) = {
                                    let item = &state.library.items[index];
                                    (
                                        item.title.clone(),
                                        format!(
                                            "{} · {}",
                                            item.created_at.format("%d %b %Y %H:%M"),
                                            item.model
                                        ),
                                    )
                                };
                                if recording_row(ui, &title, &meta, palette) {
                                    effects.extend(state.select_library(index));
                                }
                            }
                        });
                }
            });
        },
    );
}

/// One clickable recording row (title + meta). Returns true when clicked.
pub fn recording_row(ui: &mut egui::Ui, title: &str, meta: &str, palette: &Palette) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 40.0), egui::Sense::click());
    let hovered = response.hovered();
    let painter = ui.painter();
    if hovered {
        painter.rect_filled(rect, CornerRadius::same(6), palette.row_selected);
    }
    painter.text(
        pos2(rect.left() + 10.0, rect.top() + 5.0),
        Align2::LEFT_TOP,
        title,
        FontId::new(13.0, FontFamily::Proportional),
        palette.text,
    );
    painter.text(
        pos2(rect.left() + 10.0, rect.top() + 22.0),
        Align2::LEFT_TOP,
        meta,
        FontId::new(11.0, FontFamily::Proportional),
        palette.text_dim,
    );
    response.clicked()
}
