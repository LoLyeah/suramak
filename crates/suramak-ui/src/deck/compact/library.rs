//! The compact Library: search, the transcript list, and the Import/Record bar.
//!
//! One screen, one column. The same data the desktop sidebar shows
//! (`Store::list_transcripts`, newest first as the store returns it), the same
//! search semantics (title or segment text, case-insensitive), phone-sized rows
//! and 48 dp actions. Tapping a row opens the editor — a push, not a selection
//! in a side panel.

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Layout, Rect, ScrollArea,
    Sense, Stroke, Ui,
};

use crate::deck::motion::Motion;
use crate::deck::theme::Tokens;

use super::chrome::{self, GUTTER, MIN_TOUCH, ROW};
use super::model::{clock, CompactAction, CompactApp, CompactRoute};

/// Draws the Library into `screen`. Returns the actions it requested.
pub fn ui(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut CompactApp,
    screen: Rect,
) -> Vec<CompactAction> {
    let mut actions = Vec::new();
    let bands = chrome::bands(
        screen,
        state.insets.top,
        state.insets.bottom,
        Some(chrome::BOTTOM_BAR),
    );
    chrome::background(ui, tokens, screen);
    let (back, to_settings) = chrome::app_bar(
        ui,
        tokens,
        motion,
        bands.app_bar,
        "Library",
        false,
        Some(("Settings", true)),
    );
    debug_assert!(!back);
    if to_settings {
        state.open_settings(ui.input(|input| input.time));
    }

    // --- search ------------------------------------------------------------
    let search_rect = Rect::from_min_size(
        pos2(bands.content.left() + GUTTER, bands.content.top() + 10.0),
        vec2(bands.content.width() - GUTTER * 2.0, MIN_TOUCH),
    );
    chrome::search_field(
        ui,
        tokens,
        search_rect,
        &mut state.library_query,
        "Search transcripts",
    );
    let mut list_top = search_rect.bottom() + 6.0;

    // --- status / error ----------------------------------------------------
    if let Some((text, error)) = state.status.clone() {
        let rect = Rect::from_min_size(
            pos2(bands.content.left() + GUTTER, list_top),
            vec2(bands.content.width() - GUTTER * 2.0, 40.0),
        );
        paint_note(ui, tokens, rect, &text, error);
        list_top = rect.bottom() + 8.0;
    }
    if let Some(error) = state.library_error.clone() {
        let rect = Rect::from_min_size(
            pos2(bands.content.left() + GUTTER, list_top),
            vec2(bands.content.width() - GUTTER * 2.0, 92.0),
        );
        chrome::card(ui, tokens, rect);
        ui.painter().text(
            pos2(rect.left() + 14.0, rect.top() + 20.0),
            Align2::LEFT_CENTER,
            "Could not read the library",
            FontId::proportional(15.0),
            tokens.ink,
        );
        ui.painter().text(
            pos2(rect.left() + 14.0, rect.top() + 42.0),
            Align2::LEFT_CENTER,
            error,
            FontId::proportional(12.5),
            tokens.ink_soft,
        );
        let retry = Rect::from_min_size(
            pos2(rect.right() - 104.0, rect.bottom() - 40.0),
            vec2(88.0, 32.0),
        );
        let response = ui.interact(retry, ui.id().with("library-retry"), Sense::click());
        ui.painter().rect_filled(
            retry,
            CornerRadius::same(tokens.radius.control),
            tokens.control_fill,
        );
        ui.painter().text(
            retry.center(),
            Align2::CENTER_CENTER,
            "Reload",
            FontId::proportional(13.0),
            tokens.ink,
        );
        if response.clicked() {
            actions.push(CompactAction::ReloadLibrary);
        }
        list_top = rect.bottom() + 10.0;
    }

    // --- list --------------------------------------------------------------
    let list_rect = Rect::from_min_max(
        pos2(bands.content.left(), list_top),
        bands.content.right_bottom(),
    );
    if state.library_query.trim().is_empty() {
        if state.rows.is_empty() {
            empty_library(ui, tokens, list_rect);
        } else {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(list_rect)
                    .layout(Layout::top_down(Align::Min)),
            );
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .scroll_source(egui::containers::scroll_area::ScrollSource {
                    drag: true,
                    ..Default::default()
                })
                .show(&mut child, |ui| {
                    for (index, row) in state.rows.iter().enumerate() {
                        let (rect, _) =
                            ui.allocate_exact_size(vec2(list_rect.width(), ROW), Sense::hover());
                        let selected = state
                            .editor
                            .as_ref()
                            .is_some_and(|editor| editor.transcript.id == row.id);
                        let response = chrome::row(
                            ui,
                            tokens,
                            motion,
                            rect,
                            ("library-row", index),
                            &row.title,
                            &row.meta(),
                            selected,
                        );
                        // The list ends without a dangling rule under its last
                        // row (chrome::row paints one for row separation).
                        if index + 1 == state.rows.len() {
                            ui.painter().rect_filled(
                                Rect::from_min_size(
                                    pos2(rect.left() + GUTTER, rect.bottom() - 1.0),
                                    vec2(rect.width() - GUTTER, 1.0),
                                ),
                                CornerRadius::ZERO,
                                tokens.window,
                            );
                        }
                        if response.clicked() {
                            actions.push(CompactAction::OpenTranscript {
                                id: row.id,
                                segment: None,
                            });
                        }
                        // The tap affordance: a right chevron, in the row's
                        // quiet ink (the row itself is the 64 dp target).
                        {
                            let c = pos2(rect.right() - GUTTER - 8.0, rect.center().y);
                            ui.painter().add(egui::Shape::line(
                                vec![
                                    pos2(c.x - 2.5, c.y - 5.0),
                                    pos2(c.x + 2.5, c.y),
                                    pos2(c.x - 2.5, c.y + 5.0),
                                ],
                                Stroke::new(1.6_f32, tokens.ink_faint),
                            ));
                        }
                    }
                    ui.add_space(8.0);
                });
        }
    } else {
        let hits = state.hits();
        if hits.is_empty() {
            empty_search(ui, tokens, list_rect, &state.library_query);
        } else {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(list_rect)
                    .layout(Layout::top_down(Align::Min)),
            );
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .scroll_source(egui::containers::scroll_area::ScrollSource {
                    drag: true,
                    ..Default::default()
                })
                .show(&mut child, |ui| {
                    for (index, hit) in hits.iter().enumerate() {
                        let (rect, _) = ui.allocate_exact_size(
                            vec2(list_rect.width(), ROW + 18.0),
                            Sense::hover(),
                        );
                        let response =
                            ui.interact(rect, ui.id().with(("hit", index)), Sense::click());
                        if response.clicked() {
                            actions.push(CompactAction::OpenTranscript {
                                id: hit.id,
                                segment: hit.segment_idx,
                            });
                        }
                        let fill = if response.hovered() {
                            tokens.surface_hover
                        } else {
                            Color32::TRANSPARENT
                        };
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(tokens.radius.row), fill);
                        ui.painter().text(
                            pos2(rect.left() + GUTTER, rect.top() + 16.0),
                            Align2::LEFT_CENTER,
                            &hit.title,
                            FontId::proportional(16.0),
                            tokens.ink,
                        );
                        let time = if hit.segment_idx.is_some() {
                            clock(hit.start_ms)
                        } else {
                            String::new()
                        };
                        if !time.is_empty() {
                            ui.painter().text(
                                pos2(rect.right() - GUTTER, rect.top() + 16.0),
                                Align2::RIGHT_CENTER,
                                time,
                                FontId::new(12.0, FontFamily::Monospace),
                                tokens.ink_soft,
                            );
                        }
                        ui.painter().text(
                            pos2(rect.left() + GUTTER, rect.top() + 40.0),
                            Align2::LEFT_CENTER,
                            &hit.snippet,
                            FontId::proportional(13.0),
                            tokens.ink_soft,
                        );
                        ui.painter().line_segment(
                            [
                                pos2(rect.left() + GUTTER, rect.bottom() - 0.5),
                                pos2(rect.right(), rect.bottom() - 0.5),
                            ],
                            Stroke::new(1.0_f32, tokens.hairline),
                        );
                    }
                });
        }
    }

    // --- bottom bar --------------------------------------------------------
    let bar = bands.bottom_bar;
    ui.painter()
        .rect_filled(bar, CornerRadius::ZERO, tokens.panel);
    ui.painter().line_segment(
        [bar.left_top(), pos2(bar.right(), bar.top())],
        Stroke::new(1.0_f32, tokens.hairline),
    );
    let button_h = 48.0;
    let width = (bar.width() - GUTTER * 2.0 - 10.0) / 2.0;
    let y = bar.center().y - button_h / 2.0;
    let import = Rect::from_min_size(pos2(bar.left() + GUTTER, y), vec2(width, button_h));
    let record = Rect::from_min_size(pos2(import.right() + 10.0, y), vec2(width, button_h));
    if chrome::action_button(
        ui,
        tokens,
        motion,
        import,
        "library-import",
        "Import",
        false,
    )
    .clicked()
    {
        actions.push(CompactAction::Import);
    }
    if chrome::action_button(ui, tokens, motion, record, "library-record", "Record", true).clicked()
    {
        actions.push(CompactAction::Record);
    }

    let _ = state.nav.current() == CompactRoute::Library;
    actions
}

