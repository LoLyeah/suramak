# M13g1 — the `suramak-ui` split + the compact phone UI (part 1)

Workorder: `docs/WORKORDER-M13G1.md` (plan source `docs/PLAN-M13-ANDROID.md` §3
M13g, split at execution time into G1/G2 — the plan carries the note). Base
tree: `main` post-M13F (987 tests, CI green on all five platforms).

Scope delivered: the Deck/shell split inside `suramak-ui` (a library target the
phone compiles plus a desktop-only binary), the compact Library + transcript
editor + Settings profile with push/pop navigation and Deck motion, the
completion notification's deep link replacing M13E's toast, and the
foreground-service notification's Quit action in the M13F order. **No emulator
and no device run** — this workspace has neither. Every runtime sentence below
is either a host test or a compile/artifact claim; §8 is the honest boundary and
§10 is the owner-device checklist.

---

## 1. The split: modules inside `suramak-ui`, not a new crate

### 1.1 What the code looked like

`suramak-ui` was a single binary crate: `main.rs` owned winit/wgpu/AppKit
plumbing, `app/*` owned the desktop state machine and screens, and three files
were already pure egui + `suramak-core` (theme tokens, motion tokens, painted
controls) except for two `crate::` paths. Android could not compile any of it,
because the package pulled `winit`, `egui-wgpu`, `suramak-glass`,
`suramak-playback` and `suramak-taps` unconditionally.

The seam was therefore **already a module seam**, not a dependency knot: the
Deck half depends on `egui` + `suramak-core` and nothing else.

### 1.2 The shape chosen

One package, two targets:

```
crates/suramak-ui/
  src/lib.rs            ← `suramak_ui` (host AND aarch64-linux-android)
    deck/theme.rs       ← token ladder + material ladder + typography (moved)
    deck/motion.rs      ← durations, easing, springs, Reduce Motion (moved)
    deck/controls.rs    ← the whole painted control system (moved)
    deck/bounds.rs      ← the painted-bounds audit seam (moved)
    deck/nav.rs         ← NEW: push/pop stack, back semantics, transition clock
    deck/compact/       ← NEW: the phone profile (Library/editor/Settings/chrome)
    deck/snapshot.rs    ← NEW: deterministic host rasteriser for the phone suite
  src/main.rs           ← the desktop binary entry (Android: empty stub)
  src/desktop.rs        ← the desktop shell (moved from the old main.rs)
  src/desktop/*         ← app/, hud/, runtime, shell, instance, updates, …
```

Desktop paths did not change: `crate::theme` and `crate::motion` are re-exports
of `suramak_ui::deck::{theme, motion}` at the binary root, and
`crate::app::{controls, bounds}` re-export the moved modules. The desktop never
names a new path, and the moved files' bodies are unchanged apart from the two
`crate::deck::…` import lines in `theme.rs`/`controls.rs` and removing
`#[cfg(test)]` from `bounds`' audit seam (so the desktop's test-only audit works
across the crate boundary; it remains off in production).

### 1.3 Why not a `suramak-deck` crate

A second crate would need the same `egui` + `suramak-core` edges and would add a
version/publish boundary for one product on one release train. The module
topology already enforces the boundary the split exists for: the `lib` target
cannot reach winit, wgpu, AppKit, the audio engine or the updater, because those
dependencies are target-gated to `cfg(not(target_os = "android"))` and their
code lives in the binary target. A new crate buys no isolation the lib target
does not already give, so the workorder's "modules first" choice stands.

### 1.4 What the phone reuses

| Deck piece | Desktop | Phone |
|---|---|---|
| token ladder, both appearances, material levels | yes | yes |
| motion tokens, easing, springs, Reduce Motion | yes | yes |
| painted controls (`button_sized`, glyphs, tiers, state colours) | yes | yes |
| painted-bounds invariant / audit | yes | yes (debug asserts) |
| nav stack + transition clock | compact routes only | yes |
| compact screens | never | yes |

### 1.5 Desktop non-regression evidence

* The moved code is a verbatim move: `theme`/`motion`/`controls` bodies and
  their tests are byte-identical apart from the import paths above.
* The desktop route table, sidebar, titlebar, record pill, HUD and export sheet
  are untouched; `desktop.rs` is the old `main.rs` with `fn main` → `pub fn run`
  and module visibility changed to `pub(crate)`.
* **All pre-existing `suramak-ui` tests pass** (the 377-test baseline; see §7
  for the exact counts after the new tests). The M15 suites live inside
  `desktop/app/editor.rs`, `library.rs`, `settings.rs`, `search.rs` and the
  export module; none of their expectations changed.
