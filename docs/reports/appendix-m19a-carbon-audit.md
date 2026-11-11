# Appendix — Carbon's high-effort audit of M19A (gemini-4-carbon, thinking 8192)

Raw answer, verbatim. Findings were parent-verified before adoption; the adopted ones are WORKORDER-M19A2.

### (A) Blocking Defects

1. **Bottom-clearance assertion probes a middle section (`DEVICE_NOTE`), not the list’s actual bottom (`quit`)** [Severity: **High**]
   * **Evidence:** **KNOWN** from `settings.rs` diff: `DEVICE_NOTE` is drawn around line 427 (`y = device_card.bottom() + 22.0`), followed by `---- Diagnostics (M13g3) ----` and `quit` (`y = quit.bottom() + 30.0`, line 495). **KNOWN** from `emulator-smoke.sh`: after 5 swipes to the bottom, the Python check asserts `device_bottom <= content_bottom - 1.0`.
   * **Impact:** At full scroll, `DEVICE_NOTE` sits above Diagnostics and the Quit button—well above `content_bottom` even if `tail` were zero (**INFERRED**). Meanwhile, if `state.insets.bottom == 0.0` (the immersive/late-inset case mentioned in the `settings.rs` comment), `bottom_clearance(0.0)` returns only `GUTTER`, leaving `quit` clipped under the gesture pill while `SURAMAK-LAYOUT` still passes! Furthermore, any synthetic-inset golden testing `bottom_clearance(54.0)` only proves arithmetic (`54/ppp + GUTTER`), not that real Android `WindowInsets` arrived (**INFERRED**).
   * **Fix:** Probe the `quit` rect in `log_layout_probe` (in addition to `DEVICE_NOTE`), assert `inset_bottom > 0` on gesture-nav AVDs, and enforce a fallback minimum floor in `bottom_clearance` when on Android if `inset_bottom == 0.0` (or verify `WindowInsetsCompat.Type.systemBars() | navigationBars()` is wired before first draw).

2. **Emulator smoke fails randomly when status-bar clock ticks or never logs `SURAMAK-LAYOUT` on a quiet render loop** [Severity: **High**]
   * **Evidence:** **KNOWN** from `emulator-smoke.sh`: full-window `screencap` compares `settings-still.png` and `settings-before.png` taken 0.4 s apart (`--max-mean-diff 0.05`). **KNOWN** from `shell.rs`: `log_layout_probe` runs inside `CompactShell::draw` with a 1-second rate-limit (`Duration::from_secs(1)`).
   * **Impact:** (1) If the Android status-bar minute rolls over during that 0.4 s window (or battery/network icon updates), the negative control fails CI (**INFERRED**). (2) In `emulator-smoke.sh`, after the 5 swipes finish, the script runs `sleep 2` expecting a fresh `SURAMAK-LAYOUT` line. Under `winit` reactive rendering (`ControlFlow::Wait`), once scroll momentum settles, `draw()` stops running (**INFERRED**). If the last `draw()` occurred < 1.0 s after the previous log, `log_layout_probe` returns early and *never logs the final resting scroll position*, leaving `tail -1` reading a mid-swipe coordinate!
   * **Fix:** Crop the top/bottom system bars out of `inspect_screencap.py --diff` (or freeze status bar via `adb shell cmd statusbar` demo mode). Remove the 1 s wall-clock throttle in `log_layout_probe`; instead log whenever `(rect, inset)` changes and the scroll velocity is zero (or log `device_top` before/after the first swipe as a deterministic, zero-cost scroll witness alongside pixel diff).

