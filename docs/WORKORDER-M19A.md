# WORKORDER — M19A: touch→pointer, a Settings list that actually scrolls, and bottom insets

Plan: `docs/reports/M19-android-audit-plan.md` (Carbon pass) — slice WO-1.
Owner-reported: "scrolling doesn't work" on the phone (v0.4.3), plus the
DEVICE card clipped behind the gesture-nav pill.

## Step 0 — instrument before fixing (mandatory, first commit)

Log the raw egui event kinds once per frame **when input is present**, under a
new `SURAMAK_INPUT_LOG=1` env/`settings` debug switch (or `debug_assertions`
+ a build feature; your call, documented). Then the emulator smoke gains a
step: `adb shell input swipe …`, read logcat, confirm:

- do `PointerButton`/`PointerMoved` events already reach egui on Android?
  (Expected: no — winit emits Touch only, egui-winit forwards Touch only;
  this is VERIFIED in the pinned crates but the empirical check is what the
  fix's idempotence hinges on)
- are the touch coordinates in egui points (already divided by ppp)?

Record both answers in the report. **Do not skip this**: the fix's guard
(see below) depends on which answer is true.

## The fix

1. **Primary-touch pointer synthesis** in the compact shell's input prep
   (`crates/suramak-android/src/shell.rs::prime_input` or a new
   `deck/input.rs` it delegates to — the shell owns the phone's RawInput).
   Rules:
   - track `active_touch: Option<TouchId>` across frames;
   - `Start` (only if no active touch): `PointerMoved(pos)` then
     `PointerButton{Primary, pressed:true}`;
   - `Move` (same id): `PointerMoved(pos)`;
   - `End` (same id): `PointerButton{Primary, pressed:false}` then
     `PointerGone` (without it egui keeps a hover highlight on the last row);
   - `Cancel` (system back-swipe / notification shade): move the pointer far
     off-screen first, then release, then `PointerGone` — never release on
     top of a button (accidental click);
   - secondary fingers: never synthesise; leave their raw `Event::Touch`
     events in place;
   - **idempotence guard:** if the frame's event list already contains a
     `PointerButton`, do not synthesise for that phase (some future stack may
     start delivering pointer events itself).
2. **Settings layout: flow-relative rows.** Inside `ScrollArea::show`, place
   rows at `ui.cursor().top()` (the pattern `library.rs` already uses) — the
   current code reads the outer band's static `top`, which is why scrolling
   moves the viewport but not the content. Keep the visual result
   pixel-identical at offset 0 (the golden shots pin it).
3. **Bottom clearance.** The last section must not sit under the gesture-nav
   pill: pad the scroll content by the bottom inset the shell already
   receives (`state.insets.bottom`) plus the standard gutter, and assert it in
   the phone-viewport suite (a golden with a synthetic 54 px bottom inset).
4. **Touch options for thumbs:** set `max_click_dist` to ~12 pt and
   `max_click_duration` ~0.5 s on Android (egui defaults are mouse-tuned; a
   rolling thumb on a 450 ppi panel misses taps). Verify a tap still fires
   after a 10 pt wobble, and a 100 pt vertical drag scrolls WITHOUT toggling
   the row it started on.

## Acceptance (the emulator smoke extends to prove it)

- New smoke steps: swipe up inside Settings → the list moves (uiautomator
  window is a single surface, so the proof is a **screencap diff**: the
  Settings screenshot before/after a swipe must differ above a threshold;
  a stuck screen is identical);
- tap a row after a swipe works (the row's action fires — assert via logcat
  marker or the resulting screen);
- the DEVICE section's last line is fully visible above the nav bar in the
  final screenshot (bottom clearance);
- the whole existing smoke chain stays green (launch → Library → BACK → fresh
  relaunch → monkey), and the desktop goldens are unchanged at offset 0.

## Verification

- fmt/clippy -D warnings; workspace tests (baseline 1134+) green with ONE full
  run at the end; the phone-viewport suite grows with: scroll-offset rows,
  cancel-release never clicks, wobble tap, drag-on-row scrolls without toggling,
  bottom-inset golden.
- Report: `docs/reports/M19a-scroll-insets.md` — Step 0's two answers first,
  then the fix, then the smoke evidence paths.

## Do NOT

- No model/default changes (M19B owns those); no engine changes; no new deps;
  no CI edits except the smoke's new steps; no git commit/push; no sudo.
