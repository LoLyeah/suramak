//! Compact Settings: language, models, appearance, device facts and Quit.
//!
//! **M20D — de-jargoned and grouped.** The phone's Settings is the same product
//! as the redrawn Library: one frosted card per section (the Card plane — veil
//! plus 1 px rim, no shadow), an 8 pt rhythm (16 screen margins, 8 under a
//! section label, 24 between sections), hairlines only *inside* a grouped card,
//! and the type scale's 11 pt CAPS section labels.
//!
//! The model table is ONE card: each row leads with a human tier name, its
//! size and a truthful one-line purpose. The raw manifest ids (`tiny`, `base`,
//! `small`, `turbo-q5-0`) are support vocabulary and live in Diagnostics only
//! (M20D2) — the phone's primary card never asks a person to read them. A
//! recommendation is a real capsule badge, and the selected row is a full-bleed
//! tint band with a 3 px accent bar, not a pill nested inside the card. The
//! five-state actions of M19B are unchanged — `In use` still requires a
//! verified file — with one wording change: the recommended row's missing file
//! offers `Download`.
//!
//! The primary surface carries no build vocabulary: no `CI + tests`, no
//! `optional/legacy`, no `default (macOS)`, no diarization model strings, no
//! `CPL`, no `SURAMAK_` environment names. Those live in Diagnostics (or
//! nowhere on the phone, where they never applied). Android also hides the
//! Quit row (`#[cfg(not(target_os = "android"))]`): a phone exits through the
//! system, and the list's last control is then the Diagnostics row.

use egui::emath::GuiRounding;
use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, Layout, Rect, ScrollArea, Stroke, StrokeKind,
    Ui, WidgetType,
};

use std::time::{Duration, Instant};

use suramak_core::device::PhoneTier;
use suramak_core::settings::Appearance;

use crate::deck::icons::{self, Icon};
use crate::deck::material::{self, Plane};
use crate::deck::motion::{self, Motion};
use crate::deck::theme::Tokens;
use crate::deck::type_scale::{self, TypeToken};

use super::a11y;
use super::chrome::{self, GUTTER, MIN_TOUCH};
use super::model::{
    CompactAction, CompactApp, CompactSetting, DownloadView, Insets, ModelRow, ModelState,
};

/// The DEVICE card's last line — the smoke's second probe field, kept because
/// it names the phone's disk truth (M19A2).
pub const DEVICE_NOTE: &str = "Everything runs on this phone. No analytics, no uploads.";

/// The Quit row's accessibility label — the desktop list's real last control,
/// and one of the two the bottom-clearance probe can measure (M19A2).
pub const QUIT_LABEL: &str = "Quit Suramak";

/// The Diagnostics row's accessibility label. On Android, where the Quit row
/// does not exist, **this** is the list's last control and therefore the
/// clearance witness (M20D).
pub const DIAGNOSTICS_LABEL: &str = "Diagnostics";

/// The 8 pt grid (plan §D): a section is label (16) → 8 → card, cards are
/// separated by 24, and a card pads its content by 16 (the same 16 as the
/// screen gutter, so labels and row text share one vertical line).
const SECTION_GAP: f32 = 24.0;
/// The gap between a section label and its card.
const LABEL_GAP: f32 = 8.0;
/// The section label's line height (the type scale's token).
const LABEL_H: f32 = 16.0;
/// A card's horizontal (and divider) inset.
const CARD_PAD: f32 = 16.0;
/// One setting row: 48 pt body minimum, two text lines.
const ROW_H: f32 = 64.0;
/// One model row: tier name (plus the optional badge) and one guidance line.
const MODEL_ROW_H: f32 = 80.0;
/// The device card's facts block (version, memory, the disk-truth note).
const DEVICE_FACTS_H: f32 = 80.0;
/// The model action pill's painted height (its hit area is 48).
const ACTION_H: f32 = 36.0;
/// The model action column's width.
const ACTION_W: f32 = 92.0;
/// The space the content always leaves under its last control.
///
/// `floor` is the platform's minimum bottom inset: 0 on desktop (the value is
/// the platform's own inset plus the standard gutter) and
/// [`ANDROID_MIN_CLEARANCE`] on the phone, so a window that has not pushed its
/// insets yet — or an immersive one — still scrolls the last row clear of the
/// gesture pill instead of pinning it to the screen's edge.
pub fn bottom_clearance(inset_bottom: f32, floor: f32) -> f32 {
    inset_bottom.max(floor).max(0.0) + GUTTER
}

