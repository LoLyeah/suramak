//! Palette + typography for the M1 shell.
//!
//! Typography follows the direction contract (`.impeccable/surface-brief-body.md`):
//! system faces only — SF Pro / New York / SF Mono on macOS, loaded from
//! `/System/Library/Fonts` at runtime and silently skipped when absent (no font
//! files are shipped). Everywhere else egui's defaults stand in.
//!
//! The palette is the comp's window world: deep navy chrome/panes with light
//! system text. On macOS the panel fills are translucent white washes so the
//! real AppKit material shows through; on Linux/Windows they are the opaque
//! dark tones of the comp's window chrome and edges.

#[cfg(target_os = "macos")]
use egui::FontData;
use egui::{Color32, FontDefinitions, FontFamily};
#[cfg(target_os = "macos")]
use std::sync::Arc;

/// New York (serif) family name; always registered (falls back to proportional).
pub const SERIF_FAMILY: &str = "suramak-serif";
const SANS_FACE: &str = "suramak-sf-pro";
const SERIF_FACE: &str = "suramak-new-york";
const MONO_FACE: &str = "suramak-sf-mono";

/// The comp-derived palette (see `.impeccable/build/spec.json` for the source hexes).
pub struct Palette {
    /// Titlebar strip fill (comp window-chrome `#0d1b42` on opaque targets).
    pub chrome: Color32,
    /// Sidebar fill.
    pub sidebar: Color32,
    /// Main pane fill.
    pub pane: Color32,
    /// Waveform scrubber strip fill.
    pub card: Color32,
    /// Primary text (comp `#e6e7eb` / `#eaeaf2`).
    pub text: Color32,
    /// Secondary text (comp `#9fa6bb`).
    pub text_dim: Color32,
    /// Accent: playhead + selected row.
    pub accent: Color32,
    /// Waveform bars (comp scrubber `#aeb7c5`).
    pub wave: Color32,
    /// Dimmed waveform bars (comp scrubber `#83868e`).
    pub wave_dim: Color32,
    /// Hairlines / separators.
    pub hairline: Color32,
    /// Selected sidebar row background.
    pub row_selected: Color32,
    /// Record control (system red).
    pub record_red: Color32,
}

/// The palette for this platform.
pub fn palette() -> Palette {
    #[cfg(target_os = "macos")]
    {
        // Real material behind a transparent surface: translucent white washes,
        // light system text. (The window is pinned to DarkAqua in `suramak-glass`.)
        Palette {
            chrome: Color32::from_white_alpha(6),
            sidebar: Color32::from_white_alpha(12),
            pane: Color32::from_white_alpha(18),
            card: Color32::from_white_alpha(14),
            text: Color32::from_rgb(0xea, 0xea, 0xf2),
            text_dim: Color32::from_rgb(0x9f, 0xa6, 0xbb),
            accent: Color32::from_rgb(0x6e, 0xa8, 0xff),
            wave: Color32::from_rgb(0xae, 0xb7, 0xc5),
            wave_dim: Color32::from_rgb(0x83, 0x86, 0x8e),
            hairline: Color32::from_white_alpha(26),
            row_selected: Color32::from_rgba_unmultiplied(0x6e, 0xa8, 0xff, 42),
            record_red: Color32::from_rgb(0xff, 0x45, 0x3a),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Opaque dark window (glass is a macOS-only no-op here): comp window tones.
        Palette {
            chrome: Color32::from_rgb(0x0d, 0x1b, 0x42),
            sidebar: Color32::from_rgb(0x10, 0x1a, 0x3d),
            pane: Color32::from_rgb(0x0b, 0x12, 0x28),
            card: Color32::from_rgb(0x13, 0x1d, 0x41),
            text: Color32::from_rgb(0xea, 0xea, 0xf2),
            text_dim: Color32::from_rgb(0x9f, 0xa6, 0xbb),
            accent: Color32::from_rgb(0x6e, 0xa8, 0xff),
            wave: Color32::from_rgb(0xae, 0xb7, 0xc5),
            wave_dim: Color32::from_rgb(0x83, 0x86, 0x8e),
            hairline: Color32::from_rgba_unmultiplied(0xea, 0xea, 0xf2, 26),
            row_selected: Color32::from_rgba_unmultiplied(0x6e, 0xa8, 0xff, 42),
            record_red: Color32::from_rgb(0xff, 0x45, 0x3a),
        }
    }
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
