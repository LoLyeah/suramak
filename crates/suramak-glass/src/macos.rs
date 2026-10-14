//! macOS: real AppKit materials only — PLAN §1 steps 1–6, plus the M8.5
//! floating record control's glass.
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
//!
//! M8.5 adds the floating record control's material ([`Handle::set_float_rect`]):
//! a second `NSGlassEffectView`, created lazily, that sits **between the base
//! material and the winit view** — an ordered subview directly below the render
//! surface inside a plain container (the base glass's `contentView`). It is
//! never a `contentView` itself and never an arbitrary subview of the glass
//! (the header only guarantees z-order for the `contentView` property).

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{ffi, sel, MainThreadMarker};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSColor, NSGlassEffectView, NSGlassEffectViewStyle, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowOrderingMode, NSWindowStyleMask, NSWindowTitleVisibility, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::Mutex;
use winit::window::Window;

use crate::{FloatRect, GlassConfig, GlassError, GlassMode, ThemeMode};

/// Applies PLAN §1 steps 1–6 and returns the handle that drives the floating
/// record control's glass (M8.5). Must run on the main thread.
pub fn apply(window: &Window, config: GlassConfig) -> Result<Handle, GlassError> {
    if config.mode == GlassMode::Never {
        return Ok(Handle::none());
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

    // M9a: the M1 DarkAqua pin is gone — the window keeps no explicit
    // appearance here and follows the system; `Handle::set_appearance` pins or
    // clears it live when `settings.ui.appearance` changes.

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
            // M8.5: the render surface lives in a plain container that is the
            // glass's `contentView`; the floating control's glass is ordered
            // into that same container, directly below the render surface —
            // between the base material and the winit view, never a content
            // view itself. The window's content view stays the base material.
            let host = NSView::new(mtm);
            host.setFrame(bounds);
            host.setAutoresizingMask(resize_both());
            winit_view.setAutoresizingMask(resize_both());
            host.addSubview(&winit_view);
            glass.setContentView(Some(&host));
            glass.setAutoresizingMask(resize_both());
            ns_window.setContentView(Some(&glass));
            Ok(Handle {
                placement: Some(Placement {
                    host,
                    surface: winit_view,
                }),
                float: None,
                mtm: Some(mtm),
                window: Some(ns_window),
                base: Some(BaseView::Glass(glass)),
            })
        }
        Material::Visual(effect) => {
            // Pre-26 fallback: no `NSGlassEffectView` class exists, so there is
            // no floating glass to place — the UI paints its pill (M8.5). The
            // classic parent-with-child-view arrangement is unchanged.
            effect.setAutoresizingMask(resize_both());
            effect.addSubview(&winit_view);
            winit_view.setAutoresizingMask(resize_both());
            ns_window.setContentView(Some(&effect));
            Ok(Handle {
                placement: None,
                float: None,
                mtm: Some(mtm),
                window: Some(ns_window),
                base: Some(BaseView::Visual(effect)),
            })
        }
    }
}

/// Whether macOS asks for reduced motion (`NSWorkspace` accessibility
/// preference, M9a). Must run on the main thread; `false` when it cannot be
/// read (no marker) rather than failing the caller.
pub fn reduce_motion() -> bool {
    if MainThreadMarker::new().is_none() {
        return false;
    }
    let workspace = NSWorkspace::sharedWorkspace();
    workspace.accessibilityDisplayShouldReduceMotion()
}

// ---- the quit veto (M9e, P1-1) ---------------------------------------------

/// The installed quit-veto callback (M9e): consulted inside AppKit's
/// `applicationShouldTerminate:`. `true` cancels the termination.
///
/// Process-global because the AppKit delegate method has nowhere to carry user
/// data — the delegate is winit's, and the method is added to its class (see
/// [`set_terminate_veto`]). One app, one callback.
static TERMINATE_VETO: Mutex<Option<Box<dyn Fn() -> bool + Send + Sync>>> = Mutex::new(None);

