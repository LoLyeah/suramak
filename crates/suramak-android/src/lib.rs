//! suramak-android — the Android shell crate.
//!
//! **M13a:** proves the engine stack (`suramak-core` + `suramak-asr` —
//! whisper.cpp, bundled libopus, symphonia) links into one Android `cdylib`,
//! with one smoke entry point on the C ABI ([`suramak_android_version`]).
//!
//! **M13d:** adds the model-download host behind the Kotlin `JobService`
//! ([`model_host`]) and its one JNI entry point
//! (`ModelsBridge.nativeCommand`, see [`jni_surface`]). The host is plain Rust
//! so it runs (and is tested) on the host; only the JNI wrapper is
//! `cfg(target_os = "android")`.
//!
//! **M13e:** adds the import + transcription host ([`jobs`]) behind the same
//! one-function JSON pattern (`TranscribeBridge.nativeCommand`): the journal
//! that survives a process kill, the bounded copy-progress channel, and the
//! M16.1 `QueueDriver` that guarantees one inference slot. The service, the
//! notification, the wake lock and the SAF descriptor stay in Kotlin.
//!
//! Keep this crate thin: the app shell (`android_main`, the compact Deck) lands
//! in M13g — see `docs/PLAN-M13-ANDROID.md` §3.

use std::ffi::{c_char, CString};
use std::path::Path;
use std::sync::OnceLock;

pub mod compact_host;
pub mod export_job;
pub mod jobs;
pub mod mic;
pub mod model_host;
#[cfg(target_os = "android")]
pub mod shell;
// M22B: the voice-note offer and the paged transcript notification (pure).
pub mod voice_note;
// M24A: the watched inbox host (SAF source behind the core trait).
pub mod watch_host;

/// android-activity's entry point (M13g3).
///
/// The APK loads **this** library (`libsuramak_android.so`) and the window's
/// whole life — winit, wgpu, egui, the destroy gate — comes from the M13b
/// spike crate's `android_entry`. The product surface it draws is this crate's
/// [`shell::CompactShell`], so the one `CompactApp` the JNI `UiBridge` mutates
/// and the one the window paints are the same object: there is no second copy
/// of the host in a second loaded library.
#[cfg(target_os = "android")]
#[no_mangle]
pub fn android_main(app: android_activity::AndroidApp) {
    suramak_android_spike::android_entry(app, Box::new(shell::CompactShell::new()));
}

use compact_host::CompactHost;
use jobs::{AndroidJobHost, HostDirs};
use mic::MicHost;
use model_host::ModelHost;

/// The crate version (the workspace version, e.g. `0.4.0`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The process-wide download host the JNI surface talks to.
///
/// One process holds at most one download: the JobService is the only caller,
/// and `ModelHost::start` refuses a second concurrent run.
pub fn host() -> &'static ModelHost {
    static HOST: OnceLock<ModelHost> = OnceLock::new();
    HOST.get_or_init(ModelHost::new)
}

/// The process-wide import/transcription host (M13e).
///
/// Created on the first `init` command, because it needs the app's `filesDir`
/// before it can own a media directory, a journal and the library path. Later
/// commands never create it: a job protocol call before `init` fails closed
/// rather than writing to a guessed directory.
pub fn jobs_host() -> Option<&'static AndroidJobHost> {
    JOBS_HOST.get()
}

/// The one process-wide job host; `init` fills it, every other command reads it.
static JOBS_HOST: OnceLock<AndroidJobHost> = OnceLock::new();

/// The process-wide microphone host (M13F).
///
/// Created by `mic_init` after the job host exists, because stopping a take
/// hands its WAV to the M13E inbox. A mic command before init fails closed.
pub fn mic_host() -> Option<&'static MicHost> {
    MIC_HOST.get()
}

static MIC_HOST: OnceLock<MicHost> = OnceLock::new();

/// The compact UI host (M13g1): the library/settings state the shared Deck
/// screens read and the notification's deep link writes.
///
/// Reads the ONE process-wide host that `init` registers
/// ([`compact_host::host`]). A second `OnceLock` here once shadowed it — this
/// accessor always returned `None`, so the shell drew the startup placeholder
/// forever while `init` had in fact created the host (the owner's "opens and
/// stays on the splash" report, reproduced on the CI emulator).
pub fn compact_host() -> Option<&'static CompactHost> {
    compact_host::host()
}

/// Handles the compact UI protocol, including the one command that creates the
/// host (`init`). Kept beside [`dispatch_jobs`] so the JNI surface stays one
/// match per bridge.
pub fn dispatch_compact(request: &str) -> String {
    compact_host::dispatch(request)
}