* The Xvfb UI smoke starts the real desktop app and presents frames
  (`SURAMAK_SMOKE_SECS=3`, `build/smoke_ok.txt`) — §7.
* There are **no committed pixel goldens** in this tree to diff (the desktop's
  "golden" family is the painted-geometry tests plus the Xvfb smoke). That is
  stated rather than implied: the split is proved by behaviour tests, not by a
  pixel hash that does not exist.

---

## 2. The compact profile

`deck::compact` is a single-column, touch-first rendition of the same data:

* **Library** — app bar (Library / Settings), 48 dp search field, rows with
  title, date · duration · segment count, chevron; search matches title or
  segment text and shows a snippet with the timestamp. Import and Record in a
  68 dp bottom bar.
* **Editor** — back, title, meta line (duration · segments · language), segment
  rows with monospace timestamps and per-row play marks, tap-to-select then
  tap-again-to-edit with an inline 3-row field and Save/Cancel (Save persists
  exactly one segment row through the host); Find filters the rows; transport
  with play/pause, position track, time, Find and Export; Export opens a sheet
  with Markdown / Text / JSON / SRT / VTT.
* **Settings** — Language (Auto/English/Indonesian), current model, Keep the
  recording, the **M13D model table** (only models the device tier offers, plus
  installed ones; `recommended` / `installed` / tier note from
  `suramak_core::device`), Appearance (System/Light/Dark), Device facts
  (version, usable RAM, the on-device privacy line), and **Quit Suramak** behind
  a confirm sheet.

Every target is ≥ 48 dp; the content column is 16 dp-gutter; the system-bar
insets are passed in by the host (`Insets { top, bottom }`) and reserved before
any control is placed.

Semantics deliberately match desktop: transcripts are the `suramak_core`
`Transcript` rows; editing is one `update_segment_text`; export uses the same
core exporters; model guidance is the same manifest; appearance is the same
`Appearance` enum.

---

## 3. Navigation and motion

* One stack per app (`NavStack<CompactRoute>`), root = Library. Push/pop is a
  **single spring** (`motion::spring_ease` over the base 180 ms token); push
  enters from the right, pop reveals from the left, one curve for both
  directions. Overshoot is bounded by `SPRING_OVERSHOOT` (2 %) and the value
  lands exactly at 1.0.
* **Reduce Motion** settles the transition on the first frame; no animation
  frame is requested.
* A finished transition requests **no repaint** (`advance()` returns false once
  the token elapsed) — no continuous idle redraw.
* Back is the v2.4 funnel: `MainActivity.handleBack` (three dispatch paths:
  `OnBackPressedDispatcher`, `OnBackInvokedCallback`, `KEYCODE_BACK`) now asks
  `UiBridge.back()` first. A sheet dismiss or a screen pop consumes the gesture;
  at the Library root it returns `finish:true` and falls through to the
  existing v2.4 path. **G2 maps that fall-through to `finishAndRemoveTask`**;
  G1 leaves the spike's behaviour intact so the M13b checks are not disturbed.
* The Android chain is host-tested (10 nav tests): root cannot be popped,
  double-push is a no-op, a modal dismisses before navigation, deep link is
  Library → Editor, spring settles exactly, reduce-motion is a hard cut.

### 3.1 Fling / touch momentum (deferred with a probe)

egui's `ScrollArea` already implements drag-to-scroll with retained velocity
(the `scroll_source.drag` path used on every compact list). The probe here is
structural: both the library and the editor use
`ScrollArea::vertical().scroll_source(ScrollSource { drag: true, .. })` and the
suite renders the phone viewport; no custom fling was added because nothing in
the host evidence says egui's momentum is insufficient. **Real fling feel is
owner-device-only** (§8) and is on the checklist; if it is wrong, the Deck
motion layer is the place to add it, not the screens.

---

## 4. Where the routes live

The desktop route table (`desktop/app/views.rs`) is unchanged. The compact
table is `deck::compact::CompactRoute` with the dispatcher in
`deck/compact/mod.rs`; it is compiled on every target (so the host tests and the
phone suite run it) and **only the Android host activates it** — the desktop
binary never constructs a `CompactApp`. There is no `cfg` fork inside the
desktop screens: the phone route set is additive, which is what keeps the
desktop byte-for-byte behaviour the workorder asks for.

---

## 5. Completion notification → Library, deep-linked

M13E's tap path was: intent → `TranscribeBridge.open` → a toast naming the
transcript. G1 replaces the toast:

1. `MainActivity.showTranscript(id)` calls
   `UiBridge.openTranscript(id)` (one JNI function, same JSON pattern as the
   other three bridges);
2. `crates/suramak-android/src/compact_host.rs` reads the row from the real
   store and calls `CompactApp::deep_link`, which resets to **Library → Editor**
   with one push animation;
