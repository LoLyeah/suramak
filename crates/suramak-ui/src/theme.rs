//! Theme tokens + typography (M9a): ONE material ladder, TWO appearances.
//!
//! Typography follows the direction contract (`.impeccable/surface-brief-body.md`):
//! system faces only — SF Pro / New York / SF Mono on macOS, loaded from
//! `/System/Library/Fonts` at runtime and silently skipped when absent (no font
//! files are shipped). Everywhere else egui's defaults stand in.
//!
//! The M9a token ladder replaces the ad-hoc palette:
//!
//! * `window`  — the base: transparent on macOS (the real `NSGlassEffectView`
//!   material shows through), an opaque platform tone on Linux/Windows where
//!   no glass exists. The titlebar, sidebar and the main pane sit on it.
//! * `panel`   — ONE translucent fill: the control-deck band and popup chrome.
//! * `card`    — ONE card recipe: fill + single hairline + soft shadow.
//!
//! Everything else (ink, hairlines, accent + tint, record red, control-state
//! deltas, waveform greys, the pill fallback) is a token too, authored in BOTH
//! light and dark. `install` reworks egui's `Visuals` fully from the tokens so
//! no stock near-black (`extreme_bg_color = gray(10)`) or flat-white fill can
//! ever appear — the transcript rows in particular are transparent by design
//! (the P0-2 root cause: `text_edit_bg_color` and `extreme_bg_color` both come
//! from the ladder, never from egui's defaults).

use egui::epaint::Shadow;
use egui::{Color32, CornerRadius, Stroke, ThemePreference, Visuals};

use suramak_core::settings::Appearance;

#[cfg(target_os = "macos")]
use egui::FontData;
use egui::{FontDefinitions, FontFamily};
#[cfg(target_os = "macos")]
use std::sync::Arc;

/// New York (serif) family name; always registered (falls back to proportional).
pub const SERIF_FAMILY: &str = "suramak-serif";
const SANS_FACE: &str = "suramak-sf-pro";
const SERIF_FACE: &str = "suramak-new-york";
const MONO_FACE: &str = "suramak-sf-mono";

/// The resolved appearance the app draws with (System has been decided).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTheme {
    Light,
    Dark,
}

/// The OS-reported theme, decoupled from winit so the resolution rule is
/// unit-testable headless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemTheme {
    Light,
    Dark,
}

/// The effective theme for a setting + the OS's reported theme.
///
/// `System` follows the OS; `None` (the OS did not report one) falls back to
/// Light on Linux — the workorder's pinned rule for headless/X11 — and to Dark
/// elsewhere (the app's historic look; macOS always reports a theme in
/// practice).
pub fn effective_theme(setting: Appearance, system: Option<SystemTheme>) -> AppTheme {
    match setting {
        Appearance::Light => AppTheme::Light,
        Appearance::Dark => AppTheme::Dark,
        Appearance::System => match system {
            Some(SystemTheme::Light) => AppTheme::Light,
            Some(SystemTheme::Dark) => AppTheme::Dark,
            None if cfg!(target_os = "linux") => AppTheme::Light,
            None => AppTheme::Dark,
        },
    }
}

/// The spacing scale (workorder §1: 4/8/12/16/24).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Space {
    pub xs: f32,
    pub s: f32,
    pub m: f32,
    pub l: f32,
    pub xl: f32,
}

pub const SPACE: Space = Space {
    xs: 4.0,
    s: 8.0,
    m: 12.0,
    l: 16.0,
    xl: 24.0,
};

/// Corner radii: deck / card / control / row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Radii {
    pub deck: u8,
    pub card: u8,
    pub control: u8,
    pub row: u8,
}

pub const RADII: Radii = Radii {
    deck: 10,
    card: 12,
    control: 8,
    row: 6,
};

/// Stroke widths: the one hairline, a stronger band separator, the focus ring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strokes {
    pub hairline: f32,
    pub strong: f32,
    pub focus: f32,
}

