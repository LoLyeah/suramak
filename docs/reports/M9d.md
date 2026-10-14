# M9d — the owner-confirmed motion details · finish-review polish

Workorder: `docs/WORKORDER-M9D.md` (the 8 named items). Inputs: `.impeccable/motion-brief.md` §3/§7
(**owner-confirmed** — pill spring, living dot, seek/follow easing) and
`docs/reports/M9-FINISH-REVIEW.md` §3/§6/§7/§8. The M9a–M9c look is frozen beyond these items; the
asr/capture/live pipeline, playback internals, glass runtime, taps, instance guard and shutdown are
untouched (item 3 is draw/scroll-side only). No new crates or fonts.

**Method / environment.** Linux VPS (2 cores, headless), rustc/cargo 1.99, runtime evidence on
Xvfb/llvmpipe (layout + pixel truth; no glass on Linux). The macOS-only paths are compile-checked
(`cargo check -p suramak-glass --target aarch64-apple-darwin`) — there is no macOS host on this
machine. Frame numbers come from the `SURAMAK_FRAME_STATS` seam (M9b). New evidence harness:
`build/m9d-e2e/` (`shoot.sh`, `seed.py`, `pixelcheck.py`) — the M9c suite **plus** the corrected
status probe and the new checks; the M9c harness's probe was fixed in place too.

---

## 1. What shipped (item by item)

### 1) Pill — the spring (`motion.rs`, `views.rs`)

**The math.** `motion::spring_ease(t)` is a damped-spring step response (ζ = 0.80, ω₀ = 20 over the
normalised token): `0 → 0`, `1 → 1` exactly, first overshoot **1.52 %** of the travel at ~26 % of the
duration (bounded by `SPRING_OVERSHOOT = 0.02`), fully settled (1e-7) by t = 1. Because the travel is
3 % of the scale, the released scale overshoots by **< 0.05 %** — the owner's "gentle spring, no
bounce" read literally: it reads as spring, it cannot read as bounce. `spring_response` maps the
easing onto any travel.

**The stateful helper.** `Motion::press_spring(ctx, id, target)` springs the pill's **scale** between
1.0 (rest) and `SPRING_PRESS_SCALE = 0.97` over `SPRING_DURATION = 0.240 s`. A flip mid-flight
restarts the spring from the **current** value (never snap-back, never queued — the flip frame holds
the current scale, the next frame moves); the value is exactly the target once the duration elapses;
Reduce Motion returns the target immediately (no spring). Chosen over the egui animation manager
because the manager's easing contract is asymmetric on retarget (its release branch maps the linear
value through `1 − easing(1 − v)`, which is discontinuous for a spring-shaped easing — a press→release
flip would snap the scale from 0.97 to 1.0 in one frame).

**The morph's scale component.** The idle↔recording crossfade (label/colour — untouched) now also
carries a ≤ 1.5 % dip: `−0.015 · sin(π · morph)`, 0 at both ends of the morph, peaking mid-crossfade —
the brief's "gentle scale component", inside the ≤ 2 % scale ceiling (brief §6). Under Reduce Motion
the morph factor snaps to 0/1, so the dip is 0.

**The painting.** Everything the UI paints scales around the float rect's centre: the fallback body
(fill/stroke/shadow/hover lift) **and the content** (label/clock font sizes, dot radius, gaps, stop
glyph). The interaction rect and the focus ring stay at the M8.5 geometry — the click target and the
`float_rect` contract never move.

**Tests** (2): `spring_ease_has_bounded_overshoot_and_exact_endpoints` (endpoints exact, overshoot
≤ 2 %, peak mid-flight, scale overshoot < 0.1 %, `spring_response` endpoints);
`the_press_spring_ends_retargets_and_snaps_under_reduce_motion` (press in flight → deepest ≈ 0.97
within the bound → ends exactly at 0.97; mid-flight release continues from the current scale → release
overshoot ≤ 2 % of the travel → ends exactly at 1.0; Reduce Motion snaps both ways). Plus
`app::views::the_pill_body_is_the_float_rect_at_rest` (the drawn body at rest **is** the float rect;
pressed = 0.97 around the centre, inside the rect).

**Pixels** (`build/m9d-e2e/light/record-pill-{rest,pressed}.png`): held down, the pill's content ink
box shrinks **115 px → 112 px (ratio 0.974)**, centre shift 0.5 px. Hover alone changes nothing
(115 → 115 — the hover is the alpha lift only).

### 2) Record view — the living dot (`record.rs`)

