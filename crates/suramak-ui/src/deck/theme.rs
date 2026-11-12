//! Theme tokens + typography (M9a): ONE material ladder, TWO appearances.
//!
//! Typography follows the direction contract (`.impeccable/surface-brief-body.md`)
//! with one M20A amendment: macOS keeps the system faces (SF Pro / New York /
//! SF Mono, loaded from `/System/Library/Fonts` at runtime); every other
//! platform loads the five bundled OFL statics under `assets/fonts/` (Inter
//! 400/500/600, Newsreader 400, Geist Mono 400) so the phone no longer renders
//! egui's 2019 fallbacks. See [`crate::deck::fonts`] for the table.
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

use egui::FontData;
use egui::{FontDefinitions, FontFamily};
use std::sync::Arc;

/// New York (serif) family name; always registered (falls back to proportional).
pub const SERIF_FAMILY: &str = "suramak-serif";
/// Carbon's `SansMedium`: Inter Medium 500 on Android/Windows/Linux; SF Pro on
/// macOS, whose variable system face has no static Medium instance to load.
pub const SANS_MEDIUM_FAMILY: &str = "SansMedium";
/// Carbon's `SansSemiBold`: Inter SemiBold 600 (SF Pro on macOS — see above).
pub const SANS_SEMIBOLD_FAMILY: &str = "SansSemiBold";

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
    /// The record red — the dot, the pill's stop glyph, the recording accent.
    /// A non-text indicator (WCAG's 3:1 UI-component threshold applies).
    pub record_red: Color32,
    /// M9d: the **text-safe** record red — the 12 px `recording` word and every
    /// red caption. Clears WCAG AA for small text (≥ 4.5:1) on the light
    /// ladder's worst surface (the window tone); see the contrast test.
    pub record_red_text: Color32,
    /// Text/icon ink on an accent fill (white on the light ladder's deep blue,
    /// near-black on the dark ladder's light blue).
    pub on_accent: Color32,
    /// M14e: the **ready** green — the fast-engine readiness dot ("Available",
    /// "Installed"). A non-text indicator, so WCAG's 3:1 UI-component
    /// threshold applies (it clears it on every ladder's card surface).
    pub ok: Color32,
    /// M14e: the **attention** amber — "Needs permission" on the readiness
    /// list. A non-text indicator (3:1); the word beside it carries the ink.
    pub warn: Color32,
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
    /// M14e: the **popup sheet's** fill — an OPAQUE surface, unlike `card`
    /// (the macOS ladder's card is a glass wash and let the page bleed through
    /// the dropdown; the owner's "it cluttered" report). #fff / #1c1c1e family.
    pub sheet: Color32,
    /// M14e: the sheet's backdrop scrim — the page behind a popup is dimmed so
    /// the list reads above it (egui has no blur; a dim is the honest ceiling).
    pub scrim: Color32,
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
    // ---- speaker badges/lane (M16I) ---------------------------------------
    /// The six fixed speaker hues, indexed by `speakers.color_slot`. Authored
    /// per ladder; the contrast test asserts 3:1 against the card (the bar) and
    /// 4.5:1 for ink on the 12 % chip wash (the numeral) on both ladders.
    pub speaker: [Color32; 6],
    // ---- floating record control ------------------------------------------
    /// The fallback pill's fill (M8.5; the real material stands behind it on
    /// macOS 26+).
    pub float_fill: Color32,
    // ---- scale -------------------------------------------------------------
    pub space: Space,
    pub radius: Radii,
    pub stroke: Strokes,
}

/// The token ladder for one appearance, on this platform.
pub fn tokens(theme: AppTheme) -> Tokens {
    #[cfg(target_os = "macos")]
    {
        macos_ladder(theme)
    }
    #[cfg(not(target_os = "macos"))]
    {
        desktop_ladder(theme)
    }
}

