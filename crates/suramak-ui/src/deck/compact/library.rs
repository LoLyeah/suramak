//! The compact Library: search, the model banner, the transcript list, and the
//! floating Import/Record dock.
//!
//! One screen, one column. The same data the desktop sidebar shows
//! (`Store::list_transcripts`, newest first as the store returns it), the same
//! search semantics (title or segment text, case-insensitive), phone-sized rows
//! and 48 dp actions. Tapping a row opens the editor — a push, not a selection
//! in a side panel.
//!
//! **M20C — the redraw.** The app bar carries the serif title at the type
//! scale's title token and a 44 pt gear (spoken "Settings"); the field is a
//! frosted Card plane with the magnifier and the caption placeholder; the model
//! card is ONE shrink-wrapped frosted banner that disappears once a model is
//! verified; rows wear the type scale, an inset hairline and the M20A press;
//! the empty state is the waveform + Newsreader line; and the actions float in
//! a frosted dock over the list, which pads its bottom so the last row can
//! never hide behind it.
//!
//! **M13g2:** inserted rows fade/slide in and a removed row's slot collapses
//! (`super::motion::ListAnim`); a running job is a static strip between the
//! search field and the list (it redraws on events, not on a clock); and the
//! scroll offset is fed to the fling probe after a drag release.

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, Layout, Rect, ScrollArea, Sense, Stroke,
    StrokeKind, Ui, WidgetType,
};

use crate::deck::icons::Icon;
use crate::deck::material::{self, Plane};
use crate::deck::motion::{self, Motion};
use crate::deck::theme::Tokens;
use crate::deck::type_scale::{self, TypeToken};

use super::a11y;
use super::chrome::{self, GUTTER, MIN_TOUCH, ROW};
use super::first_run;
use super::model::{clock, CompactAction, CompactApp, CompactRoute, LibraryHit, LibraryRow};

/// The job strip's height (between the search field and the list).
pub const JOB_STRIP: f32 = 46.0;
/// The gap under the app bar / search field / banner.
const SECTION_GAP: f32 = 12.0;
/// The dock's backdrop fade: how far above the dock the scrim starts.
const SCRIM_FADE: f32 = 64.0;
/// The reason chip above the dock (only when the banner is absent).
const REASON_CHIP: f32 = 26.0;

