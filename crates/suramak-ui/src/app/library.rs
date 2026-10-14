//! Sidebar: the library list bound to the real store (replaces the M1
//! placeholder rows). Selecting a recording loads it into the editor.
//!
//! M9a: the selected and hovered states are distinct (critique: they shared one
//! fill) — selected carries the accent tint, hover the quiet wash — and the
//! two crossfade with the fast token.

use egui::{pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Rect};

use crate::motion::{self, Motion};
use crate::theme::Tokens;

use super::{AppState, Effect};

pub fn ui(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    if state.library.items.is_empty() {
        let rect = ui.max_rect();
        let painter = ui.painter();
        painter.text(
            pos2(rect.left() + 16.0, rect.top() + 18.0),
            Align2::LEFT_TOP,
            "No recordings yet.",
            FontId::new(12.0, FontFamily::Proportional),
            tokens.ink_soft,
        );
        painter.text(
            pos2(rect.left() + 16.0, rect.top() + 38.0),
            Align2::LEFT_TOP,
            "Drop a file to transcribe it.",
            FontId::new(12.0, FontFamily::Proportional),
            tokens.ink_soft,
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
                if sidebar_row(ui, tokens, motion, index, &title, &meta, selected) {
                    effects.extend(state.select_library(index));
                }
            }
        });
}

/// One sidebar row (title + meta): selected = accent tint, hover = quiet wash
/// (distinct states, both animated). Returns true when clicked.
fn sidebar_row(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    index: usize,
    title: &str,
    meta: &str,
    selected: bool,
) -> bool {
    let width = ui.available_width() - 16.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), egui::Sense::click());
    let rect = Rect::from_min_size(pos2(rect.left() + 8.0, rect.top()), vec2(width, 40.0));
    let hover = motion.factor(
        ui.ctx(),
        egui::Id::new(("suramak-sidebar-hover", index)),
        response.hovered() && !selected,
        motion::FAST,
    );
    let tint = motion.factor(
        ui.ctx(),
        egui::Id::new(("suramak-sidebar-selected", index)),
        selected,
        motion::FAST,
    );
    let painter = ui.painter();
    let fill = motion::tint_over(
        motion::tint_over(egui::Color32::TRANSPARENT, tokens.surface_hover, hover),
        tokens.accent_tint,
        tint,
    );
    if fill.a() > 0 {
        painter.rect_filled(rect, CornerRadius::same(tokens.radius.row), fill);
    }
    if tint > 0.01 {
        // The selected row also carries the accent edge bar (same active-row
        // language as the transcript).
        painter.rect_filled(
            Rect::from_min_size(
                pos2(rect.left() + 1.0, rect.top() + 6.0),
                vec2(3.0, rect.height() - 12.0),
            ),
            CornerRadius::same(2),
            tokens.accent.gamma_multiply(tint),
        );
    }
    painter.text(
        pos2(rect.left() + 10.0, rect.top() + 5.0),
        Align2::LEFT_TOP,
        title,
        FontId::new(13.0, FontFamily::Proportional),
        tokens.ink,
    );
    painter.text(
        pos2(rect.left() + 10.0, rect.top() + 23.0),
        Align2::LEFT_TOP,
        meta,
        FontId::new(11.0, FontFamily::Proportional),
        tokens.ink_soft,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}
