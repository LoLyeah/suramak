# M15-B — Transactional split/merge and the glossary slice (M15.3 + M15.5)

Status: complete, gates green. Scope from `docs/WORKORDER-M15B.md`; parent plan
`docs/PLAN-M15-M18.md` §3 (M15.3 and M15.5). No app version bump (v0.4.0 is the
owner's call when all of M15 lands). No commit was made, no dependency was
added, no engine call was touched.

## Result in one line

Split and merge are now single SQLite transactions over the stored rows with an
exact, bounded, per-session undo log; the glossary builds a Whisper initial
prompt inside a budget **measured from the model at runtime** (448 ÷ 2 = 224
tokens on the shipped `ggml-tiny`), reports what it dropped, and feeds both
transcription paths; a separate dictionary, opt-in Indonesian rules and a
previewed clean export operate on derived copies only — the inference text is
never rewritten.

## Gates

| gate | result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **681 passed / 0 failed / 8 ignored** (baseline 619/0/8; **+62**) |
| schema | v2 → **v3**, additive only (one column + four tables, no rebuild) |
| new dependencies | none |
| disk | `target/` 6.7 GB; 15 GB free on `/` after the full run |

The full workspace suite was run **twice**: once as a checkpoint after the last
structural change, and once as the final gate (the numbers above are that run).
During development only crate-scoped runs were used (`-p suramak-core`,
`-p suramak-asr`, `-p suramak-ui`). Two small runtime-only edits after that final
run — reloading the measured budget when the default model changes, and a helper
that refreshes the plan without re-seeding the form — were re-verified with the
`suramak-ui` + `suramak-core` suites (all green); the workspace total is
unchanged.

## Slice 1 — M15.3: transactional split/merge

### What a split is, exactly

`suramak-core::segment_edit` is the pure half; the store runs it inside one
transaction.

**Text.** The split is a word-boundary partition, not a byte cut:

- the caret must be on a UTF-8 character boundary — otherwise `NotCharBoundary`
  (a caret one byte into `é` is refused, not silently rounded);
- a caret at either end has no two sides — `EmptySide`;
- the boundary is the whitespace run the caret sits in or immediately follows,
  so a caret on the space of `halo dunia` splits there, and a caret on a dash
  keeps the dash with the right half;
- a caret at the start of a word whose preceding run would leave an empty left
  half uses the run after it instead (the closest legal boundary);
- no whitespace anywhere means the segment is one unbreakable token —
  `NoWordBoundary`, nothing written. `pertanggungjawaban` cannot be split, and
  that is the correct answer rather than a mid-word cut;
- both halves are trimmed; an empty half after trimming is `EmptySide`.

**Time.** `[start, end)` is cut at `at_ms` with `start < at < end`; the left keeps
`[start, at)`, the right `[at, end)`. No gap, no overlap, no negative or zero
crossing, and `end > start` on both sides. The UI passes no cut time, so the
store uses the span midpoint (deterministic); the store API accepts an explicit
`at_ms` for a caller that has one.

**Zero-duration segments** reuse the exporter's own policy
(`suramak_core::export::ZeroDuration`, M15.4): `Skip` refuses the split with
`ZeroDuration`; `OneFrame` widens the span to one 25 fps frame (40 ms) so the cut
has two positive sides. A negative span is treated the same way — the result can
never be a negative cue.

**Merge.** Two or more *contiguous* rows become one: the parts are trimmed and
joined with the configured separator, the span is the union (`min start`,
`max end`), the merged row takes the first row's position and later rows shift
down. Refusals: fewer than two rows, more than `MAX_MERGE_ROWS` (256), a hole in
the numbering, or a range past the end.

**Provenance (the M15-A invariant).** `source_text` — the first text ever stored,
i.e. the inference output — is never dropped:

- a split gives **both halves the parent's `source_text`** (the parent's
  inference text covers both halves' audio);
- a merge **concatenates** the parts' `source_text` with a newline (`None` only
  when no part had any);