/// Draws the Library into `screen`. Returns the actions it requested.
pub fn ui(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut CompactApp,
    screen: Rect,
) -> Vec<CompactAction> {
    let mut actions = Vec::new();
    // The Library floats its dock over the list, so the content band runs to
    // the gesture inset (the dock reserves its own space via list padding).
    let bands = chrome::bands(screen, state.insets.top, state.insets.bottom, None);
    chrome::background(ui, tokens, screen);
    let (back, to_settings, anchor) = chrome::app_bar_with(
        ui,
        tokens,
        motion,
        bands.app_bar,
        "Library",
        false,
        chrome::Trailing::Icon {
            icon: Icon::Gear,
            label: Icon::Gear.label(),
            enabled: true,
        },
    );
    state.settings_anchor = anchor;
    debug_assert!(!back);
    if to_settings {
        state.open_settings(ui.input(|input| input.time));
    }

    // --- search ------------------------------------------------------------
    let search_rect = Rect::from_min_size(
        pos2(
            bands.content.left() + GUTTER,
            bands.content.top() + SECTION_GAP,
        ),
        vec2(bands.content.width() - GUTTER * 2.0, MIN_TOUCH),
    );
    let query_changed = chrome::search_field(
        ui,
        tokens,
        search_rect,
        &mut state.library_query,
        "Search transcripts",
    );
    a11y::describe(ui, search_rect, "Search transcripts");
    if query_changed {
        // The field's spoken-text half is the store's index, not the phone's
        // loaded headers: ask the host for the ranked segment hits (M22C). The
        // previous hits are dropped at once so a snippet never describes a
        // question the reader has moved past.
        state.search_hits.clear();
        actions.push(CompactAction::SearchLibrary {
            query: state.library_query.clone(),
        });
    }
    let mut list_top = search_rect.bottom() + SECTION_GAP;

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
        let content = material::card(ui, tokens, rect, None);
        let title = type_scale::row_title();
        ui.painter().text(
            pos2(content.left(), rect.top() + 24.0),
            Align2::LEFT_CENTER,
            "Could not read the library",
            title.font,
            tokens.ink,
        );
        let caption = type_scale::caption();
        ui.painter().text(
            pos2(content.left(), rect.top() + 46.0),
            Align2::LEFT_CENTER,
            error,
            caption.font,
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
        ui.painter().rect(
            retry,
            CornerRadius::same(tokens.radius.control),
            tokens.control_fill,
            Stroke::new(1.0_f32, tokens.hairline),
            StrokeKind::Inside,
        );
        ui.painter().text(
            retry.center(),
            Align2::CENTER_CENTER,
            "Reload",
            type_scale::row_title().font,
            tokens.ink,
        );
        if response.clicked() {
            actions.push(CompactAction::ReloadLibrary);
        }
        list_top = rect.bottom() + 10.0;
    }

    // --- watched inbox ------------------------------------------------------
    // M24A: one status row while watching is on: `Watching · 3 imports` and
    // the folder's display name; a named failure in the error ink when the
    // folder (or its SAF grant) is gone. Nothing is drawn when watching is off.
    if let Some(watch) = state.watch.clone() {
        let rect = Rect::from_min_size(
            pos2(bands.content.left() + GUTTER, list_top),
            vec2(bands.content.width() - GUTTER * 2.0, 40.0),
        );
        paint_watch_row(ui, tokens, rect, &watch);
        a11y::describe(ui, rect, &format!("{} · {}", watch.line(), watch.folder));
        list_top = rect.bottom() + 8.0;
    }

    // --- first-run model banner -------------------------------------------
    // A phone with no installed model must never see an empty library. The
    // banner is the release path's one recommendation; it is absent entirely
    // once a model is verified on disk.
    if state.model_card_needed() {
        let width = bands.content.width() - GUTTER * 2.0;
        let height = first_run::height(ui.ctx(), state, width);
        let rect = Rect::from_min_size(
            pos2(bands.content.left() + GUTTER, list_top),
            vec2(width, height),
        );
        actions.extend(first_run::card(ui, tokens, motion, state, rect));
        list_top = rect.bottom() + SECTION_GAP;
    }

    // --- job strip ----------------------------------------------------------
    if let Some(job) = state.job.clone() {
        let rect = Rect::from_min_size(
            pos2(bands.content.left() + GUTTER, list_top),
            vec2(bands.content.width() - GUTTER * 2.0, JOB_STRIP),
        );
        let content = material::card(ui, tokens, rect, None);
        ui.painter().text(
            pos2(content.left(), rect.top() + 18.0),
            Align2::LEFT_CENTER,
            job.line(),
            type_scale::row_value().font,
            tokens.ink,
        );
        let track = Rect::from_min_size(
            pos2(content.left(), rect.bottom() - 14.0),
            vec2(content.width(), 3.0),
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
    let dock = chrome::dock_rect(screen, state.insets.bottom);
    let record_reason = state.record_block_reason();
    // The banner already tells a model-less phone why Record is disabled; the
    // chip is for the other case (a model is installed, the configured one is
    // not) where the banner is gone and the reason would otherwise be lost.
    let show_reason = record_reason.is_some() && !state.model_card_needed();
    let bottom_pad = list_bottom_padding(screen, state.insets.bottom, show_reason);
    let list_rect = Rect::from_min_max(
        pos2(bands.content.left(), list_top),
        pos2(bands.content.right(), bands.content.bottom()),
    );

    if state.library_query.trim().is_empty() {
        if state.rows.is_empty() {
            let empty_bottom = dock.top() - if show_reason { REASON_CHIP + 8.0 } else { 0.0 };
            empty_library(ui, tokens, list_rect, empty_bottom);
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
                        let drawn = draw_library_row(
                            &mut row_ui,
                            tokens,
                            motion,
                            rect,
                            row,
                            index,
                            selected,
                            index + 1 < state.rows.len(),
                        );
                        if drawn.play_clicked {
                            actions.push(CompactAction::PlayTranscript(row.id));
                        } else if drawn.response.clicked() {
                            actions.push(CompactAction::OpenTranscript {
                                id: row.id,
                                segment: None,
                            });
                        }
                    }
                    // The dock floats over the list: the last row must be able
                    // to scroll clear of it (and of the gesture inset).
                    ui.add_space(bottom_pad);
                });
            // The fling probe's input side: a release starts the measurement,
            // the following frames accumulate the offset egui's momentum moved.
            let velocity = ui.input(|input| input.pointer.velocity().length());
            let released = ui.input(|input| input.pointer.any_released());
            observe_fling(state, output.state.offset.y, velocity, released);
        }
    } else {
        // Two sections (M22C): title matches first, in the library's own
        // newest-first order, then the store's spoken-text hits under an "in
        // transcripts" heading. Clearing the field restores the plain list
        // (the branch above) and drops the hits.
        let title_hits = state.title_matches();
        let segment_hits = state.transcript_hits();
        if title_hits.is_empty() && segment_hits.is_empty() {
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
                    for (index, row) in title_hits.iter().enumerate() {
                        let (rect, _) =
                            ui.allocate_exact_size(vec2(list_rect.width(), ROW), Sense::hover());
                        let mut row_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt(("library-title-hit", index))
                                .max_rect(rect)
                                .layout(Layout::top_down(Align::Min)),
                        );
                        let selected = state
                            .editor
                            .as_ref()
                            .is_some_and(|editor| editor.transcript.id == row.id);
                        let drawn = draw_library_row(
                            &mut row_ui,
                            tokens,
                            motion,
                            rect,
                            row,
                            index,
                            selected,
                            true,
                        );
                        if drawn.play_clicked {
                            actions.push(CompactAction::PlayTranscript(row.id));
                        } else if drawn.response.clicked() {
                            actions.push(CompactAction::OpenTranscript {
                                id: row.id,
                                segment: None,
                            });
                        }
                    }
                    if !segment_hits.is_empty() {
                        let (heading, _) =
                            ui.allocate_exact_size(vec2(list_rect.width(), 30.0), Sense::hover());
                        chrome::section_label(
                            ui,
                            tokens,
                            pos2(heading.left() + GUTTER, heading.top() + 4.0),
                            "In transcripts",
                        );
                        for (index, hit) in segment_hits.iter().enumerate() {
                            let (rect, _) = ui
                                .allocate_exact_size(vec2(list_rect.width(), ROW), Sense::hover());
                            let mut row_ui = ui.new_child(
                                egui::UiBuilder::new()
                                    .id_salt(("library-hit", index))
                                    .max_rect(rect)
                                    .layout(Layout::top_down(Align::Min)),
                            );
                            if draw_hit_row(&mut row_ui, tokens, motion, rect, hit, index) {
                                actions.push(CompactAction::OpenTranscript {
                                    id: hit.id,
                                    segment: hit.segment_idx,
                                });
                            }
                        }
                    }
                    ui.add_space(bottom_pad);
                });
        }
    }

    // --- the floating dock --------------------------------------------------
    // The scrim fades the list into the dock (no hard rule), then the Dock
    // plane floats with the one float shadow. Painted last: it is above the
    // list in both paint and hit-test order.
    material::dock_scrim(
        ui,
        Rect::from_min_max(pos2(screen.left(), dock.top() - SCRIM_FADE), screen.max),
        tokens,
    );
    if show_reason {
        if let Some(reason) = record_reason {
            paint_reason_chip(ui, tokens, dock, reason);
        }
    }
    material::frosted_panel(ui, tokens, dock, tokens.radius.card, Plane::Dock);
    let [import, record] = chrome::dock_action_rects(dock);
    if chrome::dock_button(
        ui,
        tokens,
        motion,
        import,
        "library-import",
        "Import",
        "Import audio",
        Some(Icon::ImportWave),
        false,
        true,
    )
    .clicked()
    {
        actions.push(CompactAction::Import);
    }
    let record_label = if state.recording { "Stop" } else { "Record" };
    let record_spoken = match (record_reason, state.recording) {
        (Some(reason), _) => format!("Record from the microphone — {reason}"),
        (None, true) => "Stop recording".to_string(),
        (None, false) => "Record from the microphone".to_string(),
    };
    if chrome::dock_button(
        ui,
        tokens,
        motion,
        record,
        "library-record",
        record_label,
        &record_spoken,
        Some(Icon::Mic),
        true,
        record_reason.is_none(),
    )
    .clicked()
        && record_reason.is_none()
    {
        actions.push(if state.recording {
            CompactAction::StopRecording
        } else {
            CompactAction::Record
        });
    }

    let _ = state.nav.current() == CompactRoute::Library;
    actions
}

