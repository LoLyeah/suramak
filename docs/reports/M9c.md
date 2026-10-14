# M9c — the motion grammar · copy sweep · remake finish

Workorder: `docs/WORKORDER-M9C.md`. Contracts: `.impeccable/motion-brief.md` (**owner-confirmed**; §5
is the checklist this slice executes) · `docs/M9-BRIEF.md` (principle 7 = copy hygiene; protect list) ·
`docs/reports/M9-CRITIQUE-A.md` (the copy findings). The M9a/M9b look stays frozen except for the two
**owner-directed fixes** folded in mid-slice (§1.5, §3.1–3.2).

**Method / environment.** Linux VPS (2 cores, headless), rustc/cargo 1.99, all runtime evidence on
Xvfb/llvmpipe (layout + pixel truth; no glass on Linux). The macOS-only paths are compile-checked
(`cargo check -p suramak-glass --target aarch64-apple-darwin`) — there is no macOS host on this
machine. Every frame number below was measured with the `SURAMAK_FRAME_STATS` seam (M9b).

---

## 1. What shipped

### 1) Motion grammar — the full §5 sweep

All new motion is built on the M9a `motion.rs` foundation (fast 120 / base 180 / slow 240, ease-out
only, every animation ends, Reduce Motion zeroes every duration). One new helper was added:

* **`Motion::fade(ctx, id, on, enter, exit)`** — a directional factor with different enter/exit
  durations (the brief's "exits faster than enters"). Built on the same egui animation manager, so a
  mid-flight flip retargets from the **current** value at the new direction's rate — never queued,
  never snapped back (`interruptions_retarget_from_the_current_value`).

**Export sheet (the named leftover) — `export.rs`.** Open = base fade + 2 pt rise; close = fast fade.
The fade id is computed on **every** editor frame (also while closed), so it is registered at 0 before
the first open and the open animates instead of teleporting; the sheet keeps being drawn while it
closes (`sheet_visible`), takes no input while fading (`.interactable(open)`), and is gone the moment
the fast token ends. Leaving the editor closes it with the route change (the route's own slow fade
covers that path; documented). Tests: `the_sheet_fades_in_over_base_and_out_over_fast`,
`the_sheet_fade_snaps_under_reduce_motion`, `a_rapid_toggle_retargets_from_the_current_value`.

**Model manager — `model_manager.rs`, `models.rs`.** Progress is smoothed (`ProgressEase`: a bounded
linear approach with τ = 0.30 s, sized above the 150 ms download poll, settling **exactly** on the
target — the bar and its percent move continuously instead of jumping between chunk reports);
download-complete **crossfades** the bar out over the base token while the row's state text flips to
installed; every table load (Re-check / delete / download-done refresh) bumps a `refresh_epoch` and
every row plays a base-token arrival once — the row-level transition is seen, not teleported. The
empty "reading the manifest…" caption and the action status line use the shared fades (§4).

**Lists (library + home) — `mod.rs`, `library.rs`, `home.rs`.** Insert ease: a row that appears
*after* the first load is stamped with an arrival instant and fades + rises once (base). Remove ease:
a row a successful refresh drops is kept as a `LeavingRow` at its **old index**, drawn while its
height collapses and it fades out in place — the rows below slide up as it shrinks (`list_slots`
merges live + leaving rows by index). The first load renders statically; an errored refresh animates
nothing; the bookkeeping is pruned once the token elapsed (both animations end). *Search filter: the
app has no search/filter surface — skipped and noted (workorder allows). Rename: no rename exists
(the title is the file stem) — skipped and noted.*

**Status / inline messages — `views.rs` + every view.** One helper, `views::status_caption`: fades in
over base, out over fast; the stale text is kept while it fades (an immediate-mode label cannot fade
once it stops being drawn), then dropped. Never blocks input (a label, not a control). Wired to the
editor's cross-view line, Home (import error takes priority over the cross-view status — one slot;
the old flow-positioned error collided with the nav row on every error, see §3.3), record (source
warnings), transcribe (the failure line), models (action results) and settings (save result).
*Hover-pausable dismiss timer: no timed/dismissible toast exists anywhere in the app — skipped and
noted (workorder allows).*

