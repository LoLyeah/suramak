//! Single-instance guard.
//!
//! Two running copies of Suramak would share one data directory (SQLite
//! library, settings, recordings) and on macOS double the AppKit/audio stack —
//! observed in the wild on 2026-10-13 (`mac-smoke.sh` step 7 auto-opened the
//! app while the owner launched it from a terminal too). The second copy now
//! prints a clear message and exits instead.
//!
//! Two mechanisms, one contract: `flock(2)` (`LOCK_EX | LOCK_NB`) on
//! `suramak.lock` in the data directory on unix, and the named mutex + activation
//! event of `instance_windows.rs` on Windows (PLAN M14 D2). Both hold for the
//! whole process lifetime and both leave a second launch with a clear outcome
//! (unix: a message and exit 2; Windows: the running window raised, exit 0). The
//! platform-neutral rules — object names, SID formatting, the exists→signal→exit
//! decision — live in [`protocol`] so they are unit-tested on every host.
//!
//! Smoke modes (`SURAMAK_SMOKE_SECS` / `SURAMAK_SMOKE_JOB`) skip the guard:
//! they are transient verification runs that may legitimately overlap an
//! already-open app (mac-smoke step 4).

// Used by the unix guard and by the stub for a target with neither mechanism;
// Windows has its own guard, which takes no path.
#[cfg(not(windows))]
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

/// No guard on a target that has neither mechanism (M14b: Windows moved to its
/// own named-mutex guard, so this is left for any other non-unix target).
///
/// Returns a plain [`std::io::Result`] rather than [`GuardError`]: there is no
/// error to express here, and carrying the unix-only type would leave variants
/// unreachable on this target (see [`GuardError`]).
#[cfg(all(not(unix), not(windows)))]
pub fn acquire(_lock_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// D2 (M14b): the Windows single-instance protocol's **pure** half — the object
/// names and the two decisions — kept platform-neutral so it is unit-tested on
/// every host. The Win32 calls that use it live in `instance_windows.rs`.
///
/// `cfg(any(windows, test))`: on a non-Windows build nothing calls these, and an
/// unreachable item is a `dead_code` error under `-D warnings` (the
/// [`GuardError`] precedent); the test build compiles and tests them.
#[cfg(any(windows, test))]
pub(crate) mod protocol {
    /// `Local\`, not `Global\` (PLAN D2): per logon session, works without
    /// privileges, and fast user switching gives each user their own instance.
    const NAMESPACE: &str = "Local\\";
    const MUTEX_PREFIX: &str = "Suramak-SingleInstance-";
    const EVENT_PREFIX: &str = "Suramak-Activate-";

    /// The mutex name for a user SID: `Local\Suramak-SingleInstance-<SID>`.
    ///
    /// The SID keeps RDP sessions and other users on the same machine from
    /// colliding with this logon's instance.
    pub(crate) fn mutex_name(sid: &str) -> String {
        format!("{NAMESPACE}{MUTEX_PREFIX}{sid}")
    }

    /// The activation event's name for a user SID:
    /// `Local\Suramak-Activate-<SID>` (auto-reset, so every launch signals).
    pub(crate) fn event_name(sid: &str) -> String {
        format!("{NAMESPACE}{EVENT_PREFIX}{sid}")
    }

    /// A SID in the SDDL string form `S-<revision>-<authority>-<sub>…`, the same
    /// text `ConvertSidToStringSidW` produces for every SID that reaches this
    /// code (`NT AUTHORITY` = 5, `BUILTIN` = 16, `SECURITY_*` = 18, …): the
    /// 48-bit identifier authority printed in decimal, then each
    /// sub-authority in decimal.
    pub(crate) fn format_sid(revision: u8, authority: u64, sub_authorities: &[u32]) -> String {
        let mut sid = format!("S-{revision}-{authority}");
        for sub in sub_authorities {
            sid.push('-');
            sid.push_str(&sub.to_string());
        }
        sid
    }

    /// Who owns the single-instance mutex.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Claim {
        /// This process created the mutex: it is the one instance.
        Primary,
        /// Another instance already created it.
        Secondary,
    }

    /// The pure decision behind `claim`: `CreateMutexW` reported
    /// `ERROR_ALREADY_EXISTS`.
    pub(crate) fn claim_from(already_exists: bool) -> Claim {
        if already_exists {
            Claim::Secondary
        } else {
            Claim::Primary
        }
    }

    /// What a second launch does — signalled, then exit **0** (the launch did
    /// what the user asked: the app is up and in front).
    ///
    /// `allow_foreground`: the exiting process hands its right to set the
    /// foreground to the running instance (`AllowSetForegroundWindow(ASFW_ANY)`)
    /// *before* signalling, otherwise Windows' foreground lock can ignore the
    /// raise (PLAN D2). Kept here as data so a unit test pins the whole
    /// contract without a second process.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct SecondaryPlan {
        pub(crate) event_name: String,
        pub(crate) allow_foreground: bool,
        pub(crate) exit_code: i32,
    }

    /// The second-launch plan for a user SID.
    pub(crate) fn secondary_plan(sid: &str) -> SecondaryPlan {
        SecondaryPlan {
            event_name: event_name(sid),
            allow_foreground: true,
            exit_code: 0,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A real domain-user SID, as `ConvertSidToStringSidW` renders it.
        #[test]
        fn format_sid_matches_the_sddl_form() {
            assert_eq!(
                format_sid(
                    1,
                    5,
                    &[21, 1_000_000_000, 1_234_567_890, 1_000_000_001, 1001]
                ),
                "S-1-5-21-1000000000-1234567890-1000000001-1001"
            );
            assert_eq!(format_sid(1, 5, &[18]), "S-1-5-18"); // LOCAL SYSTEM
            assert_eq!(format_sid(1, 16, &[32, 544]), "S-1-16-32-544"); // Administrators
        }

        /// The mutex and event names are the documented D2 strings, in the
        /// `Local\` namespace, one per SID.
        #[test]
        fn the_object_names_are_the_documented_local_names() {
            let sid = format_sid(1, 5, &[21, 3623, 1_000, 2_000, 1001]);
            assert_eq!(
                mutex_name(&sid),
                r"Local\Suramak-SingleInstance-S-1-5-21-3623-1000-2000-1001"
            );
            assert_eq!(
                event_name(&sid),
                r"Local\Suramak-Activate-S-1-5-21-3623-1000-2000-1001"
            );
            assert!(mutex_name(&sid).starts_with("Local\\"), "never Global\\");
            assert_ne!(
                mutex_name(&sid),
                mutex_name(&format_sid(1, 5, &[21, 3623, 1_000, 2_000, 1002])),
                "a different user (or RDP session) gets a different instance"
            );
        }

        /// The exists→signal→exit decision: the second launch signals the running
        /// instance and exits 0 (not the unix guard's 2 — it did what was asked).
        #[test]
        fn a_second_launch_activates_and_exits_zero() {
            assert_eq!(claim_from(false), Claim::Primary);
            assert_eq!(claim_from(true), Claim::Secondary);
            let plan = secondary_plan("S-1-5-21-1-2-3-1001");
            assert_eq!(
                plan.event_name,
                r"Local\Suramak-Activate-S-1-5-21-1-2-3-1001"
            );
            assert!(plan.allow_foreground, "the foreground grant comes first");
            assert_eq!(plan.exit_code, 0);
        }
    }
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
