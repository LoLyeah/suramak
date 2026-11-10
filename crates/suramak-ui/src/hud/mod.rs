//! **Compact HUD** (M16B) — the floating dictation card, its global hotkey and
//! the insertion adapters.
//!
//! The feature is macOS-only in the product (the hotkey is Carbon; Insert needs
//! Accessibility), but the pure parts are platform-neutral and host-tested:
//!
//! * [`state`] — the six-state machine (`idle → recording → processing →
//!   result` plus `cancelled` and `permission-failure`), every transition and
//!   the always-visible recording invariant.
//! * [`settings`] — `hud.toml` (enable, chord, insertion preference), serde +
//!   defensive load and an atomic save.
//! * [`view`] — the egui card, painted from the Deck tokens; deterministic on
//!   every platform so the Linux smoke can screenshot it.
//! * [`window`] — the borderless always-on-top window (winit + wgpu), with the
//!   macOS panel surgery in [`panel`].
//! * [`panel`] — the AppKit/Accessibility/CGEvent adapters (compile-checked for
//!   `aarch64-apple-darwin`; owner-verified at runtime).
//!
//! ## The disclosure
//!
//! Insert is **our own clipboard content pasted by a synthetic ⌘V**, not a
//! privileged text-injection channel. It requires the macOS Accessibility
//! permission; without it Copy is the primary action and the card says so. The
//! app never captures the clipboard, never inspects it, and never types
//! anything the user did not just dictate.

pub mod panel;
pub mod settings;
pub mod state;
pub mod view;
pub mod window;

use std::fmt;

pub use settings::{HudSettings, InsertionPreference};
pub use state::{HudAction, HudChrome, HudIntent, HudState};

/// The card's logical size. Wide enough for one comfortable result paragraph,
/// short enough to stay out of the way; the window is fixed-size by design (a
/// HUD that jumps around while a user watches it is worse than one that
/// scrolls its result text).
pub const HUD_WIDTH: f32 = 460.0;
/// See [`HUD_WIDTH`].
pub const HUD_HEIGHT: f32 = 286.0;
/// Distance from the top of the work area (logical points).
pub const HUD_TOP_MARGIN: f64 = 28.0;

/// Why an Insert could not be performed. The distinction matters: only
/// `PermissionDenied` moves the card to `permission-failure` and opens the
/// System Settings deep-link; a plain failure stays on the result with a note.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // macOS-only constructors
pub enum InsertError {
    /// macOS Accessibility is not granted (checked immediately before posting).
    PermissionDenied,
    /// This build/platform has no synthetic-input adapter.
    Unsupported,
    /// The CGEvent post failed (rare; the paste keystroke could not be made).
    Failed(String),
}

impl fmt::Display for InsertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InsertError::PermissionDenied => {
                f.write_str("Accessibility permission is required to insert — use Copy instead")
            }
            InsertError::Unsupported => {
                f.write_str("Insert is not available on this platform — use Copy")
            }
            InsertError::Failed(reason) => write!(f, "Insert failed: {reason}"),
        }
    }
}

impl std::error::Error for InsertError {}

/// Side effects the **frame loop** owns (not the runtime): persist the HUD
/// settings, write the clipboard, post the paste, open System Settings. The
/// app's `apply` intercepts these before `Runtime::apply` — they need the
/// AppKit main thread and the live HUD window, which the runtime does not own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HudEffect {
    /// Persist `hud.toml`, then re-register/tear down the window to match.
    ///
    /// This is the only HUD effect that travels through [`crate::app::Effect`]:
    /// the card's Copy/Insert/System-Settings actions execute directly from
    /// [`state::HudAction`] (they need the live window's egui context).
    Save(HudSettings),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_errors_explain_the_way_out() {
        assert!(InsertError::PermissionDenied
            .to_string()
            .contains("Accessibility"));
        assert!(InsertError::PermissionDenied.to_string().contains("Copy"));
        assert!(InsertError::Unsupported.to_string().contains("Copy"));
        assert!(InsertError::Failed("post rejected".to_string())
            .to_string()
            .contains("post rejected"));
    }
}
