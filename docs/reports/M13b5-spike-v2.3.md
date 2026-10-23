# M13b v2.3 — the recreate edge: a named limitation, no Java call

**Status: built, packaged and statically verified on this VPS. The recreate path
no longer makes a JNI/Java call of any kind, which is provable off-device; that
the ANR is gone is owner-device-only, and §9 says exactly why.**

Workorder: `docs/WORKORDER-M13B5.md`. Field evidence this pass answers: the v2.2
run on the owner's phone (`0.3.0-m13b.2.2`, vivo V2515 / Android 16) — **the
surface fix held** (3 Home round-trips in one process, `resumes` 1→2→3,
`rebuilds` 0→2, zero surface panics, TEXT 42 chars/76 IME/41 deletions, MOTION
90.5–102.9 fps, back 5×, IME inset on keyboard open), and the one remaining
defect was the session that tapped "Recreate activity now" and was later
**ANR-killed while suspended** ("previous process vanished without onExit. No Rust
panic and no Java exception recorded for it: ANR-kill", pid 1554). Previous
builds: `docs/reports/M13b3-spike-v2.1.md`, `docs/reports/M13b4-spike-v2.2.md`.

```
android/app/build/outputs/apk/debug/app-debug.apk
  20,947,967 bytes (19.97 MiB)
  sha256 d1aae74d916fe84729a45638d40bd193e0ca446632964b6daaaa894870490970
  versionCode 5 · versionName 0.3.0-m13b.2.3 · package app.suramak.spike
  lib/arm64-v8a/libsuramak_android_spike.so — 13,557,080 bytes (LOAD Align 0x4000)
```

---

## 1. What changed

| File | Change |
|---|---|
| `crates/suramak-android-spike/src/ui.rs` | `RecreateStatus` gains `java_call: Option<&'static str>` (always `None`); new `RECREATE_UNAVAILABLE` + `RECREATE_EVIDENCE`; `note_recreate()` → `tap_recreate()` (local state only); `recreate_requested` **deleted**; lifecycle tab rewritten around the named limitation; v2.3 header/build line; the legible-error test replaced and a second test added |
| `crates/suramak-android-spike/src/app.rs` | the `recreate` tuple element, the `state.recreate_requested` take and the whole `request_activity_recreate()` branch **deleted**; v2.3 module paragraph |
| `crates/suramak-android-spike/src/platform.rs` | `request_activity_recreate()` **deleted**; `call_activity()` → `call_shim()` with the Application-vs-Activity finding documented at the call site; error text now names the object it really called |
| `crates/suramak-android-spike/src/lib.rs` | v2.3 module paragraph; the crate blurb no longer claims a recreate trigger |
| `android/…/spike/MainActivity.kt` | `postRecreate()` **deleted** (and with it the only `recreate()` call site in the repo); KDoc JNI contract rewritten; `logSettle()` + calls on `onResume`/`onStop` |
| `android/app/build.gradle.kts` | `versionCode 5`, `versionName 0.3.0-m13b.2.3` (comment updated; **nothing else**) |
| `scripts/android/build-apk.sh` | asserts versionCode 5 / versionName `0.3.0-m13b.2.3` |

No new dependency (`Cargo.toml` / `Cargo.lock` untouched). No Gradle/AGP change,
no CI change, no change outside `crates/suramak-android-spike/` + `android/`
(the manifest, resources, theme and the four checks are untouched).

---

## 2. The field evidence, restated exactly

| Evidence | What it says |
|---|---|
| `activity recreate failed: MainActivity.postRecreate(): Java exception was thrown` (v2 and v2.1, unchanged across the v2.1 Kotlin rewrite) | a Java exception was **pending** when the JNI call returned — and the wording never changed, which is the clue in §3.1 |
| the session that tapped Recreate was later ANR-killed after Home | the OS watchdog killed the process; the kill left no in-process record (no Rust panic, no Java exception, no `onExit`) |
| sessions with **no** recreate tap survived 3+ Home round-trips in one live process | backgrounding alone is not the trigger; the tap is |

The v2.1 report (§4) reasoned from the same sentence that the exception came from
inside the posted `recreate()` call and fixed the Kotlin body accordingly. The
field then produced **the identical sentence** from a body that could not throw
before `post` returned. That contradiction is what v2.3 resolves.

---

## 3. Root cause: the shim calls a method on the **Application**, not the Activity

### 3.1 The sentence is a *lookup* failure, not an Activity failure

`platform.rs` called `env.call_method(&activity, "postRecreate", "()V", &[])`.
In `jni` 0.21.1 that path is `call_method` → `get_method_id_base` →
`jni_non_null_call!(GetMethodID)`, and `jni_non_null_call!` expands to
`jni_non_void_call!` followed by `non_null!`. `jni_non_void_call!` ends with
`check_exception!`, which does **not** inspect the return value first:

```rust
// jni-0.21.1/src/wrapper/macros.rs:84
if check { return Err($crate::errors::Error::JavaException); }
```

and `Error::JavaException`'s Display is `"Java exception was thrown"`
(`wrapper/errors.rs:22`). So a `GetMethodID` that finds nothing — which throws
`NoSuchMethodError` in the VM and returns null — is reported as
`Java exception was thrown`, *before* any invocation happens.

Corroboration from the field, not from theory: v2.1 replaced
`root?.post { recreate() }` with an explicit `Handler(Looper.getMainLooper())`
plus `try/catch (Throwable)` that logs `recreate FAILED on the UI thread: …`.
If anything in that body had thrown, the ring and the status line would have
carried the real class and message. Neither ever appeared; the JNI-level sentence
was byte-identical to v2's. The throw therefore precedes every Kotlin line.

### 3.2 The object is the Application

`android-activity` 0.6.1 does not put the Activity in `ndk-context`:

```rust
// android-activity-0.6.1/src/init.rs:219
fn get_application(env, activity) { activity.call_method("getApplication", …, &[]) }
// android-activity-0.6.1/src/init.rs:275-285
let application = get_application(env, jni_activity)?;
let app_global  = env.new_global_ref(application)?;
ndk_context::initialize_android_context(vm.get_raw().cast(), app_global.cast());
```

and `ndk-context` 0.1.1 documents its own handle as *"In most cases this will be
a ptr to an `Activity`, but this isn't guaranteed"* (`src/lib.rs`, `context()`).

Chain: `GetMethodID(SpikeApp, "postRecreate", "()V")` → `NoSuchMethodError` →
`jni::Error::JavaException` → `MainActivity.postRecreate(): Java exception was
thrown`. The `MainActivity.` prefix was **our own error string**, which is why the
message read like a failure *on the Activity* for three builds. The `call_method`
target is a global ref to `SpikeApp`; `MainActivity`'s methods are not on it.

### 3.3 The workorder's hypothesis is disproved — and that changes the fix

The workorder's first hypothesis was that `postRecreate()`'s thrown exception
left Android's relaunch machinery armed on the Activity token. It cannot have:
`Activity.recreate()` was **never entered** in v2, v2.1 or v2.2, so
`ActivityThread.scheduleRelaunchActivity` was never called and no pending
relaunch exists to disarm. There is no "half-recreating" state to clean up — the
Activity never heard from this button at all.

What the tap actually did, every time, is the complete list:

1. `vm.attach_current_thread()` on the `android_main` thread, and the
   `AttachGuard`'s drop **detaches** that thread again;
2. one `GetMethodID` that left a **pending Java exception** in the VM (jni 0.21.1
   returns without clearing it);
3. a counter and a status line in app state.

That is the whole surface. Removing it removes every side effect the tap can
have on the process — which is why the fix is deletion, not repair.

### 3.4 What is still not explained (and the other two call sites)

The **ANR itself** is not explained by this: an ANR is the system's verdict, its
trace lives in `/data/anr` (not readable without root/shell), and the process
left nothing in its own record. This report therefore does **not** claim to know
which VM- or kernel-level interaction wedged the UI thread after a failed lookup
plus an attach/detach pair. It claims something narrower and checkable: the tap
has no remaining side effects, so if the ANR was caused by the tap it cannot
happen again; §9 states the boundary.

The same defect also means the shim's other two Rust→Kotlin calls are no-ops:
`request_insets_push()` (`postPushInsets`) and `request_crash_overlay()`
(`postShowCrashOverlay`) address the same Application. Both are best-effort by
design and nothing in the four checks depends on them — insets arrive on resume,
focus gain, layout and every dispatch; the crash overlay is built by Kotlin at
launch — but they are named here rather than left silent. The error text now
reads `SpikeApp(application).<method>(): …` so a future log cannot hide it again.
Repairing the handle (via `AndroidApp::activity_as_ptr()`) belongs to M13c, which
owns the real bridge; **this pass deliberately does not build a working Activity
handle**, because a working call is precisely what the recreate edge must not
have.

---

## 4. The choice: (b) — and why not (a)

| Question | Evidence | Consequence |
|---|---|---|
| Is `Activity.recreate()` "unsupported on GameActivity"? | `games-activity:3.0.5`'s `GameActivity` does not override `recreate()` (javap of the AAR's `classes.jar` lists `onCreate`/`onStart`/`onResume`/`onPause`/`onStop`/`onDestroy`/`onSaveInstanceState`/`onConfigurationChanged`, no `recreate`), so the framework method would run. `android-activity` 0.6.1 explicitly documents the opposite of unsupported: "it's possible for these activities to be created and destroyed multiple times within the lifetime of your application process", and then "`android_main()` will be called again as a new native `Activity` instance is created". winit 0.30.13's Android `EventLoop::new` has no once-per-process guard. | **Not provable — so not claimed.** The button does not say "unsupported on GameActivity"; that label would be a guess dressed as a finding. |
| Is the call even reachable? | §3.2: no. `ndk-context` holds the Application. | The only thing the old button could ever do is fail. |
| Could (a) be built instead — `finish()` + explicit relaunch intent? | It needs four mechanisms this shim does not have and cannot test here: an Activity handle (`AndroidApp::activity_as_ptr()` turned into a global ref), a `finish()` + `startActivity(getIntent())` route on the UI thread, probe persistence across a possible process death, and a second `android_main` entry (`EventLoop` rebuilt, `nativeActivityDestroyed` racing the new `onCreate`). | Four new unverified mechanisms on a phone we cannot touch, to replace one call we can delete. That is the opposite of "impossible by construction". |
| Would `recreate()` survive this stack at all? | `android-activity` 0.6.1 calls `GameActivity_finish(activity)` **unconditionally** after `android_main` returns (`game_activity/mod.rs:1247`) — the normal-return path, not just the panic path — and this shim's `onDestroy` deliberately ends the loop. So a landed `recreate()` is a framework relaunch racing an unconditional `finish()` of the Activity being relaunched. | The v2.1 report left that as "the phone's answer"; it has never been answered, because the call never landed. Not a route to build on. |
| Does the plan need it? | The workorder states later milestones never need `Activity.recreate` on Android. | No. |

