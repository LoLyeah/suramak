# M13b v2.2 — the surface lifecycle: fresh surface per resume, no gpu work while suspended

**Status: built, packaged and statically verified on this VPS. The structure is
unit-tested off-device; the Mali queue-family behaviour it removes is only
observable on the owner's Vivo X300, so §8 is explicit about what is proven and
what the next device pass has to judge.**

Workorder: `docs/WORKORDER-M13B4.md`. Field evidence this pass answers: the v2.1
run on the owner's phone (`0.3.0-m13b.2.1`) — **the egui fix held (zero texture
panics)**, and the one remaining crash was
`wgpu-27.0.1/backend/wgpu_core.rs:3793: Surface::configure: Surface does not
support the adapter's queue family`, fired on resume. Previous build:
`docs/reports/M13b3-spike-v2.1.md`.

```
android/app/build/outputs/apk/debug/app-debug.apk
  20,942,463 bytes (19.97 MiB)
  sha256 a77991321d77e57e1c4a556f83e5c9e1b0bd2976c87290a204ebf9d185515a06
  versionCode 4 · versionName 0.3.0-m13b.2.2 · package app.suramak.spike
  lib/arm64-v8a/libsuramak_android_spike.so — 13,551,576 bytes (LOAD Align 0x4000)
```

---

## 1. What changed

| File | Change |
|---|---|
| `crates/suramak-android-spike/src/surface_lifecycle.rs` | **new (620 lines)** — the lifecycle policy: epochs, build tickets, the frame gate, the surface-fault → rebuild budget. No dependencies, so it compiles and is tested on this host; `app.rs` executes its decisions verbatim. **7 tests.** |
| `crates/suramak-android-spike/src/app.rs` | surface ownership + epoch adoption in `Gpu` (with a `Drop` log naming the inventory); every resume unconditionally drops and rebuilds; frame gate as the first act of `redraw`; retry/build refusal while suspended; Kotlin `surfaceDestroyed` hint; `Outdated`/`Lost`/`Timeout` → full rebuild, never `configure()`; `Gpu::new` inside a panic boundary; `Resized` no longer configures from the event handler |
| `crates/suramak-android-spike/src/platform.rs` | `take_surface_destroyed_hint()` — the Java `surfaceDestroyed` line, edge-triggered (no Kotlin change needed) |
| `crates/suramak-android-spike/src/ui.rs` | v2.2 header/build line; `frames_gated` counter in the lifecycle tab |
| `crates/suramak-android-spike/src/lib.rs` | module docs: v2.2 paragraph, `surface_lifecycle` export |
| `android/app/build.gradle.kts` | `versionCode 4`, `versionName 0.3.0-m13b.2.2` (comment updated; **nothing else**) |
| `scripts/android/build-apk.sh` | asserts versionCode 4 / versionName `0.3.0-m13b.2.2` |

No new dependency (`Cargo.toml` / `Cargo.lock` untouched — `git status` carries
neither). No Kotlin change, no Gradle/AGP change, no CI change, no change outside
`crates/suramak-android-spike/` plus the two version sites. The four checks are
untouched.

---

## 2. The crash, restated as three contract violations

The v2.1 trace (`WORKORDER-M13B4.md`, verbatim quotes):

| t | event |
|---|---|
| 0.18 s | `gpu ready attempt 1` + fresh egui registry + first frame presented ✓ |
| 3.29 s | Kotlin `onPause` |
| 3.32 s | Kotlin `surfaceDestroyed` |
| 3.40 s | winit `suspended` → surface/device/queue/renderer/egui context dropped |
| 3.40 s | **`gpu: retrying surface init (attempt 2 of 3)`** — a raced init during the suspend path |
| 3.58 s | `gpu ready (retry, attempt 2, 183 ms)` + egui ctx rebuilt — a surface created against the already-destroyed window |
| 11.91 s | real resume (`onStart`/`onResume`/insets) |
| 11.95 s | `surface outdated/lost — reconfiguring` → **configure panics on the stale Surface** (contained; footer `rebuilds 4 T4` on the surviving session) |

Three things had to be true at once for that to happen, and each is now closed:

1. **The surface outlived its epoch.** `wgpu::Surface` is bound to the
   `ANativeWindow` `create_surface` read; the *winit* `Window` is only a logical
   handle that re-reads `AndroidApp::native_window()` on every
   `raw_window_handle` call (`winit-0.30.13/src/platform_impl/android/mod.rs:1000`).
   The surface was therefore dead the moment the native window changed, but the
   resume only rebuilt when `self.gpu.is_none()` — and after the 3.58 s build it
   was `Some`.
2. **A gpu init ran while suspended.** The 3.40 s retry came from the
   event-loop turn after `suspended`, which is exactly what the frame/retry gate
   now refuses.
3. **`Outdated` reconfigured instead of rebuilding.** `configure()` on a surface
   whose window is gone is not a recovery; it is the panic.

### 2.1 Why the 3.32 s → 3.40 s gap exists at all

`android-activity` only clears its `native_window` **after** the
`APP_CMD_TERM_WINDOW` callback returns
(`android-activity-0.6.1/src/game_activity/mod.rs:613-618`), and winit turns that
callback into `Event::Suspended`
(`winit-0.30.13/src/platform_impl/android/mod.rs:217-218`). The Java
`surfaceDestroyed` callback runs *before* the looper reaches that command, so for
~80 ms in the field a build could still read a non-null `ANativeWindow` pointer
for a surface Java had already destroyed. That is the window the 3.58 s build
used. v2.2 closes it from the Kotlin line the shim already sends
(`nativeLogLine("surfaceDestroyed")`), so no Kotlin edit was needed.

---

## 3. Fix 1 — the surface joins the gpu generation, and the generation belongs to one epoch

`Gpu` now carries the `BuildTicket` it was built with (`serial`, `epoch`), and
its `Drop` prints the inventory:

```
gpu generation #3 (epoch 2) dropped — surface, device, queue, renderer and egui context released
```

The ownership is unchanged in shape (surface, device, queue, renderer, egui state
and the `egui::Context` are all fields of one `Gpu`), so `self.gpu = None` drops
all five together — surface included. What is new is that the *adoption* is
checked:

* `resumed` calls `lifecycle.on_resumed()` → a new epoch, and **always** drops
  the previous generation (no `if self.gpu.is_none()`). A healthy-looking gpu
  from a previous epoch is exactly the v2.1 bug.
* `begin_build()` is the only permission to construct a `Gpu`, and it is only
  issued while an epoch is open.
* `adopt(ticket)` refuses a ticket whose epoch has closed (a build that straddled
  a suspend), counting it as a stale build instead of making it live.
* `Gpu::new` creates the surface from the window handle it is handed at that
  moment — inside the resume that owns the build — and logs it:
  `surface created for generation #2 (epoch 2) from the current window handle`.

---

## 4. Fix 2 — no gpu work while suspended (the gate)

The gate is the first act of `redraw`, before timing, input or any gpu call:

```rust
if let Some(gated) = self.lifecycle.gate_frame() {
    let total = self.lifecycle.gated_frames();
    lock_state().frames_gated = total;
    if gated == 1 || gated % 120 == 0 {
        log::info!("frame gate tripped: [{SURFACE_GATE}] — redraw dropped ({gated} this suspend, \
                    {total} total); no gpu init or retry can start while suspended");
    }
    return;
}
```

`SURFACE_GATE` is
`surface-gate: suspended (no ANativeWindow; the next resume owns the rebuild)`.

Three more entry points ask the same policy:

| Entry | Guard |
|---|---|
| `maybe_retry_gpu` | `lifecycle.retry_allowed()` — false while suspended **and** after the fault budget is spent |
| `build_gpu` (every path: resume, retry, restart button, surface fault, post-panic) | `begin_build()` refuses while suspended; logs `gpu: build refused (reason=retry) — frame gate [...] is closed (lifecycle=SUSPENDED, refused builds: N)` |
| `about_to_wait` | no `request_redraw` while not running, so the loop does not even feed the gate |

Two additions that the trace specifically justified:

* **The Java hint.** `platform::take_surface_destroyed_hint()` closes the epoch on
  Kotlin's `surfaceDestroyed` line, one event-loop turn before winit's
  `Suspended` (§2.1). `surface_lifecycle` treats both signals as one transition
  (`suspends` counts once).
