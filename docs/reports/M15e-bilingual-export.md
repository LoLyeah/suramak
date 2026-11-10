# M15-E — Opt-in bilingual export (M15.8)

Status: implemented on top of `main` at `5d026a9` (M15-C merged, 723 tests).
Tree after this slice: **779 passing tests** (workspace), `cargo fmt --check`
clean, `cargo clippy --workspace --all-targets` clean, one full workspace test
run at the end. No new dependencies, no engine changes, no CI edits, no Android
work, no git commit.

The real endpoint is **opt-in and saw zero traffic**: every test that touches
HTTP points at a hand-rolled `TcpListener` on `127.0.0.1`; the zero-request
rule is asserted against a live listener, not promised.

## What landed

| Area | Change |
| --- | --- |
| Core | `translate.rs` — target languages, bounded batch planner, request body, per-id reply validation, cooperative cancellation, run provenance |
| Core | `segment_edit.rs` — `SegmentRow.translation`, split/merge propagation, both always stale |
| Core | `export.rs` — stacked SRT/VTT and the bilingual ASS style pair, plus both-line advisory diagnostics |
| Core | `store.rs` — schema **v5** (additive): translation text, model, timestamp, source-id set, sticky stale flag; stale marking on every text mutation; restore preserves stale |
| UI | `app/bilingual.rs` — consent gate, run lifecycle, partial-failure retry, cancellation, editable derived column, load/save paths |
| UI | `app/export.rs` — always-visible bilingual section (disabled with the resolved reason), target control, translate/cancel/retry, export switch |
| UI | `app/editor.rs` — derived column under each source line with provenance caption, stale badge, its own CPL/CPS badge |
| UI | `runtime.rs` / `main.rs` — detached `suramak-translate` thread, polled progress/terminal events, same `CancelToken` seam as the ASR job |

## Batch sizing (the documented bound)

`translate::plan_batches` splits a run into requests of:

- **≤ 20 segments** (`MAX_BATCH_SEGMENTS`), and
- **≤ 2 000 source characters** (`MAX_BATCH_CHARS`, counted as Unicode scalar
  values, not bytes — roughly a minute of speech).

Rules:

1. **A segment is never cut.** A single segment longer than 2 000 characters is
   sent alone as one oversized request; splitting text mid-cue would break cue
   alignment and produce two translations for one cue.
2. **Empty / whitespace-only segments are not sent.** There is nothing to
   translate, and inventing a line would break cue parity; an untranslated cue
   exports as a single-line cue.
3. Order is transcript order; ids are the segment ids, so batches tile the
   transcript without overlap or gaps.
4. Failures are per batch, named per segment: an HTTP/transport failure marks
   every segment in that batch with the same message (the batch is the unit the
   endpoint saw), while a malformed reply fails only the ids it actually
   invalidated (missing, duplicate, empty). The run continues with the next
   batch and returns the partial result.
5. **Retry re-sends only the failed ids** (one bounded request set), never the
   whole transcript.

At the default 20/2 000, a 45-segment transcript is 3 requests (20 + 20 + 5);
a 21-segment transcript is 2 (20 + 1). A request timeout is 120 s for the
batch (the existing client envelope), so one stuck batch delays — never
deadlocks — the run.

## Stale semantics, mutation by mutation

A translation record carries `text`, `model`, `translated_at` (RFC 3339 UTC,
one timestamp per run), `source_ids` (the transcript rows it was produced
from), and `stale`. `stale` is **monotonic**: only a new translation can clear
it. Nothing overwrites the original inference text (`source_text`) and nothing
silently rewrites a translation.