**Settings — `settings.rs`, `controls.rs`.** The segmented control's **selection** now crossfades at
the fast token (a deliberate state change instead of an instant fill swap; every segmented call site
inherits it). A subtle one-shot **"applied" confirm**: a successful save arms `applied_at` and the
view plays a pulse ring around Save (quick rise, calm fall, exactly 0 by 700 ms, then cleared — a
one-shot, never a loop; skipped entirely under Reduce Motion, where the status line carries the
confirmation). **The appearance switch stays instant** — `sync_appearance` swaps the material/visuals
in one call with no animation (brief §5); the segmented *selection tint* animating at fast is the
control's own state, not the material swap.

**Record lifecycle audit** (idle → arming → recording → stopping → finalizing → saved):

| transition | treatment |
|---|---|
| idle → arming | route slow fade + 2 pt rise (M9a) + pill morph (base) + record view; caption "starting capture…" |
| arming → recording | **new**: the caption crossfades from the arming line to "recording from …" the moment the runtime reports the sources (base; both lines paint into one preallocated row — no measure shift) |
| recording → stopping | status word "recording" ↔ "finishing…" crossfade (base, M9a); **new**: the provisional card's hint fades in ("finishing the recording — the final pass starts next"); the Stop control disables |
| stopping → finalizing | the same `stopping` state (one crossfade covers the hand-off) |
| finalizing → saved | route slow fade into Transcribe; the engine line arrives through the status fade (base) |
| saved | route slow fade into the editor (M9a); the editor header is constant-geometry (§1.5) |

**Empty / first-run / error states.** One-shot base fade-in on arrival (`views::arriving_caption`) for:
the sidebar empty state, Home's empty recents, Home's library error, the model manager's "reading the
manifest…", the record view's failure message, and the transcribe failure line (via `status_caption`).
Nothing loops.

**Interruption + Reduce Motion audits.** The retarget test covers mid-flight flips for the shared
helpers; the sheet has its own rapid-toggle test; route flips use a fresh epoch per change (new
content arrives — the *same* property never snaps back); every new animation ends (fades reach 0/1,
the ease snaps within epsilon, arrivals are pruned, the confirm pulse clears itself). Reduce Motion:
every new animation goes through `Motion` (durations → 0 → snap); the confirm pulse is *skipped*
under Reduce Motion (opacity-only survivor rule) and the status text still confirms. Per-item column
in the checklist table below.

### 2) The §5 checklist table (state → treatment → Reduce Motion)

| §5 state | treatment (this slice + where it existed) | Reduce Motion |
|---|---|---|
| Route set (home ↔ editor ↔ …) | slow fade + 2 pt rise (M9a) | snaps (M9a) |
| Record: idle → arming | route fade + pill morph; caption "starting capture…" | snaps |
| Record: arming → recording | **caption crossfade (base)** | snaps |
| Record: recording → stopping | status-word crossfade (M9a) + **hint fade (base)** | snaps |
| Record: finalizing → saved | route fade + status fade | snaps |
| Live: line arrivals | fade + rise per arrival (M9a) | snaps (M9a) |
| Live: provisional → final | discard-on-Stop (state), final opens with the route fade | snaps |
| Playback: play/pause/seek/follow | unchanged (M8/M9a; playhead continuous, follow while playing) | n/a (position motion is data, not animation) |
| Lists: create | **insert ease: fade + rise (base)** | snaps |
| Lists: delete | **remove ease: fade + height collapse in place (base)** | snaps |
| Lists: rename | no rename surface — noted | — |
| Lists: search filter | no search surface — noted | — |
| Status: toasts/inline/errors | **fade in base / out fast** (one helper, all views) | snaps |
| Status: destructive confirm (trash) | in-place confirm cluster (M9a/M4.5); capsule bodies (§1.5) | — |
| Progress: model download | **smoothed bar/percent + completion crossfade** | snaps (exact target) |
| Progress: jobs / polish / export | event-driven (M9b cadence); status fades | snaps |
| Settings: appearance switch | **stays instant** (material swap, no animation) | instant |
| Settings: toggles/segments | **segmented selection crossfades (fast)**; stock checkboxes follow egui's animation_time (zeroed under Reduce Motion, M9a) | snaps |
| Settings: apply confirm | **one-shot pulse around Save (700 ms, ends)** | skipped (status line confirms) |
| First-run: empty library / hints | **fade in on arrival (base)** | snaps |
| Error states (import, record, transcribe, library) | **fade in on arrival / status fade** | snaps |
| Dialogs/popovers (export sheet) | **open base fade + 2 pt rise / close fast fade** | snaps |

