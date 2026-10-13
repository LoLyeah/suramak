//! Sidebar: the library list bound to the real store (replaces the M1
//! placeholder rows). Selecting a recording loads it into the editor.

use egui::{pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Rect};

use crate::theme::Palette;

use super::{AppState, Effect};

pub fn ui(ui: &mut egui::Ui, palette: &Palette, state: &mut AppState, effects: &mut Vec<Effect>) {
    if state.library.items.is_empty() {
        let rect = ui.max_rect();
        let painter = ui.painter();
        painter.text(
            pos2(rect.left() + 16.0, rect.top() + 18.0),
            Align2::LEFT_TOP,
            "No recordings yet.",
            FontId::new(12.0, FontFamily::Proportional),
            palette.text_dim,
        );
        painter.text(
            pos2(rect.left() + 16.0, rect.top() + 38.0),
            Align2::LEFT_TOP,
            "Drop a file to transcribe it.",
            FontId::new(12.0, FontFamily::Proportional),
            palette.text_dim,
        );
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(12.0);
            for index in 0..state.library.items.len() {
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
                let selected = state.library.selected == Some(index);
                if sidebar_row(ui, &title, &meta, selected, palette) {
                    effects.extend(state.select_library(index));
                }
            }
        });
}

/// One sidebar row (title + meta), highlighted when selected. Returns true when
/// clicked. Visuals follow the M1 placeholder rows.
fn sidebar_row(
    ui: &mut egui::Ui,
    title: &str,
    meta: &str,
    selected: bool,
    palette: &Palette,
) -> bool {
    let width = ui.available_width() - 16.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), egui::Sense::click());
    let rect = Rect::from_min_size(pos2(rect.left() + 8.0, rect.top()), vec2(width, 40.0));
    let painter = ui.painter();
    if selected || response.hovered() {
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
        pos2(rect.left() + 10.0, rect.top() + 23.0),
        Align2::LEFT_TOP,
        meta,
        FontId::new(11.0, FontFamily::Proportional),
        palette.text_dim,
    );
    response.clicked()
}
