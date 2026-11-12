# WORKORDER — M20B: frosted material + vector primitives

Plan: `docs/reports/M20-ui-refresh-plan.md` (Carbon high) — slice 2 of 5.
Read the craft floor before editing:
`/home/ubuntu/.hermes/skills/creative/impeccable/reference/craft-floor.md`.

## Why

Android has no system backdrop blur for one GL surface, so "frosted" must be
honest, in-surface material: layered veils, a 1 px specular rim, hairlines and
a whisper of grain — never a painted imitation of macOS Liquid Glass. On macOS
the same tokens must stay transparent over the real `NSGlassEffectView`.

## Deliverables

1. **`MaterialBackend` capability** (in theme or a small `deck/material.rs`):
   `SystemGlass` on macOS (frost tokens zero, surfaces transparent),
   `PaintedFrost` on Android/Windows/Linux.
2. **New tokens, authored per ladder and
   contrast-tested**: `frost_veil_top`, `frost_veil_bottom`, `rim_specular`,
   `rim_hairline`, `frost_grain_alpha`, `dock_scrim_top`, `dock_scrim_bottom`.
   Values from the plan: dark veil 4.5% → 1.2% white over `card`/`sheet`;
   light 62% → 38%; rim hairline white 6% dark / black 7% light; specular
   white 13% dark / 75% light; grain 1.8% dark / 1.2% light.
3. **The primitives** in `deck/material.rs` (or `surfaces.rs`):
   - `frosted_panel(ui, tokens, rect, radius, plane)` — the 4-vertex veil
     `Mesh` (VERTICAL gradient, no texture), the hairline rim, and the top
     specular arc on the upper corners; `plane` ∈ {Card, Sheet, Dock} decides
     the shadow budget (cards 0, sheets/dock 12 pt blur +4 pt Y 22% black);
   - `scrim(ui, rect, tokens)` for sheets;
   - a one-time 64×64 deterministic hash-noise `ColorImage` tiled at
     `frost_grain_alpha` (Sheet/Dock only; cards get the veil, not grain).
4. **`deck/icons.rs`**: a tiny painted-icon kit — 1.75 px round-cap strokes,
   18/20 pt sizes — with the icons the plan names: gear (settings), magnifier
   (search), import-wave, mic, chevron, check, download, share, speaker,
   close, trash. Each returns a Response via `a11y::touch` with a spoken
   label; no new dependency, no SVG parser.
5. **Card anatomy helper**: one function that draws a card = frosted panel +
   standard 12 pt internal padding + optional 1 px divider; migrate NOTHING
   yet (slices 3–5 consume it), but ship one demo golden that shows Card,
   Sheet and Dock side by side on the phone viewport in light and dark.
6. **Tests**: tokens exist per ladder + contrast assertions for ink on the
   veiled fills; the grain texture is deterministic (same bytes on two runs);
   SystemGlass zeroes the frost alphas; golden `material-kit.png` (light+dark).

## Verification

- fmt/clippy -D warnings; the Android NDK cross-check
  (`cargo ndk -t arm64-v8a check`/`clippy` on both Android crates); workspace
  tests green (one full run at the end).
- Report: `docs/reports/M20b-material.md` — the token table with the contrast
  numbers, the primitive signatures, the icon list, the two goldens, and the
  honest note that screens adopt them in slices 3–5.

## Do NOT

- No screen restyling (slices 3–5); no fake glass on macOS; no new crates; no
  blur shader that ignores the frame budget; no engine changes; no CI edits;
  no git commit/push; no sudo.
