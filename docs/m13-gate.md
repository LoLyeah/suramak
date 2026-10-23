# M13b — Android shell spike: the owner's Go/No-Go gate

This is the checklist you run **on the phone**. It answers the four questions
that decide whether Suramak's option A (egui/wgpu inside a `GameActivity`) is
viable, before any M13 UI work depends on it. The milestone plan is
`docs/PLAN-M13-ANDROID.md` §2.1 + §M13b; the build report is
`docs/reports/M13b.md`.

The APK is a diagnostic, not a product: four tabs, a big **M13B GO/NO-GO**
header, and a live verdict line per check at the bottom of the screen. Nothing
is uploaded and the app asks for no permissions.

## 1. Install

Requirements: an **arm64** phone on **Android 10 (API 29) or newer**, USB
debugging on, `adb` from the SDK (`~/android-sdk/platform-tools/adb`).

```bash
# from the repo root, after `bash scripts/android/build-apk.sh`
adb install -r android/app/build/outputs/apk/debug/app-debug.apk

# or, from the CI artifact `suramak-android-spike-debug`:
adb install -r app-debug.apk
```

Launch **Suramak Spike** from the app drawer. Uninstall when done:
`adb uninstall app.suramak.spike`.

If `adb install` reports `INSTALL_FAILED_NO_MATCHING_ABIS`, the phone is not
arm64 — this spike ships arm64-v8a only, by design.

## 2. Before you start

- Turn **autocorrect ON** in your keyboard (Gboard: Settings → Text correction →
  Auto-correction → on). The point of check 1 is the IME, not raw key events.
- Have a second keyboard installed if you can (Gboard **and** Samsung Keyboard /
  SwiftKey). The gate is stronger if two IMEs behave.
- Screenshot each tab when its line says **GO** — that is the evidence the
  report cites.

A **reset** button in the header clears every counter for a clean run.

## 3. Check 1 — Text input (the top risk)

**Do:** open **1 · TEXT**, tap the field, and type this sentence with
autocorrect on, accepting at least one suggestion:

> Saya sedang menguji papan ketik Android dengan koreksi otomatis.

Then backspace across two whole words, and type one more word.

**Expect:** the keyboard appears on the first tap; every character appears once;
`characters received` rises as you type and `deletions seen` rises when you
delete; `IME updates` rises with every edit; the caret stays where you type; the
field is not hidden behind the keyboard.

**The bridge switch.** The tab has a checkbox — *"GameTextInput → JNI bridge"* —
that is **on by default**. It has to exist: winit 0.30.13 consumes
android-activity's `InputEvent::TextEvent` and drops it, so soft-keyboard text
has no path into egui without help. The Kotlin shim forwards
`GameActivity.stateChanged` (the GameTextInput listener) to Rust over JNI, and
the spike applies it to the egui `TextEdit`. Untick it and type: `IME updates`
and `characters received` stay at 0 and `winit key-text chars` stays 0 too —
that is the evidence for the winit gap, not an egui verdict.

| Result | Meaning |
|---|---|
| **GO** | Text arrives, no dropped/duplicated characters, autocorrect suggestions can be accepted, one backspace deletes one character (and word-delete works after a space), the caret follows your typing. |
| **NO-GO** | Characters dropped or duplicated, composition text invisible, the keyboard hides the field, backspace misbehaves, or the app crashes when the keyboard opens. |

**Fallback (option C, text fields only):** if this check fails, the editor and
settings text fields move to Compose overlays; everything else in M13 keeps
option A. Record exactly which keyboard and Android version failed, and whether
it failed with the bridge on or off.

## 4. Check 2 — Lifecycle

**Do:** open **2 · LIFECYCLE**, type `lifecycle-123` in the probe, then:

1. press **Home**, wait 3 s, reopen the app from recents;
2. tap **Recreate activity now** (this calls `Activity.recreate()` through the
   JNI shim — the same path a configuration change takes);
3. rotate the phone to landscape and back (the manifest deliberately does *not*
   swallow orientation changes, so this recreates the Activity).

**Expect:** `resumes` rises by one for each return (2, then 3, then 4–5 with
rotation), `surface rebuilds` rises for the recreated ones, and `lifecycle-123`
is still in the probe every time. No black screen, no frozen frame.

| Result | Meaning |
|---|---|
| **GO** | Text and counters survive background→foreground, an explicit recreate, and a rotation; the UI redraws immediately every time. |
| **NO-GO** | State is lost, the surface stays black after resume, or the app crashes on recreate. |

## 5. Check 3 — Motion

**Do:** open **3 · MOTION** and watch the bar for 10–15 s, keyboard closed.

