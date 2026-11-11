//! The compact Library: search, the job strip, the transcript list, and the
//! Import/Record bar.
//!
//! One screen, one column. The same data the desktop sidebar shows
//! (`Store::list_transcripts`, newest first as the store returns it), the same
//! search semantics (title or segment text, case-insensitive), phone-sized rows
//! and 48 dp actions. Tapping a row opens the editor — a push, not a selection
//! in a side panel.
//!
//! **M13g2:** inserted rows fade/slide in and a removed row's slot collapses
//! (`super::motion::ListAnim`); a running job is a static strip between the
//! search field and the list (it redraws on events, not on a clock); and the
//! scroll offset is fed to the fling probe after a drag release.

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Layout, Rect, ScrollArea,
    Sense, Stroke, StrokeKind, Ui, WidgetType,
};

use crate::deck::motion::{self, Motion};
use crate::deck::theme::Tokens;

use super::a11y;
use super::chrome::{self, GUTTER, MIN_TOUCH, ROW};
use super::first_run;
use super::model::{clock, CompactAction, CompactApp, CompactRoute};

/// The job strip's height (between the search field and the list).
pub const JOB_STRIP: f32 = 46.0;

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
    a11y::describe(ui, search_rect, "Search transcripts");
    let mut list_top = search_rect.bottom() + 6.0;

    // --- status / error ----------------------------------------------------
    if let Some((text, error)) = state.status.clone() {
        let rect = Rect::from_min_size(
            pos2(bands.content.left() + GUTTER, list_top),
            vec2(bands.content.width() - GUTTER * 2.0, 40.0),
        );
        paint_note(ui, tokens, rect, &text, error);
        a11y::describe(ui, rect, &text);
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
        let response = a11y::touch(
            ui,
            tokens,
            retry,
            "library-retry",
            WidgetType::Button,
            "Reload the library",
            true,
        );
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

    // --- first-run model card ---------------------------------------------
    // A phone with no installed model must never see an empty library: the
    // release path lands on M13D's one recommendation for this device and
    // language, with the free-space line and the job's own percent. The card is
    // the Library's lead; transcripts (a restored backup) still list below it.
    if state.model_card_needed() {
        let available = (bands.content.bottom() - list_top).max(0.0);
        let height = first_run::height(state).min(available.max(220.0));
        let rect = Rect::from_min_size(
            pos2(bands.content.left() + GUTTER, list_top),
            vec2(bands.content.width() - GUTTER * 2.0, height),
        );
        actions.extend(first_run::card(ui, tokens, motion, state, rect));
        list_top = rect.bottom() + 10.0;
    }

    // --- job strip ----------------------------------------------------------
    if let Some(job) = state.job.clone() {
        let rect = Rect::from_min_size(
            pos2(bands.content.left() + GUTTER, list_top),
            vec2(bands.content.width() - GUTTER * 2.0, JOB_STRIP),
        );
        chrome::card(ui, tokens, rect);
        ui.painter().text(
            pos2(rect.left() + 14.0, rect.top() + 17.0),
            Align2::LEFT_CENTER,
            job.line(),
            FontId::proportional(13.0),
            tokens.ink,
        );
        let track = Rect::from_min_size(
            pos2(rect.left() + 14.0, rect.bottom() - 14.0),
            vec2(rect.width() - 28.0, 3.0),
        );
        ui.painter()
            .rect_filled(track, CornerRadius::same(2), tokens.control_fill);
        let fraction = state.job_fraction();
        if fraction > 0.0 {
            ui.painter().rect_filled(
                Rect::from_min_size(track.min, vec2(track.width() * fraction, 3.0)),
                CornerRadius::same(2),
                tokens.accent,
            );
        }
        a11y::describe(ui, rect, &job.line());
        list_top = rect.bottom() + 8.0;
    }

    // --- list --------------------------------------------------------------
    // M19B: with no verified model there is nothing to transcribe. The record
    // button is disabled and the reason rides directly above the bar, so the
    // list must end above that line.
    let record_reason = state.record_block_reason();
    let note_h = if record_reason.is_some() { 26.0 } else { 0.0 };
    let list_rect = Rect::from_min_max(
        pos2(bands.content.left(), list_top),
        pos2(bands.content.right(), bands.content.bottom() - note_h),
    );
    if state.library_query.trim().is_empty() {
        if state.rows.is_empty() {
            empty_library(ui, tokens, list_rect);
        } else {
            let now = ui.input(|input| input.time);
            let keys = state.library_keys();
            let snapshot = state.list_anim.sync(&keys, now, state.reduce_motion);
            let appear: Vec<f32> = snapshot.rows.iter().map(|(_, value)| *value).collect();
            let ghosts = snapshot.ghosts.clone();
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(list_rect)
                    .layout(Layout::top_down(Align::Min)),
            );
            let output = ScrollArea::vertical()
                .auto_shrink([false, false])
                .scroll_source(egui::containers::scroll_area::ScrollSource {
                    drag: true,
                    ..Default::default()
                })
                .show(&mut child, |ui| {
                    for (index, row) in state.rows.iter().enumerate() {
                        // A removed row's slot collapses before this row.
                        for ghost in ghosts.iter().filter(|ghost| ghost.at == index) {
                            ui.add_space(ROW * ghost.value);
                        }
                        let factor = appear.get(index).copied().unwrap_or(1.0);
                        let rise = motion::rise(factor, super::motion::ROW_RISE);
                        let rect = Rect::from_min_size(
                            pos2(list_rect.left(), ui.cursor().top() + rise),
                            vec2(list_rect.width(), ROW),
                        );
                        ui.allocate_space(vec2(list_rect.width(), ROW));
                        let mut row_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt(("library-row-layer", index))
                                .max_rect(rect)
                                .layout(Layout::top_down(Align::Min)),
                        );
                        row_ui.set_opacity(factor.clamp(0.0, 1.0));
                        let selected = state
                            .editor
                            .as_ref()
                            .is_some_and(|editor| editor.transcript.id == row.id);
                        let response = chrome::row(
                            &mut row_ui,
                            tokens,
                            motion,
                            rect,
                            ("library-row", index),
                            &row.title,
                            &row.meta(),
                            selected,
                        );
                        // The play affordance: a 48 dp target at the row's
                        // trailing edge opens the transcript with the transport
                        // running (the compact editor's karaoke surface).
                        let play = Rect::from_center_size(
                            pos2(rect.right() - GUTTER - 58.0, rect.center().y),
                            vec2(48.0, MIN_TOUCH),
                        );
                        let play_response = a11y::touch(
                            &mut row_ui,
                            tokens,
                            play,
                            ("library-play", index),
                            WidgetType::Button,
                            &format!("Play {}", row.title),
                            true,
                        );
                        let triangle = pos2(play.center().x + 1.0, play.center().y);
                        row_ui.painter().add(egui::Shape::convex_polygon(
                            vec![
                                pos2(triangle.x - 5.0, triangle.y - 8.0),
                                pos2(triangle.x - 5.0, triangle.y + 8.0),
                                pos2(triangle.x + 8.0, triangle.y),
                            ],
                            tokens.ink_soft,
                            Stroke::NONE,
                        ));
                        // The list ends without a dangling rule under its last
                        // row (chrome::row paints one for row separation).
                        if index + 1 == state.rows.len() {
                            row_ui.painter().rect_filled(
                                Rect::from_min_size(
                                    pos2(rect.left() + GUTTER, rect.bottom() - 1.0),
                                    vec2(rect.width() - GUTTER, 1.0),
                                ),
                                CornerRadius::ZERO,
                                tokens.window,
                            );
                        }
                        if play_response.clicked() {
                            actions.push(CompactAction::PlayTranscript(row.id));
                        } else if response.clicked() {
                            actions.push(CompactAction::OpenTranscript {
                                id: row.id,
                                segment: None,
                            });
                        }
                        // The tap affordance: a right chevron, in the row's
                        // quiet ink (the row itself is the 64 dp target).
                        {
                            let c = pos2(rect.right() - GUTTER - 8.0, rect.center().y);
                            row_ui.painter().add(egui::Shape::line(
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
            // The fling probe's input side: a release starts the measurement,
            // the following frames accumulate the offset egui's momentum moved.
            let velocity = ui.input(|input| input.pointer.velocity().length());
            let released = ui.input(|input| input.pointer.any_released());
            observe_fling(state, output.state.offset.y, velocity, released);
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
                        let response = a11y::touch(
                            ui,
                            tokens,
                            rect,
                            ("hit", index),
                            WidgetType::Button,
                            &format!("Open {} · {}", hit.title, hit.snippet),
                            true,
                        );
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
    if let Some(reason) = record_reason {
        let note = Rect::from_min_size(
            pos2(bar.left() + GUTTER, bar.top() - 24.0),
            vec2(bar.width() - GUTTER * 2.0, 20.0),
        );
        ui.painter().text(
            pos2(note.left(), note.center().y),
            Align2::LEFT_CENTER,
            reason,
            FontId::proportional(12.0),
            tokens.warn,
        );
        a11y::describe(ui, note, reason);
    }
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
    a11y::record_button(ui, import, "Import audio", true);
    let record_label = if state.recording { "Stop" } else { "Record" };
    if record_reason.is_none() {
        if chrome::action_button(
            ui,
            tokens,
            motion,
            record,
            "library-record",
            record_label,
            true,
        )
        .clicked()
        {
            actions.push(if state.recording {
                CompactAction::StopRecording
            } else {
                CompactAction::Record
            });
        }
        a11y::record_button(
            ui,
            record,
            if state.recording {
                "Stop recording"
            } else {
                "Record from the microphone"
            },
            true,
        );
    } else {
        // Disabled paint: the same rect and label, in the quiet ink — and a
        // semantic node that says it cannot be pressed.
        let reason = record_reason.unwrap_or_default();
        ui.painter().rect(
            record,
            CornerRadius::same(tokens.radius.control),
            tokens.control_fill.gamma_multiply(0.5),
            Stroke::new(1.0_f32, tokens.hairline),
            StrokeKind::Inside,
        );
        ui.painter().text(
            record.center(),
            Align2::CENTER_CENTER,
            record_label,
            FontId::proportional(15.0),
            tokens.ink_faint,
        );
        a11y::record_button(
            ui,
            record,
            &format!("Record from the microphone — {reason}"),
            false,
        );
    }

    let _ = state.nav.current() == CompactRoute::Library;
    actions
}

/// Feeds the list's scroll offset to the fling probe. Called by the screen after
/// the scroll area draws: one release starts a probe, and the following frames
/// measure the offset the momentum actually covers.
pub fn observe_fling(
    state: &mut CompactApp,
    offset: f32,
    velocity: f32,
    released: bool,
) -> Option<super::model::FlingProbe> {
    let probe = state.fling.observe(offset, velocity, released)?;
    state.note_fling(probe);
    Some(probe)
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
    let center = pos2(rect.center().x, rect.top() + rect.height() * 0.30);
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
    // The zero-empty-screen hint (M13E's two working entries): share a voice
    // note into the app, or pick a file. Record is on the bar below; the line
    // is the only place a first-time user learns the share target exists.
    let hint = Rect::from_min_size(
        pos2(rect.left() + GUTTER, center.y + 20.0),
        vec2(rect.width() - GUTTER * 2.0, 44.0),
    );
    ui.painter().text(
        pos2(hint.center().x, hint.top() + 8.0),
        Align2::CENTER_CENTER,
        "Share a voice note into Suramak, or tap Import to choose a file.",
        FontId::proportional(13.0),
        tokens.ink_soft,
    );
    ui.painter().text(
        pos2(hint.center().x, hint.top() + 28.0),
        Align2::CENTER_CENTER,
        "Record works here too — it keeps going with the screen off.",
        FontId::proportional(12.0),
        tokens.ink_faint,
    );
    a11y::describe(
        ui,
        hint,
        "Share a voice note into Suramak, or tap Import to choose a file. Record works here too.",
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
/// [`observe_fling`] records what that momentum actually did, so the owner's
/// phone run can be compared with the Deck layer's expectation.
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