**Decision: (b).** The button is a named limitation; the Kotlin method and the
Rust caller are gone; the outcome is a status line in app state.

Two deviations from the workorder's letter, both deliberate:

* **The label is not `recreate: unsupported on GameActivity`.** The line is
  `recreate: not available — no Java call, nothing relaunched. The shim has no
  Activity handle: ndk-context gives android-activity's Application, so
  MainActivity.postRecreate() can only fail its method lookup and no Activity
  code ever ran.` That is what the evidence supports; the workorder allowed the
  other wording only "once proven unsupported", and it is not proven.
* **The widget stays tappable** (label: `Recreate activity — NOT AVAILABLE (no
  Java call)`) instead of `ui.add_enabled(false, …)`. A disabled egui widget
  swallows the click, and the owner's pass in the workorder is "tap Recreate →
  outcome line → Home". Tapping produces the outcome line; the *action* is local
  state. There is no Java call either way, which is the requirement that matters.

---

## 5. "No Java call" — the structural proof

| Guard | Evidence |
|---|---|
| no callee | no `fn request_activity_recreate` exists, and nothing calls one. `grep -rn request_activity_recreate crates/ android/` returns **four prose mentions** (the app.rs module note, the app.rs frame-loop note, the `RecreateStatus` doc, the lifecycle tab's text) and no declaration, no call |
| no caller | the `recreate` branch in `app.rs::redraw` is gone; `ui.rs` has no `platform` import and is compiled on the host without JNI at all |
| no flag | `SpikeState::recreate_requested` is gone as a field; the only two mentions left are prose that records the removal |
| no Java entry point | `MainActivity.postRecreate` does not exist; `dexdump -d` of the **packaged** `classes3.dex`: 0 occurrences of `postRecreate`, and no `recreate` reference anywhere in that dex |
| what is left in the binary is copy, not a call | `strings` on the packaged `.so`: `postRecreate` ×4, all inside the UI evidence text that quotes the old sentence (`… so MainActivity.postRecreate() can only fail its method lookup …`, `… the button reported \`MainActivity.postRecreate(): Java exception was thrown\` …`); `Activity.recreate() on the UI thread` and `recreate FAILED on the UI thread` (the old Kotlin strings) are **0** |

The tap's entire code path is: egui button → `SpikeState::tap_recreate()` (two
field writes) → `diag::info` (one ring-buffer line). Nothing else.

