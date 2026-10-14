//! `suramak-glass` — native window-material integration for the Suramak shell.
//!
//! | target | behaviour |
//! |---|---|
//! | macOS | real AppKit materials only: `NSGlassEffectView` (macOS 26+, runtime class check) with an `NSVisualEffectView` fallback, plus the window-chrome surgery of PLAN §1 steps 1–6 and the M8.5 floating record-control glass (a second `NSGlassEffectView` placed between the base material and the render surface) |
//! | Windows | real DWM system backdrop: `DWMWA_SYSTEMBACKDROP_TYPE` (Mica/Acrylic per [`WindowsBackdrop`]) + `DWMWA_USE_IMMERSIVE_DARK_MODE` + the sheet-of-glass frame extension — **compile-checked only** (M4; no runtime claims, see `docs/reports/M4.md`) |
//! | Linux | no-op: the UI paints an opaque dark window; transparency is not validated here |
//!
//! The crate is pure Rust with zero C-compiling dependencies, so it cross-checks
//! from Linux for `aarch64-apple-darwin` and `x86_64-pc-windows-msvc`
//! (`cargo check -p suramak-glass --target …`). It never creates windows itself.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod stub;
#[cfg(target_os = "windows")]
mod windows;

use winit::window::Window;

/// How aggressively to apply the platform material.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GlassMode {
    /// Apply the platform material when the OS provides one: `NSGlassEffectView`
    /// where the class exists at runtime, `NSVisualEffectView` below that.
    #[default]
    Auto,
    /// Apply the material even where availability heuristics would skip it.
    /// On macOS today this behaves like [`GlassMode::Auto`] (the fallback path
    /// is always installed); the distinction exists for the user-facing
    /// `ui.glass` setting and future OS gates.
    Always,
    /// Never touch the window; it stays fully opaque.
    Never,
}

/// Configuration for [`apply`].
#[derive(Clone, Copy, Debug, Default)]
pub struct GlassConfig {
    /// Which material policy to use.
    pub mode: GlassMode,
}

impl GlassConfig {
    /// Shorthand for `GlassConfig { mode }`.
    pub const fn new(mode: GlassMode) -> Self {
        Self { mode }
    }
}

/// Which appearance the window material should resolve against (M9a).
///
/// The app's `settings.ui.appearance` maps onto this: `System` clears any
/// explicit `NSAppearance` so the material follows the OS live; `Light`/`Dark`
/// pin the window AND its material view(s) so the real blur follows the forced
/// appearance too. On platforms without an AppKit appearance this is a no-op.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    /// Follow the system (no explicit appearance — the AppKit default).
    #[default]
    System,
    Light,
    Dark,
}

/// The Windows system-backdrop flavour the M4 policy selects from a [`GlassMode`].
///
/// Kept platform-independent (and unit-tested on every platform) so the
/// Mica/Acrylic policy is reviewable where the Windows runtime cannot run —
/// `windows.rs` maps each variant to its `DWM_SYSTEMBACKDROP_TYPE` value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowsBackdrop {
    /// `DWMSBT_MAINWINDOW` (2) — the standard window material (Windows 11 "Mica").
    Mica,
    /// `DWMSBT_TRANSIENTWINDOW` (3) — the stronger transient material (Acrylic).
    Acrylic,
}

impl WindowsBackdrop {
    /// Policy (M4): `Auto` → Mica, the system's calm default main-window
    /// material; `Always` → Acrylic, the stronger explicitly-requested effect.
    /// (`Never` never reaches the backdrop call — `apply` returns first.)
    pub const fn for_mode(mode: GlassMode) -> Self {
        match mode {
            GlassMode::Always => Self::Acrylic,
            GlassMode::Auto | GlassMode::Never => Self::Mica,
        }
    }
}

/// Errors produced by [`apply`]. Never produced on Linux (the stub always succeeds).
#[derive(Debug)]
pub enum GlassError {
    /// The window did not expose a raw window handle.
    NoWindowHandle(raw_window_handle::HandleError),
    /// The raw handle was not the platform handle this crate understands.
    UnsupportedHandle,
    /// The content view has no window, or retaining it failed.
    NoWindow,
    /// Called off the main thread (AppKit is main-thread-only).
    NotMainThread,
    /// `DwmSetWindowAttribute` failed with this HRESULT (Windows only).
    BackdropFailed(i32),
}

impl core::fmt::Display for GlassError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoWindowHandle(err) => write!(f, "window has no raw window handle: {err}"),
            Self::UnsupportedHandle => {
                write!(f, "raw window handle is not a supported platform handle")
            }
            Self::NoWindow => write!(
                f,
                "could not obtain the native window from the content view"
            ),
            Self::NotMainThread => write!(f, "AppKit window surgery must run on the main thread"),
            Self::BackdropFailed(hr) => {
                write!(f, "DwmSetWindowAttribute failed (HRESULT 0x{hr:08X})")
            }
        }
    }
}

impl std::error::Error for GlassError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoWindowHandle(err) => Some(err),
            _ => None,
        }
    }
}

