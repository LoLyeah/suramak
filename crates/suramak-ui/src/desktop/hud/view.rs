//! The Compact HUD's egui card (M16B).
//!
//! Painted from the same Deck tokens as the rest of the app: one opaque card
//! (`tokens.sheet` — the popup surface, chosen because the HUD has no AppKit
//! material behind it and must stay legible over any app), one hairline, the
//! record red for the living dot, serif only for nothing (the card is utility,
//! not an article).
//!
//! The view is **stateless apart from the state machine it is handed**: every
//! button returns a [`HudIntent`]; the caller applies it and repaints. This is
//! what keeps the card screenshot-testable on Linux and the transitions tested
//! without egui.

use egui::{Align, Color32, FontFamily, FontId, Layout, RichText, Stroke};

use crate::app::controls::{self, Tier};
use crate::app::format_elapsed;
use crate::motion::Motion;
use crate::theme::{self, AppTheme, Tokens};

use super::settings::{InsertionPreference, AVOIDED_SYSTEM_CHORDS};
use super::state::{HudChrome, HudIntent, HudState, PermissionKind, CANCELLED_DWELL};
use super::{HUD_HEIGHT, HUD_WIDTH};

/// The card's outer margin (the window is transparent; this is the shadow's
/// breathing room).
const MARGIN: f32 = 12.0;
/// Inner padding of the card.
const PAD: f32 = 18.0;

/// Paints one frame of the card and returns the user's intents.
pub fn draw(
    ctx: &egui::Context,
    state: &HudState,
    chrome: &HudChrome,
    motion: &Motion,
) -> Vec<HudIntent> {
    let tokens = theme::tokens(current_theme(ctx));
    let mut intents = Vec::new();
    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(Color32::TRANSPARENT)
                .inner_margin(MARGIN),
        )
        .show(ctx, |ui| {
            let frame = egui::Frame::NONE
                .fill(card_fill(&tokens))
                .stroke(card_stroke(&tokens))
                .corner_radius(card_radius(&tokens))
                .inner_margin(PAD);
            // A compact shadow that fits inside `MARGIN` (the shared card
            // token's 20–24 pt blur would clip at the window edge).
            let frame = if chrome.card_shadow {
                frame.shadow(egui::epaint::Shadow {
                    offset: [0, 4],
                    blur: 10,
                    spread: 0,
                    color: tokens.card_shadow.color,
                })
            } else {
                frame
            };
            frame.show(ui, |ui| {
                ui.set_width((HUD_WIDTH - 2.0 * MARGIN - 2.0 * PAD).max(120.0));
                header(ui, &tokens, motion, state, chrome);
                ui.add_space(10.0);
                match state {
                    HudState::Idle => {}
                    HudState::Recording(recording) => {
                        recording_body(ui, &tokens, motion, recording, chrome, &mut intents);
                    }
                    HudState::Processing(processing) => {
                        processing_body(ui, &tokens, motion, processing, &mut intents);
                    }
                    HudState::Result(result) => {
                        result_body(ui, &tokens, motion, result, chrome, &mut intents);
                    }
                    HudState::Cancelled(cancelled) => {
                        cancelled_body(ui, &tokens, motion, cancelled, &mut intents);
                    }
                    HudState::PermissionFailure(permission) => {
                        permission_body(ui, &tokens, motion, permission, &mut intents);
                    }
                }
            });
        });
    intents
}

/// The current ladder is installed in this context by the window; the tokens
/// lookup only needs the enum, so the context's dark/light flag is read back.
fn current_theme(ctx: &egui::Context) -> AppTheme {
    if ctx.style().visuals.dark_mode {
        AppTheme::Dark
    } else {
        AppTheme::Light
    }
}

