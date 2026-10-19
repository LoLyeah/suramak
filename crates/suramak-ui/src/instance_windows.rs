//! Windows single-instance guard (PLAN M14 D2): named mutex + activation event.
//!
//! The unix guard ([`crate::instance`]) is a `flock` on a lock file. Windows has
//! a better primitive: a **named kernel mutex** in the `Local\` namespace, which
//! the OS releases when the process dies — no stale-lockfile state at all.
//!
//! The protocol, in three parts:
//!
//! 1. **Claim** — `CreateMutexW("Local\Suramak-SingleInstance-<SID>")`. The first
//!    process creates it; a later one gets a handle to the existing object and
//!    `GetLastError() == ERROR_ALREADY_EXISTS`.
//! 2. **Signal** — the later process calls `AllowSetForegroundWindow(ASFW_ANY)`
//!    (so Windows' foreground lock lets the running instance raise itself), opens
//!    `Local\Suramak-Activate-<SID>` with `EVENT_MODIFY_STATE`, sets it, and
//!    exits **0**: the launch did what the user asked — the app is up and in
//!    front. Nothing is handed over (no file associations in v1; the event can
//!    grow into a named pipe later).
//! 3. **Watch** — the first process creates that auto-reset event and waits on it
//!    from a background thread; each signal posts a winit user event
//!    ([`crate::UserEvent::Activate`]) that restores, raises and focuses the
//!    window on the main thread.
//!
//! Every call here is `unsafe` Win32 via the `windows` crate. The *rules* — the
//! object names, the SID's string form, and the exists→signal→exit decision —
//! are pure and live in [`crate::instance::protocol`], unit-tested on every host;
//! the Win32 half is unit-tested on the CI `windows` job (see the tests at the
//! bottom: a real second `CreateMutexW` in the same process takes the
//! `ERROR_ALREADY_EXISTS` path, and the event round-trips through the watcher).
//!
//! One known, bounded race: a launch that arrives between the first process's
//! mutex claim and its `CreateEventW` finds no event to open. The secondary side
//! retries for [`HANDSHAKE_ATTEMPTS`] × [`HANDSHAKE_DELAY`] (1 s) and, if the
//! event still is not there, exits 0 anyway — the single-instance guarantee is
//! intact; only the raise is lost, and it is logged by the caller.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, GetCurrentProcess, OpenEventW, OpenProcessToken, SetEvent,
    WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE,
};
use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};

pub(crate) use crate::instance::protocol::{event_name, mutex_name, secondary_plan, Claim};

/// How many times (and how far apart) the second launch looks for the running
/// instance's activation event before giving up on the raise — the bounded race
/// documented at the top of this file.
const HANDSHAKE_ATTEMPTS: u32 = 40;
const HANDSHAKE_DELAY: Duration = Duration::from_millis(25);

/// What [`activate_running_instance`] managed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActivationReport {
    /// The running instance was signalled (the raise is on its way).
    pub(crate) signalled: bool,
    /// `AllowSetForegroundWindow(ASFW_ANY)` succeeded — without it the raise can
    /// be swallowed by the foreground lock, so the caller logs when it is false.
    pub(crate) foreground_granted: bool,
    /// The exit code the second launch uses — from
    /// [`crate::instance::protocol::secondary_plan`], so the tested contract is
    /// the one executed (0: the app is up and in front).
    pub(crate) exit_code: i32,
}

/// This logon's user SID as `S-1-5-21-…`, read from the process token.
///
/// `OpenProcessToken` + `GetTokenInformation(TokenUser)`; the SID bytes are
/// formatted by [`crate::instance::protocol::format_sid`] (pure, host-tested), so
/// no `ConvertSidToStringSidW` allocation is needed.
pub(crate) fn user_sid() -> io::Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: `GetCurrentProcess` is a pseudo-handle; `token` is a valid out
    // pointer for the duration of the call.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(win32)?;

    let sid: io::Result<String> = (|| {
        let mut needed = 0u32;
        // Sizing call: fails with ERROR_INSUFFICIENT_BUFFER by design.
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
        if needed == 0 {
            return Err(io::Error::other(
                "the process token reported no user SID size",
            ));
        }
        let mut buffer = vec![0u8; needed as usize];
        // SAFETY: `buffer` is at least `needed` bytes, the length the API asked
        // for; the API writes at most that many.
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                needed,
                &mut needed,
            )
        }
        .map_err(win32)?;
        // The buffer's alignment is 1 (a `Vec<u8>`), so read the struct as a
        // value — a reference to it would be UB. `TOKEN_USER.Sid` points into the
        // same buffer, which lives until the end of this closure.
        // SAFETY: the API wrote a TOKEN_USER at the start of `buffer`.
        let user: TOKEN_USER = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast()) };
        // SAFETY: the SID lives inside `buffer` (still alive here) and is
        // well-formed — the token API produced it.
        Ok(unsafe { read_sid(user.User.Sid.0.cast()) })
    })();

    // SAFETY: `token` is an owned handle from `OpenProcessToken`; closing it once
    // is correct. Runs on both the success and failure paths.
    let _ = unsafe { CloseHandle(token) };
    sid
}

