# WORKORDER — M20E: the Editor and sheets, finished (last refresh slice)

Plan: `docs/reports/M20-ui-refresh-plan.md` §D — slice 5 of 5. Read the craft
floor first: `/home/ubuntu/.hermes/skills/creative/impeccable/reference/craft-floor.md`.
Landed: type scale, material kit, icons, motion helpers, the redrawn Library
(M20C) and Settings (M20D). This slice brings the editor to the same level and
closes the refresh.

## What changes (phone; desktop keeps its layout apart from shared chrome)

1. **Transcript body** at the type scale: 15.5/24 body, row title 15/20 for
   segment headers; timecodes and CPL-style numerals in the tabular helper
   (fixed-width cells) so they stop jittering during playback.
2. **The speaker lane and badges**: lane stays 8 px; badges use the icon kit's
   check/spacing and the type scale's caption; rename-in-place keeps its
   affordances (a small pencil icon from the kit).
3. **`CPL` → "Line length"** on the phone (the review flagged the jargon);
   the desktop keeps `CPL` only if its own critique says so — phone first.
4. **Sheets** (export, speaker picker, quit confirm, model choice) all use
   the Sheet plane + the plan's present/dismiss motion (slide + scrim) and the
   `frosted_panel` rim; their buttons take the control kit's pressed spring.
5. **Onboarding**: the hero uses Newsreader at the display token, the icon kit
   for the three steps, and the sheet motion; copy unchanged.
6. **Motion**: playback follow uses the existing seek easing; the active
   segment's highlight crossfades at FAST; nothing idles.

## Verification

- Phone goldens re-blessed: editor, editor-speakers, export-sheet,
  export-written, onboarding, download-progress, download-resume, diagnostics;
  add an editor-scrolled golden showing numerals in fixed-width cells.
- The a11y audit still records every control (the suite asserts the count).
- fmt/clippy -D warnings; NDK cross-check on both Android crates; workspace
  tests green (one full run at the end).
- Report: `docs/reports/M20e-editor.md` with before/after shots and the
  refresh's closing checklist (what shipped across M20A–E, what remains).

## Do NOT

- No Library/Settings rework (landed); no engine changes; no desktop layout
  changes beyond shared chrome/sheets; no new crates; no CI edits;
  no git commit/push; no sudo.