pub const STROKES: Strokes = Strokes {
    hairline: 1.0,
    strong: 1.5,
    focus: 2.0,
};

/// The one material ladder + ink + state deltas, for one appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    // ---- ink ---------------------------------------------------------------
    /// Primary text.
    pub ink: Color32,
    /// Secondary text (captions, gutter timecodes).
    pub ink_soft: Color32,
    /// Faint text (hints, disabled labels).
    pub ink_faint: Color32,
    // ---- lines -------------------------------------------------------------
    /// The one hairline (card borders, separators, row dividers).
    pub hairline: Color32,
    /// Stronger hairline (band separators, hovered control borders).
    pub hairline_strong: Color32,
    // ---- accent ------------------------------------------------------------
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_active: Color32,
    /// The faint accent wash: active-segment tint, selection backgrounds.
    pub accent_tint: Color32,
    pub record_red: Color32,
    /// Text/icon ink on an accent fill (white on the light ladder's deep blue,
    /// near-black on the dark ladder's light blue).
    pub on_accent: Color32,
    // ---- material ladder ---------------------------------------------------
    /// Base level: transparent over real glass (macOS), opaque elsewhere.
    pub window: Color32,
    /// ONE translucent panel fill (deck band, popup chrome).
    pub panel: Color32,
    /// Faint panel-level wash (striped rows, quiet bands).
    pub panel_faint: Color32,
    /// ONE card fill (with [`Tokens::hairline`] + [`Tokens::card_shadow`]).
    pub card: Color32,
    pub card_shadow: Shadow,
    // ---- control-state deltas ---------------------------------------------
    /// Secondary control, rest.
    pub control_fill: Color32,
    /// Secondary control, hovered.
    pub control_fill_hover: Color32,
    /// Secondary control, pressed / open.
    pub control_fill_active: Color32,
    /// Tertiary/row hover wash.
    pub surface_hover: Color32,
    /// Tertiary/row pressed wash.
    pub surface_active: Color32,
    /// Selected-row / selection fill (the same as [`Tokens::accent_tint`] in
    /// the current ladder; kept as its own token so the two can diverge).
    pub selection_bg: Color32,
    /// Disabled state: everything is multiplied by this.
    pub disabled_alpha: f32,
    // ---- waveform ----------------------------------------------------------
    pub wave: Color32,
    pub wave_dim: Color32,
    // ---- floating record control ------------------------------------------
    /// The fallback pill's fill (M8.5; the real material stands behind it on
    /// macOS 26+).
    pub float_fill: Color32,
    // ---- scale -------------------------------------------------------------
    pub space: Space,
    pub radius: Radii,
    pub stroke: Strokes,
}

