//! Sidebar: the library list bound to the real store (replaces the M1
//! placeholder rows). Selecting a recording loads it into the editor.
//!
//! M15.7: the same panel carries the library search — a field above the list,
//! and, while it has a query, ranked hits with the segment's start and a
//! snippet instead of the recording list. Clicking a hit opens the transcript
//! and parks the playhead at that segment (the seek is the editor's existing
//! playback state, not a second one). No new screen is involved.
//!
//! M9a: the selected and hovered states are distinct (critique: they shared one
//! fill) — selected carries the accent tint, hover the quiet wash — and the
//! two crossfade with the fast token.
//!
//! M9c list motion: a row that appears after the first load fades + rises in
//! once (insert ease, base token); a row that leaves the list (delete, or a
//! refresh that drops it) is drawn while its height collapses and it fades out
//! in place (remove ease) — the rows below slide up as it shrinks. Both
//! animations END; the bookkeeping is pruned once the token elapsed.

use std::time::Instant;

use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Rect};

use crate::motion::{self, Motion};
use crate::theme::Tokens;

use super::search::fmt_timestamp;
use super::views::{arriving_caption, truncated_galley};
use super::{AppState, Effect, LibraryState};

/// One drawable slot of a library list (sidebar or home recents).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListSlot {
    /// A live row, by its index into `LibraryState::items`.
    Live(usize),
    /// A row that is leaving, by its index into `LibraryState::leaving`.
    Leaving(usize),
}

/// The draw order of a library list: live rows (capped when `cap` is set) and
/// leaving rows merged by their old index — a leaving row keeps its place and
/// collapses in situ, so the rows below slide up as its height shrinks.
pub fn list_slots(state: &LibraryState, cap: Option<usize>) -> Vec<ListSlot> {
    let live_count = cap.map_or(state.items.len(), |cap| cap.min(state.items.len()));
    let mut slots: Vec<(usize, u8, ListSlot)> = (0..live_count)
        .map(|index| (index, 1, ListSlot::Live(index)))
        .collect();
    for (index, row) in state.leaving.iter().enumerate() {
        if cap.is_none_or(|cap| row.index < cap) {
            slots.push((row.index, 0, ListSlot::Leaving(index)));
        }
    }
    slots.sort_by_key(|(index, rank, _)| (*index, *rank));
    slots.into_iter().map(|(_, _, slot)| slot).collect()
}

pub fn ui(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    // M15.7: the search field sits above both lists — the library view is the
    // one search surface, there is no separate screen.
    ui.add_space(10.0);
    search_field(ui, tokens, state);
    ui.add_space(2.0);
    if state.search.is_active() {
        search_results(ui, tokens, state, effects);
        return;
    }

    if state.library.items.is_empty() && state.library.leaving.is_empty() {
        // `available_rect_before_wrap`, not `max_rect`: the field above already
        // consumed the top of the panel.
        let rect = ui.available_rect_before_wrap();
        // The empty state fades in on arrival (base; a snap under Reduce
        // Motion) — nothing loops.
        for (row, text) in [
            (0.0_f32, "No recordings yet."),
            (20.0, "Drop a file to transcribe it."),
        ] {
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(Rect::from_min_size(
                    pos2(rect.left() + 16.0, rect.top() + 18.0 + row),
                    vec2((rect.width() - 16.0).max(60.0), 18.0),
                )),
                |ui| {
                    ui.horizontal(|ui| {
                        arriving_caption(
                            ui,
                            tokens,
                            motion,
                            ("library-empty", row as u32),
                            text,
                            false,
                        );
                    });
                },
            );
        }
        return;
    }

    let now = Instant::now();
    state
        .library
        .prune_motion(now, motion::token_duration(motion::BASE));
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(12.0);
            let slots = list_slots(&state.library, None);
            for slot in slots {
                match slot {
                    ListSlot::Live(index) => {
                        let (id, title, meta) = {
                            let item = &state.library.items[index];
                            (
                                item.id,
                                item.title.clone(),
                                format!(
                                    "{} · {}",
                                    item.created_at.format("%d %b %Y %H:%M"),
                                    super::model_manager::display_name(&item.model)
                                ),
                            )
                        };
                        let selected = state.library.selected == Some(index);
                        let arrival = state
                            .library
                            .row_arrival(id, now, motion::BASE)
                            .unwrap_or(1.0);
                        if sidebar_row(ui, tokens, motion, index, &title, &meta, selected, arrival)
                        {
                            effects.extend(state.select_library(index));
                        }
                    }
                    ListSlot::Leaving(leaving_index) => {
                        let row = &state.library.leaving[leaving_index];
                        let title = row.item.title.clone();
                        let meta = format!(
                            "{} · {}",
                            row.item.created_at.format("%d %b %Y %H:%M"),
                            super::model_manager::display_name(&row.item.model)
                        );
                        let progress = motion::arrival_progress(
                            now.saturating_duration_since(row.at),
                            motion::BASE,
                        );
                        leaving_row(ui, tokens, &title, &meta, progress);
                    }
                }
            }
        });
}