/// Android's minimum bottom clearance, in points: 24 dp, the gesture handle's
/// own height on every Android release that ships gesture navigation.
///
/// A point is a dp on Android (`pixels_per_point` is `density / 160`), which
/// is why this number is the platform's, not a design choice.
pub const ANDROID_MIN_CLEARANCE: f32 = 24.0;

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
    let (back, _) = chrome::app_bar(ui, tokens, motion, bands.app_bar, "Settings", true, None);
    if back {
        state.back(now);
    }

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(bands.content)
            .layout(Layout::top_down(Align::Min)),
    );
    let mut clicks: Vec<CompactAction> = Vec::new();
    let mut scroll = ScrollArea::vertical()
        .id_salt("compact-settings-scroll")
        .auto_shrink([false, false])
        .scroll_source(egui::containers::scroll_area::ScrollSource {
            drag: true,
            ..Default::default()
        });
    if let Some(offset) = state.take_settings_scroll() {
        scroll = scroll.vertical_scroll_offset(offset);
    }
    scroll.show(&mut child, |ui| {
        let width = bands.content.width() - GUTTER * 2.0;
        let x = bands.content.left() + GUTTER;
        // M19A: the rows flow from the SCROLLED cursor, not from the outer
        // band's static top. `bands.content.top()` is where the viewport
        // begins; inside `ScrollArea::show` the content Ui's cursor starts
        // `offset` above it, so reading the cursor is what makes the rows move
        // when the finger does (the pattern `library.rs` already uses).
        //
        // The scroll area snaps its content origin to the physical pixel grid
        // (`round_to_pixels` then `round_ui`), which would leave the first row
        // a fraction of a pixel above the band it has always been drawn at and
        // soften every glyph in the shot. Add that remainder back: at offset 0
        // the frame is byte-identical to the pre-M19A render, and while
        // scrolling the remainder is a constant that never changes a row's
        // spacing.
        let snapped = bands
            .content
            .top()
            .round_to_pixels(ui.pixels_per_point())
            .round_ui();
        let top = ui.cursor().top() + (bands.content.top() - snapped);
        let mut y = top + 8.0;

        // ---- Transcription -------------------------------------------
        y = section(ui, tokens, x, y, "Transcription");
        let card = Rect::from_min_size(pos2(x, y), vec2(width, ROW_H * 3.0));
        material::frosted_panel(ui, tokens, card, tokens.radius.card, Plane::Card);

        let language = Rect::from_min_size(pos2(x, card.top()), vec2(width, ROW_H));
        if card_row(
            ui,
            tokens,
            motion,
            language,
            "settings-language",
            "Language",
            language_label(&state.settings.language),
            None,
            true,
            Affordance::Chevron,
            true,
        )
        .clicked()
        {
            let next = next_language(&state.settings.language);
            state.apply_setting(CompactSetting::Language(next.clone()));
            clicks.push(CompactAction::Setting(CompactSetting::Language(next)));
        }

        // The M19B header: the configured model, named in the human tier and
        // qualified only when its file is missing. A read-out, not a control —
        // the picker is the model card below.
        let (_, model_missing) = state.model_label();
        let model_name = state
            .models
            .iter()
            .find(|row| row.id == state.settings.model_id)
            .map(|row| row.tier_name().to_string())
            .unwrap_or_else(|| state.settings.model_id.clone());
        let model_value = if model_missing {
            format!("{model_name} · not downloaded")
        } else {
            model_name
        };
        readout_row(
            ui,
            tokens,
            Rect::from_min_size(pos2(x, card.top() + ROW_H), vec2(width, ROW_H)),
            "Model",
            &model_value,
            model_missing.then_some(tokens.warn),
        );

        let keep = state.settings.keep_audio;
        let toggle_row = Rect::from_min_size(pos2(x, card.top() + ROW_H * 2.0), vec2(width, ROW_H));
        if card_row(
            ui,
            tokens,
            motion,
            toggle_row,
            "settings-keep-audio",
            "Keep the recording",
            "Keep the source audio after transcription",
            None,
            true,
            Affordance::Toggle(keep),
            false,
        )
        .clicked()
        {
            let next = !keep;
            state.apply_setting(CompactSetting::KeepAudio(next));
            clicks.push(CompactAction::Setting(CompactSetting::KeepAudio(next)));
        }
        y = card.bottom() + SECTION_GAP;

        // ---- Watched inbox --------------------------------------------
        // M24A: two rows — the switch and the folder. Turning the switch on
        // with no folder opens the tree picker instead of persisting a
        // half-configured setting.
        y = section(ui, tokens, x, y, "Watched inbox");
        let watch_card = Rect::from_min_size(pos2(x, y), vec2(width, ROW_H * 2.0));
        material::frosted_panel(ui, tokens, watch_card, tokens.radius.card, Plane::Card);

        let watching = state.watch_enabled;
        let watch_toggle = Rect::from_min_size(pos2(x, watch_card.top()), vec2(width, ROW_H));
        if card_row(
            ui,
            tokens,
            motion,
            watch_toggle,
            "settings-watch-enabled",
            "Watch a folder",
            "Import audio files that land there",
            None,
            true,
            Affordance::Toggle(watching),
            true,
        )
        .clicked()
        {
            if watching {
                state.apply_setting(CompactSetting::WatchEnabled(false));
                clicks.push(CompactAction::Setting(CompactSetting::WatchEnabled(false)));
            } else if state.watch_folder.is_empty() {
                clicks.push(CompactAction::PickWatchFolder);
            } else {
                state.apply_setting(CompactSetting::WatchEnabled(true));
                clicks.push(CompactAction::Setting(CompactSetting::WatchEnabled(true)));
            }
        }

        let folder_value = if state.watch_folder.is_empty() {
            "Choose a folder…".to_string()
        } else {
            state.watch_folder.clone()
        };
        if card_row(
            ui,
            tokens,
            motion,
            Rect::from_min_size(pos2(x, watch_card.top() + ROW_H), vec2(width, ROW_H)),
            "settings-watch-folder",
            "Folder",
            &folder_value,
            None,
            true,
            Affordance::Chevron,
            false,
        )
        .clicked()
        {
            clicks.push(CompactAction::PickWatchFolder);
        }
        y = watch_card.bottom() + SECTION_GAP;

        // ---- Models ---------------------------------------------------
        y = section(ui, tokens, x, y, "Models");
        let sentence = model_sentence(
            state.tier,
            state.ram_bytes,
            state.recommended_model(),
            state.configured_model(),
        );
        let sentence_galley = paint_elided(
            ui.painter(),
            pos2(x, y),
            &sentence,
            &type_scale::caption(),
            tokens.ink_soft,
            width,
        );
        a11y::describe(
            ui,
            Rect::from_min_size(pos2(x, y), vec2(width, sentence_galley.size().y.max(16.0))),
            &sentence,
        );
        y += type_scale::caption().line_height + LABEL_GAP;

        let offered: Vec<ModelRow> = state
            .models
            .iter()
            .filter(|row| row.offered || row.installed)
            .cloned()
            .collect();
        let hidden = state.models.len().saturating_sub(offered.len());
        let row_count = offered.len().max(1) as f32;
        let models_card = Rect::from_min_size(pos2(x, y), vec2(width, MODEL_ROW_H * row_count));
        material::frosted_panel(ui, tokens, models_card, tokens.radius.card, Plane::Card);

        if offered.is_empty() {
            let rect = Rect::from_min_size(models_card.min, vec2(width, MODEL_ROW_H));
            let line = "No model fits this phone.";
            ui.painter().text(
                pos2(rect.left() + CARD_PAD, rect.center().y),
                Align2::LEFT_CENTER,
                line,
                type_scale::row_title().font,
                tokens.ink_soft,
            );
            a11y::describe(ui, rect, line);
        }

        for (index, row) in offered.iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(x, models_card.top() + index as f32 * MODEL_ROW_H),
                vec2(width, MODEL_ROW_H),
            );
            let selected = state.settings.model_id == row.id;
            // M19B: the live job for THIS row (the card and the table read the
            // same `DownloadView` the job reports).
            let live = state.download.clone().filter(|download| {
                download.model == row.id
                    && matches!(
                        download.state,
                        super::model::DownloadState::Running | super::model::DownloadState::Paused
                    )
            });
            let row_state = row.state(selected, live.as_ref());
            let edges = RowEdges {
                first: index == 0,
                last: index + 1 == offered.len(),
            };
            let (row_clicked, action_clicked) = model_row(
                ui,
                tokens,
                motion,
                rect,
                row,
                selected,
                live.as_ref(),
                row_state,
                edges,
            );
            if action_clicked && row.offered && row_state.is_actionable() {
                match row_state {
                    ModelState::Installed => {
                        state.apply_setting(CompactSetting::Model(row.id.clone()));
                        clicks.push(CompactAction::Setting(CompactSetting::Model(
                            row.id.clone(),
                        )));
                    }
                    ModelState::Downloading if live.is_some() => {
                        clicks.push(CompactAction::CancelDownload);
                    }
                    // A stale/corrupt file is unlinked by the host before the
                    // transfer starts; a bare `.part` resumes where it stopped.
                    ModelState::NotDownloaded
                    | ModelState::StaleCorrupt
                    | ModelState::Downloading => {
                        clicks.push(CompactAction::DownloadModel(row.id.clone()));
                    }
                    ModelState::InUse => {}
                }
            } else if row_clicked && row.offered && row_state == ModelState::Installed {
                state.apply_setting(CompactSetting::Model(row.id.clone()));
                clicks.push(CompactAction::Setting(CompactSetting::Model(
                    row.id.clone(),
                )));
            }
        }
        y = models_card.bottom();
        if hidden > 0 && !offered.is_empty() {
            let note = format!(
                "{} larger model{} hidden — this phone can't run them",
                hidden,
                if hidden == 1 { "" } else { "s" }
            );
            let galley = paint_elided(
                ui.painter(),
                pos2(x, y + 6.0),
                &note,
                &type_scale::caption(),
                tokens.ink_soft,
                width,
            );
            a11y::describe(
                ui,
                Rect::from_min_size(pos2(x, y + 6.0), vec2(width, galley.size().y.max(16.0))),
                &note,
            );
            y += galley.size().y + 6.0;
        }
        y += SECTION_GAP;

        // ---- Appearance ----------------------------------------------
        y = section(ui, tokens, x, y, "Appearance");
        let chips = Rect::from_min_size(pos2(x, y), vec2(width, MIN_TOUCH));
        ui.painter().rect(
            chips,
            CornerRadius::same(tokens.radius.control),
            tokens.control_fill,
            Stroke::new(1.0_f32, tokens.hairline),
            StrokeKind::Inside,
        );
        let chip_w = width / 3.0;
        for (index, appearance) in Appearance::ALL.iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(
                    chips.left() + index as f32 * chip_w + 4.0,
                    chips.top() + 4.0,
                ),
                vec2(chip_w - 8.0, chips.height() - 8.0),
            );
            let selected = state.settings.appearance == *appearance;
            let response = a11y::touch(
                ui,
                tokens,
                rect,
                ("appearance", appearance.label()),
                WidgetType::RadioButton,
                &format!("Appearance: {}", appearance.label()),
                true,
            );
            // The plan's accent-tint rule: a 10 % tint fill + a 1 px accent
            // stroke at 45 % — never a bright outline. The tint crossfades on
            // the FAST token so the swap reads as a state change, not a blink.
            let t = motion.factor(
                ui.ctx(),
                ui.id().with(("appearance-fill", appearance.label())),
                selected,
                motion::FAST,
            );
            if t > 0.0 {
                ui.painter().rect(
                    rect,
                    CornerRadius::same(tokens.radius.control),
                    tokens.accent_tint.gamma_multiply(t),
                    Stroke::new(1.0_f32, tokens.accent.gamma_multiply(0.45 * t)),
                    StrokeKind::Inside,
                );
            }
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                appearance.label(),
                type_scale::row_title().font,
                motion::lerp_color(tokens.ink_soft, tokens.ink, t),
            );
            if response.clicked() && !selected {
                state.settings.appearance = *appearance;
                clicks.push(CompactAction::Setting(CompactSetting::Appearance(
                    *appearance,
                )));
            }
        }
        y = chips.bottom() + SECTION_GAP;

        // ---- Device (facts + Diagnostics entry) ----------------------
        y = section(ui, tokens, x, y, "Device");
        let device_card = Rect::from_min_size(pos2(x, y), vec2(width, DEVICE_FACTS_H + ROW_H));
        material::frosted_panel(ui, tokens, device_card, tokens.radius.card, Plane::Card);
        let ram = state
            .ram_bytes
            .map(|bytes| format!("{:.0} GB usable", bytes as f64 / 1_000_000_000.0))
            .unwrap_or_else(|| "memory not reported".to_string());
        ui.painter().text(
            pos2(device_card.left() + CARD_PAD, device_card.top() + 18.0),
            Align2::LEFT_CENTER,
            format!("Suramak {}", state.version),
            type_scale::row_title().font,
            tokens.ink,
        );
        ui.painter().text(
            pos2(device_card.left() + CARD_PAD, device_card.top() + 40.0),
            Align2::LEFT_CENTER,
            ram,
            type_scale::caption().font,
            tokens.ink_soft,
        );
        ui.painter().text(
            pos2(device_card.left() + CARD_PAD, device_card.top() + 60.0),
            Align2::LEFT_CENTER,
            DEVICE_NOTE,
            type_scale::caption().font,
            tokens.ink_soft,
        );
        // The DEVICE note stays a named label (TalkBack reads the sentence
        // instead of anonymous paint) and a second probe field in the shell's
        // layout line. It is no longer the clearance witness: the Diagnostics
        // row (and, on desktop, Quit) follows it, so at full scroll the last
        // control is what sits under the gesture pill (M19A2).
        a11y::describe(
            ui,
            Rect::from_min_size(
                pos2(device_card.left() + CARD_PAD, device_card.top() + 52.0),
                vec2(width - CARD_PAD * 2.0, 16.0),
            ),
            DEVICE_NOTE,
        );
        ui.painter().line_segment(
            [
                pos2(x + CARD_PAD, device_card.top() + DEVICE_FACTS_H - 0.5),
                pos2(
                    x + width - CARD_PAD,
                    device_card.top() + DEVICE_FACTS_H - 0.5,
                ),
            ],
            Stroke::new(1.0_f32, tokens.hairline),
        );
        let checks = format!(
            "{} checks{}",
            super::diagnostics::CHECK_COUNT,
            if state.diagnostics.crash.is_some() {
                " · last crash"
            } else {
                ""
            }
        );
        let diagnostics_row = Rect::from_min_size(
            pos2(x, device_card.top() + DEVICE_FACTS_H),
            vec2(width, ROW_H),
        );
        if card_row(
            ui,
            tokens,
            motion,
            diagnostics_row,
            "settings-diagnostics",
            DIAGNOSTICS_LABEL,
            &checks,
            state.diagnostics.crash.is_some().then_some(tokens.warn),
            true,
            Affordance::Chevron,
            false,
        )
        .clicked()
        {
            state.open_diagnostics(now);
        }
        y = device_card.bottom() + SECTION_GAP;

        // ---- Quit (desktop only) -------------------------------------
        // A phone has no "quit the app" affordance beyond the system's own
        // gestures, so the row does not exist in the Android build. Its
        // absence is the reason the layout probe measures the LIST'S last
        // control rather than a fixed label (see [`LayoutProbe`]).
        #[cfg(not(target_os = "android"))]
        {
            let quit = Rect::from_min_size(pos2(x, y), vec2(width, 52.0));
            let response = a11y::touch(
                ui,
                tokens,
                quit,
                "settings-quit",
                WidgetType::Button,
                QUIT_LABEL,
                true,
            );
            ui.painter().rect(
                quit,
                CornerRadius::same(tokens.radius.control),
                Color32::TRANSPARENT,
                Stroke::new(1.0_f32, tokens.record_red.gamma_multiply(0.55)),
                StrokeKind::Inside,
            );
            ui.painter().text(
                quit.center(),
                Align2::CENTER_CENTER,
                QUIT_LABEL,
                type_scale::row_title().font,
                tokens.record_red_text,
            );
            if response.clicked() {
                state.request_quit();
            }
            y = quit.bottom();
        }
        y += 30.0;
        // M19A bottom clearance, M19A2 floor. The viewport ends at the content
        // band's bottom, which already excludes the gesture inset the shell
        // reports — but a phone that has not pushed its insets yet (or an
        // immersive window) reports zero, and a maxed-out scroll then pins the
        // last control to the pill. Padding the CONTENT by the inset (or the
        // platform's floor) plus the standard gutter is what the last row
        // scrolls above; without it the scroll range ends exactly at the row's
        // own edge.
        let tail = bottom_clearance(state.insets.bottom, state.clearance_floor);
        ui.allocate_space(vec2(width, (y - top).max(0.0) + tail));
    });

    actions.extend(clicks);
    actions
}