/// The token ladder for one appearance.
pub fn tokens(theme: AppTheme) -> Tokens {
    let space = SPACE;
    let radius = RADII;
    let stroke = STROKES;

    #[cfg(target_os = "macos")]
    {
        // Real material behind a transparent surface: translucent washes and
        // dark-or-light system ink. (M9a: the window is no longer pinned to
        // DarkAqua — `suramak-glass` clears/sets NSAppearance per this ladder.)
        match theme {
            AppTheme::Light => Tokens {
                ink: Color32::from_rgb(0x1c, 0x1f, 0x26),
                ink_soft: Color32::from_rgb(0x59, 0x61, 0x71),
                ink_faint: Color32::from_rgb(0x8a, 0x92, 0xa2),
                hairline: Color32::from_black_alpha(36),
                hairline_strong: Color32::from_black_alpha(72),
                accent: Color32::from_rgb(0x2f, 0x6f, 0xed),
                accent_hover: Color32::from_rgb(0x4a, 0x84, 0xf2),
                accent_active: Color32::from_rgb(0x24, 0x5c, 0xcf),
                accent_tint: Color32::from_rgba_unmultiplied(0x2f, 0x6f, 0xed, 44),
                record_red: Color32::from_rgb(0xe0, 0x35, 0x2b),
                on_accent: Color32::WHITE,
                window: Color32::TRANSPARENT,
                panel: Color32::from_white_alpha(96),
                panel_faint: Color32::from_white_alpha(56),
                card: Color32::from_white_alpha(150),
                card_shadow: Shadow {
                    offset: [0, 8],
                    blur: 20,
                    spread: 0,
                    color: Color32::from_black_alpha(40),
                },
                control_fill: Color32::from_white_alpha(140),
                control_fill_hover: Color32::from_white_alpha(184),
                control_fill_active: Color32::from_white_alpha(96),
                surface_hover: Color32::from_black_alpha(14),
                surface_active: Color32::from_black_alpha(26),
                selection_bg: Color32::from_rgba_unmultiplied(0x2f, 0x6f, 0xed, 44),
                disabled_alpha: 0.45,
                wave: Color32::from_rgb(0x54, 0x5c, 0x6b),
                wave_dim: Color32::from_rgb(0x8b, 0x93, 0xa3),
                float_fill: Color32::from_white_alpha(200),
                space,
                radius,
                stroke,
            },
            AppTheme::Dark => Tokens {
                ink: Color32::from_rgb(0xec, 0xed, 0xf2),
                ink_soft: Color32::from_rgb(0xa9, 0xb1, 0xc2),
                ink_faint: Color32::from_rgb(0x7e, 0x87, 0x9a),
                hairline: Color32::from_white_alpha(40),
                hairline_strong: Color32::from_white_alpha(80),
                accent: Color32::from_rgb(0x6e, 0xa8, 0xff),
                accent_hover: Color32::from_rgb(0x8a, 0xba, 0xff),
                accent_active: Color32::from_rgb(0x58, 0x90, 0xe8),
                accent_tint: Color32::from_rgba_unmultiplied(0x6e, 0xa8, 0xff, 52),
                record_red: Color32::from_rgb(0xff, 0x45, 0x3a),
                on_accent: Color32::from_rgb(0x0b, 0x12, 0x26),
                window: Color32::TRANSPARENT,
                panel: Color32::from_white_alpha(20),
                panel_faint: Color32::from_white_alpha(10),
                card: Color32::from_white_alpha(32),
                card_shadow: Shadow {
                    offset: [0, 10],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_black_alpha(120),
                },
                control_fill: Color32::from_white_alpha(24),
                control_fill_hover: Color32::from_white_alpha(40),
                control_fill_active: Color32::from_white_alpha(52),
                surface_hover: Color32::from_white_alpha(18),
                surface_active: Color32::from_white_alpha(30),
                selection_bg: Color32::from_rgba_unmultiplied(0x6e, 0xa8, 0xff, 52),
                disabled_alpha: 0.45,
                wave: Color32::from_rgb(0xae, 0xb7, 0xc5),
                wave_dim: Color32::from_rgb(0x83, 0x86, 0x8e),
                float_fill: Color32::from_white_alpha(22),
                space,
                radius,
                stroke,
            },
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Opaque window (glass is a macOS-only no-op here): the ladder's tones
        // are carried by the tokens themselves, both appearances.
        match theme {
            AppTheme::Light => Tokens {
                ink: Color32::from_rgb(0x1c, 0x1f, 0x26),
                ink_soft: Color32::from_rgb(0x59, 0x61, 0x71),
                ink_faint: Color32::from_rgb(0x8a, 0x92, 0xa2),
                hairline: Color32::from_black_alpha(30),
                hairline_strong: Color32::from_black_alpha(64),
                accent: Color32::from_rgb(0x2f, 0x6f, 0xed),
                accent_hover: Color32::from_rgb(0x4a, 0x84, 0xf2),
                accent_active: Color32::from_rgb(0x24, 0x5c, 0xcf),
                accent_tint: Color32::from_rgba_unmultiplied(0x2f, 0x6f, 0xed, 40),
                record_red: Color32::from_rgb(0xe0, 0x35, 0x2b),
                on_accent: Color32::WHITE,
                window: Color32::from_rgb(0xe9, 0xeb, 0xf0),
                panel: Color32::from_rgb(0xf4, 0xf6, 0xfa),
                panel_faint: Color32::from_rgb(0xed, 0xef, 0xf4),
                card: Color32::from_rgb(0xff, 0xff, 0xff),
                card_shadow: Shadow {
                    offset: [0, 8],
                    blur: 20,
                    spread: 0,
                    color: Color32::from_black_alpha(38),
                },
                control_fill: Color32::from_rgb(0xff, 0xff, 0xff),
                control_fill_hover: Color32::from_rgb(0xf1, 0xf4, 0xf9),
                control_fill_active: Color32::from_rgb(0xe4, 0xe8, 0xf0),
                surface_hover: Color32::from_black_alpha(12),
                surface_active: Color32::from_black_alpha(24),
                selection_bg: Color32::from_rgba_unmultiplied(0x2f, 0x6f, 0xed, 40),
                disabled_alpha: 0.45,
                wave: Color32::from_rgb(0x54, 0x5c, 0x6b),
                wave_dim: Color32::from_rgb(0x8b, 0x93, 0xa3),
                float_fill: Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 0xe8),
                space,
                radius,
                stroke,
            },
            AppTheme::Dark => Tokens {
                ink: Color32::from_rgb(0xec, 0xed, 0xf2),
                ink_soft: Color32::from_rgb(0xa9, 0xb1, 0xc2),
                ink_faint: Color32::from_rgb(0x7e, 0x87, 0x9a),
                hairline: Color32::from_white_alpha(34),
                hairline_strong: Color32::from_white_alpha(72),
                accent: Color32::from_rgb(0x6e, 0xa8, 0xff),
                accent_hover: Color32::from_rgb(0x8a, 0xba, 0xff),
                accent_active: Color32::from_rgb(0x58, 0x90, 0xe8),
                accent_tint: Color32::from_rgba_unmultiplied(0x6e, 0xa8, 0xff, 48),
                record_red: Color32::from_rgb(0xff, 0x45, 0x3a),
                on_accent: Color32::from_rgb(0x0b, 0x12, 0x26),
                window: Color32::from_rgb(0x0e, 0x15, 0x26),
                panel: Color32::from_rgb(0x17, 0x20, 0x3a),
                panel_faint: Color32::from_rgb(0x12, 0x1a, 0x30),
                card: Color32::from_rgb(0x1a, 0x24, 0x40),
                card_shadow: Shadow {
                    offset: [0, 10],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_black_alpha(140),
                },
                control_fill: Color32::from_rgb(0x1c, 0x27, 0x46),
                control_fill_hover: Color32::from_rgb(0x25, 0x33, 0x59),
                control_fill_active: Color32::from_rgb(0x2f, 0x40, 0x6b),
                surface_hover: Color32::from_white_alpha(16),
                surface_active: Color32::from_white_alpha(28),
                selection_bg: Color32::from_rgba_unmultiplied(0x6e, 0xa8, 0xff, 48),
                disabled_alpha: 0.45,
                wave: Color32::from_rgb(0xae, 0xb7, 0xc5),
                wave_dim: Color32::from_rgb(0x83, 0x86, 0x8e),
                float_fill: Color32::from_rgba_unmultiplied(0x1e, 0x2a, 0x54, 0xd9),
                space,
                radius,
                stroke,
            },
        }
    }
}

