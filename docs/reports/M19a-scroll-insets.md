# M19A — touch → pointer, a Settings list that actually scrolls, and bottom insets

Executed per `docs/WORKORDER-M19A.md` (slice WO-1 of
`docs/reports/M19-android-audit-plan.md`, the Carbon pass).

**Run status.** Everything below was implemented and verified on this VPS with
real commands: the full workspace suite (1148 passed / 0 failed / 10 ignored),
`clippy -D warnings`, `fmt --check`, a pixel diff of the committed Settings
shot against `HEAD`, a host probe of the **pinned** `winit`/`egui-winit` crates
through a real (Xvfb) window, and a dry run of the new emulator-smoke block
against a stubbed `adb`. **The emulator smoke itself did not run here**: this
host has no `/dev/kvm`, no `emulator` package and no NDK, so the Android
harness cannot boot. The script is the gate and CI is its first real run; §5
names exactly what that run will print and where.

---

## 1. Step 0 — the two answers

The workorder's premise was that `egui-winit` 0.33 forwards only
`egui::Event::Touch` and that nothing synthesises a pointer. **That premise is
wrong for the pinned 0.33.3**, and the two questions have concrete answers.

### Answer 1 — do `PointerButton`/`PointerMoved` already reach egui on Android?

**Yes.** Not from winit and not from this app: `egui-winit` translates the
**primary** touch itself. `State::on_touch` (`egui-winit-0.33.3/src/lib.rs`,
676–739) pushes the raw `Event::Touch`, then — for the first finger, tracked in
`pointer_touch_id` — calls its mouse path:

```rust
// … emit PointerButton resp. PointerMoved events to emulate mouse
match touch.phase {
    Started => { self.pointer_touch_id = Some(touch.id);
                 self.on_cursor_moved(window, touch.location);
                 self.on_mouse_button_input(Pressed, MouseButton::Left); }
    Moved   => { self.on_cursor_moved(window, touch.location); }
    Ended   => { self.pointer_touch_id = None;
                 self.on_mouse_button_input(Released, MouseButton::Left);
                 self.pointer_pos_in_points = None;
                 self.egui_input.events.push(egui::Event::PointerGone); }
    Cancelled => { self.pointer_touch_id = None;
                   self.pointer_pos_in_points = None;
                   self.egui_input.events.push(egui::Event::PointerGone); }  // ← no release
}
```

`WindowEvent::Touch` reaches it because `app.rs::window_event` forwards every
event to `egui_state.on_window_event` (`crates/suramak-android-spike/src/app.rs`,
~496), and winit's Android backend delivers `WindowEvent::Touch` for every
`MotionEvent` (`winit …/platform_impl/android/mod.rs`, 380–424).

**Measured, not just read.** A throwaway host probe (not committed) built a
real winit window under Xvfb with the pinned crates and fed it three
`WindowEvent::Touch` values at physical (520, 1040):

```
--- start: 3 egui events
    Touch(id=7, Start) at (520.000, 1040.000)
    PointerMoved(520.000, 1040.000)
    PointerButton(Primary, pressed=true) at (520.000, 1040.000)
--- move: 2 egui events
    Touch(id=7, Move) at (520.000, 980.000)
    PointerMoved(520.000, 980.000)
--- end: 3 egui events
    Touch(id=7, End) at (520.000, 980.000)
    PointerButton(Primary, pressed=false) at (520.000, 980.000)
    PointerGone
--- cancel: 2 egui events
    Touch(id=7, Cancel) at (300.000, 300.000)
    PointerGone                       ← the press is left DOWN
```

So: taps work today **because of egui-winit**, which also explains the
workorder's own observation ("the owner reaches Settings"). And the cancel row
is the bug this milestone's synthesiser exists to close: a system back-swipe
leaves egui's primary button down and the last row highlighted.

### Answer 2 — are the touch coordinates egui points (already divided by ppp)?

