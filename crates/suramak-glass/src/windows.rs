//! Windows: DWM system-backdrop stub — compile-checked only (never exercised in M1).
//!
//! The call shape is fixed now so the `windows` dependency and the HWND
//! extraction path are verified by the cross-target `cargo check`; the real
//! implementation (and any Mica/Acrylic policy) lands in M4 per PLAN §6.

use core::ffi::c_void;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
};
use winit::window::Window;

use crate::{GlassConfig, GlassError, GlassMode};

/// Requests the DWM transient (acrylic-style) backdrop for `window`.
pub fn apply(window: &Window, config: GlassConfig) -> Result<(), GlassError> {
    if config.mode == GlassMode::Never {
        return Ok(());
    }
    let handle = window.window_handle().map_err(GlassError::NoWindowHandle)?;
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return Err(GlassError::UnsupportedHandle);
    };
    let hwnd = HWND(win32.hwnd.get() as *mut c_void);

    // DWMWA_SYSTEMBACKDROP_TYPE (38) = DWMSBT_TRANSIENTWINDOW (3) — Windows 11 22H2+.
    let backdrop = DWMSBT_TRANSIENTWINDOW.0;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            core::ptr::from_ref(&backdrop).cast::<c_void>(),
            size_of_val(&backdrop) as u32,
        )
    }
    .map_err(|err| GlassError::BackdropFailed(err.code().0))
}