---

## 6. Second layer — nothing is armed, and the settle is device-visible

There is no public `Activity.isRecreating()`; the relaunch flag lives in
`ActivityThread`'s client record and app code cannot read it. So the second layer
is built from what *can* be guaranteed:

1. **Structural disarm.** No `recreate()` call site exists in the repo
   (grep-verified, §5), so a relaunch can never be scheduled from this app. The
   disarm is the absence of the call, not a cleanup that runs after one.
2. **A readable settle line.** `MainActivity.logSettle(tag)` prints
   `settle (onResume): changingConfigurations=false finishing=false destroyed=false`
   at every resume and stop. Those are exactly the states a pending or
   half-finished relaunch would show up in, and they are the only on-device
   evidence this class can offer that the Activity returns clean after a tap.
3. **A rule for the future, written where the method used to be** (KDoc + §4):
   if a later milestone ever needs an Activity restart, it must be a `finish()` +
   explicit relaunch intent — a teardown the system actually performs — never a
   `recreate()` that depends on the ActivityThread relaunch record. This pass
   does not build that route.

---

## 7. The unit tests — what each one pins

`ui.rs` is pure egui (no platform import), which is what lets these run on the
host. What a host test cannot do is prove a *JNI call is absent* — that is
structural (§5) and checked in the packaged artifacts (§8).