/// Formats the SID at `sid` by reading its header (revision, sub-authority
/// count, 48-bit big-endian authority, then the sub-authorities).
///
/// # Safety
///
/// `sid` must point at a valid SID of at least `8 + 4 * SubAuthorityCount` bytes.
unsafe fn read_sid(sid: *const u8) -> String {
    let revision = *sid;
    let count = usize::from(*sid.add(1));
    let mut authority = 0u64;
    for index in 0..6 {
        authority = (authority << 8) | u64::from(*sid.add(2 + index));
    }
    let sub_authorities: Vec<u32> = (0..count)
        .map(|index| {
            let base = sid.add(8 + index * 4);
            u32::from_le_bytes([*base, *base.add(1), *base.add(2), *base.add(3)])
        })
        .collect();
    crate::instance::protocol::format_sid(revision, authority, &sub_authorities)
}

/// Claims the single-instance mutex for this logon.
///
/// `Ok(Claim::Primary)` means this process is the one instance and must keep
/// running ([`watch_activation`] then listens). `Ok(Claim::Secondary)` means
/// another instance owns it: signal it ([`activate_running_instance`]) and exit
/// 0. `Err` means the mutex could not be created at all (the caller warns and
/// continues without the guard, like the unix path's "cannot open the lock").
pub(crate) fn claim() -> io::Result<Claim> {
    let name = wide(&mutex_name(&user_sid()?));
    // SAFETY: `name` is NUL-terminated and outlives the call; the default
    // security descriptor and an unowned mutex are what we want (ownership is
    // irrelevant — existence is the signal).
    let mutex = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }.map_err(win32)?;
    // `GetLastError` is only meaningful immediately after `CreateMutexW`; the
    // generated wrapper does not call anything in between.
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    // Everything after the API calls is the pure decision.
    match crate::instance::protocol::claim_from(already_exists) {
        Claim::Primary => {
            // Hold the mutex for the process lifetime — the OS releases it on
            // exit, including a crash. `Box::leak` is the explicit "keep
            // forever" (the unix path's `mem::forget(file)`; `forget` is
            // rejected by clippy for a type without `Drop`).
            Box::leak(Box::new(HeldMutex { _handle: mutex }));
            Ok(Claim::Primary)
        }
        Claim::Secondary => {
            // SAFETY: `mutex` is an owned handle we no longer need.
            let _ = unsafe { CloseHandle(mutex) };
            Ok(Claim::Secondary)
        }
    }
}

/// The first instance's mutex handle, deliberately never closed.
struct HeldMutex {
    _handle: HANDLE,
}

/// Creates the activation event and waits on it from a background thread,
/// calling `on_activate` for every signal.
///
/// The event is **auto-reset**: the wait returns once per `SetEvent`, so a rapid
/// second launch cannot be coalesced away, and the thread re-arms itself.
pub(crate) fn watch_activation<F>(on_activate: F) -> io::Result<()>
where
    F: Fn() + Send + 'static,
{
    let name = wide(&event_name(&user_sid()?));
    // SAFETY: `name` is NUL-terminated and outlives the call; a non-manual-reset,
    // initially-unsignalled event is exactly the D2 contract.
    let event =
        unsafe { CreateEventW(None, false, false, PCWSTR(name.as_ptr())) }.map_err(win32)?;
    // `HANDLE` wraps a raw pointer, so it is not `Send`. The event is a kernel
    // object meant to be waited on from another thread, so the value crosses as
    // an integer and the handle is rebuilt inside the thread.
    let raw_event = event.0 as usize;
    std::thread::Builder::new()
        .name("suramak-activate".to_string())
        .spawn(move || {
            let event = HANDLE(raw_event as *mut std::ffi::c_void);
            loop {
                // SAFETY: `event` is the owned handle created above.
                let wait = unsafe { WaitForSingleObject(event, INFINITE) };
                if wait == WAIT_OBJECT_0 {
                    on_activate();
                } else {
                    // A failed wait (e.g. the handle was closed) ends the watcher;
                    // there is nothing to retry without the object.
                    break;
                }
            }
        })
        .map_err(io::Error::other)?;
    Ok(())
}

