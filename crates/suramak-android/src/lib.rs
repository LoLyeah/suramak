//! suramak-android — the Android shell crate.
//!
//! **M13a scope:** this crate exists to prove the engine stack (`suramak-core` +
//! `suramak-asr` — whisper.cpp, bundled libopus, symphonia) links into one
//! Android `cdylib`, with exactly **one** smoke entry point on the C ABI:
//!
//! - [`suramak_android_version`] → `*const c_char`, the Suramak version.
//!
//! Keep this crate nearly empty. The app shell (`android_main`, the JNI bridge,
//! the compact Deck) lands in M13b+ — see `docs/PLAN-M13-ANDROID.md` §3.

use std::ffi::{c_char, CString};
use std::sync::OnceLock;

/// The crate version (the workspace version, e.g. `0.3.0`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
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
/// STL mismatch fails the build here, not at runtime). Do not remove them
/// until M13b replaces this with a real surface.
#[no_mangle]
pub extern "C" fn suramak_android_version() -> *const c_char {
    let _ = suramak_core::paths::data_dir();
    let _ = suramak_asr::system_info();
    let _ = suramak_asr::parse_lang("en");
    version_cstring().as_ptr()
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
}