/// Reworks egui's `Visuals` from the tokens — for BOTH theme slots, so no
/// matter how egui resolves its theme preference, stock fills cannot appear.
///
/// `reduce_motion` also zeroes egui's own widget animation time, so stock
/// widget fades follow the same Reduce Motion rule as the authored motion.
pub fn install(ctx: &egui::Context, theme: AppTheme, reduce_motion: bool) {
    let tokens = tokens(theme);
    let animation_time = if reduce_motion {
        0.0
    } else {
        crate::motion::FAST
    };
    ctx.set_theme(match theme {
        AppTheme::Light => ThemePreference::Light,
        AppTheme::Dark => ThemePreference::Dark,
    });
    ctx.all_styles_mut(|style| {
        style.visuals = visuals(&tokens, theme);
        style.animation_time = animation_time;
        style.spacing.item_spacing = egui::vec2(tokens.space.s, tokens.space.s);
    });
}

/// The `Visuals` for one token ladder (pure, unit-tested).
pub fn visuals(tokens: &Tokens, theme: AppTheme) -> Visuals {
    let mut visuals = match theme {
        AppTheme::Light => Visuals::light(),
        AppTheme::Dark => Visuals::dark(),
    };

    // Panels / windows / popups: the ladder's panel + card levels.
    visuals.panel_fill = tokens.panel;
    visuals.window_fill = tokens.card;
    visuals.window_stroke = Stroke::new(tokens.stroke.hairline, tokens.hairline);
    visuals.window_corner_radius = CornerRadius::same(tokens.radius.card);
    visuals.menu_corner_radius = CornerRadius::same(tokens.radius.control);
    visuals.window_shadow = tokens.card_shadow;
    visuals.popup_shadow = tokens.card_shadow;
    visuals.faint_bg_color = tokens.panel_faint;

    // The P0-2 root cause: egui's stock `extreme_bg_color` (near-black in the
    // dark preset, flat white in the light one) must never paint anything.
    // Text edits are transparent by design; the row surface carries the state.
    visuals.extreme_bg_color = Color32::TRANSPARENT;
    visuals.text_edit_bg_color = Some(Color32::TRANSPARENT);
    visuals.code_bg_color = tokens.panel_faint;

    // Selection / accent.
    visuals.selection = egui::style::Selection {
        bg_fill: tokens.selection_bg,
        stroke: Stroke::new(tokens.stroke.hairline, tokens.accent),
    };
    visuals.hyperlink_color = tokens.accent;
    visuals.slider_trailing_fill = true;
    visuals.disabled_alpha = tokens.disabled_alpha;

    // Widget tiers: stock widgets (checkbox, slider, combo, TextEdit frame)
    // wear the ladder instead of egui's grey/teal defaults.
    let radius = CornerRadius::same(tokens.radius.control);
    visuals.widgets.noninteractive.bg_fill = tokens.panel;
    visuals.widgets.noninteractive.weak_bg_fill = tokens.panel;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(tokens.stroke.hairline, tokens.hairline);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, tokens.ink_soft);
    visuals.widgets.noninteractive.corner_radius = radius;

    visuals.widgets.inactive.bg_fill = tokens.control_fill;
    visuals.widgets.inactive.weak_bg_fill = tokens.control_fill;
    visuals.widgets.inactive.bg_stroke = Stroke::new(tokens.stroke.hairline, tokens.hairline);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, tokens.ink);
    visuals.widgets.inactive.corner_radius = radius;

    visuals.widgets.hovered.bg_fill = tokens.control_fill_hover;
    visuals.widgets.hovered.weak_bg_fill = tokens.control_fill_hover;
    visuals.widgets.hovered.bg_stroke = Stroke::new(tokens.stroke.hairline, tokens.hairline_strong);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, tokens.ink);
    visuals.widgets.hovered.corner_radius = radius;

    visuals.widgets.active.bg_fill = tokens.control_fill_active;
    visuals.widgets.active.weak_bg_fill = tokens.control_fill_active;
    visuals.widgets.active.bg_stroke = Stroke::new(tokens.stroke.hairline, tokens.hairline_strong);
    visuals.widgets.active.fg_stroke = Stroke::new(1.5_f32, tokens.ink);
    visuals.widgets.active.corner_radius = radius;

    visuals.widgets.open.bg_fill = tokens.control_fill_active;
    visuals.widgets.open.weak_bg_fill = tokens.control_fill_active;
    visuals.widgets.open.bg_stroke = Stroke::new(tokens.stroke.hairline, tokens.hairline_strong);
    visuals.widgets.open.fg_stroke = Stroke::new(1.5_f32, tokens.ink);
    visuals.widgets.open.corner_radius = radius;

    visuals
}