/// `applicationShouldTerminate:` — added to winit's delegate class.
///
/// AppKit calls this before the termination sequence (`applicationWillTerminate:`
/// → winit's `internal_exit` → `LoopExiting` → the app's hard exit). Returning
/// `NSTerminateCancel` (0) aborts the termination completely — the run loop
/// keeps going, which is what lets the app show its in-app confirm and answer
/// "Keep Recording" without any state to unwind. `NSTerminateNow` (1) is
/// AppKit's default when a delegate does not answer; the veto returning false
/// must reproduce exactly that.
///
/// # Safety
///
/// The signature matches `NSApplicationDelegate`'s method: `(self, _cmd,
/// sender)` returning `NSApplicationTerminateReply` (`NSUInteger`). The
/// pointers are unused; the callback only reads state.
unsafe extern "C-unwind" fn application_should_terminate(
    _this: *mut AnyObject,
    _cmd: Sel,
    _sender: *mut AnyObject,
) -> usize {
    let veto = TERMINATE_VETO
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|callback| callback()));
    match veto {
        // NSTerminateCancel — the app keeps running and shows its confirm.
        Some(true) => 0,
        // NSTerminateNow — AppKit's default answer, byte-identical to the
        // pre-M9e behaviour.
        _ => 1,
    }
}

