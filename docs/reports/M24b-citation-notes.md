# M24B — citation-verified notes (Minutes · Action items · Bilingual brief)

**Status:** done. Notes are generated only by the endpoint the user configured,
and **every timestamp in them is checked against a real segment** before it is
stored. A citation that names audio which does not exist is removed and
counted; it is never shipped as if it were true.

---

## 1. What shipped

### Three presets, one mechanism

| Key | Sheet label | Output shape |
| --- | --- | --- |
| `minutes` | Minutes | `## Decisions`, `## Discussion`, `## Follow-ups` |
| `action_items` | Action items | `- **Owner** — action [mm:ss]`, `## Open questions` only when one was left |
| `bilingual_brief` | Bilingual brief (EN↔ID) | `## Ringkasan` / `## Summary` + one paragraph in the other language + `## Quotes` |

Each preset is a system prompt with the same contract: Markdown only, citations
as `[mm:ss]` or `[hh:mm:ss]` copied from a segment's `start`, no invented
timestamps, no invented speaker names (`Unattributed` when none was spoken),
no invented owners or due dates, keep the source language, no top-level `#`
title. The document's H1 is added by the app, so a chunked merge never repeats
it.

The prompts live in `crates/suramak-core/src/notes.rs` (`NotePreset::system_prompt`).
The shared contract is a single constant; a test pins that all three carry it.

### Generation — the polish client, no second HTTP path

`generate_notes` builds a JSON payload of the transcript's segments
(`id`, `start`, `start_ms`, `speaker`, `text`) and sends it through
`polish::OpenAiCompat::complete_markdown`, which is the **same** POST to
`{base_url}/chat/completions`, with the same model, temperature 0, error shape
and bearer key as the polish call. Nothing about the endpoint is assumed beyond
what polish already assumes.

- **One request per preset.** `parts = 1` and the sheet says `single request`.
- **Chunk-then-merge only when needed.** The segment text budget is
  `NOTES_CHUNK_CHARS = 24_000` characters. A longer transcript is split at row
  boundaries, each part is requested in order, the bodies are merged with a
  blank line, and `parts = N`; the sheet says `N chunks merged`. A test drives
  two mock requests and checks the part fields, the merged body and that every
  segment reaches exactly one part.
- **Speaker labels** are sent when the open transcript has a diarization view;
  otherwise `speaker` is `null`.

The request still carries only `{model, temperature, messages}` — the polish
request shape is unchanged; only the system prompt and the user payload differ.

### The validator — `check_notes`

Every bracketed token is parsed:

| Token | Result |
| --- | --- |
| `[12:31]` / `[00:12:31]` matching a segment start within ±1 500 ms | kept, rewritten to the segment's canonical time (`mm:ss` under an hour, `hh:mm:ss` from an hour) |
| time-like, no segment within ±1 500 ms | removed, counted as `removed` |
| time past the recording's end | removed, counted as `past_end` |
| time-like but unparseable (`[12:60]`, `[1:2:3:4]`) | removed, counted as `malformed` |
| a second citation of an already-cited segment | removed, counted as `duplicate` |
| `[banana]` (not a time token) | untouched, not counted |

`citations_checked` counts every time-like token seen; `citations_removed` is
the sum of the four breakdown fields. Valid tokens are rewritten, not merely
accepted: `[00:11]` for a 12 s segment becomes `[00:12]`; `[00:01:05]` becomes
`[01:05]`. A removed token takes its brackets with it (no stray `[` is left).

### Storage — schema v12

Additive only; no table is rebuilt:

```sql
CREATE TABLE IF NOT EXISTS notes (
    transcript_id     TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
    preset            TEXT NOT NULL,
    body_md           TEXT NOT NULL,
    citations_checked INTEGER NOT NULL DEFAULT 0,
    citations_removed INTEGER NOT NULL DEFAULT 0,
    model             TEXT NOT NULL DEFAULT '',
    created_at        TEXT NOT NULL,
    PRIMARY KEY (transcript_id, preset)
);
```

- `Store::save_note` is an upsert on `(transcript_id, preset)` — regenerating
  replaces the row, including `created_at`.
- `Store::notes_for` reads in the product's preset order (Minutes, Action
  items, Bilingual brief); `Store::note` reads one.
- A v11 library upgrades in one transaction and keeps every row (tested on a
  populated library with a watched-files ledger row).

### UI — one state machine, two surfaces

`crates/suramak-ui/src/notes_sheet.rs` owns `NotesSheet` (`Ready` / `Running` /
`Failed`, the stored rows, the gate, the short-transcript note, the selected
preset, `Export .md` / `Copy` text). It is egui-free and unit-tested; both
surfaces drive it.

- **Desktop:** a `Notes` button beside `Export…` in the deck. The sheet is an
  anchored card in the existing Sheet plane: three preset rows, Run
  (Regenerate when a result exists), the result line, the breakdown, a
  scrolling Markdown body rendered as headings/body with the existing tokens,
  a destination field, `Export .md` and `Copy`.
- **Phone:** a `Notes` trailing action in the editor's app bar; the same sheet
  content in the compact sheet plane at 600 pt, with touch targets.
- The result view is one honest line —
  `12 citations checked · 1 removed (no matching audio)` — plus the breakdown
  (`1 no match`, `1 past end`, `1 malformed`, `1 duplicate`) only when
  something was removed. With nothing removed the line is
  `12 citations checked · 0 removed`. With no citations at all it says
  `no citations returned` rather than pretending.