/// Registers the font families on the egui context (once, at startup).
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    #[cfg(target_os = "macos")]
    let (sans_loaded, serif_loaded, mono_loaded) = load_system_fonts(&mut fonts);
    #[cfg(not(target_os = "macos"))]
    let (sans_loaded, serif_loaded, mono_loaded) = (false, false, false);

    if sans_loaded {
        prepend_face(&mut fonts, FontFamily::Proportional, SANS_FACE);
    }
    if mono_loaded {
        prepend_face(&mut fonts, FontFamily::Monospace, MONO_FACE);
    }

    // The serif family always exists: New York first when present, then the
    // proportional stack so text still renders without the system face.
    let mut serif_stack: Vec<String> = Vec::new();
    if serif_loaded {
        serif_stack.push(SERIF_FACE.to_owned());
    }
    if let Some(proportional) = fonts.families.get(&FontFamily::Proportional) {
        serif_stack.extend(proportional.iter().cloned());
    }
    fonts
        .families
        .insert(FontFamily::Name(SERIF_FAMILY.into()), serif_stack);

    ctx.set_fonts(fonts);
}

/// The New York family (falls back to proportional when the system face is absent).
pub fn serif_family() -> FontFamily {
    FontFamily::Name(SERIF_FAMILY.into())
}

