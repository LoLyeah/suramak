//! Windows: the real DWM system-backdrop wiring — **compile-checked only** (M4).
//!
//! Same contract as the macOS path: `apply(window, config)` runs once, on the
//! main thread, after winit created the window and before the first frame. All
//! calls below are DWM (`dwmapi.dll`); the winit/wgpu half of transparency (a
//! `transparent` window attribute plus a premultiplied-alpha swapchain — PLAN §1
//! steps 7–8) lives in `suramak-ui`, exactly like the macOS split.
//!
//! ## What is set, with exact DW values (WORKORDER-M4 §2)
//!
//! | call | attribute | value |
//! |---|---|---|
//! | `DwmSetWindowAttribute` | `DWMWA_USE_IMMERSIVE_DARK_MODE` (**20**) | `TRUE` (`BOOL` = 1) — dark titlebar/frame chrome |
//! | `DwmSetWindowAttribute` | `DWMWA_SYSTEMBACKDROP_TYPE` (**38**) | `DWMSBT_MAINWINDOW` (**2**, Mica) for [`GlassMode::Auto`], `DWMSBT_TRANSIENTWINDOW` (**3**, Acrylic) for [`GlassMode::Always`] |
//! | `DwmExtendFrameIntoClientArea` | — | margins `-1` on all four sides ("sheet of glass": extend the frame over the whole client area) |
//!
//! The backdrop attribute needs Windows 11 22H2+; older builds reject it with
//! `E_INVALIDARG`, which surfaces as [`GlassError::BackdropFailed`] — the caller
//! (`suramak-ui`) keeps running with the opaque shell, same as the macOS
//! `NSVisualEffectView` fallback story. Dark mode and the frame extension are
//! best-effort (documented, never fatal).
//!
//! No runtime claims: no Windows machine has executed this path. It is verified
//! by `cargo check -p suramak-glass --target x86_64-pc-windows-msvc` from Linux
//! and by review of the call shapes above (see `docs/reports/M4.md`).
//!
//! M8.5: the floating record control gets no DWM material — the UI paints its
//! fallback pill on Windows ([`Handle::float_glass_available`] is `false`).

use core::ffi::c_void;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, TRUE};
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_MAINWINDOW, DWMSBT_TRANSIENTWINDOW,
    DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::UI::Controls::MARGINS;
use winit::window::Window;

use crate::{FloatRect, GlassConfig, GlassError, GlassMode, WindowsBackdrop};

/// The Windows handle: DWM materials only; no floating glass (the UI's painted
/// pill is the fallback — M8.5).
pub struct Handle;

impl Handle {
    /// No-op: the UI's painted pill is the fallback material (M8.5).
    pub fn set_float_rect(&mut self, _rect: Option<FloatRect>) {}

    /// Always `false` — no per-control material on this platform.
    pub fn float_glass_available(&self) -> bool {
        false
    }
}

/// Applies the DWM backdrop, immersive dark mode and frame extension to `window`.
pub fn apply(window: &Window, config: GlassConfig) -> Result<Handle, GlassError> {
    if config.mode == GlassMode::Never {
        return Ok(Handle);
    }
    let handle = window.window_handle().map_err(GlassError::NoWindowHandle)?;
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return Err(GlassError::UnsupportedHandle);
    };
    let hwnd = HWND(win32.hwnd.get() as *mut c_void);

    // Immersive dark mode (DWMWA_USE_IMMERSIVE_DARK_MODE = 20, payload BOOL = 1):
    // the system titlebar and frame chrome follow the shell's dark palette.
    // Best-effort: builds older than 20H1 reject the attribute and the window
    // simply keeps its light chrome — not worth failing the whole apply.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            core::ptr::from_ref(&TRUE).cast::<c_void>(),
            size_of_val(&TRUE) as u32,
        );
    }

    // System backdrop (DWMWA_SYSTEMBACKDROP_TYPE = 38, payload i32):
    // Mica (DWMSBT_MAINWINDOW = 2) for Auto, Acrylic (DWMSBT_TRANSIENTWINDOW = 3)
    // for Always — the policy lives in `WindowsBackdrop::for_mode`.
    let backdrop = match WindowsBackdrop::for_mode(config.mode) {
        WindowsBackdrop::Mica => DWMSBT_MAINWINDOW.0,
        WindowsBackdrop::Acrylic => DWMSBT_TRANSIENTWINDOW.0,
    };
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            core::ptr::from_ref(&backdrop).cast::<c_void>(),
            size_of_val(&backdrop) as u32,
        )
    }
    .map_err(|err| GlassError::BackdropFailed(err.code().0))?;

    // Sheet of glass: -1 on every side is the documented "extend to the whole
    // client area" value. The DWM frame (including the band the egui chrome
    // strip draws over) is composited with the backdrop, so the material shows
    // behind the transparent render surface across the full window.
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) }
        .map_err(|err| GlassError::BackdropFailed(err.code().0))?;

    Ok(Handle)
}
