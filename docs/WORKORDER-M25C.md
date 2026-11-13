# WORKORDER — M25C: a session is one meeting, not five files

M25B/M25B2 made long takes split at pauses. The parts are honest, but they are
also **five rows in the library** where the user has one meeting. This slice
gives the parts a session identity, one library row, and one set of notes
written over the whole meeting with citations that still land.

## Deliverables

1. **Session identity (additive schema v13)**: `transcripts.session_id` (uuid,
   nullable) + `transcripts.part_index`/`part_count` (already implied by the
   `Part N of M` title — make them columns, not string parsing). Recording
   assigns one `session_id` at start; every rotated part inherits it. An import
   split by the cap does the same. Existing rows are untouched (NULL).
2. **One library row per session**: a session with ≥ 2 parts collapses into a
   single row — title without the `Part N` suffix, meta `3 parts · 2 h 10 m`,
   and a small part-count chip. Tapping it opens a **session view**: the parts
   listed in order with their durations and a one-line opening of each. Single
   parts keep today's row exactly.
3. **Session notes**: the Notes sheet gains a scope when the transcript belongs
   to a session — `This part` / `Whole session` (default: whole session). The
   whole-session run feeds **all parts' segments in order** with
   session-absolute times (part offsets accumulated), and the citation
   validator checks against that same absolute timeline. Stored with
   `session_id` set (notes table gains a nullable `session_id`; one row per
   (session, preset)).
4. **Honesty**: a session with parts that failed to transcribe shows
   `2 of 3 parts transcribed` and the whole-session notes refuse with a named
   reason (`session_incomplete`) rather than writing minutes over a hole.
5. **Export**: exporting from the session view writes ONE Markdown file with a
   `## Part N — mm:ss` divider per part and the session-absolute times in the
   body. Per-part export from a part's editor stays byte-identical to today.

## Verification

- Store tests: v12→v13 upgrade on a populated library; session grouping and
  ordering by part_index; a NULL session renders as today; notes keyed by
  (session, preset) replace on regenerate.
- Pure tests: absolute-time mapping (part 2 starts at part 1's duration, exact
  to the sample/second), the incomplete-session refusal, the export divider
  format, and that the citation validator accepts a citation whose segment
  lives in part 3.
- UI tests: the collapsed row's meta and chip, the session view's ordering, the
  scope toggle's two labels, and the `2 of 3` line.
- fmt/clippy -D warnings; NDK check + clippy on both Android crates (env:
  ANDROID_NDK/HOME/ROOT + CMAKE_GENERATOR=Ninja); one full workspace run
  (baseline 1478).
- Report: `docs/reports/M25c-sessions.md` with a real 3-part synthetic session
  and its rendered notes.

## Do NOT

- No chunked decoding; no cross-part editing (each part stays editable on its
  own); no new crates; no queue journal/resume changes; no CI edits; no
  git commit/push; no sudo.