/// The library search box (M15.7).
fn search_field(ui: &mut egui::Ui, tokens: &Tokens, state: &mut AppState) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let width = (ui.available_width() - 16.0).max(80.0);
        let response = ui.add_sized(
            [width, 24.0],
            egui::TextEdit::singleline(&mut state.search.query)
                .hint_text("Search transcripts and terms")
                .text_color(tokens.ink)
                .margin(egui::Margin::symmetric(8, 4)),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Search the library")
        });
        if response.changed() {
            state.on_search_edited(Instant::now());
        }
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) && state.search.is_active() {
            state.search.clear();
        }
    });
}

/// The result list: snippets with their timestamps, grouped by relevance.
fn search_results(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    if let Some(note) = &state.search.seek_note {
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(note)
                    .font(FontId::new(11.0, FontFamily::Proportional))
                    .color(tokens.ink_soft),
            );
        });
        ui.add_space(2.0);
    }
    if let Some(error) = &state.search.error {
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(error)
                    .font(FontId::new(11.0, FontFamily::Proportional))
                    .color(tokens.record_red_text),
            );
        });
        return;
    }
    if state.search.results.is_empty() {
        let message = if state.search.applied.is_some() {
            "No matches."
        } else {
            "Searching…"
        };
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(message)
                    .font(FontId::new(12.0, FontFamily::Proportional))
                    .color(tokens.ink_faint),
            );
        });
        return;
    }

    let open_id = state.editor.as_ref().map(|editor| editor.transcript.id);
    let mut clicked = None;
    {
        let state_ref: &AppState = state;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height((ui.available_height() - 18.0).max(80.0))
            .show(ui, |ui| {
                for (index, hit) in state_ref.search.results.iter().enumerate() {
                    let selected = hit.transcript_id.is_some() && hit.transcript_id == open_id;
                    if search_row(ui, tokens, hit, selected) {
                        clicked = Some(index);
                    }
                }
            });
    }
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        let shown = state.search.results.len();
        let label = if state.search.total > shown {
            format!("{} matches · showing {shown}", state.search.total)
        } else if state.search.total == 1 {
            "1 match".to_string()
        } else {
            format!("{} matches", state.search.total)
        };
        ui.label(
            egui::RichText::new(label)
                .font(FontId::new(11.0, FontFamily::Proportional))
                .color(tokens.ink_faint),
        );
    });
    if let Some(index) = clicked {
        let hit = state.search.results[index].clone();
        effects.extend(state.open_search_hit(&hit));
    }
}

