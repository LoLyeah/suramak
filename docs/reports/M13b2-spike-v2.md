# M13b v2 — the Android spike as a self-diagnostics rig

**Status: built, packaged and statically verified on this VPS. It has not been
run on a phone** — there is no device and no emulator here, and the whole point
of this slice is what happens on the owner's Vivo X300. What is proven below:
the APK exists with the right identity and JNI surface, every code path that can
fail now records itself, and the Rust/Kotlin sources compile for
`aarch64-linux-android` with `clippy -D warnings`.

Workorder: `docs/WORKORDER-M13B2.md`. Owner verdict this answers:
`docs/m13-gate.md` §8. Previous build: `docs/reports/M13b.md`. Owner checklist
(now with a v2 section): `docs/m13-gate.md` §9.

```
android/app/build/outputs/apk/debug/app-debug.apk
  20,910,067 bytes (19 MiB)
  sha256 20f28703b6ae14bf1f5a08ca6fba4008c18194022b8d081cd9002881bda3741e
  versionCode 2 · versionName 0.3.0-m13b.2 · package app.suramak.spike
  lib/arm64-v8a/libsuramak_android_spike.so — 13,536,240 bytes
```

---

## 1. What changed

| File | Change |
|---|---|
| `crates/suramak-android-spike/src/diag.rs` | **new** — crash record, panic hook, event ring, crash files, UTC formatting |
| `crates/suramak-android-spike/src/platform.rs` | six panic-proof JNI exports (three new), defensive input checks, activity generations |
| `crates/suramak-android-spike/src/app.rs` | lifecycle fixes (destroy → exit, suspend → `Wait`), surface restart, GPU retry, frame-level panic containment, log bridge |
| `crates/suramak-android-spike/src/ui.rs` | crash banner, log panel, per-tab isolation + reset, "Restart surface", v2 header |
| `crates/suramak-android-spike/src/lib.rs` | panic containment + post-panic looper pump on the `android_main` boundary |
| `android/…/spike/MainActivity.kt` | crash overlay, lifecycle/surface logging, `nativeInit`, destroy signal, back behaviour after a native death |
| `android/…/spike/CrashFiles.kt` | **new** — the shared crash/session files, Java-side fatal records |
| `android/…/spike/SpikeApp.kt` | **new** — `Application`, installs the Java uncaught-exception recorder |
| `android/app/src/main/AndroidManifest.xml` | `android:name=".SpikeApp"` |
| `android/app/build.gradle.kts` | `versionCode 2`, `versionName 0.3.0-m13b.2` |
| `scripts/android/build-apk.sh` | asserts the new `versionName` (with a narrower fallback so a versionCode bump cannot hide a regression) |
| `android/.gitignore` | ignores AGP 8.7's `.kotlin/` session dir |

No new dependencies (the spike still runs on egui + wgpu + winit + jni +
`android_logger` + `ndk-context`). No changes to `suramak-asr`, `suramak-core`,
`suramak-ui` or `ci.yml`. No Gradle/AGP churn.

---

## 2. The instrumentation, item by item

### 2.1 Panic hook (`diag.rs`)

A process-wide `Mutex<Diag>` holds the event ring and the live crash record; a
`OnceLock<PathBuf>` holds the files dir separately, because the hook must still
reach the file when the diag lock is the thing that is poisoned.

On panic the hook writes one block:

```
=== Suramak spike crash #3 ===
when:   2026-10-21 06:41:12Z (uptime 12.44s)
thread: main
where:  crates/suramak-android-spike/src/app.rs:412:9
panic:  called `Option::unwrap()` on a `None` value
--- last 40 platform events (oldest first) ---
[    0.31s] info jni: nativeInit files dir = /data/user/0/app.suramak.spike/files
[    0.33s] info kotlin: onCreate
[    1.02s] info event: resumed (#1)
…
```

