# WORKORDER — M15-C: search enhancement (M15.7)

Plan source: `docs/PLAN-M15-M18.md` §3 M15.7 (size M). Tree: `main` at `2d989e2`
(M15-B merged: 681 tests, schema v3 with `segments.derived` provenance + 4 new
tables, split/merge undo, glossary). Focus: widen the workbench's search into a
findable, seekable surface without regressing the Indonesian case.

## Deliverables

1. **FTS5 upgrade over the literal LIKE scan** (the M15-A audit recorded the
   old path): build an `fts5` virtual table over transcript + segment text,
   kept in sync with every mutation path (insert/edit/delete/split/merge/
   restore — reuse the transaction hooks where they exist; cover the glossary/
   dictionary corpus too), additive schema step + one-time backfill (bounded
   transaction sizing, no giant single write for a large corpus).
2. **Timestamp-bearing snippets + seek**: search results carry their segment
   start; clicking a result seeks the library's playback state (the same state
   the M15.6 playback UI will consume — wire through the existing state struct,
   no new screens); snippet text is generated with a configurable context
   window (chars), Ellipsis boundaries documented.
3. **Language baseline**: no English stemming by default (it damages
   Indonesian; `simple` or `unicode61` tokenizer with Latin-1 extended case
   folding is the default); case-insensitive matching for both EN and ID;
   mixed-language queries return segments in EITHER column; literal Indonesian
   terms (e.g. `saya`, `kayak`, `Pegawairungu`) find exactly themselves.
4. **Empty/null handling**: segments with no text do not appear; missing media
   does not break seeking (the search row stays; the seek is a no-op with the
   documented toast/flag).
5. **Consistency tests**: insert/edit/delete/split/merge/restore/reopen — FTS
   rows must agree with the store rows after each (same discipline as M15-B's
   search-consistency assertions); large-corpus responsiveness (≥10k segments)
   for a bounded query (document the measured ms on this 2-core VPS).
6. **Migration**: v3→v4 additive; refuse a newer schema; backfill in bounded
   chunks; a corrupt/partial index rebuilds itself (deterministic full rebuild
   is acceptable, logged once).
7. **UI**: the search input gains the snippet + seek affordance in the existing
   library view (no new screens; panel modal if needed).

Verify (the gate): insert/edit/delete synchronization; migration backfill;
mixed-language + literal-ID queries; the ≥10k responsiveness number recorded;
missing-media handling; fmt/clippy/workspace-tests green (baseline 681,
grow; ONE full workspace run at the end — crate-scoped tests meanwhile).

## Do NOT

- No stems (any language), no tokenizer complexity from external crates unless
  inline; no new dependencies; no CI edits; no Android work; no git commit/
  push, no sudo. Report: `docs/reports/M15c-search.md` (snippets rules,
  tokenizer choice + why, the perf numbers, boundaries).