**Yes.** `on_touch` divides both axes by `pixels_per_point(&egui_ctx, window)`
before it builds either event (`lib.rs`, 689–692), and that helper is
`egui_zoom_factor * window.scale_factor()` — on Android, `window.scale_factor()`
is the display density, i.e. the same pixels-per-point the renderer uses. The
host probe's headless window reports a scale factor of 1.0, which is why its
numbers passed through unscaled; the division is by the window's scale factor,
not by a constant.

The on-device half of this answer is asserted mechanically by the new smoke
step (§5): the shell logs each frame's `screen=` rect in points and every touch
position; the step fails if any touch lies outside that rect — a pixel-space
coordinate would be ~2.6× too large and land off-screen.

### What the answers change in the fix

The synthesiser is still the right shape, for three reasons: the translation is
one crate's private behaviour (a `winit` that stops forwarding `Touch`, or a
future `egui-winit`, removes it silently); it never releases a cancelled
touch; and it translates only the first finger (fine, but now explicit in our
own code). The idempotence guard the workorder asked for is therefore **load-
bearing today**, not hypothetical: on Start/End the integration's
`PointerButton` is already in the frame and we append nothing.

---

## 2. The fix

### 2.1 Primary-touch → pointer synthesis

New module `crates/suramak-ui/src/deck/compact/input.rs` (the shell delegates
to it), called from `CompactShell::prime_input` on every frame before
`ctx.run`:

| Rule | Implementation |
|---|---|
| track `active_touch` across frames | `TouchPointer { active: Option<TouchId> }` |
| `Start` only if no active touch | first finger only; every later finger keeps its raw `Event::Touch` and gets no pointer |
| `Start` → `PointerMoved` then `PointerButton{Primary, pressed}` | appended in that order |
| `Move` of the same id → `PointerMoved` | a duplicate at the same position is skipped (no double-counted velocity) |
| `End` → `PointerButton{pressed:false}` then `PointerGone` | without the `PointerGone` egui keeps the last row hovered |
| `Cancel` → off-screen move, release, `PointerGone` | `Event::PointerMoved(-10_000,-10_000)`, then the release **at that point**, then `PointerGone` — never on top of a button |
| secondary fingers | never synthesised, never adopted; their raw events stay |
| **idempotence** | if the frame already contains any `PointerButton`, nothing is appended for that phase |

`reset()` is called when `SpikeState::resumes` changes: a touch that was live
when the Activity went away never gets its End/Cancel, and holding the old id
would make every later Start look like a second finger.

Because the integration already translates Start/End, the only path that fires
on today's stack is **Cancel** — which is exactly the stranded-button fix. On a
future stack without the emulation, the rest of the machine carries the phone.

### 2.2 Settings rows flow from the scrolled cursor

`crates/suramak-ui/src/deck/compact/settings.rs` painted every row at
`bands.content.top() + …` — the **outer band's static top** — inside
`ScrollArea::show`. The scroll range existed (the last `allocate_space`), the
offset changed, and the content never moved. The rows now start from the
content Ui's cursor, the pattern `library.rs` already used:

```rust
let snapped = bands.content.top().round_to_pixels(ui.pixels_per_point()).round_ui();
let top = ui.cursor().top() + (bands.content.top() - snapped);
let mut y = top + 14.0;
```

The second term is not decoration: `ScrollArea` snaps its content origin to the
physical pixel grid (`round_to_pixels` + `round_ui`), which moves the first row
0.16 pt above the band at offset 0 and softens every glyph. Adding the
remainder back keeps the offset-0 frame **byte-identical** to the pre-M19A
render (verified: `settings.png` differs from `HEAD` by **0 pixels**) while
scrolling is exactly cursor-driven.

### 2.3 Bottom clearance

The list now ends with `bottom_clearance(insets.bottom) = inset + GUTTER` of
padding (`settings.rs`, 503). With the content band already ending at the
inset, this is belt-and-braces: it matters when the shell reports zero (before
the first `nativeSetInsets` push, or an immersive window) and it guarantees the
last line can be scrolled clear of the pill rather than pinned under the clip
edge. The DEVICE note is also registered as an a11y label, and the shell logs
its rect once a second while Settings is up — that is the smoke's mechanical
clearance probe.

