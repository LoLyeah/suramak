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
