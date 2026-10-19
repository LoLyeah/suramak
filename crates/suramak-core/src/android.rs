//! Android platform seams (M13a) — path + logging only.
//!
//! Everything in this module is `cfg(target_os = "android")`-gated and
//! additive: desktop builds never compile it and are behaviourally identical.
//!
//! Why it exists: `directories::ProjectDirs` has no Android backend, so
//! [`crate::paths::data_dir`] would fail there — the Android shell hands in the
//! app-private directory instead. Logging needs the same shape: a sink the
//! shell installs (logcat, M13b+) that core can emit lines to. Both seams are
//! one-shot `OnceLock`s and add no dependencies.
//!
//! The callers are M13b's JNI bridge (`set_data_dir(filesDir)` at startup);
//! until then the seams are inert — `data_dir()` falls back to the desktop
//! lookup, and `log_line` is a no-op.

use std::path::PathBuf;
use std::sync::OnceLock;

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static LOG_SINK: OnceLock<fn(&str)> = OnceLock::new();

/// Records the app-private data directory (`Context.getFilesDir()`), supplied
/// by the Android shell before any path is used.
///
/// The first call wins; later calls are ignored — there is exactly one data
/// directory per process.
pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(dir);
}

/// The shell-supplied data directory, if [`set_data_dir`] has run.
pub(crate) fn data_dir_override() -> Option<PathBuf> {
    DATA_DIR.get().cloned()
}

/// Installs the platform log sink (logcat in M13b). First call wins.
pub fn set_log_sink(sink: fn(&str)) {
    let _ = LOG_SINK.set(sink);
}

/// Emits a log line to the shell-supplied sink; a no-op before one is set.
pub fn log_line(line: &str) {
    if let Some(sink) = LOG_SINK.get() {
        sink(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_dir_seam_keeps_the_first_value() {
        // Process-wide OnceLock: run order within this test binary is the only
        // assumption, so assert the *first* recorded value survives.
        set_data_dir(PathBuf::from("/first"));
        set_data_dir(PathBuf::from("/second"));
        assert_eq!(data_dir_override(), Some(PathBuf::from("/first")));
    }

    #[test]
    fn the_log_sink_seam_forwards_lines_once_installed() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        fn sink(line: &str) {
            if line == "hello android" {
                SEEN.store(true, Ordering::SeqCst);
            }
        }
        set_log_sink(sink);
        log_line("hello android");
        assert!(SEEN.load(Ordering::SeqCst));
    }
}