| Test | Contract it pins |
|---|---|
| `a_recreate_tap_names_the_limitation_and_records_no_java_call` (replaces v2.1's `a_failed_recreate_is_surfaced_and_cleared_by_reset`) | the tap records the limitation, sets `failed` (the action did not happen), `java_call == None`, the line contains `no Java call` and `ndk-context`, and it contains **neither** `Java exception` **nor** `recreate failed:` — the retired wording must not come back through a copy-paste; two taps count twice; the reset clears line and counter |
| `a_recreate_tap_touches_nothing_the_frame_loop_consumes` | the tap's only effects are the two it documents: `restart_surface` stays false, probe/resumes/frames-gated unchanged — an "unavailable" button must not start an action |

`cargo test --workspace`: **573 passed, 0 failed, 8 ignored** (v2.2 baseline
572 → **+1**: one test replaced 1:1, one added). The whole v2.2
`surface_lifecycle` suite (7 tests) is untouched and green, and the headless
draw test now taps the recreate button before drawing, so the new evidence block
is rendered in every tab pass on the host.

---

## 8. Verification performed here

| Check | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 (clean) |
| `cargo clippy --workspace --all-targets -- -D warnings` | **exit 0** |
| `cargo test --workspace` | **573 passed, 0 failed, 8 ignored** (24 suites) |
| `cargo test -p suramak-android-spike` | 31 passed (30 in v2.2) |
| `cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-android-spike --all-targets -- -D warnings` | **exit 0** — the only gate that compiles `app.rs`/`platform.rs` |
| `./gradlew --no-daemon :app:compileDebugKotlin --rerun-tasks` | `BUILD SUCCESSFUL in 47s`, **zero `w:`/`e:` lines** |
| `bash scripts/android/build-apk.sh` | **exit 0**, all assertions passed (`SIZE: 20947967`, `SHA256: d1aae74d…`), zero `w:`/`e:` lines in the Gradle log |
| `aapt dump badging` | `package: name='app.suramak.spike' versionCode='5' versionName='0.3.0-m13b.2.3'`, `sdkVersion:'29'`, `targetSdkVersion:'35'`, `native-code: 'arm64-v8a'`, launchable `app.suramak.spike.MainActivity` |
| `aapt dump xmltree AndroidManifest.xml` | `enableOnBackInvokedCallback=0xffffffff`; `androidx.core.content.FileProvider` with authority `app.suramak.spike.fileprovider`; `configChanges=0x70` (keyboard\|keyboardHidden\|navigation) — rotation still recreates, by design |
| `apksigner verify --verbose --print-certs` | `Verifies`, v2 scheme true, `CN=Android Debug` |
| `readelf -lW` on the packaged `.so` | four `LOAD` segments, each `Align 0x4000` (16 KB pages) |
| `unzip -l` | `lib/arm64-v8a/libsuramak_android_spike.so` (13,557,080 bytes) + `AndroidManifest.xml` + 3 dex files |
| `dexdump -d classes3.dex` | `MainActivity` has `logSettle(String)V`; **no** `postRecreate`, no `recreate` reference |
| `strings` on the packaged `.so` | `recreate: not available` ×2, `no Java call` ×4, `ndk-context gives android-activity` ×1; `Activity.recreate() on the UI thread` ×0, `recreate FAILED on the UI thread` ×0 |

---

## 9. The honest boundary

1. **The ANR's removal is owner-device-only.** Nothing that runs on this host can
   create an ANR, so nothing here can prove its absence. What *is* proven: the tap
   makes no JNI/Java call, leaves no pending exception, attaches/detaches no
   thread, and cannot schedule a relaunch — so "app crashes on recreate" is
   impossible by construction, and the tap no longer touches the process at all.
2. **The proximate ANR mechanism is not in the in-process record.** The v2.2
   record held no Rust panic and no Java exception; the watchdog's own trace is
   outside the app's reach. §3.4 says what is known and refuses to invent the
   rest.
3. **Rotation is still an untested criterion, and it is now the only relaunch the
   app can experience.** The manifest recreates the Activity on rotation
   (`configChanges` covers only the hardware keyboard), which means
   `onDestroy` → `nativeActivityDestroyed` → `android_main` re-entry → a new
   `EventLoop`. That path has never run on the device. It is the next pass's
   subject, not this pass's claim; §10.7 says what to record.
4. **The other two Rust→Kotlin calls are still broken** (§3.4) and are documented,
   not fixed: they need the Activity handle this pass deliberately did not add.
   No check depends on them.
5. **Nothing ran on Android at runtime.** The `.so` compiles and links for
   `aarch64-linux-android` under `clippy -D warnings`, the Kotlin compiles and
   dexes, the APK is signed and well-formed. **No frame was drawn on a GPU in
   this pass.**

---

## 10. The device pass (owner)

Install the new APK, then:

1. **Identity.** Header `M13B v2.3 DIAGNOSTICS`, build `0.3.0-m13b.2.3`. If it
   still says `0.3.0-m13b.2.2`, the old APK is installed.
2. **Launch.** Cold start. Expect `event: resumed (#1) …`, `surface created for
   generation #1`, `gpu ready (resume, attempt 1, …)`, `first frame of this gpu
   generation presented`, and
   `kotlin: settle (onResume): changingConfigurations=false finishing=false destroyed=false`.
3. **Tap `Recreate activity — NOT AVAILABLE (no Java call)`.** Expect, in order:
   * the red line `recreate: not available — no Java call, nothing relaunched. …`
     and, under it, `java call: none`;
   * `recreate requests: 1` and `recreate java calls: 0 …`;
   * in the LOG: `recreate: not available — the button made no JNI/Java call …`;
   * **not** `kotlin: recreate: …` (there is no such method), **not**
     `Java exception was thrown`, **not** an `onDestroy`/`onCreate` pair, no
     crash banner.
   The numbered evidence block under the button is the same finding in full.
4. **Home → return.** Expect `kotlin: onStop`, `settle (onStop): …`,
   `surfaceDestroyed`, `gpu generation #N (epoch N) dropped …`, then on return
   `resumed (#2)`, a new generation, `first frame … presented`, the **same probe
   text**, `resumes` +1, and `settle (onResume): …` again.
   **The pass/fail:** the app must come back as its own live process. If the
   header counters restart at zero, the process was replaced — record that and
   stop; it is the v2.2 failure shape.
5. **Repeat step 3 + 4 three times.** Watch for a crash banner after
   backgrounding (the v2.2 shape) and for the pid changing.
6. **Optional — the surface path.** "Restart surface" should still behave as
   v2.2 left it: `surface restart #1 … dropping the whole gpu generation`,
   `restarts 1`, no panic.
7. **Rotate once (the untested criterion).** Expect
   `onDestroy (finishing=false, changingConfigurations=true)`, then a **new**
   `android_main entered (entry #2, activity generation 2)`, a fresh
   `resumed (#1)` for the new Activity, a new gpu generation, and — because the
   counters are process-wide — `resumes` continuing rather than restarting. If
   the app dies or restarts at zero on rotation, record it: it is a real finding
   for the next pass, not a NO-GO for this one.
8. **Record** for the gate: pid before/after (from the crash banner or logcat),
   `resumes`, `rebuilds`, `restarts`, `recreate requests`, the panic count, and
   every `settle (…)` line.

**Go if:** the tap shows the named line + `java call: none`; then three
Home → return round-trips in the **same pid**, `resumes` climbing, probe kept, all
`settle` lines `changingConfigurations=false finishing=false destroyed=false`, no
crash banner. If the process is replaced at any point, record it and stop — that
is the defect this pass exists to remove.

---

## 11. Files

| File | Lines | Role |
|---|---|---|
| `crates/suramak-android-spike/src/ui.rs` | 1555 (was 1425) | `RECREATE_UNAVAILABLE` + `RECREATE_EVIDENCE`, `RecreateStatus::java_call`, `tap_recreate()`, lifecycle tab, 2 tests |
| `crates/suramak-android-spike/src/app.rs` | 1108 (was 1117) | recreate branch/flag removed |
| `crates/suramak-android-spike/src/platform.rs` | 449 (was 421) | `request_activity_recreate` removed, `call_shim` + the Application finding |
| `crates/suramak-android-spike/src/lib.rs` | 107 (was 98) | v2.3 paragraph |
| `android/app/src/main/java/app/suramak/spike/MainActivity.kt` | 608 (was 595) | `postRecreate` removed, `logSettle`, KDoc |
| `docs/reports/M13b5-spike-v2.3.md` | — | this report |
