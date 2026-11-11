# WORKORDER — M16H-S0: diarization slice 0 — real EN+ID multi-speaker gate

Plan source: `docs/reports/M16h-diarization-feasibility.md` — CONDITIONAL GO;
slice 0 is the report's own condition: "real EN+ID fixtures". Tree: `main`
post-`d4cf5f3` (1079 tests; the probe infrastructure from M16H is gitignored
under build/ and reusable).

## Deliverable

Re-run the M16H probe pipeline (the same harness, models, thresholds) against
**multi-speaker EN+ID fixtures that match the product's real languages**:

1. **Fixtures**: generate once with edge-tts (the repo's committed-fixture
   voice generator — the same quality class as `fixtures/audio/{en,id}.wav`),
   2-speaker and 3-speaker EN+ID variants: distinct neural voices, alternating
   turns, some rapid exchanges, some code-switching (an EN sentence inside an
   ID run and vice versa — diarization is language-agnostic, but the fixture
   must be the product's audio shape). Documented commands + sha256 + voice
   names (provenance like M15.2). Fixtures are small test-sets (≤ 2 MB each).
2. **The gate** (per fixture): DER + speaker-count accuracy + turn-boundary
   stability 3/3, same thresholds as M16H (the rapid-3-way 20.6% DER collapse
   is the number to beat; note where Indonesian voices land vs the M16H
   English-only TTS ceiling).
3. **The recommendation**: with the real-language evidence in, update the
   M16H decision to GO-for-production (with the integration workorder's slice
   list) or DEFER (what evidence is still missing — e.g. real meeting audio
   stays the owner's contribution at their next device pass).

## Verification

- Same discipline as M16H: pinned models, gitignored probe, fixtures'
  provenance documented, numbers machine-named.
- fmt/clippy -D warnings; workspace tests green (1079 baseline, untouched);
  no CI edits; no production changes; no git commit/push; no sudo.
- Report: `docs/reports/M16h0-diarization-slice0.md` — the gate table + the
  updated decision.