// ---- floating record control (M8.5) ----------------------------------------

/// Inset from the content area's bottom edge to the floating control.
pub const FLOAT_INSET: f32 = 24.0;
/// The floating control pill's designed width at 1x (the impeccable comp's
/// bottom-center control).
pub const FLOAT_PILL_WIDTH: f32 = 184.0;
/// The floating control pill's designed height at 1x.
pub const FLOAT_PILL_HEIGHT: f32 = 52.0;
/// Air kept on each side when the content area is too narrow for the pill.
pub const FLOAT_SIDE_MARGIN: f32 = 16.0;

/// The floating record control's rect, in **logical points with a top-left
/// origin** — the convention egui and the winit view share on every platform.
///
/// One value, two consumers (M8.5): the egui painter draws the control's
/// pixels at this rect, and `suramak-ui` hands the same rect to
/// [`GlassHandle::set_float_rect`] so the macOS glass material sits behind
/// exactly those pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl FloatRect {
    /// Shorthand for `FloatRect { x, y, width, height }`.
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// The floating control's geometry: bottom-center of `content_size`, in
/// logical points, snapped to the device-pixel grid of `pixels_per_point` so
/// its edges stay crisp at any scale.
///
/// `content_size` is the content area's size in logical points. The pill keeps
/// its designed size wherever there is room; on a content area too small for
/// it, it shrinks (never leaving the area, never negative).
pub fn float_rect(content_size: (f32, f32), pixels_per_point: f32) -> FloatRect {
    let ppp = if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    let (content_width, content_height) = (content_size.0.max(0.0), content_size.1.max(0.0));

    // Shrink to fit: keep `FLOAT_SIDE_MARGIN` of air on both sides at minimum.
    let width = FLOAT_PILL_WIDTH.min((content_width - 2.0 * FLOAT_SIDE_MARGIN).max(0.0));
    let height = FLOAT_PILL_HEIGHT.min(content_height);
    let x = ((content_width - width) / 2.0).max(0.0);
    let y = (content_height - FLOAT_INSET - height).max(0.0);

    let snap = |value: f32| (value * ppp).round() / ppp;
    FloatRect::new(snap(x), snap(y), snap(width), snap(height))
}

/// A live handle to the applied window material, returned by [`apply`].
///
/// This is the cross-platform surface `suramak-ui` talks to: the floating
/// record control's glass is driven through [`GlassHandle::set_float_rect`],
/// and [`GlassHandle::float_glass_available`] tells the UI whether a real
/// material is behind the control (macOS 26+) or its painted fallback pill
/// should be used (Linux, Windows, older macOS). The macOS-specific view
/// surgery stays inside this crate (cfg-gated); the handle stays a clean
/// no-op everywhere else.
pub struct GlassHandle {
    #[cfg(target_os = "macos")]
    imp: macos::Handle,
    #[cfg(target_os = "windows")]
    imp: windows::Handle,
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    imp: stub::Handle,
}

impl GlassHandle {
    /// Shows and positions (`Some(rect)`) or hides (`None`) the floating
    /// record control's glass material at `rect` (logical points, top-left
    /// origin — see [`float_rect`]).
    ///
    /// A clean no-op where the platform has no real float material (Linux,
    /// Windows, macOS < 26 with the `NSVisualEffectView` fallback), and safe
    /// to call on every resize.
    pub fn set_float_rect(&mut self, rect: Option<FloatRect>) {
        self.imp.set_float_rect(rect);
    }

    /// Whether this platform actually provides the floating control's material
    /// (macOS 26+, where `NSGlassEffectView` exists at runtime). `false` is the
    /// UI's cue to paint its honest fallback pill instead.
    pub fn float_glass_available(&self) -> bool {
        self.imp.float_glass_available()
    }

    /// Sets (`Light`/`Dark`) or clears (`System`) the window's `NSAppearance`,
    /// on the window and its material container view(s) (M9a). A no-op on
    /// platforms without an AppKit appearance; the only glass edit M9a makes.
    pub fn set_appearance(&mut self, mode: ThemeMode) {
        self.imp.set_appearance(mode);
    }

    /// Installs (`Some`) or clears (`None`) the **quit veto** (M9e, P1-1): a
    /// callback AppKit consults inside `applicationShouldTerminate:` — return
    /// `true` to cancel a termination (Cmd-Q / menu Quit) so the app can show
    /// its in-app confirm first. The macOS implementation adds the method to
    /// winit's application delegate (it does not implement it; see `macos.rs`);
    /// everywhere else this is a clean no-op — there is no menu Quit to veto.
    ///
    /// The callback runs on the main thread, synchronously inside AppKit's
    /// terminate sequence; it must be cheap and must not block. Keep it pure
    /// state-reading: record a pending flag, wake the event loop, return.
    pub fn set_terminate_veto(
        &mut self,
        veto: Option<Box<dyn Fn() -> bool + Send + Sync + 'static>>,
    ) {
        self.imp.set_terminate_veto(veto);
    }
}

