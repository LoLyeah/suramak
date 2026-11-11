//! Compact Settings: language, models (with M13D's phone tiers), appearance,
//! storage, device facts, version and Quit.
//!
//! The phone's Settings is deliberately smaller than the desktop surface: no
//! HUD section (macOS-only), no fast-engine toggles that the Android build does
//! not compile, and no update channel. What remains is every setting the phone
//! build actually honours, plus the owner-requested Quit.

use egui::emath::GuiRounding;
use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Rect, ScrollArea, Stroke,
    StrokeKind, Ui, WidgetType,
};

use suramak_core::settings::Appearance;

use crate::deck::motion::Motion;
use crate::deck::theme::Tokens;

use super::a11y;
use super::chrome::{self, GUTTER, MIN_TOUCH};
use super::model::{bytes_label, CompactAction, CompactApp, CompactSetting, Sheet};

/// The DEVICE card's last line — also the smoke's bottom-clearance probe (the
/// shell's layout log reports this label's rect).
pub const DEVICE_NOTE: &str = "Everything runs on this phone. No analytics, no uploads.";

/// The padding the list leaves under its last control: the gesture inset the
/// shell reports plus the standard gutter (M19A).
pub fn bottom_clearance(inset_bottom: f32) -> f32 {
    inset_bottom.max(0.0) + GUTTER
}

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
        let mut y = top + 14.0;

        // ---- Transcription -------------------------------------------
        chrome::section_label(ui, tokens, pos2(x, y), "Transcription");
        y += 18.0;
        let card = Rect::from_min_size(pos2(x, y), vec2(width, 168.0));
        chrome::card(ui, tokens, card);
        let (model_label, model_missing) = state.model_label();
        if chrome::setting_row_with_ink(
            ui,
            tokens,
            motion,
            Rect::from_min_size(pos2(x, y), vec2(width, 56.0)),
            "settings-language",
            "Language",
            language_label(&state.settings.language),
            true,
            None,
        )
        .clicked()
        {
            let next = next_language(&state.settings.language);
            state.apply_setting(CompactSetting::Language(next.clone()));
            clicks.push(CompactAction::Setting(CompactSetting::Language(next)));
        }
        // The model row opens the list below rather than a second screen. Its
        // value carries the disk truth (M19B): a configured model with no
        // verified file says so in the warning ink instead of reading "in use".
        let _ = chrome::setting_row_with_ink(
            ui,
            tokens,
            motion,
            Rect::from_min_size(pos2(x, y + 56.0), vec2(width, 56.0)),
            "settings-model",
            "Model",
            &model_label,
            false,
            model_missing.then_some(tokens.warn),
        );
        let keep = state.settings.keep_audio;
        let toggle_row = Rect::from_min_size(pos2(x, y + 112.0), vec2(width, 56.0));
        let response = chrome::setting_row(
            ui,
            tokens,
            motion,
            toggle_row,
            "settings-keep-audio",
            "Keep the recording",
            "Keep the source audio after transcription",
            true,
        );
        if response.clicked() {
            let next = !keep;
            state.apply_setting(CompactSetting::KeepAudio(next));
            clicks.push(CompactAction::Setting(CompactSetting::KeepAudio(next)));
        }
        let toggle = chrome::toggle(
            ui,
            tokens,
            motion,
            pos2(toggle_row.right() - 40.0, toggle_row.center().y),
            "keep-audio-toggle",
            "Keep the recording",
            keep,
        );
        let _ = toggle; // the whole row is the hit area
        y = card.bottom() + 22.0;

        // ---- Models ---------------------------------------------------
        chrome::section_label(ui, tokens, pos2(x, y), "Models");
        y += 18.0;
        let tier_note = match state.tier {
            Some(tier) => format!("{} · {}", tier.label(), tier_note(tier)),
            None => "Device memory not reported by the shell".to_string(),
        };
        ui.painter().text(
            pos2(x, y),
            Align2::LEFT_CENTER,
            tier_note,
            FontId::proportional(12.5),
            tokens.ink_soft,
        );
        y += 16.0;
        let offered: Vec<_> = state
            .models
            .iter()
            .filter(|row| row.offered || row.installed)
            .cloned()
            .collect();
        let hidden = state.models.len().saturating_sub(offered.len());
        for row in offered {
            let height = 74.0;
            let rect = Rect::from_min_size(pos2(x, y), vec2(width, height));
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
            let in_use = row_state.is_in_use();
            let response = a11y::touch(
                ui,
                tokens,
                rect,
                ("model-row", &row.id),
                WidgetType::Button,
                &format!("Model {}", row.id),
                row.offered,
            );
            let fill = if in_use {
                tokens.accent_tint
            } else if response.hovered() {
                tokens.surface_hover
            } else {
                tokens.card
            };
            ui.painter().rect(
                rect,
                CornerRadius::same(tokens.radius.card),
                fill,
                Stroke::new(
                    1.0_f32,
                    if in_use {
                        tokens.accent
                    } else {
                        tokens.hairline
                    },
                ),
                StrokeKind::Inside,
            );
            let ink = if row.offered {
                tokens.ink
            } else {
                tokens.ink_faint
            };
            // Only a verified + selected file may say "In use" (M19B); every
            // other state names what is really there.
            let action = match (row_state, live.is_some()) {
                (super::model::ModelState::Downloading, false) => "Resume",
                // The configured model with no file is the one the row must
                // not lie about: a primary Download, not "In use"/"Get".
                (super::model::ModelState::NotDownloaded, _) if selected => "Download",
                _ => row_state.action(),
            };
            let action_width = 92.0;
            // The action sits on the title line; the note below gets the
            // full card width so a manifest sentence never runs under it.
            let action_rect = Rect::from_min_size(
                pos2(rect.right() - action_width - 12.0, rect.top() + 9.0),
                vec2(action_width, 32.0),
            );
            ui.painter().text(
                pos2(rect.left() + 14.0, rect.top() + 18.0),
                Align2::LEFT_CENTER,
                format!(
                    "{}  {}",
                    row.id,
                    if row.recommended {
                        "· recommended"
                    } else {
                        ""
                    }
                ),
                FontId::proportional(16.0),
                ink,
            );
            ui.painter().text(
                pos2(rect.left() + 14.0, rect.top() + 37.0),
                Align2::LEFT_CENTER,
                row.subtitle(),
                FontId::proportional(12.0),
                tokens.ink_soft,
            );
            let note = match (row_state, live.as_ref()) {
                (super::model::ModelState::Downloading, Some(download)) => format!(
                    "Downloading · {}% · {} of {}",
                    download.percent,
                    bytes_label(download.bytes_done),
                    bytes_label(download.bytes_total)
                ),
                (super::model::ModelState::StaleCorrupt, _) => {
                    "The file on this phone is incomplete — download it again.".to_string()
                }
                (super::model::ModelState::NotDownloaded, _) if selected => {
                    "This is the model Suramak is set to, but its file is not here yet.".to_string()
                }
                _ => row.note.clone(),
            };
            ui.painter().text(
                pos2(rect.left() + 14.0, rect.top() + 57.0),
                Align2::LEFT_CENTER,
                truncate(&note, rect.width() - 28.0, 11.5),
                FontId::proportional(11.5),
                if row.offered {
                    tokens.ink_soft
                } else {
                    tokens.ink_faint
                },
            );
            let action_response = a11y::touch(
                ui,
                tokens,
                action_rect,
                ("model-action", &row.id),
                WidgetType::Button,
                &format!("{} {}", action, row.id),
                row.offered && row_state.is_actionable(),
            );
            let primary = row_state == super::model::ModelState::Installed
                || (row_state == super::model::ModelState::NotDownloaded && selected);
            let fills = if in_use {
                tokens.accent_tint
            } else if primary {
                tokens.accent
            } else {
                tokens.control_fill
            };
            ui.painter().rect(
                action_rect,
                CornerRadius::same(tokens.radius.control),
                fills,
                Stroke::new(
                    1.0_f32,
                    if in_use {
                        tokens.accent
                    } else {
                        tokens.hairline
                    },
                ),
                StrokeKind::Inside,
            );
            ui.painter().text(
                action_rect.center(),
                Align2::CENTER_CENTER,
                action,
                FontId::proportional(12.5),
                if in_use {
                    tokens.accent
                } else if primary {
                    tokens.on_accent
                } else {
                    tokens.ink
                },
            );
            if action_response.clicked() && row.offered && row_state.is_actionable() {
                match row_state {
                    super::model::ModelState::Installed => {
                        state.apply_setting(CompactSetting::Model(row.id.clone()));
                        clicks.push(CompactAction::Setting(CompactSetting::Model(
                            row.id.clone(),
                        )));
                    }
                    super::model::ModelState::Downloading if live.is_some() => {
                        clicks.push(CompactAction::CancelDownload);
                    }
                    // A stale/corrupt file is unlinked by the host before the
                    // transfer starts; a bare `.part` resumes where it stopped.
                    super::model::ModelState::NotDownloaded
                    | super::model::ModelState::StaleCorrupt
                    | super::model::ModelState::Downloading => {
                        clicks.push(CompactAction::DownloadModel(row.id.clone()));
                    }
                    super::model::ModelState::InUse => {}
                }
            } else if response.clicked()
                && row.offered
                && row_state == super::model::ModelState::Installed
            {
                state.apply_setting(CompactSetting::Model(row.id.clone()));
                clicks.push(CompactAction::Setting(CompactSetting::Model(
                    row.id.clone(),
                )));
            }
            y += height + 8.0;
        }
        if hidden > 0 {
            ui.painter().text(
                pos2(x, y + 2.0),
                Align2::LEFT_CENTER,
                format!(
                    "{} larger model{} hidden — not offered on this phone",
                    hidden,
                    if hidden == 1 { "" } else { "s" }
                ),
                FontId::proportional(11.5),
                tokens.ink_faint,
            );
            y += 18.0;
        }
        y += 14.0;

        // ---- Appearance ----------------------------------------------
        chrome::section_label(ui, tokens, pos2(x, y), "Appearance");
        y += 18.0;
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
                    chips.left() + index as f32 * chip_w + 3.0,
                    chips.top() + 3.0,
                ),
                vec2(chip_w - 6.0, chips.height() - 6.0),
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
            if selected {
                ui.painter().rect(
                    rect,
                    CornerRadius::same(tokens.radius.control),
                    tokens.card,
                    Stroke::new(1.0_f32, tokens.accent),
                    StrokeKind::Inside,
                );
            }
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                appearance.label(),
                FontId::proportional(13.5),
                if selected {
                    tokens.ink
                } else {
                    tokens.ink_soft
                },
            );
            if response.clicked() && !selected {
                state.settings.appearance = *appearance;
                clicks.push(CompactAction::Setting(CompactSetting::Appearance(
                    *appearance,
                )));
            }
        }
        y = chips.bottom() + 22.0;

        // ---- Device / About ------------------------------------------
        chrome::section_label(ui, tokens, pos2(x, y), "Device");
        y += 18.0;
        let device_card = Rect::from_min_size(pos2(x, y), vec2(width, 84.0));
        chrome::card(ui, tokens, device_card);
        let ram = state
            .ram_bytes
            .map(|bytes| format!("{:.0} GB usable", bytes as f64 / 1_000_000_000.0))
            .unwrap_or_else(|| "memory not reported".to_string());
        ui.painter().text(
            pos2(device_card.left() + 14.0, device_card.top() + 24.0),
            Align2::LEFT_CENTER,
            format!("Suramak {} · Android arm64", state.version),
            FontId::proportional(14.0),
            tokens.ink,
        );
        ui.painter().text(
            pos2(device_card.left() + 14.0, device_card.top() + 48.0),
            Align2::LEFT_CENTER,
            ram,
            FontId::proportional(12.5),
            tokens.ink_soft,
        );
        ui.painter().text(
            pos2(device_card.left() + 14.0, device_card.top() + 68.0),
            Align2::LEFT_CENTER,
            DEVICE_NOTE,
            FontId::proportional(11.5),
            tokens.ink_faint,
        );
        // The DEVICE note is the bottom-clearance probe: the shell's layout log
        // reports this label's rect, and the emulator smoke asserts it sits
        // above the gesture pill after a scroll to the end. It also gives
        // TalkBack the sentence instead of leaving it as anonymous paint.
        a11y::describe(
            ui,
            Rect::from_min_size(
                pos2(device_card.left() + 14.0, device_card.top() + 60.0),
                vec2(width - 28.0, 16.0),
            ),
            DEVICE_NOTE,
        );
        y = device_card.bottom() + 22.0;

        // ---- Diagnostics (M13g3) --------------------------------------
        // The M13b spike's four checks live here now, one card per group, with
        // the v2.4 crash banner. It stays a push (back returns to Settings) and
        // every read-out is on that screen.
        chrome::section_label(ui, tokens, pos2(x, y), "Diagnostics");
        y += 18.0;
        let diagnostics_row = Rect::from_min_size(pos2(x, y), vec2(width, 58.0));
        let response = chrome::setting_row(
            ui,
            tokens,
            motion,
            diagnostics_row,
            "settings-diagnostics",
            "Diagnostics",
            "Four checks · last crash",
            true,
        );
        if response.clicked() {
            state.open_diagnostics(now);
        }
        y = diagnostics_row.bottom() + 22.0;

        // ---- Quit -----------------------------------------------------
        let quit = Rect::from_min_size(pos2(x, y), vec2(width, 52.0));
        let response = a11y::touch(
            ui,
            tokens,
            quit,
            "settings-quit",
            WidgetType::Button,
            "Quit Suramak",
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
            "Quit Suramak",
            FontId::proportional(15.0),
            tokens.record_red_text,
        );
        if response.clicked() {
            state.request_quit();
        }
        y = quit.bottom() + 30.0;
        // M19A bottom clearance. The viewport ends at the content band's
        // bottom, which already excludes the gesture inset the shell reports —
        // but a phone that has not pushed its insets yet (or an immersive
        // window) reports zero, and a maxed-out scroll then pins the last
        // line to the pill. Padding the CONTENT by the inset plus the standard
        // gutter is what the last line scrolls above; without it the scroll
        // range ends exactly at the DEVICE card's edge.
        let tail = bottom_clearance(state.insets.bottom);
        ui.allocate_space(vec2(width, (y - top).max(0.0) + tail));
    });

    actions.extend(clicks);
    if state.sheet == Some(Sheet::Quit) {
        // The sheet itself is drawn by the dispatcher.
    }
    actions
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

fn tier_note(tier: suramak_core::device::PhoneTier) -> &'static str {
    use suramak_core::device::PhoneTier;
    match tier {
        PhoneTier::High => "the small model is the phone default",
        PhoneTier::Mid => "base is the default; small is available",
        PhoneTier::Low => "base is the default",
        PhoneTier::BelowFloor => "only tiny runs here",
        PhoneTier::Unsupported => "no 64-bit arm64 support",
    }
}

/// Truncates a model note to the width left of the action column, with an
/// ellipsis (the notes come from the manifest and are longer than a phone row).
fn truncate(text: &str, width: f32, font_size: f32) -> String {
    let approximate = (width / (font_size * 0.46)).max(8.0) as usize;
    if text.chars().count() <= approximate {
        return text.to_string();
    }
    let mut out: String = text.chars().take(approximate.saturating_sub(1)).collect();
    out.push('…');
    out
}
