# M13b v2.1 — four shim fixes: the resume texture panic, recreate, back, insets

**Status: built, packaged and statically verified on this VPS. Every behaviour
below is fixed in structure and unit-tested off-device; none of it has been
re-run on the owner's Vivo X300 yet.** §10 says per fix exactly what is proven
and what is not; §11 is the device pass that closes it.

Workorder: `docs/WORKORDER-M13B3.md`. Field evidence this pass answers: the v2
run on the owner's phone (crash trace + screen recording) — the resume texture
panic, the dead "Recreate activity now" button, the back gesture leaving the app,
zeroed insets, and ghost/double-rendered text after contained panics.
Previous build: `docs/reports/M13b2-spike-v2.md`.

```
android/app/build/outputs/apk/debug/app-debug.apk
  20,933,123 bytes (19.96 MiB)
  sha256 574f14638c14dee9677f034bdf2bd730ba1ca887ddac8ea36665fc143c19ea8f
  versionCode 3 · versionName 0.3.0-m13b.2.1 · package app.suramak.spike
  lib/arm64-v8a/libsuramak_android_spike.so — 13,542,232 bytes (LOAD Align 0x4000)
```

---

## 1. What changed

| File | Change |
|---|---|
| `crates/suramak-android-spike/src/gpu_context.rs` | **new** — the only `egui::Context` factory; the ownership rule in prose; three host tests that replay real egui texture deltas into a model of `egui_wgpu`'s registry |
| `crates/suramak-android-spike/src/app.rs` | context moved **into** `Gpu`; IME caret via the current generation; recreate outcome → status line; insets-stale request loop; bounded post-panic clean repaint; first-frame repaint log |
| `crates/suramak-android-spike/src/platform.rs` | new `request_insets_push()` (`postPushInsets`); recreate docs |
| `crates/suramak-android-spike/src/ui.rs` | `RecreateStatus` + status line under the recreate button; `insets_stale` + "no push yet" hint; text updated (v2.1 header, crash-file path); no check behaviour changed |
| `crates/suramak-android-spike/src/diag.rs`, `lib.rs` | docs (external files dir; v2.1 module) |
| `android/…/spike/MainActivity.kt` | back funnel across three dispatch paths (`OnBackInvokedCallback` + `OnBackPressedDispatcher` + `dispatchKeyEvent`) with dedupe, never `finish()`; insets re-push on resume/focus/dispatch/layout/request; exception-proof `postRecreate()`; **Share log** button; `nativeInit` gets the external dir |
| `android/…/spike/CrashFiles.kt` | external files dir + `migrate()` from the private dir, `shareableFile()`, fallbacks |
| `android/…/spike/SpikeApp.kt` | runs `CrashFiles.migrate()` once per process |
| `android/app/src/main/AndroidManifest.xml` | `enableOnBackInvokedCallback="true"`; `FileProvider` (`${applicationId}.fileprovider`) |
| `android/app/src/main/res/xml/file_paths.xml` | **new** — the two crash dirs are shareable, nothing else |
| `android/app/build.gradle.kts` | `versionCode 3`, `versionName 0.3.0-m13b.2.1` |
| `scripts/android/build-apk.sh` | asserts the new version, the packaged FileProvider/authority **and** `enableOnBackInvokedCallback` |

No new dependency (the FileProvider ships in `androidx.core`, already on the
classpath via appcompat). No Gradle/AGP change, no CI change, no change outside
`crates/suramak-android-spike/`, `android/` and this report.

---

## 2. Fix 1 — the resume texture panic (context belongs to the GPU generation)

### 2.1 The field trace, and the mechanism it names

The owner's v2 trace: `crash #3`, `egui-wgpu-0.33.3/src/renderer.rs:557:18`,
`Tried to update a texture that has not been allocated yet`, on the **first frame
after every successful resume-rebuild** (attempts 3, 4 and 5 in one session), with
the GPU-retry loop running its `attempt N of 3` messages in between and each
panic contained ("frame panicked" handled, app alive).