| Mutation | Translation text | Stale | Provenance |
| --- | --- | --- | --- |
| **Manual source edit** (`update_segment_text`, autosave) | kept | **set** (only if the text actually changed) | unchanged |
| **Wholesale rewrite** (`upsert_segments`, e.g. re-polish) | kept | **set** when the row's `text` differs from the stored text | unchanged |
| **Split** (M15.3) | parent's translation duplicated onto **both** halves | **set on both** (the source was repartitioned; neither half is a verified translation of its new text) | parent's model/timestamp/source ids kept on both |
| **Merge** (M15.3) | parts' translations joined with the merge separator | **set** (the joined line is new text) | model kept only when every part agrees, else `mixed models`; timestamp = latest part; source ids unioned |
| **Edit the translation** (derived column) | replaced by the user's text | **unchanged** (editing the derived line is not a source change) | unchanged |
| **Undo / redo of a structural edit** (`restore_segment_rows`) | rows restored exactly | **sticky**: a row position that is currently stale stays stale even if the restored snapshot predates the edit | restored with the row |
| **Re-translate** (whole transcript or failed ids) | replaced | **cleared** | replaced with the new run's model/timestamp/source ids |
| **Delete transcript** | removed with the row (FK cascade) | — | — |

The stale badge is visible in two places: the editor's derived-column caption
(`stale — source changed`) and the export sheet's status line
(`N stale — the source changed, re-translate to refresh`). Exporting a stale
translation is still allowed — the text is the user's data — but the badge is
never hidden.

The restore rule is deliberately conservative. A byte-exact history replay
could theoretically clear a marker that was set by an edit outside the
snapshot; keeping it can only over-warn, never silently vouch for derived text
the app did not re-check.

## Output formats

All three bilingual exporters align cues **by segment id**, never by array
position: an orphan translation (a row whose segment no longer exists after an
edit) is ignored; a segment without a translation exports as a single-line cue.
Partial runs are therefore exportable.

### SRT — stacked

One cue per segment:

```
1
00:00:00,000 --> 00:00:02,400
Hello world.
Halo dunia.

2
00:00:02,400 --> 00:00:05,000
This is a test.
```

- The source line comes first, the translation directly beneath it.
- **The "blank" line in the workorder's schematic is not written into the cue.**
  In SRT an empty line *terminates* the cue; a literal blank between the two
  languages would split every cue into two and destroy the timing alignment.
  The vertical separation is carried by the ASS style pair instead, and the
  omission is documented in the exporter itself.
- No escaping is applied beyond newline normalization: SRT has no markup, and
  a blank line is removed from the payload (it could only be a terminator).
- Multiline translations are kept as hard line breaks, so a translator that
  returns two lines still reads as one cue.

### WebVTT — stacked

Same two-line layout, with cue-payload escaping: `&` → `&amp;`, `<` → `&lt;`,
`>` → `&gt;` on **both** lines. Without it WebVTT would parse `<i>` as a tag and
a bare `&` as an entity start; the timing line is untouched.

### ASS — the M15-A style pair

Two styles and two `Dialogue` events per cue:

| Style | Position | Alignment | Purpose |
| --- | --- | --- | --- |
| `Primary` | bottom-centre | `2` | the transcript's own line |
| `Secondary` | top-centre | `8` | the translation |

- Both events share one start/end, so the pair cannot drift.
- The zero-duration policy applies to the **pair as a unit**: `Skip` drops both
  events, `OneFrame` widens both to the same 25 fps frame (40 ms). A skipped
  cue never leaves half a bilingual row behind.
- Text escaping is the existing M15-A ASS path: `{`/`}` escaped, hard breaks
  become `\N`, a literal `\N`/`\n`/`\h` gets a zero-width separator; commas are
  safe because `Text` is the last field.
- An empty half is omitted (the other half still renders); no empty `Dialogue`
  is written.

### Per-line budget and badges

- `BILINGUAL_LINE_BUDGET_CPL = 42` — the honest ceiling for **each** of the two
  stacked lines, taken from the documented CPL ladder's error threshold. It is
  advisory: the exporter never wraps, shortens or truncates a translation
  (cutting a machine translation mid-sentence is worse than a long line).