A 3 pt red dot beside the `recording` status word (left of it, vertically centred), opacity
breathing **0.7 ↔ 1.0** (0.85 ± 0.15·sin) over `DOT_BREATH_PERIOD = 1.6 s` — subtle and slow, well
above the 1 s floor. `dot_breath(time, reduce)` is pure; Reduce Motion returns a constant 1.0 (a
static dot). Nothing else pulses: the meter and the waveform remain data. While `finishing…`, the dot
crossfades toward `ink_soft` with the word (the session is ending) but keeps breathing until the
state leaves. The breath **samples the shared clock** and requests no frames of its own — it rides the
recording cadence (100 ms), so the record budget is untouched (below).

**Test**: `the_living_dot_breaths_subtly_and_slowly` (range ≥ 0.7 and ≤ 1.0, span > 0.25, period
≥ 1 s and periodic, static under Reduce Motion).

**Pixels**: 28 reddish pixels in the dot box (x 1148–1166, y 170–188; the word starts at x≈1170);
crop `record-dot-crop.png`.

### 3) Waveform — seek eases, follow animates (`motion.rs`, `editor.rs`)

**Seek (draw-side only).** `Motion::seek_ease(ctx, id, target_ms, BASE)` keeps the drawn position: a
change **> `SEEK_JUMP_MS = 120 ms`** is a seek and eases from the current drawn value over the base
token (cubic ease-out), ending exactly at the target; smaller changes are the playback data itself and
are followed **exactly** (the playing playhead is never lagged); a new jump mid-flight retargets from
the current drawn value; Reduce Motion returns the target immediately. The data side is untouched:
`Playback::position_ms`, `waveform_ms_at`, the readout and the transport model are unchanged — the
strip just draws `drawn_ms` instead of `position_ms`.

**Follow-scroll.** `ui.scroll_to_rect_animation(row_rect, Align::Center, follow_scroll_animation(motion))`
with `ScrollAnimation::duration(BASE)` — a fixed base-token duration regardless of distance, ending
exactly at the target; Reduce Motion uses `ScrollAnimation::none()` (a snap). egui's scroll machinery
carries the interruption discipline for free (a retarget mid-flight updates the target and continues
from the current offset; a user drag cancels it). *Note for accuracy: egui 0.33's plain
`scroll_to_rect` was already animated (its default 0.1–0.3 s ease-in-out), so the review's "instant
snap" reading was not literally right — but it did not honor the base token or Reduce Motion, and
that contract gap is what this item closes.*

**Tests** (3): `seek_ease_ends_follows_playback_and_snaps_under_reduce_motion` (first frame static;
a jump eases and ends exactly; playback advance followed exactly; mid-flight retarget; Reduce Motion
instant); `follow_scroll_animation_is_the_base_token_or_a_snap`; and the frame-level
`the_follow_scroll_animates_and_ends_or_snaps_under_reduce_motion` (a real `ScrollArea`: the offset
moves across several frames, ends stable, and under Reduce Motion is at the target on the first
applying frame).

**Pixels** (click at x = 800 on the strip, which spans ≈ 408–974): pre-click playhead at
**(406, 409)** = position 0; settled at **(798, 801)** = the clicked position (the data jump + the
drawn position agree). The opportunistic mid-ease grab caught **(690, 693)** this run — strictly
between (and (753,756) in the previous run). See §3.6 for why the mid-frame check is evidence, not a
gate.

### 4) Home — the "Re" fragment (`home.rs`)

The capture-free build's note moved out of the header flow into the **message slot** (below the nav
row), priority error → status → note; the header block is now only the title + the one caption. No
geometry moved: the drop zone (top y = 130) and the status band (y 380–400, the item-8 probe target)
are exactly where they were. `CAPTURE_NOTE` is the single source of the string.

**Test**: `the_capture_note_never_renders_under_the_drop_zone` — the note's text shape sits **below**
the zone's bottom edge, and the header block's text ends above the zone's top edge (nothing can peek
from under it).

**Pixels** (`home-fresh.png`): the band above the zone's top edge (y 118–145, x 296–1279) is **clean**
— before the fix the note's glyph tops reached y = 129 across the width and the "Re" fragment sat at
x 313–320, y 130–137. The note itself is present in the message slot (ink 321–876 × 384–394).

### 5) Source-row tooltip (`record.rs`)

`.on_hover_text(&source.label)` on the truncated source row (the a11y label already existed). One
line; no layout change.

### 6) First frame static (`views.rs`)

