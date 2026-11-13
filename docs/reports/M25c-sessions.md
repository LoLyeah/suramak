# M25C — a session is one meeting, not five files

**Status:** implemented. No commits, no pushes, no CI changes.
**Box:** 2 vCPU AMD EPYC 7K62, 3.7 GiB RAM, Linux 7.0.0-14, rustc 1.99.0.
**Date:** 2026-11-13.
**Schema:** v13 (from v12, additive on `transcripts`; the `notes` key is rebuilt
once so a part note and a session note can coexist).

## 1. What shipped

### 1.1 Session identity (schema v13)

`transcripts` gains three nullable columns:

```sql
session_id  TEXT,      -- one uuid per meeting; NULL on every pre-v13 row
part_index  INTEGER,   -- 1-based
part_count  INTEGER
```

The numbers are **columns, not title parsing**. `PartInfo::from_name` is still
how an import's file name is read, but the stored identity is what the library,
the notes and the export use. Existing rows keep NULL in all three columns and
render exactly as they did.

`notes` gains `session_id TEXT`. Its primary key becomes
`(transcript_id, preset, session_id)` — SQLite cannot alter a key in place, so
v13 copies every note row into the new table, drops the old one and recreates
two partial unique indexes:

```sql
CREATE UNIQUE INDEX notes_transcript_preset
    ON notes(transcript_id, preset) WHERE session_id IS NULL;
CREATE UNIQUE INDEX notes_session_preset
    ON notes(session_id, preset) WHERE session_id IS NOT NULL;
```

A whole-session note is anchored on the session's first *present* part for the
foreign key, and is keyed by `(session, preset)`; a part keeps its own
`(transcript, preset)` row. The v12→v13 test opens a populated v12 file
(one transcript, one segment, one stored note), asserts the version, the row,
the note and `transcript_session(id) == None`, then writes an identity into the
old row and reads it back.

### 1.2 Assigning the identity

| path | identity |
|---|---|
| recording | `AppState::start_recording` generates one `Uuid` at Start (`record_session`); `on_recording_stopped_parts` hands each `Part N of M` wav the same id, index and count |
| cap-split import | `begin_imports` calls `assign_split_imports` after `split_over_cap_imports`; the `Part N of M` names are grouped by base name + count and share a fresh id |
| saving | `Effect::SaveTranscript` carries the identity and the runtime writes it in the same save; the journaled path (whose journal is untouched) gets one `Effect::AssignSession` after the committed row |
| one-part take | no identity — a single transcript is not a session |

The queue journal, its resume and the DB job rows are unchanged.

### 1.3 One library row, one session view

On the phone's compact Library, a session with ≥ 2 **present** parts collapses
into one row:

* title without the `Part N` suffix (`Meeting`);
* meta exactly `3 parts · 2 h 10 m`;
* a small part-count chip (`3 parts`);
* tapping it (or its play affordance, which has no single stream) opens a
  session view sheet: the parts listed in `part_index` order, each with
  `Part N — hh:mm:ss` and a one-line opening (the first segment, 96 chars,
  `…`); tapping a part opens that part's editor. A missing part shows
  `2 of 3 parts transcribed` above the list.

A lone present part, and every NULL-session row, keeps today's row verbatim
(date · segment count, no chip). Part rows are never edited across parts.

### 1.4 Session notes on a session-absolute timeline

The Notes sheet gains a scope when the open transcript belongs to a session:

| Scope | What it feeds |
|---|---|
| `This part` | that part's segments and clock (M24B, unchanged) |
| `Whole session` (default) | every part in order, on one absolute clock |

`session::session_timeline` sorts by `part_index`, accumulates each part's
audio duration (`offset += duration`), rewrites every segment's `start_ms` /
`end_ms` to the absolute clock and **reassigns `idx` 0..n** across the session.
Part 2 starts exactly at part 1's duration — to the sample when the caller has
the sample-exact duration, which the recording/import plan does.

