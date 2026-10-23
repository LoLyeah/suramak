//! The JNI surface between the Kotlin shim and the Rust UI thread.
//!
//! Kotlin → Rust (exported here, called from `MainActivity`):
//!   * `nativeInit(filesDir)` — where the crash files live (the app's external
//!     files dir since v2.1); first call wins.
//!   * `nativeSetInsets(top, bottom, left, right, imeBottom)` — window insets in px;
//!   * `nativeBackPressed()` — a back gesture/button was intercepted;
//!   * `nativeImeText(text, selectionStart, selectionEnd)` — the soft keyboard's
//!     editing state from `GameActivity.stateChanged`;
//!   * `nativeLogLine(line)` — a plain lifecycle line (onCreate, surfaceCreated,
//!     `Activity.recreate()` …) for the on-screen event ring;
//!   * `nativeActivityDestroyed()` — the Activity is going away, stop the loop.
//!
//! Rust → Kotlin: [`request_activity_recreate`] calls `MainActivity.postRecreate()`
//! over JNI, which posts `Activity.recreate()` onto the UI thread.
//!
//! **Every export is panic-proof.** A panic that unwinds out of an `extern
//! "system"` function is undefined behaviour, and Rust ≥1.81 aborts the process
//! instead — which on Android looks exactly like "the app closed itself". Each
//! body therefore runs inside `catch_unwind`; a panic is recorded in the crash
//! buffer (`diag`) and the call returns normally. `AssertUnwindSafe` is sound
//! here because the only state these closures touch is a `Mutex`-guarded global
//! whose lock is poison-tolerant: a half-applied update can be a stale count or
//! a lost inset push, never a broken invariant, and the next callback overwrites
//! it. All state still crosses threads through one `Mutex<PlatformEvents>`; the
//! JNI calls arrive on the Android UI thread, the UI polls them each frame.
//!
//! Bad input is defensive, never fatal: out-of-range insets are clamped and
//! logged, null/oversized strings are rejected or truncated, and every negative
//! selection index is treated as 0.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use crate::diag;

/// Insets beyond this are not a phone: clamp rather than trust the caller.
/// 100 000 px is far past any real panel, so a hit means a broken shim.
const MAX_INSET_PX: i32 = 100_000;

/// `(top, bottom, left, right, ime_bottom)` in physical pixels.
pub type InsetPx = (i32, i32, i32, i32, i32);

/// Ceiling for a single IME buffer. Typing never reaches this; a paste of a
/// whole document might, and the spike only needs enough to judge the bridge.
const MAX_IME_BYTES: usize = 256 * 1024;

/// How much of the IME text is echoed into the event ring.
const IME_PREVIEW_CHARS: usize = 32;

/// Events queued by the Kotlin shim, drained by the UI thread once per frame.
#[derive(Debug, Default)]
struct PlatformEvents {
    /// `(top, bottom, left, right, ime_bottom)` in physical pixels.
    insets: Option<InsetPx>,
    back_presses: u32,
}

/// One IME editing state, as reported by `MainActivity.stateChanged`.
#[derive(Debug, Clone)]
pub struct ImeState {
    pub text: String,
    /// Caret/selection converted from Java UTF-16 indices to char indices.
    pub selection_start: usize,
    pub selection_end: usize,
}

static EVENTS: Mutex<PlatformEvents> = Mutex::new(PlatformEvents {
    insets: None,
    back_presses: 0,
});

/// Latest IME state; only the newest matters (the IME sends full buffers).
static IME_TEXT: Mutex<Option<ImeState>> = Mutex::new(None);

/// `surfaceDestroyed` arrived from the Activity and no one has acted on it yet.
///
/// The Java callback runs before android-activity's looper processes
/// `APP_CMD_TERM_WINDOW`, and winit only turns *that* into `Event::Suspended`.
/// In the owner's v2.1 trace the gap was 80 ms (`3.32s surfaceDestroyed` →
/// `3.40s suspended`) and the raced gpu init lived inside it. This flag lets the
/// event loop close its gate on the Java signal instead of waiting for winit,
/// without changing the Kotlin shim: it is set from the `nativeLogLine` the shim
/// already sends.
static SURFACE_DESTROYED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Take the pending `surfaceDestroyed` signal (edge-triggered).
pub fn take_surface_destroyed_hint() -> bool {
    SURFACE_DESTROYED.swap(false, Ordering::SeqCst)
}