- every other row is rewritten byte-identically, provenance included.

`segments.derived` (new in v3) records how the row's `text` came to be: `split`,
`merge`, or empty for an untouched row.

### The transaction

`Store::split_segment` / `Store::merge_segments` share `transact_rows`: one
savepoint that reads the row set, lets the pure function derive the next set,
replaces all rows with their own `source_text`/`derived`, and touches
`updated_at`. A refusal from the pure function returns **before any write**; a
write failure rolls the whole set back. The transcript is therefore the old set
or the new one, never a mix — and a fresh app session reads exactly one of them
because SQLite commits the savepoint atomically (test:
`a_reopened_store_reads_only_committed_rows`; the mid-write trigger test
`a_failed_structural_write_rolls_the_whole_set_back` aborts the second row after
the delete and the first insert are already written).

**Alignment and derived staleness.** The only alignment this tree stores is the
segment `start_ms`/`end_ms`; a split recomputes both sides and a merge their
union, so there is no separate alignment table to invalidate (M15-A's inventory
found no word timestamps). There are still **no polish or translation staleness
columns**; `derived` is the marker this slice adds and uses, and a later slice
that stores polish/translation provenance can invalidate on it.

### Undo/redo: exact, bounded, per session

The log is `SegmentHistory` — an in-memory `VecDeque` per open transcript:

- **bounded at 20 entries** (`UNDO_DEPTH`); the oldest falls off the back;
- an entry is a `SegmentSnapshot { before, after }` — the **actual rows**, not an
  inverse operation. Undo writes `before`, redo writes `after`, byte-exact, so no
  re-derivation can drift (a merge that trimmed a double space undoes to the
  double space);
- a new edit clears the redo branch (the linear rule);
- **reset on close**: opening another transcript clears it, and the log lives in
  `AppState`, so a fresh app session offers no undo for edits it did not witness;
- the cursor moves **only after a successful write** (`undo_segment_edit` peeks;
  `on_segments_restored` moves it), so a failed restore cannot desynchronise the
  log from the database.

### Shortcuts (documented, platform-appropriate)

| binding | action | scope |
|---|---|---|
| **⌘⇧S** / **Ctrl+Shift+S** | split the focused row at the caret | editor |
| **⌘⇧M** / **Ctrl+Shift+M** | merge the focused row with the next | editor |
| **⌘Z** / **Ctrl+Z** | undo the last split/merge | editor, **not** while typing |
| **⌘⇧Z** / **Ctrl+Shift+Z** | redo the last undone split/merge | editor, **not** while typing |

`⌘` is `egui::Modifiers::COMMAND`, which egui-winit fills from the control key on
Windows/Linux — the same mechanism the existing ⌘R/⌘E/⌘⌫ bindings use, no
platform fork. Two deliberate details:

- **Split and merge are exceptions to the text-focus guard** (alongside Esc):
  they need the caret, which only a focused row has, and ⌘⇧ is not a typing key.
  Undo/redo stay guarded — while typing, ⌘Z belongs to the text field's own undo.
- **Shift is exact** in `matches_binding`: ⌘Z never fires for ⌘⇧Z, and a shifted
  ⌘E no longer toggles the sheet. Tests:
  `a_shifted_command_binding_never_fires_its_unshifted_sibling`.

### UI surface (no new screens)

- The focused row shows two tertiary capsules, **Split** and **Merge**, in its
  spare width (hover text names the shortcut). They act exactly like the
  keyboard path.
- The caret is read from egui's `TextEditState` each frame (char index → byte
  offset) and kept on the `Editor`; without a caret the split uses the
  whitespace nearest the text's midpoint.
- pending text is flushed **before** the structural effect runs: when the editor
  is dirty the split/merge waits for the save (`pending_structural`), so the
  transaction partitions what the reader typed. If that save fails the waiting
  edit is **cancelled** and the reader's text stays in the buffer — a split can
  never silently replace unsaved text with the stored row;
