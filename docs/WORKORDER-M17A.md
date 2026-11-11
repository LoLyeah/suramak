# WORKORDER — M17A: local SLM runtime feasibility (M17.1 — one direction at a time)

Plan source: `docs/PLAN-M15-M18.md` §4 M17.1 (size L-class → bounded
feasibility, **STOP is a first-class outcome**). Tree: dispatches after M16F
lands (CPU contention discipline on this 2-core box). The prize: optional
OFFLINE refinement (polish/notes) without the endpoint — and a possible local
translation source for M15's bilingual export if the evaluation passes.

## Scope (ANALYSIS + THROWAWAY PROBE; no production code)

1. **One runtime direction**: **llama.cpp bindings** (`llama-cpp-2` or the
   FFI over the vendored llama.cpp — the same bundled-C pattern the repo
   already trusts for whisper.cpp/libopus). `candle` is the alternate —
   evaluate it ONLY if llama.cpp fails a gate, not both at once (the plan's
   "one direction at a time").
2. **Candidate model set** (small, per the plan): 2–3 B-class instruct models
   with permissive licenses that handle EN+ID. Candidates to review for
   license + EN/ID quality signals: Qwen2.5/Qwen3 1.7B–3B instruct (Apache-2.0
   — strong multilingual), Gemma-2/3 2B (Gemma terms), Phi-3.5-mini (MIT).
   Record: license text + size (q4_K_M quant), the HuggingFace sha256 of the
   exact file pinned.
3. **Measured probes on THIS VPS** (2 vCPU EPYC — the honest worst case for
   the desktop; the 4 GB Android verdict is a LATER gate, not this slice):
   - load time, tokens/s (gen) at 1/2/4 threads, RAM peak
   - EN+ID behavior on a fixed probe set: 10 short polish prompts (5 EN, 5
     ID) mirroring the endpoint's polish contract (correct transcript text;
     preserve names; no invented facts), scored by inspection + a
     hallucination count (the plan's own gate: hallucination rates on
     names/numbers/negation)
   - cancellation: abort mid-generation cleanly (the llama.cpp context
     abort path — measure the abort latency)
   - package-size delta of the runtime library
4. **The decision block**: GO (with the M17.2 production slice list) or STOP
   (with the reason + the re-visit condition — e.g. "no model under X GB
   meets the Indonesian register at Y tokens/s on 2 cores").

## Verification

- All numbers from named machines (this VPS) and pinned model files; the
  probe crates stay gitignored under build/ (the M16DF pattern); the probe
  models live in /tmp or ~/.hermes cache, NOT the repo.
- fmt/clippy -D warnings; workspace tests green (baseline untouched; a
  feature-gated throwaway probe only); no CI edits.
- Report: `docs/reports/M17a-slm-feasibility.md` — decision first, the probe
  table, the license review, the honest boundary (the 4 GB Android verdict
  and the owner-Mac measurements are later gates).

## Do NOT

- No production polish-path changes (the endpoint stays the default); no new
  committed dependencies; no CI edits; no Android work; no UI; no
  git commit/push; no sudo.
