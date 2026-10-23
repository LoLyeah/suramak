# M13b v2.4 — the rotation kill: a gate inside init, a signal dump, and the probe in the instance state

**Status: built, packaged and statically verified on this VPS. The rotation kill
cannot be *proven* gone from here — that is owner-device-only — but the window it
happened in is now closed by construction, and if a native signal ever fires
again the next launch says which stage it interrupted. §10 states the boundary
line by line.**

Workorder: `docs/WORKORDER-M13B6.md` (commit `8539315`). Field evidence this pass
answers (owner, vivo V2515 / Android 16, share-log card from the v2.3 build):

```
=== Suramak spike crash (session marker) ===
when:   2026-10-23 10:38:10Z (detected at launch)
where:  previous process
panic:  the session started at 2026-10-23 10:38:09Z · pid 20902 ·
        android 16 (API 36) · vivo V2515 — vanished without onStop.
        No Rust panic and no Java exception were recorded for it:
        the process was killed natively (SIGSEGV/SIGABRT/ANR-kill).
```

Previous builds: `docs/reports/M13b3-spike-v2.1.md`,
`docs/reports/M13b4-spike-v2.2.md`, `docs/reports/M13b5-spike-v2.3.md`.

```
android/app/build/outputs/apk/debug/app-debug.apk
  20,967,731 bytes (19.99 MiB)
  sha256 dce9e703bddbfa6d8cce452f94438c6eaeed6368515dbf35ea55e83c1f132883
  versionCode 6 · versionName 0.3.0-m13b.2.4 · package app.suramak.spike
  lib/arm64-v8a/libsuramak_android_spike.so — 13,576,840 bytes (LOAD Align 0x4000)
```

---

## 1. What changed

| File | Change |
|---|---|
| `crates/suramak-android-spike/src/diag.rs` | **native signal dump**: `Phase` (10 stages), `SignalSnapshot` (pre-rendered, seqlock), `init_signal_handlers()` + `on_fatal_signal()` (SIGSEGV/SIGABRT/SIGBUS → `spike-last-crash.txt` + history → `SIG_DFL` + re-raise); `set_phase()`; the ring refreshes the snapshot on every `push`; `set_files_dir` arms the handler; 3 new tests (one of them a **subprocess that dies of a real SIGSEGV**) |
| `crates/suramak-android-spike/src/surface_lifecycle.rs` | `Phase::Destroying` (terminal) + `on_destroyed()` + `DestroyReport`; `resumed` refuses to reopen a destroyed epoch; **`InitStage`** (surface → adapter → device → configure → present) and **`KillLatch`** (one-way, counts refusals); 3 new tests |
| `crates/suramak-android-spike/src/app.rs` | `Handler::note_destroyed()` (the terminal gate), destroy check in `resumed` / `about_to_wait` / `redraw` / `build_gpu` / `maybe_retry_gpu`; `Gpu::new` takes `&mut KillLatch` + generation and calls `stage(...)` before every window call; `GpuInitError::{Failed,Killed}`; every stage published to `diag::set_phase`; v2.4 module paragraph |
| `crates/suramak-android-spike/src/probe_state.rs` | **new** — the instance-state payload (`suramak-probe/1`, length-prefixed `probe` + `text`), `encode`/`decode`, 6 tests |
| `crates/suramak-android-spike/src/platform.rs` | `window_gone()` + `kill_pending(generation)` (the atomics the init path reads); `nativeSaveProbe(): jstring` and `nativeRestoreProbe(jstring)`; `nativeActivityDestroyed` also publishes `Phase::Destroying`; v2.4 module paragraph |
| `crates/suramak-android-spike/src/ui.rs` | `frames_after_destroy`, `probe_saves`, `probe_restores`, `probe_restore_note`; `restore_from_instance_state()` (explicit, does not inflate the typed counters), `note_probe_saved()`; lifecycle tab renders the instance-state line; header `M13B v2.4`; 1 new test |
| `crates/suramak-android-spike/src/lib.rs` | `init_signal_handlers()` at the top of `android_main`; `pub mod probe_state`; v2.4 paragraph |
| `android/…/spike/MainActivity.kt` | `nativeSaveProbe`/`nativeRestoreProbe` externals; `onSaveInstanceState` stores the payload under `app.suramak.spike.probe`; `onCreate` hands it back **before `super.onCreate`**; KDoc rewritten for v2.4 |
| `android/app/build.gradle.kts` | `versionCode 6`, `versionName 0.3.0-m13b.2.4` (comment updated; **nothing else**) |
| `scripts/android/build-apk.sh` | asserts versionCode 6 / versionName `0.3.0-m13b.2.4` |