3. **`ui.allocate_space` over-allocates by `ui.cursor().top()` delta if `child` already advanced** [Severity: **Medium-High**]
   * **Evidence:** **KNOWN** from `settings.rs`: rows are manually painted starting at `top` (`ui.cursor().top() + remainder`), and at the bottom `ui.allocate_space(vec2(width, (y - top).max(0.0) + tail))` is called. However, `a11y::describe` is called mid-function (line 435), and interactive widgets (`quit`, rows) exist in the omitted hunk.
   * **Impact:** If `a11y::describe` or any intermediate helper uses `ui.allocate_rect` (which advances `ui.cursor()`) rather than `ui.interact(rect, id, sense)` (which does not advance cursor in egui 0.33, **INFERRED**), then calling `ui.allocate_space((y - top) + tail)` at the end *adds* the full height `(y - top)` onto an already-advanced cursor, nearly doubling the scrollable height and leaving huge blank space at the bottom!
   * **Fix:** Replace `ui.allocate_space(...)` with `ui.allocate_rect(Rect::from_min_size(pos2(x, top), vec2(width, (y - top).max(0.0) + tail)), Sense::hover())` so the final content `min_rect` is anchored explicitly from `top` regardless of intermediate cursor movement.

---

### (B) Should-Fix Risks

1. **Frame-wide `integration_translated` guard mishandles split multi-phase frames and `Cancel` ordering** [Severity: **Medium**]
   * **Evidence:** **KNOWN** from `input.rs`: `integration_translated` checks if *any* `Event::PointerButton` exists anywhere in `events`. `prepare_events` iterates `touches` and appends synthesised events at the *end* of `events`.
   * **Impact:** 
     * *Cancel + Start in one frame:* If finger 1 cancels (`egui-winit` emits `PointerGone`, no `PointerButton`) and finger 2/new gesture starts (`egui-winit` emits `PointerMoved` + `PointerButton(pressed: true)`) in the same frame batch, `integration_translated` is `true` (**KNOWN**). `Cancel` ignores `integration_translated` and appends `PointerMoved(OFF_SCREEN)`, `PointerButton(false)`, `PointerGone` *at the very end of `events`*—**after** the new touch's `PointerButton(true)`! That immediately cancels and parks the new live touch (**KNOWN** from `events.extend(appended)`).
     * *Multi-touch transition:* In `egui-winit` 0.33, `on_touch` tracks `primary_pointer_id` (first finger) and ignores secondary fingers (**INFERRED**). `TouchPointer` matches this (`if self.active.is_some() { continue; }`): when finger 1 lifts while finger 2 stays down, `self.active` becomes `None` and finger 2's moves stay ignored (no jump, pointer isn't stranded down because finger 1 emitted release). However, if `reset()` ran across an Activity pause while finger 1 was somehow still held, the next finger 2 `Start` claims `self.active` while `egui-winit` might disagree on primary ID (**INFERRED**).
   * **Fix:** Process events in-place (or rebuild the `Vec<Event>` sequentially) rather than appending at the tail, and scope idempotence per-phase/ordering rather than a single frame-wide boolean.

2. **0.5-pt `already_moved` filter swallows slow sub-pixel drags when `TouchPointer` is active** [Severity: **Medium**]
   * **Evidence:** **KNOWN** from `input.rs`: `TouchPhase::Move` skips synthesis if *any* `Event::PointerMoved(at)` in `events` has `at.distance(pos) < 0.5`.
   * **Impact:** When `egui-winit` is NOT translating (the exact fallback case `TouchPointer` was written for!), `events` initially has zero `PointerMoved`, EXCEPT if a fast-polling touch screen delivers two `TouchPhase::Move` samples in one 60 Hz frame separated by 0.3 pt (e.g., 120/240 Hz touch sampling on Vivo X300). Notice `already_moved` checks `events.iter()`, NOT `appended.iter()`, so when `egui-winit` is absent it emits both; when `egui-winit` IS present, `egui-winit` already emitted all `PointerMoved` events (at exact `pos`), making `distance < 0.5` redundant except where rounding differs. Worse: if `egui-winit` emitted a `PointerMoved` for an earlier coalesced sample within 0.5 pt of a *later* sample it skipped, velocity tracking loses micro-deltas (**INFERRED**).
   * **Fix:** Guard `Move` with a frame-level check (`let integration_moved = events.iter().any(|e| matches!(e, Event::PointerMoved(_)));`) or exact equality `at == *pos`.