/// Paints a section label and returns the top of the card that follows it:
/// label (16) → 8 → card, the M20D rhythm.
fn section(ui: &mut Ui, tokens: &Tokens, x: f32, y: f32, label: &str) -> f32 {
    chrome::section_label(ui, tokens, pos2(x, y), label);
    y + LABEL_H + LABEL_GAP
}

/// A row's trailing affordance (the read-out row has none and is painted by
/// [`readout_row`], which never registers a hit area).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Affordance {
    Chevron,
    Toggle(bool),
}

/// One row in a grouped card: title, second line, a trailing chevron or
/// toggle, and the 1 px hairline under it (inset to the card's padding).
///
/// The whole row is the hit area (`a11y::touch`), and the painted body presses
/// with the M20A grammar — `FAST` in to 0.97, spring out — while the hit rect
/// never moves. `value_ink` lets the truth tint a value (the model's missing
/// file, the last crash) instead of painting a reassuring grey.
#[allow(clippy::too_many_arguments)]
fn card_row(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    id: impl std::hash::Hash,
    title: &str,
    value: &str,
    value_ink: Option<Color32>,
    enabled: bool,
    affordance: Affordance,
    divider: bool,
) -> egui::Response {
    let response = a11y::touch(ui, tokens, rect, &id, WidgetType::Button, title, enabled);
    let base = ui.id().with(&id);
    let press = chrome::press_frame(
        ui.ctx(),
        base.with("press"),
        enabled && response.is_pointer_button_down_on(),
        motion.reduce,
    );
    let scale = press.scale;
    let visual = Rect::from_center_size(rect.center(), rect.size() * scale);
    let radius = CornerRadius::same(tokens.radius.control);
    if press.fill > 0.0 {
        ui.painter().rect_filled(
            visual,
            radius,
            tokens.surface_active.gamma_multiply(press.fill),
        );
    } else if enabled && response.hovered() {
        ui.painter()
            .rect_filled(visual, radius, tokens.surface_hover);
    }

    let ink = if enabled {
        tokens.ink
    } else {
        tokens.ink_faint
    };
    let title_token = chrome::scaled(&type_scale::row_title(), scale);
    let value_token = chrome::scaled(&type_scale::caption(), scale);
    paint_elided(
        ui.painter(),
        pos2(
            visual.left() + CARD_PAD * scale,
            visual.top() + 12.0 * scale,
        ),
        title,
        &title_token,
        ink,
        visual.width() - CARD_PAD * 2.0 - 34.0,
    );
    paint_elided(
        ui.painter(),
        pos2(
            visual.left() + CARD_PAD * scale,
            visual.top() + 36.0 * scale,
        ),
        value,
        &value_token,
        value_ink.unwrap_or(tokens.ink_soft),
        visual.width() - CARD_PAD * 2.0 - 34.0,
    );
    match affordance {
        Affordance::Chevron => {
            let center = pos2(visual.right() - CARD_PAD - 9.0 * scale, visual.center().y);
            let size = icons::INLINE * scale;
            icons::paint(
                ui.painter(),
                Rect::from_center_size(center, vec2(size, size)),
                Icon::Chevron,
                size,
                tokens.ink_faint,
            );
        }
        Affordance::Toggle(on) => {
            paint_toggle(
                ui,
                tokens,
                motion,
                pos2(visual.right() - CARD_PAD - 23.0 * scale, visual.center().y),
                &id,
                on,
                scale,
            );
        }
    }
    if divider {
        ui.painter().line_segment(
            [
                pos2(rect.left() + CARD_PAD, rect.bottom() - 0.5),
                pos2(rect.right() - CARD_PAD, rect.bottom() - 0.5),
            ],
            Stroke::new(1.0_f32, tokens.hairline),
        );
    }
    response
}