3. if the row is gone, `deep_link_missing` lands on Library with
   *"That transcript is no longer in the library."* — never an empty editor;
4. Kotlin logs the route (`open transcript: <id> → editor`), shows no toast, and
   does not crash on any failure.

Host tests: a notification tap opens Library → Editor (depth 2, title and id
present); a deleted row lands on Library at depth 1 with the note.

**Visible-surface boundary:** the APK in this G1 cut still hosts the M13b spike
surface; the route host is the destination the notification now writes. The
compact screens are drawn in the phone-viewport suite and are ready for G2's
shell — the report does not claim the phone currently paints them.

---

## 6. Quit on the foreground notification

Both persistent notifications (transcription progress and recording) gain a
**Quit** action, a broadcast PendingIntent so it works with no Activity and from
the lock screen. `app.suramak.ui.QuitReceiver` executes the owner's order:

1. **capture first** — `MicBridge.quit()` stops, finalizes and hands off a live
   take synchronously;
2. **jobs second** — `TranscribeBridge` `{"op":"quit"}` pauses every live job
   (copy + journal kept, so the next launch offers Resume) and
   `TranscriptionJobService.shutdown` clears the scheduler and stops the
   service;
3. **services third** — the recording service is stopped;
4. **exit last** — `Process.killProcess`.

Rust side: `JobHost::quit()` is new and unit-tested
(`quit_pauses_the_live_job_and_keeps_it_resumable`): one live job → paused,
copy kept, journal records the pause, no row written.

The source order is asserted mechanically by `scripts/android/build-apk.sh`
(line numbers of the four calls in `QuitReceiver.kt`), so a future edit cannot
quietly reorder capture behind the service.

---

## 7. Verification (run in this workspace)

