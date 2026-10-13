//! Non-macOS stub: the crate's API compiles everywhere; system audio is unsupported.
//!
//! The Linux error path is unit-tested in `lib.rs` (`start_reports_unsupported_off_macos`).

use crate::{TapConfig, TapError};

/// Placeholder for platforms without CoreAudio taps; can never be constructed.
pub struct TapInner {
    name: String,
}

impl TapInner {
    /// Always [`TapError::Unsupported`] on this platform.
    pub fn start(_config: &TapConfig) -> Result<Self, TapError> {
        Err(TapError::Unsupported)
    }

    /// Never reached (no instance can exist).
    pub fn device_uid(&self) -> &str {
        ""
    }

    /// Never reached (no instance can exist).
    pub fn device_name(&self) -> &str {
        &self.name
    }

    /// Never reached (no instance can exist).
    pub fn device_id(&self) -> Option<u32> {
        None
    }

    /// No-op.
    pub fn stop(&mut self) -> Result<(), TapError> {
        Ok(())
    }
}