- the status line reports the outcome (`split applied — 4 segments (⌘Z undoes
  it)`, or the named refusal).

### Search consistency

`Store::search` is the same literal LIKE scan; it has no index, so the next call
re-reads the rows the transaction wrote. Tests assert it explicitly after each
mutation: after a split both halves are found once, after an edit the removed
word is gone, after a merge the joined text is found as one row
(`search_stays_consistent_after_each_structural_edit`). There is no stale index
to invalidate, by construction.

### Verification (34 new tests)

Pure rules (`segment_edit`): caret snapping (mid-word, on the space, multi-byte,
unbreakable token), half-open spans, edge cuts refused, zero-duration under both
policies, split row renumbering and span tiling, provenance duplication,
merge separators, provenance concatenation, merge refusals, snapshot round-trip,
bounded history (depth, redo clearing, undo/redo order).

Store: split persists two rows and leaves the rest byte-identical, search after
each mutation, refusals write nothing, `Skip`/`OneFrame` in the store, merge with
provenance, undo/redo exact rows, mid-write rollback, reopen reads committed
rows, v2 → v3 additive upgrade.

UI: split uses the caret and flushes first, midpoint fallback, applied split
rebuilds buffers and records one step, undo moves the log only on success, empty
log is a status line, ⌘⇧S dispatches, shift exactness, structural shortcuts
survive the typing guard.

### Honest boundaries

- The time cut is the span midpoint when the UI calls it; a caller may pass an
  explicit cut through the store API but nothing derives it from the caret's
  position in the text.
- Undo covers split/merge only — not text edits, not deletes, not polish. It is
  a session convenience, not history: 20 steps, gone on close.
- `MAX_MERGE_ROWS = 256` bounds one transaction; a merge of an entire long
  transcript is not offered (the UI merges pairs).
- A split of a zero-duration segment under `OneFrame` *widens* the parent to
  40 ms; that is the M15.4 policy, but it does invent 40 ms of timing where the
  model produced none. `Skip` refuses instead.
- Alignment is segment-level only; there is no word-level alignment to keep in
  sync (M15.6 may add one — it will have to invalidate on `derived`).

## Slice 2 — M15.5: glossary, dictionary, rules, clean export

Four mechanisms, kept apart on purpose:

| mechanism | input | output | stored where |
|---|---|---|---|
| glossary | terms | Whisper initial prompt | `glossary_terms` |
| dictionary | `from = to` rules | derived text | `dictionary_entries` |
| normalization | built-in Indonesian rules | derived text | `text_prefs.normalize` |
| clean export | the above + whitespace | a derived transcript (previewed) | — |

### The prompt budget: measured, not assumed

The cap comes from the loaded model, not from a constant:

```
cap = whisper_n_text_ctx(model) / 2
```

whisper.cpp keeps at most half of the model's text context for
`<|startofprev|>` prompt tokens and silently drops the rest from the front, so
that quotient is the real budget, not an estimate. The measurement path
(`suramak-asr::prompt_budget::ModelTokenizer`) opens the model read-only on the
CPU, reads `n_text_ctx`, and can tokenize with the model's own byte-level BPE.
It never creates a state, never calls `full()`, and is not on the inference path.

**Measured on this machine** (`ggml-tiny.bin`, the model the app ships with):

```
$ suramak-cli prompt-budget --model tiny --text "Kecamatan Cilandak" \
      --text "The quick brown fox jumps over the lazy dog."
  n_text_ctx:  448 tokens (measured from the loaded model)
  prompt cap:  224 tokens (n_text_ctx / 2)
  tokens:      6   "Kecamatan Cilandak"
  tokens:      10  "The quick brown fox jumps over the lazy dog."
```

A 64-term glossary (`Kecamatan Contoh Wilayah 0..63`) yields
**219 of 224 tokens, 22 terms in, 42 dropped** — the real tokenizer, not a guess.
Those numbers are reproducible with the command above and
`--glossary <file>`; the ASR test `the_measured_budget_bounds_the_real_prompt`
asserts the plan equals the tokenizer's own count for the text it built.