The **same** absolute segment list is sent to the model and passed to
`check_notes`, so a citation in part 3 is matched against the timeline the model
saw. The test below proves it by citing `[50:00]` (part 3's first row) and
keeping it, while the same citation against part 1 alone is `past_end`.

The stored row has `session_id` set; regenerating replaces the one
`(session, preset)` row. A per-part note can coexist on the anchor transcript.

### 1.5 The incomplete-session refusal

`session::session_incomplete_refusal(2, 3)` returns, before any HTTP call:

```text
session_incomplete: 2 of 3 parts transcribed — whole-session notes need every part;
transcribe the missing part or use This part
```

The sheet shows `2 of 3 parts transcribed` under the toggle and `start()`
refuses with the named reason (it does not go to `Running`). `This part` still
runs; the missing part is a hole, not a shorter meeting.

### 1.6 One export file

`export::to_session_markdown` writes one Markdown document: the meeting's
header once, then a `## Part N — mm:ss` divider per part at its **absolute
start**, with absolute `[hh:mm:ss.mmm → …]` lines in the body. The phone's
session view has `Export .md` (same staging/share path as a note export).
`export::to_markdown` — the per-part export from a part's editor — is not
touched; a test pins its exact bytes.

## 2. A real three-part synthetic session

Test: `crates/suramak-core/tests/m25c_session_notes.rs`, run with
`--nocapture`. Three parts with real WAV-length durations: 45:00 + 5:00 + 10:00
= 60:00, inserted into a real in-memory `Store` and given one `session_id` by
`set_transcript_session`. The mock endpoint is a hand-rolled `TcpListener` on
`127.0.0.1`.

```text
--- parts ---
Part 1: offset 0 ms, duration 2700000 ms
Part 2: offset 2700000 ms, duration 300000 ms
Part 3: offset 3000000 ms, duration 600000 ms
total: 3600000 ms
--- absolute segment starts ---
[0] idx 0
[1200000] idx 1
[2700000] idx 2
[2820000] idx 3
[3000000] idx 4
[3300000] idx 5
```

`[2700000]` is part 2's first row and `[3000000]` is part 3's, exactly.

**The mock reply** (it cites part 3 at `[50:00]` and `[55:00]`; the last
`[55:00]` is a deliberate duplicate and `[1:00:01]` names no row):

```markdown
## Decisions

- Ship on Friday [00:00].
- The budget was approved [50:00].
- Budi owns the migration [55:00].
- The launch date is certain [1:00:01].

## Follow-ups

- Budi sends the plan [55:00].
```

**The stored session note** (`body_md`, read back with
`Store::notes_for_session`; one row, `session_id` set):

```markdown
# Minutes

## Decisions

- Ship on Friday [00:00].
- The budget was approved [50:00].
- Budi owns the migration [55:00].
- The launch date is certain.

## Follow-ups

- Budi sends the plan.
```

```text
checked=5 removed=2 breakdown=CitationBreakdown { removed: 1, past_end: 0, malformed: 0, duplicate: 1 } parts=1
```

The `[50:00]` and the first `[55:00]` survive because they name real part-3
rows; the unmatched time and the duplicate are removed and counted, never
shipped.

**The one-file export** (`to_session_markdown`, verbatim):

```markdown
# Project Atlas

- Date: 2026-11-13 03:35:55 UTC
- Source: Meeting — Part 1 of 3.wav
- Model: tiny
- Language: en
- Parts: 3

## Part 1 — 00:00

[00:00:00.000 → 00:00:08.000] Good morning, everyone.

[00:20:00.000 → 00:45:00.000] We will ship on Friday.

## Part 2 — 45:00

[00:45:00.000 → 00:45:06.000] Part two begins after the pause.

[00:47:00.000 → 00:50:00.000] The budget is approved.

## Part 3 — 50:00

[00:50:00.000 → 00:50:07.000] Part three starts after the second pause.

[00:55:00.000 → 01:00:00.000] Budi owns the migration.

```

**The incomplete refusal** (same session with part 2 missing):

```text
session_incomplete: 2 of 3 parts transcribed — whole-session notes need every part;
transcribe the missing part or use This part
```

## 3. Tests

| Suite | What it pins |
|---|---|
| `suramak-core` `store::tests` (2 new) | v12→v13 on a populated library (version, row, note, NULL identity, then a written identity); session parts ordered 1-2-3 from out-of-order inserts; one `(session, preset)` row that replaces on regenerate; per-part and session notes coexist; `notes_for` does not leak session rows |
| `suramak-core` `session::tests` (3 new) | exact absolute mapping (0 / 2,700,000 / 3,000,000, total 3,060,000, contiguous idx); `session_incomplete` reason and `2 of 3`; `3 parts · 2 h 10 m` meta |
| `suramak-core` `notes::tests` (1 new) | a citation in part 3 (`[50:00]`) is accepted on the session timeline and removed against part 1 alone |
| `suramak-core/tests/export.rs` (1 new) | `## Part 1 — 00:00` / `## Part 2 — 45:00` + absolute body, and `to_markdown` byte-identical to today's exact string |
| `suramak-core/tests/m25c_session_notes.rs` (1 new, real session) | the three-part timeline, mock-endpoint whole-session notes with part-3 citations, one stored row, one-file export |
| `suramak-ui` compact `model::tests` (4 new) | collapsed row meta/chip/title; NULL-session row unchanged; session view ordering; `2 of 3 parts transcribed` and the named refusal; session export dividers |
| `suramak-ui` `notes_sheet::tests` (1 new) | `["This part", "Whole session"]`, whole-session default, incomplete refusal, `This part` runs |
| `suramak-ui` bin `desktop::app::tests` (1 new) | a recording's `Part N of M` wavs share one id from Start; a single file gets none; a cap split groups by name and count |

## 4. Verification

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | clean |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| Android check | `cargo ndk -t arm64-v8a --platform 29 check -p suramak-android -p suramak-android-spike --locked` | clean |
| Android lints | `cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-android -p suramak-android-spike --all-targets -- -D warnings` | exit 0 |
| Full workspace | `cargo test --workspace` | see below |

```text
FULL_WORKSPACE_PLACEHOLDER
```

## 5. Deliberate limits

- **The desktop sidebar still lists the parts individually.** The collapsed
  row and the session view are the compact (phone) library surfaces; the
  desktop's `LibraryState` is an index into a flat header list and collapsing
  it would need a virtual row type, so this slice leaves it. The desktop Notes
  sheet *does* get the scope toggle and whole-session runs, and the desktop
  editor's per-part export is unchanged.
- **No cross-part editing.** Each part's editor edits its own rows; the session
  view opens a part, never a merged editor.
- **No chunked decoding.** This slice adds no decode path; the parts are the
  same ordinary jobs M25B produced.
- **The queue journal is untouched.** A split import's identity is assigned in
  the app when the batch is queued. If the process dies before those jobs save,
  the journal resumes the files without the identity (they still transcribe;
  they just don't collapse until a later slice puts parts in the journal). The
  journaled-done path assigns the identity right after the committed row.
- **`session_parts` returns only the parts that exist.** That is deliberate:
  the caller compares `len()` with `part_count` to say `2 of 3 parts
  transcribed` rather than pretending the hole is a shorter meeting.
- **The UI's export uses segment ends as part durations** (it has no decoder);
  the notes/host path uses the audio header (`probe_duration_ms`) and falls
  back to the last segment's end. For a part with trailing silence the export's
  divider can therefore start a little earlier than the audio-exact offset; the
  stored timeline contract and the core tests use the exact duration.
- **A session note is anchored on the first present part** for the FK. Deleting
  that part deletes the note (the FK cascade); deleting a later part leaves it.