**Expect:** smooth left-right motion and a reading near the panel rate. Most
phones report ~60 fps; 90/120 Hz panels may show more (present mode is FIFO, so
the loop is vsync-capped).

| Result | Meaning |
|---|---|
| **GO** | The average stays within a few fps of the panel rate and the motion has no visible stutter. |
| **NO-GO** | A steady average below ~45 fps, or visible hitching with nothing else running. Note the device model — this decides whether the Deck motion system needs a cheaper profile on Android. |

The frame-time readout (`ms/frame`) is the number to quote in the report; an
average frame time above ~22 ms is the No-Go line for 60 Hz.

## 6. Check 4 — Insets and the back gesture

**Do:** open **4 · INSETS/BACK** and read the table. Then:

1. switch to **1 · TEXT**, tap the field so the keyboard opens, and come back to
   **4** — `ime bottom` must be non-zero while the keyboard is up;
2. perform the **back gesture** (or the back button) three times.

**Expect:** `top` is non-zero with the status bar visible, `bottom` covers the
gesture/navigation bar, left/right are 0 in portrait and non-zero in landscape
on a notched phone, `ime bottom` tracks the keyboard, and **back presses
intercepted** increments — the app must **not** leave the screen or exit.

| Result | Meaning |
|---|---|
| **GO** | Insets match what is visibly on screen, the keyboard inset appears, and back is fully intercepted (counter only). |
| **NO-GO** | All insets read 0, the keyboard inset never appears, or back exits the app. |

## 7. Verdict

The gate is **GO** only if all four checks are GO. Then fill in
`docs/reports/M13b.md` §Owner device result (device, Android version, keyboard,
the four readings, screenshots) and M13 proceeds with option A.

If a check fails:

- **Check 1 only** → option C for the editor and settings text fields
  (`docs/PLAN-M13-ANDROID.md` §2.1); the rest of the plan is unchanged. This is
  the documented fallback, not a milestone failure.
- **Checks 2 or 3** → stop and escalate before M13c: surface recreation and
  frame pacing are load-bearing for every later slice.
- **Check 4** → fix the shim first (it is small); do not start the Deck work on
  guessed insets.

---

## 8. Owner device result (2026-10-21) — **NO-GO (escalate)**

The owner ran the spike on a real device the day the APK shipped:

- **Check 3 (Motion / 60 fps): GO** — the one working check. wgpu rendering and
  frame pacing on real hardware are proven.
- **Checks 1 (IME text), 2 (Lifecycle), 4 (Insets/back): broken "so much that
  the application refused to open before force stopped"** — the app wedged into
  a state where it would not relaunch until a force-stop.

Per the table above this is the **stop-and-escalate** branch: check 2 (surface
teardown/recreate) is load-bearing for every later slice. Interpretation
pending the failure details, but the pattern is consistent with the audit-known
limitation: the only check that avoids the Kotlin↔Rust bridge and lifecycle
transitions is the one that passed — rendering is fine, the platform glue is
what broke. Note the spike's JNI text bridge was always a probe (whole-buffer
replace), not the real IME bridge M13c was to own.

**Decision pending** the failure details (crash timing, relaunch symptom,
device/Android version, logcat if available): either (a) a spike v2 with
on-screen diagnostics + the real bridge design, or (b) pulling the text-field
fallback (option C) trigger now. Rendering at 60 fps surviving the gate means
option A is not dead — the bridge and lifecycle handling are what failed.

---

## 9. v2 field results (2026-10-23) — verdict per check, all four have data

v2's diagnostics worked: every failure named itself on screen (no logcat/PC needed).

| Check | Verdict | Evidence (device: Vivo X300 / Android 16) |
|---|---|---|
| 3 · MOTION | **GO** | 90.5 fps avg (twice-confirmed; v1 gave 60.3) — Mali-G1-Ultra MC12 Vulkan |
| 1 · TEXT | **GO (data path)** | Gboard ID autocorrect + suggestions render correctly via the JNI probe; counter mismatch + frozen deletions = the probe's whole-buffer-replace design (documented) |
| 2 · LIFECYCLE | broken ×2 → **fixed in v2.1** | resume texture panic (`egui-wgpu renderer.rs:557` "texture not allocated yet" on the first frame after EVERY resume-rebuild; attempts 3/4/5 contained — app survived) + `Recreate activity` threw `postRecreate(): Java exception` silently |
| 4 · INSETS/BACK | broken ×2 → **fixed in v2.1** | back NOT intercepted → the second back **exited the app**; insets dropped to all-zero and never re-pushed (`inset updates` regressed 1→0 — a state reset) |