fn header(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &HudState,
    chrome: &HudChrome,
) {
    ui.horizontal(|ui| {
        // The state beacon: red and breathing while the mic is open, a calm
        // accent otherwise. Purely decorative — the words beside it carry the
        // state for screen readers.
        let (color, filled) = match state {
            HudState::Recording(_) => (tokens.record_red, true),
            HudState::Processing(_) => (tokens.accent, true),
            HudState::Result(_) => (tokens.ok, true),
            HudState::Cancelled(_) => (tokens.ink_faint, false),
            HudState::PermissionFailure(_) => (tokens.warn, true),
            HudState::Idle => (tokens.ink_faint, false),
        };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        // The living dot: a gentle 1.6 s breath while recording; Reduce Motion
        // keeps it still (the words carry the state either way).
        let radius = if state.capture_live() && !motion.reduce {
            let time = ui.ctx().input(|input| input.time) as f32;
            4.2 + 1.1 * (time * 3.9).sin().abs()
        } else {
            4.2
        };
        if filled {
            ui.painter().circle_filled(rect.center(), radius, color);
        } else {
            ui.painter()
                .circle_stroke(rect.center(), radius, Stroke::new(1.0_f32, color));
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new(state.title())
                .size(14.0)
                .strong()
                .color(tokens.ink),
        );
        if state.capture_live() {
            ui.label(
                RichText::new("microphone open")
                    .size(11.0)
                    .color(tokens.record_red_text),
            );
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let hint = match state {
                HudState::Recording(_) => format!("{} to stop", chrome.hotkey_label),
                HudState::Processing(_) => "Esc cancels".to_string(),
                HudState::Result(_) => format!("{} focuses this card", chrome.hotkey_label),
                HudState::PermissionFailure(_) | HudState::Cancelled(_) => "Esc closes".to_string(),
                HudState::Idle => String::new(),
            };
            if !hint.is_empty() {
                ui.label(
                    RichText::new(hint)
                        .size(11.0)
                        .family(FontFamily::Proportional)
                        .color(tokens.ink_soft),
                );
            }
        });
    });
}

fn recording_body(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    recording: &super::state::RecordingHud,
    chrome: &HudChrome,
    intents: &mut Vec<HudIntent>,
) {
    ui.horizontal(|ui| {
        // Elapsed — the one clock format (M9c): hh:mm:ss.
        ui.label(
            RichText::new(format_elapsed(recording.elapsed))
                .font(FontId::new(32.0, FontFamily::Monospace))
                .color(tokens.ink),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // A humble level bar: the RMS the capture layer reports. It is a
            // level, not a waveform (the HUD has no peaks to draw).
            let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 6.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 3.0, tokens.panel_faint);
            let width = rect.width() * recording.level.clamp(0.0, 1.0);
            if width > 0.5 {
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(rect.min, egui::vec2(width, rect.height())),
                    3.0,
                    tokens.record_red,
                );
            }
        });
    });
    ui.add_space(2.0);
    ui.label(
        RichText::new("The microphone is open — everything is transcribed locally.")
            .size(12.0)
            .color(tokens.ink_soft),
    );
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if controls::button(
            ui,
            tokens,
            motion,
            "hud-stop",
            "Stop",
            Tier::Primary,
            !recording.stopping,
        )
        .clicked()
        {
            intents.push(HudIntent::Stop);
        }
        if controls::button(
            ui,
            tokens,
            motion,
            "hud-cancel-recording",
            "Cancel",
            Tier::Tertiary,
            !recording.stopping,
        )
        .clicked()
        {
            intents.push(HudIntent::Cancel);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{} or the button", chrome.hotkey_label))
                    .size(11.0)
                    .color(tokens.ink_soft),
            );
        });
    });
    // The cadence policy (100 ms) is what makes the dot breathe; this view
    // never requests a repaint of its own (the M9 budget rule).
    ui.add_space(6.0);
    ui.label(
        RichText::new("Recording is visible here and in the main window until it stops.")
            .size(10.5)
            .color(tokens.ink_soft),
    );
}