### 3) Copy sweep — one system, plain words

* **Time formats — exactly two.** `format_elapsed` is now **elapsed hh:mm:ss** (record-view timer +
  floating pill + anything that shows a recording duration) and `format_transport` stays **mm:ss.mmm**
  (the deck readout). The record view's `mm:ss.t` and the pill's `mm:ss` are gone; the sidebar shows
  no duration (nothing to change — noted). Tests reject the old variants by shape
  (`the_two_sanctioned_time_formats_are_the_only_clocks`). The playback crate's `format_mmss` helper
  still exists but is no longer used by the UI (left for the finish review; §4).
* **Record view.** `current recording: N of the last 96 meter samples` → `the meter shows the last
  10 seconds`. Engine line in plain words (no `planned`).
* **Model display names.** `model_manager::display_name` maps every manifest id to a human label
  (raw ids stay as the model-manager detail line); used by the manager table, the settings combobox,
  the sidebar/home meta lines, the transcribe card and the model status lines.
* **Pluralization.** One helper (`count_noun`) — no `N item(s)` strings remain in the UI.
* **Jargon sweep.** `meter samples`, `window(s)`, `16 kHz`, `· tiny`, `planned`, `file(s)`,
  `device(s)`, `segment(s)` — all fixed in UI copy. The protect list (the provisional live-transcript
  label, the on-device/privacy lines, the error/status copy) was left **exactly** as-is.

**Before → after table**

| where | before | after |
|---|---|---|
| record view timer | `00:07.3` (`mm:ss.t`) | `00:00:07` (hh:mm:ss) |
| floating pill clock | `00:07` (`mm:ss`) | `00:00:07` (hh:mm:ss) |
| record view, next to Stop | `current recording: 96 of the last 96 meter samples` | `the meter shows the last 10 seconds` |
| record view, skipped windows | `3 window(s) skipped to keep up — …` | `3 windows skipped to keep up — …` (1 → `1 window`) |
| transcribe card, engine | `engine: coreml → planned metal` | `engine: Core ML` (unresolved: `engine: choosing…`) |
| transcribe card, model | `model: turbo-q5-0` | `model: Large v3 Turbo (balanced)` |
| job outcome status | `engine: cpu — saved to library: …` | `engine: CPU — saved to library: …` |
| editor meta | `tiny · en · 5 segment(s) · 14 Oct 2026 06:42` | `Tiny (fastest) · en · 5 segments · 14 Oct 2026 06:42` |
| sidebar / home row meta | `14 Oct 2026 06:42 · tiny` | `14 Oct 2026 06:42 · Tiny (fastest)` |
| models table | `ID` column + `turbo-q5-0` + role | `MODEL` column + `Large v3 Turbo (balanced)` + `turbo-q5-0 · default (macOS)` |
| settings, default model | raw ids in the combobox | `Auto (best installed)`, `Tiny (fastest)`, `Small (fast)`, `Large v3 Turbo (balanced)`, … (values unchanged) |
| model manager status | `turbo-q5-0: removed 1 file(s)` | `Large v3 Turbo (balanced): removed 1 file` |
| polish status | `polish accepted — 5 segment(s) updated (Undo available)` | `polish accepted — 5 segments updated (Undo available)` |
| polish error | `… with the 5 sent segment(s): got 4` | `… with the 5 sent segments: got 4` |
| settings, sources note | `… recorded together into one 16 kHz recording` | `… recorded together into one file` |
| settings, devices | `2 input device(s) enumerated` | `2 input devices enumerated` |
| live backlog log line | `window(s) skipped and merged into the next` | `the skipped window merged into the next` |

**Kept verbatim (protect list):** `live transcript — provisional · the final pass runs on Stop` ·
`recording from … — transcribed on this machine, never uploaded` · the Home drop-zone on-device line ·
`whisper.cpp models — downloaded from HuggingFace, sha256-verified, kept on this machine` · the
settings privacy line · every error/permission/codec message.

### 4) Defensive audit — the painted-bounds invariant