/// A painted toggle (46 × 28 body) for a row whose whole body is the hit area.
/// The knob crossfades on the FAST token; no second hit rect is registered, so
/// TalkBack sees one control for the row, not two.
fn paint_toggle(
    ui: &Ui,
    tokens: &Tokens,
    motion: &Motion,
    center: egui::Pos2,
    id: impl std::hash::Hash,
    on: bool,
    scale: f32,
) {
    let t = motion.factor(ui.ctx(), ui.id().with(("toggle", &id)), on, motion::FAST);
    let track = Rect::from_center_size(center, vec2(46.0, 28.0) * scale);
    let fill = motion::lerp_color(tokens.control_fill, tokens.accent, t);
    ui.painter().rect_filled(
        track,
        CornerRadius::same((14.0 * scale).round() as u8),
        fill,
    );
    let knob_x = track.left() + 14.0 * scale + (track.width() - 28.0 * scale) * t;
    ui.painter()
        .circle_filled(pos2(knob_x, track.center().y), 11.0 * scale, Color32::WHITE);
}

/// A non-interactive row: the same anatomy as [`card_row`] without a chevron,
/// a toggle or a click. Used for the TRANSCRIPTION → Model summary, which is a
/// read-out (the picker is the model card below it).
fn readout_row(
    ui: &Ui,
    tokens: &Tokens,
    rect: Rect,
    title: &str,
    value: &str,
    value_ink: Option<Color32>,
) {
    // One line, value right-aligned: a summary, visibly not a control (the
    // Language row above it has a chevron; this one has nothing to open).
    let title_galley =
        ui.painter()
            .layout_no_wrap(title.to_owned(), type_scale::row_title().font, tokens.ink);
    ui.painter().galley(
        pos2(
            rect.left() + CARD_PAD,
            rect.center().y - title_galley.size().y / 2.0,
        ),
        title_galley,
        tokens.ink,
    );
    ui.painter().text(
        pos2(rect.right() - CARD_PAD, rect.center().y),
        Align2::RIGHT_CENTER,
        value,
        type_scale::caption().font,
        value_ink.unwrap_or(tokens.ink_soft),
    );
    a11y::describe(ui, rect, &format!("{title}: {value}"));
    ui.painter().line_segment(
        [
            pos2(rect.left() + CARD_PAD, rect.bottom() - 0.5),
            pos2(rect.right() - CARD_PAD, rect.bottom() - 0.5),
        ],
        Stroke::new(1.0_f32, tokens.hairline),
    );
}

/// The leading and trailing edges of one row inside the grouped card (M20D2).
///
/// The selected band is full-bleed, so it borrows the card's corner radius at
/// the top and bottom instead of painting a square tint over the rounded frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowEdges {
    first: bool,
    last: bool,
}

impl RowEdges {
    fn radius(self, card: u8) -> CornerRadius {
        CornerRadius {
            nw: if self.first { card } else { 0 },
            ne: if self.first { card } else { 0 },
            sw: if self.last { card } else { 0 },
            se: if self.last { card } else { 0 },
        }
    }
}

/// The word the recommendation capsule carries.
pub const RECOMMENDED: &str = "Recommended";

/// The text one model row paints on the primary Settings card (M20D2).
///
/// This is the row's paint source, not a parallel description: [`model_row`]
/// lays these strings out verbatim, and the tests read the same struct to prove
/// the manifest ids never reach a person on this surface. The raw id belongs to
/// Diagnostics ([`super::diagnostics::model_file_lines`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRowText {
    /// The human tier name, e.g. `Balanced`.
    pub name: &'static str,
    /// The pinned size, e.g. `488 MB`.
    pub size: String,
    /// The recommendation capsule's word, when this row is the tier default.
    pub badge: Option<&'static str>,
    /// The truthful second line: progress, a stale-file warning, or purpose.
    pub note: String,
    /// The action word: `Get`, `Download`, `Resume`, `Cancel`, `Re-download`,
    /// `Use`, or `In use`.
    pub action: &'static str,
}

impl ModelRowText {
    /// Every string the row paints, joined. The test surface for "no raw ids on
    /// the primary card"; keeping it here means the check cannot drift from the
    /// paint paths below.
    pub fn painted(&self) -> String {
        let mut out = format!("{} · {}", self.name, self.size);
        if let Some(badge) = self.badge {
            out.push(' ');
            out.push_str(badge);
        }
        out.push(' ');
        out.push_str(&self.note);
        out.push(' ');
        out.push_str(self.action);
        out
    }
}

/// Builds one row's painted text. `selected` is the configured id; `live` is
/// the row's job, when one is running.
pub fn model_row_text(
    row: &ModelRow,
    state: ModelState,
    selected: bool,
    live: Option<&DownloadView>,
) -> ModelRowText {
    let note = match (state, live) {
        (ModelState::Downloading, Some(download)) => format!(
            "Downloading · {}% · {} of {}",
            download.percent,
            super::model::bytes_label(download.bytes_done),
            super::model::bytes_label(download.bytes_total)
        ),
        (ModelState::StaleCorrupt, _) => {
            "The file on this phone is incomplete — download it again.".to_string()
        }
        (ModelState::NotDownloaded, _) if selected => {
            "This is the model Suramak is set to, but its file is not here yet.".to_string()
        }
        _ => row.phone_note(),
    };
    let action = match (state, live.is_some()) {
        (ModelState::Downloading, false) => "Resume",
        // The recommended row with no file — which may also be the configured
        // row — asks for a Download, not a vague Get.
        (ModelState::NotDownloaded, _) if selected || row.recommended => "Download",
        _ => state.action(),
    };
    ModelRowText {
        name: row.tier_name(),
        size: row.tier_line(),
        badge: row.recommended.then_some(RECOMMENDED),
        note,
        action,
    }
}