/// Whether the OS asks for reduced motion (macOS
/// `NSWorkspace.accessibilityDisplayShouldReduceMotion`; `false` elsewhere —
/// Linux/Windows have no equivalent wired up yet, documented in the M9a report).
///
/// Read on the main thread when the settings screen opens/applies (the app's
/// refresh points) — the property is cheap, but the UI only needs it at those
/// moments.
pub fn reduce_motion() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::reduce_motion()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Applies the native window material to `window` and returns the handle that
/// drives the floating record control's glass (M8.5).
///
/// Call once, on the main thread, after `winit` created the window and before
/// the first frame is presented (PLAN §1). Steps 7–8 of the plan (wgpu surface
/// alpha mode, translucent egui root) are not AppKit and live in `suramak-ui`.
pub fn apply(window: &Window, config: GlassConfig) -> Result<GlassHandle, GlassError> {
    #[cfg(target_os = "macos")]
    {
        macos::apply(window, config).map(|imp| GlassHandle { imp })
    }
    #[cfg(target_os = "windows")]
    {
        windows::apply(window, config).map(|imp| GlassHandle { imp })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        stub::apply(window, config).map(|imp| GlassHandle { imp })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_backdrop_policy_maps_auto_to_mica_and_always_to_acrylic() {
        assert_eq!(
            WindowsBackdrop::for_mode(GlassMode::Auto),
            WindowsBackdrop::Mica
        );
        assert_eq!(
            WindowsBackdrop::for_mode(GlassMode::Always),
            WindowsBackdrop::Acrylic
        );
        // `Never` returns before any backdrop call; the policy value is unused.
        assert_eq!(
            WindowsBackdrop::for_mode(GlassMode::Never),
            WindowsBackdrop::Mica
        );
    }

    #[test]
    fn glass_config_new_carries_the_mode() {
        assert_eq!(GlassConfig::new(GlassMode::Never).mode, GlassMode::Never);
        assert_eq!(GlassConfig::default().mode, GlassMode::Auto);
    }

    // ---- M8.5: the floating control's geometry -------------------------------

    #[test]
    fn float_rect_is_bottom_centered_with_the_designed_inset() {
        let rect = float_rect((1280.0, 800.0), 1.0);
        assert_eq!(rect, FloatRect::new(548.0, 724.0, 184.0, 52.0));
        assert_eq!(
            rect.y + rect.height + FLOAT_INSET,
            800.0,
            "the inset is measured from the bottom edge"
        );
        assert_eq!(
            rect.x + rect.width / 2.0,
            640.0,
            "horizontally centered in the content area"
        );
    }

    #[test]
    fn float_rect_keeps_the_logical_size_and_snaps_to_device_pixels() {
        // Retina (2x): the same logical rect — it is already on the pixel grid.
        assert_eq!(
            float_rect((1280.0, 800.0), 2.0),
            float_rect((1280.0, 800.0), 1.0)
        );
        // An odd content width centers at a half point; at 2x that is still a
        // whole device pixel, at 1x it snaps to a whole point.
        let at_2x = float_rect((1281.0, 800.0), 2.0);
        assert_eq!(at_2x.x, 548.5);
        assert_eq!(at_2x.x * 2.0, 1097.0, "whole device pixels at 2x");
        let at_1x = float_rect((1281.0, 800.0), 1.0);
        assert_eq!(at_1x.x, 549.0, "snapped to whole points at 1x");
    }

    #[test]
    fn float_rect_clamps_into_a_small_window() {
        // Narrow: the pill shrinks, keeping the side margin on both sides.
        let rect = float_rect((200.0, 600.0), 1.0);
        assert_eq!(rect.width, 168.0);
        assert_eq!(rect.x, FLOAT_SIDE_MARGIN);
        assert_eq!(rect.height, FLOAT_PILL_HEIGHT);
        // Short: the pill keeps the bottom inset while it fits…
        let rect = float_rect((400.0, 90.0), 1.0);
        assert_eq!(rect.y, 90.0 - FLOAT_INSET - FLOAT_PILL_HEIGHT);
        // …and hugs the top edge when even that is impossible.
        let rect = float_rect((400.0, 60.0), 1.0);
        assert_eq!(rect.y, 0.0);
    }

    #[test]
    fn float_rect_survives_degenerate_inputs() {
        let sane = float_rect((1280.0, 800.0), 1.0);
        assert_eq!(
            float_rect((1280.0, 800.0), 0.0),
            sane,
            "zero scale falls back to 1x"
        );
        assert_eq!(
            float_rect((1280.0, 800.0), f32::NAN),
            sane,
            "NaN scale falls back to 1x"
        );
        let rect = float_rect((20.0, 10.0), 1.0);
        assert!(rect.width >= 0.0 && rect.height >= 0.0);
        assert!(rect.x >= 0.0 && rect.y >= 0.0);
        let rect = float_rect((-5.0, -5.0), 1.0);
        assert!(rect.width >= 0.0 && rect.height >= 0.0);
        assert!(rect.x >= 0.0 && rect.y >= 0.0);
    }
}
