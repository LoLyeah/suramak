# M13I-b — the relaunch SIGSEGV: the finish path now ends the process

Executed per `docs/WORKORDER-M13IB.md`. The bug: **one cold launch was fine, but
backing out during the splash and relaunching killed the app with a native
SIGSEGV** — the owner's "opens then closes", reproduced in CI
(run `43017092638`, artifact `suramak-android-smoke-evidence`).

**Run status.** The fix, the smoke assertions and this report are landed on the
working tree (`b3a4f7f` + the changes below). The emulator rerun is the
orchestrator's: this pass may not push, and the VPS has no `/dev/kvm`. §6 lists
what was verified here; §7 states what only the rerun can prove.

## 1. The evidence chain

Artifact `suramak-android-smoke-evidence` (run `43017092638`), extracted to
`/tmp/smoke-ev/`. The app's own log, `logcat-app.txt`, is 43 lines; the relevant
ones are below, in order (pid/tid from logcat's threadtime columns):

| Time (2026-11-11) | pid / tid | Line |
|---|---|---|
| 12:10:20.118 | 3356 / 3381 | `android_main entered (entry #1, activity generation 1)` |
| 12:10:20.871 | 3356 / 3381 | `first frame of this gpu generation presented` |
| 12:10:31.589 | 3356 / 3381 | `event: focus false` |
| 12:10:32.819 | 3356 / 3381 | `gpu generation #1 (epoch 1) dropped — surface, device, queue, renderer and egui context released` |
| 12:10:32.928 | 3356 / 3381 | `event: suspended — surface epoch 1 closed, frame gate closed (had gpu: true)` |
| 12:10:33.146 | 3356 / 3356 | `crash files: …` — `nativeInit` on the UI thread: a **new Activity instance in the same process** |
| 12:10:33.229 | 3356 / **3765** | `android_main entered (entry #2, activity generation 2)` |
| 12:10:33.229 | 3356 / 3765 | `event loop creation failed: EventLoop can't be recreated` |
| 12:10:33.411 | 3356 / 3381 | `destroy gate: activity generation 1 is being destroyed …` |
| 12:10:33.412 | 3356 / 3381 | `M13b spike: android_main returned (entry #1)` |

`exit-info.txt` (`dumpsys activity exit-info app.suramak.spike`):

```
ApplicationExitInfo #0:
  timestamp=2026-11-11 12:10:33.845 pid=3356 realUid=10209 packageUid=10209 …
  process=app.suramak.spike reason=2 (SIGNALED) subreason=0 (UNKNOWN) status=11
  importance=100 pss=0.00 rss=0.00 description=null state=empty trace=null
```

`reason=2` is `ApplicationExitInfo.REASON_SIGNALED`; `status=11` is `SIGSEGV`.
The process died **433 ms after its event loop returned**.

The same artifact shows the failure being misnamed by the old smoke:
`dumpsys-activities.txt` has `topResumedActivity=…nexuslauncher/…NexusLauncherActivity`
and `library.png` is the launcher, so the old script reported *"the focused
window is not app.suramak.spike/.MainActivity — the app did not come to the
front"* — as if the app had merely stayed in the background, never that it had
died on a signal.

### 1.1 What actually happened

1. BACK finished the Activity. The loop saw `nativeActivityDestroyed`, exited
   `run_app`, logged `android_main returned (entry #1)` — **and then returned
   from `android_main` while the process stayed alive.**
2. Before generation 1's `onDestroy` even reached the loop (33.411), the
   relaunch (`am start`) had created a **new Activity instance in the same
   process** (33.146): `android-activity` calls `android_main` once per Activity
   instance, on a fresh thread (tid 3765 vs 3381).
3. `EventLoop::builder().build()` failed on the second entry because winit's
   `EVENT_LOOP_CREATED` is a **process-global** `AtomicBool`
   (`winit/src/event_loop.rs:118-119` at the fork's rev `63f21f8`):

   ```rust
   if EVENT_LOOP_CREATED.swap(true, Ordering::Relaxed) {
       return Err(EventLoopError::RecreationAttempt);
   }
   ```

   whose `Display` is the logged `EventLoop can't be recreated`
   (`winit/src/error.rs:103`).
4. The old `run()` logged that error and **returned**, leaving the process with
   no event loop. The JNI/GameActivity side then drove the dead runtime, and
   433 ms later the process died on SIGSEGV — the status=11 record above.

The ordering matters: the new Activity existed *before* the old Activity's
destroy signal arrived. This is not fixable by "returning sooner"; the only
shape winit supports is **one loop per process, therefore one Activity per
process** — the process must end with the loop.

## 2. The fix

### 2.1 Layer 1 — the finish path ends the process

`crates/suramak-android-spike/src/app.rs`, `run()`: `run_app` returning is now
terminal. After the handler is dropped (so the GPU generation is released while
the Activity is still coherent) and the existing
`M13b spike: android_main returned (entry #N)` line is written, the loop end is
classified and the process exits through one helper:

```rust
let end = relaunch::classify_loop_end(platform::activity_destroyed(generation), loop_failed);
relaunch::end_process(end, std::process::exit)
```

`std::process::exit(0)`, so Android records a voluntary exit (`EXIT_SELF`), not
a signal. The next launch is a fresh process with a fresh `EventLoop` and a
fresh GPU generation — the only configuration winit allows.

The classification is the small host-compiled module
`crates/suramak-android-spike/src/relaunch.rs`:

| `LoopEnd` | Meaning | Exit |
|---|---|---|
| `ActivityDestroyed` | the finish path: `onDestroy` reached the loop | 0 |
| `ExitRequested` | `event_loop.exit()` without a destroy (`CloseRequested`) | 0 |
| `LoopFailed` | `run_app` returned an error | 0 |
| `LoopUnavailable` | `EventLoop::build` failed (layer 2) | 0 |

`end_process` takes the exit function as a parameter, so the decision table is
unit-testable on the host without killing the test process; the only production
caller passes `std::process::exit`, whose `!` return makes the call site
diverge. A function that returned here would be the old bug, and a non-zero
code would be a crash by another name — both are pinned by tests.

### 2.2 Layer 2 — the `EventLoop can't be recreated` guard

The `EventLoop::builder().build()` error arm used to log and return. It now does

```rust
relaunch::end_process(relaunch::LoopEnd::LoopUnavailable, std::process::exit)
```

— same clean exit, same code. In steady state layer 1 makes this unreachable:
the process is gone before a new Activity can be created. The CI interleaving
shows the window is real, though — the new Activity appeared **266 ms before**
the old process exited — so the guard is not purely theoretical: a launch that
lands inside the exit window now ends cleanly instead of driving a loop-less
runtime into a SIGSEGV.

### 2.3 What must not regress: the M13b suspend/resume containment

Untouched, and made unrepresentable rather than merely avoided:

* `Handler::suspended` still closes the frame gate, **drops the whole GPU
  generation** (surface, device, queue, renderer, `egui::Context`) and parks the
  loop on `ControlFlow::Wait`; `resumed` rebuilds it from the window current at
  that moment. The HIDE/resume probe cycle is this path and never returns from
  `run_app`.
* `relaunch::LoopEnd` has **no `Suspended` variant** on purpose: "a suspend exits
  the process" cannot be expressed. The module docs and the `suspended`
  handler's comment state the invariant.
* Nothing in `diag.rs` changed: the v2.4/v2.1 crash snapshot, the signal
  handlers that write `spike-last-crash.txt`, and the Share-log path are
  byte-identical in behaviour.

### 2.4 Files changed

| File | Change |
|---|---|
| `crates/suramak-android-spike/src/relaunch.rs` | **new** — the loop-end decision table, the exit helper and three host tests |
| `crates/suramak-android-spike/src/lib.rs` | `pub mod relaunch;` (host-compiled) |
| `crates/suramak-android-spike/src/app.rs` | layer 1 at the end of `run()`, layer 2 in the build-error arm, the M13I-b module-doc section, the suspend invariant comment |
| `scripts/android/emulator-smoke.sh` | the relaunch assertion block (§3) |
| `docs/android.md` | the smoke paragraph now names the finish-path/relaunch assertions (and the current `angle_indirect` GPU mode) |
| `docs/reports/M13ib-relaunch-sigsegv.md` | this report |

No CI workflow change (`ci.yml` untouched — the evidence upload already takes
all of `build/android-smoke/`), no new dependency, no engine change, no release
change.

## 3. The smoke asserts the fix

The flow is the same repro — splash screencap → BACK → relaunch → assert — with
the assertion block extended in `scripts/android/emulator-smoke.sh`. All new
work is in the script; the CI job (`ci.yml`) is unchanged and calls it.

| # | Assertion | What it proves |
|---|---|---|
| 0 | after BACK, `pidof app.suramak.spike` is empty within `SURAMAK_FINISH_TIMEOUT` (20 s; one extra BACK is sent first if a sheet absorbed the first) | **layer 1**: the finish path ends the process, not just the loop |
| a | the relaunch logs `android_main entered (entry #1, …)` from a **pid different from the first launch's** | the second launch is a **fresh process**. The entry counter is process-wide, so a fresh process necessarily logs `#1`; the old bug shape was the *same* pid logging `#2` — that fails here, named |
| b | `first frame of this gpu generation presented` in the **new pid's** log lines only | the new process's GPU generation painted (not the old process's marker) |
| c | the string `EventLoop can't be recreated` appears **nowhere** in the run's log | android_main never ran twice in one process |
| d | `dumpsys activity exit-info` contains no `reason=2 (SIGNALED)`; on failure the whole record is printed and the message names the signal death (`reason=2 (SIGNALED)`; `status=11` was SIGSEGV in the CI repro) | no native death. This check runs **before** the focus assertion, so a SIGSEGV is named as one instead of "the app did not come to the front" |

The pre-existing assertions (focus on `MainActivity`, the uiautomator dump,
the Library screencap gate, the monkey canary) are unchanged and still run
afterwards. A clean `reason=1 (EXIT_SELF)` record is printed as positive
evidence when present, but is not required: a system reap after the task is
removed is also not a crash.

The first launch's pid is captured before BACK from the `android_main entered`
line itself (logcat threadtime `$3`), and every later marker is scoped by pid.
Progress is reported on stderr so the helper output cannot be mistaken for a
marker.

**A note on the workorder's wording** — "(a) wait for the SECOND `android_main`
marker (entry #2 in a FRESH process)". A fresh process resets the process-wide
counter, so the second launch's marker reads `entry #1`; `entry #2` can only
come from the same process, i.e. the bug. The smoke asserts the *stronger*
fact: new pid **and** `entry #1`. The same-pid/`entry #2` log from
run `43017092638` is the negative fixture in §6's offline harness.

## 4. Asserted vs platform-only

**Asserted on this VPS**

* the decision table (`classify_loop_end`), the always-0 exit code and the
  reason text — three host unit tests in `relaunch.rs`;
* that the Android-gated code type-checks and is clippy-clean for
  `aarch64-linux-android` with `-D warnings` (host clippy cannot see `app.rs`);
* the smoke's log parsing (pid scoping, entry counter, exit-info pattern) against
  the real CI logcat and a synthetic fresh-process log — nine checks, §6.

**Platform-only (the smoke, not host code)**

* the exit syscall and the `ApplicationExitInfo` record it produces (`EXIT_SELF`,
  status 0) — written by Android, not by this code;
* whether `ActivityManager` restarts an Activity whose process exits **while the
  launch is in flight**. In the CI evidence that window was 266 ms (new
  Activity at 33.146, old destroy at 33.411). Layer 1 closes it as fast as the
  destroy signal allows; layer 2 makes a launch that still lands in it a clean
  restart rather than a SIGSEGV. The smoke deliberately waits for the process to
  be gone before relaunching, so the assertions do not depend on that race; the
  real-device tap timing is the owner-device gate.

**Not assertable by any host test**

* the GPU/driver behaviour of the relaunched process on hardware: the emulator
  is x86_64 + ANGLE, the owner's phone is arm64 + Adreno/Mali. No arm64
  instruction and no vendor driver is exercised.

## 5. The honest boundary

* **The emulator is x86_64; the owner's phone is arm64.** This fix is
  architecture-independent by construction (`EventLoop` creation and
  `std::process::exit` are the same code on both), but the run proves it on
  x86_64 only.
* **The relaunched process's GPU is ANGLE on the emulator**, not the phone's
  Vulkan driver. A fresh-process GPU generation that works here can still fail
  on a vendor driver; that stays owner-verify.
* **Rotation is now a process restart too.** A configuration change destroys the
  Activity → the loop returns → the process exits → Android recreates the
  Activity in a fresh process. That is the only supported shape, and the
  instance state (`probe_state`, the v2.4 Bundle path) survives it exactly as it
  survives a process kill. The visual cost is the platform's relaunch, not a
  correctness change; the smoke does not rotate.
* **The artifact's launch surface was the startup placeholder.** In
  `launch-onboarding.png` (the screencap the smoke takes just before BACK, ~10 s
  after the first frame) the app still draws `draw_starting` — "Suramak /
  Opening the library…". `library.png` is the launcher only because the app was
  already dead by then. This pass does not touch the compact-host attach path
  and makes no claim about the Library render; if the rerun is red at the
  **screencap gate**, that is the M13i §7 "host missing" row, not the relaunch
  contract. The relaunch assertions run before the screencap gate, so the
  evidence separates the two.
* **The smoke was not rerun here** (no `/dev/kvm`, no push). One rerun to green
  is the acceptance; §7 says what to read if it is red.

## 6. Verification ledger

Run on this VPS against the final tree:

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean (exit 0) |
| `RUST_TEST_THREADS=1 cargo test --workspace` | 1133 passed / 0 failed / 10 ignored, exit 0 |
| `cargo test -p suramak-android-spike` | 47 passed / 0 failed (44 before + 3 new) |
| `cargo ndk -t arm64-v8a -P 29 clippy -p suramak-android-spike -- -D warnings` | clean (NDK r27d; this is what compiles `app.rs`, which host clippy skips) |
| `bash -n scripts/android/emulator-smoke.sh` | clean |
| offline smoke-assertion harness (real CI logcat + a synthetic fresh-process log) | 9/9, `HARNESS-OK` |
| `python3 tests/android-smoke/inspect_screencap.py --self-test` | OK: 3 positives pass, 2 blank controls fail |

The offline harness (`/tmp/smoke-test/harness.sh`, assertions only, not a repo
artifact) extracts the new shell functions from the smoke verbatim and checks:

1. the CI log's newest entry is pid 3356, entry #2 (the bug shape);
2. no "fresh launch" is accepted from that log — the new assertion would fail;
3. pid scoping attributes the first frame to pid 3356 and finds none for a
   different pid;
4. the exit-info SIGNALED pattern matches the CI record;
5. a fresh process reads as pid 4102, entry #1;
6. `wait_for_fresh_launch` accepts it;
7. the new generation's first frame is found in the new pid's lines;
8. the guard marker is absent from a clean log;
9. a clean `EXIT_SELF` record does not trip the SIGNALED assertion.

Not run here: the emulator job itself (`android-smoke` needs KVM and a push).

## 7. Reproduce / what to read on the rerun

```bash
# local gates
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
RUST_TEST_THREADS=1 cargo test --workspace
bash -n scripts/android/emulator-smoke.sh

# the Android-gated code (needs the NDK; this is the half host clippy cannot see)
export ANDROID_NDK_HOME=/home/ubuntu/android-ndk/android-ndk-r27d
cargo ndk -t arm64-v8a -P 29 clippy -p suramak-android-spike -- -D warnings

# the smoke (KVM host + SDK + system image)
SURAMAK_ANDROID_ABI=x86_64 bash scripts/android/build-apk.sh
bash scripts/android/emulator-smoke.sh
```

On the rerun's artifact `suramak-android-smoke-evidence`, the passing shape is:

* `logcat-app.txt` — first process: `android_main entered (entry #1, …)`,
  `… android_main returned (entry #1)`, then
  `relaunch: the Activity was destroyed (finish path) … ending the process
  (code 0)`; second process: `android_main entered (entry #1, …)` **with a new
  pid** and its own `first frame … presented`;
* `exit-info.txt` — no `reason=2 (SIGNALED)`; ideally `reason=1 (EXIT_SELF)`;
* `library.png` — the Library, judged by the screencap gate (unchanged).

If it is red, the smoke now names the layer: a surviving process after BACK is
the finish path; a same-pid `entry #2` or the `EventLoop can't be recreated`
string is the one-loop regression; `reason=2 (SIGNALED)` is the native death,
printed in full; and only after those does it blame focus or the render.
