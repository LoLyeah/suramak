# WORKORDER — M15-B: transactions + glossary slices (M15.3 + M15.5)

Plan source: `docs/PLAN-M15-M18.md` §3 M15.3 and M15.5 (both size M — bounded,
cross-layer). Tree: `main` at `829ec09` (M15-A merged: 619 tests, source_text
column shipped, ASS/diagnostics in). Deliverable orientation still: correct the
transcript and the model prompt without touching engines.

## Slice 1 — M15.3 Transactional split/merge · M

- Segment split/merge as TRANSACTIONS on the store (reuse the v2 schema's
  source_text / derived provenance columns): split = deterministic text
  partitioning (validated caret/word boundary), merge = concatenated text with
  configurable separator, both invalidating/recomputing the derived alignment
  and marking polish/translation stale where columns exist.
- Time-boundary validation rules (documented): split lines keep ordering, no
  negative/zero crossing, end>start, zero-duration handled per the M15.4 policy
  (OneFrame/Skip config).
- Undo/redo per session for split/merge (an in-memory transaction log with a
  bounded depth is sufficient; document the bound + reset-on-close semantics).
- Keyboard shortcuts: platform-appropriate binding in the existing editor (mac:
  ⌘⇧S-ish per the Deck conventions — follow whatever the existing editor
  already uses; document the bindings in the report). Search results MUST stay
  consistent after edits (the LIKE scanner re-scans on next call — assert in
  tests that no stale index exists).
- Wire into the existing editor UI surface only (no new screens).

Verify (the slice's gate): boundary splits, merges, undo/reopen (fresh app
session = no partially-applied transaction), unchanged unrelated segments,
search consistency after each mutation. Plus the same gates: fmt/clippy/
workspace tests (baseline 619, grow; run the FULL suite only at the end).

## Slice 2 — M15.5 Small glossary and reversible text rules · M

- Glossary (bounded; document a sane default cap, e.g. 64 terms) prompt terms
  appended to the Whisper initial prompt within the actual context budget
  (~224 tokens for the integrated model — measure the tokenizer's real cap at
  runtime, never hardcode blind); visible truncation feedback (count of
  dropped terms) when the budget exactly.
- Separate deterministic post-correction dictionary (reversible: applied AFTER
  inference into a derived layer that is always re-applicable; source text
  NEVER rewritten — builds on M15-A's source_text invariant).
- Optional Indonesian normalization rules: conservative, opt-in, reversible;
  word-boundary rules only; code-switching respected (do not "fix" inside
  foreign tokens); "kayak"-class words preserved when semantically meaningful
  (rule = never apply when the match is adjacent to a determiner/clause-final
  marker without a rule entry — the test encodes the fixture from the plan).
- Clean/verbatim export toggle: conservative cleanup PREVIEWED (a diff panel
  before apply), never unconditional; the preview never writes without an
  explicit user action (state machine covered in tests).
- Wire into the existing settings surface (Models/Editor settings), all data
  stored through the store (schema additive only, bump if a new table).

Verify (the slice's gate): prompt budgeting maths, word-boundary rules, EN+ID
code-switching fixtures, the "kayak" preservation case, clean-export preview +
explicit apply, dictionary reversibility (re-run yields the same result, the
inverse rule never double-corrects).

## Do NOT

- No engine changes (whisper-rs calls untouched), no new dependencies, no CI
  edits, no Android work, no `git commit`/`push`, no sudo. Disk discipline:
  crate-scoped tests during work; ONE full workspace test run at the end.
- Report: `docs/reports/M15b-transactions-glossary.md` with UI bindings
  documented, the token-budget measurement method, and honest boundaries.
