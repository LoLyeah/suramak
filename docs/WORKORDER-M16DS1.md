# WORKORDER — M16.4-S1: VAD join-quality harness (the decision gate)

Plan source: `docs/reports/M16d-vad-feasibility.md` — production slice 1 of the
5-slice GO. **Pure gate, no capture, no UI.** Tree: `main` post-`6efedf7`
(v0.4.1 shipped; tests green across all five CI platforms).

## Deliverable

A **join-quality harness** that answers: does VAD-driven chunking lose or
duplicate words at the cut, versus the full-pass transcript?

1. **The chunker** (pure function, unit-tested): feed PCM 16 kHz mono through
   the Silero VAD (the model sha256 `2623a295…` from the M16DF report — host
   path; the binding is `sherpa-rs 0.6.8`'s `SileroVad` per the feasibility
   report's binding decision) fed EXACTLY 512 samples per call; emit chunks of
   ~10–14 s cut **116 ms after** the last voiced frame, with the chunk's
   overlap context joined per the M7 skip-and-merge conventions. Determinism:
   3 runs → identical chunks.
2. **The harness** (host test, whisper tiny/base on the review fixtures):
   for each of the three M16DF fixtures (EN + ID + the 60 s speech+silence
   one): (a) full-pass transcript = the authority; (b) chunked run =
   transcribe each chunk, join with the overlap trim, compare —
   **the gate: zero dropped phrases, ≤ 1 duplicated phrase, WER delta
   ≤ 1.0 pt** vs the full pass, on clean AND room-tone variants. Gate
   failure → the workorder's decision becomes STOP for chunk-only capture
   (record it; M7's cadence stays; production slices 2+ are cancelled).
3. **Where it lives**: `crates/suramak-asr/src/vad.rs` + a host test file; the
   feature is `vad-chunking`, default OFF, nothing in the capture path, no
   service change, no settings surface yet (slice 5's job).
4. **The dependency**: `sherpa-rs 0.6.8` becomes an OPTIONAL crate dependency
   gated by `vad-chunking` (license chain Apache-2.0/MIT already verified in
   M16DF; the model file itself is NOT committed — host tests locate it via
   `SURAMAK_VAD_MODEL_PATH` or skip, like the alignment test's model gate).

## Verification

- Determinism (3 identical runs), 512-sample feed invariant (the report's
  finding: 1 s blocks shift starts up to 960 ms), boundary margins held
  (cut ≥ 90 ms before the next voiced frame).
- The WER harness numbers PUBLISHED in the report (per fixture: EN/ID, clean/
  room tone) — the decision text: GO-on for slices 2–5 or STOP for chunk-only.
- fmt/clippy -D warnings (workspace incl. the feature-gated crate); workspace
  tests green (baseline per current main; ONE full run at the end);
  **no Android/CI changes**; the desktop capture path untouched.
- Report: `docs/reports/M16d1-vad-join-gate.md` (the numbers, the gate
  verdict, the harness's exact join rules, boundaries).

## Do NOT

- No capture-path code, no UI, no settings, no Android work, no CI edits;
  no `git commit`/`push`; no sudo. The `vad-chunking` feature OFF by default
  keeps every shipped surface byte-identical.