- `Export .md` writes the exact stored `body_md` bytes; `Copy` puts the same
  bytes on the clipboard.

### Failure paths, named

| State | What the sheet says / does |
| --- | --- |
| no endpoint or key | Run is disabled; the setup sentence is shown verbatim (`notes are not configured — set the polish base URL in Settings and provide an API key …`) |
| endpoint error | `{preset} failed — Polish backend error: HTTP {status}: {body}`; the previous stored note stays |
| no citations returned | a successful note, line = `no citations returned` |
| transcript under 20 s | allowed; `this transcript is under 20 s — the notes will be short` |
| empty transcript | refused before any request: `Notes: the transcript has no segments — there is nothing to summarize` |
| reply for the wrong preset | refused, named (`the reply was for X but Y was requested`) |

Notes are **user-triggered only**: nothing runs on transcribe, no scanner opens
the sheet, and the only network call is the one the Run button starts.

---

## 2. Verbatim before/after — the mock endpoint

Captured from
`cargo test -p suramak-core --test notes minutes_request_carries -- --nocapture`.
The endpoint is the hand-rolled `TcpListener` mock; the reply is the assistant
message's `content` exactly as it arrived.

**Before — the mock endpoint's reply (the model fabricated `[13:00]`):**

```markdown
## Decisions

- Ship on Friday [00:12].
- The budget was approved [13:00].

## Follow-ups

- Budi owns the migration [01:05].
```

**After — the stored `body_md` (the fabricated citation is gone):**

```markdown
# Minutes

## Decisions

- Ship on Friday [00:12].
- The budget was approved.

## Follow-ups

- Budi owns the migration [01:05].
```

(A removed citation takes its own separator with it: the sentence reads
"approved.", never "approved .". The validator trims the space that belonged
to the citation, and the tests pin that for the end-of-line, mid-line and
sentence-final cases.)

**Counts for that run:**

```text
checked=3 removed=1 breakdown=CitationBreakdown { removed: 0, past_end: 1, malformed: 0, duplicate: 0 } parts=1
```

The segment starts in the fixture are 0:00, 0:12 and 1:05; 13:00 is past the
recording's 1:18 end, so it is counted as `past_end` — separately from a
no-match removal, exactly as the breakdown requires. The note kept its real
citations and the sentence that lost its citation remains (without the
timestamp), rather than the whole line being silently rewritten.

---

## 3. Tests

| Suite | What it pins |
| --- | --- |
| `suramak-core` `notes::tests` (16) | exact match; ±1.4 s rewrite; 2 s off removed; `hh:mm:ss`; several tokens per line; `[banana]` untouched; malformed; past-end; duplicate; breakdown sum; canonical clock; payload starts/speakers; all three prompts carry the contract; preset key round-trip; result-line copy (0 / N / none); chunk boundaries and coverage |
| `suramak-core/tests/notes.rs` (5) | one mock-endpoint test per preset (prompt carries the segments, body lands, fabricated `[13:00]` stripped); endpoint 503 names status + message; chunked run makes two requests and merges |
| `suramak-core` `store::tests` (3) | v11→v12 on a populated library; one row per `(transcript, preset)`; regenerate replaces body/counts/model/created_at; ordering; transcript delete cascades |
| `suramak-ui` `notes_sheet::tests` (11) | the three presets; not-configured refusal; result copy with 0 and N removed; no-citations line; run lifecycle; regenerate replaces; failure keeps the old result and names the endpoint error; wrong-preset refusal; export/copy exact bytes; short transcript; product order |
| `suramak-ui` desktop `app::notes::tests` (8) | the Notes action opens the sheet; not-configured refuses; Run snapshots unsaved edits; result asks for a save; failure keeps the stored note; three runtime+store round trips (Minutes / Action items / Bilingual) with a fabricated time stripped; `Export .md` writes the exact stored bytes |
| `suramak-ui` compact `model::tests` (2) | the phone sheet runs the shared machine (presets, Run, SaveNote, Export/Copy, host-loaded ordering); not-configured refuses with the named sentence |

---

## 4. Verification

| Gate | Command | Result |
| --- | --- | --- |
| Formatting | `cargo fmt --all -- --check` | clean |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| Android check | `cargo ndk -t arm64-v8a --platform 29 check -p suramak-android -p suramak-android-spike --locked` | clean |
| Android lints | `cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-android -p suramak-android-spike --all-targets -- -D warnings` | exit 0 |
| Full workspace | `cargo test --workspace` | see the count below |

---

## 5. Deliberate limits

- **No streaming.** One blocking request per preset (or per part), like polish.
- **The 24 000-character budget is the notes path's own.** The audio chunk cap
  measures WAV bytes, not prompt text; the constant is documented and a test
  pins the split.
- **The breakdown is not persisted.** The table carries the two counts the work
  order names (`citations_checked`, `citations_removed`); the four-field
  breakdown is live for the run that produced it. A reloaded note still shows
  the honest count line.
- **The phone host's generation runs on the calling thread** (`notes_generate`
  in `compact_host.rs`); the Android Activity is expected to dispatch it off
  the UI thread, the same rule the desktop's detached `suramak-notes` thread
  follows.
