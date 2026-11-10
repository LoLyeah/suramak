# M15-C — FTS5 search, timestamped snippets and seek (M15.7)

Status: complete, gates green. Scope from `docs/WORKORDER-M15C.md`; parent plan
`docs/PLAN-M15-M18.md` §3 (M15.7). No app version bump (v0.4.0 is the owner's
call when all of M15 lands). No commit was made, no dependency was added, no
engine call site was touched, no Android code was touched.

## Result in one line

Library search is now SQLite **FTS5** over a schema-v4 index (`search_fts` +
`search_docs`) that every store mutation keeps in step — insert, edit, delete,
split, merge, undo/restore, glossary/dictionary saves — returning ranked,
timestamped snippets; clicking one opens the transcript and parks the existing
playback transport at that segment, and an index that disagrees with the store
(or fails FTS5's integrity check) rebuilds itself on open.

## Gates

| gate | result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **722 passed / 0 failed / 8 ignored** (baseline 681/0/8; **+41**) — final full run; one UI regression test added afterwards, so the current tree is 723 (both crates re-run green) |
| schema | v3 → **v4**, additive only (one virtual table + one map table) |
| new dependencies | none (FTS5 is already compiled into the bundled SQLite) |
| FTS5 availability | verified: bundled SQLite **3.46.0**, `libsqlite3-sys` 0.30.1 builds with `-DSQLITE_ENABLE_FTS5` in its `bundled` path |
| disk | `target/` 7.7 GB; 14 GB free on `/` after the release build + workspace run |
| perf (≥10k segments) | **4.47 ms** median for a bounded 50-hit query (release), 12.5 ms debug — see §7 |

## 1. The index (schema v4)

Two additive objects, no existing table touched:

```sql
CREATE VIRTUAL TABLE search_fts USING fts5(
    body, original,
    tokenize = 'unicode61 remove_diacritics 1'
);
CREATE TABLE search_docs (
    rowid INTEGER PRIMARY KEY,
    doc_key TEXT NOT NULL UNIQUE,   -- 'seg:<uuid>:<idx>' | 'title:<uuid>' | 'glossary:<pos>' | 'dict:<pos>'
    kind TEXT NOT NULL,
    transcript_id TEXT,
    segment_idx INTEGER
);
CREATE INDEX search_docs_transcript ON search_docs(transcript_id);
```

**Documents.** One per segment (only when its current text has a non-whitespace
character), one per transcript title, one per glossary term, one per dictionary
entry. `body` is the current text; `original` is the preserved inference text
(`source_text`), stored as `NULL` when it equals `body` so it is not indexed
twice. A dictionary entry uses `body = from_text`, `original = to_text`; a
glossary term uses `body = term` only.

**Why a separate document map.** FTS5's implicit rowid is only addressable if
something stable remembers it. `segments` has a composite primary key and no
`INTEGER PRIMARY KEY`, and SQLite explicitly allows `VACUUM` to renumber such
tables' rowids — so an external-content FTS5 table keyed on `segments.rowid`
would silently desynchronise after `Store::backup_to` (`VACUUM INTO`).
`search_docs.rowid` is an explicit `INTEGER PRIMARY KEY`, `doc_key` is the
logical identity, and the FTS row is addressed by the map's rowid. Deletes and
edits are then O(log n) instead of a scan of the FTS content table, and no
`VACUUM` can renumber the identity.

**Sync points (all inside the caller's existing transaction/savepoint):**

| mutation | hook | index effect |
|---|---|---|
| `insert_transcript` | `write_segments` (per row) + title doc | N segment docs + 1 title doc |
| `upsert_segments` (CLI polish) | `write_segments` + stale-tail delete | rewrites the rewritten rows, deletes docs past the new length |
| `update_segment_text` (editor/polish accept) | after the `UPDATE` | rewrites that one doc (or removes it when the text became blank) |
| `delete_transcript` | new savepoint around the delete | removes every doc of the transcript (segments + title) |
| `split_segment` / `merge_segments` / `restore_segment_rows` | `replace_rows` | replaces the transcript's segment docs with the new row set |
| `save_text_settings` | inside its savepoint | re-indexes the small glossary/dictionary corpus wholesale |
| `rebuild_search_index` | — | drop + recreate + bounded backfill |

Nothing is left to a trigger. The `write_segments` hook reads the row back
after the upsert because `source_text` is `COALESCE`-preserved — the value the
caller passed is not necessarily what the original column now holds.

**Empty rule.** A segment whose current text is only ASCII whitespace has no
document, so it cannot be a result. The Rust predicate and the SQL count use the
same character set (`trim(text, char(9)||char(10)||char(13)||' ')`) deliberately:
a mismatch would make every open see a phantom inconsistency and rebuild.

## 2. Tokenizer: `unicode61`, no stemmer — why

- **`porter` is disqualified.** It is English-only and would conflate
  Indonesian forms (`rapat`/`rapatnya`, `jalan`/`jalanan`) by stripping endings
  that carry meaning — the workorder forbids stems in any language, and the
  parent plan names English stemming as the thing that must not damage
  Indonesian retrieval.
- **`simple` is ASCII-only.** It would leave `Éclair` and `éclair` as different
  tokens; the M15.1 audit recorded exactly that limitation in the old LIKE scan.
- **`unicode61` is Unicode-aware and has no stemming.** With
  `remove_diacritics 1` it also folds accents, so `cafe` finds `café` and
  `ÉCLAIR` finds `éclair` — the "Latin-1 extended case folding" the workorder
  asks for, plus case folding beyond Latin-1 for free. Both EN and ID are Latin
  script; the tokenizer is identical for them, which is the point: nothing
  routes by language.
- **Whole tokens, not substrings.** This is a deliberate contract change from
  the old `LIKE '%q%'` scan: `anggara` no longer finds `anggaran`, and
  `saya` does not match `sayang` (a false positive the scan produced). A
  trailing `*` is an explicit prefix query (`pegawai*` finds `pegawairungu`),
  which is the documented workaround for Indonesian agglutination.

## 3. Query shaping

User input never reaches FTS5 as an expression. `search::match_query` splits on
whitespace, drops words with no alphanumeric character, strips `"` and `*`, caps
a word at 64 chars, and quotes the rest, joined by the implicit `AND`:

```
Selamat OR jadwal  ->  "Selamat" "OR" "jadwal"     (requires all three words)
say"a -jakarta     ->  "saya" "jakarta"            (punctuation is text)
pegawai*           ->  "pegawai"*                  (the one explicit wildcard)
- -- …             ->  no query at all             (never a syntax error)
```

Consequences, all pinned by tests: a query can never be an FTS5 syntax error; a
user cannot accidentally ask for an OR; query words in different rows do not
match (AND, not OR); a term must appear as a whole token; and `%`/`_` are
separators, not wildcards (the old `ESCAPE` rule is gone with the LIKE scan).

## 4. Snippets — rules and ellipsis boundaries

`search::snippet(text, terms, context_chars)` is a pure function over the
matched column. It **does not** use FTS5's `snippet()`: that function counts
tokens, while the workorder asks for a **character** window — and FTS5's
`offsets()` is unusable in this build (`unable to use function offsets in the
requested context`, reproduced with literal and bound queries on SQLite 3.46.0),
so the window is built in Rust.

Rules, in order:

1. The text is whitespace-collapsed (a merged segment's `\n` becomes one space)
   and trimmed, so a snippet is always one display line.
2. The earliest occurrence of any query term is located **case- and
   diacritic-insensitively, on word boundaries**, mirroring the tokenizer
   (`fold_char` covers Latin-1 + Latin Extended-A accents; other scripts fall
   back to simple lowercase).
3. The window is `context_chars` chars before and after the match
   (default **80**; clamped to **10..=400**). The match itself is never cut.
4. Each edge snaps outward to a word boundary, at most 24 chars, so a snippet
   does not start mid-word; the start snaps forward past a partial word, the
   end snaps back to the last space.
5. A cut edge is marked with exactly one `…` (U+2026 HORIZONTAL ELLIPSIS, no
   surrounding spaces). An ellipsis therefore means "more text this way",
   never "the segment is empty"; no ellipsis means the window reached the edge.
6. The matched span is returned as a byte range (`SearchHit::highlight`) so the
   UI can paint the word in the stronger ink; the library view does.
7. If the term cannot be located (an unusual script outside the fold table, a
   tokenizer split we cannot reproduce), the snippet is the head of the text
   with `highlight = None` — the row still shows, it just cannot mark the word.

If the current text does not contain the query but the preserved `source_text`
does, the snippet comes from that column and the hit sets
`from_source_text = true` (the UI appends "(original text)").

## 5. Seek — through the existing playback state

A segment hit carries `start_ms`; the UI's row click goes through
`AppState::open_search_hit`:

- the transcript is already open → `AppState::seek_playback(ms)` (the M8
  transport) and no reload, so edit buffers are not thrown away;
- otherwise the hit's id is stored as `search.pending_seek`, the ordinary
  `Effect::LoadTranscript` runs, and `on_transcript_loaded` applies the seek
  **after** `open_editor` rebuilt the transport for that transcript. The pending
  seek is keyed by id and is dropped if a different transcript loads, so it can
  never move the wrong playhead.
- an idle transport remembers the position (the next Play starts there); a live
  session gets `Command::Seek`.
- **missing media**: `resolve_playback_source` says why (file gone / keep-audio
  off / never had audio), `Playback::seek` is a no-op, and
  `search.seek_note` carries the documented flag — "Opened at 1:23 — the
  playhead cannot move: the source audio is gone…". The result row is never
  removed.
- title hits open the transcript without a seek; glossary/dictionary hits are
  not tied to a recording and open the Text rules section of Settings.

No new screen, no second playhead: the same `AppState::playback` the editor's
deck draws from.

## 6. Migration and self-repair

`Store::open` on a v3 file runs `upgrade_to_v4`, three phases:

1. **Objects**, one transaction (`CREATE ... IF NOT EXISTS`).
2. **Backfill**, `SEARCH_BACKFILL_CHUNK = 500` documents per transaction. Each
   chunk commits its `meta` cursor (`search_backfill_segments` /
   `search_backfill_titles`) **with** its rows, so an interrupted backfill
   resumes at the last committed document instead of redoing the corpus — and a
   crash loses at most one chunk. The cursor is a zero-padded doc key, so the
   string order matches the integer `idx` order.
3. **Stamp** `schema_version = 4`, only after every document is in.

A crash mid-backfill therefore leaves a coherent v3 library (which an older
build can still open) plus a partial index this build resumes.
`tests/search.rs::a_v3_library_backfills_in_bounded_chunks_and_resumes_where_it_stopped`
proves both halves: a trigger aborts the second chunk, the test asserts exactly
**500** documents committed, version still **3**, cursor at `…:0000000499`;
after dropping the trigger the next open finishes 1,200 segments and reports
v4.

**Self-repair.** Every open (after migration) runs `search_index_consistent`:
per-kind counts against the store rows, the map count against `count(*) FROM
search_fts`, and FTS5's own `INSERT INTO search_fts(search_fts)
VALUES('integrity-check')`. A mismatch or an unreadable index triggers one
`tracing::warn!` and a deterministic rebuild (`DROP` + `CREATE` + the same
bounded backfill). Covered by a test that deletes one FTS row (partial index)
and another that drops the table entirely.

Cost of the check at 10k segments: **27.1 ms release / 71.9 ms debug** on this
VPS, paid once per open. That is the price of the guarantee; a corpus an order
of magnitude larger would want the integrity check behind a clean-shutdown
marker (see §9).

## 7. Measured responsiveness (this VPS)

Machine: AMD EPYC 7K62, **2 vCPU**, 3.8 GB RAM, Linux 7.0.0-14. Corpus:
**10,000 segments across 20 transcripts**, realistic EN+ID sentences (~60 chars
each), 20 title documents. Command:
`cargo test --release -p suramak-core --test search ten_thousand -- --nocapture`.

| measurement | release | debug |
|---|---|---|
| index + database size | 6.5 MB | — |
| insert all 10k segments (incl. index) | 472 ms | 1,316 ms |
| reopen + full consistency check | 27.1 ms | 71.9 ms |
| `anggaran` (1,250 hits), limit 50, median | **4.47 ms** | 12.50 ms |
| …worst of 5 | 4.51 ms | 14.28 ms |
| `anggaran rapat` (two terms), median | 6.19 ms | 15.26 ms |
| `rapat` (matches all 10k + titles), median | 15.98 ms | 43.81 ms |

Interpretation: a bounded 50-hit query over 10k segments costs **~4.5 ms** in
the shipping profile — one debounced query per typing pause (220 ms) is
comfortably inside a frame budget, so the search runs on the UI thread against
the open store. The all-match case (10k candidates ranked by `bm25`, only 50
materialized) is ~16 ms; that is the realistic worst shape a common word can
produce at this corpus size.

## 8. Tests added (42)

- `suramak-core::search` (15 unit): query quoting/AND, punctuation-only,
  prefix, interior punctuation, snippet window/ellipsis/highlight/fold/word
  boundary/whitespace collapse/unlocatable fallback/empty text.
- `crates/suramak-core/tests/search.rs` (12): insert/edit/delete sync;
  split/merge/restore index-vs-rows; reopen without rebuild; mixed-language and
  literal Indonesian queries; empty segments; glossary/dictionary sync; snippet
  window + original-column hit; a transcript without audio still has timestamped
  hits; v3 bounded backfill + resume; corrupt/partial index self-repair;
  backup/restore; the 10k responsiveness gate.
- `suramak-ui` (14 + 1 frame-level): debounce, stale-answer drop (and its
  in-flight marker released), error surface, click-to-seek on the open editor,
  load-then-seek across transcripts, stale seek never lands on the wrong
  transcript, missing-media flag, glossary hit opens Settings, a hit whose
  sidebar row vanished refreshes instead of opening nothing, clear, timestamp
  formatting, invalidation after an edit, and a headless `views::draw` test that
  the panel actually paints the timestamp, title and snippet.
- Two pre-existing tests were **changed on purpose**, with the M15.1/M15-B
  comments saying so: `persistence.rs`'s LIKE-scan pin became the FTS5 token
  contract, and `store.rs`'s wildcard-literal test became "no implicit
  wildcards, one explicit prefix".

## 9. Boundaries and honest limits

- **Whole-token matching.** A substring that is not a whole token is not found
  (`anggara` ≠ `anggaran`). `prefix*` is the escape hatch, and it is the only
  wildcard. This is stricter than the old scan and is the intended trade.
- **No stemming, by design.** `rapat` does not find `rapatnya`. Indonesian
  affixation is handled by the user's word choice and the explicit prefix, not
  by a stemmer that would damage both languages.
- **The corpus is small and the map is a second write.** Every document costs a
  `search_docs` row plus the FTS row; the whole database for the 10k test corpus
  is 6.5 MB (text + index + map), so disk is not a concern at this scale, but
  the index is not free: it roughly duplicates the transcript text plus FTS
  bookkeeping.
- **Search runs on the UI thread**, one query at a time, synchronously in
  `Runtime::apply`. Measured 4.5 ms at 10k (release), 16 ms worst case; at
  ~100k segments a common-word query would plausibly reach ~150 ms and stutter.
  The honest next step is a dedicated read-only connection on a worker thread,
  not a bigger debounce. Nothing in the API prevents it.
- **Splits duplicate provenance.** M15.3 gives both halves the parent's
  `source_text`; the index therefore legitimately returns two hits (two
  timestamps) for a word that came from a split parent — one current, one
  "(original text)". Merges concatenate provenance. That is the stored truth,
  not an indexing artefact.
- **The snippet locator is an approximation of the tokenizer.** It folds
  Latin-1/Latin Extended-A accents and lowercases; a script outside that table
  (CJK, Arabic) can match in FTS5 but not be located for the window, in which
  case the snippet is the head of the text and no word is highlighted.
- **Accent folding is lossy in the index**: `cafe` and `café` are the same
  token. That is deliberate (`remove_diacritics 1`), but it means a search
  cannot distinguish them.
- **Ranking is plain `bm25`.** A short title document can outrank a longer
  segment; there is no recency boost, no per-kind weighting, and `total` counts
  documents (a title hit and three segment hits from one transcript are four
  results).
- **The integrity check is paid on every open.** 27.1 ms at 10k; for a much
  larger library the design should gate it on a clean-shutdown marker, because
  a full check per launch scales linearly.
- **Not encrypted, not compressed.** The index is ordinary SQLite pages in the
  same file as the library (which is itself unencrypted today). Backup carries
  it; restore either carries a working index or rebuilds on the next open.
- **`offsets()` is unavailable in this SQLite build**; the implementation does
  not depend on it. If a future SQLite restores it, snippets could drop the
  Rust locator, but the character window would still need custom code.
- **No CLI surface.** `suramak-cli library` still lists only; the workorder
  asked for the library view, and the store API is ready for a CLI `search`
  verb when one is wanted.
