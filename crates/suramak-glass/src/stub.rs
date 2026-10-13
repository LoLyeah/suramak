//! Linux (and every other non-macOS/Windows target): deliberate no-op.
//!
//! Transparency is not validated on Linux by design (PLAN §1) — the UI paints
//! an opaque dark window there. This module exists so `suramak-ui` has a
//! uniform call surface across platforms.

use winit::window::Window;

use crate::{GlassConfig, GlassError};

/// Does nothing, always succeeds.
pub fn apply(_window: &Window, _config: GlassConfig) -> Result<(), GlassError> {
    Ok(())
}