- `export::bilingual_advisories` measures the source line **and** the
  translated line separately against the same `SubtitleLimits` and cue timing.
  The editor paints a badge on each line (CPL and/or CPS), and the hover text
  says the translated line is measured separately. This is the "CPL/CPS
  advisory badges show on BOTH lines" gate.

## Failure and cancellation

- **Per-segment naming**: every failure is `segment <idx>: <reason>`. The export
  sheet lists up to five inline and `and N more failed segments`; Retry resends
  exactly the failed ids. A failed segment's editor row also carries a red
  `translation failed — retry in Export…` caption, so the gap is visible where
  the text would have been.
- **Partial results are real results**: validated rows are persisted with
  provenance immediately when the run returns; failed rows are untouched (no
  placeholder, no empty translation).
- **Cancellation** uses the same `CancelToken` seam as the transcription job
  (`suramak-asr::cancel::CancelToken`, `Effect::CancelTranslation`). It is
  observed **before and after every batch** — never mid-request, matching the
  worker's stage-boundary rule. Completed batches are kept and persisted; the
  status line says `cancelled after N/M batches`. No request follows an
  observed cancellation.
- **Switching transcripts mid-run** cancels the run at its next batch boundary
  and resets the UI state, so a late result can never land on the wrong
  transcript's derived column.
- **Zero requests while disabled** is asserted at two levels:
  - `bilingual_gate` returns the resolved reason (no settings section / blank
    base URL / no API key), `can_translate` is false, `start_translation`
    returns no effect, and the UI draws the disabled action with that reason;
  - a UI test runs the state machine against a real non-blocking
    `TcpListener` and asserts **no connection** is ever accepted.
  The runtime also re-checks the gate before spawning the thread, so a stale UI
  flag cannot fire a request.

## No-endpoint users

The bilingual section is always drawn in the export sheet: target chips, the
disabled `Translate for export` action, and the resolved reason
(`bilingual export needs the endpoint configured in Settings (base URL + API
key) — the transcript still exports normally without it`, or the specific
missing-key message). Markdown/TXT/JSON and the ordinary SRT/VTT/ASS paths are
unchanged; the bilingual checkbox is disabled without a translation and the
surface says to choose SRT/VTT/ASS when the selected format has no second-line
slot.

## Tests added (56)

- `suramak-core/src/translate.rs` — 14: batch count/char bounds, oversized
  segment, empty skip, request shape, target mapping, per-id reply failures
  (missing/empty/duplicate/garbage/fenced), EN→ID and ID→EN fixture runs,
  partial batch + retry, cancellation before/between batches, progress order,
  provenance.
- `suramak-core/tests/translate.rs` — 6 over a real HTTP mock: EN→ID, ID→EN,
  capped batches with exact ids, HTTP 500 names every segment in the batch and
  the next batch still lands, retry sends only failed ids, cancelled run sends
  no further request, unreachable endpoint fails the batch not the process.
- `suramak-core/tests/export.rs` — 7: bilingual SRT golden, id alignment with
  orphans, VTT escaping on both lines, ASS style pair + two events per cue,
  ASS escaping + zero-duration pair policy, both-line advisories/budget,
  zero-duration SRT/VTT never dropped.
- `suramak-core/src/store.rs` — 7: provenance round trip, source-edit stale vs
  translation-edit not stale, no-op save does not stale, split duplicates +
  stales, merge joins + stales, stale survives restore and re-translate clears,
  v4→v5 additive upgrade.
- `suramak-core/src/segment_edit.rs` — 2: split duplicate/stale, merge join /
  mixed models / source-id union.
- `suramak-ui/src/app/bilingual.rs` — 18: gate reasons, disabled surface sends
  nothing, run snapshot/target, save-before-translate, finished run applies +
  persists provenance, partial failure + retry ids, cancellation keeps partial,
  run-level failure, stray result, stale semantics, load by id, derived-save
  debounce, EN→ID and ID→EN runtime round trips, zero-request listener,
  mid-flight cancel over HTTP, transcript switch reset.
