//! Compact Settings: language, models (with M13D's phone tiers), appearance,
//! storage, device facts, version and Quit.
//!
//! The phone's Settings is deliberately smaller than the desktop surface: no
//! HUD section (macOS-only), no fast-engine toggles that the Android build does
//! not compile, and no update channel. What remains is every setting the phone
//! build actually honours, plus the owner-requested Quit.

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
        let mut y = bands.content.top() + 14.0;

        // ---- Transcription -------------------------------------------
        chrome::section_label(ui, tokens, pos2(x, y), "Transcription");
        y += 18.0;
        let card = Rect::from_min_size(pos2(x, y), vec2(width, 168.0));
        chrome::card(ui, tokens, card);
        let model_label = state.settings.model_id.clone();
        if chrome::setting_row(
            ui,
            tokens,
            motion,
            Rect::from_min_size(pos2(x, y), vec2(width, 56.0)),
            "settings-language",
            "Language",
            language_label(&state.settings.language),
            true,
        )
        .clicked()
        {
            let next = next_language(&state.settings.language);
            state.apply_setting(CompactSetting::Language(next.clone()));
            clicks.push(CompactAction::Setting(CompactSetting::Language(next)));
        }
        // The model row opens the list below rather than a second screen.
        let _ = chrome::setting_row(
            ui,
            tokens,
            motion,
            Rect::from_min_size(pos2(x, y + 56.0), vec2(width, 56.0)),
            "settings-model",
            "Model",
            &model_label,
            false,
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
            let response = a11y::touch(
                ui,
                tokens,
                rect,
                ("model-row", &row.id),
                WidgetType::Button,
                &format!("Model {}", row.id),
                row.offered,
            );
            let selected = state.settings.model_id == row.id;
            let fill = if selected {
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
                    if selected {
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
            let action = if selected {
                "In use"
            } else if row.installed {
                "Use"
            } else {
                "Get"
            };
            let action_width = 86.0;
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
                format!(
                    "{} · {}{}",
                    bytes_label(row.size_bytes),
                    row.role,
                    if row.installed { " · installed" } else { "" }
                ),
                FontId::proportional(12.0),
                tokens.ink_soft,
            );
            ui.painter().text(
                pos2(rect.left() + 14.0, rect.top() + 57.0),
                Align2::LEFT_CENTER,
                truncate(&row.note, rect.width() - 28.0, 11.5),
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
                row.offered && !selected,
            );
            let primary = !selected && row.installed;
            let fills = if selected {
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
                    if selected {
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
                if selected {
                    tokens.accent
                } else if primary {
                    tokens.on_accent
                } else {
                    tokens.ink
                },
            );
            if action_response.clicked() && row.offered && !selected {
                if row.installed {
                    state.apply_setting(CompactSetting::Model(row.id.clone()));
                    clicks.push(CompactAction::Setting(CompactSetting::Model(
                        row.id.clone(),
                    )));
                } else {
                    clicks.push(CompactAction::DownloadModel(row.id.clone()));
                }
            } else if response.clicked() && row.installed && row.offered && !selected {
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
            "Everything runs on this phone. No analytics, no uploads.",
            FontId::proportional(11.5),
            tokens.ink_faint,
        );
        y = device_card.bottom() + 22.0;

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
        ui.allocate_space(vec2(width, (y - bands.content.top()).max(0.0)));
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
