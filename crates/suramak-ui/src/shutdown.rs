//! The quit path: best-effort teardown, then a hard exit that cannot run
//! ggml's static Metal-device destructor (WORKORDER-HOTFIX-QUIT).
//!
//! ## Why this module exists
//!
//! Quitting (menu Quit / Cmd-Q / window close) after any whisper session
//! aborted on macOS with
//!
//! ```text
//! GGML_ASSERT([rsets->data count] == 0) failed   (ggml-metal-device.m:608)
//! ggml_metal_device_free ← __cxa_finalize_ranges ← exit ← [NSApplication terminate:]
//! zsh: abort
//! ```
//!
//! The window was already gone when it fired, but the abort noise (and the
//! crash report) had to go. The chain: the app's whisper contexts — and the
//! Metal residency sets of their buffers — live on the worker / live-decode
//! threads; nothing freed them before exit, and ggml's **static** Metal-device
//! destructor (`ggml-metal-device.cpp`: the function-local
//! `static ggml_metal_device_ptr ctx` in `ggml_metal_device_get`, whose deleter
//! calls `ggml_metal_device_free`) asserts that every residency set was
//! released first. That is the llama.cpp#17869 / #19137 class — "drop the
//! context before stopping the process".
//!
//! ## The load-bearing Darwin fact
//!
//! On Darwin **every** `exit()` runs the process's C++ static destructors:
//! `exit(3)` calls `_tlv_exit()` and then `__cxa_finalize(NULL)` (Apple libc,
//! `Libc/stdlib/FreeBSD/exit.c`), and `__cxa_finalize` runs the destructors
//! registered by the C++ runtime — including ggml's Metal device. So
//! **`std::process::exit` is not a hard exit on macOS**: it would still run the
//! destructor and still abort whenever a whisper context is alive at exit
//! (exactly the "quitting while a job runs" case). The only termination that
//! cannot run it is `_exit(2)` — the raw syscall; `_Exit` is a thin wrapper
//! over it (`Libc/stdlib/FreeBSD/_Exit_.c`), and nothing finalizes.
//!
//! That is why [`hard_exit`] calls `libc::_exit` (after flushing stdio by hand,
//! since `_exit` does not) rather than `std::process::exit`. The smoke paths
//! and the single-instance guard keep their existing `std::process::exit`
//! codes — they run before any whisper context can exist — but every quit
//! flavor of the running app ends in this module.
//!
//! The teardown in front of it ([`crate::runtime::Runtime::shutdown`], driven
//! by `App::quit` in `main.rs`) is deliberately bounded and best-effort: it
//! stops capture / live / playback / the worker through the existing teardown
//! paths, waiting only for threads that are already idle. A job in flight is
//! never interrupted; [`hard_exit`] covers it.

use std::time::Duration;

/// Upper bound of the quit teardown (WORKORDER-HOTFIX-QUIT: ≤ ~250 ms).
///
/// The teardown only *waits* for threads that are already idle — it never
/// blocks on a running job. Anything still alive when the grace expires is
/// covered by [`hard_exit`], which skips finalization entirely.
pub const TEARDOWN_GRACE: Duration = Duration::from_millis(250);

/// Poll interval while the quit teardown waits for a thread to exit.
pub const TEARDOWN_POLL: Duration = Duration::from_millis(2);

/// Flushes stdio and terminates the process **without running any finalizer** —
/// the one exit that cannot fire ggml's Metal teardown assert (see the module
/// docs for the full chain).
///
/// `std::process::exit` is deliberately not used: on Darwin it calls `exit(3)`,
/// which runs `__cxa_finalize(NULL)` and with it ggml's static Metal-device
/// destructor, which aborts (`GGML_ASSERT([rsets->data count] == 0)`) whenever
/// a whisper context is still alive — the abort this workorder removes.
/// `_exit(2)` skips all of that: no finalizers, no static destructors, no
/// assert, deterministic on every quit path.
///
/// Stdio is flushed by hand first because `_exit` does not flush it.
pub fn hard_exit(code: i32) -> ! {
    use std::io::Write as _;
    // Best-effort: a closed or wedged stream must never hold up the quit.
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    #[cfg(unix)]
    {
        // SAFETY: `_exit` is the raw termination syscall — it takes no locks,
        // touches no Rust state and never returns.
        unsafe { libc::_exit(code) }
    }
    #[cfg(not(unix))]
    {
        // Windows (compile-only port today) has no ggml Metal; the CRT's
        // atexit handlers are acceptable for this fallback.
        std::process::exit(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The workorder bound: the quit teardown must stay within ~250 ms.
    #[test]
    fn the_teardown_grace_stays_within_the_workorder_bound() {
        assert!(TEARDOWN_GRACE > Duration::ZERO);
        assert!(
            TEARDOWN_GRACE <= Duration::from_millis(250),
            "the quit teardown must never block longer than ~250 ms"
        );
        // The poll must be fine enough that the grace is actually usable.
        assert!(TEARDOWN_POLL < TEARDOWN_GRACE);
    }
}