/// Handles the mic protocol, including the one command that runs before the
/// host exists (`init`).
pub fn dispatch_mic(request: &str) -> String {
    let value = match serde_json::from_str::<serde_json::Value>(request) {
        Ok(value) => value,
        Err(_) => return r#"{"ok":false,"error":"invalid JSON request"}"#.to_string(),
    };
    let op = value.get("op").and_then(|v| v.as_str()).unwrap_or_default();
    if op == "init" {
        let root = value
            .get("root")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if root.is_empty() {
            return r#"{"ok":false,"error":"mic init needs the app files directory"}"#.to_string();
        }
        #[cfg(target_os = "android")]
        {
            let host = MIC_HOST.get_or_init(|| MicHost::production(std::path::Path::new(root)));
            return host.dispatch(r#"{"op":"status"}"#);
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = root;
            return r#"{"ok":false,"error":"microphone capture is only available on Android"}"#
                .to_string();
        }
    }
    match mic_host() {
        Some(host) => host.dispatch(request),
        None => r#"{"ok":false,"error":"the microphone host has not been initialised (send init first)"}"#
            .to_string(),
    }
}

/// Handles the job protocol, including the one command that runs before the
/// host exists (`init`).
pub fn dispatch_jobs(request: &str) -> String {
    let value = match serde_json::from_str::<serde_json::Value>(request) {
        Ok(value) => value,
        Err(_) => return r#"{"ok":false,"error":"invalid JSON request"}"#.to_string(),
    };
    let op = value.get("op").and_then(|v| v.as_str()).unwrap_or_default();
    if op == "init" {
        let root = value
            .get("root")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if root.is_empty() {
            return r#"{"ok":false,"error":"init needs the app files directory"}"#.to_string();
        }
        #[cfg(target_os = "android")]
        suramak_core::android::set_data_dir(std::path::PathBuf::from(root));
        let host =
            JOBS_HOST.get_or_init(|| AndroidJobHost::production(HostDirs::under(Path::new(root))));
        return host.dispatch(r#"{"op":"status"}"#);
    }
    match jobs_host() {
        Some(host) => host.dispatch(request),
        None => r#"{"ok":false,"error":"the job host has not been initialised (send init first)"}"#
            .to_string(),
    }
}

/// NUL-terminated form of [`version`], initialised once for the C ABI.
fn version_cstring() -> &'static CString {
    static VERSION_C: OnceLock<CString> = OnceLock::new();
    VERSION_C.get_or_init(|| CString::new(version()).expect("crate version contains no NUL"))
}

/// The one smoke symbol (M13a): returns the Suramak version as a NUL-terminated
/// C string. The pointer is static — the caller must not free it.
///
/// The engine calls below are deliberate and must stay: they are the only
/// references to `suramak-core` / `suramak-asr` from an exported symbol, and
/// [`suramak_asr::system_info`] reaches whisper.cpp's C API — so the final link
/// must resolve the static whisper/ggml archives for the target (an arch or
/// STL mismatch fails the build here, not at runtime).
#[no_mangle]
pub extern "C" fn suramak_android_version() -> *const c_char {
    let _ = suramak_core::paths::data_dir();
    let _ = suramak_asr::system_info();
    let _ = suramak_asr::parse_lang("en");
    version_cstring().as_ptr()
}

/// The JNI surface (M13d), exactly one function.
///
/// Kotlin (`app.suramak.models.ModelsBridge`) declares
/// `private external fun nativeCommand(request: String): String` and calls it
/// with a JSON object. The whole protocol lives in [`model_host`]; this module
/// only marshals the string and makes a panic harmless.
#[cfg(target_os = "android")]
mod jni_surface {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use jni::objects::{JObject, JString};
    use jni::sys::jstring;
    use jni::JNIEnv;

    /// `ModelsBridge.nativeCommand(String): String`.
    ///
    /// Every failure mode returns a JSON object with `ok:false` — a null return
    /// would make Kotlin's `JSONObject(reply)` throw. A panic inside the host is
    /// caught (unwinding across `extern "system"` is UB, and Rust ≥1.81 aborts
    /// the process) and reported as an internal error.
    #[no_mangle]
    pub extern "system" fn Java_app_suramak_models_ModelsBridge_nativeCommand<'local>(
        mut env: JNIEnv<'local>,
        _this: JObject<'local>,
        request: JString<'local>,
    ) -> jstring {
        let reply = catch_unwind(AssertUnwindSafe(|| {
            let input: String = match env.get_string(&request) {
                Ok(text) => text.into(),
                Err(_) => {
                    return "{\"ok\":false,\"error\":\"could not read the request string\"}"
                        .to_string()
                }
            };
            crate::host().dispatch(&input)
        }))
        .unwrap_or_else(|_| {
            "{\"ok\":false,\"error\":\"internal error in the model host\"}".to_string()
        });

        match env.new_string(reply) {
            Ok(string) => string.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// The M13e JNI surface: one function, same shape as the model bridge.
///
/// Kotlin (`app.suramak.transcribe.TranscribeBridge`) declares
/// `private external fun nativeCommand(request: String): String`. Every call
/// goes through [`crate::dispatch_jobs`], which owns the `init` special case.
#[cfg(target_os = "android")]
mod jni_transcribe {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use jni::objects::{JObject, JString};
    use jni::sys::jstring;
    use jni::JNIEnv;

    /// `TranscribeBridge.nativeCommand(String): String`.
    #[no_mangle]
    pub extern "system" fn Java_app_suramak_transcribe_TranscribeBridge_nativeCommand<'local>(
        mut env: JNIEnv<'local>,
        _this: JObject<'local>,
        request: JString<'local>,
    ) -> jstring {
        let reply = catch_unwind(AssertUnwindSafe(|| {
            let input: String = match env.get_string(&request) {
                Ok(text) => text.into(),
                Err(_) => {
                    return "{\"ok\":false,\"error\":\"could not read the request string\"}"
                        .to_string()
                }
            };
            crate::dispatch_jobs(&input)
        }))
        .unwrap_or_else(|_| {
            "{\"ok\":false,\"error\":\"internal error in the job host\"}".to_string()
        });

        match env.new_string(reply) {
            Ok(string) => string.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// The M13g1 compact-UI JNI surface: one function, the same shape as the other
/// bridges. Kotlin (`app.suramak.ui.UiBridge`) declares
/// `private external fun nativeCommand(request: String): String`; every call
/// goes through [`crate::dispatch_compact`], which owns the `init` special case.
#[cfg(target_os = "android")]
mod jni_compact {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use jni::objects::{JObject, JString};
    use jni::sys::jstring;
    use jni::JNIEnv;

    /// `UiBridge.nativeCommand(String): String`.
    #[no_mangle]
    pub extern "system" fn Java_app_suramak_ui_UiBridge_nativeCommand<'local>(
        mut env: JNIEnv<'local>,
        _this: JObject<'local>,
        request: JString<'local>,
    ) -> jstring {
        let reply = catch_unwind(AssertUnwindSafe(|| {
            let input: String = match env.get_string(&request) {
                Ok(text) => text.into(),
                Err(_) => {
                    return "{\"ok\":false,\"error\":\"could not read the request string\"}"
                        .to_string()
                }
            };
            crate::dispatch_compact(&input)
        }))
        .unwrap_or_else(|_| {
            "{\"ok\":false,\"error\":\"internal error in the compact UI host\"}".to_string()
        });

        match env.new_string(reply) {
            Ok(string) => string.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// The M13F JNI surface: one function, the same shape as the other bridges.
///
/// Kotlin (`app.suramak.mic.MicBridge`) declares
/// `private external fun nativeCommand(request: String): String`. Every call
/// goes through [`crate::dispatch_mic`], which owns the `init` special case.
#[cfg(target_os = "android")]
mod jni_mic {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use jni::objects::{JObject, JString};
    use jni::sys::jstring;
    use jni::JNIEnv;

    /// `MicBridge.nativeCommand(String): String`.
    #[no_mangle]
    pub extern "system" fn Java_app_suramak_mic_MicBridge_nativeCommand<'local>(
        mut env: JNIEnv<'local>,
        _this: JObject<'local>,
        request: JString<'local>,
    ) -> jstring {
        let reply = catch_unwind(AssertUnwindSafe(|| {
            let input: String = match env.get_string(&request) {
                Ok(text) => text.into(),
                Err(_) => {
                    return "{\"ok\":false,\"error\":\"could not read the request string\"}"
                        .to_string()
                }
            };
            crate::dispatch_mic(&input)
        }))
        .unwrap_or_else(|_| {
            "{\"ok\":false,\"error\":\"internal error in the microphone host\"}".to_string()
        });

        match env.new_string(reply) {
            Ok(string) => string.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn version_is_the_workspace_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert!(!version().is_empty());
    }

    #[test]
    fn the_smoke_symbol_returns_the_version_over_the_c_abi() {
        let ptr = suramak_android_version();
        assert!(!ptr.is_null());
        // SAFETY: the pointer is static and NUL-terminated (see `version_cstring`).
        let cstr = unsafe { CStr::from_ptr(ptr) };
        assert_eq!(cstr.to_str().unwrap(), version());
    }

    /// The process-wide host answers the same protocol as a fresh one.
    #[test]
    fn the_process_host_dispatches_the_command_protocol() {
        let reply: serde_json::Value =
            serde_json::from_str(&host().dispatch(r#"{"op":"version"}"#)).unwrap();
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["version"], version());
    }
}