fn processing_body(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    processing: &super::state::ProcessingHud,
    intents: &mut Vec<HudIntent>,
) {
    let stage = processing
        .stage
        .as_deref()
        .map(|stage| match stage {
            "decoding" => "Preparing the audio",
            "transcribing" => "Transcribing",
            other => other,
        })
        .unwrap_or("Transcribing");
    let progress = processing
        .progress
        .map(|pct| format!("{stage} — {pct}%"))
        .unwrap_or_else(|| format!("{stage}…"));
    ui.label(RichText::new(progress).size(13.0).color(tokens.ink));
    ui.add_space(8.0);
    // Indeterminate when the worker has not reported a percentage yet; a
    // determinate fill once it has. Reduce Motion: a static third.
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 4.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, tokens.panel_faint);
    let fraction = processing
        .progress
        .map(|pct| (pct as f32 / 100.0).clamp(0.0, 1.0))
        .unwrap_or_else(|| {
            if motion.reduce {
                0.33
            } else {
                let time = ui.ctx().input(|input| input.time) as f32;
                (time * 0.6).fract()
            }
        });
    let width = (rect.width() * fraction).max(6.0);
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(width, rect.height())),
        2.0,
        tokens.accent,
    );
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if controls::button(
            ui,
            tokens,
            motion,
            "hud-cancel-processing",
            "Cancel",
            Tier::Secondary,
            true,
        )
        .clicked()
        {
            intents.push(HudIntent::Cancel);
        }
    });
}

fn result_body(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    result: &super::state::ResultHud,
    chrome: &HudChrome,
    intents: &mut Vec<HudIntent>,
) {
    ui.label(
        RichText::new("Your transcript")
            .size(12.0)
            .color(tokens.ink_soft),
    );
    ui.add_space(4.0);
    egui::Frame::NONE
        .fill(tokens.panel_faint)
        .corner_radius(tokens.radius.control)
        .inner_margin(10.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("hud-result-text")
                .max_height(66.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&result.text)
                                .size(13.0)
                                .color(tokens.ink)
                                .family(FontFamily::Proportional),
                        )
                        .selectable(true)
                        .wrap(),
                    );
                });
        });
    ui.add_space(10.0);

    // Primary: Insert when the permission is there and the preference asks for
    // it; Copy otherwise (the honest fallback).
    let insert_primary =
        chrome.accessibility && chrome.insertion == InsertionPreference::InsertPrimary;
    ui.horizontal(|ui| {
        let insert = controls::button(
            ui,
            tokens,
            motion,
            "hud-insert",
            "Insert",
            if insert_primary {
                Tier::Primary
            } else {
                Tier::Secondary
            },
            chrome.accessibility && !result.text.is_empty(),
        );
        if insert.clicked() {
            intents.push(HudIntent::Insert);
        }
        let copy = controls::button(
            ui,
            tokens,
            motion,
            "hud-copy",
            if result.copied { "Copied" } else { "Copy" },
            if insert_primary {
                Tier::Secondary
            } else {
                Tier::Primary
            },
            !result.text.is_empty(),
        );
        if copy.clicked() {
            intents.push(HudIntent::Copy);
        }
        let keep = controls::button(
            ui,
            tokens,
            motion,
            "hud-keep",
            if result.kept { "Kept" } else { "Keep" },
            Tier::Secondary,
            true,
        );
        let keep = keep.on_hover_text(
            "Keeps this card on screen. The transcript is already in your Library either way.",
        );
        if keep.clicked() {
            intents.push(HudIntent::Keep);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if controls::button(
                ui,
                tokens,
                motion,
                "hud-dismiss-result",
                "Dismiss",
                Tier::Tertiary,
                true,
            )
            .clicked()
            {
                intents.push(HudIntent::Dismiss);
            }
        });
    });

    if let Some(note) = &result.note {
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                RichText::new(note)
                    .size(11.0)
                    .color(tokens.record_red_text)
                    .family(FontFamily::Proportional),
            )
            .wrap(),
        );
    }
    ui.add_space(4.0);
    if chrome.accessibility {
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Insert writes this text to your clipboard, then presses ⌘V in the app you \
                     were in. Copy only writes the clipboard.",
                )
                .size(10.5)
                .color(tokens.ink_soft)
                .family(FontFamily::Proportional),
            )
            .wrap(),
        );
    } else {
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Copy puts the transcript on your clipboard. Insert needs the Accessibility \
                     permission — Suramak presses ⌘V for you in the app you were in.",
                )
                .size(10.5)
                .color(tokens.ink_soft)
                .family(FontFamily::Proportional),
            )
            .wrap(),
        );
        ui.add_space(2.0);
        if controls::button(
            ui,
            tokens,
            motion,
            "hud-open-accessibility",
            "Open Accessibility Settings",
            Tier::Tertiary,
            true,
        )
        .on_hover_text("System Settings › Privacy & Security › Accessibility")
        .clicked()
        {
            intents.push(HudIntent::OpenSettings);
        }
    }
}