`route_epoch` now returns `(epoch, first)`; the app's **first presented frame** registers the startup
route **without an arrival** — the pane paints at full opacity instead of fading in from 0 (the
review's §6.3 reading of the brief). Route *changes* still animate exactly as before.

**Test**: `the_first_presented_frame_is_static_and_route_changes_still_animate` — frame one's pane
title is fully opaque; a route change is hidden on its first frame (registered), mid-flight on the
next, settled after the token.

### 7) Contrast artifact (`theme.rs`)

* `macos_ladder(theme)` / `desktop_ladder(theme)` split out of `tokens()` as **pure, portable**
  functions (the platform only picks which ladder `tokens()` returns) — so the test exercises the
  macOS ladder from Linux; values unchanged.
* New token **`record_red_text`**: light `#c8281e` (200,40,30), dark `#ff5a50` (255,90,80). The old
  light `record_red` (`#e0352b`) measured **4.46:1** on the light card — under AA for 12 px text —
  and 4.12:1 on the panel / 3.74:1 on the window; the text-safe variant clears **all three**
  (5.56 / 5.14 / 4.66). `record_red` stays for the non-text indicators (the pill dot/glyph, the
  record view's dot, the meter fill).
* Text consumers switched to `record_red_text`: the `recording` word, `pane_caption` errors,
  `status_caption` errors, `arriving_caption` errors, the export sheet's error line.
* Helpers: `relative_luminance`, `contrast_ratio`, `composite_over` (premultiplied-alpha correct).

**Tests** (2): `wcag_contrast_clears_the_small_text_threshold_on_both_ladders` — ink & ink_soft on
window/panel/card and `record_red_text` on window/panel/card, both ladders × both appearances, each
colour composited over its surface (the translucent macOS window over a documented worst-case
backdrop: white light / black dark) — **all ≥ 4.5:1**, plus the `record_red` dot ≥ 3:1;
`contrast_math_matches_the_wcag_anchors` (21:1 black/white, 1:1 self, symmetry, compositing, and the
old-vs-new light red boundary).

**Measured (from the test's `--nocapture` table; the full 40-line table in `build/m9d/contrast-table.txt`):**

| pair (worst surface per family) | macos light | macos dark | desktop light | desktop dark |
|---|---|---|---|---|
| ink on panel | 16.49 | 15.76 | 15.24 | 13.77 |
| ink on card | 16.49 | 13.94 | 16.49 | 13.11 |
| ink_soft on panel | 6.23 | 8.56 | 5.75 | 7.48 |
| ink_soft on card | 6.23 | 7.57 | 6.23 | 7.12 |
| record_red_text on card | 5.56 | 5.30 | 5.56 | 4.98 |
| record_red_text on window (worst) | 5.56 | 6.83 | 4.66 | 5.92 |
| record_red dot on card | 4.46 | 4.78 | 4.46 | 4.50 |

**Pixels**: on the light card, the `recording` word renders r = 200 (the token) while the dot renders
r = 224 — the text-safe variant is visibly in use.

### 8) Harness probe fix (`build/m9c-e2e/pixelcheck.py`, `build/m9d-e2e/pixelcheck.py`)

The status-line probe moved from y 355–380 (which landed on the Models/Settings buttons) to the actual
status band **y 380–400**. Re-run against the frozen M9c captures: **18 PASS / 0 FAIL**, with the
status ink now measured at (321, 384, 397, 392) — the claim is verified for the right reason.

---

## 2. Verification (the gates)

| Gate | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `RUST_TEST_THREADS=1 cargo test --workspace` | **346 passed / 0 failed / 6 ignored** (was 335/0/6; +11 new tests) |
| xvfb smoke — plain (idle), 30 s | exit 0, **10–11 frames** (all in the first 0.4 s) — M9c: 8 |
| xvfb smoke — `SURAMAK_SMOKE_RECORD=1`, 30 s | exit 0, **293 frames / 9.8 fps** — M9c: 294 (the dot rides the cadence) |
| xvfb smoke — `SURAMAK_SMOKE_JOB=fixtures/audio/en.wav` | exit 0, `SMOKE-JOB-OK` (2 segments, model tiny, 11 frames) |
| xvfb smoke — `SURAMAK_SMOKE_MODELS=1`, 30 s | exit 0, 169 frames / 5.6 fps (the 150 ms download cadence) |
| pixel suite — M9d (`build/m9d-e2e/pixelcheck.py`, light + dark) | **25 PASS / 0 FAIL** |
| pixel suite — M9c re-run with the corrected probe | **18 PASS / 0 FAIL** |
| `cargo check -p suramak-glass --target aarch64-apple-darwin` | clean |

New tests (11): `motion::spring_ease_has_bounded_overshoot_and_exact_endpoints`,
`motion::the_press_spring_ends_retargets_and_snaps_under_reduce_motion`,
`motion::seek_ease_ends_follows_playback_and_snaps_under_reduce_motion`,
`theme::wcag_contrast_clears_the_small_text_threshold_on_both_ladders`,
`theme::contrast_math_matches_the_wcag_anchors`,
`app::views::the_first_presented_frame_is_static_and_route_changes_still_animate`,
`app::views::the_pill_body_is_the_float_rect_at_rest`,
`app::record::the_living_dot_breaths_subtly_and_slowly`,
`app::editor::follow_scroll_animation_is_the_base_token_or_a_snap`,
`app::editor::the_follow_scroll_animates_and_ends_or_snaps_under_reduce_motion`,
`app::home::the_capture_note_never_renders_under_the_drop_zone`.

**Screenshots** (`build/m9d-e2e/light/`): `home-fresh.png` (no fragment; the note in the slot),
`editor-titleline.png` + `editor-seek-{mid,mid2,settled}.png` (the strip settled at the click),
`record.png` + `record-dot-crop.png` (the living dot), `record-pill-{rest,pressed}.png` (the spring),
plus the M9c set (sheet, confirm, delete, models) re-captured on this tree; dark set in `dark/`.

---

## 3. Deviations (with whys)

1. **The spring scales everything the UI paints, not the macOS glass material.** The material's frame
   is the M8.5 `float_rect` (bottom-centre, one shared rect, re-applied on resize) and the glass
   runtime is out of scope for this slice, so the press is painter-side: on macOS it reads as the
   content compressing ≈3 % inside the fixed capsule. If the owner wants the material body itself to
   move, that is a glass-runtime change (a follow-up, not this slice).
2. **The Home fix went to the message slot, not "above the zone".** There is no room above: the header
   block (title + caption) already reaches y ≈ 118 and the zone's top edge is y = 130. The message
   slot is the workorder's sanctioned alternative and moves no geometry (the status band stays
   y 380–400 — exactly what item 8's corrected probe targets).
3. **The follow-scroll's easing is egui's `ease_in_ease_out`** (the scroll seam's own curve) over the
   base token, while the seek uses the house cubic ease-out. egui's `ScrollAnimation` is the only
   robust scroll-offset seam (interruption + drag-cancel come with it); 180 ms of ease-in-out between
   two rows reads as gentle, and the token/Reduce-Motion contract is honored.
4. **The seek eases only jumps > 120 ms.** Sub-120 ms changes follow the data exactly, so the playing
   playhead is never lagged and a click within ~9 px of the current position is an instant (sub-pixel)
   move. Documented in `SEEK_JUMP_MS`.
5. **Idle smoke 8 → 10–11 frames** (still all inside the first ~0.4 s; the remaining ~29.6 s of a
   30 s run are event-driven — 0.3–0.4 fps average). The added frames are the startup burst of
   one-shot fades and first-layout passes on the capture-free Linux build (the route arrival is now
   suppressed; every added animation ends — nothing loops). The record/download/job cadences are
   unchanged (293 frames/30 s, 169/30 s, 11 frames). The app is not held at tilt.
6. **The mid-flight seek pixel check is recorded evidence, not a gate.** The Xvfb `import` turnaround
   is ~160 ms against the 180 ms ease — a gated mid-frame check is a coin toss (observed: catches in
   ~half the runs; (690,693) this run, (753,756) in an earlier one). The deterministic proof is the
   unit tests plus the gated start-position and settled-position pixel checks.
7. **`ink_faint` remains unused** (review §6.6, 2.89:1 on light). It has no consumer and is out of
   this slice's named items; noted for the finish pass.

---

## 4. What remains for M10 / finish

Nothing new is expected: the slice closes every named item and every gate is green on this tree. What
remains is the **owner's macOS acceptance test** — the felt quality of the spring/dot/seek (and the
glass body's fixed frame, deviation 1), the real glass + `NSAppearance` re-resolve, Reduce Motion
from `NSWorkspace`, and the system-font header metrics — then the v0.1.1 release cut and M10 (fast
engines). The carried-over list is unchanged: quit-mid-recording warning, recents cap/`and more`,
settings/models scrolling at 900×600, duplicate empty-state copy, `suramak_playback::format_mmss`.

*Evidence trail: `build/m9d/tests.log` (346/0/6), `build/m9d/contrast-table.txt` (the WCAG table),
`build/m9d/smoke-{plain,record,job,models}.log` (frame stats), `build/m9d/shoot.log`,
`build/m9d-e2e/` (harness + captures), `build/m9c-e2e/pixelcheck.py` (corrected probe, re-run 18/18).*
