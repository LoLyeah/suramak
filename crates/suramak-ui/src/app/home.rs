//! Home route: the drop zone (idle state), the import affordance and the recent
//! recordings list bound to the store.
//!
//! M9a: composition unchanged (protect list) — tokens, the unified control
//! system and the motion system only. The drop zone wears the one card recipe;
//! the buttons are painted controls; rows crossfade their hover wash.
//!
//! Import paths: winit `WindowEvent::DroppedFile` (all platforms, handled in
//! `main.rs`) plus — on macOS/Windows — an rfd open dialog; on Linux (rfd is
//! deliberately not a dependency there) the path field + Enter.

use std::path::PathBuf;

use egui::{pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Rect};

use crate::app::controls::{self, Tier};
use crate::motion::{self, Motion};
use crate::theme::{self, Tokens};

use super::views::{paint_card, pane_caption, pane_title, PANE_PAD_X};
use super::{AppState, Effect};

/// macOS/Windows: native open dialog (rfd). Linux: `None` — the path field is
/// the affordance there (avoids the zbus/portal dependency chain).
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn pick_media_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Import audio or video")
        .add_filter(
            "Audio or video",
            &[
                "wav", "mp3", "m4a", "flac", "ogg", "aiff", "mp4", "mov", "m4v", "mkv",
            ],
        )
        .pick_file()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn pick_media_file() -> Option<PathBuf> {
    None
}

/// Whether this build offers the native "Choose file…" dialog.
const HAS_NATIVE_DIALOG: bool = cfg!(any(target_os = "macos", target_os = "windows"));