/// The macOS ladder: real material behind a transparent surface — translucent
/// washes and dark-or-light system ink. (M9a: the window is no longer pinned
/// to DarkAqua — `suramak-glass` clears/sets NSAppearance per this ladder.)
///
/// M9d: split out of [`tokens`] as a **pure, portable** function so the
/// contrast test can exercise the macOS values from any platform (the
/// platform only decides *which* ladder [`tokens`] picks).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // used by the contrast test off-macOS
pub fn macos_ladder(theme: AppTheme) -> Tokens {
    let space = SPACE;
    let radius = RADII;
    let stroke = STROKES;
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
            record_red_text: Color32::from_rgb(0xc8, 0x28, 0x1e),
            on_accent: Color32::WHITE,
            ok: Color32::from_rgb(0x2e, 0x8b, 0x45),
            warn: Color32::from_rgb(0xb4, 0x6a, 0x00),
            window: Color32::TRANSPARENT,
            panel: Color32::from_white_alpha(96),
            panel_faint: Color32::from_white_alpha(56),
            card: Color32::from_white_alpha(150),
            sheet: Color32::from_rgb(0xff, 0xff, 0xff),
            scrim: Color32::from_black_alpha(72),
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
            speaker: [
                Color32::from_rgb(0x2f, 0x6f, 0xed), // accent blue
                Color32::from_rgb(0xa8, 0x5f, 0x00), // amber
                Color32::from_rgb(0x0f, 0x76, 0x6e), // teal
                Color32::from_rgb(0x7a, 0x3e, 0x9d), // plum
                Color32::from_rgb(0x5c, 0x66, 0x00), // olive
                Color32::from_rgb(0xb3, 0x30, 0x5f), // rose
            ],
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
            record_red_text: Color32::from_rgb(0xff, 0x5a, 0x50),
            on_accent: Color32::from_rgb(0x0b, 0x12, 0x26),
            ok: Color32::from_rgb(0x54, 0xc8, 0x7a),
            warn: Color32::from_rgb(0xe0, 0xa4, 0x3c),
            window: Color32::TRANSPARENT,
            panel: Color32::from_white_alpha(20),
            panel_faint: Color32::from_white_alpha(10),
            card: Color32::from_white_alpha(32),
            sheet: Color32::from_rgb(0x1c, 0x1c, 0x1e),
            scrim: Color32::from_black_alpha(130),
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
            speaker: [
                Color32::from_rgb(0x6e, 0xa8, 0xff),
                Color32::from_rgb(0xe0, 0xa4, 0x3c),
                Color32::from_rgb(0x4f, 0xd1, 0xc5),
                Color32::from_rgb(0xc4, 0x9b, 0xff),
                Color32::from_rgb(0xb7, 0xc9, 0x5a),
                Color32::from_rgb(0xff, 0x7a, 0x9c),
            ],
            float_fill: Color32::from_white_alpha(22),
            space,
            radius,
            stroke,
        },
    }
}

