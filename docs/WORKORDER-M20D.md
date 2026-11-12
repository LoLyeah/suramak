# WORKORDER — M20D: Settings, de-jargoned and grouped

Plan: `docs/reports/M20-ui-refresh-plan.md` §D — slice 4 of 5. Read the craft
floor first: `/home/ubuntu/.hermes/skills/creative/impeccable/reference/craft-floor.md`.
Landed: type scale, material primitives, icon kit, motion helpers, the redrawn
Library (M20C). This slice makes Settings feel like the same product.

## What changes (phone only)

1. **Grouped model card**: the four per-model cards become ONE frosted card
   with hairline dividers; rows show a **human tier name** first — Quick test
   (78 MB), Compact (148 MB), Balanced (488 MB · Recommended), Highly accurate
   (574 MB) — with the raw ids (`tiny`, `base`, `small`, `turbo-q5-0`) moved to
   a small mono second line (or the Diagnostics sheet) so support can still
   see them. Keep the five-state actions and the truthful header from M19B.
2. **Section rhythm**: 8 pt grid; section labels at the type scale's 11 pt
   CAPS; cards use the Card plane (veil + hairline rim, 0 shadow).
3. **De-jargon the primary screen**: `CI + tests`, `optional/legacy`,
   `default (macOS)`, `pyannote-segmentation-3.0 + CAM++`, `CPL` and env-var
   names leave the primary Settings surface — rewrite as human copy or move to
   Diagnostics. The Quit row is Android-hidden (`#[cfg(not(target_os =
   "android"))]`) — a phone has no "quit the app" affordance beyond the system.
4. **Appearance control**: segmented System/Light/Dark — selected = accent
   tint fill + 1 px accent stroke at 45% (the plan's rule), not a bright
   outline.
5. **Diagnostics entry** becomes a row with a chevron and the count of checks;
   the Quit confirm sheet (desktop) adopts the Sheet plane + slide motion.
6. **Motion**: rows press with FAST-in/spring-out; the list keeps the
   scroll/cancel behaviour from M19A untouched.

## Verification

- Settings phone goldens re-blessed (light + dark + bottom-inset) and a new
  grouped-models golden; a grep test asserting zero `CI + tests`, `macOS`,
  `pyannote`, `SURAMAK_` env names on the primary Settings surface.
- The emulator smoke's Settings assertions must stay valid: the
  `SURAMAK-MODEL` row-state lines (M19B) still exist with the SAME state slugs
  (display names may change; the slugs are the machine contract).
- fmt/clippy -D warnings; NDK cross-check on both Android crates; workspace
  tests green (one full run at the end).
- Report: `docs/reports/M20d-settings.md` with before/after shots.

## Do NOT

- No Library/editor changes; no model-state logic changes (display only); no
  new crates; no desktop layout changes beyond the sheet's material; no CI
  edits; no git commit/push; no sudo.
