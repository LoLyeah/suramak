//! Compact Settings: language, models, appearance, device facts and Quit.
//!
//! **M20D — de-jargoned and grouped.** The phone's Settings is the same product
//! as the redrawn Library: one frosted card per section (the Card plane — veil
//! plus 1 px rim, no shadow), an 8 pt rhythm (16 screen margins, 8 under a
//! section label, 24 between sections), hairlines only *inside* a grouped card,
//! and the type scale's 11 pt CAPS section labels.
//!
//! The model table is ONE card: each row leads with a human tier name and its
//! size, keeps the manifest id on a small mono second line (and in Diagnostics)
//! for support, and says what the tier is for in plain words. The five-state
//! actions of M19B are unchanged — `In use` still requires a verified file.
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
/// One model row: tier name, mono id, one guidance line.
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

        // ---- Models ---------------------------------------------------
        y = section(ui, tokens, x, y, "Models");
        let sentence = tier_sentence(state.tier);
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
            let last = index + 1 == offered.len();
            let (row_clicked, action_clicked) = model_row(
                ui,
                tokens,
                motion,
                rect,
                row,
                selected,
                live.as_ref(),
                row_state,
                !last,
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

/// One model row in the grouped card (M20D): the human tier name and size on
/// the first line, the manifest id in mono plus an optional "Recommended" tag
/// on the second, and one truthful third line (a job's progress, a stale-file
/// warning, the configured-but-missing sentence, or the tier's purpose). The
/// state action stays the M19B pill: `Get`, `Resume`, `Cancel`, `Re-download`,
/// `Use`, or `In use` (only ever with a verified file).
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
    divider: bool,
) -> (bool, bool) {
    let in_use = row_state.is_in_use();
    let spoken = format!("{} model, {}", row.tier_name(), row_state.label());
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

    // The selection / hover / press band is inset from the card's rim so it
    // never paints over the card's rounded corners.
    let band = Rect::from_min_max(
        pos2(visual.left() + 8.0, visual.top() + 2.0),
        pos2(visual.right() - 8.0, visual.bottom() - 2.0),
    );
    let radius = CornerRadius::same(tokens.radius.control);
    if in_use {
        // The plan's selected rule: tint fill + 1 px accent stroke at 45 %.
        ui.painter().rect(
            band,
            radius,
            tokens.accent_tint,
            Stroke::new(1.0_f32, tokens.accent.gamma_multiply(0.45)),
            StrokeKind::Inside,
        );
    } else if press.fill > 0.0 {
        ui.painter().rect_filled(
            band,
            radius,
            tokens.surface_active.gamma_multiply(press.fill),
        );
    } else if row.offered && response.hovered() {
        ui.painter().rect_filled(band, radius, tokens.surface_hover);
    }

    let ink = if row.offered {
        tokens.ink
    } else {
        tokens.ink_faint
    };
    let name_token = chrome::scaled(&type_scale::row_title(), scale);
    let caption = chrome::scaled(&type_scale::caption(), scale);
    let mono = chrome::scaled(&type_scale::mono(), scale);
    let text_width = visual.width() - CARD_PAD * 2.0 - ACTION_W - 12.0;

    // Line 1: "Balanced · 488 MB".
    let mut job = egui::text::LayoutJob::default();
    job.append(row.tier_name(), 0.0, name_token.format(ink));
    job.append(" · ", 0.0, caption.format(tokens.ink_soft));
    job.append(&row.tier_line(), 0.0, caption.format(tokens.ink_soft));
    job.wrap.max_width = text_width.max(48.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    let name_galley = ui.painter().layout_job(job);
    ui.painter().galley(
        pos2(
            visual.left() + CARD_PAD * scale,
            visual.top() + 12.0 * scale,
        ),
        name_galley,
        ink,
    );

    // Line 2: the manifest id in mono (support can name the file), then the
    // Recommended tag — attached to the id, never floating beside the action
    // pill. `ink_soft`, not `ink_faint`: the id is the line support reads
    // aloud, and the light ladder's faint is under the small-text floor.
    let mut id_job = egui::text::LayoutJob::default();
    id_job.append(&row.id, 0.0, mono.format(tokens.ink_soft));
    if row.recommended {
        id_job.append("  ·  ", 0.0, caption.format(tokens.ink_soft));
        // Ink, not accent: on the selected row's accent tint the ladder's
        // accent is 3.7:1 in light and 4.47:1 in dark — under the small-text
        // floor. The tint band and the check carry the accent instead.
        id_job.append("Recommended", 0.0, caption.format(tokens.ink));
    }
    id_job.wrap.max_width = text_width.max(48.0);
    id_job.wrap.max_rows = 1;
    id_job.wrap.break_anywhere = false;
    id_job.wrap.overflow_character = Some('…');
    let id_galley = ui.painter().layout_job(id_job);
    ui.painter().galley(
        pos2(
            visual.left() + CARD_PAD * scale,
            visual.top() + 34.0 * scale,
        ),
        id_galley,
        tokens.ink_soft,
    );

    // Line 3: the truth — a live job's progress, the stale-file sentence, the
    // configured-but-missing sentence, or the tier's purpose.
    let note = match (row_state, live) {
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
    let note_ink = if row.offered {
        tokens.ink_soft
    } else {
        tokens.ink_faint
    };
    let note_galley = paint_elided(
        ui.painter(),
        pos2(
            visual.left() + CARD_PAD * scale,
            visual.top() + 55.0 * scale,
        ),
        &note,
        &caption,
        note_ink,
        visual.width() - CARD_PAD * 2.0,
    );
    a11y::describe(
        ui,
        Rect::from_min_size(
            pos2(visual.left() + CARD_PAD, visual.top() + 53.0),
            vec2(
                visual.width() - CARD_PAD * 2.0,
                note_galley.size().y.max(16.0),
            ),
        ),
        &format!("{}, {}: {note}", row.tier_name(), row.tier_line()),
    );

    // The action pill: a 48 pt hit area around a 36 pt painted body, on the
    // title line's trailing edge.
    // The action sits on the two text lines (name + id); the note underneath
    // takes the row's full width, so a manifest sentence can never run under
    // the pill. The hit area is 48 pt even though the painted pill is 36.
    let action_hit = Rect::from_min_size(
        pos2(rect.right() - CARD_PAD - ACTION_W, rect.top() + 4.0),
        vec2(ACTION_W, MIN_TOUCH),
    );
    let action_rect = Rect::from_min_size(
        pos2(action_hit.left(), rect.top() + 10.0),
        vec2(ACTION_W, ACTION_H * scale),
    );
    let action = match (row_state, live.is_some()) {
        (ModelState::Downloading, false) => "Resume",
        // The configured model with no file is the one the row must not lie
        // about: a primary Download, not "In use"/"Get".
        (ModelState::NotDownloaded, _) if selected => "Download",
        _ => row_state.action(),
    };
    let action_response = a11y::touch(
        ui,
        tokens,
        action_hit,
        ("model-action", &row.id),
        WidgetType::Button,
        &format!("{action} {}", row.tier_name()),
        row.offered && row_state.is_actionable(),
    );
    let primary =
        row_state == ModelState::Installed || (row_state == ModelState::NotDownloaded && selected);
    let (fill, stroke, action_ink) = if !row.offered {
        (Color32::TRANSPARENT, tokens.hairline, tokens.ink_faint)
    } else if in_use {
        // A status, not a button: the tint fill and an accent check, no
        // outline. The word is ink — accent text on the tint is 3.7:1 in the
        // light ladder (4.47:1 in the dark), under the 4.5:1 floor.
        (tokens.accent_tint, Color32::TRANSPARENT, tokens.ink)
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
        let text =
            ui.painter()
                .layout_no_wrap(action.to_owned(), type_scale::caption().font, action_ink);
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
            action,
            &type_scale::caption(),
            action_ink,
            action_rect.width() - 12.0,
        );
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

/// The Models section's supporting sentence: the phone's memory class in human
/// words. The tier's raw model ids never appear here (M20D).
pub fn tier_sentence(tier: Option<PhoneTier>) -> String {
    match tier {
        Some(PhoneTier::High) => "8 GB or more · Suramak picks Balanced here".to_string(),
        Some(PhoneTier::Mid) => "6 GB · Suramak picks Compact here".to_string(),
        Some(PhoneTier::Low) => "4 GB · Suramak picks Compact here".to_string(),
        Some(PhoneTier::BelowFloor) => "Below 4 GB · only Quick test runs here".to_string(),
        Some(PhoneTier::Unsupported) => "This phone can't run a model".to_string(),
        None => "Reading this phone's memory…".to_string(),
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

    #[test]
    fn the_tier_sentence_names_human_tiers_and_never_a_raw_id() {
        for tier in [
            Some(PhoneTier::High),
            Some(PhoneTier::Mid),
            Some(PhoneTier::Low),
            Some(PhoneTier::BelowFloor),
            Some(PhoneTier::Unsupported),
            None,
        ] {
            let sentence = tier_sentence(tier);
            for id in ["tiny", "base", "small", "turbo-q5-0"] {
                assert!(
                    !sentence.split_whitespace().any(|word| word == id),
                    "{tier:?}: {sentence}"
                );
            }
        }
        assert!(tier_sentence(Some(PhoneTier::High)).contains("Balanced"));
        assert!(tier_sentence(Some(PhoneTier::Mid)).contains("Compact"));
    }
}
