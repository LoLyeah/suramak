# WORKORDER — M13B5: spike v2.3 — the recreate/ANR edge (final shim item)

Tree: current `main` at `b0d4ea9`. **v2.2 device pass verified the surface fix**:
3 Home round-trips in ONE process (resumes 1→2→3, rebuilds 0→2, `frames gated
while suspended` counting), zero surface panics anywhere, TEXT 42 chars/76 IME/
41 deletions (per-edit + per-backspace counters all live), MOTION 90.5–102.9
fps, back 5× intercepted, ime inset appears when the keyboard opens. The
stale-Surface crash class is **dead on device**.

**The one remaining defect — the crash banner named it exactly** (v2.2's
diagnostics again): the previous process "vanished without onExit. No Rust panic
and no Java exception was recorded for it: **ANR-kill**" (pid 1554, Android 16,
vivo V2515) — after a session where **"Recreate activity now" had been tapped**.

## What is known

- `postRecreate()` throws as before: `MainActivity.postRecreate(): Java
  exception was thrown` — surfaced honestly (v2.1's criterion met — the outcome
  is legible, not silent).
- BUT the session that tapped recreate + then backgrounded the app was
  **ANR-killed by the OS while suspended** — the process vanished without any
  recorded panic/exception. The reproduce shape: tap Recreate (exception)
  → keep typing, Restart surface ✓, Home → app never comes back as its own
  process (fresh start, state gone).
- Sessions with NO recreate tap: 3+ Home round-trips survive in one live
  process. So the ANR path is bound to the recreate attempt, not to the
  backgrounding itself.

## Root-cause hypothesis to test first

`postRecreate()` throws (likely an IllegalStateException inside
`Activity.recreate()`'s restart machinery on GameActivity), but Android's
recreate/relaunch machinery may still book a *pending* recreate on the
Activity's token — leaving the system expecting a relaunch that never
completes. Home/refresh then stalls the or activity lifecycle; the OS
watchdog ANRs (Application Not Responding) and kills the process. The Rust
side sees nothing — the kill is outside the process.

## Fix (two-layered; the second layer is mandatory)

1. **Remove the trigger**: the shim should no longer call
   `Activity.recreate()`/`postRecreate()` at all on the GameActivity path if the
   Java API is not supported there — instead, the button performs a **full
   stop/start restart** (finish() + relaunch via an explicit intent with
   `FLAG_ACTIVITY_CLEAR_TOP|SINGLE_TOP` or, cleaner, the
   `android-activity`-provided exit → androidx restart route), OR the button
   labels itself `recreate: unsupported on GameActivity` and the "probe"
   explains precisely why (once proven unsupported, honest naming is the fix —
   the plan's later milestones never need Activity.recreate on Android).
   Choose by evidence: read `android-activity`'s handling, the AOSP docs on
   `Activity.recreate()` + GameActivity, and the M13B3/M13B4 reports — then
   pick the route that produces a REAL relaunch (state must be saved as part of
   it) or a permanent honest-explicit disabled state.
2. **No ANR ever again**: whatever the choice, a failed/widowed recreate must
   leave NO pending relaunch machinery on the Activity: catch the exception,
   then verify via `isRecreating()`/token state — and if Android's relaunch
   machinery must be armed, disarm it (recreate the only clean path: call
   `finish()` to terminate that activity cleanly so the system is never left
   half-recreating).

## Tests / verification

- The legible-error state keeps working (the exact one-line error the spike
  shows on a failed recreate is test-pinned already — extend it for the new
  outcome, if the button is disabled it renders that reason without Java).
- Kotlin: compileDebugKotlin lint-clean; aapt badging shows versionCode 5,
  versionName `0.3.0-m13b.2.3`.
- `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D
  warnings`; `cargo test --workspace` (572 baseline — add tests if the Rust
  side gains state-restore behavior for the stop/start restart; the surface
  lifecycle suite from v2.2 must stay green); `cargo ndk clippy -p
  suramak-android-spike --all-targets -- -D warnings`.
- `bash scripts/android/build-apk.sh` green with all assertions.
- **Honest boundary (report it)**: whether the ANR trigger is removed can only
  be proven ON DEVICE: the owner's pass = tap Recreate → see the outcome →
  Home → app must come back to its own live process (resumes +1, NOT a fresh
  process, no crash banner). If recreate becomes a disabled button: the check's
  NO-GO line "app crashes on recreate" is then impossible BY CONSTRUCTION, and
  the report says the recreate path is a documented shim limitation, with the
  owner's next pass re-running rotation to close the last untested piece.
- Report: `docs/reports/M13b5-spike-v2.3.md`.

## Do NOT

- No new dependencies; no changes outside `crates/suramak-android-spike/` +
  `android/` (+ version bumps only in the two sites already used in v2.2); no
  CI edits; no Gradle/AGP churn; no `git commit`/`push`; no sudo.