/// One model row in the grouped card (M20D2): the human tier name and size on
/// the first line, the recommendation capsule beside it when this is the tier
/// default, and one truthful second line (a job's progress, a stale-file
/// warning, the configured-but-missing sentence, or the tier's purpose).
///
/// The raw manifest id is **not** painted here; Diagnostics owns it. The state
/// action stays the M19B pill: `Get`, `Download`, `Resume`, `Cancel`,
/// `Re-download`, `Use`, or `In use` (only ever with a verified file).
///
/// Returns `(row_clicked, action_clicked)`; the caller owns the state machine.
#[allow(clippy::too_many_arguments)]
fn model_row(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    row: &ModelRow,
    selected: bool,
    live: Option<&DownloadView>,
    row_state: ModelState,
    edges: RowEdges,
) -> (bool, bool) {
    let in_use = row_state.is_in_use();
    let text = model_row_text(row, row_state, selected, live);
    let spoken = format!("{} model, {}", text.name, row_state.label());
    let response = a11y::touch(
        ui,
        tokens,
        rect,
        ("model-row", &row.id),
        WidgetType::Button,
        &spoken,
        row.offered,
    );
    let base = ui.id().with(("model", &row.id));
    let press = chrome::press_frame(
        ui.ctx(),
        base.with("press"),
        row.offered && response.is_pointer_button_down_on(),
        motion.reduce,
    );
    let scale = press.scale;
    let visual = Rect::from_center_size(rect.center(), rect.size() * scale);
    let radius = edges.radius(tokens.radius.card);

    // M20D2: the selected row is a FULL-BLEED tint band — the card's whole
    // width, no inset, no inner border — with a 3 px accent bar on its leading
    // edge. It groups with the card instead of reading as a pill nested in it.
    if in_use {
        ui.painter().rect_filled(rect, radius, tokens.accent_tint);
        // The bar starts and ends where the card's rounded corner meets its
        // straight edge: a 3 px rect cannot follow a 12 pt curve (egui clamps
        // the radius to half its width), and letting it run to the square top
        // pokes a nub outside the card on a first/last row.
        let trim = tokens.radius.card as f32;
        let bar_top = rect.top() + if edges.first { trim } else { 0.0 };
        let bar_bottom = rect.bottom() - if edges.last { trim } else { 0.0 };
        if bar_bottom > bar_top {
            let bar = Rect::from_min_max(
                pos2(rect.left(), bar_top),
                pos2(rect.left() + 3.0, bar_bottom),
            );
            ui.painter()
                .rect_filled(bar, CornerRadius::ZERO, tokens.accent);
        }
    } else if press.fill > 0.0 {
        ui.painter().rect_filled(
            visual,
            radius,
            tokens.surface_active.gamma_multiply(press.fill),
        );
    } else if row.offered && response.hovered() {
        ui.painter()
            .rect_filled(visual, radius, tokens.surface_hover);
    }

    let ink = if row.offered {
        tokens.ink
    } else {
        tokens.ink_faint
    };
    let name_token = chrome::scaled(&type_scale::row_title(), scale);
    let caption = chrome::scaled(&type_scale::caption(), scale);
    let text_width = visual.width() - CARD_PAD * 2.0 - ACTION_W - 12.0;

    // Measure the badge first so the title can elide around it. The note line
    // below keeps the card's full width: losing a few points off the size on
    // the narrowest 360 dp column is cheaper than truncating the one sentence
    // that explains a missing file.
    let badge = text.badge.map(|word| {
        ui.painter()
            .layout_no_wrap(word.to_owned(), caption.font.clone(), tokens.ink)
    });
    let badge_w = badge
        .as_ref()
        .map_or(0.0, |galley| galley.size().x + 16.0 * scale);
    let badge_gap = if badge.is_some() { 8.0 * scale } else { 0.0 };
    let title_width = (text_width - badge_w - badge_gap).max(48.0);

    // Line 1: "Balanced · 488 MB", then the recommendation capsule. The badge
    // is a shape with a word in it — not grey text appended to a raw id.
    let mut job = egui::text::LayoutJob::default();
    job.append(text.name, 0.0, name_token.format(ink));
    job.append(" · ", 0.0, caption.format(tokens.ink_soft));
    job.append(&text.size, 0.0, caption.format(tokens.ink_soft));
    job.wrap.max_width = title_width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    let name_galley = ui.painter().layout_job(job);
    ui.painter().galley(
        pos2(
            visual.left() + CARD_PAD * scale,
            visual.top() + 16.0 * scale,
        ),
        name_galley.clone(),
        ink,
    );
    if let (Some(galley), Some(word)) = (badge, text.badge) {
        let height = 20.0 * scale;
        let width = galley.size().x + 16.0 * scale;
        let capsule = Rect::from_min_size(
            pos2(
                visual.left() + CARD_PAD * scale + name_galley.size().x + badge_gap,
                visual.top() + 16.0 * scale,
            ),
            vec2(width, height),
        );
        ui.painter().rect(
            capsule,
            CornerRadius::same((height / 2.0).round() as u8),
            tokens.accent_tint,
            Stroke::new(1.0_f32, tokens.accent.gamma_multiply(0.45)),
            StrokeKind::Inside,
        );
        ui.painter().galley(
            pos2(
                capsule.center().x - galley.size().x / 2.0,
                capsule.center().y - galley.size().y / 2.0,
            ),
            galley,
            tokens.ink,
        );
        a11y::describe(ui, capsule, word);
    }

    // Line 2: the truth — a live job's progress, the stale-file sentence, the
    // configured-but-missing sentence, or the tier's purpose. It sits below
    // the action pill, so it can use the card's full width.
    let note_ink = if row.offered {
        tokens.ink_soft
    } else {
        tokens.ink_faint
    };
    let note_galley = paint_elided(
        ui.painter(),
        pos2(
            visual.left() + CARD_PAD * scale,
            visual.top() + 48.0 * scale,
        ),
        &text.note,
        &caption,
        note_ink,
        visual.width() - CARD_PAD * 2.0,
    );
    a11y::describe(
        ui,
        Rect::from_min_size(
            pos2(visual.left() + CARD_PAD, visual.top() + 46.0),
            vec2(
                visual.width() - CARD_PAD * 2.0,
                note_galley.size().y.max(16.0),
            ),
        ),
        &format!("{}, {}: {}", text.name, text.size, text.note),
    );

    // The action pill: a 48 pt hit area around a 36 pt painted body, centred
    // on the title line.
    let action_hit = Rect::from_min_size(
        pos2(rect.right() - CARD_PAD - ACTION_W, rect.top() + 4.0),
        vec2(ACTION_W, MIN_TOUCH),
    );
    let action_rect = Rect::from_min_size(
        pos2(action_hit.left(), rect.top() + 8.0),
        vec2(ACTION_W, ACTION_H * scale),
    );
    let action_response = a11y::touch(
        ui,
        tokens,
        action_hit,
        ("model-action", &row.id),
        WidgetType::Button,
        &format!("{} {}", text.action, text.name),
        row.offered && row_state.is_actionable(),
    );
    let primary = row_state == ModelState::Installed
        || (row_state == ModelState::NotDownloaded && (selected || row.recommended));
    let (fill, stroke, action_ink) = if !row.offered {
        (Color32::TRANSPARENT, tokens.hairline, tokens.ink_faint)
    } else if in_use {
        // A status, not a button — and since the row itself is already the
        // tint band, the pill paints no fill of its own (that would be the
        // nested pill M20D2 removed). The check and the word carry it.
        (Color32::TRANSPARENT, Color32::TRANSPARENT, tokens.ink)
    } else if primary {
        (tokens.accent, tokens.accent, tokens.on_accent)
    } else {
        (tokens.control_fill, tokens.hairline, tokens.ink)
    };
    ui.painter().rect(
        action_rect,
        CornerRadius::same(tokens.radius.control),
        fill,
        Stroke::new(1.0_f32, stroke),
        StrokeKind::Inside,
    );
    if in_use {
        // The check makes "In use" read as a state instead of a tap target.
        let text = ui.painter().layout_no_wrap(
            text.action.to_owned(),
            type_scale::caption().font,
            action_ink,
        );
        let icon = 15.0_f32;
        let gap = 5.0;
        let start = action_rect.center().x - (icon + gap + text.size().x) / 2.0;
        icons::paint(
            ui.painter(),
            Rect::from_center_size(
                pos2(start + icon / 2.0, action_rect.center().y),
                vec2(icon, icon),
            ),
            Icon::Check,
            icon,
            tokens.accent,
        );
        ui.painter().galley(
            pos2(
                start + icon + gap,
                action_rect.center().y - text.size().y / 2.0,
            ),
            text,
            action_ink,
        );
    } else {
        paint_elided_centered(
            ui.painter(),
            action_rect,
            text.action,
            &type_scale::caption(),
            action_ink,
            action_rect.width() - 12.0,
        );
    }

    if !edges.last {
        ui.painter().line_segment(
            [
                pos2(rect.left() + CARD_PAD, rect.bottom() - 0.5),
                pos2(rect.right() - CARD_PAD, rect.bottom() - 0.5),
            ],
            Stroke::new(1.0_f32, tokens.hairline),
        );
    }
    (response.clicked(), action_response.clicked())
}