/// Which Activity instance this process is serving. Bumped on every
/// `android_main` entry, so a "destroyed" flag left by a *previous* Activity can
/// never stop the next one — the flag records which generation was destroyed,
/// not merely that one was.
static CURRENT_GENERATION: AtomicU32 = AtomicU32::new(0);
static DESTROYED_GENERATION: AtomicU32 = AtomicU32::new(0);

fn events() -> std::sync::MutexGuard<'static, PlatformEvents> {
    EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Take everything the shim reported since the last call.
pub fn drain() -> (Option<InsetPx>, u32) {
    let mut events = events();
    (
        events.insets.take(),
        std::mem::take(&mut events.back_presses),
    )
}

/// Take the newest IME editing state, if the IME reported one since the last
/// call. `None` means "nothing changed".
pub fn take_ime_text() -> Option<ImeState> {
    IME_TEXT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Claim the next Activity generation. One call per `android_main` entry.
pub fn begin_activity() -> u32 {
    CURRENT_GENERATION.fetch_add(1, Ordering::SeqCst) + 1
}

/// Was **this** generation's Activity destroyed? `android-activity` documents
/// that `android_main` must return promptly once the Activity is destroyed
/// (`android-activity-0.6.1/src/lib.rs`, "No associated Activity after
/// MainEvent::Destroy") — and winit 0.30.13 never forwards that event
/// (`MainEvent::Destroy => warn!("TODO: forward onDestroy notification")`), so
/// `MainActivity.onDestroy` has to tell us or the loop outlives the Activity.
pub fn activity_destroyed(generation: u32) -> bool {
    DESTROYED_GENERATION.load(Ordering::SeqCst) >= generation
}

fn mark_activity_destroyed() {
    let generation = CURRENT_GENERATION.load(Ordering::SeqCst);
    let previous = DESTROYED_GENERATION.swap(generation, Ordering::SeqCst);
    if previous < generation {
        diag::info(format!(
            "jni: nativeActivityDestroyed (activity generation {generation}) — the event loop will exit"
        ));
    }
}

/// Run one JNI export body so a panic can never cross the ABI boundary.
fn jni_guard(name: &'static str, body: impl FnOnce()) {
    // Belt and braces: `nativeInit` installs the hook, but a JNI call that
    // somehow arrives first must still be recorded.
    diag::init_panic_hook();
    // SAFETY (unwind safety): see the module docs — the closures only touch
    // poison-tolerant `Mutex` globals, so an abandoned update is recoverable.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    if let Err(payload) = result {
        // The hook already recorded message + location; this marks *where* it
        // was contained, i.e. that the process survived.
        diag::note_contained(name);
        log::error!(
            "panic contained in JNI export {name}: {}",
            diag::payload_text(payload.as_ref())
        );
    }
}

fn read_jstring(
    env: *mut jni::sys::JNIEnv,
    value: jni::sys::jstring,
    what: &str,
) -> Option<String> {
    if env.is_null() || value.is_null() {
        diag::warn(format!(
            "jni: {what} called with a null env/string — ignored"
        ));
        return None;
    }
    // SAFETY: the JVM passes a valid JNIEnv and a valid local jstring for the
    // duration of this call; both are only used before we return.
    let Ok(mut env) = (unsafe { jni::JNIEnv::from_raw(env) }) else {
        diag::warn(format!("jni: {what}: JNIEnv::from_raw failed"));
        return None;
    };
    let jtext = unsafe { jni::objects::JString::from_raw(value) };
    // Bind first: `JavaStr` borrows `jtext`, so returning it from a tail
    // expression would drop `jtext` while still borrowed.
    let text = match env.get_string(&jtext) {
        Ok(raw) => Some(String::from(raw)),
        Err(err) => {
            diag::warn(format!("jni: {what}: get_string failed: {err}"));
            None
        }
    };
    text
}

fn clamp_inset(name: &str, value: i32) -> i32 {
    if (0..=MAX_INSET_PX).contains(&value) {
        return value;
    }
    let clamped = value.clamp(0, MAX_INSET_PX);
    diag::warn(format!(
        "jni: nativeSetInsets {name}={value} out of range — clamped to {clamped}"
    ));
    clamped
}