The event run-up is the part that makes an unreproducible failure legible: it
shows what the bridge was doing immediately before the panic.

Mechanics that matter:

- **Two files.** `files/spike-last-crash.txt` is replaced atomically (write to
  `.tmp`, `sync_all`, `rename`) so a reader never sees half a record;
  `files/spike-crash-log.txt` is append-only history, rolled over at 256 KiB to
  the newest ~48 KiB on a record boundary. Both are deleted only by **Clear
  crash**.
- **The hook chains.** `panic::set_hook` wraps the previous hook and calls it
  after recording, so logcat and the platform's own crash reporting are
  unchanged.
- **It cannot recurse.** No logging, no allocation beyond the message, every I/O
  error ignored, and the diag lock is taken with `try_lock` — a panic that
  happens *inside* a diagnostics call leaves the hook writing message + location
  with an empty run-up instead of deadlocking the process. Everything the block
  needs (including the uptime) is captured under that one guard: the first
  version called `uptime_secs()` from inside `crash_block`, i.e. re-locked the
  mutex it already held, and the headless draw test hung on it (see §7).
- **Wall clock without a dependency.** `utc_stamp` is Hinnant's
  `civil_from_days`; unit tests pin 1970-01-01, 2023-11-14T22:13:20Z, a leap day
  and a year end.

### 2.2 Crash-proof JNI exports (`platform.rs`)

All six `Java_app_suramak_spike_MainActivity_*` exports run inside
`jni_guard(name, body)` → `catch_unwind(AssertUnwindSafe(...))`; a panic is
recorded, marked `contained: <export> — the process kept running`, and the call
returns normally. Without this, a panic leaving an `extern "system"` function is
UB, and Rust ≥ 1.81 aborts the process instead — on Android that is
indistinguishable from "the app closed itself".

| Export | Defensive handling |
|---|---|
| `nativeInit(filesDir)` | null/empty rejected (persistence off, log line), first call wins, installs the hook |
| `nativeLogLine(line)` | null rejected, one line enforced, 400-char cap |
| `nativeSetInsets(5×i32)` | each value clamped to `0..=100_000` with a warning line naming the field |
| `nativeBackPressed()` | saturating counter, logs the queued count |
| `nativeImeText(text, start, end)` | null env/string ignored, `get_string` failure logged, >256 KiB truncated on a char boundary, negative/out-of-range selections clamp to 0/len |
| `nativeActivityDestroyed()` | idempotent, generation-scoped (see §4) |

`nativeImeText` also logs `chars=N, sel=start..end` plus a 32-char preview for
every callback — that is the on-screen evidence that the IME bridge is alive.

### 2.3 Persistence and display

- **Kotlin overlay (survives a dead Rust surface).** `addContentView` puts a
  non-focusable panel over the GameActivity surface: title
  `⚠ CRASH RECORDED — LAST SESSION` (or `⚠ NATIVE CRASH — THIS SESSION`), the full
  record in monospace, the file paths, and `Close app` / `Hide` buttons. It is
  built at launch if the file exists and rebuilt in place when Rust reports a new
  panic. `Hide` hides it until the next launch; only **Clear crash** deletes.
- **egui red banner (live view).** Fed by `diag::crash_for_display()` on a
  revision counter: `CRASH — THIS SESSION (caught, app still running)` or
  `CRASH — LAST LAUNCH (read from the crash file)`, with **Clear crash**.
- **A third recorder, because process death is not always a panic.**
  `SpikeApp` installs a default uncaught-exception handler that writes the same
  file for Java-side deaths, and `CrashFiles` keeps a session marker that is
  written on create/resume and deleted on stop. A marker left behind means the
  process vanished while visible — SIGSEGV, SIGABRT, an ANR kill — and the next
  launch says so explicitly, because no Rust panic and no Java exception
  recorded it.

### 2.4 The on-screen log ring