/// Paints `text` on one line, elided with `…` when it does not fit `width`.
fn paint_elided(
    painter: &egui::Painter,
    pos: egui::Pos2,
    text: &str,
    token: &TypeToken,
    ink: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, token.format(ink));
    job.wrap.max_width = width.max(24.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    let galley = painter.layout_job(job);
    painter.galley(pos, galley.clone(), ink);
    galley
}

/// Paints `text` centred on one line inside `rect`, elided if needed.
fn paint_elided_centered(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    token: &TypeToken,
    ink: Color32,
    width: f32,
) {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, token.format(ink));
    job.wrap.max_width = width.max(24.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    let galley = painter.layout_job(job);
    painter.galley(
        pos2(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        ink,
    );
}

/// The geometry one `SURAMAK-LAYOUT` line carries (M19A2, M20D).
///
/// The two `quit_*` keys are **frozen machine names**: the emulator smoke
/// parses them. They mean "the list's real last control" — the Quit row on
/// desktop, the Diagnostics row on Android (where Quit does not exist), and
/// whichever of the two sits lower if both are drawn. `device_note` rides along
/// as the second probe field because it still names the device's disk truth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutProbe {
    pub last: Rect,
    pub device_note: Rect,
    pub inset_top: f32,
    pub inset_bottom: f32,
    /// The padding the list leaves under its last control, floor included.
    pub clearance: f32,
    /// Where the scroll viewport ends: the content band's bottom, i.e. the
    /// screen's bottom minus the bottom inset.
    pub viewport_bottom: f32,
    pub screen_bottom: f32,
}

impl LayoutProbe {
    /// Reads the last control and the DEVICE note out of the frame's a11y
    /// registry.
    ///
    /// `None` when either is missing — which is exactly "the Settings list is
    /// not on screen this frame", since the two are painted together.
    pub fn from_controls(
        controls: &[a11y::Control],
        insets: Insets,
        floor: f32,
        screen: Rect,
    ) -> Option<Self> {
        let find = |label: &str| {
            controls
                .iter()
                .find(|control| control.label == label)
                .map(|control| control.rect)
        };
        let last = [QUIT_LABEL, DIAGNOSTICS_LABEL]
            .iter()
            .filter_map(|label| find(label))
            .max_by(|a, b| a.bottom().total_cmp(&b.bottom()))?;
        let device_note = find(DEVICE_NOTE)?;
        Some(Self {
            last,
            device_note,
            inset_top: insets.top,
            inset_bottom: insets.bottom,
            clearance: bottom_clearance(insets.bottom, floor),
            viewport_bottom: screen.bottom() - insets.bottom,
            screen_bottom: screen.bottom(),
        })
    }

    /// The line's geometry identity, in tenths of a point.
    ///
    /// Two frames with the same key drew the list at the same place; the
    /// settle detector compares these, not floats, so pixel-snap noise cannot
    /// keep a resting list looking busy.
    pub fn key(&self) -> ProbeKey {
        fn tenths(value: f32) -> i32 {
            (value * 10.0).round() as i32
        }
        ProbeKey {
            quit_top: tenths(self.last.top()),
            quit_bottom: tenths(self.last.bottom()),
            device_bottom: tenths(self.device_note.bottom()),
            inset_top: tenths(self.inset_top),
            inset_bottom: tenths(self.inset_bottom),
            clearance: tenths(self.clearance),
        }
    }

    /// One machine-readable line. `ts` is the device's Unix time in
    /// milliseconds, so the emulator smoke can prove the line is newer than
    /// the swipe it asserts on; `settled` marks a line written after the
    /// geometry held still, i.e. the resting scroll position.
    pub fn line(&self, timestamp_ms: u128, settled: bool) -> String {
        format!(
            "SURAMAK-LAYOUT ts={timestamp_ms} settled={} route=Settings \
             quit_top={:.1} quit_bottom={:.1} device_top={:.1} device_bottom={:.1} \
             clearance={:.1} viewport_bottom={:.1} content_bottom={:.1} \
             screen_bottom={:.1} inset_bottom={:.1} inset_top={:.1}",
            settled as u8,
            self.last.top(),
            self.last.bottom(),
            self.device_note.top(),
            self.device_note.bottom(),
            self.clearance,
            self.viewport_bottom,
            self.viewport_bottom,
            self.screen_bottom,
            self.inset_bottom,
            self.inset_top,
        )
    }
}

/// [`LayoutProbe`]'s comparable geometry ([`LayoutProbe::key`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeKey {
    quit_top: i32,
    quit_bottom: i32,
    device_bottom: i32,
    inset_top: i32,
    inset_bottom: i32,
    clearance: i32,
}

/// The longest a changed probe may go unlogged: four lines a second.
pub const PROBE_LOG_INTERVAL: Duration = Duration::from_millis(250);

/// How long the geometry must hold before it counts as the resting position.
pub const PROBE_SETTLE: Duration = Duration::from_millis(300);

/// Decides when the shell writes a `SURAMAK-LAYOUT` line (M19A2).
///
/// M19A used a one-second wall clock. A swipe that ends in the same second as
/// its last log left the shell's `tail -1` reading a **mid-swipe** line — and
/// under reactive rendering (`ControlFlow::Wait`) the draw that would have
/// written a newer one may never happen at all. This gate logs
///
/// * whenever the geometry **changes**, at most [`PROBE_LOG_INTERVAL`] apart;
/// * once, [`PROBE_SETTLE`] after the geometry stops changing — the resting
///   position the smoke asserts on.
#[derive(Debug, Default)]
pub struct ProbeLog {
    current: Option<ProbeKey>,
    current_since: Option<Instant>,
    /// The key the resting line has already been written for.
    settled: Option<ProbeKey>,
    logged_at: Option<Instant>,
}

impl ProbeLog {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// `Some(settled)` when a line should be written now: `false` for a change
    /// line, `true` for the resting line.
    pub fn observe(&mut self, key: ProbeKey, now: Instant) -> Option<bool> {
        if self.current != Some(key) {
            self.current = Some(key);
            self.current_since = Some(now);
            // A new position owes its own resting line, even if this exact
            // key was resting earlier (swipe away and back).
            self.settled = None;
            if self
                .logged_at
                .is_some_and(|at| now.saturating_duration_since(at) < PROBE_LOG_INTERVAL)
            {
                return None;
            }
            self.logged_at = Some(now);
            return Some(false);
        }
        if self.settled == Some(key) {
            return None;
        }
        if now.saturating_duration_since(self.current_since.unwrap_or(now)) < PROBE_SETTLE {
            return None;
        }
        self.settled = Some(key);
        self.logged_at = Some(now);
        Some(true)
    }

    /// When the resting line is still owed, the instant it becomes due.
    ///
    /// The shell asks egui for a repaint at that instant: `ControlFlow::Wait`
    /// stops drawing the moment the finger stops, so without a timer the
    /// resting line would wait for the next unrelated frame.
    pub fn pending(&self) -> Option<Instant> {
        let key = self.current?;
        if self.settled == Some(key) {
            return None;
        }
        Some(self.current_since? + PROBE_SETTLE)
    }
}

fn language_label(language: &str) -> &'static str {
    match language {
        "en" => "English",
        "id" => "Indonesian",
        _ => "Auto-detect",
    }
}

