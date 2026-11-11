# WORKORDER — M16H: offline diarization — feasibility + integration part 1

Plan source: `docs/PLAN-M15-M18.md` §4 M16.6 ("Diarization packaging gate,
inference, integration · S + M + M"): the packaging gate CLOSED with M16F
(sherpa dylibs bundled, gated, smoked). This workorder is the next piece:
**one validated offline diarization pipeline with pinned models** — bounded
first (the plan's L-class rule), production integration only after the
interface decision.

## Deliverables

1. **The pipeline candidate review** (one direction at a time): sherpa-onnx's
   own speaker-diarization pipeline (the sherpa-rs binding already in the
   graph post-M16F — verify what its API exposes: segmentation + embedding +
   clustering, model file shapes/sizes, licenses) vs any alternative the
   repo could reuse without a second runtime. Decision by evidence: what the
   sherpa-rs 0.6.8 binding exposes for diarization TODAY (cite the crate
   source), the candidate model set (e.g. pyannote segmentation rev/models
   shipped by sherpa — record exact HF repo + file sha256s + license text),
   and whether EN+ID audio is covered (diarization is language-agnostic —
   voice clustering; state it).
2. **A host probe** (throwaway, gitignored under build/): run the candidate
   over a real multi-speaker fixture (generate once with the VPS ffmpeg —
   two+ synthetic voices interleaved, provenance documented like M15.2);
   measure: per-audio-second CPU, the speaker-turn boundaries' stability
   across 3 runs, and the speaker-count accuracy on a 2-speaker and a
   3-speaker fixture.
3. **The interface decision** (the slice's real deliverable): what
   `crates/suramak-core` would store (session-local speaker turns: transcript
   id, segment ranges, a speaker label + color), the additive schema shape,
   and how the M15 provenance rules apply (derived data; never overwrites the
   source text; manual correction is a first-class edit).
4. **Decision block**: GO (with the production integration workorder's slice
   list: schema + persistence + the editor's speaker badges/lanes per the
   plan's restrained form) or STOP/DEFER (reason + re-visit condition).

## Verification

- Numbers from named machines (this VPS, 2 cores), pinned model files, the
  fixtures' provenance documented (sha256 + generator commands).
- The probe crates stay gitignored under build/ (the M16DF pattern); the
  model files live in /tmp or ~/.hermes cache, never the repo.
- fmt/clippy -D warnings; workspace tests green (baseline untouched);
  no CI edits; no Android work.
- Report: `docs/reports/M16h-diarization-feasibility.md` — decision first.

## Do NOT

- No production schema/persistence changes yet (the interface decision
  DESCRIBES them; the integration workorder implements); no UI; no new
  committed dependencies; no CI edits; no git commit/push; no sudo.
