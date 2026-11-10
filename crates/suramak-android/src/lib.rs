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
//! Keep this crate thin: the app shell (`android_main`, the compact Deck) lands
//! in M13g — see `docs/PLAN-M13-ANDROID.md` §3.

use std::ffi::{c_char, CString};
use std::sync::OnceLock;

pub mod model_host;

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
