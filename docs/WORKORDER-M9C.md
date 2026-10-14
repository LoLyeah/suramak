# WORKORDER — M9c: the motion grammar · copy sweep · remake finish

Landed: M9a (`8e472db`), M9b (`d24a3b2`) — read their reports' "En route" sections (`docs/reports/M9a.md` §4, `M9b.md` §2.7). The look stays **frozen** (no restyle beyond the copy strings and motion named here).
Contracts: `.impeccable/motion-brief.md` (**owner-confirmed** — this slice executes its §5 checklist and the leftover L1/L2/L3 applications) · `docs/M9-BRIEF.md` (principle 7 = copy hygiene; protect list) · `docs/reports/M9-CRITIQUE-A.md` (the copy findings; keyboard/help heuristics).

## Read first
1. `.impeccable/motion-brief.md` in full. 2. The M9a/M9b reports' en-route sections. 3. This workorder. 4. The critique's copy observations.

## Deliverables

### 1) Motion grammar — the full §5 sweep
Walk the brief's §5 state list; make each state's treatment deliberate and Reduce-Motion aware; implement the gaps:
- **Export sheet**: open = base fade + 2 pt rise; close = fast fade. (Currently instant — the named leftover.)
- **Model manager**: progress smoothed (no jitter); download-complete crossfade; row-level state transitions (re-check/delete) at fast/base.
- **Lists** (library + home): insert/remove ease (base); search-filter result fade if search exists (else note it); rename stays motion-free beyond the edit itself.
- **Status/inline messages**: fade in base / out fast; never block input; any hoverable dismissible pauses its timer (only if such a thing exists — else skip + note).
- **Settings**: segmented/toggle state changes fast; a subtle one-shot "applied" confirm after Save; **appearance switch stays instant** (brief §5 — material swaps don't animate).
- **Record lifecycle audit**: idle → arming → recording → stopping → finalizing → saved — verify every transition is deliberate (most exist from M9a); fill gaps.
- **Empty/first-run/error states**: fade-in on arrival (base); nothing loops.
- **Interruption audit**: rapid context switches/route flips retarget from current values (never queue, never snap back); nothing animates forever; durations honored.
- **Reduce Motion**: every entry above snaps; audit the whole list in code and say so per item in the report.
### 2) Copy sweep — one system, plain words
- **Time formats**: exactly TWO sanctioned formats (per brief §3): elapsed **hh:mm:ss** for recordings (record-view timer, pill, sidebar when a duration shows) and **mm:ss.mmm** for the transport. Kill the record view's `mm:ss.t` and the pill's `mm:ss`; document any exception.
- **Record view**: `current recording: N of the last 96 meter samples` → human phrasing (or remove); engine line in plain words (no `planned`).
- **Model display names**: human labels in the UI (e.g. `turbo-q5-0` → `Large v3 Turbo (balanced)`); raw id may stay in the model-manager detail if useful.
- **Pluralization**: `5 segment(s)` → `5 segments`.
- **Sweep**: grep user-facing strings for jargon (`ppm`, `drift`, `revision`, `window(s)`, `samples`); fix them. **Protect list overrides**: keep the provisional/on-device honesty strings exactly as-is.
- Deliver a before→after strings table in the report.
### 3) Defensive audit (open owner question)
The owner flagged an apparent overlap between the title line's delete glyph and its Polish button (unconfirmed on their Mac screenshot; likely a font-metric-class issue that Linux can't show). Audit + TEST the invariant: **no painted control draws outside its allocated rect** (the focus ring's +1.5 pt is the only sanctioned excess) — add a debug seam/assertion covering `controls.rs` and the title-line cluster, and verify the trash ↔ Polish spacing math explicitly. Document what the invariant guarantees.

## Do NOT
- Restyle the M9a/M9b look; touch asr/capture/live, playback internals, glass runtime, taps, instance, shutdown; add crates or fonts; `git commit`/`push`; modify files outside `/home/ubuntu/suramak`.

## Verification
- fmt clean; clippy `--workspace --all-targets -- -D warnings` clean; `RUST_TEST_THREADS=1 cargo test --workspace` green with new tests: sheet open/close state transitions, the two time-format fns (+ rejecting the old variants), pluralization, model display-name map, painted-bounds assertions.
- xvfb smokes green (plain + record + job); screenshots: sheet open, a model-download state, record view (new copy), library with an item mid-delete or similar; darwin glass check clean.
- Report `docs/reports/M9c.md`: the §5 checklist table (state → treatment → reduce-motion), the copy before/after table, painted-bounds audit results, deviations with whys, and a "left for the finish review" list (anything deliberately untouched).

## Finish review (after this slice, run by someone else)
The parent will run a fresh-eyes finish pass over the whole remake (the critique rubric + the motion checklist) with new screenshots — nothing in this slice needs to pre-empt it; just leave the tree honest.