fn next_language(language: &str) -> String {
    match language {
        "auto" => "en".to_string(),
        "en" => "id".to_string(),
        _ => "auto".to_string(),
    }
}

/// The Models section's one-line truth (M20D2): the phone's RAM, what is in
/// use, and what the tier recommends — in that order, naming RAM explicitly.
///
/// Three states carry the review's copy:
///
/// * tier default in use — `12 GB RAM · using Balanced`;
/// * another model in use — `12 GB RAM · using Compact — Balanced recommended`;
/// * nothing installed — `12 GB RAM · Balanced recommended`.
///
/// Display only: every fact comes from rows the caller already has, and the
/// model state machine is untouched.
pub fn model_sentence(
    tier: Option<PhoneTier>,
    ram_bytes: Option<u64>,
    recommended: Option<&ModelRow>,
    configured: Option<&ModelRow>,
) -> String {
    if tier == Some(PhoneTier::Unsupported) {
        return "This phone can't run a model".to_string();
    }
    if tier.is_none() && ram_bytes.is_none() {
        return "Reading this phone's memory…".to_string();
    }
    let ram = ram_label(tier, ram_bytes);
    let Some(recommended) = recommended else {
        return match tier {
            Some(PhoneTier::BelowFloor) => format!("{ram} RAM · only Quick test runs here"),
            _ => format!("{ram} RAM · no model fits this phone"),
        };
    };
    let name = recommended.tier_name();
    match configured.filter(|row| row.installed) {
        Some(row) if row.id == recommended.id => format!("{ram} RAM · using {name}"),
        Some(row) => format!("{ram} RAM · using {} — {name} recommended", row.tier_name()),
        // The tier default is on the phone but not selected: say so rather
        // than recommending a download that already happened.
        None if recommended.installed => format!("{ram} RAM · {name} downloaded — not in use"),
        None => format!("{ram} RAM · {name} recommended"),
    }
}