**Where the measurement lives.** The settings surface shows the measured caption
(`prompt budget: 224 tokens measured from ggml-tiny (n_text_ctx 448 ÷ 2)`) or,
when nothing has been measured for the current model, the documented fallback
(224) with `documented fallback` spelled out. "Measure model" runs the load on a
detached thread, and the result is cached in `prompt_budgets` (one row per model,
upserted). The runtime seeds the budget from the cache at startup and after a
model change.

**Counting in the job path.** Building the prompt on the inference path cannot
use the tokenizer (that would mean holding a model context in the UI runtime or
touching the whisper call sites), so the builder counts with
`EstimatedTokens = ceil(bytes / 3)`, a deliberately conservative byte estimate.
Its calibration was measured rather than assumed:

| text | estimate | real |
|---|---|---|
| `Kecamatan Cilandak` | 6 | 6 |
| `pertanggungjawaban` | 6 | 6 |
| `Kecamatan Cilandak, Jakarta Selatan` | 13 | 11 |
| `The quick brown fox jumps over the lazy dog.` | 15 | 10 |
| `🎙️ emoji and café` | 7 | 7 |
| `BPJS Kesehatan dan NIK` | 8 | **10** |

It over-counts English by ~50 % and matched the Indonesian phrases above, but
the acronym-heavy phrase shows the direction that is not safe: an estimate can
under-count. This is stated as a boundary below, not hidden behind a "never over
budget" claim.

**The builder** (`build_prompt`) is deterministic: the configured
`settings.asr.initial_prompt` first (verbatim — an over-budget setting is kept
and every term dropped, with `base_over_budget` set), then terms in stored order
joined with `", "`, no label and no sentence wrapper (instruction text is what
tends to leak into the transcript). A term that does not fit is **skipped and
the next one is tried**, so one long term cannot shut out every short one after
it. The plan reports `used_tokens`, `included`, `dropped`, and a one-sentence
`feedback()`.

**Wiring:** `Runtime::initial_prompt()` is the single place base + glossary +
budget are combined, and both transcription paths (live and file jobs) read it —
a glossary can never reach one and not the other.

### The dictionary: idempotent and reversible by construction

Whole-word, case-sensitive, single pass over the original text (`from = to`,
`->`, `=>` all accepted). Two algebraic properties make it safe to re-run:

- **idempotent**: a pass never re-scans its own output, and an entry whose
  `from` appears as a whole word inside another entry's `to` (in either
  direction) is **refused at parse time**. `gak = tidak` plus `tidak = bukan`
  would chain, so the second line is rejected with a reason instead of producing
  a dictionary that changes its answer on the second run;
- **reversible**: `Corrected` carries an edit log of the output span and the
  exact text it replaced, so `restore()` rebuilds the input byte-for-byte
  without a second rule pass. `apply_inverse` (text-level `to → from`) is the
  weaker inverse, exact whenever `to` is unambiguous.

Applied after inference to a **derived** layer: either the previewed export copy
or, for a caller that asks for it, the editable `text` (which is already derived;
`source_text` is never touched — `update_segment_text` cannot reach it).

### Indonesian normalization: opt-in, guarded, reversible

Seven built-in rules (`gak/nggak → tidak`, `kalo → kalau`, `udah → sudah`,
`gimana → bagaimana`, `aja → saja`, `kayak → seperti`), word-boundary only, off
by default, with an edit log like the dictionary's (so every application can be
restored exactly).

Code-switching is respected by three guards, checked per token:

1. no rule fires inside a longer token (word boundaries throughout; a hyphen is
   a boundary, an underscore is not);
2. rules with `Guard::SafeContext` — `kayak` — never fire next to a determiner
   (`the/a/an/this/that/my/…/ini/itu/sang/para`) or at a clause-final position
   (immediately before `. , ? ! ; :` or the end of the text);
