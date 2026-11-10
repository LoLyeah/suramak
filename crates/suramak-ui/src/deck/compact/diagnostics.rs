//! Settings → Diagnostics (M13g3): the M13b spike's four checks and v2.4
//! crash banner, as one compact card per check group.
//!
//! **Coverage is not reduced.** Every counter the spike's tabs exposed is
//! still on screen — the four check groups keep their evidence, and the crash
//! banner is the same `spike-last-crash.txt` the overlay and the share sheet
//! read. What changes is the shape: no tabs, no live text field, no motion
//! tab's animation harness; the read-outs and the banner are what the release
//! rotation test needs, and they are reachable from Settings.
//!
//! The screen is a view. `Clear crash` and `Share log` are actions; the shell
//! owns the file and the share intent.

use egui::{
    pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Rect, ScrollArea, Stroke, StrokeKind, Ui,
    WidgetType,
};

use crate::deck::motion::Motion;
use crate::deck::theme::Tokens;

use super::a11y;
use super::chrome::{self, GUTTER};
use super::model::{CompactAction, CompactApp};

/// Draws the Diagnostics route into `screen`.
pub fn ui(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut CompactApp,
    screen: Rect,
) -> Vec<CompactAction> {
    let mut actions = Vec::new();
    let bands = chrome::bands(screen, state.insets.top, state.insets.bottom, None);
    chrome::background(ui, tokens, screen);
    let now = ui.input(|input| input.time);
    let (back, _) = chrome::app_bar(ui, tokens, motion, bands.app_bar, "Diagnostics", true, None);
    if back {
        state.back(now);
    }
    let view = state.diagnostics.clone();
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(bands.content)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            let width = bands.content.width() - GUTTER * 2.0;
            let x = bands.content.left() + GUTTER;
            let mut y = bands.content.top() + 14.0;

            // The v2.4 banner first: whatever killed the last process is the
            // single most important line on this screen.
            if let Some(crash) = &view.crash {
                let height = 218.0;
                let rect = Rect::from_min_size(pos2(x, y), vec2(width, height));
                ui.painter().rect(
                    rect,
                    CornerRadius::same(tokens.radius.card),
                    tokens.record_red.gamma_multiply(0.12),
                    Stroke::new(1.0_f32, tokens.record_red.gamma_multiply(0.55)),
                    StrokeKind::Inside,
                );
                ui.painter().text(
                    pos2(rect.left() + 12.0, rect.top() + 20.0),
                    Align2::LEFT_CENTER,
                    crash.title(),
                    FontId::proportional(13.0),
                    tokens.record_red_text,
                );
                let body = truncate_lines(&crash.text, 7);
                let galley = ui.painter().layout(
                    body,
                    FontId::new(10.5, FontFamily::Monospace),
                    tokens.ink,
                    rect.width() - 24.0,
                );
                ui.painter().galley(
                    pos2(rect.left() + 12.0, rect.top() + 36.0),
                    galley,
                    tokens.ink,
                );
                a11y::describe(ui, rect, &format!("{}: {}", crash.title(), crash.text));
                let clear = Rect::from_min_size(
                    pos2(rect.left() + 12.0, rect.bottom() - 48.0),
                    vec2(112.0, 34.0),
                );
                let share = Rect::from_min_size(
                    pos2(rect.left() + 132.0, rect.bottom() - 48.0),
                    vec2(112.0, 34.0),
                );
                for (rect, id, label, action) in [
                    (clear, "diag-clear", "Clear crash", CompactAction::ClearCrash),
                    (share, "diag-share", "Share log", CompactAction::ShareCrashLog),
                ] {
                    let response = a11y::touch(
                        ui,
                        tokens,
                        rect,
                        id,
                        WidgetType::Button,
                        label,
                        true,
                    );
                    ui.painter().rect(
                        rect,
                        CornerRadius::same(tokens.radius.control),
                        tokens.control_fill,
                        Stroke::new(1.0_f32, tokens.hairline),
                        StrokeKind::Inside,
                    );
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        label,
                        FontId::proportional(12.0),
                        tokens.ink,
                    );
                    if response.clicked() {
                        actions.push(action);
                    }
                }
                y = rect.bottom() + 18.0;
            }

            let text_group = format!(
                "{} chars · {} IME updates · {} deletions",
                view.text_chars, view.ime_updates, view.deletions
            );
            y = group(ui, tokens, x, y, width, "1 · Text", &[("Evidence", &text_group)]);
            let lifecycle = format!(
                "{} resumes · {} surface rebuilds · {} frames gated · {} after destroy · probe {} saved / {} restored",
                view.resumes,
                view.surface_recreates,
                view.frames_gated,
                view.frames_after_destroy,
                view.probe_saves,
                view.probe_restores
            );
            y = group(ui, tokens, x, y, width, "2 · Lifecycle", &[("Evidence", &lifecycle)]);
            let motion_line = match view.avg_fps {
                Some(fps) => format!("{fps:.1} fps avg · {} frames", view.frames),
                None => format!("sampling… · {} frames", view.frames),
            };
            y = group(ui, tokens, x, y, width, "3 · Motion", &[("Evidence", &motion_line)]);
            let insets = format!(
                "{} inset pushes · top {:.0} pt / bottom {:.0} pt · {} back",
                view.inset_updates, view.inset_top, view.inset_bottom, view.back_presses
            );
            y = group(ui, tokens, x, y, width, "4 · Insets / Back", &[("Evidence", &insets)]);
            let note = "The release rotation check: rotate during launch, then read the lifecycle counters and the crash banner here.";
            let note_galley = ui.painter().layout(
                note.to_string(),
                FontId::proportional(11.0),
                tokens.ink_faint,
                width,
            );
            ui.painter()
                .galley(pos2(x, y + 4.0), note_galley.clone(), tokens.ink_faint);
            a11y::describe(
                ui,
                Rect::from_min_size(
                    pos2(x, y + 4.0),
                    vec2(width, note_galley.size().y.max(16.0)),
                ),
                note,
            );
            y += note_galley.size().y + 12.0;
            ui.allocate_space(vec2(width, (y - bands.content.top()).max(0.0)));
        });
    actions
}

