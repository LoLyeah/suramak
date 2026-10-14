# WORKORDER — M9a: the remake, slice 1 (material · deck · controls · motion · appearance)

Owner decisions (2026-10-14): composition **C — Deck** (`.impeccable/mocks/m9/C-deck.png`); appearance **System-following with Light/Dark override** (default System).

## Read first, in this order
1. `docs/M9-BRIEF.md` — principles + protect list; these are the acceptance targets.
2. `docs/reports/M9-CRITIQUE-A.md` — the defect list you are fixing (2 P0s, 3 P1s).
3. `.impeccable/mocks/m9/C-deck.png` + `C-deck.prompt.txt` — the composition. Micro-text noise in the comp is placeholder; the composition/geometry is the spec. (A/B comps are reference only.)
4. This workorder. Where this conflicts with older docs, this wins for UI files.

## Deliverables

### 1) Theme tokens — ONE material ladder, TWO appearances (`crates/suramak-ui/src/theme.rs`)
- Keep the existing system-font loading (`SANS_FACE`/`SERIF_FACE`/`MONO_FACE`; `/System/Library/Fonts/*.ttf`). Do not add fonts.
- Replace the ad-hoc palette with a token struct: ink / ink-soft / ink-faint, hairline, accent (+ tint), record-red; ladder fills: `window` (stays transparent — real glass behind), `panel` (one translucent fill), `card` (fill + hairline + shadow recipe); spacing scale 4/8/12/16/24; radii (deck/card/control/pill); stroke widths; control-state deltas.
- Author the ladder in BOTH light and dark.
- Rework egui `Visuals` fully from tokens: `panel_fill`, `window_fill`, `faint_bg_color`, `extreme_bg_color` must NOT stay stock near-black (set transparent or panel-level — critique P0-2's root cause), widget bg/weak/hover/active per tier, selection, strokes, shadows. Nothing stock-near-black or flat-white may survive anywhere.

### 2) Appearance: System / Light / Dark
- `crates/suramak-core/src/settings.rs` — add `appearance: Appearance` enum (`System | Light | Dark`, default `System`) with serde default + roundtrip test.
- `crates/suramak-glass/src/macos.rs` lines ~72–79 pin the window to DarkAqua (M1 placeholder) — remove that. Add a public `set_appearance(mode)` that sets/clears `NSAppearance` (`NSAppearanceNameAqua` / `NSAppearanceNameDarkAqua` / nil → system) on the window AND the glass container view(s) so the real material follows. Every other glass behavior stays byte-identical (no geometry/blend/runtime changes — appearance adaptation is the only sanctioned glass edit).
- Shell: follow the system live — read winit `Window::theme()`, handle `WindowEvent::ThemeChanged`; mode=System applies the effective theme to egui visuals + titlebar + glass; forced modes override. Linux: System → Light (document in code).
- Settings UI (`app/settings.rs`, `app/settings_form.rs`): a new "Appearance" row (segmented: System / Light / Dark), persisted, applied live without restart.

### 3) Editor rebuilt to comp C — the Deck (`app/editor.rs`, `app/views.rs`)
- Under the titlebar: ONE full-width control deck band (panel-level surface, ~96px):
  - LEFT: compact transport capsule (play/pause + skip-to-start, painted glyphs, follow the existing `transport_glyph` pattern) — no duplicated play controls anywhere;
  - MIDDLE: waveform strip, full width (existing data source for now — real peaks are slice 2; the geometry here must be final);
  - RIGHT: time readout (`mm:ss.mmm / mm:ss.mmm`) + ONE tier-2 `Export…` button. Nothing else floats in this header.
- Under the deck: title line (serif, ~20px) with a single quiet `Polish` text button at far right.
- Transcript below as clean rows: mono timecode gutter (muted; one time-format system — `hh:mm:ss` elapsed), serif body ~16px at ~66–70% measure, subtle hover, active segment = 3pt accent edge bar + faint tint. Inline editing keeps its current behavior, restyled only.
- Floating pill: semantics/position UNCHANGED (M8.5 contract, 24pt inset, 184×52 float_rect). Audit EVERY state for visual collision — the old export/pill overlap is a named defect.
- Record view: composition unchanged (protect list); it receives the new tokens/controls/motion only. Home: same.

### 4) ONE control system (`app/controls.rs`, new)
- Three tiers (primary / secondary / tertiary), fully painted — no emoji/unicode glyph buttons. Replace the emoji set in `editor.rs` (~lines 215–262: 🗑 ✔ ✖) with painted glyphs, including trash + confirm/cancel.
- Every state defined: rest / hover / active / focus / disabled, with token deltas; route every control call site through it (home, editor, record, settings, models, export, polish, library).
- Audit the titlebar strip for dead affordances; remove what the deck now owns.

### 5) Motion system (`crates/suramak-ui/src/motion.rs`, new)
- Tokens: fast 120 ms / base 180 ms / slow 240 ms, ease-out only. Helpers for fade / slide / tint crossfade / scale; egui `animate_*_with_time` where it fits, interpolation otherwise. Every animation must END (no perpetual repaint past its duration).
- Reduce Motion: read macOS `NSWorkspace.accessibilityDisplayShouldReduceMotion` (objc2, same pattern as existing platform calls; refresh when settings open/apply) — when ON, durations → 0 (snap; keep opacity only where legibility demands).
- Apply at minimum: route changes (slow — fade + ~2px rise, no teleport), pill idle↔recording morph (base — label crossfade + color), live transcript line arrivals (base — fade + rise), active-segment change (fast tint), every control hover/press (fast). Text measure must not shift during animation.

## Do NOT
- Touch: asr/capture/live pipeline, playback semantics/internals, glass material geometry/blend/runtime beyond appearance, taps, instance guard, shutdown.
- Add fonts or crates (if a crate seems required, argue it in the report; prefer make-before-break).
- `git commit`/`push`; modify anything outside `/home/ubuntu/suramak`.

## Verification (all must pass; evidence in the report)
- `cargo fmt --check`; `clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` (use `RUST_TEST_THREADS=1`; `E2E_LOCK` exists) — all green. New tests: appearance serde roundtrip + live apply; motion helper math (incl. reduce-motion=0); token coverage (no stock `extreme_bg`); deck geometry anchors (transport / readout / export); control-state deltas.
- xvfb smoke: plain + job runs pass. Screenshots of Home / Editor / Record / Settings in BOTH forced light and forced dark. Pixel-check: transcript-region background not near-black in either appearance; active-row tint present (sample px); deck band present.

## Report — `docs/reports/M9a.md`
Per-file change summary; token table (both appearances); before/after screenshot paths; test counts; every deviation with its why; anything discovered en route for slice 2.