/// What a drawn library row reported: the row's hit response and whether its
/// play affordance (not the row body) was tapped.
struct DrawnLibraryRow {
    response: egui::Response,
    play_clicked: bool,
}

/// Draws one full library row: the M20C press, the M22C anatomy (waveform,
/// pips, tabular duration), the play target and the chevron. The caller owns
/// the action mapping, so the plain list and the search's title section share
/// exactly one row implementation.
#[allow(clippy::too_many_arguments)]
fn draw_library_row(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    row: &LibraryRow,
    index: usize,
    selected: bool,
    divider: bool,
) -> DrawnLibraryRow {
    let part_label = row.part.as_ref().map(|part| part.label());
    let (response, scale) = chrome::row(
        ui,
        tokens,
        motion,
        rect,
        ("library-row", index),
        &row.title,
        &row.meta(),
        chrome::RowExtras {
            waveform: &row.waveform,
            speakers: row.speakers,
            duration_ms: row.duration_ms,
            part: part_label.as_deref(),
        },
        selected,
        divider,
    );
    // The press scales the whole painted row about its centre; the trailing
    // affordances map with it.
    let map = |point: egui::Pos2| {
        pos2(
            rect.center().x + (point.x - rect.center().x) * scale,
            rect.center().y + (point.y - rect.center().y) * scale,
        )
    };
    // The play affordance: a 48 dp target at the row's trailing edge opens the
    // transcript with the transport running (the compact editor's karaoke
    // surface).
    let play = Rect::from_center_size(
        pos2(rect.right() - GUTTER - 58.0, rect.center().y),
        vec2(48.0, MIN_TOUCH),
    );
    let play_response = a11y::touch(
        ui,
        tokens,
        play,
        ("library-play", index),
        WidgetType::Button,
        &format!("Play {}", row.title),
        true,
    );
    let triangle = map(pos2(play.center().x + 1.0, play.center().y));
    let half_w = 6.5 * scale;
    let half_h = 8.0 * scale;
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            pos2(triangle.x - half_w, triangle.y - half_h),
            pos2(triangle.x - half_w, triangle.y + half_h),
            pos2(triangle.x + half_w * 1.2, triangle.y),
        ],
        tokens.ink_soft,
        Stroke::NONE,
    ));
    // The tap affordance: a right chevron, in the row's quiet ink (the row
    // itself is the 64 dp target).
    {
        let c = map(pos2(rect.right() - GUTTER - 8.0, rect.center().y));
        ui.painter().add(egui::Shape::line(
            vec![
                pos2(c.x - 2.5 * scale, c.y - 5.0 * scale),
                pos2(c.x + 2.5 * scale, c.y),
                pos2(c.x - 2.5 * scale, c.y + 5.0 * scale),
            ],
            Stroke::new(1.6_f32 * scale, tokens.ink_faint),
        ));
    }
    DrawnLibraryRow {
        response,
        play_clicked: play_response.clicked(),
    }
}

