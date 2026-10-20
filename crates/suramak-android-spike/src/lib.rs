//! suramak-android-spike — the M13b Go/No-Go harness.
//!
//! A **cdylib-only** Android app: a `GameActivity` (androidx `games-activity`)
//! hosts a winit window, wgpu draws with the Vulkan backend, and egui owns the
//! entire UI. The Kotlin side is a thin shim (`android/app/src/main/java/…`)
//! that loads this library and forwards three things over JNI: window insets,
//! back presses, and a recreate trigger (Rust -> Kotlin).
//!
//! Nothing here is shared with `suramak-ui`: the spike exists to answer four
//! questions on a real device before M13 commits to option A
//! (`docs/PLAN-M13-ANDROID.md` §2.1). The screen it draws is [`ui`].
//!
//! Build: `scripts/android/build-apk.sh` (cargo-ndk → Gradle).

pub mod ui;

#[cfg(target_os = "android")]
mod app;
#[cfg(target_os = "android")]
mod platform;

/// android-activity's entry point. Called on a dedicated thread, once per
/// Activity instance — i.e. again after a configuration-change recreation,
/// which is exactly the behaviour check (2) is about.
///
/// Note the ABI: android-activity declares this as a Rust-ABI function
/// (`fn android_main(app: AndroidApp)`), not `extern "C"` — matching its own
/// docs. `AndroidApp` is a Rust type with no C layout, so `extern "C"` here
/// would be wrong (and warns under `-D warnings`).
#[cfg(target_os = "android")]
#[no_mangle]
pub fn android_main(app: android_activity::AndroidApp) {
    app::run(app);
}