No new dependency (`Cargo.toml` / `Cargo.lock` untouched — the libc calls the
handler needs are declared locally, §7.2). No Gradle/AGP change, no CI change, no
change outside `crates/suramak-android-spike/` + `android/` + the version
assertion in the build script (the manifest is untouched: rotation still
recreates, by design).

---

## 2. The evidence, decoded

| Fact in the card | What it means |
|---|---|
| `started at 10:38:09Z`, detected `10:38:10Z` | the process lived **≈1 s** — launch, not a session |
| `vanished without onStop` | it never reached a background transition; it died **in the foreground, during startup** |
| no Rust panic, no Java exception | nothing in-process recorded it: no unwind, no `catch`, no `onDestroy` |
| the owner's note: "rotate portrait to landscape that crashes the app" | the **first** rotation ever exercised on this build (every earlier session stayed portrait), and it happened near launch |
| `pid 20902` | a fresh pid for that session — the *previous* launch's process was already gone, so this was a clean start |

A rotation on this manifest destroys and recreates the Activity. The whole
session therefore consists of: launch → `android_main` entry #1 → window →
**rotation** → destroy → (would-be) entry #2. Dying inside that second, before
`onStop`, with no in-process record, is the signature of a fault in **native
code the Rust panic hook cannot see** — a driver or a use-after-free, delivered
as SIGSEGV/SIGABRT/SIGBUS.

What the previous build could not answer, and this one can: *which stage* was
running. §7 is the machine that answers it.

---

## 3. The rotation contract, read from `android-activity` 0.6.1

All line numbers are from `~/.cargo/registry/src/*/android-activity-0.6.1/`.

### 3.1 One process, many Activities — by design

```rust
// src/lib.rs:40-43
//! Although native applications will typically only have a single instance of
//! [`NativeActivity`] or [`GameActivity`], it's possible for these activities
//! to be created and destroyed multiple times within the lifetime of your
//! application process.
```

and the rotation case explicitly:

```rust
// src/lib.rs:74-78
//! Beware that, by default, certain configuration changes (e.g. device
//! rotation) will cause the Android system to destroy and re-create your
//! `Activity`, which will lead to a [`MainEvent::Destroy`] event being sent to
//! your `android_main()` thread and then `android_main()` will be called again
//! as a new native `Activity` instance is created.
```

So the **pivot is the supported path**, not an edge: entry #1 must end, entry #2
must start, in the same process. This is what the owner's device pass exercises
(§11).

### 3.2 What the native thread is, and when it is joined

```rust
// src/game_activity/mod.rs:1178-1180
// This is a spring board between android_native_app_glue and the user's
// `android_main` function. This is run on a dedicated thread spawned
// by android_native_app_glue.
```

```rust
// src/game_activity/mod.rs:1201-1204
// SAFETY: We know jni_activity is a valid JNI global ref to an Activity instance
// that will remain valid until `onDestroy` is handled (not possible until we start
// `android_main()`).
```

```rust
// src/game_activity/mod.rs:1230-1239
catch_unwind(|| {
    android_main(app);
})
.unwrap_or_else(log_panic);
```

```rust
// src/game_activity/mod.rs:1245-1247
// Let JVM know that our Activity can be destroyed before detaching from the JVM
// "Note that this method can be called from any thread; it will send a message
//  to the main thread of the process where the Java finish call will take place"
ffi::GameActivity_finish((*game_activity_glue).activity);
```

`GameActivity_finish` runs **unconditionally after `android_main` returns** — the
normal-return path, not just the panic path (v2.3 §4 already noted this). On a
rotation the Activity is already finishing, so this is a no-op `finish()` on a
finishing Activity; it is *not* a hazard. It does mean `android_main` must return
promptly (§3.4).

### 3.3 The destroy handshake — the app must be pumping

```rust
// src/game_activity/mod.rs:620-626
ffi::NativeAppGlueAppCmd_APP_CMD_DESTROY => {
    // We need to clear our `*mut android_app` pointer here because
    // `android_native_app_glue.c` is going to free the `android_app` once it
    // knows that this `android_main` thread has handled the `APP_CMD_DESTROY`
    // event. In this case the Java main thread is in the middle
    // of running `android_app_free()` in response to `onDestroy()`.
    self.game_activity.clear_app();
}
```

Two things follow, and both matter for the fix:

1. `onDestroy` (Java) only returns once the native thread has **handled**
   `APP_CMD_DESTROY` — the glue frees `android_app` after that acknowledgement,
   and the crate's comment says the Java thread is inside `android_app_free()`
   while it happens. That is what serialises entry #1's return before entry #2's
   start (the crate does not ship `android_native_app_glue.c`, so this report
   attributes the ordering to that comment, not to C source it read).