In the `egui-wgpu 0.33.3` copy pinned here, that sentence appears twice — line
657 (`update_texture`, the partial-delta branch) and line 861 (the user-texture
sampler path). The trace's 557 is the same `.expect` in a shorter revision of the
same file (line numbers move between crate revisions; the sentence and the
condition do not). Both are the same rule:

```rust
let Texture { .. } = self.textures.remove(&id)
    .expect("Tried to update a texture that has not been allocated yet.");
```

`egui::Context` owns the texture **bookkeeping**; `egui_wgpu::Renderer` owns the
GPU **textures**. Per frame egui decides between a *full* image
(`ImageDelta::pos == None` → create/replace) and a *patch* (`pos == Some` → write
into the texture you already have). v2 kept one context for the process and
dropped only the GPU objects on suspend, so after resume the context still
believed the font atlas and every painted image were resident, while the new
renderer had an empty registry → the first frame's `textures_delta.set` contained
a patch for an id the new renderer had never seen → `renderer.rs:657`.

Two more rebuild paths had the same defect: **"Restart surface"** and the
**post-panic retry** (`self.gpu = None` then `build_gpu`) — both rebuilt a
renderer against the kept, stale context, which is why a contained panic was not
a way out of the bug (and, most likely, why crashes repeated as #3/#4/#5).

### 2.2 The fix

`egui::Context` is a field of `Gpu`, built in `Gpu::new` by
`gpu_context::new_context()` and dropped with the surface, device, queue, renderer
and `egui_winit::State`:

```rust
// app.rs (excerpt)
struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    egui_ctx: egui::Context, // this generation's registry, this generation's context
    renderer: egui_wgpu::Renderer,
    egui_state: egui_winit::State,
    first_frame: bool,
    adapter_info: String,
}
```

`Handler` has no `egui::Context` field, so "a context that outlives its renderer"
is not representable. Suspend drops the whole generation (`self.gpu = None`),
resume builds a new one, and every texture id re-registers through a real
full-image create on that generation's first frame. `ui.rs` still takes
`&egui::Context`; `gpu_context::new_context` is the only factory (verified by
grep: no other owned `Context` exists outside test code).

### 2.3 Why a full rebuild, and what it costs

| Option | Verdict |
|---|---|
| **Full generation rebuild** (context + `egui_winit::State` + renderer + surface/device/queue) | **Chosen.** The illegal state stops existing; every id re-registers through the normal create path; one code path serves suspend/resume, the restart button and the post-panic repaint. |
| Keep the context, drop only the renderer, make egui re-send full state | Rejected — no such API. `Context::tex_manager()` exposes `TextureManager`, whose `take_delta` only *drains*; there is no "forget everything / force full uploads". Faking it by freeing every id and re-setting them fights the delta protocol. |
| Build a second context, keep the old one alive | Rejected — two contexts mean two widget-state stores for the same ids (`TEXT_FIELD_ID`, panel ids): the caret/text-edit state splits and the old ids stay stale. Strictly worse. |

**Cost, stated plainly:** egui's *per-context* UI state resets on every rebuild —
caret/selection, scroll offsets, hover, panel geometry, `TextEdit` cursor. The
spike's own state is `SpikeState` (text buffer, probe, counters, verdicts), so
**check 2 measures exactly what it measured before**. The IME bridge re-pushes
the caret on its next callback; a push that lands before the field has state in
the new context is a no-op (`TextEditState::load` → `None`), which the code says
out loud. Rebuild cost is unchanged in kind: the GPU objects dominate the ~77 ms
the field log shows; the context is a few ms (fonts are built lazily).

---

## 3. Fix 1 evidence (host, no GPU)

`gpu_context.rs` models the one rule that panicked and replays **real egui
deltas** into it the way `Gpu::frame` replays them into the renderer:

| Test | What it proves |
|---|---|
| `a_kept_context_against_a_fresh_registry_reproduces_the_field_panic` | gen 1 allocates a texture; the registry is dropped while the context survives (the v2 shape); the context then patches that id and the fresh registry rejects it with `NotAllocated(id)` — the `renderer.rs:657` condition, reproduced. The same patch against a registry that saw the create is accepted. |
| `a_rebuilt_context_re_registers_every_texture_with_full_images` | the v2.1 shape: a rebuilt context's first frame carries **only full images** (`pos == None`), the swatch is re-allocated, and a patch inside the new generation is legal. Also asserts the numeric `TextureId` is reused across contexts — which is exactly why the renderer must never outlive a context rebuild. |
| `every_call_builds_a_distinct_context` | the factory is not a cache (a shared context would reintroduce the bug), and both contexts carry the same styling. |

The second test also pins the documented cost: a value stashed in generation 1's
`Context::data` is gone in generation 2.

---

## 4. Fix 2 — "Recreate activity now" threw and did nothing

**Field:** `activity recreate failed: MainActivity.postRecreate(): Java exception
was thrown`; `recreate requests` incremented, no recreate happened, no reason on
screen.

**What was wrong.** v2's shim did `root?.post { recreate() }` from the
`android_main` (Rust) thread. `jni-rs` reports *any* pending Java exception after
`call_method` as `Java exception was thrown`, without saying which one — so the
exception was thrown inside that call, on a View/Activity pair that was not
guaranteed to be in a postable state, from a thread that is not the UI thread.
Nothing in the UI said so: the button looked like it worked.

**What it does now.**

* `postRecreate()` posts through an explicit `Handler(Looper.getMainLooper())`
  instead of `View.post`, so the shim never depends on the decor view being
  attached at that instant.
* It guards `isFinishing || isDestroyed`, and wraps `recreate()` in
  `try/catch` that logs the **real** class + message into the event ring
  (`recreate FAILED on the UI thread: java.lang.X: …`) and logcat.
* The JNI call can no longer see a Java exception: it returns `Ok` once the work
  is queued. Its outcome is recorded in the state and rendered as a **status line
  directly under the button** (`ui.rs::RecreateStatus`): amber "recreate
  requested — watch the LOG…" or red "recreate failed: …".

**The structural caveat, stated honestly.** Even when the call succeeds, back in
v2 the app decided to *exit the event loop* when `onDestroy` arrives, and
`android-activity` calls `GameActivity_finish()` when `android_main` returns. A
`recreate()` that lands therefore meets that contract; whether the relaunch
survives `finish()` is the phone's answer. The button's helper text now says so
and names the fallback that exercises the same surface rebuild without asking
Android to relaunch the Activity: **Home → return**, and **Restart surface**
(the same drop/rebuild path as a suspend, on demand). §11.5 says what to look for
in the log to tell the two outcomes apart.

---

## 5. Fix 3 — the back gesture left the app

**Field:** first back press not intercepted (`back presses intercepted` stayed
0), second back press landed on the launcher. Insets also read zero afterwards
(see fix 4 — a separate defect).

**What was wrong.** v2 registered exactly one thing: an androidx
`OnBackPressedCallback` on `onBackPressedDispatcher`. That callback is reached
from `Activity.onBackPressed()`, i.e. the **legacy** back path. Modern Android has
three ways for a back gesture to arrive:

1. `KEYCODE_BACK` key events (legacy, 3-button bar, hardware keyboards);
2. `Activity.onBackPressed()` → `OnBackPressedDispatcher` (legacy framework path);
3. `OnBackInvokedDispatcher` on the **window** (predictive back, API 33+). Android
   routes back here when `enableOnBackInvokedCallback` is on — and for apps
   targeting SDK 35 that is the default on Android 15/16.

On path 3 nothing was registered, so the **system's default callback finished the
Activity**. `GameActivity` additionally forwards key events to its native side
first (`onKeyDown`/`onKeyUp` → `onKeyDownNative`), which is a second way for a
synthesised `KEYCODE_BACK` to disappear before `onBackPressed()` runs. That is
consistent with the symptom: press 1 vanished on a path nobody owned, press 2
reached the default handler and the app left.

**What it does now.**

* `android:enableOnBackInvokedCallback="true"` in the manifest — the platform
  uses one consistent dispatcher on API 33+, instead of OEM-dependent key events.
* A single funnel, `handleBack(source)`, registered on **all three** paths: an
  `OnBackInvokedCallback` at `PRIORITY_DEFAULT` on `window.onBackInvokedDispatcher`
  (API 33+, guarded and try/caught), the existing
  `OnBackPressedCallback`, and `dispatchKeyEvent` for `KEYCODE_BACK`.
* **Never exits.** The old `nativePanicked → finish()` escape is gone; the crash
  overlay's "Close app" is the way out after a native death. (With the IME up,
  `dispatchKeyEvent` deliberately lets back through so it closes the keyboard,
  which is what the user expects there.)
