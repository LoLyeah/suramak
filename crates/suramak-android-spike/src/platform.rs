//! The JNI surface between the Kotlin shim and the Rust UI thread.
//!
//! Kotlin → Rust (exported here, called from `MainActivity`):
//!   * `nativeSetInsets(top, bottom, left, right, imeBottom)` — window insets in px;
//!   * `nativeBackPressed()` — a back gesture/button was intercepted;
//!
//! Rust → Kotlin: [`request_activity_recreate`] calls `MainActivity.postRecreate()`
//! over JNI, which posts `Activity.recreate()` onto the UI thread.
//!
//! All state crosses threads through one `Mutex<PlatformEvents>`; the JNI calls
//! arrive on the Android UI thread, the UI polls them each frame. Each export
//! stays panic-free (a panic across `extern "system"` aborts the process), so
//! the lock is taken with `unwrap_or_else` rather than `unwrap`.

use std::sync::Mutex;

/// Events queued by the Kotlin shim, drained by the UI thread once per frame.
#[derive(Debug, Default)]
struct PlatformEvents {
    /// `(top, bottom, left, right, ime_bottom)` in physical pixels.
    insets: Option<(i32, i32, i32, i32, i32)>,
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

fn events() -> std::sync::MutexGuard<'static, PlatformEvents> {
    EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Take everything the shim reported since the last call.
pub fn drain() -> (Option<(i32, i32, i32, i32, i32)>, u32) {
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
    events().insets = Some((top, bottom, left, right, ime_bottom));
}

/// `MainActivity.nativeBackPressed()`.
#[no_mangle]
pub extern "system" fn Java_app_suramak_spike_MainActivity_nativeBackPressed(
    _env: *mut jni::sys::JNIEnv,
    _this: jni::sys::jobject,
) {
    let mut events = events();
    events.back_presses = events.back_presses.saturating_add(1);
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
    if env.is_null() || text.is_null() {
        return;
    }
    // SAFETY: the JVM passes a valid JNIEnv and a valid local jstring for the
    // duration of this call; both are only used before we return.
    let Ok(mut env) = (unsafe { jni::JNIEnv::from_raw(env) }) else {
        return;
    };
    let jtext = unsafe { jni::objects::JString::from_raw(text) };
    let Ok(raw) = env.get_string(&jtext) else {
        return;
    };
    let value: String = raw.into();
    let start = crate::ui::utf16_to_char_index(&value, selection_start.max(0) as usize);
    let end = crate::ui::utf16_to_char_index(&value, selection_end.max(0) as usize);
    *IME_TEXT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ImeState {
        text: value,
        selection_start: start,
        selection_end: end,
    });
}

/// Ask the Activity to recreate itself (the lifecycle check's trigger).
///
/// `Activity.recreate()` must run on the UI thread, so this calls the Kotlin
/// helper `postRecreate()`, which posts the call; we never touch a View from
/// this (android_main) thread.
pub fn request_activity_recreate() -> Result<(), String> {
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
    env.call_method(&activity, "postRecreate", "()V", &[])
        .map(|_| ())
        .map_err(|err| format!("MainActivity.postRecreate(): {err}"))
}