`diag::push` keeps the last 40 lines; `app.rs` installs a `log::Log` that
forwards everything to logcat and mirrors only `suramak*` targets into the ring
(wgpu/winit info-level chatter would otherwise evict every platform event). The
panel is a bottom panel with a one-line header that is **always** visible —
`▸ LOG — 40 events` plus the newest line — expanding to a 150 pt scroll area
with `[   12.34s] warn surface outdated/lost — reconfiguring` rows, colour-coded
by level, with a dropped-line counter and a `↻ surface` shortcut.

What lands in it: every JNI callback (IME text + length + selection, inset
values, back presses), `onCreate/onStart/onResume/onPause/onStop/onDestroy`,
`surfaceCreated/surfaceChanged/surfaceDestroyed`, `onWindowFocusChanged`, every
winit event the app handles (resumed, suspended, resize, focus, close), GPU
adapter/format/size on init, every wgpu surface error, every surface restart with
its reason, and every contained panic.

### 2.5 Isolation and recovery

- **Tab isolation.** Each tab is drawn inside its own `catch_unwind`. A panic
  disables *that tab only*: the tab body is replaced by an error card with the
  panic text and `Retry this tab` / `Reset this check`, the tab button grows a
  `!`, and the other three checks keep drawing. `reset_tab` clears one check's
  data and leaves the others alone (unit-tested).
