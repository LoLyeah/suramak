//! `suramak-glass` — native window-material integration for the Suramak shell.
//!
//! | target | behaviour |
//! |---|---|
//! | macOS | real AppKit materials only: `NSGlassEffectView` (macOS 26+, runtime class check) with an `NSVisualEffectView` fallback, plus the window-chrome surgery of PLAN §1 steps 1–6 |
//! | Windows | DWM system-backdrop stub — compile-checked only (real wiring lands in M4) |
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

/// Applies the native window material to `window`.
///
/// Call once, on the main thread, after `winit` created the window and before
/// the first frame is presented (PLAN §1). Steps 7–8 of the plan (wgpu surface
/// alpha mode, translucent egui root) are not AppKit and live in `suramak-ui`.
pub fn apply(window: &Window, config: GlassConfig) -> Result<(), GlassError> {
    #[cfg(target_os = "macos")]
    {
        macos::apply(window, config)
    }
    #[cfg(target_os = "windows")]
    {
        windows::apply(window, config)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        stub::apply(window, config)
    }
}
