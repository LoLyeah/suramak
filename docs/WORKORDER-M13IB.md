# WORKORDER — M13I-b: fix the relaunch SIGSEGV (the one-loop-per-process contract)

CI evidence (run 43017092638, artifact suramak-android-smoke-evidence):
cold launch OK (2.07 s, first frame painted), then BACK during the splash →
activity destroyed → relaunch in the SAME process →
`event loop creation failed: EventLoop can't be recreated` →
**ApplicationExitInfo reason=2 (SIGNALED) status=11** — a native SIGSEGV on
the second launch. This is the owner's "opens then closes" report,
reproduced on a stock Android 15 x86_64 emulator without a device.

## The contract

winit on Android allows exactly ONE EventLoop per process. The app's current
behavior: android_main entry #1 exits its loop when the activity is FINISHED
(user back), the process stays alive (the spike's containment design), and
the next launch's android_main entry #2 cannot create a loop → the JNI side
drives a dead runtime → SIGSEGV.

**The fix — two layers:**
1. When the event loop RETURNS because the activity is finishing (not
   suspending), the process exits cleanly (std::process::exit(0)) — the next
   launch then starts a fresh process with a fresh loop and GPU. The
   containment design stays for SUSPEND/resume (the M13b fix path: suspend
   does NOT finish the activity and does NOT end the loop — unchanged).
2. Defensive: on the `EventLoop can't be recreated` failure path, exit
   cleanly instead of proceeding (an unreachable-by-construction guard, but
   it turns any future regression into a clean restart, not a SIGSEGV).

## Deliverables

1. The two-layer fix in the Android app's main path (the spike crate's app.rs
   drives the compact surface; keep the probe/diag behavior intact — the
   spike's OWN containment loop for diagnostics must not regress: the probe's
   HIDE/resume cycle is the suspend path, not the finish path).
2. The M13b6 crash snapshot must still record a native kill if one ever
   happens (unchanged); the v2.1 Share-log path unchanged.
3. Tests: the finish-path exit contract (unit-level where testable; the
   loop-return path is platform-bound — document what is asserted vs
   platform-only). All suites stay green.
4. **The smoke asserts the fix**: in ci.yml's emulator smoke, after the
   relaunch: (a) wait for the SECOND `android_main entered` marker (entry #2
   in a FRESH process — logcat shows a NEW pid), (b) the first-frame marker
   for the new generation, (c) focus on MainActivity, (d) exit-info carries
   NO SIGNALED death for the package after the relaunch window (reason=2 is
   named in the failure message if it ever reappears). The smoke's flow
   (splash screencap → BACK → relaunch → assert) stays — it is the exact
   repro.
5. fmt/clippy -D warnings; workspace tests green; the android-spike suite
   green.

## Verification

- One rerun of the smoke to GREEN (the orchestrator pushes; you do not).
- Report: `docs/reports/M13ib-relaunch-sigsegv.md` — the evidence chain
  (run id, artifact paths, the logcat markers, the exit-info line), the fix,
  the honest boundary (arm64/GPU specifics stay the owner-device gate).

## Do NOT

- No CI changes beyond the smoke's assertion block; no release-path changes;
  no new dependencies; no engine/whisper changes; no git commit/push; no
  sudo. The M13b probe behaviors (MOTION/TEXT/INSETS/LIFECYCLE checks, the
  crash banner) must stay byte-compatible in spirit — the spike is the
  diagnostics surface behind Settings.