/// Installs (`Some`) or clears (`None`) the quit veto and adds
/// `applicationShouldTerminate:` to winit's application delegate class.
///
/// winit's `WinitApplicationDelegate` implements only `applicationDidFinishLaunching:`
/// and `applicationWillTerminate:` (verified against the pinned fork), so the
/// method is *added* with `class_addMethod`; if a future winit implements it,
/// the implementation is replaced instead. The class is taken from the live
/// delegate object (no hardcoded class name), so any winit version works.
///
/// Must run on the main thread (it is called from `Shell::new`). Silently does
/// nothing when AppKit has no delegate yet — the veto is then simply never
/// consulted (the pre-M9e behaviour).
pub fn set_terminate_veto(callback: Option<Box<dyn Fn() -> bool + Send + Sync>>) {
    match TERMINATE_VETO.lock() {
        Ok(mut guard) => *guard = callback,
        Err(_) => return, // poisoned: never install a half state
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let Some(delegate) = app.delegate() else {
        return;
    };
    // SAFETY: any object pointer can be viewed as `AnyObject`; `class()` only
    // reads the object's `isa`.
    let object: &AnyObject = unsafe { &*(Retained::as_ptr(&delegate) as *const AnyObject) };
    let class = object.class();
    let selector = sel!(applicationShouldTerminate:);
    let imp = application_should_terminate
        as unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize;
    if let Some(method) = class.instance_method(selector) {
        // A winit that already implements it: replace the implementation.
        // SAFETY: the replacement has the method's exact signature and only
        // reads shared state.
        unsafe {
            method.set_implementation(std::mem::transmute::<
                unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize,
                objc2::runtime::Imp,
            >(imp));
        }
    } else {
        // The pinned winit does not implement it — add it. Types: return
        // `Q` (NSUInteger), then `@` (self), `:` (_cmd), `@` (sender).
        // SAFETY: the encoding matches the trampoline's signature exactly.
        unsafe {
            ffi::class_addMethod(
                class as *const AnyClass as *mut AnyClass,
                selector,
                std::mem::transmute::<
                    unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize,
                    objc2::runtime::Imp,
                >(imp),
                c"Q@:@".as_ptr(),
            );
        }
    }
}

/// The handle `suramak-ui` drives the floating record control through (M8.5).
pub struct Handle {
    /// Where the float glass goes and what it must render below: the container
    /// that holds the render surface. `None` when the material was not applied
    /// (`GlassMode::Never`) or this is the pre-26 fallback — `set_float_rect`
    /// is then a clean no-op and the UI's painted pill stands in.
    placement: Option<Placement>,
    /// The floating control's glass view, created lazily on the first
    /// `Some(rect)` (macOS 26+ only).
    float: Option<Retained<NSGlassEffectView>>,
    /// Main-thread token for the lazy creation; `None` in [`Handle::none`].
    mtm: Option<MainThreadMarker>,
    /// The window the appearance is set on (M9a); `None` when no material was
    /// applied at all.
    window: Option<Retained<NSWindow>>,
    /// The base material view the appearance is also set on (M9a): the glass
    /// view (macOS 26+) or the `NSVisualEffectView` fallback.
    base: Option<BaseView>,
}

/// The base material view, either flavour — only used for appearance pinning.
enum BaseView {
    Glass(Retained<NSGlassEffectView>),
    Visual(Retained<NSVisualEffectView>),
}

/// The ordered subview list the float glass is inserted into.
struct Placement {
    /// The container that holds the render surface (the base glass's
    /// `contentView` on macOS 26+).
    host: Retained<NSView>,
    /// The render surface — the float glass is ordered directly below it.
    surface: Retained<NSView>,
}

impl Handle {
    /// A handle with no float material behind it (pre-26 fallback, `Never`).
    fn none() -> Self {
        Self {
            placement: None,
            float: None,
            mtm: None,
            window: None,
            base: None,
        }
    }

    /// Sets (`Light`/`Dark`) or clears (`System`) the window's `NSAppearance`
    /// on the window, its material container and the base material view — so
    /// the real blur re-resolves with the forced appearance (M9a).
    pub fn set_appearance(&mut self, mode: ThemeMode) {
        let appearance = match mode {
            ThemeMode::System => None,
            ThemeMode::Light => NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }),
            ThemeMode::Dark => NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }),
        };
        let appearance = appearance.as_deref();
        if let Some(window) = &self.window {
            window.setAppearance(appearance);
        }
        if let Some(base) = &self.base {
            match base {
                BaseView::Glass(view) => view.setAppearance(appearance),
                BaseView::Visual(view) => view.setAppearance(appearance),
            }
        }
        if let Some(placement) = &self.placement {
            placement.host.setAppearance(appearance);
        }
    }

    /// Shows and positions (`Some`) or hides (`None`) the floating control's
    /// glass at `rect` (logical points, top-left origin — `float_rect`'s
    /// convention). Re-applied on every window resize by `suramak-ui`.
    pub fn set_float_rect(&mut self, rect: Option<FloatRect>) {
        let Some(rect) = rect else {
            // Hidden: keep the (hidden) view around so a later `Some` is cheap.
            if let Some(float) = &self.float {
                float.setHidden(true);
            }
            return;
        };
        self.ensure_float();
        let Some(float) = &self.float else {
            return; // no NSGlassEffectView on this machine — no-op
        };
        let Some(placement) = &self.placement else {
            return;
        };
        // The float rect is in top-left points; an NSView frame lives in the
        // superview's coordinate space (bottom-left origin unless flipped).
        let bounds = placement.host.bounds();
        let (x, y) = if placement.host.isFlipped() {
            (rect.x, rect.y)
        } else {
            (rect.x, bounds.size.height as f32 - rect.y - rect.height)
        };
        float.setFrame(NSRect::new(
            NSPoint::new(x as f64, y as f64),
            NSSize::new(rect.width as f64, rect.height as f64),
        ));
        // The pill's radius follows its height (a capsule).
        float.setCornerRadius((rect.height / 2.0) as f64);
        float.setHidden(false);
    }

    /// Whether a real float material can exist here: macOS 26+ (the
    /// `NSGlassEffectView` class is present at runtime) and the window
    /// material was actually applied.
    pub fn float_glass_available(&self) -> bool {
        self.placement.is_some() && AnyClass::get(c"NSGlassEffectView").is_some()
    }

    /// Installs (`Some`) or clears (`None`) the quit veto (M9e, P1-1) — see
    /// [`set_terminate_veto`], which owns the AppKit delegate-method surgery.
    pub fn set_terminate_veto(
        &mut self,
        veto: Option<Box<dyn Fn() -> bool + Send + Sync + 'static>>,
    ) {
        set_terminate_veto(veto);
    }

    /// Creates the float glass view on first use, ordered directly below the
    /// render surface (between the base material and the winit view). No-op
    /// where the class does not exist at runtime (macOS < 26).
    fn ensure_float(&mut self) {
        if self.float.is_some() {
            return;
        }
        if self.placement.is_none() || AnyClass::get(c"NSGlassEffectView").is_none() {
            return;
        }
        let Some(mtm) = self.mtm else {
            return;
        };
        let float = NSGlassEffectView::new(mtm);
        float.setStyle(NSGlassEffectViewStyle::Regular);
        float.setHidden(true);
        if let Some(placement) = &self.placement {
            placement.host.addSubview_positioned_relativeTo(
                &float,
                NSWindowOrderingMode::Below,
                Some(&placement.surface),
            );
        }
        self.float = Some(float);
    }
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