**Invariant.** *No painted control draws outside its allocated rect; the focus ring is the one
sanctioned excess.* The ring's path sits at `rect.expand(FOCUS_RING_EXPAND = 1.5)`; with
`StrokeKind::Outside` its ink reaches that path plus the focus stroke's width (2 pt), i.e. at most
**+3.5 pt** — and the control system's inter-control spacing (8 pt) exceeds that, so rings can never
collide with a neighbour either. What it guarantees: every painted shape of a control (fills,
strokes, glyph ink, label galleys, capsule bodies, segment bodies) is contained in the rect the
control allocated, so no control can visually bleed into another on any font metric.

**Enforcement at the source (debug builds):** `paint_glyph` computes each glyph's ink bounds
(`glyph_bounds`, the pure mirror of its shape math) and `debug_assert`s they fit the rect it was
handed — every glyph, every call site; `button_sized` / `capsule_button` / `capsule_glyph_button` /
`glyph_label_button` / `segmented` assert their label/glyph fits (including the ≥ `space.s` capsule
padding). The glyph math itself was corrected where it did not fit: the trash lid handle sat 0.3 pt
above a 16 pt box (now 7.0 from centre — invisible, but the assertion is honest).

**Frame-level seam + tests.** `app::bounds` records every control body (id + rect + kind) under a test
seam; `bounds::violations` checks every painted shape of a real frame against the recorded rects.
Tests: `no_control_shape_escapes_its_allocated_rect` (a showcase frame with every control kind),
`the_focus_ring_is_the_only_sanctioned_excess` (focus requested: the ring paints outside the rect —
proving the test can see it — and never past the sanctioned 3.5 pt), `every_glyph_fits_its_control_rect`,
`capsule_bodies_are_padded_capsules_at_the_designed_height`.

