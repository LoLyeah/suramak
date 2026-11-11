//! M13I-b — what this process does when `android_main`'s event loop ends.
//!
//! winit 0.30's Android backend allows **exactly one `EventLoop` per process**:
//! `EventLoop::builder().build()` trips a process-global `EVENT_LOOP_CREATED`
//! and returns `EventLoopError::RecreationAttempt` (`winit/src/event_loop.rs`,
//! whose `Display` is `EventLoop can't be recreated`, `src/error.rs`).
//! android-activity, however, calls `android_main` again for every Activity
//! instance — a rotation, or a launch after the user backed out while the
//! process was still alive.
//!
//! The old end-of-loop path returned and left the process alive with a dead
//! runtime. The next launch's `android_main` could not build a loop, returned,
//! and the JNI side then drove a runtime that no longer existed. In CI this is
//! `ApplicationExitInfo reason=2 (SIGNALED) status=11` — a native SIGSEGV, and
//! the owner's "opens then closes" report.
//!
//! The contract encoded here, in two layers:
//!
//! 1. **The finish path exits the process.** When the Activity is destroyed and
//!    the loop returns, the process ends (`std::process::exit(0)`). The next
//!    launch is then a fresh process with a fresh `EventLoop` and a fresh GPU
//!    generation — the only shape winit supports.
//! 2. **The loop-unavailable guard exits too.** If `EventLoop::build` ever fails,
//!    the process exits cleanly instead of proceeding with a runtime that cannot
//!    run. In steady state layer 1 makes this unreachable; the CI trace shows a
//!    launch can still land inside the exit window (the new Activity appeared
//!    ~270 ms before the old process left), and this branch is what turns that
//!    into a clean restart instead of a SIGSEGV.
//!
//! **A suspend is not a loop end.** The M13b containment — drop the whole GPU
//! generation on `suspended`, rebuild it from the window current at `resumed` —
//! never returns from the loop and never reaches this module. There is
//! deliberately no `Suspended` variant in [`LoopEnd`]: "a suspend exits the
//! process" is unrepresentable rather than merely discouraged, and the HIDE /
//! resume probe cycle keeps its own loop.
//!
//! Nothing here is Android-specific, so the decision table is host-testable
//! (`cargo test -p suramak-android-spike`); the call sites are the Android-only
//! `app::run`.

/// Why `android_main`'s event loop ended. Every variant is terminal for the
/// process: winit cannot build a second loop in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopEnd {
    /// The Activity was destroyed (`MainActivity.onDestroy`) — the user's BACK
    /// at the compact root, a removed task, or the system finishing it. This is
    /// the finish path.
    ActivityDestroyed,
    /// `event_loop.exit()` for a reason other than a destroy (`CloseRequested`).
    ExitRequested,
    /// `run_app` returned an error; the loop existed but cannot be reused.
    LoopFailed,
    /// `EventLoop::builder().build()` failed before a loop ever existed — the
    /// `EventLoop can't be recreated` guard (layer 2).
    LoopUnavailable,
}

impl LoopEnd {
    /// The sentence the device log carries. Each names its layer, so a support
    /// log says why the process left instead of guessing from its absence.
    pub fn reason(self) -> &'static str {
        match self {
            Self::ActivityDestroyed => {
                "the Activity was destroyed (finish path) — the loop is over and the next launch \
                 must get a fresh process"
            }
            Self::ExitRequested => {
                "the event loop was asked to exit — the next launch must get a fresh process"
            }
            Self::LoopFailed => {
                "the event loop ended with an error — the next launch must get a fresh process"
            }
            Self::LoopUnavailable => {
                "this process could not create an EventLoop (winit allows one per process) — \
                 exiting so the next launch gets a fresh one"
            }
        }
    }

    /// The process exit code. Always `0`: every variant is a voluntary, clean
    /// exit. A SIGNALED death (`status=11`) is exactly what this contract
    /// replaces, so no variant may ever map to a non-zero code — that would be
    /// a crash by another name.
    pub fn exit_code(self) -> i32 {
        0
    }
}

/// Classify a returned loop from the two facts `app::run` can observe after
/// `run_app` returns: whether this Activity generation was destroyed, and
/// whether the loop itself reported an error.
pub fn classify_loop_end(activity_destroyed: bool, loop_failed: bool) -> LoopEnd {
    if loop_failed {
        LoopEnd::LoopFailed
    } else if activity_destroyed {
        LoopEnd::ActivityDestroyed
    } else {
        LoopEnd::ExitRequested
    }
}

/// Log the reason and end the process through `exit`.
///
/// The exit call is a parameter so the decision is unit-testable on the host
/// without killing the test process; the only production caller passes
/// `std::process::exit`, whose `!` return makes the call site diverge. A
/// function that returned here would be the old bug.
pub fn end_process<T>(end: LoopEnd, exit: impl FnOnce(i32) -> T) -> T {
    log::info!(
        "relaunch: {} — ending the process (code {})",
        end.reason(),
        end.exit_code()
    );
    exit(end.exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_returned_loop_is_classified_by_its_cause() {
        assert_eq!(classify_loop_end(true, false), LoopEnd::ActivityDestroyed);
        assert_eq!(classify_loop_end(false, false), LoopEnd::ExitRequested);
        assert_eq!(classify_loop_end(false, true), LoopEnd::LoopFailed);
        // An error plus a destroy is named by the error: it is the new fact.
        assert_eq!(classify_loop_end(true, true), LoopEnd::LoopFailed);
    }

    #[test]
    fn every_loop_end_exits_with_the_clean_code() {
        // The contract against the CI evidence: reason=2 (SIGNALED) status=11
        // must be replaced by a voluntary exit, never by another non-zero code.
        for end in [
            LoopEnd::ActivityDestroyed,
            LoopEnd::ExitRequested,
            LoopEnd::LoopFailed,
            LoopEnd::LoopUnavailable,
        ] {
            assert_eq!(end.exit_code(), 0, "{end:?} must exit cleanly");
            assert_eq!(end_process(end, |code| code), 0, "{end:?} must reach exit");
        }
    }

    #[test]
    fn the_guard_names_the_one_loop_rule_and_the_finish_path_names_the_activity() {
        assert!(LoopEnd::LoopUnavailable
            .reason()
            .contains("one per process"));
        assert!(LoopEnd::ActivityDestroyed.reason().contains("destroyed"));
        // Every reason says why the next launch is safe (it gets a new process).
        for end in [
            LoopEnd::ActivityDestroyed,
            LoopEnd::ExitRequested,
            LoopEnd::LoopFailed,
            LoopEnd::LoopUnavailable,
        ] {
            assert!(
                end.reason().contains("fresh process") || end.reason().contains("fresh one"),
                "{end:?}: {}",
                end.reason()
            );
        }
    }
}