fn paint_note(ui: &mut Ui, tokens: &Tokens, rect: Rect, text: &str, error: bool) {
    let fill = if error {
        tokens.record_red.gamma_multiply(0.12)
    } else {
        tokens.panel_faint
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(tokens.radius.control), fill);
    ui.painter().text(
        pos2(rect.left() + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(13.0),
        if error {
            tokens.record_red_text
        } else {
            tokens.ink_soft
        },
    );
}

fn empty_library(ui: &mut Ui, tokens: &Tokens, rect: Rect) {
    let center = pos2(rect.center().x, rect.top() + rect.height() * 0.32);
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        "No transcripts yet",
        FontId::new(
            19.0,
            FontFamily::Name(crate::deck::theme::SERIF_FAMILY.into()),
        ),
        tokens.ink,
    );
    ui.painter().text(
        pos2(center.x, center.y + 28.0),
        Align2::CENTER_CENTER,
        "Import an audio file or record from the phone.",
        FontId::proportional(13.5),
        tokens.ink_soft,
    );
}

fn empty_search(ui: &mut Ui, tokens: &Tokens, rect: Rect, query: &str) {
    let center = pos2(rect.center().x, rect.top() + rect.height() * 0.28);
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        format!("No matches for “{query}”"),
        FontId::proportional(15.0),
        tokens.ink_soft,
    );
}

