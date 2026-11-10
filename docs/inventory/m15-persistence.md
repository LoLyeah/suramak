# M15.1 — Persistence and provenance inventory

Status: written against the live tree on 2026-11-10 (`suramak-core` 0.3.1, schema
v2 after this slice). Every path below names code that exists; nothing here is a
proposed module. Line numbers are deliberately omitted (they drift) — function
names are the anchors.

## 1. Storage shape

One SQLite file (`paths::db_path()`), WAL mode, `synchronous=NORMAL`,
`foreign_keys=ON` (per connection, set in `Store::init`).

| table | columns | notes |
|---|---|---|
| `meta` | `key TEXT PRIMARY KEY`, `value TEXT NOT NULL` | `schema_version` lives here |
| `transcripts` | `id TEXT PK`, `title`, `created_at`, `updated_at`, `source_json`, `model`, `language`, `audio_path` | `id` is the UUID string; timestamps are RFC 3339 |
| `segments` | `transcript_id`, `idx`, `start_ms`, `end_ms`, `text`, `source_text` | PK `(transcript_id, idx)`; FK → `transcripts(id) ON DELETE CASCADE` |

Indices: only the two primary keys (SQLite's implicit indices). No FTS table, no
trigram index, no separate segments table for revisions, no attachment table.

`schema_version`: **1** was shipped in app 0.3.0/0.3.1. **2** is this slice's
additive migration (`segments.source_text`). The app version and the schema
version are independent numbers.

## 2. Transcript source text — where it is written

"Source text" here means the text produced by inference (whisper/Parakeet/Apple
SpeechTranscriber) and handed to the store. It enters through exactly one API:

- `Store::insert_transcript(&Transcript)` — serializes `source` into
  `source_json`, writes the header row and every segment row. The insert and the
  segments are **one transaction** (`unchecked_transaction` + `write_segments` +
  `commit`), so a crash cannot leave a header with half a segment set.
- `Store::upsert_segments(id, &[Segment])` — the "replace the full segment set"
  path used by the CLI polish command and by a future re-polish. It runs inside
  a **savepoint**: inserts/updates, the stale-tail delete (`idx >= len`), and the
  `updated_at` touch are one unit; a failure rolls all of it back (SUR2-NEW-05).
- `Store::update_segment_text(id, idx, text)` — the editor/autosave and in-app
  polish-accept path. It updates `segments.text` and touches
  `transcripts.updated_at`, now in one savepoint (M15.1; previously two
  autocommit statements).
- `Store::delete_transcript(id)` — single statement; segments go with the
  cascade.

The UI's batch autosave (`runtime::save_edits`) calls `update_segment_text` once
per changed segment: each row's text+stamp is atomic, the *batch* is not. A
failure mid-batch keeps the earlier rows and returns the error; the editor keeps
the buffers dirty and retries. That is a partial-save, not a source-text loss —
no row loses text it had.

## 3. Mutation coverage matrix

| mutation | rows written | transaction | source-text effect |
|---|---|---|---|
| `insert_transcript` | 1 header + N segments | one transaction | writes `text` and `source_text` (same value) |
| `upsert_segments` | N segments + header stamp | one savepoint | rewrites `text`; keeps the row's first `source_text` (`COALESCE`); new rows get their own first text |
| `update_segment_text` | 1 segment + header stamp | one savepoint (M15.1) | rewrites `text` only; `source_text` untouched |
| `delete_transcript` | 1 header + cascade | single statement | deletes the rows and their provenance together |
| `backup_to` | none (read snapshot) | `VACUUM INTO` read transaction | byte copy of both columns |
| `restore_from` | whole file | copy + fsync + rename | byte copy; no row rewrite |

## 4. Audit: could any mutation lose source text?

Grep of the write paths (`insert_transcript`, `write_segments`,
`upsert_segments`, `update_segment_text`, `delete_transcript`) and their callers
(runtime `save_edits`, in-app polish accept/undo, CLI polish, CLI transcribe)
found **one real loss path**, and it was the product's core editing behaviour:

- **`segments.text` was the only copy of the inference output.**
  `update_segment_text` (editor typing, autosave, accepted polish) overwrote it
  in place. The UI's one-step polish undo only restores the *pre-polish buffer*
  (which is itself already edited text), so the original inference wording was
  unrecoverable after the first keystroke. This contradicts the M15–M18 rule
  "Preserve original inference text. Normalization, correction, translation, and
  summaries are explicit derived outputs, not destructive replacements."

**Fix (applied, additive only):** schema v2 adds `segments.source_text TEXT`.
`insert_transcript` fills it on first write; `write_segments`' conflict branch
uses `COALESCE(segments.source_text, excluded.source_text)` so no later polish or
editor save can overwrite it; the v1→v2 migration backfills it from `text`. It is
readable through `Store::segment_source_text(id, idx)`. No table was rebuilt, no
column removed, no row deleted. A database written by 0.3.0/0.3.1 opens in place
and upgrades in one transaction.

Paths considered and cleared:

- The stale-tail delete in `upsert_segments` removes rows a shorter rewrite no
  longer has. That is the requested shape of the new segment set (a merge), not
  a loss of surviving rows' text; the deleted rows' provenance goes with them.
- Re-transcription writes a **new** transcript id (`Transcript::new` → UUID v4);
  it does not reuse an id, so an upsert is always a derived rewrite, never a
  second inference that should reset provenance. Documented here so a future
  "re-transcribe into the same row" feature must explicitly reset
  `source_text`.
- `delete_transcript` is an explicit user action; the backup API below is the
  recovery path.

No other source-loss path exists; no further migration is needed for M15.

## 5. Derived text and provenance fields today

- **In-app polish** keeps its suggestion in memory (`PolishState::Review`); on
  accept the texts go through `Effect::SaveEdits` →
  `update_segment_text`. After M15.1 the pre-polish inference text remains in
  `source_text`. There is **no** `derived` table, no polish model id stored per
  segment, and no "polished from" pointer.
- **CLI polish** calls `upsert_segments`; same rule.
- **Translation does not exist** in the live tree (M15.8 is unbuilt): no
  translation column, no request path, no stale marker. The schema reserves
  nothing for it — M15.8 will need its own additive migration and must store
  provenance (`endpoint`, `model`, `source revision`) rather than overwrite
  `text`.
- Transcript-level provenance that *does* exist: `source_json` (file name, or
  recording start + duration), `model`, `language`, `created_at`, `updated_at`,
  `audio_path`.
- Segment-level provenance that did **not** exist before this slice: none. Now:
  `source_text` only. There is no per-segment model, confidence, speaker, or
  revision counter.

## 6. Backup and restore (new library API, no UI)

- `Store::backup_to(dest)` — `VACUUM INTO` through the live connection: one read
  transaction (WAL included), a compact single file, no `-wal`/`-shm` sidecars.
  Refuses an existing destination instead of overwriting it silently.
- `Store::restore_from(backup, db)` — validates the backup read-only
  (`PRAGMA integrity_check` = `ok`), refuses a schema newer than this build,
  copies to `<db>.restore-tmp`, fsyncs, removes the target's stale
  `-wal`/`-shm`, then renames over the target (atomic on Unix; on Windows
  `std::fs::rename` cannot replace, so it removes the target and renames — a
  crash between the two leaves the validated temp file, never a half-written
  database). The caller must hold no open `Store` for `db`; that precondition is
  documented on the function because SQLite cannot enforce it.
- No UI/CLI calls either function yet. The M15.1 gate exercises them from
  `crates/suramak-core/tests/persistence.rs`.

## 7. Search — what pre-exists (M15.1 finding)

`Store::search(query)` is the only search code:

- SQL: `SELECT DISTINCT t.* FROM transcripts t LEFT JOIN segments s ON … WHERE
  t.title LIKE ?1 ESCAPE '\' OR s.text LIKE ?1 ESCAPE '\' ORDER BY
  t.created_at DESC` with `%…%` around the escaped query.
- **LIKE substring**, not FTS5: no `MATCH`, no `fts5` virtual table, no index.
  Cost is a full scan of titles + segment text.
- Wildcards are literal: `%`, `_`, `\` are escaped (SUR-LOW-02).
- Case folding is **ASCII-only** (SQLite's default `LIKE`): `SELAMAT` finds
  `Selamat`; `éclair` does not find `Éclair`. Indonesian is Latin script, so the
  common case works; accented loanwords do not.
- Results are **transcript headers only** (segments empty), newest first. No
  snippet, no match offset, no timestamp, no seek target.
- It searches the **current** `segments.text`, not `source_text`: an edit that
  removes a word also removes it from results while the original remains stored.
- It has **no caller in the product**: neither the UI nor the CLI invokes
  `Store::search` today (only the store tests do). The library sidebar lists
  `list_transcripts()` and has no search field; `suramak-cli library` only has
  `list`. M15.7 therefore starts from a tested store API with zero UI wiring,
  and should decide deliberately whether the LIKE scan or FTS5 is the base.

## 8. Alignment metadata

Segment-level only: `start_ms`/`end_ms` (`i64`, milliseconds) on each row. No
word timestamps, token probabilities, confidence, speaker id, or per-word table
— `Segment` is `{idx, start_ms, end_ms, text}` and nothing else. Exports derive
their cue times from `start_ms`/`end_ms`; playback seeks by segment index. M15.6
(word alignment) has no stored shape to build on yet, so it will add its own
tables/columns rather than reinterpret an existing field.

## 9. CPL/CPS counting rules (M15.4)

Implemented in `suramak_core::diagnostics::diagnose`; the module doc is the
normative version.

- Split on hard line breaks (`\n`, `\r\n`, `\r`); trim each line; interior
  spaces count.
- Count **grapheme clusters**, not bytes/chars: combining marks, variation
  selectors, emoji modifiers, tag characters and keycap marks attach to the
  previous cluster; a ZWJ joins both sides; a pair of regional indicators is one
  flag. Bounded approximation of UAX #29 for Latin/Indonesian/Arabic-script
  Indonesian and common emoji (no dependency).
- **CPL** = longest line's grapheme count (no implicit wrapping).
- **CPS** = total graphemes across lines ÷ duration seconds. `end_ms <=
  start_ms` → CPS 0.0 and `zero_duration = true` (a timing defect, not a
  reading-speed finding).
- Thresholds are **strictly greater**: defaults 37/42 CPL and 17/20 CPS. 37 is
  OK, 38 warning, 42 warning, 43 error; 17.0 CPS OK, 17.1 warning, 20.0 warning,
  20.1 error.
- Badge text lists only the crossed dimension(s): `CPL 44`, `CPS 20.5`, or
  `CPL 44 · CPS 21.3`. Advisory only — nothing truncates or blocks.

The editor reads `AppState::subtitle_limits` (default `SubtitleLimits::default()`)
so a future settings surface can replace the values without touching the
counting code; no settings UI ships in this slice.

## 10. Shared-decode contract (M15.2, for Android M13-C)

`suramak_core::decode::decode_file` — or `decode_file_cancellable(path,
&|| token.is_cancelled())` when a job token exists — is the single import
decoder for every platform: it probes with symphonia (wav/mp3/flac/ogg-vorbis/
m4a/mp4/mov/m4v/mkv/webm), registers the bundled libopus adapter for Opus, reads
the first real audio track, returns interleaved `f32` at the container's rate
and channel count, rejects structurally truncated Ogg (EOS tail page) and
Matroska (declared Segment size) files with `SuramakError::Truncated`, converts
foreign-decoder panics into `SuramakError::DecoderPanic`, and honours a
cooperative cancel between demuxed packets with `SuramakError::Cancelled`. The
Android M13-C import path must copy the SAF/share bytes to a local
`files/media/<uuid>.<ext>` file and call this function on it — never
`MediaCodec`, a JNI `AudioTrack` decode, or a Kotlin-side resampler — so the same
containers, the same 48 kHz Opus path, the same named errors and the same
cancellation behaviour hold on device; the only device-specific work is the byte
copy and the filename/extension it preserves.