/// `MainActivity.nativeInit(filesDir)` — called before `super.onCreate`, i.e.
/// before `android_main` exists, so the panic hook is armed as early as the
/// library allows and the persisted crash record is loaded for the banner.
#[no_mangle]
pub extern "system" fn Java_app_suramak_spike_MainActivity_nativeInit(
    env: *mut jni::sys::JNIEnv,
    _this: jni::sys::jobject,
    files_dir: jni::sys::jstring,
) {
    jni_guard("nativeInit", || {
        diag::init_panic_hook();
        if let Some(dir) = read_jstring(env, files_dir, "nativeInit") {
            if dir.is_empty() {
                diag::warn("jni: nativeInit got an empty files dir — crash persistence off");
            } else {
                diag::set_files_dir(&dir);
                diag::info(format!("jni: nativeInit files dir = {dir}"));
            }
        }
    });
}

/// `MainActivity.nativeLogLine(line)` — one-way lifecycle logging for the ring.
#[no_mangle]
pub extern "system" fn Java_app_suramak_spike_MainActivity_nativeLogLine(
    env: *mut jni::sys::JNIEnv,
    _this: jni::sys::jobject,
    line: jni::sys::jstring,
) {
    jni_guard("nativeLogLine", || {
        if let Some(text) = read_jstring(env, line, "nativeLogLine") {
            diag::info(format!("kotlin: {text}"));
            // The surface half of the suspend, ~1 event-loop turn before
            // winit's `Suspended` (see `SURFACE_DESTROYED`). A log line is the
            // only channel the shim already has for it, and adding a JNI export
            // would mean touching Kotlin, which this workorder does not.
            if text == "surfaceDestroyed" {
                SURFACE_DESTROYED.store(true, Ordering::SeqCst);
            }
        }
    });
}

/// `MainActivity.nativeActivityDestroyed()` — the Activity is finishing or being
/// recreated; stop the event loop so `android_main` returns and android-activity
/// can drop the `AndroidApp` (its JNI global ref) instead of leaking a spinning
/// loop per recreation.
#[no_mangle]
pub extern "system" fn Java_app_suramak_spike_MainActivity_nativeActivityDestroyed(
    _env: *mut jni::sys::JNIEnv,
    _this: jni::sys::jobject,
) {
    jni_guard("nativeActivityDestroyed", || {
        mark_activity_destroyed();
    });
}

/// `MainActivity.nativeSetInsets(...)` — the JNI name is derived from the Kotlin
/// package (`app.suramak.spike`), class and method name; keep the three in sync.
#[no_mangle]
pub extern "system" fn Java_app_suramak_spike_MainActivity_nativeSetInsets(
    _env: *mut jni::sys::JNIEnv,
    _this: jni::sys::jobject,
    top: jni::sys::jint,
    bottom: jni::sys::jint,
    left: jni::sys::jint,
    right: jni::sys::jint,
    ime_bottom: jni::sys::jint,
) {
    jni_guard("nativeSetInsets", || {
        let top = clamp_inset("top", top);
        let bottom = clamp_inset("bottom", bottom);
        let left = clamp_inset("left", left);
        let right = clamp_inset("right", right);
        let ime_bottom = clamp_inset("ime_bottom", ime_bottom);
        diag::info(format!(
            "jni: nativeSetInsets(top={top}, bottom={bottom}, left={left}, right={right}, ime={ime_bottom})"
        ));
        events().insets = Some((top, bottom, left, right, ime_bottom));
    });
}

/// `MainActivity.nativeBackPressed()`.
#[no_mangle]
pub extern "system" fn Java_app_suramak_spike_MainActivity_nativeBackPressed(
    _env: *mut jni::sys::JNIEnv,
    _this: jni::sys::jobject,
) {
    jni_guard("nativeBackPressed", || {
        let mut events = events();
        events.back_presses = events.back_presses.saturating_add(1);
        let count = events.back_presses;
        drop(events);
        diag::info(format!("jni: nativeBackPressed (queued #{count})"));
    });
}