* **Counts once.** One press can reach two paths on some OEM builds, so
  `handleBack` ignores a second call within 250 ms and logs that it did. The LOG
  names the path that fired: `back (OnBackInvoked): intercepted, counted`,
  `back (key): …`, `back (OnBackPressed): …` — which also tells us, on the next
  device pass, which path that phone actually uses.
* The JNI call is wrapped: a failing `nativeBackPressed()` logs and leaves the
  app alone rather than closing it.

---

## 6. Fix 4 — insets went to zero and never came back

**Field:** after the first back press the panel read all-zero and `inset updates`
went **1 → 0**, with no further `nativeSetInsets` despite the status bar being
visible.

**Two defects, and only one of them is about back.**

1. **The reset buttons zero the insets and nothing re-pushed them.**
   `reset_counters()` and `reset_tab(Tab::Insets)` set `insets = Insets::default()`
   — which includes `updates = 0`. The platform only dispatches window insets when
   they *change*, and v2 asked once at startup and once per resume. So a reset
   (the gate asks for one before each check: "a reset button in the header clears
   every counter for a clean run") left the panel at 0/0 for the rest of the
   session. `1 → 0` is that reset, not a stale value.
2. **The push was allowed to fail silently.** `pushCurrentInsets()` did
   `ViewCompat.getRootWindowInsets(view) ?: return` — before the decor view is
   laid out that is `null`, and v2 then waited for a change that might never come.

**What it does now.**

* `SpikeState::insets_stale` — `true` at start and set by **both** reset paths;
  cleared only when a real push arrives (`set_insets`). The frame loop sees it and
  calls the new `postPushInsets()` (at most once a second while stale), so a
  reset is followed by a real push instead of a silent zero.
* Kotlin re-pushes at every cheap opportunity: `onResume`, `onWindowFocusChanged`
  (focus gain), every `OnApplyWindowInsetsListener` dispatch (which is also how
  IME changes arrive), and once after layout (`view.post` + `postDelayed(300ms)`).
* `pushCurrentInsets()` no longer returns silently: if the window-insets object is
  not there yet it calls `ViewCompat.requestApplyInsets(view)` so the listener
  fires, instead of doing nothing.
* The Insets tab shows "no push yet — asked Kotlin for one…" instead of a bare
  0/0, and its Go line now names the three back paths.

---

## 7. The ghost / double-rendered text, and the one clean repaint

**Field:** overlapping/double-rendered text while scrolling the LIFECYCLE tab
around contained-panic moments.

That is the same mechanism as fix 1, one step further: a contained panic dropped
the `Gpu` and v2 rebuilt a renderer while keeping the context, so the next frame
patched the font atlas by id into a registry that either did not have it (panic
again) or had a *differently sized* texture under that id (glyphs written at the
wrong offsets → double/ghosted text). The fresh-context-per-generation fix
removes both: a rebuilt generation allocates the atlas from a full image and
never patches an id it did not create.

On top of that, v2.1 makes the clean frame explicit:

* every generation's **first presented frame is a full repaint** — the render
  pass clears the surface (`LoadOp::Clear`), every texture is created from a full
  image, and the context has no stale ids. `Gpu::frame` logs it once:
  `first frame of this gpu generation presented — full repaint (cleared surface,
  fresh texture registry, no stale ids)`.
* after a **contained frame panic** the app no longer waits for a button: it
  drops the whole generation and rebuilds immediately (`post-panic repaint 1 of
  2`), so the next frame presented is that clean one. Bounded to two automatic
  attempts, then it stops and waits for **Restart surface**, so a repeatable
  panic cannot spin.

---

## 8. Crash logs you can get off the phone (share sheet + external dir)

**Location.** The crash files now live in the app's **external files dir**:

```
/storage/emulated/0/Android/data/app.suramak.spike/files/spike-last-crash.txt
/storage/emulated/0/Android/data/app.suramak.spike/files/spike-crash-log.txt
/storage/emulated/0/Android/data/app.suramak.spike/files/spike-session.txt
```

`context.getExternalFilesDir(null)` is per-app external storage: **no permission**
on any API level, removed on uninstall, and the one tree the phone's own Files app
may browse for its own apps. v2's `/data/data/…/files/` was invisible to every
file manager.

**Migration.** `CrashFiles.migrate()` runs once per process from
`SpikeApp.onCreate` — before any Activity and before `nativeInit` — so the Rust
hook, the egui banner and the Kotlin overlay all see the new location on the
first launch after the update. Rules: move when the destination is missing/empty
(`renameTo`, copy+delete across mount points); if both history logs exist
(interrupted migration) append the private one to the external one; if only the
destination exists, drop the stale private copy. `dir()` falls back to the private
dir when external storage is not mounted, and reads fall back to a leftover
private copy, so a half-finished migration degrades to "still works".

**Share log.** The Kotlin crash overlay — the banner that exists precisely
because the Rust surface may be dead — now has **Share log**, **Close app**,
**Hide**. Share log does `FileProvider.getUriForFile(this,
"$packageName.fileprovider", file)` + `ACTION_SEND` (`text/plain`,
`EXTRA_STREAM`, `EXTRA_SUBJECT`, `ClipData`, `FLAG_GRANT_READ_URI_PERMISSION` on
both the send intent and the chooser), then `startActivity(createChooser(...))`.
It is pure Kotlin on the UI thread from the file the overlay already read: no JNI,
no egui, no wgpu. Every failure is a toast plus a line in the event ring.

Manifest: `androidx.core.content.FileProvider`, authority
`app.suramak.spike.fileprovider`, `exported="false"`,
`grantUriPermissions="true"`, meta-data → `@xml/file_paths`, which grants exactly
two roots: `<external-files-path path=".">` and `<files-path path=".">`.

---

## 9. Verification performed here

| Check | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | **exit 0** |
| `cargo test --workspace` | **565 passed, 0 failed, 8 ignored** (24 binaries). Baseline 561 / 0 / 8 → **+4**: three `gpu_context` tests + the recreate-status test |
| `cargo test -p suramak-android-spike` | **23 passed** (19 in v2), including the headless full-screen draws of all four tabs with the new status lines on screen |
| `cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-android-spike --all-targets -- -D warnings` | **exit 0** — the only gate that compiles `app.rs`/`platform.rs` |
| `./gradlew :app:compileDebugKotlin --rerun-tasks` | a **real** Kotlin compile (not `FROM-CACHE`/`UP-TO-DATE`): `BUILD SUCCESSFUL in 37s`, **zero `w:` lines** |
| `./gradlew --no-daemon assembleDebug` ×2 | `BUILD SUCCESSFUL in 19s` / `15s`, `35 actionable tasks: 35 up-to-date`, **zero warnings in both** |
| `bash scripts/android/build-apk.sh` | **exit 0**, all assertions passed, `SIZE: 20933123`, `SHA256: 574f1463…` |
| `aapt dump badging` | `package: name='app.suramak.spike' versionCode='3' versionName='0.3.0-m13b.2.1'`, `sdkVersion:'29'`, `targetSdkVersion:'35'`, `native-code: 'arm64-v8a'`, launchable `app.suramak.spike.MainActivity` |
| `aapt dump xmltree AndroidManifest.xml` | `android:enableOnBackInvokedCallback=0xffffffff`; provider present (`androidx.core.content.FileProvider`, `exported=0x0`, `grantUriPermissions=0xffffffff`, authority `app.suramak.spike.fileprovider`, meta-data `@0x7f0f0000`) |
| `aapt dump xmltree res/xml/file_paths.xml` | `external-files-path name="crash_external" path="."`, `files-path name="crash_internal" path="."` |
| `dexdump classes3.dex` | `CrashFiles.{dir,isExternal,location,migrate,shareableFile}`, `MainActivity.{shareCrashLog,postPushInsets,dispatchKeyEvent}` are in the package |
| `apksigner verify --print-certs` | verified, `CN=Android Debug` (`adb install -r` works) |
| `llvm-readelf -l` on the packaged `.so` | four `LOAD` segments, each `Align 0x4000` (16 KB pages) |
| `llvm-nm -D --defined-only` | `android_main` + the six `Java_app_suramak_spike_MainActivity_*` exports — JNI surface unchanged |
| `strings` on the packaged `.so` | carries the new lifecycle lines: `egui context rebuilt with it`, `first frame of this gpu generation presented`, `post-panic repaint`, `insets: stale` — the fixes are in the shipped binary |

---

## 10. The honest boundaries, per fix

1. **Texture panic (fix 1).** Proven in structure and unit tests, not on a GPU.
   The tests show that a context kept across a renderer rebuild emits a patch the
   fresh registry cannot satisfy and that rebuilding the context removes the
   condition. They cannot show that the Mali driver comes back cleanly from a real
   `ANativeWindow` loss, nor which texture (atlas vs painted image) was patched in
   the field. **The device repro is the only proof.**
2. **Recreate (fix 2).** The throw is removed and the real exception will now be
   named in the event ring; whether `recreate()` completes a relaunch under the
   v2 destroy→exit contract is unknown off-device — and it may legitimately end
   in "the app closes and relaunches", which the status line will now say instead
   of hiding.
3. **Back (fix 3).** The dispatch analysis above is inferred from the symptom plus
   the platform's documented routing; which of the three paths the Vivo uses is
   not observed here. The fix makes *every* path land in the same counter and
   never in `finish()`, and the LOG names the path — so the next pass answers it.
   Also unverified: whether an OEM delivers one press through two paths (the
   250 ms dedupe is the guard, and it is untested on a device).
4. **Insets (fix 4).** The `1 → 0` regression is explained by the reset buttons
   zeroing `updates`; if the owner did **not** press reset, the new log lines
   (`insets: stale — asked Kotlin for a fresh push` followed by a `nativeSetInsets`
   line) will show where the push was lost. The continuity fix is robust either
   way, but the exact trigger on that phone is not confirmed.
5. **Ghost text (§7).** Mechanistically explained and structurally removed;
   nobody has watched the screen after a contained panic on the device.
6. **Crash files / share (§8).** The provider, authority, XML roots, external
   path and DEX methods are verified in the package. Whether Vivo's Files app
   browses `Android/data/app.suramak.spike/files` is a device fact (Android 11+
   DocumentsUI hides that tree); the share sheet is the route that does not depend
   on it, and whether WhatsApp/Telegram accept the `text/plain` stream is untested
   here.
7. **Nothing ran on Android at runtime.** The `.so` compiles and links for
   `aarch64-linux-android` under `clippy -D warnings`; the Kotlin compiles and
   dexes; the APK is signed and well-formed. No frame has been presented on a GPU
   by this pass.

---

## 11. The device pass (owner)

Install the new APK, then, in this order:

1. **Identity.** Header `M13B v2.1 DIAGNOSTICS`, `build 0.3.0-m13b.2.1`. If it
   still says `v2` / `0.3.0-m13b.2`, the old APK is installed.
2. **Resume (was crash #3/#4/#5).** Tab `2 · LIFECYCLE`, type `lifecycle-123`,
   then **Home → 3 s → reopen from recents**. Expect the first frame to draw, no
   red banner, and in `LOG`, in order:
   `event: suspended — … egui context dropped`, `event: resumed (#2)`,
   `gpu ready (resume, attempt N, … ms)`, `egui context rebuilt with it …`,
   `first frame of this gpu generation presented — full repaint …`.
   `resumes` = 2 and the probe still there. Repeat five times, plus one rotation.
3. **Restart surface ×3** (same rebuild, no suspend). Expect `gpu ready (restart,
   attempt 1…)` + the context line + the first-frame line each time, no panic.
4. **Recreate.** Press **Recreate activity now**. Three outcomes are possible, and
   each is now legible:
   * `kotlin: recreate: Activity.recreate() on the UI thread` then
     `onDestroy`/`onCreate` → it worked (Activity relaunched, counters survive);
   * the same line then `recreate FAILED on the UI thread: java.lang.X: …` → the
     real exception is now named, on screen and in the file;
   * the same line then the app closes and reopens → the destroy→exit contract
     ended the old loop; that is a design decision for the next slice, and
     Home → return / Restart surface remain the supported way to exercise the
     rebuild.
   Whatever happens, the status line under the button must say something — a
   silent no-op is the bug this fix removes.
5. **Back.** Tab `4 · INSETS/BACK`, press back **four times**. Expect
   `back presses intercepted` to go 1, 2, 3, 4 (never 0, never skipping), the app
   to stay, and the LOG to name the path (`back (OnBackInvoked): intercepted,
   counted` on Android 13+). If it counts 2 per press, the dedupe window is too
   short for that OEM — note which path lines appear twice.
6. **Insets continuity.** Press the header **reset**, then look at tab 4 without
   touching anything else: within a second or two the values must repopulate and
   `inset updates` must climb (0 → 1 → …), with
   `insets: stale — asked Kotlin for a fresh push` followed by a
   `nativeSetInsets(…)` line. Then: raise the keyboard (`ime bottom` > 0), hide
   it, background/foreground once — the values must be non-zero every time.
7. **Ghost text.** Force a contained panic if one is cheap (or after any frame
   panic that does occur): the app should recover by itself (`post-panic repaint 1
   of 2` + `first frame … full repaint`), and the LIFECYCLE tab must scroll
   without doubled/overlapping glyphs.
8. **Crash log + share.** With a crash record present, relaunch, press **Share
   log** on the red overlay, pick any chat app: the `.txt` must arrive. Also try
   the Files app at `Android/data/app.suramak.spike/files/`.
9. **Then the four checks** (`docs/m13-gate.md` §3–§6) — check 1 especially, since
   the caret now lives in a context that is rebuilt on suspend: type, background,
   return, type again (buffer must survive, caret must follow the next keystroke).
10. **Stop rule.** If the same failure repeats four times, stop and send the red
    banner text plus the expanded LOG — with the new lines, that is a design
    decision, not another build.

---

## 12. Files

Added:

- `crates/suramak-android-spike/src/gpu_context.rs`
- `android/app/src/main/res/xml/file_paths.xml`
- `docs/reports/M13b3-spike-v2.1.md` (this file)

Modified:

- `crates/suramak-android-spike/src/{app.rs, ui.rs, platform.rs, diag.rs, lib.rs}`
- `android/app/src/main/java/app/suramak/spike/{MainActivity.kt, CrashFiles.kt, SpikeApp.kt}`
- `android/app/src/main/AndroidManifest.xml`
- `android/app/build.gradle.kts`
- `scripts/android/build-apk.sh`

Generated, not committed: `android/app/build/`, `android/.gradle/`,
`android/.kotlin/`, `android/local.properties`,
`android/app/src/main/jniLibs/arm64-v8a/*.so`.

---

## 13. What the phone session must answer

1. **Does the first frame after a resume draw?** (§11.2–3.) Everything else is
   secondary.
2. **Does back stay in the app and count once?** (§11.5.) The LOG now names the
   dispatch path, which is the fact this build needs from the phone.
3. **Do the insets repopulate after a reset and after a background trip?**
   (§11.6.)
4. **What does recreate actually do — relaunch, throw, or close?** (§11.4.) The
   status line and the log lines make the three outcomes distinguishable.
5. **Does the share sheet deliver the log, and is `Android/data/…/files`
   browseable?** (§11.8.)