pub fn ui(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    let rect = ui.max_rect();
    let left = rect.left() + PANE_PAD_X;
    let right = rect.right() - PANE_PAD_X;

    ui.horizontal(|ui| {
        ui.add_space(PANE_PAD_X - 8.0);
        ui.vertical(|ui| {
            pane_title(ui, "New transcription", tokens);
            pane_caption(
                ui,
                "Drop an audio or video file anywhere in this window, or import one below.",
                tokens,
                false,
            );
            if !state.capture_available {
                pane_caption(
                    ui,
                    "Recording is not compiled into this build (feature `capture`) — import or drop audio or video files instead.",
                    tokens,
                    false,
                );
            }
        });
    });

    // ---- drop zone card -----------------------------------------------------
    let zone_top = rect.top() + 84.0;
    let zone = Rect::from_min_size(pos2(left, zone_top), vec2(right - left, 168.0));
    let painter = ui.painter();
    paint_card(painter, zone, tokens);
    // The hover ring crossfades with the fast token and scales in from a
    // slightly inset rect (no instant stroke swap — critique P1-3).
    let ring = motion.factor(
        ui.ctx(),
        egui::Id::new("suramak-drop-ring"),
        state.home.drop_hover,
        motion::FAST,
    );
    if ring > 0.01 {
        let inset = motion::scale(ring, 4.0, 0.0);
        painter.rect_stroke(
            zone.shrink(inset),
            CornerRadius::same(tokens.radius.card),
            egui::Stroke::new(tokens.stroke.strong, tokens.accent.gamma_multiply(ring)),
            egui::StrokeKind::Inside,
        );
    }
    painter.text(
        pos2(zone.center().x, zone.center().y - 12.0),
        Align2::CENTER_CENTER,
        if state.home.drop_hover {
            "Release to transcribe"
        } else {
            "Drop a file here"
        },
        FontId::new(24.0, theme::serif_family()),
        tokens.ink,
    );
    painter.text(
        pos2(zone.center().x, zone.center().y + 20.0),
        Align2::CENTER_CENTER,
        "wav · mp3 · m4a · flac · ogg · mp4 · mov · m4v · mkv — transcribed on this machine, never uploaded",
        FontId::new(12.0, FontFamily::Proportional),
        tokens.ink_soft,
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
                if HAS_NATIVE_DIALOG {
                    let choose = controls::button(
                        ui,
                        tokens,
                        motion,
                        "home-choose",
                        "Choose file…",
                        Tier::Secondary,
                        true,
                    );
                    if choose.clicked() {
                        if let Some(path) = pick_media_file() {
                            effects.extend(state.begin_import(path));
                        }
                    }
                    ui.add_space(tokens.space.xs);
                }
                let reserved = if HAS_NATIVE_DIALOG { 100.0 } else { 112.0 };
                let field_width = (ui.available_width() - reserved).max(120.0);
                let response = ui.add_sized(
                    [field_width, 24.0],
                    egui::TextEdit::singleline(&mut state.home.path_field)
                        .hint_text("path to an audio or video file — press Enter to import")
                        .text_color(tokens.ink),
                );
                let submitted =
                    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(tokens.space.xs);
                let import = controls::button(
                    ui,
                    tokens,
                    motion,
                    "home-import",
                    "Import",
                    Tier::Secondary,
                    true,
                );
                if submitted || import.clicked() {
                    effects.extend(state.import_from_field());
                }
            });
        },
    );

    // ---- navigation (M3: models + settings live here) -----------------------
    let nav_top = row_top + 34.0;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(
            pos2(left, nav_top),
            vec2(right - left, 26.0),
        )),
        |ui| {
            ui.horizontal(|ui| {
                let models = controls::button(
                    ui,
                    tokens,
                    motion,
                    "home-models",
                    "Models",
                    Tier::Secondary,
                    true,
                );
                if models.clicked() {
                    effects.extend(state.open_models());
                }
                let settings = controls::button(
                    ui,
                    tokens,
                    motion,
                    "home-settings",
                    "Settings",
                    Tier::Secondary,
                    true,
                );
                if settings.clicked() {
                    effects.extend(state.open_settings());
                }
            });
        },
    );

    // One message slot (M9c): the import error takes priority over the
    // cross-view status; both fade in base / out fast. The old flow-positioned
    // error collided with the nav row on every error — one fixed slot.
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(
            pos2(left, nav_top + 30.0),
            vec2(right - left, 20.0),
        )),
        |ui| {
            ui.horizontal(|ui| {
                let message = match (&state.home.error, &state.status) {
                    (Some(err), _) => Some((err.as_str(), true)),
                    (None, Some(status)) => Some((status.text.as_str(), status.error)),
                    (None, None) => None,
                };
                super::views::status_caption(ui, tokens, motion, "home-status", message);
            });
        },
    );

    // ---- recent recordings --------------------------------------------------
    let list_top = row_top + 82.0;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(
            pos2(left, list_top),
            vec2(right - left, rect.bottom() - list_top - 8.0),
        )),
        |ui| {
            ui.vertical(|ui| {
                super::views::section_title(ui, "Recent recordings", tokens);
                ui.add_space(6.0);
                let error = state.library.error.clone();
                let empty = state.library.items.is_empty() && state.library.leaving.is_empty();
                if let Some(error) = &error {
                    // An error state: fades in on arrival (base; a snap under
                    // Reduce Motion).
                    super::views::arriving_caption(
                        ui,
                        tokens,
                        motion,
                        "home-library-error",
                        error,
                        true,
                    );
                } else if empty {
                    super::views::arriving_caption(
                        ui,
                        tokens,
                        motion,
                        "home-empty",
                        "Nothing yet — your transcriptions will appear here.",
                        false,
                    );
                } else {
                    // M9c list motion: insert ease (arrival) + remove ease
                    // (leaving rows collapse in place), same as the sidebar.
                    let now = std::time::Instant::now();
                    state
                        .library
                        .prune_motion(now, motion::token_duration(motion::BASE));
                    let slots = super::library::list_slots(&state.library, Some(6));
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for slot in slots {
                                match slot {
                                    super::library::ListSlot::Live(index) => {
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
                                        let arrival = state
                                            .library
                                            .row_arrival(id, now, motion::BASE)
                                            .unwrap_or(1.0);
                                        if recording_row(
                                            ui, tokens, motion, index, &title, &meta, arrival,
                                        ) {
                                            effects.extend(state.select_library(index));
                                        }
                                    }
                                    super::library::ListSlot::Leaving(leaving_index) => {
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
                                        super::library::leaving_row(
                                            ui, tokens, &title, &meta, progress,
                                        );
                                    }
                                }
                            }
                        });
                }
            });
        },
    );
}

/// One clickable recording row (title + meta): `arrival` fades + rises the row
/// on an insert (1.0 = settled). Returns true when clicked.
pub fn recording_row(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    index: usize,
    title: &str,
    meta: &str,
    arrival: f32,
) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 40.0), egui::Sense::click());
    // M9b a11y: the painted row is a labeled button; it carries the one focus
    // ring when tabbed to.
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Open {title}"))
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
        egui::Id::new(("suramak-home-row-hover", index)),
        response.hovered(),
        motion::FAST,
    );
    let painter = ui.painter();
    if hover > 0.01 {
        painter.rect_filled(
            rect,
            CornerRadius::same(tokens.radius.row),
            motion::tint_over(egui::Color32::TRANSPARENT, tokens.surface_hover, hover),
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
        pos2(rect.left() + 10.0, rect.top() + 22.0 + rise),
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