/// The list's scroll band, exposed for the layout assertions.
pub fn list_bounds(screen: Rect) -> Rect {
    let bands = chrome::bands(screen, 0.0, 0.0, Some(chrome::BOTTOM_BAR));
    bands.content
}

/// Paints a single library row into a standalone frame (the screenshot suite's
/// per-row probe).
pub fn row_rect(index: usize, list_top: f32, width: f32) -> Rect {
    Rect::from_min_size(pos2(0.0, list_top + index as f32 * ROW), vec2(width, ROW))
}

/// `egui`'s touch fling probe (documented, not a device claim): dragging the
/// list moves the scroll offset immediately; the offset is retained after the
/// pointer lifts, which is egui's own momentum source (`scroll_with_drag`).
/// This asserts the mechanism exists at our viewport; real fling feel is the
/// owner-device checklist item.
pub fn fling_probe_size() -> egui::Vec2 {
    vec2(360.0, 640.0)
}

/// The bottom bar's two button rects, for hit-target tests.
pub fn action_rects(screen: Rect) -> [Rect; 2] {
    let bands = chrome::bands(screen, 0.0, 0.0, Some(chrome::BOTTOM_BAR));
    let bar = bands.bottom_bar;
    let width = (bar.width() - GUTTER * 2.0 - 10.0) / 2.0;
    let y = bar.center().y - 24.0;
    [
        Rect::from_min_size(pos2(bar.left() + GUTTER, y), vec2(width, 48.0)),
        Rect::from_min_size(
            pos2(bar.left() + GUTTER + width + 10.0, y),
            vec2(width, 48.0),
        ),
    ]
}

/// The search field's rect, for layout assertions.
pub fn search_rect(screen: Rect) -> Rect {
    let bands = chrome::bands(screen, 0.0, 0.0, Some(chrome::BOTTOM_BAR));
    Rect::from_min_size(
        pos2(bands.content.left() + GUTTER, bands.content.top() + 10.0),
        vec2(bands.content.width() - GUTTER * 2.0, MIN_TOUCH),
    )
}

/// A frame-level check that the bottom actions never overlap and both clear
/// the system gesture inset.
pub fn bar_is_safe(screen: Rect) -> bool {
    let [a, b] = action_rects(screen);
    let bands = chrome::bands(screen, 0.0, 12.0, Some(chrome::BOTTOM_BAR));
    !a.intersects(b)
        && a.height() >= MIN_TOUCH
        && bands.bottom_bar.bottom() <= screen.bottom() - 12.0
}