/// The second launch's side: grant the foreground hand-off, signal the running
/// instance's event, and report what happened.
///
/// The caller exits 0 whatever this returns — the app *is* running.
pub(crate) fn activate_running_instance() -> io::Result<ActivationReport> {
    let plan = secondary_plan(&user_sid()?);
    let name = wide(&plan.event_name);
    let mut last: Option<windows::core::Error> = None;
    for attempt in 0..HANDSHAKE_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(HANDSHAKE_DELAY);
        }
        // SAFETY: `name` is NUL-terminated and outlives the call.
        match unsafe { OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(name.as_ptr())) } {
            Ok(event) => {
                // PLAN D2: the grant must come from the process that holds the
                // foreground right now (this launch), *before* the signal.
                // Best-effort: a refused grant must not drop the signal.
                // SAFETY: no preconditions; ASFW_ANY needs no handle.
                let foreground_granted = unsafe { AllowSetForegroundWindow(ASFW_ANY) }.is_ok();
                // SAFETY: `event` is an owned handle; the caller's exit closes it
                // anyway, but be tidy.
                let signalled = unsafe { SetEvent(event) }.is_ok();
                // SAFETY: as above.
                let _ = unsafe { CloseHandle(event) };
                return Ok(ActivationReport {
                    signalled,
                    foreground_granted,
                    exit_code: plan.exit_code,
                });
            }
            // The first instance may not have created the event yet: retry.
            Err(err) => last = Some(err),
        }
    }
    Err(match last {
        Some(err) => win32(err),
        None => io::Error::other("the activation event was never found"),
    })
}

/// Maps a `windows` error to `io::Error`, keeping the Win32 code when the HRESULT
/// is the `HRESULT_FROM_WIN32` shape.
fn win32(err: windows::core::Error) -> io::Error {
    let code = err.code().0 as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((code & 0xFFFF) as i32)
    } else {
        io::Error::other(err.to_string())
    }
}

/// A NUL-terminated UTF-16 copy of `text` for the `*W` APIs.
fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real token, the real SID — and the name the guard actually uses.
    #[test]
    fn the_current_user_sid_formats_as_an_sddl_string() {
        let sid = user_sid().expect("the process token has a user SID");
        assert!(sid.starts_with("S-1-"), "revision 1: {sid}");
        assert!(
            sid.chars()
                .all(|c| c.is_ascii_digit() || c == 'S' || c == '-'),
            "only digits and dashes: {sid}"
        );
        assert!(sid.matches('-').count() >= 2, "{sid}");
        assert_eq!(
            mutex_name(&sid),
            format!("Local\\Suramak-SingleInstance-{sid}")
        );
        assert_eq!(event_name(&sid), format!("Local\\Suramak-Activate-{sid}"));
    }

    /// The exists path against the real kernel object, without a second process:
    /// a Windows mutex is recursive within a process, so the *second*
    /// `CreateMutexW` on the same name returns `ERROR_ALREADY_EXISTS` — exactly
    /// what a second launch sees. (`HANDLE`s are leaked on purpose; the test
    /// binary's exit releases them.)
    #[test]
    fn a_second_claim_on_the_same_name_reports_secondary() {
        assert_eq!(claim().expect("first claim"), Claim::Primary);
        assert_eq!(claim().expect("second claim"), Claim::Secondary);
    }

    /// The event round-trip, end to end, in one process: the watcher thread is
    /// waiting on the real auto-reset event, and the secondary side's
    /// grant→open→set path must wake it. No second process, no spawning.
    #[test]
    fn the_activation_event_wakes_the_watcher() {
        let (tx, rx) = std::sync::mpsc::channel();
        watch_activation(move || {
            let _ = tx.send(());
        })
        .expect("the activation event is created");
        let report = activate_running_instance().expect("the event is signalled");
        assert!(report.signalled, "SetEvent succeeded");
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the watcher thread delivered the activation");
    }
}