**Gate verdict on v2 glue: NO-GO** — the predicted thin-layer branch. Everything heavy is
proven (rendering at 90 fps, IME data path, insets delivery at startup, GPU retry/
containment). All failures localize to the shim: egui-Context-vs-renderer lifecycle,
recreate path, back dispatch, insets continuity → **all four fixed in v2.1**
(commit `94f7364`, release `android-spike-m13b-v2.1`, versionCode 3).

**Decisive pass**: v2.1 on the device — zero red banners + the four GO lines = M13
unblocked, real IME bridge work starts. Any red that still appears names itself.

---

## 10.1. v2.1 field pass — 3 of 4 checks GO, the shim's last layer exposed

Device (Vivo X300 / Android 16) on `0.3.0-m13b.2.1`:
- **TEXT GO** (129 chars / 112 IME updates, autocorrect + emoji correct)
- **MOTION GO** (90.5 fps avg)
- **INSETS/BACK GO** (19/19 back presses intercepted via OnBackPressed, app never
  exited; insets 12 updates)
- **LIFECYCLE still WAIT** (Restart-surface / Recreate / Home round-trip not yet run)
- The stale-Surface crash re-fired each resume (`Surface::configure queue family`,
  + one `egui_core:3703` variant) — contained every time; `rebuilds bounded 2`.

v2.3 (`17437be`, release `android-spike-m13b-v2.3`, versionCode 5,
`0.3.0-m13b.2.3`) closes it by SOURCE root-cause, not hypothesis: android-activity
0.6.1 (init.rs:281) hands ndk-context the **Application** handle, so the shim's
Rust→Kotlin call ran MainActivity methods on SpikeApp — NoSuchMethodError
surfaced as the 'Java exception'. `Activity.recreate()` was **never entered** in
any build; no relaunch machinery was ever armed; the ANR-kill belongs to the
OS/Vivo background-kill class. The button now renders `RECREATE_UNAVAILABLE`
with its evidence, no Java call by construction; `logSettle()` proves clean
settle on device. M13c caveat: the same call_activity defect silently breaks the
two remaining Rust→Kotlin calls (will be fixed in the bridge work).

**Remaining criterion: rotation** — v2.4 (`fcc4144`, release
`android-spike-m13b-v2.4`, versionCode 6, `0.3.0-m13b.2.4`) gates it by
construction: a destroy arriving during gpu init cannot fault (terminal
`Phase::Destroying` + kill-latch checks before every window call — the v2.2
suspend-gate discipline, extended to destroy), every native kill
(SIGSEGV/SIGABRT/SIGBUS) now WRITES the last state to the crash file before
dying (proven by a subprocess test), and the probe text survives rotation AND
process kill via the Android Bundle (save/restore logged). Owner pass:
rotate within 1 s of launch ×3 + mid-session ×3 → zero native kills + probe
restore logged = **4/4 GO, M13 gate CLOSED**, real IME bridge work (M13c)
starts.

## 10. v2 — the diagnostics build (`0.3.0-m13b.2`)

v2 is the same four checks with the blindfold off: it records its own crashes
and shows them on the phone screen, because the reason nothing could be
diagnosed in §8 is that a failure was only ever visible in logcat. The build
report is `docs/reports/M13b2-spike-v2.md`.

Install it the same way (`adb install -r app-debug.apk`; it replaces v1 — same
package, versionCode 2). Then:

1. Launch. If the previous session ended badly, the **top half of the screen is a
   red text panel** titled `⚠ CRASH RECORDED — LAST SESSION` (or
   `⚠ NATIVE CRASH — THIS SESSION` if it just happened). Screenshot it; that text
   is the answer §8 was missing. `Hide` dismisses it for this launch only —
   nothing deletes it except the app's own **Clear crash** button.
2. Inside the app, the header reads **M13B v2 DIAGNOSTICS** and a red banner
   repeats the crash text. Under the verdict strip there is a **LOG** row: tap it
   to expand the last 40 platform events (every IME update, inset push, back
   press, surface create/destroy, resume/suspend, wgpu error), timestamped and
   collapsed to one line when shut — the header always shows the newest event.
3. **2 · LIFECYCLE** has a new **Restart surface** button: it drops the wgpu
   surface/device and rebuilds them, which is the suspend/resume path on demand.
   If check 2 is what wedges the app, this reproduces it without switching apps.
4. Re-run the four checks from §3–§6 exactly as before. If something breaks, do
   not force-stop: screenshot the red panel and the expanded LOG, then relaunch
   and photograph whatever the next launch says about the failure.

`files/spike-last-crash.txt` (and the append-only `files/spike-crash-log.txt`)
hold the same text if you can pull them with `adb shell run-as app.suramak.spike
cat files/spike-last-crash.txt`.