/// `MainActivity.nativeImeText(text, selectionStart, selectionEnd)` — the soft
/// keyboard's editing state, forwarded from `GameActivity.stateChanged`.
///
/// This exists because winit 0.30.13 consumes android-activity's
/// `InputEvent::TextEvent` and drops it (`handle_input_event` matches only
/// motion and key events), so egui would otherwise never see a soft-keyboard
/// character. The bridge is spike-local; M13c owns the real one.
#[no_mangle]
pub extern "system" fn Java_app_suramak_spike_MainActivity_nativeImeText(
    env: *mut jni::sys::JNIEnv,
    _this: jni::sys::jobject,
    text: jni::sys::jstring,
    selection_start: jni::sys::jint,
    selection_end: jni::sys::jint,
) {
    jni_guard("nativeImeText", || {
        let Some(mut value) = read_jstring(env, text, "nativeImeText") else {
            return;
        };
        if value.len() > MAX_IME_BYTES {
            let original = value.len();
            let mut cut = MAX_IME_BYTES;
            while cut > 0 && !value.is_char_boundary(cut) {
                cut -= 1;
            }
            value.truncate(cut);
            diag::warn(format!(
                "jni: nativeImeText truncated {original} bytes to {cut}"
            ));
        }
        let chars = value.chars().count();
        let preview: String = value.chars().take(IME_PREVIEW_CHARS).collect();
        diag::info(format!(
            "jni: nativeImeText(chars={chars}, sel={selection_start}..{selection_end}) \"{preview}\""
        ));

        let start = crate::ui::utf16_to_char_index(&value, selection_start.max(0) as usize);
        let end = crate::ui::utf16_to_char_index(&value, selection_end.max(0) as usize);
        *IME_TEXT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ImeState {
            text: value,
            selection_start: start,
            selection_end: end,
        });
    });
}

/// Ask the Activity to recreate itself (the lifecycle check's trigger).
///
/// `Activity.recreate()` must run on the UI thread, so this calls the Kotlin
/// helper `postRecreate()`, which posts the call; we never touch a View from
/// this (android_main) thread. Since v2.1 the helper is exception-proof: a
/// failure comes back as `Err` (and lands on the lifecycle tab's status line),
/// instead of v2's silent `Java exception was thrown`.
pub fn request_activity_recreate() -> Result<(), String> {
    call_activity("postRecreate")
}

/// Ask Kotlin to re-read the window insets and push them again. Used when the
/// UI knows its inset values are stale: a reset button zeroes the counters, and
/// before v2.1 nothing ever pushed them again (the panel sat at 0/0 for the rest
/// of the session).
pub fn request_insets_push() -> Result<(), String> {
    call_activity("postPushInsets")
}

/// Ask Kotlin to (re)read `spike-last-crash.txt` and put it on screen. Used on
/// the fatal-panic path, where the egui surface is gone and a Kotlin `TextView`
/// is the only thing that can still draw the reason.
pub fn request_crash_overlay() {
    // This runs *after* a panic: nothing here may panic again, and a missing
    // ndk-context (init failed early) must not turn one failure into two.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = call_activity("postShowCrashOverlay");
    }));
}

/// Call a no-argument `MainActivity` method on the UI thread (each Kotlin helper
/// posts itself). Shared by the recreate trigger and the crash overlay.
fn call_activity(method: &str) -> Result<(), String> {
    let context = ndk_context::android_context();
    // SAFETY: android-activity initialised ndk-context with the process' JavaVM
    // and a global ref to the Activity (`ndk_context::initialize_android_context`
    // in its init path), and both stay valid for the process lifetime.
    let vm = unsafe { jni::JavaVM::from_raw(context.vm().cast()) }
        .map_err(|err| format!("JavaVM::from_raw: {err}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|err| format!("attach_current_thread: {err}"))?;
    let activity = unsafe { jni::objects::JObject::from_raw(context.context().cast()) };
    env.call_method(&activity, method, "()V", &[])
        .map(|_| ())
        .map_err(|err| format!("MainActivity.{method}(): {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insets_are_clamped_into_the_plausible_range() {
        assert_eq!(clamp_inset("top", 63), 63);
        assert_eq!(clamp_inset("top", -20), 0);
        assert_eq!(clamp_inset("top", i32::MAX), MAX_INSET_PX);
    }

    #[test]
    fn jni_guard_swallows_a_panic_and_returns() {
        diag::init_panic_hook();
        jni_guard("testExport", || panic!("deliberate test panic"));
        // Reaching this line is the assertion: the panic did not unwind out.
        let view = diag::crash_for_display();
        assert!(view.is_some(), "the panic should have been recorded");
        assert!(view.unwrap().text.contains("deliberate test panic"));
    }
}
