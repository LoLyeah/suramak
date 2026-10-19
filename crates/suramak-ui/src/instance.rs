//! Single-instance guard.
//!
//! Two running copies of Suramak would share one data directory (SQLite
//! library, settings, recordings) and on macOS double the AppKit/audio stack —
//! observed in the wild on 2026-10-13 (`mac-smoke.sh` step 7 auto-opened the
//! app while the owner launched it from a terminal too). The second copy now
//! prints a clear message and exits instead.
//!
//! Implemented with `flock(2)` (`LOCK_EX | LOCK_NB`) on `suramak.lock` in the
//! data directory; unix-only, no-op stub elsewhere until the Windows port needs
//! its own mechanism. The lock is held for the whole process lifetime.
//!
//! Smoke modes (`SURAMAK_SMOKE_SECS` / `SURAMAK_SMOKE_JOB`) skip the guard:
//! they are transient verification runs that may legitimately overlap an
//! already-open app (mac-smoke step 4).

use std::path::Path;

/// What can go wrong acquiring the instance lock.
///
/// Unix-only, like the flock guard itself: on other targets `acquire` returns
/// a plain [`std::io::Result`] (it cannot fail), so this type would be
/// unreachable there — and `-D warnings` rightly flags an enum nothing can
/// construct (`clippy::dead_code`, caught by the first windows CI run).
#[cfg(unix)]
#[derive(Debug)]
pub enum GuardError {
    /// Another live instance holds the lock.
    AlreadyRunning,
    /// The lock file could not be created or opened.
    Io(std::io::Error),
}

#[cfg(unix)]
impl std::fmt::Display for GuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GuardError::AlreadyRunning => write!(f, "another instance is already running"),
            GuardError::Io(err) => write!(f, "cannot open the instance lock: {err}"),
        }
    }
}

#[cfg(unix)]
impl std::error::Error for GuardError {}

/// Acquires the exclusive instance lock at `lock_path`.
///
/// The fd is intentionally leaked (`mem::forget`) so the lock lives until the
/// process exits — there is nothing to release early.
#[cfg(unix)]
pub fn acquire(lock_path: &Path) -> Result<(), GuardError> {
    use std::os::unix::io::AsRawFd;

    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(GuardError::Io)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(GuardError::Io)?;
    // SAFETY: `file` owns a valid fd for the duration of the call; `flock`
    // only requires an open file descriptor.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        std::mem::forget(file);
        Ok(())
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Err(GuardError::AlreadyRunning)
        } else {
            Err(GuardError::Io(err))
        }
    }
}

/// Non-unix stub: no guard yet (the Windows port does not exist).
///
/// Returns a plain [`std::io::Result`] rather than [`GuardError`]: there is no
/// error to express here, and carrying the unix-only type would leave variants
/// unreachable on this target (see [`GuardError`]).
#[cfg(not(unix))]
pub fn acquire(_lock_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Unique per test + process; the lock intentionally leaks into the test
    /// process, so each test must use its own file.
    fn scratch_lock(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!(
                "suramak-instance-test-{tag}-{}",
                std::process::id()
            ))
            .join("suramak.lock")
    }

    #[test]
    fn second_acquire_reports_already_running() {
        let path = scratch_lock("guard");
        acquire(&path).expect("first acquire must succeed");
        let second = acquire(&path).expect_err("second acquire must fail");
        assert!(matches!(second, GuardError::AlreadyRunning));
    }

    #[test]
    fn distinct_paths_do_not_conflict() {
        let a = scratch_lock("a");
        let b = scratch_lock("b");
        acquire(&a).expect("acquire a");
        acquire(&b).expect("acquire b (a different lock file)");
    }
}
