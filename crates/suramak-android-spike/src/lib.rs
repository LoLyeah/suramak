//! suramak-android-spike — the M13b Go/No-Go harness.
//!
//! A **cdylib-only** Android app: a `GameActivity` (androidx `games-activity`)
//! hosts a winit window, wgpu draws with the Vulkan backend, and egui owns the
//! entire UI. The Kotlin side is a thin shim (`android/app/src/main/java/…`)
//! that loads this library and forwards window insets, back presses, IME text,
//! lifecycle lines and a recreate trigger over JNI (Rust -> Kotlin).
//!
//! v2 (`docs/WORKORDER-M13B2.md`) turns the spike into a self-diagnostics rig:
//! [`diag`] owns a panic hook, a crash file that survives the process and an
//! event ring; every JNI export is panic-proof ([`platform`]); the UI carries a
//! crash banner and a collapsible log panel ([`ui`]). Nothing here changes the
//! four checks' intent — it instruments them.
//!
//! v2.1 (`docs/WORKORDER-M13B3.md`) fixes the resume texture panic by tying the
//! `egui::Context` to the GPU generation ([`gpu_context`]) and makes the crash
//! files reachable from the phone (external files dir + a Kotlin share sheet).
//!
//! Nothing here is shared with `suramak-ui`: the spike exists to answer four
//! questions on a real device before M13 commits to option A
//! (`docs/PLAN-M13-ANDROID.md` §2.1).
//!
//! Build: `scripts/android/build-apk.sh` (cargo-ndk → Gradle).

pub mod diag;
pub mod gpu_context;
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
///
/// **Panic containment.** `android-activity` wraps this call in its own
/// `catch_unwind` and then calls `GameActivity_finish()` — one unwinding panic
/// and the Activity closes instantly, with the reason only in logcat (see
/// `docs/reports/M13b2-spike-v2.md` §3). The guard below records the panic and
/// keeps the process in a looper pump instead of returning, so Kotlin can put
/// the recorded text on screen over the dead surface: the failure stays
/// readable instead of the window vanishing.
#[cfg(target_os = "android")]
#[no_mangle]
pub fn android_main(app: android_activity::AndroidApp) {
    diag::init_panic_hook();
    // Each `android_main` serves one Activity instance; the generation makes a
    // "destroyed" signal from a previous Activity harmless to this one.
    let generation = platform::begin_activity();

    // `AndroidApp` is a cheap handle; keep one for the post-panic pump because
    // the event loop consumes the other.
    let salvage = app.clone();
    // SAFETY (unwind safety): nothing is reused after the panic — `app::run`
    // owns the event loop and drops it while unwinding, and the pump below only
    // touches the `AndroidApp` clone (an `Arc`-backed handle).
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app::run(app, generation)));
    if let Err(payload) = outcome {
        diag::note_contained("android_main");
        log::error!(
            "android_main panicked: {} — keeping the Activity alive for the crash overlay",
            diag::payload_text(payload.as_ref())
        );
        platform::request_crash_overlay();
        // Do NOT sleep here: Android's Java main thread blocks on lifecycle
        // callbacks once `android_main` stops polling (android-activity's docs
        // warn about exactly that ANR), so the wait is a looper pump. It exits
        // when the Activity is destroyed, including a user `finish()` from the
        // overlay's close button.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            use android_activity::{MainEvent, PollEvent};
            let mut destroyed = false;
            while !destroyed && !platform::activity_destroyed(generation) {
                salvage.poll_events(Some(std::time::Duration::from_millis(250)), |event| {
                    if let PollEvent::Main(MainEvent::Destroy) = event {
                        destroyed = true;
                    }
                });
            }
        }));
        log::info!("android_main: post-panic pump finished — returning");
    }
}
