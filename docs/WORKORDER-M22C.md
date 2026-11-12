# WORKORDER — M22C: library-wide search + richer rows

Roadmap: `docs/reports/M21-roadmap.md` #3 (Carbon high; the M15 note required
an FTS audit before building — this workorder IS the gate's output: FTS5 is
in the bundled SQLite, no new dependency, and the audit question, "does a
phrase search across 200 transcripts stay under 50 ms on a 2-core box", is a
test in this slice).

Today search is per-transcript (search-with-seek inside one editor). The
Library's field only filters titles. This slice makes the Library search what
was SAID, and makes rows carry enough shape to be scanned.

## Deliverables

1. **FTS5 index over segments** (additive schema, next version): a virtual
   table `segment_fts(transcript_id, seg_index, speaker, text)` with
   `unicode61 remove_diacritics 2` and a trigram mirror only if the audit
   says prefix search is too slow (measure, don't guess). Populated on
   transcript write; rebuilt for existing rows on open when the index is
   empty (one-time, progress-logged, bounded by a transaction per transcript).
2. **Search API**: `Store::search_segments(query, limit)` → hits with
   transcript id, segment index, speaker, a ±60-char snippet with the matched
   term bounded by `[`/`]`, and rank. Query parsing: quoted phrases, bare
   terms AND-ed, `speaker:` prefix filter (names as the user typed them).
   Empty/garbage queries return nothing (no error toast).
3. **The Library field searches both**: title matches first, then segment
   hits as a "in transcripts" section — each row shows the transcript title,
   the snippet with the match emphasized, the timecode, and taps through to
   the editor scrolled to and highlighting that segment (the existing seek
   path). Clearing the field restores the plain list.
4. **Richer rows**: a 28 px mini-waveform (down-sampled RMS from the stored
   peaks the editor already computes — reuse, don't add analysis), speaker
   pips (up to 4, then +N), and the duration in the tabular face. Rows must
   not regress the M20C press motion or the dock clearance.
5. **The audit test**: a fixture of 200 synthetic transcripts × 40 segments
   (a realistic 8,000 segments, ~400k words): build the index, then measure
   p50/p95 of 20 queries on this 2-core box. Assert p95 < 50 ms and record
   the numbers in the report. If the assert fails, stop and report the
   numbers — do not lower the bar.

## Verification

- Store tests: index population on write, rebuild when empty, phrase/AND/
  speaker: queries, brackets in snippets, limit/ordering.
- UI tests: the Library field's two sections, tap-through seek, clear
  restores, pips/pips-overflow, and goldens re-blessed (library,
  library-empty, library-light, library-bottom) + a new `library-search`
  golden.
- fmt/clippy -D warnings; NDK cross-check on both Android crates; one full
  workspace run (baseline 1281).
- Report: `docs/reports/M22c-library-search.md` with the audit numbers,
  before/after shots, and the exact FTS5 schema.

## Do NOT

- No new crates (FTS5 is already linked); no cloud/semantic search; no
  changes to the editor's own search; no dock/clearance changes; no CI edits;
  no git commit/push; no sudo.
