# WORKORDER — M23A2: the speaker screen, judged by a design director

M23A landed manual speaker tagging (1335 tests). A design review of
`docs/reports/M23a-shots/editor-tag-speakers-after.png` found four things
that keep the screen reading as a prototype. Small slice: one screen, its
widgets, goldens.

## Fixes

1. **The advisory stops looking like debug output.** "Line length 47" in
   orange on the selected row reads as a stray debug label. Keep the advisory
   (it is real product) but present it as a quiet caption chip at the row's
   trailing edge, in the helper tone, only when the row is selected *and* the
   value is outside the target band; otherwise no chip. Same for "Pace".
2. **The timeline strip must account for unassigned audio.** Today it draws
   only the assigned speaker chunks, so the strip appears truncated. Unlabelled
   stretches paint in a muted tone with a 1 px hairline top, so the strip always
   spans the recording and the silence between labels is visible.
3. **Unlabelled rows get a state, not an absence.** A 4 px dashed muted rail
   plus the existing `+ Speaker` pill, so an untagged row reads as
   "unassigned" rather than as a different kind of row.
4. **The words stop being ambiguous.** The pill says `Assign speaker`; the
   bottom-bar control reads `Speakers · 2` (count as the secondary line, not a
   sentence); the pencil's a11y label is `Rename speaker`, and the header's is
   `Speaker options`.
5. **Badges stay consistent**: a default speaker shows its numeral in the
   badge and its full label (`Speaker 1`) in the header; a renamed one shows
   the initial and the name. Both always carry the name column — no row where
   the identity is only an icon.

## Verification

- Goldens re-blessed: editor, editor-speakers, editor-tag-speakers,
  editor-speaker-sheet, editor-turn-strip; add `editor-advisory-chip` and
  `editor-unassigned-rail`.
- Tests: the chip's condition (inside band → absent), the strip spanning the
  full duration with unlabelled stretches painted, the dashed rail on
  unlabelled rows, the two labels, a11y names.
- fmt/clippy -D warnings; NDK check + clippy on both Android crates; one full
  workspace run (baseline 1336).
- Report: append §"design review" to `docs/reports/M23a-manual-tagging.md`.

## Do NOT

- No store or export changes; no diarization work; no new crates; no CI edits;
  no git commit/push; no sudo.
