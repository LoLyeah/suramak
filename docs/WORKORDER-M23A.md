# WORKORDER — M23A: manual speaker tagging on the phone, and speakers in exports

Plan: `docs/reports/M23-speaker-tagging.md`. The owner's literal ask is
"tag Speaker 1/2/3 for different speaker" — per recording, no voiceprints, no
engine needed. The store primitives already exist (`rename_speaker`,
`set_speaker_color`, `merge_speakers`, `split_speaker`, time-anchored turns,
derived ranges). This slice builds the missing verbs and the phone UI.

## Part 1 — the store verbs (core)

1. `add_speaker(transcript_id)` → next free ordinal, `default_label`, next
   unused colour slot; idempotent under a transaction.
2. `assign_speaker(transcript_id, speaker_id, range)` — reassign the turns of
   a time range (one segment, or from-segment-to-end) to a speaker, splitting
   the containing turn at the boundaries so ranges stay consistent. This is
   the "tap a segment → Speaker 2" path; it must be exact, not approximate.
3. `clear_speakers(transcript_id)` — removes speakers + turns, leaving
   Whisper segments untouched (the derived view returns to unlabelled).
4. `speakers_edited_at(transcript_id)` — a monotonic stamp for the export
   honesty note.
5. Undo: every mutating verb returns a small undo token the caller can apply
   (or, if the store already has an undo pattern, follow it — do not invent a
   second one).

## Part 2 — the phone UI

6. **Editor**: coloured badge + a 4 px left rail per speaker turn (desktop
   keeps 8 px); unlabelled segments show a muted `[+ Speaker]` pill; the
   bottom bar's Speakers button reads `Tag speakers` or `3 speakers`.
7. **Speaker sheet** (tap a header): rename field, colour dots, `Merge
   into…`, `Remove label`. Motion: the Sheet plane + slide, pressed-spring
   buttons, Undo toast.
8. **Turn strip** (tap a segment or the pill): chips for each speaker plus
   `+ New`, and a scope toggle `This turn | From here down`, with Undo.
9. **Speakers sheet** (bottom-bar button): list, `Add speaker`, `Clear all
   speakers` (destructive, confirm). Desktop's Re-run stays desktop-only.
10. Everything reachable with a thumb; no long-press requirements.

## Part 3 — exports

11. `Include speakers` toggle in the export sheet; SRT `Speaker 1: …`, VTT
    `<v Speaker 1>…`, MD `**Speaker 1:** …`; names when renamed.
12. If `speakers_edited_at` is newer than the last export's stamp, the sheet
    shows "Speakers edited since last export — export again to update".
    Never rewrite an exported file silently.

## Verification

- Core tests: next-ordinal/colour allocation, assign single segment, assign
  from-here-down, splitting a containing turn at both boundaries, clear
  restores unlabelled, stamp monotonicity, undo of each verb.
- Export tests: all three formats with generic and renamed speakers; the
  toggle off produces today's bytes exactly (regression).
- UI tests: the sheets' state machines, the `[+ Speaker]` pill, the button's
  two labels, goldens (editor, editor-speakers, export-sheet) + new
  `editor-tag-speakers` and `editor-speaker-sheet`.
- fmt/clippy -D warnings; NDK check + clippy on both Android crates; one full
  workspace run (baseline 1281).
- Report: `docs/reports/M23a-manual-tagging.md` with before/after shots.

## Do NOT

- No diarization engine work (that is M23B's gate); no voiceprint storage; no
  cross-file anything; no new crates; no CI edits; no git commit/push; no
  sudo.
