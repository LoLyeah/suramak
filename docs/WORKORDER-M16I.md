# WORKORDER — M16I: diarization production integration (schema v7 + badges/lanes)

Plan source: `docs/reports/M16h-diarization-feasibility.md` (the interface
decision) + `docs/reports/M16h0-diarization-slice0.md` (slice-0 gate PASSED —
real EN+ID multi-speaker: 2-spk DER 3.29% 2/2, rapid 3-way DER 2.11% 3/3 vs the
20.64% English collapse, 3/3 deterministic). Tree: `main` post-`ecae588`. The
plan's restrained form is the contract: session-local clustering, manual
correction first-class, no identity claims.

## Deliverables

1. **Schema v7 additive** (per the M16H interface decision):
   - `diarization_runs` (transcript id, model + version + file sha256, created
     at, status);
   - `speakers` (run id, label like "Speaker 1", color index, an optional
     user-set name — the manual correction entry);
   - `speaker_turns` (run id, speaker id, start/end ms — time-anchored;
     segment ranges are DERIVED on read, never stored, per the decision).
   - M15 provenance rules: derived data; deleting a run restores the
     pre-diarization view; the source text and segments are never touched.
2. **The runner**: diarization executes as a JOB (the M16.1 queue discipline —
   one slot, the same cancellation seam; note the M16H finding: the binding
   has NO mid-run abort, so cancel = discard the run's partial state and stop
   at the next await point; document the contract). The model files load via
   the M16F-packaged dylibs (bundled on mac; Android/Windows/Linux follow the
   per-platform packaging when each ships — a platform without the dylibs
   shows the feature as unavailable, never silently absent).
3. **The editor surface** (restrained, per the plan): speaker badges on
   segment rows (color + label), ONE 8 px speaker lane above the waveform
   strip; rename-in-place on the badge (a user-set name replaces the label
   everywhere in the run); "Delete diarization" restores the plain view.
   Colors: the Deck palette's first N hues, fixed per label index.
4. **Staleness**: an edit/split/merge marks the run stale (the M15-B/E
   machinery) — the lane/badges dim with a re-run affordance; the source
   segments/text are never modified by diarization.
5. **A11y + motion**: the lane is decorative-but-labeled; badges carry labels
   + focus rings; no idle redraws (the M16.1 frame-budget rules).

## Verification

- Schema v6→v7 additive; upgrade-in-place from every populated fixture loses
  nothing (the M15-A discipline); corrupt/newer-schema refusals.
- Unit tests: turn persistence round-trip, rename propagation, stale
  propagation on edit/split/merge, delete-restores, the runner's cancel
  contract (discard-at-await), the platform-without-dylibs unavailable state.
- The editor surface: desktop golden shots for the badges + lane; the phone
  inherits via the shared Deck components (no separate Android work — the
  compact editor reuses them).
- fmt/clippy -D warnings; workspace tests (baseline 1079 → grow; ONE full run
  at the end; crate-scoped tests meanwhile; disk: clean if < 10 GB). Keep
  every M15 suite green.
- **Honest boundary**: real-meeting audio quality is the owner-device pass
  (the M16H/S0 reports' named residual); 3+ speakers stay provisional and
  manually correctable by design.
- Report: `docs/reports/M16i-diarization-integration.md`.

## Do NOT

- No engine/whisper changes; no cross-session identity or voice matching
  (the plan forbids it); no Android-specific code; no CI edits; no new
  dependencies (sherpa-rs already bundled); no git commit/push; no sudo.
