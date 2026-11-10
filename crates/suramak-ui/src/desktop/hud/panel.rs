//! Platform adapters for the Compact HUD (M16B).
//!
//! | concern | macOS | elsewhere |
//! |---|---|---|
//! | Accessibility check | `AXIsProcessTrusted` (ApplicationServices) | always false |
//! | Copy | `NSPasteboard.general`, synchronous | egui's clipboard command |
//! | Insert | write the pasteboard, then post a synthetic ⌘V (`CGEvent`) | `Unsupported` |
//! | System Settings | `NSWorkspace.openURL` deep links | `Unsupported` |
//! | Panel behaviour | floating level, all-spaces/full-screen auxiliary, order-front-without-activation | normal borderless window |
//!
//! **The disclosure is structural**: Insert writes only the text this app just
//! transcribed, presses ⌘V, and is gated on `AXIsProcessTrusted`. There is no
//! clipboard reading and no privileged input path. A denied permission is a
//! recovery card, never a failure of the dictation.
//!
//! Everything here is main-thread AppKit. The calls arrive from the winit event
//! handler, which is the OS main thread.

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSPasteboard, NSPasteboardTypeString, NSWorkspace};
    use objc2_foundation::{NSString, NSURL};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use crate::hud::InsertError;

    // ApplicationServices umbrella framework (`HIServices`): the Accessibility
    // trust check. `Boolean` is a C `unsigned char`; return `u8` and compare.
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> u8;
    }

    // CoreGraphics: the synthetic keystroke. A keyboard event posted to the HID
    // tap is the documented AppKit-adjacent way to press a key in the focused
    // app; it requires Accessibility (checked before every post).
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreateKeyboardEvent(
            source: *mut c_void,
            virtual_key: u16,
            key_down: bool,
        ) -> *mut c_void;
        fn CGEventSetFlags(event: *mut c_void, flags: u64);
        fn CGEventPost(tap: u32, event: *mut c_void);
        fn CFRelease(cf: *const c_void);
    }

    /// `kCGHIDEventTap`.
    const K_CG_HID_EVENT_TAP: u32 = 0;
    /// `kCGEventFlagMaskCommand`.
    const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
    /// `kVK_ANSI_V` (Events.h).
    const K_VK_ANSI_V: u16 = 0x09;

    /// `NSFloatingWindowLevel`.
    const NS_FLOATING_WINDOW_LEVEL: i64 = 3;
    /// `NSWindowCollectionBehaviorCanJoinAllSpaces | Stationary |
    /// IgnoresCycle | FullScreenAuxiliary`.
    const HUD_COLLECTION_BEHAVIOR: u64 = (1 << 0) | (1 << 4) | (1 << 6) | (1 << 8);

    /// Whether the app may synthesize input. Cheap; called on the main thread
    /// when the HUD opens, when a result appears, and after returning from
    /// System Settings (the poll is once a second while a result is up).
    pub fn accessibility_granted() -> bool {
        if MainThreadMarker::new().is_none() {
            return false;
        }
        unsafe { AXIsProcessTrusted() != 0 }
    }

    /// Writes `text` to the general pasteboard. Synchronous, so the ⌘V that
    /// follows always reads exactly this text.
    pub fn copy(_ctx: &egui::Context, text: &str) -> Result<(), String> {
        let _mtm = MainThreadMarker::new()
            .ok_or_else(|| "the clipboard must be written on the main thread".to_string())?;
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        let string = NSString::from_str(text);
        // SAFETY: `NSPasteboardTypeString` is an immutable AppKit constant.
        let written = unsafe { pasteboard.setString_forType(&string, NSPasteboardTypeString) };
        if written {
            Ok(())
        } else {
            Err("the pasteboard refused the transcript".to_string())
        }
    }

    /// Writes the transcript to the clipboard and presses ⌘V in the focused app.
    ///
    /// This is **not** text injection: no unicode keyboard event, no reading of
    /// the user's clipboard — just the standard paste shortcut over content this
    /// app owns. Accessibility is checked immediately before the post.
    pub fn insert(ctx: &egui::Context, text: &str) -> Result<(), InsertError> {
        if MainThreadMarker::new().is_none() {
            return Err(InsertError::Failed(
                "insert must run on the main thread".to_string(),
            ));
        }
        if !accessibility_granted() {
            return Err(InsertError::PermissionDenied);
        }
        copy(ctx, text).map_err(InsertError::Failed)?;
        // The permission can be revoked between the check and the post; a
        // second check makes a revocation a recovery card, not a silent no-op.
        if !accessibility_granted() {
            return Err(InsertError::PermissionDenied);
        }
        post_paste();
        Ok(())
    }

    fn post_paste() {
        unsafe {
            let down = CGEventCreateKeyboardEvent(std::ptr::null_mut(), K_VK_ANSI_V, true);
            if down.is_null() {
                tracing::warn!("insert: CGEventCreateKeyboardEvent returned null");
                return;
            }
            CGEventSetFlags(down, K_CG_EVENT_FLAG_MASK_COMMAND);
            CGEventPost(K_CG_HID_EVENT_TAP, down);
            CFRelease(down as *const c_void);

            let up = CGEventCreateKeyboardEvent(std::ptr::null_mut(), K_VK_ANSI_V, false);
            if up.is_null() {
                return;
            }
            CGEventSetFlags(up, K_CG_EVENT_FLAG_MASK_COMMAND);
            CGEventPost(K_CG_HID_EVENT_TAP, up);
            CFRelease(up as *const c_void);
        }
    }

    fn open_url(url: &str) -> Result<(), String> {
        let _mtm = MainThreadMarker::new()
            .ok_or_else(|| "opening System Settings must run on the main thread".to_string())?;
        let text = NSString::from_str(url);
        let url = NSURL::URLWithString(&text)
            .ok_or_else(|| format!("invalid System Settings URL: {url}"))?;
        if NSWorkspace::sharedWorkspace().openURL(&url) {
            Ok(())
        } else {
            Err("macOS did not open the System Settings pane".to_string())
        }
    }

    /// Accessibility › Privacy & Security.
    pub fn open_accessibility_settings() -> Result<(), String> {
        open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
    }

    /// Microphone › Privacy & Security.
    pub fn open_microphone_settings() -> Result<(), String> {
        open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
    }

    /// Makes the winit window behave as a floating HUD: above normal windows,
    /// present on every Space (including over full-screen apps), never hidden by
    /// app deactivation, and **not activated by being shown**.
    ///
    /// The one thing this cannot buy without replacing winit's `NSWindow` with an
    /// `NSPanel` is click-through interaction without app activation. Showing
    /// the card never activates Suramak; clicking a button does (and must, for
    /// keyboard focus and Accessibility). See the M16b report's boundaries.
    pub fn configure_hud_window(window: &winit::window::Window) -> Result<(), String> {
        let _mtm = MainThreadMarker::new()
            .ok_or_else(|| "HUD panel configuration must run on the main thread".to_string())?;
        let ns_window = ns_window(window)?;
        unsafe {
            let _: () = msg_send![ns_window, setHidesOnDeactivate: false];
            let _: () = msg_send![ns_window, setCanHide: false];
            // winit owns the window; AppKit must not over-release it on close.
            let _: () = msg_send![ns_window, setReleasedWhenClosed: false];
            let _: () = msg_send![ns_window, setLevel: NS_FLOATING_WINDOW_LEVEL];
            let _: () = msg_send![ns_window, setCollectionBehavior: HUD_COLLECTION_BEHAVIOR];
        }
        Ok(())
    }

    /// Shows the card without making Suramak active or stealing the caret.
    /// `orderFrontRegardless` is the documented "show, do not activate" order.
    pub fn present_inactive(window: &winit::window::Window) -> Result<(), String> {
        let _mtm = MainThreadMarker::new()
            .ok_or_else(|| "HUD presentation must run on the main thread".to_string())?;
        let ns_window = ns_window(window)?;
        unsafe {
            let _: () = msg_send![ns_window, orderFrontRegardless];
        }
        window.request_redraw();
        Ok(())
    }

    /// Hides the card (the window stays alive; showing it again is cheap).
    pub fn hide(window: &winit::window::Window) {
        window.set_visible(false);
    }

    /// Brings the card forward **and activates**, for keyboard-only operation
    /// (the chord re-press on a result). This is the one path that intentionally
    /// takes focus.
    pub fn focus(window: &winit::window::Window) {
        window.focus_window();
        window.request_redraw();
    }

    /// The AppKit window behind winit's content view.
    fn ns_window(window: &winit::window::Window) -> Result<*mut AnyObject, String> {
        let handle = window
            .window_handle()
            .map_err(|err| format!("the HUD window has no native handle: {err}"))?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return Err("the HUD window is not an AppKit window".to_string());
        };
        let view = appkit.ns_view.as_ptr().cast::<AnyObject>();
        // SAFETY: winit hands us a live NSView; `-window` returns its (unowned,
        // still-live) NSWindow for the duration of the app.
        let ns_window: *mut AnyObject = unsafe { msg_send![view, window] };
        if ns_window.is_null() {
            return Err("the HUD content view has no window".to_string());
        }
        Ok(ns_window)
    }

    /// Kept so the import is used if a future path needs the shared app object
    /// (activation currently goes through winit's `focus_window`, which does the
    /// same `activateIgnoringOtherApps:` call).
    #[allow(dead_code)]
    fn shared_application() -> Option<Retained<NSApplication>> {
        MainThreadMarker::new().map(NSApplication::sharedApplication)
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    //! Honest stubs: the HUD lays out and paints (the Linux smoke proves it),
    //! but there is no Accessibility permission, no synthetic input and no
    //! AppKit deep-link. Copy uses egui's own clipboard command.

    use crate::hud::InsertError;

    pub fn accessibility_granted() -> bool {
        false
    }

    pub fn copy(ctx: &egui::Context, text: &str) -> Result<(), String> {
        // egui-winit writes this through arboard when the frame's platform
        // output is handled.
        ctx.copy_text(text.to_string());
        Ok(())
    }

    pub fn insert(_ctx: &egui::Context, _text: &str) -> Result<(), InsertError> {
        Err(InsertError::Unsupported)
    }

    pub fn open_accessibility_settings() -> Result<(), String> {
        Err("the Accessibility pane is macOS-only".to_string())
    }

    pub fn open_microphone_settings() -> Result<(), String> {
        Err("the Privacy & Security deep link is macOS-only".to_string())
    }

    pub fn configure_hud_window(_window: &winit::window::Window) -> Result<(), String> {
        Ok(())
    }

    pub fn present_inactive(window: &winit::window::Window) -> Result<(), String> {
        window.set_visible(true);
        window.focus_window();
        window.request_redraw();
        Ok(())
    }

    pub fn hide(window: &winit::window::Window) {
        window.set_visible(false);
    }

    pub fn focus(window: &winit::window::Window) {
        window.focus_window();
        window.request_redraw();
    }
}

pub use imp::{
    accessibility_granted, configure_hud_window, copy, focus, hide, insert,
    open_accessibility_settings, open_microphone_settings, present_inactive,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::InsertError;

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn copy_uses_the_egui_clipboard_and_the_platform_stubs_are_honest() {
        let ctx = egui::Context::default();
        copy(&ctx, "hello").expect("the clipboard path exists");
        assert!(!accessibility_granted());
        assert_eq!(
            insert(&egui::Context::default(), "text"),
            Err(InsertError::Unsupported)
        );
        assert!(open_accessibility_settings().is_err());
        assert!(open_microphone_settings().is_err());
    }
}
