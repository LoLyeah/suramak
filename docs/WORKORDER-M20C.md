# WORKORDER — M20C: the Library, redrawn (first visible slice)

Plan: `docs/reports/M20-ui-refresh-plan.md` §D — slice 3 of 5. Read the craft
floor first: `/home/ubuntu/.hermes/skills/creative/impeccable/reference/craft-floor.md`.
The material kit and type scale are landed: `deck/material.rs` (frosted_panel,
scrim, dock_scrim, card), `deck/icons.rs` (11 icons), `deck/type_scale.rs`.

## What changes (phone only; desktop keeps its layout)

1. **App bar**: the serif title stays (the house voice) but at the type scale's
   title token; "Settings" text becomes the **gear icon** (20 pt, 44 pt hit
   box) with the spoken label "Settings". Keep the current golden's spacing
   unless the plan's 8 pt rhythm says otherwise.
2. **Search**: the plain field gains the **magnifier** (18 pt inline) and the
   type scale's caption placeholder; the field is a frosted panel (Card plane,
   0 shadow) not a flat rect.
3. **Model banner** (the "Get a model" card): ONE shrink-wrapped frosted
   banner — headline, one muted reason line, an inline model pill + primary
   Download button, footer storage caption. No nested inner card, no dead
   space (the current shot has ~80 pt of it). When a model is verified and in
   use, the banner disappears entirely.
4. **Library rows**: keep the painted kit, but apply the type scale (row title
   / meta / numerals), a 1 px hairline divider (not a full-width rule), and
   the row's pressed wash via the material primitives. Row press motion uses
   the M20A helpers (FAST in, spring out).
5. **Empty state**: centred between banner and dock — 28 pt waveform glyph in
   ink_faint, Newsreader 21 pt "No transcripts yet", one 2-line prompt in
   ink_soft. Copy unchanged (owner-approved).
6. **Bottom dock**: floating frosted **Dock plane** (16 pt inset, 12 pt above
   the gesture inset, `dock_scrim` fade behind); left Import (secondary
   frosted pill + import-wave icon), right Record (primary, `record_red` dot
   + mic icon, high-contrast fill — NOT the generic blue). The dock floats
   OVER the list; the list's bottom padding must clear it (and the gesture
   inset) so the last row is never hidden.
7. **Onboarding/first-run sheet**: adopt the Sheet plane + the plan's
   present/dismiss motion (slide + scrim); copy unchanged.

## Verification

- Phone-viewport goldens re-blessed: library, library-empty, library-light,
  launch-library, first-run-download, onboarding; plus new goldens for the
  in-use banner (absent) and a scrolled-to-bottom library (dock clearance).
- The emulator smoke unchanged except: its Library screencap gate must still
  pass (the new dock/banner change pixels but not structure); the
  `SURAMAK-LAYOUT` clearance assertion is Settings-only and stays.
- Tests: dock clearance math (list padding ≥ dock height + inset); the gear
  icon's a11y label; the banner's absent state; type-scale application
  (a11y audit still records every control).
- fmt/clippy -D warnings; NDK cross-check on both Android crates; workspace
  tests green (one full run at the end).
- Report: `docs/reports/M20c-library.md` with before/after shots (paths) and
  the honest list of what still looks dated until slices 4–5.

## Do NOT

- No Settings/editor changes (slices 4–5); no new crates; no desktop layout
  changes; no fake glass; no engine changes; no CI edits; no git commit/push;
  no sudo.