/// The RAM number the sentence quotes: the shell's report when it exists,
/// otherwise the tier's band. "RAM" is appended by [`model_sentence`].
fn ram_label(tier: Option<PhoneTier>, ram_bytes: Option<u64>) -> String {
    if let Some(bytes) = ram_bytes.filter(|bytes| *bytes > 0) {
        return format!("{:.0} GB", bytes as f64 / 1_000_000_000.0);
    }
    match tier {
        Some(PhoneTier::High) => "8 GB or more".to_string(),
        Some(PhoneTier::Mid) => "6 GB".to_string(),
        Some(PhoneTier::Low) => "4 GB".to_string(),
        Some(PhoneTier::BelowFloor) => "Below 4 GB".to_string(),
        Some(PhoneTier::Unsupported) | None => "Unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn probe(top: f32) -> LayoutProbe {
        LayoutProbe {
            last: Rect::from_min_size(pos2(16.0, top), egui::vec2(300.0, 52.0)),
            device_note: Rect::from_min_size(pos2(16.0, top - 200.0), egui::vec2(300.0, 16.0)),
            inset_top: 24.0,
            inset_bottom: 54.0 / 2.6,
            clearance: bottom_clearance(54.0 / 2.6, ANDROID_MIN_CLEARANCE),
            viewport_bottom: 900.0,
            screen_bottom: 920.8,
        }
    }

    #[test]
    fn the_clearance_has_a_floor_and_desktops_keep_the_old_number() {
        assert_eq!(
            bottom_clearance(0.0, 0.0),
            GUTTER,
            "a desktop window with no inset keeps the plain gutter"
        );
        assert_eq!(
            bottom_clearance(0.0, ANDROID_MIN_CLEARANCE),
            ANDROID_MIN_CLEARANCE + GUTTER,
            "an Android window that reports zero still leaves room for the pill"
        );
        assert_eq!(
            bottom_clearance(30.0, ANDROID_MIN_CLEARANCE),
            30.0 + GUTTER,
            "a real inset above the floor is used as reported"
        );
        assert_eq!(
            bottom_clearance(-4.0, ANDROID_MIN_CLEARANCE),
            ANDROID_MIN_CLEARANCE + GUTTER,
            "a negative inset is not trusted"
        );
    }

    #[test]
    fn the_probe_line_carries_the_last_control_and_the_viewport() {
        let line = probe(800.0).line(1_763_000_000_123, true);
        assert!(
            line.starts_with("SURAMAK-LAYOUT ts=1763000000123 settled=1"),
            "{line}"
        );
        assert!(line.contains("quit_bottom=852.0"), "{line}");
        assert!(line.contains("device_bottom=616.0"), "{line}");
        assert!(line.contains("viewport_bottom=900.0"), "{line}");
        assert!(line.contains("content_bottom=900.0"), "{line}");
        assert!(line.contains("inset_bottom=20.8"), "{line}");
        assert!(line.contains("inset_top=24.0"), "{line}");
        assert!(
            line.contains("clearance=40.0"),
            "the phone floor lifts a 20.8 pt inset to 24 dp + gutter: {line}"
        );
        assert!(
            !probe(800.0).line(0, false).contains("settled=1"),
            "a change line is not a settled line"
        );
    }

    fn control(label: &str, rect: Rect) -> a11y::Control {
        a11y::Control {
            label: label.to_string(),
            role: WidgetType::Button,
            rect,
            enabled: true,
        }
    }

    #[test]
    fn the_probe_measures_the_lowest_of_quit_and_diagnostics() {
        let insets = Insets {
            top: 24.0,
            bottom: 20.8,
        };
        let screen = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(360.0, 920.8));
        let note = Rect::from_min_size(pos2(16.0, 600.0), egui::vec2(320.0, 16.0));
        let diagnostics = Rect::from_min_size(pos2(16.0, 760.0), egui::vec2(320.0, 64.0));
        let quit = Rect::from_min_size(pos2(16.0, 840.0), egui::vec2(320.0, 52.0));

        // Desktop: both rows exist, Quit is lower and is the witness.
        let both = [
            control(DEVICE_NOTE, note),
            control(DIAGNOSTICS_LABEL, diagnostics),
            control(QUIT_LABEL, quit),
        ];
        let probe = LayoutProbe::from_controls(&both, insets, 0.0, screen).expect("desktop");
        assert_eq!(probe.last, quit);

        // Android: no Quit row, so the Diagnostics row is the last control.
        // (The machine keys still say `quit_*` — the smoke's contract.)
        let android = [
            control(DEVICE_NOTE, note),
            control(DIAGNOSTICS_LABEL, diagnostics),
        ];
        let probe = LayoutProbe::from_controls(&android, insets, 24.0, screen).expect("android");
        assert_eq!(probe.last, diagnostics);
        assert!(probe.line(0, true).contains("quit_bottom=824.0"));
    }

    #[test]
    fn a_changed_probe_logs_at_once_but_never_faster_than_four_hertz() {
        let mut log = ProbeLog::default();
        let start = Instant::now();
        assert_eq!(
            log.observe(probe(800.0).key(), start),
            Some(false),
            "the first frame of Settings is worth a line"
        );
        assert_eq!(
            log.observe(probe(760.0).key(), start + Duration::from_millis(100)),
            None,
            "a 60 Hz drag must not bury logcat"
        );
        assert_eq!(
            log.observe(probe(720.0).key(), start + Duration::from_millis(260)),
            Some(false),
            "four lines a second is the ceiling"
        );
    }

    #[test]
    fn a_resting_list_logs_once_after_the_settle_window() {
        let mut log = ProbeLog::default();
        let start = Instant::now();
        let key = probe(800.0).key();
        assert_eq!(log.observe(key, start), Some(false));
        assert_eq!(
            log.observe(key, start + Duration::from_millis(200)),
            None,
            "not settled yet"
        );
        assert_eq!(
            log.observe(key, start + Duration::from_millis(301)),
            Some(true),
            "the resting position gets its own line"
        );
        assert_eq!(
            log.observe(key, start + Duration::from_millis(900)),
            None,
            "and only one"
        );
    }

    #[test]
    fn a_throttled_change_still_gets_its_resting_line() {
        // The mid-swipe shape the audit found: two changes inside one 250 ms
        // window, so the second is throttled away — the resting line at +300 ms
        // is what `tail -1` must read.
        let mut log = ProbeLog::default();
        let start = Instant::now();
        assert_eq!(log.observe(probe(800.0).key(), start), Some(false));
        let resting = probe(700.0).key();
        assert_eq!(
            log.observe(resting, start + Duration::from_millis(100)),
            None,
            "throttled"
        );
        assert_eq!(
            log.observe(resting, start + Duration::from_millis(399)),
            None,
            "300 ms has not passed since the change at +100 ms"
        );
        assert_eq!(
            log.observe(resting, start + Duration::from_millis(401)),
            Some(true),
            "the last change is still reported, as the resting line"
        );
    }

    #[test]
    fn a_settle_is_owed_until_it_is_written() {
        let mut log = ProbeLog::default();
        let start = Instant::now();
        let key = probe(800.0).key();
        assert_eq!(log.pending(), None, "nothing to log before Settings draws");
        log.observe(key, start);
        assert_eq!(
            log.pending(),
            Some(start + PROBE_SETTLE),
            "the shell must wake itself for the resting line"
        );
        log.observe(key, start + Duration::from_millis(301));
        assert_eq!(log.pending(), None, "the line is written; no timer left");
    }

    #[test]
    fn leaving_and_returning_to_settings_logs_again() {
        let mut log = ProbeLog::default();
        let start = Instant::now();
        let key = probe(800.0).key();
        assert_eq!(log.observe(key, start), Some(false));
        log.reset();
        assert_eq!(
            log.observe(key, start + Duration::from_secs(5)),
            Some(false),
            "a fresh visit is a fresh line, even at the same scroll offset"
        );
    }

    const RAW_IDS: [&str; 4] = ["tiny", "base", "small", "turbo-q5-0"];

    fn model_row(id: &str, size_bytes: u64, installed: bool, recommended: bool) -> ModelRow {
        ModelRow {
            id: id.to_string(),
            size_bytes,
            role: String::new(),
            note: String::new(),
            offered: true,
            installed,
            disk: if installed {
                suramak_core::models::ModelFileState::Verified
            } else {
                suramak_core::models::ModelFileState::Absent
            },
            recommended,
        }
    }

    /// M20D2: the Models header names RAM and tells the truth in each of the
    /// three states the review called out.
    #[test]
    fn the_models_header_names_ram_and_the_state_across_the_three_states() {
        let recommended = model_row("small", 488_000_000, false, true);
        let other = model_row("base", 148_000_000, true, false);
        let recommended_installed = model_row("small", 488_000_000, true, true);
        let high = Some(PhoneTier::High);
        let ram = Some(12_000_000_000);

        assert_eq!(
            model_sentence(high, ram, Some(&recommended), None),
            "12 GB RAM · Balanced recommended",
            "nothing installed: name the recommendation, not a fake 'in use'"
        );
        assert_eq!(
            model_sentence(high, ram, Some(&recommended), Some(&other)),
            "12 GB RAM · using Compact — Balanced recommended",
            "a different model in use: say which, then what the tier recommends"
        );
        assert_eq!(
            model_sentence(
                high,
                ram,
                Some(&recommended_installed),
                Some(&recommended_installed)
            ),
            "12 GB RAM · using Balanced",
            "the tier default is in use: one truth, no redundant recommendation"
        );

        // The same sentence at the mid tier, off the shell's own RAM report.
        let compact = model_row("base", 148_000_000, true, true);
        assert_eq!(
            model_sentence(
                Some(PhoneTier::Mid),
                Some(6_000_000_000),
                Some(&compact),
                Some(&compact)
            ),
            "6 GB RAM · using Compact"
        );
    }

    /// M20D2: the primary model card paints human tier names only; the raw
    /// manifest ids live in Diagnostics' "Model files" lines. The row struct is
    /// the paint source ([`model_row`] lays it out verbatim), so this reads the
    /// exact strings a person sees.
    #[test]
    fn the_primary_card_paints_no_raw_id_but_diagnostics_keeps_them() {
        let rows = [
            model_row("tiny", 78_000_000, false, false),
            model_row("base", 148_000_000, true, false),
            model_row("small", 488_000_000, false, true),
            model_row("turbo-q5-0", 574_000_000, false, false),
        ];
        let selected = "base";
        for row in &rows {
            let state = row.state(row.id == selected, None);
            let text = model_row_text(row, state, row.id == selected, None);
            for id in RAW_IDS {
                assert!(
                    !text.painted().split_whitespace().any(|word| word == id),
                    "the primary card must not paint the manifest id {id:?}: {text:?}"
                );
            }
        }

        // The action wording the review asked for: the recommended row's
        // missing file downloads; other missing rows keep Get; an installed
        // row offers Use.
        let balanced = rows[2].state(false, None);
        assert_eq!(
            model_row_text(&rows[2], balanced, false, None).action,
            "Download",
            "recommended but not installed is a Download"
        );
        assert_eq!(
            model_row_text(&rows[0], ModelState::NotDownloaded, false, None).action,
            "Get",
            "a non-recommended missing model keeps Get"
        );
        assert_eq!(
            model_row_text(&rows[1], ModelState::Installed, false, None).action,
            "Use",
            "an installed, unselected model offers Use"
        );

        // Diagnostics keeps the raw vocabulary: one line per installed file.
        let files = crate::deck::compact::diagnostics::model_file_lines(&rows, selected);
        assert_eq!(
            files
                .iter()
                .map(|line| line.id.as_str())
                .collect::<Vec<_>>(),
            vec!["base"],
            "the installed model is the one Diagnostics names"
        );
        assert_eq!(files[0].value, "148 MB · in use");

        // With more than one verified file, every id that left the card is
        // still spelled in Diagnostics — one line each, none collapsed.
        let installed_rows = [
            model_row("tiny", 78_000_000, true, false),
            model_row("base", 148_000_000, true, false),
            model_row("small", 488_000_000, true, true),
            model_row("turbo-q5-0", 574_000_000, false, false),
        ];
        let files = crate::deck::compact::diagnostics::model_file_lines(&installed_rows, selected);
        assert_eq!(
            files
                .iter()
                .map(|line| line.id.as_str())
                .collect::<Vec<_>>(),
            vec!["tiny", "base", "small"],
            "a 'Model files' line per installed model, in manifest order"
        );
        assert!(
            files.iter().all(|line| RAW_IDS.contains(&line.id.as_str())),
            "Diagnostics speaks the raw manifest vocabulary: {files:?}"
        );
    }

    #[test]
    fn the_models_sentence_names_human_tiers_and_never_a_raw_id() {
        let recommended = model_row("small", 488_000_000, false, true);
        let installed = model_row("base", 148_000_000, true, false);
        for tier in [
            Some(PhoneTier::High),
            Some(PhoneTier::Mid),
            Some(PhoneTier::Low),
            Some(PhoneTier::BelowFloor),
            Some(PhoneTier::Unsupported),
            None,
        ] {
            for (ram, configured) in [
                (Some(12_000_000_000), None),
                (Some(12_000_000_000), Some(&installed)),
                (None, None),
            ] {
                let sentence = model_sentence(tier, ram, Some(&recommended), configured);
                for id in RAW_IDS {
                    assert!(
                        !sentence.split_whitespace().any(|word| word == id),
                        "{tier:?}: {sentence}"
                    );
                }
            }
        }
        assert!(model_sentence(
            Some(PhoneTier::High),
            Some(12_000_000_000),
            Some(&recommended),
            None
        )
        .contains("Balanced"));
        assert!(model_sentence(
            Some(PhoneTier::Mid),
            Some(6_000_000_000),
            Some(&installed),
            Some(&installed)
        )
        .contains("Compact"));
    }
}