### 2.4 Touch options for thumbs

`apply_touch_options(ctx)` sets `max_click_dist = 12.0` and
`max_click_duration = 0.5`, applied by the shell (and re-applied if a surface
rebuild hands it a fresh Context). Verified in the phone-viewport suite: a
**10 pt wobble** still toggles the row (egui's 6 pt default would drop it), and
a **100 pt vertical drag** that starts on that row scrolls instead of toggling.

---

## 3. Tests added (14)

| Test | What it pins |
|---|---|
| `input::tests::a_tap_becomes_move_press_release_gone` | the phase → event mapping and ordering |
| `…a_second_finger_is_never_synthesised` | secondary fingers produce nothing; the primary keeps the pointer |
| `…an_integration_that_already_delivers_pointer_events_is_not_doubled` | the idempotence guard (the today-stack shape) |
| `…a_cancel_parks_the_pointer_before_releasing` | the release lands 10 000 pt away |
| `…a_duplicate_move_is_not_sent_twice` | no double velocity |
| `…reset_drops_a_finger_that_died_with_the_activity` | a resume re-arms the machine |
| `…a_synthesised_tap_clicks_and_a_cancel_does_not` | a real egui frame: click fires; cancel never clicks |
| `…the_touch_options_are_thumb_sized` | 12 pt / 0.5 s land on the Context |
| `…the_input_log_line_names_the_event_kinds` | the Step 0 line's format |
| `ui_compact::settings_rows_follow_the_scroll_offset` | rows move with the offset; offset 0 is where the static layout put them |
| `ui_compact::a_wobble_tap_still_toggles_the_row` | 10 pt wobble is a tap |
| `ui_compact::a_vertical_drag_on_a_row_scrolls_without_toggling` | 100 pt drag scrolls > 60 pt, no toggle action |
| `ui_compact::a_cancelled_touch_never_toggles_the_row` | no click, pointer not left down, the next tap still fires |
| `ui_compact::the_device_note_clears_a_54_px_bottom_inset` | 54 px inset → note above the band by `inset + gutter`; scroll range ends with the padding |

Plus the new golden `tests/ui-compact/shots/settings-bottom-inset.png`
(12 GB phone, 54 px synthetic inset, scrolled to the end: DEVICE card and Quit
fully visible above the pill).

---

## 4. Instrumentation (Step 0's switch)

`SURAMAK_INPUT_LOG` — `1`/`0` override; **default on in debug builds**, which
is exactly the APK the smoke builds (`build-apk.sh` builds x86_64 with
`profile.dev`), and off in release. One line per frame that carries input:

```
SURAMAK-INPUT ppp=2.625 screen=411.4x914.3 n=3 touch=[0:start@205.7,594.3] ptrmoved=1 ptrbutton=1 ptrgone=0 synth=0
```

`describe_frame` is taken **before** the synthesiser runs, so `ptrbutton=1`
means "the integration delivered it" and `synth=` names what the shell had to
add. A second, rate-limited line carries the Settings geometry:

```
SURAMAK-LAYOUT route=Settings device_top=789.3 device_bottom=805.3 content_bottom=893.5 screen_bottom=914.3 inset_bottom=20.8
```

Navigation changes log `compact route: Settings` / `… Library`, which is how
the smoke proves a tap landed without OCR.

---

## 5. The emulator smoke's new steps

`scripts/android/emulator-smoke.sh` gains one section (after the Library
screenshot, before the monkey canary):

1. **Open Settings by tap** at `(width − 40 dp, 46 dp)` — the trailing action's
   hit area — with a 46/70/30 dp retry ladder for unknown status-bar heights;
   success is the `compact route: Settings` marker.
2. **Swipe** 65% → 35% of the height, 250 ms. Two screencaps before/after are
   compared by the new `inspect_screencap.py --diff` mode
   (`mean |ΔL| ≥ 0.4`), with a **still-frame negative control** (`≤ 0.05`)
   so a comparator that always says "different" cannot pass a stuck list.
   `--self-test` gains both controls.
3. **Step 0's answers** are read from `input-log.txt`: it prints whether any
   frame carried a `PointerButton`, and fails if any touch position falls
   outside the `screen=` rect in points.
4. **Tap after the swipe** on the back chevron → `compact route: Library`.
5. **Clearance**: re-enter Settings, swipe to the end five times, then assert
   the last `SURAMAK-LAYOUT` line has `device_bottom ≤ content_bottom − 1`.
6. The emulator now boots with **`-memory 8192`** instead of 2048: the model
   table is the device tier's, and at 2 GB the phone reports "below the 4 GB
   floor", offers one model row, and the list fits the screen — the swipe
   assertion would be vacuous.

Artifacts (uploaded by the existing `build/android-smoke/` artifact step):

| File (`build/android-smoke/…`) | Content |
|---|---|
| `settings-before.png` | Settings at rest, before the swipe |
| `settings-still.png` | a second capture with no input (negative control) |
| `settings-after-swipe.png` | the same screen after the swipe |
| `settings-bottom.png` | Settings at the end of its scroll |
| `settings-swipe-diff.json`, `settings-still-diff.json` | the two `mean |ΔL|` numbers |
| `input-log.txt` | Step 0's raw answer lines |
| `layout.txt` | the last `SURAMAK-LAYOUT` probe |

**Not run here.** No `/dev/kvm`, no `emulator` package, no NDK on this host, so
the harness cannot boot. What was exercised instead: the block was extracted
and run end-to-end against a stubbed `adb` (fake wm size/density, logcat and
screencaps). All five assertions fired in order and the tap coordinates came
out as expected (Settings at `(975, 120)`, back at `(73, 120)` on a 1080×2400
420 dpi screen); making the stub return an identical "after" frame turns it red
with `the Settings list did not move after a swipe`. That proves the control
flow, not the phone.

---

## 6. Verification

| Command | Result |
|---|---|
| `cargo test --workspace --no-fail-fast` | **1148 passed, 0 failed, 10 ignored** (baseline 1134 + 14 new) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo fmt --all -- --check` | clean |
| `settings.png` vs `HEAD` (decoded pixel diff) | **0 differing pixels** |
| `settings-tiers.png` vs `HEAD` | 240 px (0.007 %, one glyph region) — the screenshot renderer's own ±200 px run-to-run noise |
| desktop shots (`editor-speakers-*`) | unchanged; 177–184 px regenerated, the same renderer noise, no code path touched |

One full run hit a **pre-existing flake**:
`suramak-asr::worker::tests::a_cancel_queued_with_the_job_is_observed_before_the_model_load`
failed on a loaded 2-core box (its own comment records the same failure). It
passed 3/3 in isolation and in the final full run; nothing in this slice
touches `suramak-asr`.

`Cargo.lock` carries a mechanical version-only diff (0.4.2 → 0.4.3 for the
workspace members): it was already stale against `Cargo.toml` at HEAD, and any
`cargo` invocation rewrites it. No dependency changed.

---

## 7. What this does not claim

- **No emulator evidence yet.** Whether the phone path really delivers
  `PointerButton` and how the panel's density maps to points is asserted by the
  smoke on its first CI run, not by this report. The crate-level answer above
  is measured on the pinned crates, and the source path from `MotionEvent` to
  `Event::Touch` is verified in the fork, but the end-to-end line comes from
  `input-log.txt`.
- **The Settings list still starts where it always did.** Offset 0 is
  pixel-identical on purpose; what changed is that the content now moves with
  the viewport.
- **The padding double-counts the inset** (the content band already ends above
  it). That is deliberate: the failure mode being fixed is a phone that reports
  zero insets, and the extra gutter never hurts.
- **Reduce Motion, fling feel and the back gesture** remain owner-device
  checks; the fling probe is untouched.
- M19B's model-state work (truthful "In use", tier default) is **not** in this
  slice; `settings.rs` still shows the M13D table it always did.