3. **`TOUCH_MAX_CLICK_DURATION = 0.5` widens unintended double-click window & slow-tap semantics** [Severity: **Low-Medium**]
   * **Evidence:** **KNOWN** from `input.rs`: `max_click_duration = 0.5` (up from 0.25 s), `max_click_dist = 12.0` (up from 6.0 pt).
   * **Impact:** In egui 0.33 (**INFERRED**): (a) `ScrollArea` drag-to-scroll on a `Sense::click()` row works because `ScrollArea` checks `is_decidedly_dragging()` (which triggers once pointer delta exceeds `max_click_dist`, i.e., 12 pt = ~2 mm on screen), and `Sense::click()` only fires on release if `!is_decidedly_dragging()` and held `< max_click_duration`. Raising `max_click_dist` to 12 pt means a scroll gesture has 12 pt of initial deadzone before `ScrollArea` steals the gesture from a clickable row—acceptable for thumbs, though noticeable on tiny nudges. (b) In egui's `PointerState`, `max_click_duration` or `max_double_click_delay` (default 0.3 s) governs multi-clicks; two distinct taps 0.28 s apart within 12 pt will register `double_clicked()` in addition to `clicked()`, which is harmless unless a widget checks `double_clicked()`. (c) Note on parked `OFF_SCREEN` after `Cancel`: because `TouchPointer` emits `PointerGone` right after `PointerButton(OFF_SCREEN, false)`, egui clears `latest_pos` to `None` before the next frame's hit-test (**INFERRED**)—so next-frame hit-testing is safe.

---

### (C) Nits

1. **Pixel-snap remainder (`bands.content.top() - snapped`) stability** [Severity: **Low / Verified Safe**]
   * **Analysis:** **KNOWN** from `settings.rs`: `snapped` depends *only* on `bands.content.top()` and `ui.pixels_per_point()`, neither of which changes during scrolling. Inside `ScrollArea::show` in egui 0.33 (**INFERRED**), the inner `Ui` starts at `(viewport.min - state.offset).round_to_pixels(ppp).round_ui()`. At `offset == 0`, `ui.cursor().top() == snapped`, so `top == bands.content.top()` identically. During scroll on fractional `ppp` (e.g., `2.625`), `ui.cursor().top()` steps by exact physical pixels (`1 / 2.625` pt), and adding the constant `(bands.content.top() - snapped)` shifts the whole content column by a constant sub-pixel offset without changing inter-row spacing or `id_salt("compact-settings-scroll")` + `vertical_scroll_offset` restoration (**INFERRED**). Keep the math, just document that `bands.content.top()` must remain constant during scroll.

2. **`prime_input` logs `ppp=0.000` on frame 1** [Severity: **Nit**]
   * **Evidence:** **KNOWN** from `shell.rs`: `CompactShell::new()` sets `self.ppp = 0.0`; `prime_input` logs `self.ppp` *before* `draw()` updates `self.ppp = ctx.pixels_per_point()`.
   * **Fix:** Read `raw.pixels_per_point` (fallback `self.ppp`) inside `prime_input`.

---

### (D) What Must NOT Be Changed vs. Verified Next on Vivo X300

* **Do NOT change:**
  * The core `ScrollArea` cursor fix (`ui.cursor().top()`), `id_salt("compact-settings-scroll")`, or the `OFF_SCREEN` + `PointerGone` cancel-park pattern—these directly solve the dead scroll and stranded back-swipe highlight without over-engineering.
  * Leaving secondary fingers as raw `Event::Touch`.
* **Must verify next on physical Vivo X300 (Android 16, GameActivity):**
  1. **System back-swipe from edge over a Settings row:** Confirm `MotionEvent.ACTION_CANCEL` actually reaches winit's `GameActivity` glue as `TouchPhase::Cancel` (some `android-activity` versions drop or delay edge-intercepted cancels) and leaves `synth=3` in `SURAMAK-INPUT` with zero row activation.
  2. **Real `state.insets.bottom` on Vivo OriginOS/FuntouchOS:** Verify `SURAMAK-LAYOUT` logs a non-zero `inset_bottom` on startup without requiring a rotation/focus toggle, and that the **Quit** button (not `DEVICE_NOTE`) clears the gesture pill at max scroll.
  3. **240 Hz touch sampling + 120 Hz LTPO fling:** Verify kinetic scroll momentum feels natural (not damped by the 12 pt `TOUCH_MAX_CLICK_DIST` threshold or `already_moved` check) when starting a flick directly on top of a clickable model row.