* **The build is inside a panic boundary.** `Gpu::new` ends in
  `surface.configure`, so a configure against a window that died mid-build would
  otherwise unwind through winit and finish the Activity. It is now
  `catch_unwind`-wrapped and reported as `gpu init panicked (...) — contained`.

The frame's observable counter is on the lifecycle tab:
`frames gated while suspended: N`. A non-zero N with no rebuild in between is the
fix working — in v2.1 that turn started a gpu init.

---

## 5. Fix 3 — a dead surface is rebuilt, never reconfigured

`Gpu::frame` no longer calls `configure()` on an error path. It *reports*:

```rust
Err(err) => {
    if let Some(fault) = surface_fault(&err) { return Some(fault); }
    log::error!("get_current_texture: {err}");
    return None;
}
```

`surface_fault` maps `Outdated`, `Lost` and `Timeout`; `OutOfMemory` and `Other`
stay on the log path (they are not lifecycle statements). The handler then:

```
rebuild (outdated) — fresh surface from the current window (surface outdated,
auto rebuild 1 of 2, epoch 3); the dead surface is never configured
```

…drops the whole generation and calls the *same* `build_gpu` the resume uses, so
the new surface comes from the current window and the new egui context comes with
it (`rebuild (outdated): fresh surface + fresh egui context ready`). The budget is
`MAX_AUTO_REBUILDS_PER_EVENT = 2`, reset by every `resumed` and by the manual
"Restart surface" button. On the third fault in one lifecycle event the path is
contained: the generation is dropped, the retry loop stays parked
(`is_exhausted()`), and the lifecycle tab carries
`surface outdated: automatic rebuild budget spent (2 of 2) — tap Restart surface`.