- **Restart surface.** A lifecycle-tab button (and the log panel's `↻ surface`)
  drops `Gpu` — surface, device, queue, renderer, `egui_winit::State` — and
  rebuilds against the current window, counting attempts and logging each step,
  including the failure reason. This makes check 2 repeatable on demand instead
  of requiring an app switch.
- **GPU retry.** A failed init is retried twice at 2 s intervals before giving
  up; while retrying the loop idles on `ControlFlow::Wait`, so a dead GPU does
  not spin a core.
- **Frame containment.** `gpu.frame(..)` runs inside `catch_unwind`; a panic
  there drops the poisoned surface, records the reason in the verdict strip and
  waits for a restart instead of unwinding into winit.

### 2.6 Version marker

`versionName 0.3.0-m13b.2`, `versionCode 2`, header **M13B v2 DIAGNOSTICS**,
subtitle line `build 0.3.0-m13b.2 · crash text survives a restart`.

---

## 3. Why v1 "opens then immediately closes" — the mechanism

This is the finding the workorder asked for, and it is mechanical, not
speculative.

`android-activity` runs `android_main` inside its own guard and then **always**
finishes the Activity:

```rust
// android-activity-0.6.1/src/game_activity/mod.rs, _rust_glue_entry
// "We want to specifically catch any panic from the application's android_main
//  so we can finish + destroy the Activity gracefully via the JVM"
catch_unwind(|| { android_main(app); })
    .unwrap_or_else(log_panic);

ffi::GameActivity_finish((*game_activity_glue).activity);
```

So **any unwinding panic on the `android_main` thread closes the Activity** — the
window appears and disappears, and the reason goes only to logcat. v1 had no
panic containment on that thread, no hook, and no file, so every failure looked
identical from the owner's side: "it opens and immediately closes again". The
Activity is the app's only component, so it relaunches the same way until a
force-stop clears the process. That single sentence explains the §8 report
without needing to guess which call panicked.

The plausible panic sources in v1 are exactly the ones that were broken: the IME
bridge (JNI `get_string` / selection arithmetic on every keystroke), the insets
push (called from `onResume` and every inset change), and `Activity.recreate()`
through the shim. v2 does not fix them — it makes them say their name, on
screen, in this session and the next one.

---

## 4. Lifecycle audit of v1 — was it leaking the surface?

**No. It was leaking the event loop.** Both halves matter, so both are recorded
with the evidence.

| Question | v1 | Verdict |
|---|---|---|
| Are GPU objects dropped on suspend? | `suspended()` sets `self.gpu = None`, dropping surface, device, queue, renderer and `egui_winit::State` | **Correct** — no surface leak |
| Is keeping the `Window` across suspend a stale-handle bug? | winit's Android `Window` is `{ app: AndroidApp, redraw_requester }`; `raw_window_handle_rwh_06` re-reads `app.native_window()` and returns `Err(Unavailable)` while suspended | **Not a bug** — the "prime suspect" is clean. The `Window` is a logical handle, not an `ANativeWindow` |
| Does the loop stop when the Activity is destroyed? | No. winit 0.30.13 comments `MainEvent::Destroy` out (`warn!("TODO: forward onDestroy notification to application")`) and `run_on_demand` only returns after `exit()` | **Leak** — one `android_main` thread per recreation survives the Activity it served |
| What does that thread hold? | The `EventLoop` (an `AndroidApp` clone → **JNI global ref to the destroyed Activity**, plus a looper waker) and a `ControlFlow::Poll` loop with `gpu == None` | Thread + global ref leaked, **and a core spun at 100 %** |
| What happens while backgrounded? | Still `Poll`; `about_to_wait` only requests redraws when a GPU exists, so a suspended app spins with no work | **Bug** — battery/thermal cost on every background trip |

The compounding story for §8: each rotation, `Activity.recreate()` or relaunch
left another spinning `android_main` in the same process. `singleTask` means a
relaunch reuses that process, so the zombie count grows, each holding a global
ref to a dead Activity and burning a core — on a phone that reads as "the app
refuses to open, and force-stopping it fixes it".

Fixed in v2:

1. `MainActivity.onDestroy` calls `nativeActivityDestroyed()`; the handler exits
   the loop at its next `about_to_wait`, `run_app` returns, `android_main`
   returns, and `android-activity` drops the `AndroidApp` (the documented
   contract: "You should return from `android_main()` as soon as possible if you
   receive a `MainEvent::Destroy` event").
2. Generation-scoped: each `android_main` claims an id and a destroyed signal
   only applies to its own generation, so a stale signal cannot kill the
   Activity that replaced it (the naive `AtomicBool` version would have made
   every later launch exit instantly — the bug is unit-visible, so it is
   designed out).
3. `suspended` parks the loop on `ControlFlow::Wait`; `resumed` restores `Poll`.
4. `discard`-style guard on frame timing: a dt above 0.5 s is a suspend gap, not
   a frame, so the fps readout no longer shows a bogus sub-1 fps for two seconds
   after every resume.

---

## 5. What the owner will see, element by element

| Element | When | What it shows |
|---|---|---|
| Kotlin red panel (top ~55 %) | a crash file exists at launch, or Rust reports a live panic | title (`LAST SESSION` / `THIS SESSION`), the full record (message, thread, location, timestamp, the last 40 events), the file paths, `Close app` / `Hide` |
| egui red banner | same, inside the app | `CRASH — …` + the same text, scrollable, with **Clear crash** (the only delete) |
| `LOG` row (bottom, always visible) | always | `▸ LOG — 40 events` + the newest line collapsed; tap for 40 timestamped rows |
| Header | always | **M13B v2 DIAGNOSTICS**, build string, `N panic(s) this session` in red when non-zero |
| Verdict strip | always | the four verdicts, `gpu: <adapter> · restarts N · rebuilds N T<n> · uptime`, and the current GPU error |
| `2 · LIFECYCLE` tab | on demand | resumes / surface rebuilds / surface restarts / recreate requests + **Restart surface** |
| Per-tab error card | a tab panics | `… is disabled after a panic`, the panic text, `Retry` / `Reset` |

---

## 6. `AssertUnwindSafe` — why it is sound here

Four sites: the JNI exports, the frame, the tab draw, and the `android_main`
boundary. The justification is the same in all four and rests on what the
closures actually touch:

1. **Only poison-tolerant globals.** Every shared value is a counter, a `String`,
   or a `Vec`/`VecDeque` behind a `Mutex` taken with
   `unwrap_or_else(|p| p.into_inner())`. A panic mid-update leaves a stale number
   or a lost inset push; the next callback overwrites it. Nothing observes a
   half-written value across a lock boundary.
2. **The failure is terminal for the affected object.** After a caught frame
   panic the `Gpu` is dropped and nothing uses it again; after a caught
   `android_main` panic the event loop is already gone and only an `AndroidApp`
   handle is reused (an `Arc`, not borrowed state).
3. **egui's transient state is per-frame.** A tab that panics mid-layout leaves a
   partially laid-out `Ui`, which `Context::run` discards at the next frame; the
   tab is disabled before that frame can repeat the panic.
4. **The alternative is worse and unsound.** Unwinding out of `extern "system"`
   is UB (and aborts on Rust ≥ 1.81); unwinding out of `android_main` closes the
   Activity. Between a documented, reasoned `AssertUnwindSafe` and either of
   those, this is the safe choice — and it is the only choice that keeps the
   evidence on screen.

---

## 7. Verification performed here

| Check | Result |
|---|---|
| `bash scripts/android/build-apk.sh` | **exit 0** — `SIZE: 20910067`, `SHA256: 20f28703…` |
| `aapt dump badging` | `package: name='app.suramak.spike' versionCode='2' versionName='0.3.0-m13b.2'`, `sdkVersion:'29'`, `targetSdkVersion:'35'`, `native-code: 'arm64-v8a'`, launchable `app.suramak.spike.MainActivity` |
| `apksigner verify --print-certs` | verified, `CN=Android Debug` (so `adb install -r` works with no signing setup) |
| `llvm-nm -D --defined-only` | `android_main`, `…_nativeInit`, `…_nativeLogLine`, `…_nativeSetInsets`, `…_nativeBackPressed`, `…_nativeImeText`, `…_nativeActivityDestroyed`, `Java_com_google_androidgamesdk_GameActivity_initializeNativeCode` |
| `dexdump` | the DEX declares all six natives with the descriptors the Rust exports expect (`nativeInit:(Ljava/lang/String;)V`, `nativeLogLine:(Ljava/lang/String;)V`, `nativeActivityDestroyed:()V`, plus the three v1 signatures); `postRecreate`/`postShowCrashOverlay` exist as the Rust→Kotlin callbacks |
| `aapt dump xmltree` | `android:name="app.suramak.spike.SpikeApp"` on `<application>`; `configChanges=0x70`, `launchMode=0x2`, min 29 / target 35 preserved |
| `llvm-readelf -l` | every `LOAD` segment `Align 0x4000` (16 KB pages) |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **511 passed, 0 failed, 8 ignored** (24 binaries; the spike crate contributes 19 of them — the balance includes the concurrent M14B2 audio work's tests, which are not part of this slice) |
| `cargo test -p suramak-android-spike` | 19 passed: crash-file round trip + history trim, ring cap/drop/one-line, crash-block content, UTC dates, panic-hook deadlock probe, inset clamp, per-tab reset/isolation, suspend-gap frame filter, and **headless draws of the whole screen** (all four tabs × log open/closed × crash banner present) — the panels and tab bodies execute in the test, so a UI panic is caught here rather than on the phone. 2 further tests live in `platform.rs` and only build on the Android target |
| `cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-android-spike --all-targets -- -D warnings` | clean — this is the only gate that sees `platform.rs`, and it caught a **pre-existing** `type_complexity` lint on v1's `drain()` return type (host clippy cannot see android-gated code) |
| APK contents | `lib/arm64-v8a/libsuramak_android_spike.so` (13,536,240 B) + `AndroidManifest.xml`; size 19 MiB, inside the plan's 25 MB budget |

**What the verification caught in the rig itself.** The headless draw test was
not decoration: its first run hung, and the cause was real — the panic hook
called `uptime_secs()` from `crash_block`, i.e. took the diagnostics mutex a
second time on the same thread, inside the hook. On the phone that would have
frozen the `android_main` thread on the first panic instead of recording it: a
worse outcome than v1, and invisible without a device. Fixed by capturing the
uptime under the single `try_lock` guard, and pinned by
`the_panic_hook_records_without_deadlocking`, which fails on a 5 s channel
timeout instead of hanging the suite. This is the one bug in this slice that
would have shipped silently without a host-side execution gate.

---

## 8. The honest boundary

Everything above is static: the APK is well-formed, signed, carries the right
manifest, ABI and JNI surface, and the sources compile for the target. **Nothing
here says the app launches on the Vivo, or that the crash record is readable
when it does.** The device is the only place these can be answered:

1. **Does the overlay draw over the SurfaceView?** GameActivity uses the default
   surface z-order (no `setZOrder*` in its bytecode), so window-level views draw
   on top — but that is a bytecode-level inference, not a photograph.
2. **Does `onDestroy` arrive before android-activity waits for the native
   thread?** It should (`super.onDestroy` terminates the native code afterwards),
   and the loop exits within one frame, but the join timeout is the vendor's.
3. **Is `ControlFlow::Wait` resumed correctly after a real suspend?** The
   `MainEvent::Resume` path is what wakes it; only the phone proves it.
4. **Does a real panic actually reach the hook before `android-activity`'s
   `catch_unwind`?** The hook runs first by construction, but the file write
   happens on a dying thread and has never been exercised on a device.
5. **Does the IME bridge still behave as it did in v1?** Unchanged code, but the
   ring now logs every callback, which is how we find out.

Two failure modes v2 cannot cover, stated plainly:

- **SIGSEGV / SIGKILL inside native code** — no hook runs. The session marker is
  the only trace, and it says "the previous session vanished" without a stack.
  A fault handler is possible (a signal-safe writer) but is a different
  workorder, and Android's own tombstone is the better artifact there.
- **A panic during `System.loadLibrary` or before `onCreate`** — the files dir is
  not known yet, so nothing is persisted. `SpikeApp` narrows this window; it
  cannot close it.

---

## 9. Files

Added:

- `crates/suramak-android-spike/src/diag.rs`
- `android/app/src/main/java/app/suramak/spike/CrashFiles.kt`
- `android/app/src/main/java/app/suramak/spike/SpikeApp.kt`
- `docs/reports/M13b2-spike-v2.md` (this file)

Modified:

- `crates/suramak-android-spike/src/{lib.rs, app.rs, platform.rs, ui.rs}`
- `android/app/src/main/java/app/suramak/spike/MainActivity.kt`
- `android/app/src/main/AndroidManifest.xml`
- `android/app/build.gradle.kts`
- `android/.gitignore`
- `scripts/android/build-apk.sh`
- `docs/m13-gate.md` (§9 — how to read the v2 diagnostics)

Generated, not committed: `android/app/build/`, `android/.gradle/`,
`android/.kotlin/`, `android/local.properties`,
`android/app/src/main/jniLibs/arm64-v8a/*.so`.

---

## 10. What the phone session has to answer

In order of value:

1. **Does it stay open?** Launch, leave it for 30 s, come back. If it closes,
   the next launch's red panel is the report.
2. **Screenshot the LOG after a home → return → rotate cycle.** That single
   screenshot carries the resume counter, the surface timeline and any wgpu
   error — it replaces logcat for check 2.
3. **Press "Restart surface" three times.** If the wedge is in the drop/rebuild
   path, this finds it without app switching, and the failure text stays on
   screen.
4. **Then the four checks** as written in `docs/m13-gate.md` §3–§6.

If the same failure repeats more than four times, stop and send the red panel
text plus the expanded LOG — per the workorder, that is the point at which the
next step is a design decision, not another build.
