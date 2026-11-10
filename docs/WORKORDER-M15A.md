# WORKORDER — M15-A: Workbench core slices (M15.1 + M15.2 + M15.4)

Plan source: `docs/PLAN-M15-M18.md` §3 (M15, ordered slices 1, 2, 4) — the three
size-S slices that carry no engine risk. Execute **in order 1 → 2 → 4**.
Milestone goal (M15): make imported transcripts easier to correct, search, and
export WITHOUT a resident inference service. Vector: a correct Mac test build
the owner runs (v0.4.0 lands when ALL of M15 is done — do not tag).

## Slice 1 — M15.1 Persistence and provenance audit · S

Deliverable: a schema/API inventory over the LIVE repo (no speculative paths):
- inventory in `docs/inventory/m15-persistence.md`: transcript source text and
  where it is written; segment mutations and their transaction coverage;
  derived text (polish/translation) storage + provenance fields; search
  indexing (what exists today, FTS5 or not); alignment metadata (none or
  partial?).
- audit result: which mutations can lose source text (grep the segment write
  paths) — fix any source-loss path that already exists with an ADDITIVE
  migration only if genuinely needed (v0.3 database → upgraded in place, no
  destructive change); otherwise record "no loss path; no migration needed" as
  the finding.

Verify (the slice's own gate): upgrade a populated SQLite fixture created from
the shipped v0.3 schema path (build it in-test via the store's own schema code,
not a binary committed blob), exercise backup/restore round-trip, assert
zero source-text loss, and record which search capabilities pre-exist.

## Slice 2 — M15.2 Import parity and shared decode reuse · S

- Deliverable: fixture parity suite — commit SMALL binary fixtures generated
  ONCE with the VPS's system ffmpeg (fixture GENERATION is tooling, not a
  product dependency; document the exact generator commands + sha256 pins in a
  fixtures README): `webm-opus`, `mkv-opus`, `raw-ogg-opus`, short (2–5 s),
  one EN + one Indonesian speech-like track per container where sensible.
- Add a decode-parity test: each fixture decodes through the SHIPPED path
  (symphonia + libopus adapter) identically by PCM hash after 48 kHz mono
  normalization; add malformed/truncated variants (prefix the same bytes cut
  short) where the shipped path must FAIL CLEANLY (named error, no panic) and
  cancellation mid-decode is tested at the existing cancellation seam.
- Document the shared-decode boundary: the Android M13 path must reuse the same
  decode code — write the one-paragraph interface contract in the inventory doc
  (M13-C will consume it). No new decoders; no ffmpeg in the product.

## Slice 3 — M15.4 Subtitle diagnostics and basic ASS · S

- Advisory CPL/CPS badges on segments: configurable thresholds (defaults CPL
  37/42 warning/error lines, CPS 17/20), Unicode/grapheme-aware counting
  (Indonesian + EN fixtures: long words, no spaceless glyphs used to cheat),
  documented counting rules in the module doc + the inventory doc.
- Basic ASS serializer from the current segment model (the same text the SRT/VTT
  paths read): styles header sane defaults, newline/multiline handling, zealous
  escaping ONLY where required by the spec, zero-duration cues rejected
  (downgraded to 1-frame or skipped per config, documented), playback verified
  in an INDEPENDENT reader — if no ASS reader exists locally, ship the fixture
  and assert round-trip via `cargo`-independent tooling where possible; a
  hand-rolled parser over our own output + a second export path comparison is
  the fallback (state which you did).
- Both wired into the existing export surface (no new UI flow; diagnostics
  visible where segments already render).

## Tests / verification gates

- `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D
  warnings`; `cargo test --workspace` (baseline 586 passed / 0 failed / 8
  ignored — grow it with the new suites; never regress).
- Every new fixture carries a README + sha256 pins + generator commands.
- No new dependencies unless a test-only dev-dependency is strictly required
  (format instructions + justification in the report if so).
- Docs touched: `docs/inventory/m15-persistence.md` (new) + fixture README.
- Report: `docs/reports/M15a-workbench-core.md` — per-slice honest boundaries,
  the search-capability findings, and the shared-decode contract paragraph.

## Do NOT

- No `git commit`/`push`; no sudo; no changes outside the listed code areas
  (fixtures are data, allowed); no CI edits; no new product dependencies; no
  Android work (M13-C consumes the contract later, not now); no destructive
  migrations; the four Mac-facing surfaces (record/transcribe/editor/library)
  must keep compiling and testing as-is.