| gate | command | result |
|---|---|---|
| fmt | `cargo fmt --all --check` | clean |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| Android compile (Deck) | `ANDROID_NDK_HOME=… cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-ui --all-targets -- -D warnings` | clean; its Android graph is `chrono, egui, suramak-core, uuid` only |
| workspace tests | `RUST_TEST_THREADS=1 cargo test --workspace` | **1026 passed, 0 failed, 10 ignored**, exit 0 (987 baseline → +39) |
| phone-viewport shots | `tests/ui-compact/run.sh` (Xvfb) | 4 tests pass; 4 PNGs at 1216×2640 |
| desktop Xvfb smoke | `SURAMAK_SMOKE_SECS=3 xvfb-run -a cargo run -p suramak-ui` | `SMOKE-OK` |
| Kotlin | `:app:compileDebugKotlin` with `allWarningsAsErrors = true` | `BUILD SUCCESSFUL` |
| APK | `bash scripts/android/build-apk.sh` | green: 35 958 065 bytes (34 MiB), sha256 `b94acd3f…d258943039`, versionCode 9 / `0.4.0-m13g1.1`, **Verifies** (v2 scheme, 1 debug signer) |
| packaged manifest | aapt (inside the build script) | `QuitReceiver`, recording/transcription services, share matrix |
| packaged dex | dexdump (inside the build script) | `UiBridge`, `QuitReceiver`, mic/transcribe classes |
| JNI symbols | llvm-nm (inside the build script) | Models/Transcribe/Mic/**Ui** bridges |

Test growth: `suramak-ui` gains 10 nav tests, 15 compact-model tests, 3
snapshot tests and 4 phone-suite tests (377 → 409 in that package); `suramak-android` gains 6 compact-host
tests and 1 job-quit test. The desktop's 377 pre-existing `suramak-ui` tests
still pass unchanged.

### 7.1 The screenshots

`tests/ui-compact/shots/`:

* `library.png` — dark ladder; app bar, search, three rows with chevrons and
  metadata, Import/Record bar.
* `library-light.png` — the same layout on the light ladder.
* `editor.png` — back/title, meta, selected segment, play marks, transport with
  position track, Find and Export.
* `settings.png` — transcription card, four offered model rows with the M13D
  tier line, hidden-larger-models note, appearance chips, device card, Quit.

Reviewed at full size while building: no text/control overlap, no clipping at
the viewport edge, the bottom bar clears the 20 pt gesture inset, and the model
notes truncate with an ellipsis before the action column. (The first pass had a
real defect — the `turbo-q8-0` note ran under its button and the Device/Quit
card fell below the fold; the rows now show only offered/installed models and
reserve a fixed action column. That is why the screenshots are part of the
gate, not decoration.)

---

## 8. The honest boundary

1. **No emulator, no device.** Nothing here was exercised on Android: the
   compact surface never drew a frame on a phone, the notification Quit was
   never tapped, the deep link never crossed a real intent, and touch/fling/IME
   were never delivered. All Android-side claims are compile/artifact plus host
   logic.
2. **The visible app is still the M13b spike surface.** G1's compact screens run
   in the host suite and the route host; G2 owns the `android_main` window that
   paints them. The notification now lands in the route host instead of a toast.
3. **Back-stack at the root.** G1 pops screens and dismisses sheets correctly;
   the root's `finish:true` still falls through to the v2.4 spike funnel rather
   than `finishAndRemoveTask`, because replacing the spike's surface is G2.
4. **Fling momentum** is egui's built-in drag scrolling; its feel is not proven
   here.
5. **Exports** are rendered and returned over the bridge (`export` op) but the
   `ACTION_CREATE_DOCUMENT` write is G2.
6. **Playback** is transport state in the compact editor; the phone's audio
   engine wiring is G2 (the desktop's `suramak-playback` is not compiled for
   Android).

---

## 9. Files touched

**Rust — `crates/suramak-ui`**

* `Cargo.toml` — `[lib]` target added; desktop deps target-gated to
  `cfg(not(target_os = "android"))`.
* `src/lib.rs`, `src/main.rs` — new lib root and the desktop/Android entry
  split.
* `src/theme.rs` → `src/deck/theme.rs`; `src/motion.rs` → `src/deck/motion.rs`;
  `src/app/controls.rs` → `src/deck/controls.rs`; `src/app/bounds.rs` →
  `src/deck/bounds.rs` (moves + import paths only).
* `src/main.rs` → `src/desktop.rs`; `src/app/` → `src/desktop/app/`; the other
  desktop modules → `src/desktop/` (move only, `pub(crate)` visibility).
* `src/deck/{mod,nav,compact/*,snapshot}.rs` — new.
* `tests/ui_compact.rs` — new.

**Rust — `crates/suramak-android`**

* `src/compact_host.rs` — new (library/settings/route host + 6 tests).
* `src/lib.rs` — module, process host, `dispatch_compact`, `UiBridge` JNI.
* `src/jobs.rs` — `JobHost::quit` + protocol arm.
* `Cargo.toml` — `suramak-ui` path edge.
* `tests/jobs_host.rs` — Quit test.

**Kotlin / resources**

* `java/app/suramak/ui/UiBridge.kt`, `QuitReceiver.kt` — new.
* `spike/MainActivity.kt` — compact host init + device probe, notification tap
  without toast, back asks the compact stack first.
* `transcribe/Notify.kt` — Quit action on both persistent notifications,
  `clearAll`.
* `transcribe/TranscriptionJobService.kt` — `shutdown`.
* `AndroidManifest.xml` — `QuitReceiver`.
* `values/strings.xml`, `values-in/strings.xml` — Quit label.
* `app/build.gradle.kts` — versionCode 9 / `0.4.0-m13g1.1`.

**Build / docs / tests**

* `scripts/android/build-apk.sh` — version, QuitReceiver manifest, UiBridge
  symbol/classes, Quit source-order assertion.
* `tests/ui-compact/{run.sh,README.md,shots/*.png}` — new.
* `docs/PLAN-M13-ANDROID.md` — G1/G2 execution note.
* `Cargo.lock` — one edge (`suramak-android → suramak-ui`); no new package.

---

## 10. Owner-device checklist (the runtime boundary)

Install the debug APK from `scripts/android/build-apk.sh`
(`versionName 0.4.0-m13g1.1`), then:

1. **Library** — import a voice note; the compact Library lists it with date,
   duration and segment count (G2 owns the visible shell; until then this step
   is the G2 acceptance).
2. **Notification tap** — share/transcribe a file, wait for "Transcript ready",
   tap it. Expect the app to land on Library → the transcript's editor with no
   toast. Tap a notification whose row was deleted → Library with
   "no longer in the library".
3. **Back** — open a transcript, swipe back: the editor pops to Library; swipe
   back again: the app leaves (G2) / the spike funnel counts it (G1).
4. **Quit from the notification** — long transcription running, pull the
   notification, tap **Quit**. Expect: the recording (if any) is saved and
   handed to the inbox, the transcription pauses (Resume offered next launch),
   both services are gone and the app exits. No data loss, no orphan
   notification.
5. **Quit from Settings** — same order, with the confirm sheet.
6. **Fling** — scroll a long transcript; note whether egui's momentum feels
   native. If not, the fix belongs in `deck/motion.rs` (G2).
7. **IME** — open search/edit on the phone, check the keyboard does not cover
   the field (the insets are wired; the device is the proof).
8. **Rotation** — the v2.4 Activity recreation still restores the probe; the
   compact host's state is process-wide, so the route survives a rotation.