- `suramak-ui/src/app/mod.rs` — 2: bilingual SRT through `Effect::Export`,
  refusal for formats without a second-line slot.

## Honest boundaries

1. **The real endpoint was never contacted.** All network-shaped tests use the
   loopback mock. Translation quality, prompt adherence and the endpoint's
   actual JSON mode are therefore unverified by this slice; the system prompt
   asks for same-id JSON and a single line per cue, and the parser tolerates
   fences, but a real provider's quirks (refusals, truncation, extra fields) are
   only covered as far as the per-id validation reaches.
2. **No per-row "re-translate this one" button.** Re-translation is the sheet's
   action over the whole transcript or the Retry-failed set. A single stale row
   is re-translated by retrying with only that id only through the failed path;
   a targeted stale-row action is future work.
3. **Cancellation is between batches, not mid-request.** A batch already on the
   wire finishes (up to the 120 s client timeout) before the cancel is observed.
   This is the same boundary the ASR worker documents.
4. **A run abandoned by switching transcripts is discarded.** The switch
   cancels the token, but if a batch already returned, its validated rows are
   not persisted because the run no longer has an editor to attach to. The
   alternative (persisting by transcript id from the thread) would need the
   runtime to own the derived-column write path; not done here.
5. **Staleness is intentionally conservative across undo.** A restore keeps a
   current stale marker even if the restored snapshot was fresh. It can
   over-warn; it never under-warns.
6. **No wrapping or truncation.** The 42-CPL budget is advisory only. A long
   translation stays long, with a badge. There is no automatic line-breaking,
   because the formats only break where the text says so (the M15-A ASS
   `WrapStyle: 2` invariant).
7. **SRT/VTT cannot carry the blank separator.** Documented above; the ASS pair
   is where the vertical separation lives.
8. **The CLI has no bilingual flag.** The desktop export sheet is the only
   surface; `suramak-cli export` still writes the single-language formats. The
   workorder's surface was the app's export sheet.
9. **One timestamp per run, not per segment.** All records from a run share the
   run timestamp, taken once when the run starts; a retry gets its own. That is
   enough to order runs, not enough to prove which batch produced which row.
10. **No glossary/prompt-budget integration.** Translation sends the segment
    text as-is; the M15.5 glossary and prompt builder are not injected (the
    system prompt is fixed and pinned). A future slice could route the glossary
    into translation, but that changes the wire contract and was out of scope.
11. **Target language is a user choice, not detection.** It is seeded from
    `transcript.language` (ID → English, anything else → Indonesian) and is
    editable; a wrong transcript language defaults to the wrong direction, which
    the user must correct before the run.
12. **Only SRT/VTT/ASS have bilingual variants.** Markdown/TXT/JSON are
    single-language by design and the export surface says so instead of writing
    a half-bilingual file.

## Files

- `crates/suramak-core/src/translate.rs` (new)
- `crates/suramak-core/src/export.rs`
- `crates/suramak-core/src/segment_edit.rs`
- `crates/suramak-core/src/store.rs`
- `crates/suramak-core/src/lib.rs`
- `crates/suramak-core/tests/translate.rs` (new)
- `crates/suramak-core/tests/export.rs`
- `crates/suramak-core/tests/persistence.rs`
- `crates/suramak-ui/src/app/bilingual.rs` (new)
- `crates/suramak-ui/src/app/mod.rs`
- `crates/suramak-ui/src/app/polish.rs` (shared test env lock)
- `crates/suramak-ui/src/app/export.rs`
- `crates/suramak-ui/src/app/editor.rs`
- `crates/suramak-ui/src/runtime.rs`
- `crates/suramak-ui/src/main.rs`
- `docs/reports/M15e-bilingual-export.md` (this report)