2. Handling the command requires the loop to **poll**. `src/lib.rs:216-220` says
   it outright: *"Some `Activity` lifecycle callbacks on the Java main thread will
   block until the next time `poll_events()` is called, so if you don't call
   `poll_events()` regularly you may trigger an ANR dialog"*. And
   `src/lib.rs:222-225` / `250-260`: after `MainEvent::Destroy` the `AndroidApp`
   has no Activity, most methods are no-ops, and a new Activity gets a new
   `AndroidApp` and a new `android_main`.

### 3.4 The event we never receive

winit 0.30.13 (the workspace's fork, `63f21f8`) does **not** forward it:

```rust
// winit-0.30.13/src/platform_impl/android/mod.rs:292
warn!("TODO: forward onDestroy notification to application");
```

That is why `MainActivity.onDestroy` calls `nativeActivityDestroyed()` over JNI
(v2.2 design, unchanged), and why every statement below about "the process knows"
means "the JNI export told it", not "winit delivered an event".

**The pivot, as it now runs:**

| # | Old Activity (UI thread) | Native thread | New Activity |
|---|---|---|---|
| 1 | `onPause` → `onStop` → `onSaveInstanceState` (payload saved) | frame loop | — |
| 2 | `surfaceDestroyed` → `logToRust("surfaceDestroyed")` **before** `super.surfaceDestroyed` | `SURFACE_DESTROYED` set (atomic) | — |
| 3 | `onDestroy` → `nativeActivityDestroyed()` **before** `super.onDestroy` | `DESTROYED_GENERATION = gen`, phase `Destroying` | — |
| 4 | `super.onDestroy` → glue writes `APP_CMD_DESTROY`, blocks in `android_app_free` | `about_to_wait` sees the flag → `event_loop.exit()`, `android_main` returns, `GameActivity_finish` (no-op) | — |
| 5 | `onDestroy` returns | thread exits | `onCreate(savedInstanceState)` → `nativeInit`, `nativeRestoreProbe` |
| 6 | — | — | `super.onCreate` → **new** native thread → `android_main` entry #2, `begin_activity()` = gen 2 |

Generation arithmetic keeps the two apart: `activity_destroyed(gen)` is
`DESTROYED_GENERATION >= gen`, so entry #2 (gen 2) is unaffected by the destroy
of gen 1, and `SURFACE_DESTROYED` is consumed at the top of entry #2's `resumed`
(`app.rs`, `let _ = platform::take_surface_destroyed_hint();`), so a stale window
signal cannot close the new epoch.

---

## 4. Why the v2.2 gate could not see it

v2.2's gate is a **policy fed by events**: `resumed` opens an epoch, `suspended`
(or the Java `surfaceDestroyed` line) closes it, and `gate_frame` /
`begin_build` consult that state. It is correct — and it is blind during exactly
one interval: **while the loop is inside `Gpu::new`.** No looper turn happens
there, so no `Suspended`, no `surfaceDestroyed` handling, no destroy handling.

`Gpu::new` is not fast: `pollster::block_on(request_adapter)` and
`pollster::block_on(request_device)` are synchronous Vulkan round trips, and the
build's own `gpu ready (…, {ms} ms, …)` log line measures them on every launch.
A rotation inside that interval produces this sequence:

```
t+0.0s  resumed  → epoch 1 → Gpu::new starts
t+0.3s  user rotates
t+0.31s onPause → surfaceDestroyed (Java) → SURFACE_DESTROYED = true   [unread]
t+0.35s onStop → onSaveInstanceState → onDestroy → nativeActivityDestroyed
        DESTROYED_GENERATION = 1, phase = Destroying                    [unread]
        Java thread now blocks in android_app_free(), waiting for the
        native thread to handle APP_CMD_DESTROY — which cannot happen
        while the native thread is inside request_device()
t+0.6s  Gpu::new continues: surface.configure(&device, &config)
        → against the ANativeWindow the platform already released
t+?     SIGSEGV/SIGABRT inside the driver. No Rust frame to unwind, no Java
        exception, no onStop, nothing in the ring — the v2.3 card.
```

Which call faulted and at which instruction is **not** in the record — the
driver's own fault is invisible to both languages. That is precisely the gap §7
is built to close on the next occurrence, and it is why the fix below is
"do not make the call", not "catch the fault".

---

## 5. Layer 2 — the destroy gate, inside the build

Two mechanisms, both host-tested, both wired into the one path that can fault.

### 5.1 `Phase::Destroying` — terminal, not another suspend

`SurfaceLifecycle` gains a third phase. `on_destroyed()` takes the live
generation, ends the epoch, and **never reopens**: `begin_build()` refuses with
`BuildRefusal { phase: DESTROYED }`, `gate_frame()` counts and drops, `adopt()`
counts a stale build, `retry_allowed()` is false, and a `Resumed` still queued in
the looper is refused (`refused_resumes`) instead of opening a new epoch for an
Activity that no longer exists.

The distinction from `suspended` is load-bearing and is pinned by a test: a
suspend is followed by a resume *in the same Activity* and must reopen; a destroy
is followed by a **new** `SurfaceLifecycle` in entry #2. Treating a Home →
return as a destroy would leave the session permanently dead — hence
`note_destroyed()` reads only `activity_destroyed(generation)`, never
`surfaceDestroyed`.

### 5.2 `KillLatch` + `InitStage` — one check per window call

`Gpu::new` now walks five named stages and asks the latch before each:

```rust
let mut stage = |stage: InitStage| -> Result<(), GpuInitError> {
    kill.allow(stage, platform::kill_pending(generation))
        .map_err(GpuInitError::Killed)
};

stage(InitStage::Surface)?;   diag::set_phase(Phase::GpuSurface);
… create_surface …
stage(InitStage::Adapter)?;   diag::set_phase(Phase::GpuAdapter);
… request_adapter …
stage(InitStage::Device)?;    diag::set_phase(Phase::GpuDevice);
… request_device …
stage(InitStage::Configure)?; diag::set_phase(Phase::GpuConfigure);
… surface.configure …
```

`platform::kill_pending(gen)` is two atomic loads: `activity_destroyed(gen) ||
window_gone()`. No lock, no looper, no allocation — it is readable from inside a
blocking init, which is the whole point. The latch is **one-way within a build**:
once a stage has seen the signal, every later stage is refused at that same name,
so a signal that arrives mid-build cannot be cancelled by a later "looks clear
now" read.

The result on the device is one of two log lines, both naming the stage:

```
gpu: init aborted at stage 'adapter' — the Activity or the window went away
mid-build (activity destroyed: true, window gone: true, stages refused: 3);
nothing was configured and nothing is adopted
```

and it is **not retried** (`GpuInitError::Killed` clears `next_gpu_retry`),
because either the Activity is gone (the loop exits on the next turn) or the
window is (the next resume owns the rebuild).

### 5.3 Both race directions, and the mid-session path

| Direction | What happens now |
|---|---|
| **destroy during init** (the v2.3 kill) | the latch trips at the stage that saw it; the build returns before the next window call; nothing is adopted; `about_to_wait` exits the loop; `android_main` returns promptly (required by `lib.rs:222-225`) so `android_app_free()` is not held |
| **init during/after destroy** | `build_gpu` calls `note_destroyed()` *first*, before it even clones the window handle, and returns; `resumed` refuses to open an epoch; a retry timer cannot fire (`maybe_retry_gpu` checks the same gate) |
| **mid-session rotation** (healthy, first time exercised) | old loop: `Destroy` → gate → exit; new entry: fresh `SurfaceLifecycle`, fresh `KillLatch`, `resumed` clears any stale window hint, builds generation #1 of the new epoch. The probe text is already in the process-wide state, and (§6) the Bundle re-applies it |
| **Home → return** (unchanged from v2.2/2.3) | `surfaceDestroyed` closes the epoch, `resumed` reopens it; the destroy gate is *not* involved — that is why a Home session still comes back |

### 5.4 What is left

The gap between `stage(Configure)` returning and `surface.configure` entering is
one non-blocking call wide (a few microseconds on the device), against hundreds
of milliseconds of Vulkan init before this pass. It is not zero, and §10.2 says
so rather than claiming "impossible".

---

## 6. Layer 3 — the probe in the instance state

### 6.1 What actually needed saving

The spike keeps both text fields in a process-wide `OnceLock<Mutex<SpikeState>>`,
so a **rotation in the same process** already preserved them — that was true in
v2.3 and is true now. What v2.3 could not survive is the **process** going away,
which is exactly what happened: the session died, and its text died with it.

The Bundle is the platform's answer, and it covers both cases: the system keeps
the Activity's instance state for a configuration change *and* hands it to the
recreated Activity in a **fresh process** after a kill. So the honest claim is:

* rotation → text intact, and the log now also shows the Bundle round trip
  (`instance state: saved` → `probe restored from instance state (N chars)`),
  because `onCreate` receives the same Bundle;
* process kill + relaunch from recents → text restored **from the Bundle**,
  which is the case that only exists because of this pass.

### 6.2 The payload

`crates/suramak-android-spike/src/probe_state.rs`, one string under the Bundle
key `app.suramak.spike.probe`:

```
suramak-probe/1
probe=<bytes of the probe field>
text=<bytes of the text field>
<probe bytes><text bytes>
```

Length prefixes instead of escaping: either field may contain newlines, `=`,
quotes, emoji; the decoder never guesses. Lengths are **bytes** (a UTF-8 slice),
which is also what survives the UTF-16 round trip through `Bundle`. Refusals are
named — `Magic`, `Header`, `Truncated`, `Boundary` (a length that splits a
character), `TooLarge` — and the restore path logs the reason, so a rejected
payload never reads like "no payload".

### 6.3 The round trip

| Side | Code |
|---|---|
| save | `MainActivity.onSaveInstanceState` → `nativeSaveProbe()` → `probe_state::encode(&ProbePayload{probe, text})` → `outState.putString(KEY_PROBE, payload)`; logs `instance state: saved N payload chars` |
| restore | `onCreate(savedInstanceState)` → **before `super.onCreate`** → `nativeRestoreProbe(payload)` → `decode` → `SpikeState::restore_from_instance_state` → logs `probe restored from instance state (13 chars)` / `text restored from instance state (142 chars)` |

Restore is **explicit**: `MainActivity.onCreate` is its only caller, no frame or
event can trigger it, and every outcome is logged (`restored`, `refused — <reason>`,
or `none (cold start, or nothing was saved)`). It also does not lie about what
the owner typed: `restore_from_instance_state` resets `last_text_chars` and drops
the caret, so check 1's `characters received` counter does not jump by the
restored length (test-pinned).

The lifecycle tab shows the mechanism directly:
`instance state: 2 save(s) · 2 restore(s)` and `last restore: restored 13 probe
char(s) + 19 text char(s) from the instance state`. The **reset** button still
clears the probe (workorder condition) and the header reset clears the counters.

---

## 7. Layer 1 — the native signal dump

### 7.1 What it writes

`diag::init_signal_handlers()` is called from `nativeInit` (via `set_files_dir`,
i.e. before `super.onCreate` and before any native window exists) and again from
`android_main`. It installs handlers for **SIGSEGV(11) / SIGABRT(6) / SIGBUS(7)**.
The handler writes `spike-last-crash.txt` (truncate + write + `fsync`), appends
the same block to `spike-crash-log.txt`, then restores `SIG_DFL` and re-raises.

A dump produced by a real SIGSEGV on this host (the subprocess test, §8.2):

```
=== Suramak spike NATIVE SIGNAL ===
when:   2026-10-23 10:56:26Z (uptime 0.00s)
pid:    2845241
phase:  gpu init: device
--- last 2 platform events (oldest first) ---
[    0.00s] info native signal handler installed (SIGSEGV/SIGABRT/SIGBUS -> spike-last-crash.txt)
[    0.00s] info in-flight: request_device returned; about to configure
SIGNAL: SIGSEGV (11) - the process faulted and is dying now.
This file is the last in-process state; no Rust panic was raised.
```

`phase` is the **in-flight event**; the ring is the run-up. On the phone the same
line would read `phase: gpu init: configure` if the v2.3 kill was where §4
suspects, or `phase: frame` if it was after the first present. Either way the
next launch's banner names it — which is the value this layer adds even if the
kill never happens again.

Because it writes `LAST_CRASH_FILE` (the file `CrashFiles` and the egui banner
already read), **no Kotlin change was needed for the display path**: the crash
card the owner already shares is the dump. `CrashFiles.foldAbandonedSession` only
writes its session-marker inference when there is no newer record, so a real dump
wins over the inference.

### 7.2 Async-signal-safety

The constraints shaped the code, and each choice is deliberate:

| Requirement | How |
|---|---|
| no allocation, no locks, no formatting in the handler | the block is **pre-rendered** by the normal thread into a static `[u8; 16 KiB]` (`SignalSnapshot`) on every `push` and every `set_phase`; the handler does `write(2)` on those bytes |
| no torn reads | a seqlock (`seq` odd while copying, even when stable); the handler never spins — a torn snapshot is still better than none |
| no `Path`/`File` in the handler | both paths are NUL-terminated `Vec<u8>` built once at install |
| open a file with only syscalls | `openat(AT_FDCWD, …)` through `syscall(2)`. `open` is **not** declared directly: it is variadic in glibc and Rust reserves the name as a runtime symbol (`invalid_runtime_symbol_definitions` refuses the declaration on the host), so the handler goes one level lower |
| no `sigaction` struct | `signal(2)` + an explicit `SIG_DFL` before the re-raise. The cost is no `SA_SIGINFO` (no fault address) and no `SA_ONSTACK` — see §10.3 |
| still die of the original signal | `signal(sig, SIG_DFL); raise(sig);` **then return**. The signal is blocked while its own handler runs, so the re-raise is delivered as the handler returns — with the default action installed, the process dies of the real signal and Android still writes its tombstone |

That last point was found the hard way and is worth recording: the first version
ended with `_exit(128 + sig)` as a belt-and-braces fallback, which meant the
handler exited **before** returning — the pending re-raise was never delivered
and the child died with exit code 139 instead of a signal death. The subprocess
test caught it (`status.signal() == None`), the `_exit` is gone, and the comment
at the re-raise says why there is deliberately nothing after it.

Cost, measured on this host (x86_64, `-O`, ring at capacity, 20 000 pushes):
**7.5 µs per `push`** (which includes re-rendering the whole ≤40-line block) and
**7.0 µs per `set_phase`**. The spike logs tens of events per second; that is
well under 0.1 % of one core. It is a host number, not a phone number — the
aarch64 cost is the same order (memcpy + `write`-shaped formatting).

### 7.3 Where it is armed in the lifecycle

`set_files_dir` → `init_signal_handlers()` is the earliest point the library is
reached (before `super.onCreate`, before `android_main`), so a fault in
`android_main` **entry #1** — the v2.3 case — is covered. `android_main` calls it
again (idempotent) in case the library is loaded without `onCreate`.

---

## 8. Tests — what each one pins

`cargo test -p suramak-android-spike`: **44 passed** (31 in v2.3 → **+13**).

### 8.1 Host-testable state machines

| Test | Contract |
|---|---|
| `a_destroy_ends_the_epoch_for_good_and_refuses_builds_frames_and_resumes` | `on_destroyed` takes the generation, phase is `DESTROYED`, `begin_build` refuses, `gate_frame` counts, `adopt` counts a stale build, a queued `Resumed` is refused and does not advance the epoch, a later `Suspended` is not counted as a suspend, a second destroy is a non-event |
| `the_kill_latch_stops_an_init_at_the_first_stage_that_saw_it` | a clean build passes all five stages; a signal at `adapter` aborts there, names `adapter` for every later stage, counts 3 refusals, and stays latched when the flag reads clear |
| `every_init_stage_can_trip_the_latch_and_configure_sorts_after_the_rest` | every stage can be the one that names the abort; the order is `surface < adapter < device < configure < present`; names are unique (they are the log contract) |
| `a_round_trip_keeps_every_byte_of_both_fields` (×6 in `probe_state`) | empty payloads, a typed sentence, CRLF, embedded `bytes=`/`text=`/magic text, emoji, trailing newline; header counts bytes; foreign/future magic, missing headers, wrong lengths and a length that splits a UTF-8 character are refused by name |
| `an_instance_state_restore_is_explicit_and_does_not_inflate_the_typed_counters` | restore sets both fields, counts one restore, writes the note, does **not** move `characters received`, and the reset button still clears the probe |
| `the_signal_snapshot_is_a_whole_crash_block_with_phase_and_pid` | the pre-rendered block carries `when`/`pid`/`phase`/the ring header; the handler's own header is *not* in the snapshot (so a truncated snapshot cannot pose as a whole file); all ten phase names are distinct |

### 8.2 The subprocess proof (the handler's persistence)

`the_signal_handler_writes_the_ring_then_re_raises_sigsegv` re-executes **this
same test binary** with `--exact diag::tests::signal_child_raises_sigsegv` and an
environment variable; the child sets the files dir, publishes a phase and an
in-flight event, installs the handler and calls `raise(SIGSEGV)`. The parent then
asserts the three things a mock cannot:

1. `ExitStatus::signal() == Some(11)` — the process **really** died of SIGSEGV,
   so the handler re-raised instead of swallowing it;
2. `spike-last-crash.txt` exists anyway, starts with the `NATIVE SIGNAL` header,
   and contains `phase:  gpu init: device`, the in-flight event text, `SIGSEGV
   (11)` and `no Rust panic was raised`;
3. `spike-crash-log.txt` carries the same block (history).

Without the environment variable the child body returns immediately, so a normal
`cargo test` run counts it as a pass and raises nothing.

This is the closest a host can come to the field: the signal, the death, the file
and the re-raise are all real. What it cannot prove is that *Android's* tombstone
path runs after our handler — §10.3.

---

## 9. Verification performed here

| Check | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 (clean) |
| `cargo clippy --workspace --all-targets -- -D warnings` | **exit 0** |
| `cargo test --workspace` | **586 passed, 0 failed, 8 ignored** (24 suites; v2.3 baseline 573 → **+13**) |
| `cargo test -p suramak-android-spike` | **44 passed** (31 in v2.3) |
| `cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-android-spike --all-targets -- -D warnings` | **exit 0** — the only gate that compiles `app.rs`/`platform.rs` |
| `./gradlew --no-daemon :app:compileDebugKotlin --rerun-tasks` | `BUILD SUCCESSFUL`, **0 `w:`/`e:` lines** |
| `bash scripts/android/build-apk.sh` | **exit 0**, all assertions passed (`SIZE: 20967731`, `SHA256: dce9e703…`), Kotlin log warning-free |
| `aapt dump badging` | `package: name='app.suramak.spike' versionCode='6' versionName='0.3.0-m13b.2.4'`, `sdkVersion:'29'`, `targetSdkVersion:'35'`, `native-code: 'arm64-v8a'`, launchable `app.suramak.spike.MainActivity` |
| `aapt dump xmltree AndroidManifest.xml` | `versionCode=0x6`, `versionName="0.3.0-m13b.2.4"`, `enableOnBackInvokedCallback=0xffffffff`, `screenOrientation=0xffffffff` (unspecified), **`configChanges=0x70`** (keyboard\|keyboardHidden\|navigation) — rotation still recreates, by design |
| `apksigner verify --verbose --print-certs` | `Verifies`, v2 scheme true, 1 signer |
| `readelf -lW` on the packaged `.so` | four `LOAD` segments, each `Align 0x4000` (16 KB pages) |
| `unzip -l` | `lib/arm64-v8a/libsuramak_android_spike.so` (13,576,840 bytes) + `AndroidManifest.xml` + 3 dex files |
| `nm -D` on the packaged `.so` | all eight `Java_app_suramak_spike_MainActivity_*` exports present, **including `nativeSaveProbe` and `nativeRestoreProbe`** — a missing symbol here would have been an `UnsatisfiedLinkError` at the first rotation |
| `dexdump -d classes3.dex` | `MainActivity` declares `nativeSaveProbe`/`nativeRestoreProbe`, calls `nativeRestoreProbe` from `onCreate` and overrides `onSaveInstanceState(Landroid/os/Bundle;)V` |
| `strings` on the packaged `.so` | `NATIVE SIGNAL` ×2, `probe restored from instance state` ×1, `gpu init aborted at` ×1, `destroy gate` ×2, `native signal handler installed` ×1; `SA_ONSTACK` ×0 (documented, §10.3) |
| `git diff --stat -- Cargo.toml Cargo.lock` | **empty** — no dependency added |

---

## 10. The honest boundary

1. **The rotation kill's removal is owner-device-only.** Nothing on this host can
   run a Mali driver, an `ANativeWindow` or a `GameActivity`. What is provable
   here: after either signal, this generation makes **no** window or gpu call
   (structural: `build_gpu` refuses before the window handle is even cloned, and
   `Gpu::new` checks before each stage), the structural checks are host-tested,
   and the remaining window is one non-blocking call wide (§10.2). Whether the
   driver ever faults again is the phone's answer.
2. **The residual race is small, not zero.** Between `stage(Configure)` returning
   and `surface.configure` entering, a destroy could still land. It is one
   non-blocking call, against the two blocking Vulkan round trips
   (`request_adapter`, `request_device`) the previous build left fully exposed.
   Closing it completely would need a lock shared with the UI thread around every
   window call — which is a different failure mode (the ANR class) and is not what
   this pass claims.
3. **The signal handler cannot catch everything.** Not covered: ANR kills and
   `kill -9` (no signal to us), stack-overflow SIGSEGV (the handler runs on the
   exhausted stack — no `SA_ONSTACK` without `sigaction`, so it may fault again;
   the tombstone remains the record), `SIGILL`/`SIGFPE` (left to the tombstone on
   purpose), and a fault inside the snapshot refresh itself (a torn block is still
   written). It also cannot prove Android's tombstone is still produced on the
   phone: the re-raise is verified on the host by `ExitStatus::signal() == 11`,
   which is the same kernel mechanism but not the same crash-reporting stack.
4. **The probe restore proves itself only where the platform restores the
   Activity record.** A rotation and a low-memory/`am kill` while backgrounded do;
   a force-stop or a swipe away from recents does not (the system discards the
   task's saved state) — that is the platform, not this code. And on a rotation,
   the text would also survive *without* the Bundle, because `SpikeState` is
   process-wide; the Bundle's independent value is the killed-process case, and
   the log line is what tells the two apart on the device.
5. **The ANR/Vivo background-kill class is out of scope** (v2.3 §3.4/§9): it is
   not this code, and nothing here changes it.
6. **Nothing ran on Android at runtime.** The `.so` compiles and links for
   `aarch64-linux-android` under `clippy -D warnings`, the Kotlin compiles and
   dexes, the APK is signed and well-formed, and every new JNI export is in the
   packaged library. No frame was drawn on a GPU, and no rotation was performed
   except in the state-machine tests.

---

## 11. The device pass (owner)

Install the new APK (it replaces v2.3 — same package, versionCode 6). Header must
read **`M13B v2.4 DIAGNOSTICS`** and the build line `0.3.0-m13b.2.4`; if it still
says `2.3`, the old APK is installed.

1. **Launch.** Cold start. Expect `event: resumed (#1) …`, `surface created for
   generation #1`, `first frame of this gpu generation presented`, and
   `kotlin: settle (onResume): …`.
2. **Rotate within 1 s of launch — three times.** Launch, and rotate
   portrait → landscape immediately (the v2.3 kill shape). Each time:
   * the app **must** come back (new Activity, same process — watch the pid in
     the log or the crash card);
   * the log must show `android_main entered (entry #N, activity generation N)`
     climbing, and **no** `NATIVE SIGNAL` line, no crash banner;
   * a healthy rotation may log `gpu: init aborted at stage '…'` if the destroy
     beat the build — that is the gate working, not a failure: the next entry
     builds.
3. **Rotate mid-session — three times.** Type a sentence in `1 · TEXT` and
   `lifecycle-123` in `2 · LIFECYCLE` first. After each rotate:
   * both texts still there; `resumes` continuing (process-wide);
   * `2 · LIFECYCLE` shows `instance state: N save(s) · M restore(s)` with
     **both counters above zero**, and `last restore: restored 13 probe char(s)
     + … text char(s) from the instance state`;
   * the log carries `kotlin: instance state: saved …` and `kotlin: instance
     state: handed … back to Rust`, then the Rust lines
     `probe restored from instance state (13 chars)`.
4. **Process kill.** Type text, press **Home**, then `adb shell am kill
   app.suramak.spike` (or wait for a low-memory kill), and reopen from recents.
   Expect the text back and the same `probe restored from instance state (13
   chars)` line — that path is the one only the Bundle can do.
5. **If it ever dies anyway**, the next launch's banner must now read
   `=== Suramak spike NATIVE SIGNAL ===` with `phase:` and the last 40 events.
   Screenshot it and share the log: that is the answer the v2.3 card could not
   give. Also grab `adb logcat -b crash` for the tombstone.
6. **Record** for the gate: pid before/after each rotate, `resumes`,
   `instance state` save/restore counts, every `aborted at stage` line, the
   panic count, and whether any banner appeared.

**Go if:** three fast rotations and three mid-session rotations leave the app in
the same live process with the text intact and the instance-state counters
climbing, no `NATIVE SIGNAL` block, and the kill test restores the text. If a
signal block *does* appear, that is a finding, not a NO-GO — it names the stage,
and the next pass starts from there.

---

## 12. Files

| File | Lines | Role |
|---|---|---|
| `crates/suramak-android-spike/src/diag.rs` | 1202 (was 686) | `Phase`, `SignalSnapshot`, handler, `init_signal_handlers`, 3 tests |
| `crates/suramak-android-spike/src/surface_lifecycle.rs` | 941 (was 620) | `Phase::Destroying`, `on_destroyed`, `InitStage`, `KillLatch`, 3 tests |
| `crates/suramak-android-spike/src/app.rs` | 1302 (was 1108) | destroy gate, per-stage latch, `GpuInitError`, phase publishing |
| `crates/suramak-android-spike/src/probe_state.rs` | 278 (new) | the payload format + 6 tests |
| `crates/suramak-android-spike/src/platform.rs` | 577 (was 449) | `window_gone`, `kill_pending`, `nativeSaveProbe`, `nativeRestoreProbe` |
| `crates/suramak-android-spike/src/ui.rs` | 1699 (was 1555) | instance-state counters/line, `restore_from_instance_state`, header v2.4, 1 test |
| `crates/suramak-android-spike/src/lib.rs` | 126 (was 107) | `probe_state`, signal handlers at entry |
| `android/…/spike/MainActivity.kt` | 684 (was 608) | save/restore pair, KDoc |
| `docs/reports/M13b6-spike-v2.4.md` | — | this report |