`WindowEvent::Resized` no longer configures either: it records `pending_size` and
the frame applies it, so the only two `surface.configure` call sites are
`Gpu::new` (inside the build's panic boundary) and `Gpu::frame` (inside the
frame's).

---

## 6. The unit tests — what each one pins

`app.rs` is `#[cfg(target_os = "android")]`, so nothing in it can run on this
host. The policy therefore lives in `surface_lifecycle.rs` and `app.rs` executes
it verbatim; the tests drive that same state machine (not a parallel model).

| Test | Contract it pins |
|---|---|
| `suspending_drops_every_gpu_scope_object_and_leaves_nothing_behind` | (a) the suspend drops **all five** gpu-scope objects — `SuspendReport::dropped` is `surface, device, queue, renderer, egui::Context` — leaves no live generation, and a second suspend has nothing to drop (no leak, no double drop) |
| `a_generation_built_for_an_ended_epoch_can_never_become_live` | the v2.1 bug itself: a build issued in epoch N cannot be adopted in epoch N+1 (the 3.40 s build vs the 11.91 s resume); the fresh epoch's build has a new serial |
| `a_generation_alive_at_resume_is_invalidated_and_reported` | a gpu still live at `resumed` is reported as invalidated instead of adopted |
| `a_frame_while_suspended_is_gated_and_cannot_start_gpu_work` | (b) a frame while suspended is counted by the gate, `begin_build` is refused and counted, `retry_allowed` is false; the next resume reopens the gate and resets the per-epoch count but keeps the process total |
| `the_java_surface_destroyed_hint_closes_the_epoch_before_winit_reports_it` | the Kotlin `surfaceDestroyed` signal and winit's later `Suspended` are one transition: gate shut immediately, one suspend counted |
| `surface_faults_rebuild_a_fresh_generation_and_stop_after_two` | (c) `Outdated`/`Lost` invalidate the generation and demand a new build (new serial), and the third fault is `Contained` with the retry loop parked; the manual path clears the budget |
| `the_auto_rebuild_budget_resets_on_the_next_lifecycle_event` | the cap is per lifecycle event, not per process |

`cargo test -p suramak-android-spike`: **30 passed** (23 in v2.1 + 7 here).
`cargo test --workspace`: **572 passed, 0 failed, 8 ignored** (baseline 565 →
+7).

What a host test structurally *cannot* do is create a `wgpu::Surface` (wgpu is an
Android-only dependency of this crate) or reach Mali's WSI. That boundary is §8.

---

## 7. Verification performed here

| Check | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 (clean) |
| `cargo clippy --workspace --all-targets -- -D warnings` | **exit 0** |
| `cargo test --workspace` | **572 passed, 0 failed, 8 ignored** (baseline 565 / 0 / 8 → **+7**) |
| `cargo test -p suramak-android-spike` | **30 passed** (23 in v2.1) |
| `cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-android-spike --all-targets -- -D warnings` | **exit 0** — the only gate that compiles `app.rs`/`platform.rs` |
| `./gradlew --no-daemon :app:compileDebugKotlin --rerun-tasks` | a **real** Kotlin compile (not `FROM-CACHE`): `BUILD SUCCESSFUL in 37s`, **zero `w:` lines** |
| `assembleDebug` task notes | one `stripDebugDebugSymbols` note: `Unable to strip … packaging them as they are` (the `.so` still carries `.symtab`). The build script and Gradle config are untouched, so this is the packaging step's own note, not a compiler warning — and **zero `w:`/`e:` lines** from Kotlin or javac |
| `bash scripts/android/build-apk.sh` | **exit 0**, all assertions passed (`SIZE: 20942463`, `SHA256: a7799132…`) |
| `aapt dump badging` | `package: name='app.suramak.spike' versionCode='4' versionName='0.3.0-m13b.2.2'`, `sdkVersion:'29'`, `targetSdkVersion:'35'`, `native-code: 'arm64-v8a'`, launchable `app.suramak.spike.MainActivity` |
| `aapt dump xmltree AndroidManifest.xml` | `enableOnBackInvokedCallback=0xffffffff`; `androidx.core.content.FileProvider` with authority `app.suramak.spike.fileprovider` (v2.1's crash-log share route intact) |
| `apksigner verify --print-certs` | verified, `CN=Android Debug` (`adb install -r` works) |
| `llvm-readelf -l` on the packaged `.so` | four `LOAD` segments, each `Align 0x4000` (16 KB pages) |
| `strings` on the packaged `.so` | carries the v2.2 lines: `frame gate tripped`, `surface-gate: suspended`, `rebuild (outdated)` + `fresh surface from the current window (surface`, `surface created for generation`, `gpu generation #`, `surfaceDestroyed (Kotlin)`, `build refused`, `fresh surface + fresh egui context ready`, `no gpu init or retry can start while suspended` |
| `unzip -l` | `lib/arm64-v8a/libsuramak_android_spike.so` (13,551,576 bytes) + `AndroidManifest.xml` |

`strings` note: the sentence `Surface does not support the adapter's queue
family` is still present in the binary — it is wgpu's own error string, compiled
in with the crate. Our code no longer has a path that configures a surface whose
window is gone; the string is not evidence of a call site.

---

## 8. The honest boundary

1. **The Mali/WSI queue-family probe is owner-device-only.** This pass proves the
   *structure*: the surface is dropped with its generation, no generation can be
   adopted by an epoch it was not built in, no build or frame can start while
   suspended, and a fault produces a new surface from the current window instead
   of a `configure()`. It cannot prove that Mali's `VkSurfaceKHR` creation against
   a freshly created `ANativeWindow` succeeds — that is exactly what the owner's
   next pass measures. **Nobody has drawn a frame on a GPU in this pass.**
2. **The 80 ms Java→winit gap is reasoned, not measured, on the device.** The
   delivery order is read from `android-activity`/`winit` source (§2.1) and the
   gap from the owner's own timestamps, but whether the Vivo delivers
   `surfaceDestroyed` before the looper command on every round-trip is a device
   fact. If it does not, the winit `suspended` gate is still in place; if both
   are late, `Gpu::new`'s panic boundary contains the failure and the log says
   `gpu init panicked`.
3. **"Rebuild count stays small" has a precise meaning now.** Because every
   resume deliberately creates a fresh surface, the footer's
   `rebuilds (surfaced) N` is expected to track the resume count (one per
   resume), **not** stay at 1. What must *not* happen is rebuilds growing between
   resumes (the v2.1 `rebuilds 4 T4`: four generations for two resumes). The
   device verdict is: `rebuilds == resumes` (or resumes − 1 if a build failed) and
   no `rebuild (outdated)` line unless the driver really dropped a live surface.
4. **The gate's trip count is not a quality metric.** A handful of gated frames
   per backgrounding is normal (events arriving before the loop parks). Thousands
   would mean the loop is not parked — also visible as battery drain.
5. **`egui` per-context state is still reset on every rebuild** (caret, scroll,
   hover; §2.3 of the v2.1 report). The probe text and counters survive because
   they are app state — that is check (2)'s subject, unchanged.
6. **Nothing ran on Android at runtime.** The `.so` compiles and links for
   `aarch64-linux-android` under `clippy -D warnings`, the Kotlin compiles and
   dexes, the APK is signed and well-formed.

---

## 9. The device pass (owner)

Install the new APK, then:

1. **Identity.** Header `M13B v2.2 DIAGNOSTICS`, `build 0.3.0-m13b.2.2`. If it
   still says `v2.1` / `0.3.0-m13b.2.1`, the old APK is installed.
2. **Launch.** Cold start on the Vivo X300 / Android 16. Expect, in order:
   `event: resumed (#1) — surface epoch 1 open, frame gate open`,
   `surface created for generation #1 (epoch 1) from the current window handle`,
   `gpu ready (resume, attempt 1, … ms, generation #1, epoch 1): <adapter>`,
   `first frame of this gpu generation presented — full repaint`.
   Zero panic lines; no crash banner.
3. **Home → reopen, once.** Expect:
   - `event: surfaceDestroyed (Kotlin) — surface epoch 1 closed early, frame gate closed (had gpu: true)`
   - `gpu generation #1 (epoch 1) dropped — surface, device, queue, renderer and egui context released`
   - optionally `frame gate tripped: [surface-gate: suspended …] — redraw dropped (1 this suspend, 1 total)`
   - then on return `event: resumed (#2) — surface epoch 2 open, frame gate open`,
     `surface created for generation #2 (epoch 2) from the current window handle`,
     `gpu ready (resume, attempt 1, … ms, generation #2, epoch 2)`,
     `first frame of this gpu generation presented — full repaint`.
   - **No** `Surface does not support the adapter's queue family`, **no** `frame
     panicked`, **no** `retrying surface init` while the app was in the
     background.
4. **Repeat Home → reopen five times.** After each return: the first frame is
   presented (no black screen), the probe text is still there, `resumes`
   increments, `frames gated while suspended` grows only by a few, and the footer
   reads `gpu: <adapter> · restarts 0 · rebuilds (surfaced) N · T{resumes}` with
   **N == resumes** (one fresh surface per resume by design) and flat between
   resumes.
5. **Optional, the old failure mode on demand.** "Restart surface" (lifecycle tab
   or the log panel's `↻ surface`): expect `surface restart #1 … dropping the
   whole gpu generation`, a new generation number, `first frame of this gpu
   generation presented`, and `restarts 1`. No panic.
6. **Record** for the gate: `resumes`, `rebuilds`, `frames gated`, `restarts`,
   the panic count in the header (must be 0), and any `rebuild (outdated)` line
   with its auto-rebuild index. If a crash does occur, `Share log` on the crash
   banner sends `spike-crash.txt` — the new lines are the ones quoted above.

Go if: after five Home round-trips there is no panic, the first frame presents
every time, `restarts 0`, and `rebuilds` equals `resumes` with no extra rebuild
between them.

---

## 10. Files

| File | Lines | Role |
|---|---|---|
| `crates/suramak-android-spike/src/surface_lifecycle.rs` | 620 | **new** — epochs, tickets, gate, fault budget, 7 tests |
| `crates/suramak-android-spike/src/app.rs` | 1117 (was 779) | surface ownership + adoption, unconditional resume rebuild, gate, Kotlin hint, fault → rebuild, build panic boundary |
| `crates/suramak-android-spike/src/platform.rs` | 421 | `take_surface_destroyed_hint()` |
| `crates/suramak-android-spike/src/ui.rs` | 1425 | v2.2 header, `frames gated` counter |
| `crates/suramak-android-spike/src/lib.rs` | 98 | v2.2 module paragraph, export |
| `docs/reports/M13b4-spike-v2.2.md` | — | this report |