/// The opaque desktop ladder (Linux/Windows — glass is a macOS-only no-op):
/// the ladder's tones are carried by the tokens themselves, both appearances.
///
/// M9d: split out of [`tokens`] like [`macos_ladder`] — pure and portable, so
/// the contrast test can exercise both ladders on any platform.
#[cfg_attr(target_os = "macos", allow(dead_code))] // used by the contrast test on macOS
pub fn desktop_ladder(theme: AppTheme) -> Tokens {
    let space = SPACE;
    let radius = RADII;
    let stroke = STROKES;
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
            record_red_text: Color32::from_rgb(0xc8, 0x28, 0x1e),
            on_accent: Color32::WHITE,
            ok: Color32::from_rgb(0x2e, 0x8b, 0x45),
            warn: Color32::from_rgb(0xb4, 0x6a, 0x00),
            window: Color32::from_rgb(0xe9, 0xeb, 0xf0),
            panel: Color32::from_rgb(0xf4, 0xf6, 0xfa),
            panel_faint: Color32::from_rgb(0xed, 0xef, 0xf4),
            card: Color32::from_rgb(0xff, 0xff, 0xff),
            sheet: Color32::from_rgb(0xff, 0xff, 0xff),
            scrim: Color32::from_black_alpha(60),
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
            speaker: [
                Color32::from_rgb(0x2f, 0x6f, 0xed), // accent blue
                Color32::from_rgb(0xa8, 0x5f, 0x00), // amber
                Color32::from_rgb(0x0f, 0x76, 0x6e), // teal
                Color32::from_rgb(0x7a, 0x3e, 0x9d), // plum
                Color32::from_rgb(0x5c, 0x66, 0x00), // olive
                Color32::from_rgb(0xb3, 0x30, 0x5f), // rose
            ],
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
            record_red_text: Color32::from_rgb(0xff, 0x5a, 0x50),
            on_accent: Color32::from_rgb(0x0b, 0x12, 0x26),
            ok: Color32::from_rgb(0x54, 0xc8, 0x7a),
            warn: Color32::from_rgb(0xe0, 0xa4, 0x3c),
            window: Color32::from_rgb(0x0e, 0x15, 0x26),
            panel: Color32::from_rgb(0x17, 0x20, 0x3a),
            panel_faint: Color32::from_rgb(0x12, 0x1a, 0x30),
            card: Color32::from_rgb(0x1a, 0x24, 0x40),
            sheet: Color32::from_rgb(0x1c, 0x1c, 0x1e),
            scrim: Color32::from_black_alpha(140),
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
            speaker: [
                Color32::from_rgb(0x6e, 0xa8, 0xff),
                Color32::from_rgb(0xe0, 0xa4, 0x3c),
                Color32::from_rgb(0x4f, 0xd1, 0xc5),
                Color32::from_rgb(0xc4, 0x9b, 0xff),
                Color32::from_rgb(0xb7, 0xc9, 0x5a),
                Color32::from_rgb(0xff, 0x7a, 0x9c),
            ],
            float_fill: Color32::from_rgba_unmultiplied(0x1e, 0x2a, 0x54, 0xd9),
            space,
            radius,
            stroke,
        },
    }
}