3. no `SafeContext` rule fires in a ±3-token window that reads as English (two
   or more English function words).

**The `kayak` fixture from the plan**, encoded in
`the_kayak_fixture_preserves_the_english_noun_and_the_trailing_hedge`:

| input | output | why |
|---|---|---|
| `The kayak drifted downstream.` | unchanged | determiner before, clause-final after |
| `We rented a kayak in Ubud.` | unchanged | determiner |
| `I packed my kayak, then left.` | unchanged | possessive + clause-final |
| `Dia bilang gitu, kayak.` | unchanged | clause-final hedge |
| `terus gitu kayak` | unchanged | clause-final (end of text) |
| `Dia kayak gitu terus` | `Dia seperti gitu terus` | safe non-final position |
| `She said the thing is kayak gitu` | unchanged | English run (`the`, `is`) |

Plus `The meeting is at four; aku gak bisa datang.` → the English clause survives,
`gak → tidak` in the Indonesian one, and `gak_bisa` is never touched (one token).

### Clean export: preview first, apply explicitly

The export sheet now has a **Verbatim | Clean** segmented control. Verbatim is
the default and byte-identical to the old behaviour. Clean shows three opts
(dictionary, normalization, collapse whitespace), a **Preview** button, and the
diff it produced (every changed row: `before → after`, struck-through before).
The primary action becomes **Apply & export**, and `AppState::start_export`
**refuses to write without a preview** ("preview the clean-up first — nothing is
exported without it").

The state machine is covered by tests:

- `a_clean_export_is_refused_until_a_preview_exists` — refusal, then preview,
  then the `Export` effect carries the cleaned derived transcript;
- `any_edit_or_option_change_drops_the_preview` — a keystroke or a transform
  toggle invalidates it, and the next export is refused again;
- `verbatim_mode_exports_the_stored_text_without_a_preview`;
- `the_clean_preview_is_a_diff_and_nothing_else` (core) — the preview writes
  nothing and `apply` returns a copy;
- `the_clean_pipeline_is_idempotent` (core) — cleaning a cleaned transcript is a
  no-op.

### Storage (schema v3, additive)

```
ALTER TABLE segments ADD COLUMN derived TEXT NOT NULL DEFAULT '';
CREATE TABLE glossary_terms      (position INTEGER PRIMARY KEY, term TEXT, created_at TEXT);
CREATE TABLE dictionary_entries  (position INTEGER PRIMARY KEY, from_text TEXT, to_text TEXT, created_at TEXT);
CREATE TABLE text_prefs          (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE prompt_budgets      (model_id TEXT PRIMARY KEY, n_text_ctx INTEGER, cap_tokens INTEGER, measured_at TEXT);
```

No table rebuilt, no column dropped, no row deleted. The v1 → v2 → v3 chain runs
in one open (each step in its own transaction, each stamping its own version);
`a_v2_database_upgrades_to_v3_additively` and the existing v1 fixture test both
pass. Every value the settings screen edits is written through
`Store::save_text_settings` in one transaction; nothing lives in `settings.toml`
except the pre-existing `asr.initial_prompt` (which the builder uses as the
base).

### Verification (28 new tests across core, UI, CLI)

Core: glossary parsing/caps/round-trip, budget arithmetic, builder ordering and
truncation, over-budget base, empty glossary, conservative estimate, dictionary
parse/separators/refusals, whole-word matching, idempotence and the no-chain
rule, edit-log round-trip, normalizer rewrites, the `kayak` fixture, code-switch
fixtures, idempotence/reversibility, clean preview diff/apply, whitespace-only
cleaning, pipeline idempotence.

UI: clean export refused without preview, preview then apply, invalidation on
edit and on option change, verbatim path, text-rules save only when changed,
prompt plan reports dropped terms.

ASR: the real model reports `n_text_ctx`/cap and tokenizes (measured on this
machine, skipped when no model file exists), the measured budget bounds a real
64-term prompt, a missing model is a named error.

CLI: `suramak-cli prompt-budget` is the reproducible headless measurement.

### Honest boundaries

- **Prompt biasing is not a WER fix.** A glossary shifts decoding toward the
  listed spellings; it cannot make an inaudible name audible.
- **The token count in the job path is an estimate.** The cap is measured and
  exact; the count is `ceil(bytes/3)`, which over-counts English (~50 %) and
  matched the Indonesian samples — but under-counted one acronym-heavy phrase
  (8 vs 10 real tokens). If the real count exceeds 224, whisper.cpp truncates the
  *front* of the prompt, which would drop early terms silently. The settings
  caption marks the figure with `~`; the exact count is one command away
  (`suramak-cli prompt-budget`) and the Measure action reports the model's own
  context. Making every job exact would need the tokenizer on the inference
  path, which this slice was told not to touch.
- The 224 fallback is shown as a fallback, in words, until a model is measured.
  A model change (or `auto` resolving to a different file) needs a re-measure;
  nothing measures automatically at startup.
- **The normalization table is seven rules.** It is a heuristic over a fixed
  list, not a POS tagger: a code-switched bare noun without a determiner
  (`beli kayak baru`) can still be rewritten. That is why the whole feature is
  opt-in, previewed in the export sheet, and reversible.
- **The dictionary is case-sensitive.** `BPJS` does not match `Bpjs`; the
  caption says so. It has no plurals, no morphology, no regex.
- **"Reversible" means the edit log.** `restore()` is exact from the corrected
  string; the text-level `apply_inverse` is exact only when the replacement does
  not occur for another reason. The clean export never stores its output — it
  recomputes from the stored text and rules, which is the strongest form of
  re-applicable but also means the export is not a snapshot.
- The normalization and dictionary never run automatically on transcription
  output: nothing changes until the user opts in (normalization) or previews an
  export. A dictionary applied to the editable `text` is still a manual caller
  action, not a background pass.
- The settings section is one long scroll under "Text rules"; there is no
  per-term editor, no import/export of a glossary file in the UI (the CLI reads
  one for measurement), and no undo inside the field beyond egui's own.

## Files

New:
- `crates/suramak-core/src/segment_edit.rs` — pure split/merge rules, snapshots, bounded history
- `crates/suramak-core/src/textrules.rs` — glossary, prompt budget, dictionary, normalizer, clean preview
- `crates/suramak-asr/src/prompt_budget.rs` — measured model tokenizer/budget
- `crates/suramak-cli/src/cmd/prompt_budget.rs` — headless measurement
- `docs/reports/M15b-transactions-glossary.md` — this report

Modified:
- `crates/suramak-core/src/store.rs` — schema v3, transactional split/merge/restore, text-rules persistence
- `crates/suramak-core/src/error.rs` — `SegmentEdit` variant
- `crates/suramak-core/src/lib.rs` — module list
- `crates/suramak-ui/src/app/mod.rs` — editor caret, structural effects, clean-export state machine, history
- `crates/suramak-ui/src/app/keymap.rs` — ⌘⇧S/⌘⇧M/⌘Z/⌘⇧Z, exact-shift matching, guard exceptions
- `crates/suramak-ui/src/app/editor.rs` — caret tracking, row Split/Merge, edit/preview invalidation
- `crates/suramak-ui/src/app/export.rs` — Verbatim/Clean, preview diff, Apply & export
- `crates/suramak-ui/src/app/settings.rs` — "Text rules" section
- `crates/suramak-ui/src/app/settings_form.rs` — text-rule fields and parse/seed
- `crates/suramak-ui/src/runtime.rs` — effect handling, prompt merge, background measurement
- `crates/suramak-ui/src/main.rs` — startup seeding, measurement poll
- `crates/suramak-core/tests/persistence.rs` — v3 expectations
- `crates/suramak-cli/src/{main.rs,cmd/mod.rs}` — `prompt-budget` command