fn cancelled_body(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    cancelled: &super::state::CancelledHud,
    intents: &mut Vec<HudIntent>,
) {
    ui.add(
        egui::Label::new(
            RichText::new(&cancelled.reason)
                .size(13.0)
                .color(tokens.ink)
                .family(FontFamily::Proportional),
        )
        .wrap(),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if controls::button(
            ui,
            tokens,
            motion,
            "hud-dismiss-cancelled",
            "Dismiss",
            Tier::Secondary,
            true,
        )
        .clicked()
        {
            intents.push(HudIntent::Dismiss);
        }
        ui.label(
            RichText::new(format!("closes in {:.1} s", CANCELLED_DWELL.as_secs_f32()))
                .size(11.0)
                .color(tokens.ink_soft),
        );
    });
}

fn permission_body(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    permission: &super::state::PermissionHud,
    intents: &mut Vec<HudIntent>,
) {
    let (title, action) = match permission.kind {
        PermissionKind::Microphone => ("Microphone access is off", "Open Microphone settings"),
        PermissionKind::Accessibility => {
            ("Accessibility access is off", "Open Accessibility settings")
        }
    };
    ui.label(RichText::new(title).size(15.0).color(tokens.ink));
    ui.add_space(4.0);
    ui.add(
        egui::Label::new(
            RichText::new(&permission.message)
                .size(12.0)
                .color(tokens.ink_soft)
                .family(FontFamily::Proportional),
        )
        .wrap(),
    );
    if permission.kind == PermissionKind::Accessibility {
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                RichText::new(
                    "System Settings › Privacy & Security › Accessibility — add Suramak there, \
                     then come back. Copy remains available without it.",
                )
                .size(10.5)
                .color(tokens.ink_soft)
                .family(FontFamily::Proportional),
            )
            .wrap(),
        );
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if controls::button(
            ui,
            tokens,
            motion,
            "hud-open-permission",
            action,
            Tier::Primary,
            true,
        )
        .clicked()
        {
            intents.push(HudIntent::OpenSettings);
        }
        if (permission.retry_text.is_some() || permission.kind == PermissionKind::Microphone)
            && controls::button(
                ui,
                tokens,
                motion,
                "hud-retry",
                if permission.kind == PermissionKind::Microphone {
                    "Try again"
                } else {
                    "Back to result"
                },
                Tier::Secondary,
                true,
            )
            .clicked()
        {
            intents.push(HudIntent::Retry);
        }
        if permission.retry_text.is_some()
            && controls::button(
                ui,
                tokens,
                motion,
                "hud-copy-permission",
                "Copy text",
                Tier::Secondary,
                true,
            )
            .clicked()
        {
            intents.push(HudIntent::Copy);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if controls::button(
                ui,
                tokens,
                motion,
                "hud-dismiss-permission",
                "Dismiss",
                Tier::Tertiary,
                true,
            )
            .clicked()
            {
                intents.push(HudIntent::Dismiss);
            }
        });
    });
    ui.add_space(6.0);
    ui.label(
        RichText::new(format!(
            "Default chord: {}. Avoids {}.",
            suramak_taps::hotkey::DEFAULT_HOTKEY.label(),
            AVOIDED_SYSTEM_CHORDS
                .iter()
                .map(|(chord, owner)| format!("{chord} ({owner})"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .size(10.0)
        .color(tokens.ink_soft),
    );
}

/// The card's logical size as an egui size (the window uses the same constants).
pub fn card_size() -> egui::Vec2 {
    egui::vec2(HUD_WIDTH, HUD_HEIGHT)
}

/// The card's height for a state. The window follows it (the HUD stays compact:
/// a recording card is not padded out to the result card's height).
pub fn height_for(state: &HudState) -> f32 {
    match state {
        HudState::Idle => HUD_HEIGHT,
        HudState::Recording(_) => 220.0,
        HudState::Processing(_) => 176.0,
        HudState::Result(_) => 286.0,
        HudState::Cancelled(_) => 184.0,
        HudState::PermissionFailure(_) => 276.0,
    }
}

/// Reserved for the paint tests: a token surface that is known to be opaque on
/// both ladders (the card must not depend on a window material).
pub fn card_fill(tokens: &Tokens) -> Color32 {
    tokens.sheet
}

/// The stroke used around the card.
pub fn card_stroke(tokens: &Tokens) -> Stroke {
    Stroke::new(tokens.stroke.hairline, tokens.hairline)
}

/// Corner radius of the card (kept here so the paint tests assert the Deck
/// token, not a literal).
pub fn card_radius(tokens: &Tokens) -> egui::CornerRadius {
    egui::CornerRadius::same(tokens.radius.card)
}

/// A tiny helper the paint tests use to describe what state they seeded.
#[cfg(test)]
pub fn state_for_smoke(which: &str) -> Option<super::state::HudPhase> {
    Some(match which {
        "recording" => super::state::HudPhase::Recording,
        "processing" => super::state::HudPhase::Processing,
        "result" => super::state::HudPhase::Result,
        "cancelled" => super::state::HudPhase::Cancelled,
        "permission" => super::state::HudPhase::PermissionFailure,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::state::{HudChrome, HudState};
    use crate::hud::InsertionPreference;

    fn run(state: &HudState, chrome: &HudChrome) -> egui::FullOutput {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::install(&ctx, AppTheme::Dark, false);
        ctx.run(Default::default(), |ctx| {
            let intents = draw(ctx, state, chrome, &Motion::new(false));
            // The paint tests never click; any intent is ignored here.
            let _ = intents;
        })
    }

    #[test]
    fn every_state_paints_without_panicking_and_produces_shapes() {
        let chrome = HudChrome::default();
        let now = std::time::Instant::now();
        for which in [
            "recording",
            "processing",
            "result",
            "cancelled",
            "permission",
        ] {
            let mut state = HudState::idle();
            state.seed_demo(which, &chrome, now);
            let output = run(&state, &chrome);
            assert!(!output.shapes.is_empty(), "{which} painted no shapes");
            assert_eq!(
                state_for_smoke(which),
                Some(state.phase()),
                "smoke label and seeded state agree"
            );
        }
    }

    #[test]
    fn the_card_is_opaque_on_both_ladders() {
        for theme in [AppTheme::Dark, AppTheme::Light] {
            let tokens = crate::theme::tokens(theme);
            let fill = card_fill(&tokens);
            assert_eq!(
                fill.a(),
                255,
                "the HUD has no material behind it; the card must be opaque"
            );
            assert!(card_radius(&tokens).nw > 0);
        }
    }

    #[test]
    fn a_result_with_insert_primary_and_accessibility_still_paints() {
        let chrome = HudChrome::new(
            true,
            InsertionPreference::InsertPrimary,
            "⇧⌘Space".to_string(),
        );
        let now = std::time::Instant::now();
        let mut state = HudState::idle();
        state.seed_demo("result", &chrome, now);
        let output = run(&state, &chrome);
        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn the_card_size_matches_the_window_constants() {
        assert_eq!(card_size(), egui::vec2(HUD_WIDTH, HUD_HEIGHT));
    }

    #[test]
    fn every_visible_state_gets_a_compact_height_that_fits_its_content() {
        let now = std::time::Instant::now();
        let chrome = HudChrome::default();
        for (which, minimum, maximum) in [
            ("recording", 200.0, 240.0),
            ("processing", 160.0, 200.0),
            ("result", 270.0, 300.0),
            ("cancelled", 170.0, 210.0),
            ("permission", 260.0, 300.0),
        ] {
            let mut state = HudState::idle();
            state.seed_demo(which, &chrome, now);
            let height = height_for(&state);
            assert!(
                (minimum..=maximum).contains(&height),
                "{which}: {height} outside {minimum}..={maximum}"
            );
        }
        assert_eq!(height_for(&HudState::idle()), HUD_HEIGHT);
    }
}
