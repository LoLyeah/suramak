//! Linux (and every other non-macOS/Windows target): deliberate no-op.
//!
//! Transparency is not validated on Linux by design (PLAN §1) — the UI paints
//! an opaque dark window there. This module exists so `suramak-ui` has a
//! uniform call surface across platforms: the floating record control's glass
//! (M8.5) has no real material here, and the UI paints its honest fallback
//! pill instead ([`Handle::float_glass_available`] is always `false`).

use winit::window::Window;

use crate::{FloatRect, GlassConfig, GlassError};

/// The Linux handle: no material, no floating glass — every call is a no-op.
pub struct Handle;

impl Handle {
    /// No-op: the UI's painted pill is the fallback material (M8.5).
    pub fn set_float_rect(&mut self, _rect: Option<FloatRect>) {}

    /// Always `false` — there is no system material behind the control here.
    pub fn float_glass_available(&self) -> bool {
        false
    }
}

/// Does nothing, always succeeds.
pub fn apply(_window: &Window, _config: GlassConfig) -> Result<Handle, GlassError> {
    Ok(Handle)
}
