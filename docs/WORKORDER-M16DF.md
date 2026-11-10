# WORKORDER — M16.4-F: Silero VAD chunking — BOUNDED FEASIBILITY (may end STOP)

Plan source: `docs/PLAN-M15-M18.md` §4 M16.4 (size M, L-class uncertainty → the
plan's rule: **a feasibility workorder precedes production work**). Tree:
`main` post-`ceb79f0` (987 tests). The prize: pause-delimited chunked capture
for long recordings (no promise of cloud-style realtime).

## Scope (ANALYSIS + THROWAWAY PROBE, no production code)

1. **Candidate + license review**: Silero VAD v5 (ONNX, MIT licensed — verify
   the exact GitHub repo + license text, record the file's sha256 candidate);
   its ONNX runtime cost on THIS VPS via a pinned ONNX runtime choice that the
   repo could eventually bundle (sherpa-onnx already exists in the graph for
   the Parakeet feature — evaluate reusing THAT for the VAD probe before
   reaching for ort directly; declare the rust/ONNX binding decision with
   license + size notes).
2. **A measured probe** (host-only): run the VAD model over a real WAV fixture
   (the M15-A/M15.2 fixtures decode to PCM already; generate a longer one if
   needed — e.g. 60 s of speech+silence via the VPS ffmpeg, provenance
   documented like M15.2): measure per-chunk CPU time, latency, and the
   speech/pause boundaries' stability. The probe's shape = "chunked capture
   with 300 ms pauses → Whisper over 10–30 s chunks" (the plan's shape, NOT a
   cloud-parity promise).
3. **Integration argument**: whether the M16.1 queue + whisper pipeline can
   accept chunked input with overlap-context join without the correctness
   risks (dropped/duplicated text at joins — the plan's own stated gate).
   Reference past evidence: the M7 live-transcription skip-and-merge approach
   ALREADY solved overlapping-window joins — compare honestly.
4. **CPU budget for phones** (the owner's 4 GB floor): the VAD's per-second
   CPU cost and whether it fits under a transcription's share.
5. **Decision block**: GO (with the integration workorder's exact slice list)
   or STOP (with the reason + the cheapest re-visit condition). NO production
   capture-path code in this run.

## Verification

- All numbers measured on this VPS (2 cores) + stated machine; reused fixtures
  carry provenance; the probe crate lives in a throwaway dir (delete or
  feature-gate before merge, state which).
- fmt/clippy -D warnings; workspace tests must stay at 987/0/10 (nothing lands
  beyond docs + reports; a dev-only probe crate is allowed if feature-gated
  OFF by default and clippy-clean).
- Report: `docs/reports/M16d-vad-feasibility.md` with the GO/STOP decision
  first.

## Do NOT

- No production capture-path changes; no new product dependencies; no CI
  edits; no Android work; no git commit/push; no sudo.