/// Draws one "in transcripts" hit: the transcript's title, its timecode on the
/// tabular grid, and the ±60-char snippet with the matched span in the accent
/// ink (the store's `[`/`]` markers are display syntax, never shown). Returns
/// whether the row was tapped.
fn draw_hit_row(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    hit: &LibraryHit,
    index: usize,
) -> bool {
    let label = format!("Open {} · {}", hit.title, hit.snippet);
    let response = a11y::touch(
        ui,
        tokens,
        rect,
        ("library-hit", index),
        WidgetType::Button,
        &label,
        true,
    );
    let press = chrome::press_frame(
        ui.ctx(),
        ui.id().with(("library-hit-press", index)),
        response.is_pointer_button_down_on(),
        motion.reduce,
    );
    let scale = press.scale;
    let visual = Rect::from_center_size(rect.center(), rect.size() * scale);
    if press.fill > 0.0 {
        ui.painter().rect_filled(
            visual,
            CornerRadius::same((f32::from(tokens.radius.row) * scale).round() as u8),
            tokens.surface_active.gamma_multiply(press.fill),
        );
    } else if response.hovered() {
        ui.painter().rect_filled(
            visual,
            CornerRadius::same(tokens.radius.row),
            tokens.surface_hover,
        );
    }

    let numerals = chrome::scaled(&type_scale::numerals(), scale);
    let time = clock(hit.start_ms);
    let time_width = type_scale::tabular_width(&time, numerals.size());
    type_scale::paint_tabular(
        ui.painter(),
        pos2(
            visual.right() - GUTTER * scale,
            visual.top() + 12.0 * scale + numerals.line_height / 2.0,
        ),
        Align2::RIGHT_CENTER,
        &time,
        &numerals.font,
        tokens.ink_soft,
    );

    let title = chrome::scaled(&type_scale::row_title(), scale);
    let mut job = egui::text::LayoutJob::default();
    job.append(&hit.title, 0.0, title.format(tokens.ink));
    job.wrap.max_width =
        (visual.width() - GUTTER * 2.0 * scale - time_width - 10.0 * scale).max(40.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    ui.painter().galley(
        pos2(visual.left() + GUTTER * scale, visual.top() + 12.0 * scale),
        ui.painter().layout_job(job),
        tokens.ink,
    );

    let caption = chrome::scaled(&type_scale::caption(), scale);
    let (before, matched, after) = split_brackets(&hit.snippet);
    let mut job = egui::text::LayoutJob::default();
    job.append(before, 0.0, caption.format(tokens.ink_soft));
    if let Some(matched) = matched {
        job.append(matched, 0.0, caption.format(tokens.accent));
    }
    job.append(after, 0.0, caption.format(tokens.ink_soft));
    job.wrap.max_width = (visual.width() - GUTTER * 2.0 * scale).max(40.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    ui.painter().galley(
        pos2(visual.left() + GUTTER * scale, visual.top() + 40.0 * scale),
        ui.painter().layout_job(job),
        tokens.ink_soft,
    );
    ui.painter().line_segment(
        [
            pos2(rect.left() + GUTTER, rect.bottom() - 0.5),
            pos2(rect.right() - GUTTER, rect.bottom() - 0.5),
        ],
        Stroke::new(1.0_f32, tokens.hairline),
    );
    response.clicked()
}

/// Splits the store's `prefix[match]suffix` snippet into its three display
/// spans. A snippet without markers (no locatable term) is all `before`.
fn split_brackets(snippet: &str) -> (&str, Option<&str>, &str) {
    let Some(open) = snippet.find('[') else {
        return (snippet, None, "");
    };
    let Some(close) = snippet[open + 1..].find(']') else {
        return (snippet, None, "");
    };
    let close = open + 1 + close;
    (
        &snippet[..open],
        Some(&snippet[open + 1..close]),
        &snippet[close + 1..],
    )
}

/// How many pips a row shows and how many it folds into `+N` (M22C: up to
/// four, then the overflow count). A pure helper so the rule is testable
/// without a frame.
pub fn pip_layout(speakers: usize) -> (usize, usize) {
    (speakers.min(4), speakers.saturating_sub(4))
}

/// The list's bottom padding: enough to scroll the last row clear of the
/// floating dock, the dock's gap, the gesture inset and a breathing space. The
/// dock floats OVER the list, so this is the only thing keeping the last
/// transcript reachable.
pub fn list_bottom_padding(screen: Rect, bottom_inset: f32, with_reason: bool) -> f32 {
    let dock = chrome::dock_rect(screen, bottom_inset);
    let viewport_bottom = screen.bottom() - bottom_inset;
    (viewport_bottom - dock.top()).max(0.0) + bottom_inset + if with_reason { 24.0 } else { 16.0 }
}

/// The disabled-Record reason, in a small chip above the dock: frosted enough
/// to stay legible while rows pass beneath it, too small to read as a card.
fn paint_reason_chip(ui: &mut Ui, tokens: &Tokens, dock: Rect, reason: &str) {
    let caption = type_scale::caption();
    let mut job = egui::text::LayoutJob::default();
    job.append(reason, 0.0, caption.format(tokens.warn));
    job.wrap.max_width = (dock.width() - 20.0).max(40.0);
    job.wrap.max_rows = 1;
    job.wrap.overflow_character = Some('…');
    let galley = ui.painter().layout_job(job);
    let width = (galley.size().x + 20.0).min(dock.width());
    let chip = Rect::from_min_size(
        pos2(dock.left(), dock.top() - 8.0 - REASON_CHIP),
        vec2(width, REASON_CHIP),
    );
    material::frosted_panel(ui, tokens, chip, tokens.radius.control, Plane::Card);
    ui.painter().galley(
        pos2(chip.left() + 10.0, chip.center().y - galley.size().y / 2.0),
        galley,
        tokens.warn,
    );
    a11y::describe(ui, chip, reason);
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

fn paint_watch_row(ui: &mut Ui, tokens: &Tokens, rect: Rect, watch: &super::model::WatchView) {
    let failed = watch.failure.is_some();
    let fill = if failed {
        tokens.record_red.gamma_multiply(0.12)
    } else {
        tokens.panel_faint
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(tokens.radius.control), fill);
    ui.painter().text(
        pos2(rect.left() + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        watch.line(),
        type_scale::row_value().font,
        if failed {
            tokens.record_red_text
        } else {
            tokens.ink_soft
        },
    );
    // The folder name is the row's second half, right-aligned so the import
    // count never pushes it off the row.
    ui.painter().text(
        pos2(rect.right() - 12.0, rect.center().y),
        Align2::RIGHT_CENTER,
        &watch.folder,
        type_scale::caption().font,
        tokens.ink_soft,
    );
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
        type_scale::row_value().font,
        if error {
            tokens.record_red_text
        } else {
            tokens.ink_soft
        },
    );
}

/// The 28 pt waveform mark: seven round-capped bars, tall in the middle, in
/// `ink_faint`. Drawn from the same 1.75 px pen language as the icon kit.
fn waveform(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    const BARS: [f32; 7] = [0.34, 0.62, 1.0, 0.52, 0.84, 0.44, 0.66];
    let step = 28.0 / BARS.len() as f32;
    for (index, amount) in BARS.iter().enumerate() {
        let x = center.x - 14.0 + step * (index as f32 + 0.5);
        let height = 28.0 * amount;
        painter.rect_filled(
            Rect::from_center_size(pos2(x, center.y), vec2(2.0, height)),
            CornerRadius::same(1),
            color,
        );
    }
}

fn empty_library(ui: &mut Ui, tokens: &Tokens, rect: Rect, dock_top: f32) {
    let display = type_scale::editorial_display();
    let caption = type_scale::caption();
    let width = (rect.width() - 48.0).max(120.0);
    let title = ui.painter().layout(
        "No transcripts yet".to_owned(),
        display.font.clone(),
        tokens.ink,
        width,
    );
    // The owner-approved copy, kept verbatim: two working entries, one line
    // each. (The share target is the only place a first-time user learns it
    // exists; Record is in the dock below.)
    let lines = [
        "Share a voice note into Suramak, or tap Import to choose a file.",
        "Record works here too — it keeps going with the screen off.",
    ];
    let galleys: Vec<_> = lines
        .iter()
        .map(|line| {
            ui.painter().layout(
                (*line).to_owned(),
                caption.font.clone(),
                tokens.ink_soft,
                width,
            )
        })
        .collect();
    let prompt_h: f32 = galleys.iter().map(|galley| galley.size().y + 2.0).sum();
    let block = 28.0 + 12.0 + title.size().y + 10.0 + prompt_h;
    let band_bottom = (dock_top - 8.0).max(rect.top() + 80.0);
    let top = rect.top()
        + ((band_bottom - rect.top() - block) / 2.0)
            .max(0.0)
            .min((band_bottom - block - rect.top()).max(0.0));

    waveform(
        ui.painter(),
        pos2(rect.center().x, top + 14.0),
        tokens.ink_faint,
    );
    ui.painter().galley(
        pos2(rect.center().x - title.size().x / 2.0, top + 40.0),
        title,
        tokens.ink,
    );
    let mut y = top + 40.0 + display.line_height + 10.0;
    for galley in &galleys {
        ui.painter().galley(
            pos2(rect.center().x - galley.size().x / 2.0, y),
            galley.clone(),
            tokens.ink_soft,
        );
        y += galley.size().y + 2.0;
    }
    a11y::describe(
        ui,
        Rect::from_min_size(
            pos2(rect.left() + 12.0, top),
            vec2(rect.width() - 24.0, block),
        ),
        &format!("No transcripts yet. {}", lines.join(" ")),
    );
}

fn empty_search(ui: &mut Ui, tokens: &Tokens, rect: Rect, query: &str) {
    let center = pos2(rect.center().x, rect.top() + rect.height() * 0.28);
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        format!("No matches for “{query}”"),
        type_scale::body().font,
        tokens.ink_soft,
    );
}

/// The list's scroll band, exposed for the layout assertions.
pub fn list_bounds(screen: Rect) -> Rect {
    chrome::bands(screen, 0.0, 0.0, None).content
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

/// The dock's two button rects, for hit-target tests.
pub fn action_rects(screen: Rect) -> [Rect; 2] {
    chrome::dock_action_rects(chrome::dock_rect(screen, 12.0))
}

/// The search field's rect, for layout assertions.
pub fn search_rect(screen: Rect) -> Rect {
    let bands = chrome::bands(screen, 0.0, 0.0, None);
    Rect::from_min_size(
        pos2(
            bands.content.left() + GUTTER,
            bands.content.top() + SECTION_GAP,
        ),
        vec2(bands.content.width() - GUTTER * 2.0, MIN_TOUCH),
    )
}

/// A frame-level check that the dock's actions never overlap, both clear the
/// system gesture inset, and neither escapes the viewport.
pub fn bar_is_safe(screen: Rect) -> bool {
    let dock = chrome::dock_rect(screen, 12.0);
    let [a, b] = chrome::dock_action_rects(dock);
    !a.intersects(b)
        && a.height() >= MIN_TOUCH
        && b.height() >= MIN_TOUCH
        && dock.bottom() <= screen.bottom() - 12.0
        && screen.contains_rect(dock)
}

/// The Library's type roles, exposed so the suite can assert the scale is
/// actually applied (the tokens are the plan's, not per-screen numbers).
pub fn search_hint() -> TypeToken {
    chrome::search_hint_token()
}

/// The row's title token (the painted rows use this, possibly press-scaled).
pub fn row_title() -> TypeToken {
    type_scale::row_title()
}

/// The row's meta token.
pub fn row_meta() -> TypeToken {
    type_scale::caption()
}

/// The app bar's serif title token.
pub fn title() -> TypeToken {
    chrome::title_token()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::theme::{self, AppTheme};

    fn tokens(theme: AppTheme) -> Tokens {
        crate::deck::compact::tokens_for(theme)
    }

    /// The dock's clearance math, on both ladders' geometry: the padding the
    /// list adds must cover the dock, its gap and the gesture inset, so the
    /// last row is never hidden behind the floating plane.
    #[test]
    fn the_list_clears_the_dock_and_the_gesture_inset() {
        let screen = crate::deck::compact::phone_screen_rect();
        for inset in [0.0, 20.0, 54.0] {
            let dock = chrome::dock_rect(screen, inset);
            let padding = list_bottom_padding(screen, inset, false);
            assert!(
                padding >= dock.height() + inset,
                "inset {inset}: padding {padding} < dock {} + inset",
                dock.height()
            );
            assert!(padding >= chrome::DOCK_HEIGHT + chrome::DOCK_GAP);
            // The dock itself sits 12 pt above the gesture area and is
            // inside the screen.
            assert!((screen.bottom() - inset - dock.bottom() - chrome::DOCK_GAP).abs() < 0.01);
            assert!(screen.contains_rect(dock));
        }
    }

    /// The dock's two pills are 48 pt targets that never overlap.
    #[test]
    fn the_dock_pills_are_touch_targets() {
        let screen = crate::deck::compact::phone_screen_rect();
        let [import, record] = action_rects(screen);
        assert!(import.height() >= MIN_TOUCH && record.height() >= MIN_TOUCH);
        assert!(!import.intersects(record));
        assert!(record.width() > import.width(), "Record is the primary");
        assert!((import.width() / (import.width() + record.width()) - 0.38).abs() < 0.02);
    }

    /// The Record fill clears the small-text contrast floor with the ladder's
    /// own ink on both ladders (the light ladder's plain red is 4.46:1).
    #[test]
    fn the_record_fill_is_legible_on_both_ladders() {
        for theme in [AppTheme::Dark, AppTheme::Light] {
            let tokens = tokens(theme);
            let fill = chrome::record_fill(&tokens);
            let ratio = theme::contrast_ratio(tokens.on_accent, fill);
            assert!(
                ratio >= 4.5,
                "{theme:?}: on_accent on the Record fill = {ratio:.2}:1"
            );
            assert!(
                fill == tokens.record_red || fill == tokens.record_red_text,
                "{theme:?}: the fill must be one of the two record reds"
            );
        }
    }
}
