# WORKORDER — M19A2: fix the audit findings on the scroll/insets slice

Source: Carbon's high-effort re-audit of `fcef4a4` (answer in
`carbon-audit-m19a-high-answer.md`, scratch; plan
`docs/reports/M19-android-audit-plan.md`). Parent verified each finding against
the code before adopting it — the ones below are real.

## Blocking (verified)

1. **The clearance probe measures the wrong element.** `log_layout_probe`
   reports the `DEVICE_NOTE` label's rect, but the list continues with the
   Diagnostics section and the **Quit** row (`settings.rs:445–495`); the note
   is a middle element. At full scroll the Quit row is the one under the pill.
   - Probe the **Quit row** (its a11y label) AND keep DEVICE_NOTE as a second
     field; the smoke asserts `quit_bottom + bottom_clearance <= viewport_bottom`.
   - Assert `inset_bottom > 0` on the gesture-nav AVD, and give
     `bottom_clearance` a floor when the platform reports 0 (a floor of 24 dp
     on Android; desktops unchanged) so an insetless draw cannot clip the row.
2. **The smoke's screencap diff can flake on the status bar.** The still-frame
   negative control compares two full-window shots 0.4 s apart; a clock minute
   rollover or battery/network icon change moves pixels the crop shouldn't see.
   - Crop the status-bar band (use the shell's `inset_top`, or a fixed
     `status_bar_height` from `dumpsys window`) out of both images before the
     diff; assert the crop is non-empty and document the band.
   - A unit test for the cropper with synthetic PNGs, one with a "ticking
     clock" patch in the cropped band — must compare equal after cropping.
3. **The layout probe can leave a stale line.** The 1 s wall-clock throttle
   plus reactive rendering (`ControlFlow::Wait`) can mean the post-swipe
   `tail -1` reads a mid-swipe line.
   - Log whenever the probed rect or inset **changes** (plus at most 4 Hz),
     and once when the scroll offset has been stable for 300 ms; the smoke
     then reads the last *settled* line and asserts it is newer than the
     swipe (timestamp in the line).

## Should-fix (verified)

4. **Cancel-then-start in one frame breaks the new gesture.** Events are
   appended at the tail, so a cancel for finger A lands AFTER a start for
   finger B in the same frame — parking the pointer mid-press.
   - Rebuild the event list so each synthesized event sits immediately after
     the touch event that caused it; add tests for [cancel A, start B] and
     [start B, cancel A] orders.
5. **The `Move` dedupe uses a 0.5 pt distance.** When the integration already
   delivered this frame's moves, skip synthetic moves for the whole frame
   (frame-level `PointerMoved` presence), instead of a proximity guess that
   can double-count or swallow slow drags. Keep a unit test that two
   sub-pixel moves in one frame both survive when the integration is absent.
6. **`ppp` logs 0.000 on the first frame** (`prime_input` reads `self.ppp`
   before `draw` sets it). Read `raw.pixels_per_point` with the cached value
   as fallback.

## Do not change

The `ui.cursor().top()` fix, `id_salt("compact-settings-scroll")`, the
OFF_SCREEN+PointerGone cancel pattern, secondary fingers left as raw Touch
events — Carbon's audit calls these correct.

## Verification

- The new/changed unit tests above; fmt/clippy -D warnings; workspace tests
  green (one full run at the end).
- The emulator smoke (CI) is the on-device gate: swipe → settled layout line
  shows the Quit row clearing the inset; diff cropped; tap after swipe fires.
- Report: append a §6 to `docs/reports/M19a-scroll-insets.md` with the
  audit-answer file, the adopted findings, and the evidence paths.

## Do NOT

No model/default changes (M19B owns them); no new deps; no CI edits beyond
the smoke block; no git commit/push; no sudo.