/// One group card: a section title and its evidence line(s).
#[allow(clippy::too_many_arguments)]
fn group(
    ui: &mut Ui,
    tokens: &Tokens,
    x: f32,
    y: f32,
    width: f32,
    title: &str,
    rows: &[(&str, &str)],
) -> f32 {
    let height = 76.0;
    let rect = Rect::from_min_size(pos2(x, y), vec2(width, height));
    chrome::card(ui, tokens, rect);
    ui.painter().text(
        pos2(rect.left() + 12.0, rect.top() + 18.0),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(11.5),
        tokens.ink_faint,
    );
    let mut row_y = rect.top() + 34.0;
    for (label, value) in rows {
        ui.painter().text(
            pos2(rect.left() + 12.0, row_y),
            Align2::LEFT_CENTER,
            *label,
            FontId::proportional(11.0),
            tokens.ink_faint,
        );
        let galley = ui.painter().layout(
            (*value).to_string(),
            FontId::proportional(12.0),
            tokens.ink,
            width - 100.0,
        );
        ui.painter()
            .galley(pos2(rect.left() + 86.0, row_y - 7.0), galley, tokens.ink);
        a11y::describe(
            ui,
            Rect::from_min_size(
                pos2(rect.left() + 12.0, row_y - 9.0),
                vec2(width - 24.0, 18.0),
            ),
            &format!("{title}: {value}"),
        );
        row_y += 16.0;
    }
    rect.bottom() + 8.0
}

/// The banner's preview: the block's first `lines` lines (the whole text stays
/// in the a11y description and in the file the share route sends).
fn truncate_lines(text: &str, lines: usize) -> String {
    let mut out = String::new();
    for (index, line) in text.lines().take(lines).enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    if text.lines().count() > lines {
        out.push_str("\n…");
    }
    out
}