/// WCAG 2.x relative luminance of an sRGB colour (Rec. 709 weights on the
/// linearised channels) — the numerator/denominator of the contrast ratio.
#[allow(dead_code)] // the WCAG contrast artifact — exercised by the theme tests
pub fn relative_luminance(color: Color32) -> f32 {
    fn channel(value: u8) -> f32 {
        let c = f32::from(value) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
}

/// The WCAG 2.x contrast ratio of two opaque colours (1.0..=21.0; AA for small
/// text is ≥ 4.5:1, for large text and UI components ≥ 3:1).
#[allow(dead_code)] // the WCAG contrast artifact — exercised by the theme tests
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// `fg` composited over an opaque `bg` — the effective colour of a translucent
/// token (the macOS ladder's washes) on its surface. `Color32` stores
/// **premultiplied** alpha, so the stored channels are already multiplied by
/// the alpha; the composite is `fg + bg × (1 − α)`.
#[allow(dead_code)] // the WCAG contrast artifact — exercised by the theme tests
pub fn composite_over(fg: Color32, bg: Color32) -> Color32 {
    let alpha = f32::from(fg.a()) / 255.0;
    let over = |f: u8, b: u8| {
        (f32::from(f) + f32::from(b) * (1.0 - alpha))
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(
        over(fg.r(), bg.r()),
        over(fg.g(), bg.g()),
        over(fg.b(), bg.b()),
    )
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
        crate::deck::motion::FAST
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
    register_fonts(&mut fonts);
    ctx.set_fonts(fonts);
}

/// Which faces a registration filled (the tests read this; every field is the
/// *effective* table, not the intent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Registered {
    pub sans: bool,
    pub medium: bool,
    pub semibold: bool,
    pub serif: bool,
    pub mono: bool,
}

impl Registered {
    /// Every role resolves to a loaded face — the five bundled statics off
    /// macOS, the three system faces (plus SF Pro standing in for the weight
    /// roles) on it.
    pub fn all(&self) -> bool {
        self.sans && self.medium && self.semibold && self.serif && self.mono
    }
}

/// Applies the platform font table to `fonts`.
///
/// * **macOS** — SF Pro / New York / SF Mono from `/System/Library/Fonts`,
///   exactly as M9a did; a missing file keeps egui's stack (PLAN §1).
/// * **Android / Windows / Linux** — the five bundled statics: Inter 400 fills
///   `Proportional`, 500 and 600 fill the named weight families, Newsreader
///   fills the serif family, Geist Mono fills `Monospace`.
///
/// The weight families exist on BOTH platforms so a screen can ask for
/// `theme::sans_medium()` without a `cfg`: on macOS they resolve to SF Pro
/// (the system ships no static Medium/SemiBold face to load).
pub fn register_fonts(fonts: &mut FontDefinitions) -> Registered {
    let faces = platform_faces(fonts);
    debug_assert!(
        fonts
            .families
            .get(&FontFamily::Proportional)
            .is_some_and(|stack| !stack.is_empty()),
        "egui's default Proportional stack must be non-empty"
    );

    if let Some(id) = faces.sans {
        prepend_face(fonts, FontFamily::Proportional, id);
    }
    if let Some(id) = faces.mono {
        prepend_face(fonts, FontFamily::Monospace, id);
    }

    // The proportional stack AFTER the bundled/system sans was prepended: the
    // serif and weight families fall back through it, so a glyph the display
    // face does not carry lands on the sans of the same vintage (Inter off
    // macOS, SF Pro on it), not on egui's Ubuntu-Light.
    let proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    // The serif family always exists: its own face first when present, then
    // the proportional stack so text still renders without the serif.
    let mut serif = Vec::new();
    if let Some(id) = faces.serif {
        serif.push(id.to_owned());
    }
    serif.extend(proportional.iter().cloned());
    fonts.families.insert(serif_family(), serif);

    // The named weight families, same shape: the weight face then the
    // proportional stack for glyphs the face does not carry.
    for (family, face) in [
        (SANS_MEDIUM_FAMILY, faces.medium.or(faces.sans)),
        (SANS_SEMIBOLD_FAMILY, faces.semibold.or(faces.sans)),
    ] {
        let mut stack = Vec::new();
        if let Some(id) = face {
            stack.push(id.to_owned());
        }
        stack.extend(proportional.iter().cloned());
        fonts
            .families
            .insert(FontFamily::Name(family.into()), stack);
    }

    Registered {
        sans: faces.sans.is_some(),
        medium: faces.medium.is_some(),
        semibold: faces.semibold.is_some(),
        serif: faces.serif.is_some(),
        mono: faces.mono.is_some(),
    }
}

/// The effective face ids for this platform, after the system files are read.
struct FaceIds {
    sans: Option<&'static str>,
    medium: Option<&'static str>,
    semibold: Option<&'static str>,
    serif: Option<&'static str>,
    mono: Option<&'static str>,
}

#[cfg(target_os = "macos")]
fn platform_faces(fonts: &mut FontDefinitions) -> FaceIds {
    use crate::deck::fonts::MACOS_SYSTEM;
    let [sans, serif, mono] = MACOS_SYSTEM;
    let sans_loaded = load_face(fonts, sans.id, sans.path);
    let serif_loaded = load_face(fonts, serif.id, serif.path);
    let mono_loaded = load_face(fonts, mono.id, mono.path);
    FaceIds {
        // SF Pro is the only proportional face the system ships, so the weight
        // roles resolve to it — no static Medium/SemiBold instance exists to
        // load, and the rendering is the unchanged M9a one.
        sans: sans_loaded.then_some(sans.id),
        medium: sans_loaded.then_some(sans.id),
        semibold: sans_loaded.then_some(sans.id),
        serif: serif_loaded.then_some(serif.id),
        mono: mono_loaded.then_some(mono.id),
    }
}

#[cfg(not(target_os = "macos"))]
fn platform_faces(fonts: &mut FontDefinitions) -> FaceIds {
    use crate::deck::fonts::{
        GEIST_MONO_REGULAR, INTER_MEDIUM, INTER_REGULAR, INTER_SEMIBOLD, NEWSREADER_REGULAR,
    };
    for face in crate::deck::fonts::bundled_faces() {
        fonts.font_data.insert(
            face.id.to_owned(),
            Arc::new(FontData::from_static(face.bytes)),
        );
    }
    FaceIds {
        sans: Some(INTER_REGULAR),
        medium: Some(INTER_MEDIUM),
        semibold: Some(INTER_SEMIBOLD),
        serif: Some(NEWSREADER_REGULAR),
        mono: Some(GEIST_MONO_REGULAR),
    }
}

/// The New York family (falls back to proportional when the system face is absent).
pub fn serif_family() -> FontFamily {
    FontFamily::Name(SERIF_FAMILY.into())
}

/// `SansMedium` — Inter Medium 500 off macOS, SF Pro on it.
pub fn sans_medium() -> FontFamily {
    FontFamily::Name(SANS_MEDIUM_FAMILY.into())
}

/// `SansSemiBold` — Inter SemiBold 600 off macOS, SF Pro on it.
pub fn sans_semibold() -> FontFamily {
    FontFamily::Name(SANS_SEMIBOLD_FAMILY.into())
}

fn prepend_face(fonts: &mut FontDefinitions, family: FontFamily, face: &str) {
    fonts
        .families
        .entry(family)
        .or_default()
        .insert(0, face.to_owned());
}

/// Loads one face from an absolute system path. Returns whether it loaded;
/// absent files are skipped silently (PLAN §1).
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
            // M9d: the text-safe red is its own token, distinct from the dot red.
            assert_ne!(t.record_red, t.record_red_text);
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
        assert_eq!(ctx.style().animation_time, crate::deck::motion::FAST);
        install(&ctx, AppTheme::Dark, true);
        assert_eq!(ctx.style().animation_time, 0.0);
    }

    // ---- M20A: the platform font table -------------------------------------

    /// Every family the Deck asks for exists, is non-empty, and references
    /// only faces that were actually inserted — so `set_fonts` cannot panic on
    /// a dangling key.
    #[test]
    fn the_platform_table_fills_every_family() {
        let mut fonts = FontDefinitions::default();
        let registered = register_fonts(&mut fonts);

        assert!(!fonts.families.is_empty());
        for (family, stack) in &fonts.families {
            assert!(!stack.is_empty(), "{family:?} must not be an empty stack");
            for id in stack {
                assert!(
                    fonts.font_data.contains_key(id),
                    "{family:?} references {id}, which was never inserted"
                );
            }
        }
        // The plan's family names are exact.
        assert!(fonts.families.contains_key(&serif_family()));
        assert!(fonts.families.contains_key(&sans_medium()));
        assert!(fonts.families.contains_key(&sans_semibold()));
        assert_eq!(SANS_MEDIUM_FAMILY, "SansMedium");
        assert_eq!(SANS_SEMIBOLD_FAMILY, "SansSemiBold");

        #[cfg(not(target_os = "macos"))]
        {
            use crate::deck::fonts::{
                GEIST_MONO_REGULAR, INTER_MEDIUM, INTER_REGULAR, INTER_SEMIBOLD, NEWSREADER_REGULAR,
            };
            assert!(registered.all(), "the five bundled faces all load");
            assert_eq!(
                fonts.families[&FontFamily::Proportional].first(),
                Some(&INTER_REGULAR.to_owned())
            );
            assert_eq!(
                fonts.families[&FontFamily::Monospace].first(),
                Some(&GEIST_MONO_REGULAR.to_owned())
            );
            assert_eq!(
                fonts.families[&serif_family()].first(),
                Some(&NEWSREADER_REGULAR.to_owned())
            );
            assert_eq!(
                fonts.families[&sans_medium()].first(),
                Some(&INTER_MEDIUM.to_owned())
            );
            assert_eq!(
                fonts.families[&sans_semibold()].first(),
                Some(&INTER_SEMIBOLD.to_owned())
            );
            // egui's own fallbacks remain behind the bundled face (emoji and
            // any glyph the subset does not carry).
            assert!(
                fonts.families[&FontFamily::Proportional].len() > 1,
                "the proportional stack keeps its fallbacks"
            );
            // The display/weight families fall back to the bundled sans
            // before egui's stack — a missing serif glyph must not turn into
            // Ubuntu-Light in the middle of a title.
            assert_eq!(
                fonts.families[&serif_family()].get(1),
                Some(&INTER_REGULAR.to_owned())
            );
            assert_eq!(
                fonts.families[&sans_medium()].get(1),
                Some(&INTER_REGULAR.to_owned())
            );
        }

        #[cfg(target_os = "macos")]
        {
            use crate::deck::fonts::{NEW_YORK, SF_MONO, SF_PRO};
            assert_eq!(
                fonts.families[&FontFamily::Proportional]
                    .first()
                    .map(String::as_str),
                registered.sans.then_some(SF_PRO),
            );
            assert_eq!(
                fonts.families[&FontFamily::Monospace]
                    .first()
                    .map(String::as_str),
                registered.mono.then_some(SF_MONO),
            );
            assert_eq!(
                fonts.families[&serif_family()].first().map(String::as_str),
                registered.serif.then_some(NEW_YORK),
            );
            // No static SF weight instances exist: the weight families fall
            // back to the one proportional face, exactly the M9a rendering.
            assert_eq!(
                fonts.families[&sans_medium()].first().map(String::as_str),
                registered.sans.then_some(SF_PRO),
            );
        }
    }

    /// Width of `text` in `family` at `size`, through egui's real layout.
    fn width(ctx: &egui::Context, family: FontFamily, size: f32, text: &str) -> f32 {
        ctx.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(
                    text.to_owned(),
                    egui::FontId::new(size, family),
                    Color32::WHITE,
                )
                .size()
                .x
        })
    }

    /// The faces that render are the ones the table installed: the bundled
    /// Inter covers the punctuation and Indonesian accents, and its metrics
    /// differ from egui's baked-in Ubuntu-Light (which is the bug M20A closes).
    #[test]
    fn the_installed_faces_are_the_ones_that_render() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |_| {});

        let body = egui::FontId::new(15.5, FontFamily::Proportional);
        let covered = "éñüß’“”—…€™→";
        assert!(
            ctx.fonts_mut(|fonts| fonts.has_glyphs(&body, covered)),
            "the proportional face must carry the Deck's punctuation and accents"
        );

        #[cfg(not(target_os = "macos"))]
        {
            // Stock egui (no install_fonts) renders Ubuntu-Light. The bundled
            // Inter must beat a different width, or nothing was installed.
            let stock = egui::Context::default();
            let _ = stock.run(egui::RawInput::default(), |_| {});
            let stock_width = width(&stock, FontFamily::Proportional, 20.0, "Hamburgefonstiv");
            let inter_width = width(&ctx, FontFamily::Proportional, 20.0, "Hamburgefonstiv");
            assert!(stock_width > 0.0 && inter_width > 0.0);
            assert!(
                (stock_width - inter_width).abs() > 0.5,
                "the bundled face must replace Ubuntu-Light (stock {stock_width}, installed {inter_width})"
            );
            // The weight families resolve to the real 500/600 statics: the
            // same string is wider as the weight rises.
            let regular = width(&ctx, FontFamily::Proportional, 15.0, "Hamburgefonstiv");
            let medium = width(&ctx, sans_medium(), 15.0, "Hamburgefonstiv");
            let semibold = width(&ctx, sans_semibold(), 15.0, "Hamburgefonstiv");
            assert!(
                medium > regular && semibold > medium,
                "weights must step: {regular} < {medium} < {semibold}"
            );
            // The serif is Newsreader, not a fallback to the sans: its
            // metrics differ from the proportional face at the same size.
            let serif = width(&ctx, serif_family(), 21.0, "No transcripts yet");
            let sans = width(&ctx, FontFamily::Proportional, 21.0, "No transcripts yet");
            assert!(
                (serif - sans).abs() > 0.5,
                "the serif family must resolve to its own face ({serif} vs {sans})"
            );
        }
    }

    // ---- M20A: the WCAG contrast artifact ----------------------------

    /// The key text pairs must clear WCAG AA for small text (≥ 4.5:1) on
    /// **both** ladders (macOS translucent + desktop opaque) and both
    /// appearances. Each colour is composited over the surface it actually
    /// sits on; the translucent macOS window is composited over a documented
    /// worst-case backdrop (white in light, black in dark). The measured
    /// ratios print under `--nocapture` (the report's numbers).
    #[test]
    fn wcag_contrast_clears_the_small_text_threshold_on_both_ladders() {
        /// A named ladder builder — the test walks both.
        type Ladder = (&'static str, fn(AppTheme) -> Tokens);
        let ladders: [Ladder; 2] = [("macos", macos_ladder), ("desktop", desktop_ladder)];
        let mut measured: Vec<String> = Vec::new();
        for (ladder, build) in ladders {
            for theme in [AppTheme::Light, AppTheme::Dark] {
                let t = build(theme);
                let backdrop = match theme {
                    AppTheme::Light => Color32::WHITE,
                    AppTheme::Dark => Color32::BLACK,
                };
                let window = composite_over(t.window, backdrop);
                let panel = composite_over(t.panel, window);
                let card = composite_over(t.card, window);
                let pairs: [(&str, Color32, Color32); 9] = [
                    ("ink on window", t.ink, window),
                    ("ink on panel", t.ink, panel),
                    ("ink on card", t.ink, card),
                    ("ink_soft on window", t.ink_soft, window),
                    ("ink_soft on panel", t.ink_soft, panel),
                    ("ink_soft on card", t.ink_soft, card),
                    ("record_red_text on window", t.record_red_text, window),
                    ("record_red_text on panel", t.record_red_text, panel),
                    ("record_red_text on card", t.record_red_text, card),
                ];
                for (name, fg, bg) in pairs {
                    let ratio = contrast_ratio(fg, bg);
                    measured.push(format!("{ladder} {theme:?}: {name}: {ratio:.2}:1"));
                    assert!(
                        ratio >= 4.5,
                        "{ladder} {theme:?}: {name} = {ratio:.2}:1 — under the 4.5:1 small-text floor"
                    );
                }
                // The record dot stays a legible non-text indicator (≥ 3:1).
                let indicator = contrast_ratio(t.record_red, card);
                measured.push(format!(
                    "{ladder} {theme:?}: record_red dot on card: {indicator:.2}:1"
                ));
                assert!(
                    indicator >= 3.0,
                    "{ladder} {theme:?}: the record dot = {indicator:.2}:1 — under the 3:1 UI floor"
                );
                // M14e readiness chips: the green/amber dots are non-text
                // indicators too, and they sit on the card (the list) as well as
                // on the popup sheet — both must clear 3:1.
                for (name, color) in [("ok", t.ok), ("warn", t.warn)] {
                    for (surface, bg) in [("card", card), ("sheet", t.sheet)] {
                        let ratio = contrast_ratio(color, bg);
                        measured.push(format!(
                            "{ladder} {theme:?}: {name} dot on {surface}: {ratio:.2}:1"
                        ));
                        assert!(
                            ratio >= 3.0,
                            "{ladder} {theme:?}: the {name} dot on {surface} = {ratio:.2}:1 \
                             — under the 3:1 UI floor"
                        );
                    }
                }
            }
        }
        for line in &measured {
            println!("{line}");
        }
    }

    /// M16I: the six speaker slots clear the Deck's contrast rules on both
    /// ladders: the bar is a non-text indicator (≥ 3:1 against the card) and
    /// the badge numeral is small text on the 12 % chip wash (≥ 4.5:1). A slot
    /// that fails must not ship.
    #[test]
    fn every_speaker_slot_clears_the_badge_contrast_rules_on_both_ladders() {
        type Ladder = (&'static str, fn(AppTheme) -> Tokens);
        let ladders: [Ladder; 2] = [("macos", macos_ladder), ("desktop", desktop_ladder)];
        let mut measured: Vec<String> = Vec::new();
        for (ladder, build) in ladders {
            for theme in [AppTheme::Light, AppTheme::Dark] {
                let t = build(theme);
                let backdrop = match theme {
                    AppTheme::Light => Color32::WHITE,
                    AppTheme::Dark => Color32::BLACK,
                };
                let card = composite_over(t.card, backdrop);
                for (slot, color) in t.speaker.iter().enumerate() {
                    let bar = contrast_ratio(*color, card);
                    measured.push(format!(
                        "{ladder} {theme:?}: speaker {slot} bar on card: {bar:.2}:1"
                    ));
                    assert!(
                        bar >= 3.0,
                        "{ladder} {theme:?}: speaker slot {slot} = {bar:.2}:1 — under 3:1"
                    );
                    // The numeral sits on the 12 % chip wash over the card.
                    let chip = crate::deck::speakers::chip_fill(&t, slot as u8);
                    let wash = composite_over(chip, card);
                    let numeral = contrast_ratio(t.ink, wash);
                    measured.push(format!(
                        "{ladder} {theme:?}: speaker {slot} numeral on the 12 % wash: {numeral:.2}:1"
                    ));
                    assert!(
                        numeral >= 4.5,
                        "{ladder} {theme:?}: speaker slot {slot} numeral = {numeral:.2}:1 — under 4.5:1"
                    );
                }
                // The six slots are distinct colours (a badge must identify).
                for left in 0..6 {
                    for right in (left + 1)..6 {
                        assert_ne!(t.speaker[left], t.speaker[right]);
                    }
                }
            }
        }
        for line in &measured {
            println!("{line}");
        }
    }

    /// The measured helper itself: known WCAG anchors (black/white = 21:1,
    /// a colour against itself = 1:1, the 4.5:1 boundary of the light text
    /// red) and the compositing rule.
    #[test]
    fn contrast_math_matches_the_wcag_anchors() {
        let black = Color32::BLACK;
        let white = Color32::WHITE;
        assert!((contrast_ratio(black, white) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(black, black) - 1.0).abs() < 1e-6);
        // Order does not matter.
        let a = Color32::from_rgb(0x1c, 0x1f, 0x26);
        let b = Color32::from_rgb(0xf4, 0xf6, 0xfa);
        assert!((contrast_ratio(a, b) - contrast_ratio(b, a)).abs() < 1e-6);
        // An opaque colour composites to itself.
        assert_eq!(composite_over(a, white), a);
        // A half-transparent white over black is mid grey.
        assert_eq!(
            composite_over(Color32::from_white_alpha(128), black),
            Color32::from_rgb(128, 128, 128)
        );
        // The old light red was under the floor on the light card; the
        // text-safe variant clears it (the M9d fix, pinned here).
        let card = Color32::WHITE;
        assert!(contrast_ratio(Color32::from_rgb(0xe0, 0x35, 0x2b), card) < 4.5);
        assert!(contrast_ratio(Color32::from_rgb(0xc8, 0x28, 0x1e), card) >= 4.5);
    }
}
