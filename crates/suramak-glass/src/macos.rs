//! macOS: real AppKit materials only — PLAN §1 steps 1–6.
//!
//! Executed once, on the main thread, after `winit` created the window and
//! before the first frame is presented:
//!
//! 1. `window.window_handle()` (rwh 0.6) → the AppKit handle. rwh 0.6 names the
//!    field `ns_view` (winit's content `NSView`); the `NSWindow` is that view's
//!    `window` property — PLAN §1 calls the field `ns_window`, which rwh 0.6 no
//!    longer has.
//! 2. `Retained::retain` → `Retained<NSWindow>`.
//! 3. Non-opaque window: `setOpaque(false)`, clear background color.
//! 4. Custom chrome: transparent titlebar, full-size content view, hidden title —
//!    native traffic lights stay, the egui titlebar draws under/around them.
//! 5. Material view sized to the content view's bounds:
//!    macOS 26+ → `AnyClass::get("NSGlassEffectView")` present → typed wrapper;
//!    otherwise `NSVisualEffectView` (`BehindWindow`, `UnderWindowBackground`,
//!    `Active`).
//! 6. Re-parent so the material sits BEHIND the render surface: subviews
//!    composite above their parent, so the winit view becomes a child of the
//!    material view, which becomes the window's content view.

use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSAutoresizingMaskOptions,
    NSColor, NSGlassEffectView, NSGlassEffectViewStyle, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowStyleMask,
    NSWindowTitleVisibility,
};
use objc2_foundation::NSRect;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::{GlassConfig, GlassError, GlassMode};

/// Applies PLAN §1 steps 1–6. Must run on the main thread.
pub fn apply(window: &Window, config: GlassConfig) -> Result<(), GlassError> {
    if config.mode == GlassMode::Never {
        return Ok(());
    }
    let mtm = MainThreadMarker::new().ok_or(GlassError::NotMainThread)?;

    // Steps 1–2: the rwh 0.6 AppKit handle carries winit's content NSView.
    let handle = window.window_handle().map_err(GlassError::NoWindowHandle)?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return Err(GlassError::UnsupportedHandle);
    };
    let winit_view = unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }
        .ok_or(GlassError::NoWindow)?;
    let ns_window = winit_view.window().ok_or(GlassError::NoWindow)?;

    // Steps 3–4: non-opaque window + transparent titlebar with full-size content.
    ns_window.setOpaque(false);
    ns_window.setBackgroundColor(Some(&NSColor::clearColor()));
    ns_window.setTitlebarAppearsTransparent(true);
    ns_window.setStyleMask(ns_window.styleMask() | NSWindowStyleMask::FullSizeContentView);
    ns_window.setTitleVisibility(NSWindowTitleVisibility::Hidden);

    // M1 placeholder: pin the window to DarkAqua so the static light-on-glass
    // shell reads the same whatever the user's system appearance is. Revisited
    // when the real UI lands (M2+).
    if let Some(dark) = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }) {
        ns_window.setAppearance(Some(&dark));
    }

    // Step 5: material view sized to the content view.
    let bounds = winit_view.bounds();
    let material = make_material_view(mtm, bounds);

    // Step 6: swap the content view so the material sits BEHIND winit's view.
    material.setAutoresizingMask(resize_both());
    ns_window.setContentView(Some(&material));
    material.addSubview(&winit_view);
    winit_view.setAutoresizingMask(resize_both());

    Ok(())
}

/// macOS 26+ (`NSGlassEffectView` present at runtime): the typed wrapper from
/// objc2-app-kit 0.3.2; otherwise the `NSVisualEffectView` fallback of PLAN §1.
fn make_material_view(mtm: MainThreadMarker, bounds: NSRect) -> Retained<NSView> {
    if AnyClass::get(c"NSGlassEffectView").is_some() {
        let glass = NSGlassEffectView::new(mtm);
        glass.setStyle(NSGlassEffectViewStyle::Regular);
        glass.setFrame(bounds);
        glass.into_super()
    } else {
        let effect = NSVisualEffectView::new(mtm);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
        effect.setState(NSVisualEffectState::Active);
        effect.setFrame(bounds);
        effect.into_super()
    }
}

/// Both views track the window on resize (PLAN risk 2 mitigation).
fn resize_both() -> NSAutoresizingMaskOptions {
    NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable
}