/// One result row: `[time] title` on the first line, the snippet (with the
/// matched word in the stronger ink) on the second. Returns true when clicked.
fn search_row(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    hit: &suramak_core::search::SearchHit,
    selected: bool,
) -> bool {
    use suramak_core::search::HitKind;

    let width = ui.available_width() - 16.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 52.0), egui::Sense::click());
    let rect = Rect::from_min_size(pos2(rect.left() + 8.0, rect.top()), vec2(width, 52.0));

    let label = match (hit.kind, hit.start_ms) {
        (HitKind::Segment, Some(ms)) => format!("Open at {}", fmt_timestamp(ms)),
        _ => format!("Open {}", hit.title.as_deref().unwrap_or("the text rules")),
    };
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, &label)
    });
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(tokens.radius.row),
            egui::Stroke::new(tokens.stroke.focus, tokens.accent),
            egui::StrokeKind::Inside,
        );
    }
    let fill = if response.hovered() && !selected {
        tokens.surface_hover
    } else if selected {
        tokens.accent_tint
    } else {
        Color32::TRANSPARENT
    };
    if fill.a() > 0 {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(tokens.radius.row), fill);
    }

    // Left slot: the segment's start (or the corpus kind), in the accent ink so
    // the eye can scan timestamps without reading titles.
    let stamp = match hit.kind {
        HitKind::Segment => hit.start_ms.map(fmt_timestamp).unwrap_or_default(),
        HitKind::Title => "Title".to_string(),
        HitKind::Glossary => "Glossary".to_string(),
        HitKind::Dictionary => "Dictionary".to_string(),
    };
    ui.painter().text(
        pos2(rect.left() + 10.0, rect.top() + 7.0),
        Align2::LEFT_TOP,
        &stamp,
        FontId::new(11.0, FontFamily::Monospace),
        tokens.accent,
    );

    let title_x = rect.left() + 76.0;
    let title = hit
        .title
        .clone()
        .unwrap_or_else(|| "Text rules".to_string());
    let galley = truncated_galley(
        ui,
        &title,
        FontId::new(13.0, FontFamily::Proportional),
        tokens.ink,
        (rect.right() - title_x - 8.0).max(40.0),
    );
    ui.painter()
        .galley(pos2(title_x, rect.top() + 5.0), galley, tokens.ink);

    let second = match hit.kind {
        HitKind::Title => "Transcript · title match".to_string(),
        HitKind::Glossary | HitKind::Dictionary => "Text rules · opens Settings".to_string(),
        HitKind::Segment if hit.from_source_text => {
            format!("{}  (original text)", hit.snippet)
        }
        HitKind::Segment => hit.snippet.clone(),
    };
    let highlight = if hit.kind == HitKind::Segment {
        hit.highlight
    } else {
        None
    };
    let galley = snippet_galley(
        ui,
        &second,
        highlight,
        FontId::new(11.5, FontFamily::Proportional),
        tokens.ink_soft,
        tokens.ink,
        (rect.width() - 20.0).max(60.0),
    );
    ui.painter().galley(
        pos2(rect.left() + 10.0, rect.top() + 27.0),
        galley,
        tokens.ink_soft,
    );

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// A one-row snippet galley where the matched span is drawn in `strong`.
fn snippet_galley(
    ui: &egui::Ui,
    text: &str,
    highlight: Option<(usize, usize)>,
    font: FontId,
    base: Color32,
    strong: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    let mut section = |range: std::ops::Range<usize>, color: Color32| {
        if range.start < range.end
            && text.is_char_boundary(range.start)
            && text.is_char_boundary(range.end)
        {
            job.append(
                &text[range],
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color,
                    ..Default::default()
                },
            );
        }
    };
    match highlight {
        Some((start, len))
            if start + len <= text.len()
                && text.is_char_boundary(start)
                && text.is_char_boundary(start + len) =>
        {
            section(0..start, base);
            section(start..start + len, strong);
            section(start + len..text.len(), base);
        }
        _ => section(0..text.len(), base),
    }
    job.wrap = egui::text::TextWrapping {
        max_width,
        max_rows: 1,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    ui.ctx().fonts_mut(|fonts| fonts.layout_job(job))
}

/// One sidebar row (title + meta): selected = accent tint, hover = quiet wash
/// (distinct states, both animated); `arrival` fades + rises the row on an
/// insert (1.0 = settled). Returns true when clicked.
#[allow(clippy::too_many_arguments)]
fn sidebar_row(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    index: usize,
    title: &str,
    meta: &str,
    selected: bool,
    arrival: f32,
) -> bool {
    let width = ui.available_width() - 16.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), egui::Sense::click());
    let rect = Rect::from_min_size(pos2(rect.left() + 8.0, rect.top()), vec2(width, 40.0));
    // M9b a11y: the painted row is a labeled button; the selected state rides
    // along, and focus draws the one ring.
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            true,
            selected,
            format!("Open {title}"),
        )
    });
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(tokens.radius.row),
            egui::Stroke::new(tokens.stroke.focus, tokens.accent),
            egui::StrokeKind::Inside,
        );
    }
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
    let rise = motion::rise(arrival, 3.0);
    painter.text(
        pos2(rect.left() + 10.0, rect.top() + 5.0 + rise),
        Align2::LEFT_TOP,
        title,
        FontId::new(13.0, FontFamily::Proportional),
        tokens.ink.gamma_multiply(arrival),
    );
    painter.text(
        pos2(rect.left() + 10.0, rect.top() + 23.0 + rise),
        Align2::LEFT_TOP,
        meta,
        FontId::new(11.0, FontFamily::Proportional),
        tokens.ink_soft.gamma_multiply(arrival),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// A row that is leaving: it collapses its height and fades out in place
/// (`progress` 0 → 1 over the base token), so the list closes the gap
/// smoothly. Non-interactive — it is on its way out. Shared by the sidebar and
/// the home recents list.
pub(super) fn leaving_row(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    title: &str,
    meta: &str,
    progress: f32,
) {
    let alpha = (1.0 - progress).clamp(0.0, 1.0);
    let width = ui.available_width() - 16.0;
    let height = 40.0 * alpha;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), egui::Sense::hover());
    if height < 10.0 {
        return;
    }
    let rect = Rect::from_min_size(pos2(rect.left() + 8.0, rect.top()), vec2(width, height));
    let painter = ui.painter();
    painter.text(
        pos2(rect.left() + 10.0, rect.top() + 5.0),
        Align2::LEFT_TOP,
        title,
        FontId::new(13.0, FontFamily::Proportional),
        tokens.ink.gamma_multiply(alpha),
    );
    painter.text(
        pos2(rect.left() + 10.0, rect.top() + 23.0),
        Align2::LEFT_TOP,
        meta,
        FontId::new(11.0, FontFamily::Proportional),
        tokens.ink_soft.gamma_multiply(alpha),
    );
}