**The title-line trash ↔ Polish cluster (the owner's question).** The cluster's controls are recorded
like every other control, and `the_title_line_cluster_has_a_real_gap_and_no_overlap` asserts, on a
real frame: exactly the trash + Polish capsules (and trash + Undo + Polish with a pending undo);
**pairwise non-overlap**; the **painted gap ≥ `space.s`** (measured 9 px in the screenshot: the exact
8.0 pt layout gap plus the bodies' antialiased edges); nothing paints across the gap; every shape
stays inside its control's rect (+ the ring allowance) and inside the title row. The spacing math:
the cluster's RTL layout sets `item_spacing.x = tokens.space.s` explicitly, so the gap is exactly
8.0 pt between the allocated rects (screenshot bodies: trash 1174–1205, Polish 1214–1261).

### 5) Owner-directed fixes (mid-slice feedback)

**Fix 1 — the title-line cluster reads as real buttons (iOS-26 Liquid Glass language).** Scoped to
the cluster (trash, Polish, Undo; the confirm step wears the same bodies as a trivially consistent
tweak): a new `Tier::Capsule` — translucent `control_fill` body + hairline at rest (the pill's
material family), `control_fill_hover` / `control_fill_active` on hover/press with the fast response
(no bounce), capsule radius = half height, **28 pt** body height, ≥ `space.s` padding around the
glyph/label (`capsule_button` / `capsule_glyph_button`), and ≥ `space.s` between controls. The audit
above covers both bodies, and the gap is asserted in a test. Recorded as a deviation (§3.1).

**Fix 2 — the editor header's vertical rhythm is fully explicit.** Root cause class: the header below
the deck used to flow from *measured* galley heights (the 20 pt serif title label + the caption
labels), so under macOS's real system fonts the rows shifted a few pixels while the deck's separator
sat at a fixed offset — the owner's faint full-width hairline inside the title/caption zone. Now:
one set of pixel constants (`HEADER_TITLE_GAP` 6, `HEADER_TITLE_ROW` 32, `HEADER_CAPTION_GAP` 2,
`HEADER_CAPTION_ROW` 18 × 3 rows, `HEADER_CLUSTER_RESERVE` 280, `DECK_SEPARATOR_OFFSET` 0.5) and a
pure `header_layout(pane)` → `HeaderLayout { separator_y, title, captions }`; the title is painted
into its constant row as a **one-row ellipsis-truncated galley** (a long title can never grow the row
or push the cluster), the captions live in three explicit rows (`header_caption_rect`, contiguous,
zero overlap), and the deck's bottom separator is the **only** horizontal rule in the header zone
(both it and the layout read the same constant). Geometry tests:
`the_header_separator_never_enters_the_title_or_caption_zone`,
`caption_rows_stack_from_explicit_heights_with_zero_overlap`, `the_header_rhythm_is_constant_across_panes`,
and the frame-level audit `the_header_zone_has_exactly_one_full_width_rule` (a real editor frame
paints exactly one full-width rule in the header zone, at the constant y, and **no** full-width row
in the title/caption text zone). Screenshot: `light/editor-titleline.png` (+ the 2× crop) shows the
title, the caption row, the cluster, and the single separator above the title. Recorded as a
deviation (§3.2).

### 6) Per-file change summary

| file | change |
|---|---|
| `motion.rs` | `fade` (enter/exit), `token_duration`; +2 tests |
| `app/controls.rs` | `Tier::Capsule` + capsule buttons, `FOCUS_RING_EXPAND`, `glyph_bounds` + `debug_assert`s, the bounds seam calls, segmented selection crossfade + ring const; +2 tests |
| `app/bounds.rs` | **new**: the painted-bounds audit seam + frame-level checks; +2 tests |
| `app/export.rs` | sheet open/close transition (`sheet_fade`, `sheet_visible`); +3 tests |
| `app/views.rs` | `status_caption`, `arriving_caption`, `truncated_galley`; pill clock → hh:mm:ss |
| `app/editor.rs` | header constants + `header_layout`/`header_caption_rect`; title row (truncated galley) + capsule cluster; constant caption rows + status fade; pluralization; sheet call; +5 tests |
| `app/mod.rs` | hh:mm:ss `format_elapsed`, `count_noun`, `engine_display`; library arrivals/leaving + `prune_motion`/`row_arrival`; `applied_at`; model status copy; +7 tests |
| `app/library.rs` | insert/remove ease (`ListSlot`, `list_slots`, leaving rows), empty-state fade, model display names |
| `app/home.rs` | recents insert/remove ease, one message slot (error priority), display names |
| `app/model_manager.rs` | `ProgressEase`, `refresh_epoch`/`on_rows_loaded`, `display_name`, status copy; +3 tests |
| `app/models.rs` | human names + raw-id detail, column shift + name truncation, smoothed bar + completion crossfade, row arrivals, status/empty fades |
| `app/settings.rs` | combobox display labels, "applied" pulse, status fade, pluralization, 16 kHz → one file |
| `app/settings_form.rs` | `applied_at`, `confirm_pulse`; +1 test |
| `app/record.rs` | meter line, skipped pluralization, arming caption crossfade, hint fade, status fade, failure fade |
| `app/transcribe.rs` | engine line plain words, model display name, failure fade |
| `app/polish.rs` | pluralized status/error strings |
| `runtime.rs` | `model_rows()` accessor (smoke harness); backlog log grammar |
| `main.rs` | `SURAMAK_SMOKE_MODELS` harness (seeded download state; smoke-only) |

---

## 2. Verification

### 2.1 Gates (final tree)

```text
cargo fmt --all --check                                    → clean
cargo clippy --workspace --all-targets -- -D warnings      → clean
RUST_TEST_THREADS=1 cargo test --workspace                 → 335 passed / 0 failed / 6 ignored
cargo check -p suramak-glass --target aarch64-apple-darwin → clean
```

**Tests: 335 passed + 6 ignored (baseline 310 + 6 → +25 new).** New tests:

* motion: `directional_fades_honour_enter_and_exit_durations`, `interruptions_retarget_from_the_current_value`.
* controls: `capsule_bodies_are_padded_capsules_at_the_designed_height`, `every_glyph_fits_its_control_rect`.
* bounds: `no_control_shape_escapes_its_allocated_rect`, `the_focus_ring_is_the_only_sanctioned_excess`.
* export sheet: `the_sheet_fades_in_over_base_and_out_over_fast`, `the_sheet_fade_snaps_under_reduce_motion`, `a_rapid_toggle_retargets_from_the_current_value`.
* editor header/cluster: `the_header_separator_never_enters_the_title_or_caption_zone`, `caption_rows_stack_from_explicit_heights_with_zero_overlap`, `the_header_rhythm_is_constant_across_panes`, `the_header_zone_has_exactly_one_full_width_rule`, `the_title_line_cluster_has_a_real_gap_and_no_overlap`.
* model manager: `every_manifest_model_has_a_human_display_name`, `progress_ease_is_monotonic_settles_and_snaps_under_reduce_motion`, `a_refresh_bumps_the_epoch_so_rows_replay_their_arrival`.
* settings: `the_applied_confirm_pulse_rises_falls_and_ends`, `a_successful_settings_save_arms_the_applied_confirm`.
* copy/list motion: `the_two_sanctioned_time_formats_are_the_only_clocks`, `counts_are_pluralized_never_bracketed`, `engine_ids_render_as_human_names`, `the_first_library_load_is_static_then_inserts_and_removals_animate`, `a_refresh_that_errored_animates_nothing`, `list_slots_merge_leaving_rows_at_their_old_positions`.

### 2.2 xvfb smokes + the frame cadence budget

```text
plain   SURAMAK_FRAME_STATS=1 SURAMAK_SMOKE_SECS=30            → exit 0, frames=8   (idle, event-driven; 8 of 8 in the first 0.3 s)
record  SURAMAK_SMOKE_RECORD=1 SURAMAK_SMOKE_SECS=30           → exit 0, frames=294 (100 % `recording`, ≈9.8 fps — the 100 ms meter cadence)
models  SURAMAK_SMOKE_MODELS=1 SURAMAK_SMOKE_SECS=20           → exit 0, frames=115 (100 % `download`, ≈5.8 fps — the 150 ms poll cadence)
job     SURAMAK_SMOKE_JOB=fixtures/audio/en.wav SURAMAK_SMOKE_SECS=300 → exit 0, `SMOKE-JOB-OK` (2 segments, model tiny, engine cpu), frames=10 (event-driven)
```

Logs: `build/m9c/smoke-{plain,record,models,job}.log`. The new animations do not hold the app at
tilt: every helper ends, the idle run still presents 8–9 frames in 30 s, and the download's eased
bar rides the existing 150 ms cadence (no extra wakes).

### 2.3 Screenshots + pixel checks (harness `build/m9c-e2e/`)

`seed.py` (as M9b), `shoot.sh` (Xvfb :98, xdotool), `pixelcheck.py` (ImageMagick pixel probes,
`build/m9c-e2e/pixel-checks.log` — **18 PASS / 0 FAIL**).

| shot | what it proves |
|---|---|
| `light/editor-titleline.png` + `titleline-crop.png` | the title row: `en.wav`, the caption `Tiny (fastest) · en · 5 segments · …`, the trash + Polish capsules with a clean gap; the single deck separator **above** the title — no rule in the title/caption zone |
| `light/editor-sheet-open.png` | the export sheet open (white card at its anchor, chips + Destination + Export) |
| `light/editor-sheet-close.png` / `-closed.png` | the fast close caught mid-flight (the card is a ghost blend, not the card, not the pane) and the settled pane after |
| `light/editor-confirm.png` | the in-place confirm: `Delete?` + check + cross capsules + Polish, all separate bodies |
| `light/library-delete-1/2/3.png`, `library-after.png` | the delete: the sidebar re-lays out across the rapid shots (the leaving row's ghost → the settled single row) and the Home status `deleted en.wav` |
| `light/models-download.png` | the model manager: human names + raw-id details, the seeded download at `31% — 180.0 MB / 574.0 MB`, Download disabled |
| `light/record.png` | the record view: timer `00:00:50` and pill `00:00:50` (hh:mm:ss), `the meter shows the last 10 seconds`, the provisional label + on-device line verbatim |
| `dark/editor-titleline.png` (+ crop), `dark/editor-sheet-open.png` | the same cluster + sheet in the dark ladder |

**Pixel-check highlights** (each asserted on the real captures): two capsule bodies with a 9 px
painted gap and nothing in the gap's interior; exactly one full-width rule in the header rule zone
(at the constant separator y) and **no** full-width row in the title/caption text zone; the sheet's
open/ghost/settled pixel values; the record timer ink 139 px wide (hh:mm:ss at 30 pt; the old
`mm:ss.t` was ~7 chars) and the pill clock 115 px (hh:mm:ss at 19 pt; the old `mm:ss` ~57 px); the
download row's accent bar; the sidebar's re-layout across the delete and the settled status line.

### 2.4 Darwin cross-check

```text
cargo check -p suramak-glass --target aarch64-apple-darwin   → clean
```

As in M9a/M9b: the glass crate is compile-checked, never executed (no macOS host here). This slice
adds **no** macOS-only code (UI + a smoke harness only). The two owner-directed fixes are
font-metric-independent *by construction* (constants + truncated galleys), which is exactly the
class the Linux render cannot reproduce — acceptance is the owner's macOS screenshot after the next
pull (recorded in the workorder's terms).

---

## 3. Deviations (each with its why)

1. **Owner-directed fix 1 — the title-line cluster's capsule bodies (an exception to the look
   freeze, scoped).** Requested mid-slice: the delete/trash glyph + Polish (+ Undo) must read as real
   buttons in the iOS-26 Liquid Glass language. Implemented as `Tier::Capsule` (translucent
   `control_fill` + hairline, capsule radius, 28 pt, ≥ `space.s` padding/gaps, fast hover/press, no
   bounce) used only by the title-line cluster; the confirm step reuses the same glyph bodies (the
   trivially consistent tweak the direction allowed). Everything else keeps the frozen look.
2. **Owner-directed fix 2 — the editor header's constant rhythm (the macOS hairline collision).**
   The header below the deck no longer flows from measured text; explicit constants + a pure
   `header_layout` position the title row and the three caption rows, the title is a truncated
   one-row galley, and the deck separator is the only rule in the zone (verified in pixels and by a
   frame-level audit). The visual delta on Linux is ~5 px on the first transcript row (measured), so
   the M9a look is preserved.
3. **Home's import error moved to the one message slot.** While auditing error states, the old
   flow-positioned error line was found to collide with the nav row on every error (it painted at
   the same y as the Models/Settings controls). The error now shares the status slot below the nav
   (error priority), fading in/out with the same helper. Small, deliberate, and inside the copy/
   motion remit; no other Home geometry changed.
4. **`SURAMAK_SMOKE_MODELS` harness (main.rs, smoke modes only).** The VPS has no models and the
   workorder asks for a "model-download state" screenshot; with the env var set *and* a smoke mode
   active, the app seeds the manager table + a mid-flight download in state only (no network, no
   download thread). Inert in normal runs — same pattern as the M9a `SURAMAK_SMOKE_RECORD` harness.
5. **The mid-close screenshot is caught late.** The fast exit is 120 ms and the Xvfb click→`import`
   latency is a large share of it; the first frame after the click is the ghosted one and the next is
   settled. The transition itself is unit-tested; the screenshots show both ends plus the ghost.
6. **Model-manager column shift + name truncation.** The human names are wider than the old raw ids
   and (found in the first screenshot round) ran into the SIZE column. The columns moved right
   (SIZE 215 / INSTALLED 305 / ENCODER 395 / default 475) and the name is truncated to its column
   with the shared `truncated_galley` helper — geometry over measurement, same principle as fix 2.
7. **Reduce Motion skips the settings confirm pulse.** A one-shot pulse has no meaningful
   zero-duration form (it would flash invisibly); the status line already confirms the save. The
   `arrive`-based fades survive as snaps (opacity-only), per the M9a rule.
8. **The confirm-step "Delete?" label stays a plain label** (not a capsule): it is text, not a
   control — the owner's direction named the buttons.
9. **`suramak_playback::format_mmss` is no longer used by the UI** but is kept in the playback crate
   (its own tests cover it). Removing it would touch the protected playback crate for no user-visible
   gain; left for the finish review (§4).
10. **CLI `(s)` strings stay** (`suramak-cli`'s polish output): the copy sweep is the app's UI
    surface; the CLI is a separate surface and was not in scope.

---

## 4. Left for the finish review

* The macOS runtime checks (glass material, NSAppearance, Reduce Motion from `NSWorkspace`) — always
  the owner's Mac; this slice adds nothing macOS-specific.
* `suramak_playback::format_mmss` removal (or a doc note that the UI no longer calls it) if the
  finish review wants the "exactly two formats" rule to hold at the crate API level too.
* The polish bar / polish panel rows still flow below the constant caption block (conditional rows);
  if the finish review wants the *whole* editor header constant, the polish bar can join the
  `header_layout` scheme (it is already outside the title/caption zone).
* `Tier::Capsule` is general-purpose: if the finish review decides other quiet actions should read as
  buttons (e.g. the record view's nav), the tier is ready — but the owner's direction scoped it to
  the title-line cluster only.
* Transcript virtualization (M9b §2.7) and the peaks refinements are untouched, as is everything in
  the protect list.