fn prepend_face(fonts: &mut FontDefinitions, family: FontFamily, face: &str) {
    fonts
        .families
        .entry(family)
        .or_default()
        .insert(0, face.to_owned());
}

/// Loads SF Pro / New York / SF Mono from the system font directory.
/// Returns which faces loaded; absent files are skipped silently (PLAN §1).
#[cfg(target_os = "macos")]
fn load_system_fonts(fonts: &mut FontDefinitions) -> (bool, bool, bool) {
    (
        load_face(fonts, SANS_FACE, "/System/Library/Fonts/SFNS.ttf"),
        load_face(fonts, SERIF_FACE, "/System/Library/Fonts/NewYork.ttf"),
        load_face(fonts, MONO_FACE, "/System/Library/Fonts/SFNSMono.ttf"),
    )
}

#[cfg(target_os = "macos")]
fn load_face(fonts: &mut FontDefinitions, name: &str, path: &str) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false; // absent (or unreadable) → egui defaults, silently
    };
    if !looks_like_font(&bytes) {
        tracing::warn!("{path} is not a TTF/OTF font — skipping");
        return false;
    }
    fonts
        .font_data
        .insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
    true
}

/// sfnt magic sniff: epaint panics on unparseable font bytes, so only hand it
/// files that at least start like a font.
#[cfg(target_os = "macos")]
fn looks_like_font(bytes: &[u8]) -> bool {
    matches!(
        bytes.get(..4),
        Some(
            [0x00, 0x01, 0x00, 0x00]
                | [b'O', b'T', b'T', b'O']
                | [b't', b'r', b'u', b'e']
                | [b't', b't', b'c', b'f']
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stock egui fills the ladder must never leave behind (critique P0-1/
    /// P0-2: `extreme_bg_color = gray(10)` in the dark preset).
    const STOCK_DARK_EXTREME: Color32 = Color32::from_gray(10);
    const STOCK_LIGHT_EXTREME: Color32 = Color32::WHITE;
    const STOCK_DARK_PANEL: Color32 = Color32::from_gray(27);
    const STOCK_LIGHT_PANEL: Color32 = Color32::from_gray(248);

    #[test]
    fn effective_theme_follows_the_system_and_forced_modes_override() {
        use Appearance::{Dark, Light, System};
        assert_eq!(
            effective_theme(System, Some(SystemTheme::Dark)),
            AppTheme::Dark
        );
        assert_eq!(
            effective_theme(System, Some(SystemTheme::Light)),
            AppTheme::Light
        );
        // Forced modes ignore the system entirely.
        assert_eq!(
            effective_theme(Light, Some(SystemTheme::Dark)),
            AppTheme::Light
        );
        assert_eq!(
            effective_theme(Dark, Some(SystemTheme::Light)),
            AppTheme::Dark
        );
        // An unreported system theme: Light on Linux (the pinned rule), Dark
        // elsewhere.
        let expected = if cfg!(target_os = "linux") {
            AppTheme::Light
        } else {
            AppTheme::Dark
        };
        assert_eq!(effective_theme(System, None), expected);
    }

    #[test]
    fn both_appearances_author_the_full_ladder() {
        for theme in [AppTheme::Light, AppTheme::Dark] {
            let t = tokens(theme);
            // The ladder is authored in both appearances.
            assert_ne!(t.window, t.panel, "{theme:?}: window and panel differ");
            assert_ne!(t.panel, t.card, "{theme:?}: panel and card differ");
            assert_ne!(t.ink, t.ink_soft);
            assert_ne!(t.ink_soft, t.ink_faint);
            assert!(t.space.xs == 4.0 && t.space.xl == 24.0);
            assert!(t.radius.deck > 0 && t.radius.card > 0);
            // Control-state deltas are distinct at every step.
            assert_ne!(t.control_fill, t.control_fill_hover);
            assert_ne!(t.control_fill_hover, t.control_fill_active);
            assert_ne!(t.surface_hover, t.surface_active);
            assert!(t.disabled_alpha > 0.0 && t.disabled_alpha < 1.0);
        }
    }

    #[test]
    fn installing_visuals_never_leaves_stock_fills() {
        for theme in [AppTheme::Light, AppTheme::Dark] {
            let ctx = egui::Context::default();
            install(&ctx, theme, false);
            let visuals = ctx.style().visuals.clone();
            let t = tokens(theme);

            // No stock near-black / flat-white extremes.
            assert_ne!(visuals.extreme_bg_color, STOCK_DARK_EXTREME);
            assert_ne!(visuals.extreme_bg_color, STOCK_LIGHT_EXTREME);
            assert_eq!(
                visuals.extreme_bg_color,
                Color32::TRANSPARENT,
                "{theme:?}: text-edit extremes are transparent by design"
            );
            assert_eq!(visuals.text_edit_bg_color, Some(Color32::TRANSPARENT));
            assert_ne!(visuals.panel_fill, STOCK_DARK_PANEL);
            assert_ne!(visuals.panel_fill, STOCK_LIGHT_PANEL);
            assert_eq!(visuals.panel_fill, t.panel);
            assert_eq!(visuals.window_fill, t.card);
            assert_eq!(visuals.faint_bg_color, t.panel_faint);
            assert_eq!(visuals.selection.bg_fill, t.selection_bg);
            assert_eq!(visuals.widgets.hovered.weak_bg_fill, t.control_fill_hover);
            assert_eq!(visuals.widgets.active.weak_bg_fill, t.control_fill_active);
            assert!(visuals.slider_trailing_fill);
        }
    }

    #[test]
    fn switching_the_appearance_reinstalls_different_visuals() {
        let ctx = egui::Context::default();
        install(&ctx, AppTheme::Light, false);
        let light = ctx.style().visuals.clone();
        install(&ctx, AppTheme::Dark, false);
        let dark = ctx.style().visuals.clone();
        assert_ne!(light.panel_fill, dark.panel_fill);
        assert_ne!(light.window_fill, dark.window_fill);
        assert!(dark.dark_mode && !light.dark_mode);
        // The forced light ladder really is light, the dark one really is dark.
        assert!(light.panel_fill.r() > 200);
        assert!(dark.panel_fill.r() < 200);
    }

    #[test]
    fn reduce_motion_zeroes_the_stock_widget_animation_time() {
        let ctx = egui::Context::default();
        install(&ctx, AppTheme::Dark, false);
        assert_eq!(ctx.style().animation_time, crate::motion::FAST);
        install(&ctx, AppTheme::Dark, true);
        assert_eq!(ctx.style().animation_time, 0.0);
    }
}
