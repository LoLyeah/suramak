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
//! 6. The material wraps the render surface: the material is the window's
//!    content view and winit's view goes inside the material's `contentView`
//!    property — the only arrangement `NSGlassEffectView.h` documents as having
//!    a z-order guarantee; content as a plain sibling above the glass does not
//!    (and aborted with `AppKit/NSGlassEffectView.swift:736: Fatal error` once
//!    the window ran modal machinery — the import open panel — on macOS 27,
//!    2026-10-13). The pre-26 `NSVisualEffectView` fallback uses the classic
//!    parent-with-child-view arrangement.

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

    // Steps 5–6: the material wraps the render surface.
    //
    // `NSGlassEffectView.contentView` is the *only* documented way to place
    // content on the material (NSGlassEffectView.h: "only guarantees the
    // contentView will be placed inside the glass effect; arbitrary subviews
    // aren't guaranteed specific behavior with regard to z-order"). A plain
    // sibling above the glass is not supported — that arrangement aborted with
    // `AppKit/NSGlassEffectView.swift:736: Fatal error` as soon as the window
    // ran modal machinery (the import open panel) on macOS 27, 2026-10-13.
    let bounds = winit_view.bounds();
    winit_view.setFrame(bounds);
    match make_material_view(mtm, bounds) {
        Material::Glass(glass) => {
            glass.setAutoresizingMask(resize_both());
            glass.setContentView(Some(&winit_view));
            ns_window.setContentView(Some(&glass));
        }
        Material::Visual(effect) => {
            effect.setAutoresizingMask(resize_both());
            effect.addSubview(&winit_view);
            winit_view.setAutoresizingMask(resize_both());
            ns_window.setContentView(Some(&effect));
        }
    }

    Ok(())
}

/// macOS 26+ (`NSGlassEffectView` present at runtime): the typed wrapper from
/// objc2-app-kit 0.3.2; otherwise the `NSVisualEffectView` fallback of PLAN §1.
enum Material {
    Glass(Retained<NSGlassEffectView>),
    Visual(Retained<NSVisualEffectView>),
}

fn make_material_view(mtm: MainThreadMarker, bounds: NSRect) -> Material {
    if AnyClass::get(c"NSGlassEffectView").is_some() {
        let glass = NSGlassEffectView::new(mtm);
        glass.setStyle(NSGlassEffectViewStyle::Regular);
        glass.setFrame(bounds);
        Material::Glass(glass)
    } else {
        let effect = NSVisualEffectView::new(mtm);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
        effect.setState(NSVisualEffectState::Active);
        effect.setFrame(bounds);
        Material::Visual(effect)
    }
}

/// Both views track the window on resize (PLAN risk 2 mitigation).
fn resize_both() -> NSAutoresizingMaskOptions {
    NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable
}
