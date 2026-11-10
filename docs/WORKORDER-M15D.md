# WORKORDER — M15-D: alignment feasibility → playback UI (M15.6)

Plan source: `docs/PLAN-M15-M18.md` §3 M15.6 ("M + M" — the FIRST half is a
feasibility gate that may end in a documented stop). Tree: `main` at `304596c`
(M15-E merged, 779 tests). **This is the LAST M15 slice.** After it: v0.4.0
prep.

## Phase 1 — alignment feasibility (bounded, evidence-first)

- Establish what the SHIPPED whisper-rs binding actually exposes: token
  probabilities, token→word mapping, and whether `whisper_token_data` /
  per-token timestamps are recoverable through the current API surface (inspect
  the pinned whisper-rs version's source; cite lines). Map what IT gives onto
  word-level timing: start/end at token granularity, joined per word.
- Check the Parakeet side for an equivalent (it likely lacks it — state it).
- Deliverable: a measured position — "usable per-token timing: yes/no, with
  which calls, with what estimated error" + a small offline test against a
  real model run IF a local ggml model + local audio fixture exists in-repo
  (M15-A's Opus fixtures decode to PCM; the tiny model pin `be07e04…` is in
  the store path) — otherwise fixture-less unit coverage of the mapping code
  with hand-built token arrays, and the real-audio measurement marked owner/
  next-machine.
- **Feasibility gate**: if the binding cannot yield per-token timing, STOP —
  write the finding + the lowest-cost upstream option (patch/issue link, one
  paragraph), and mark M15.6's second half deferred. Do NOT fake tokens or
  interpolate segment boundaries to masquerade as alignment.

## Phase 2 — playback UI (only if Phase 1 passes)

- Persist the supported alignment (per-word start/end in a NEW additive table,
  gated by the Phase-1 call surface), regenerate/mark stale per the M15-B/E
  staleness machinery on any source mutation.
- Click-to-seek on a word (the M15-C pending-seek path already exists) +
  karaoke-style highlighting of the current word during playback per the Deck
  motion conventions (spring, reduced-motion removes only the spring, never
  the state), with an explicit fallback to segment seeking when alignment is
  missing/stale.
- Bounded evaluation: compute alignment-error stats over the reviewed EN+ID
  fixtures and PUBLISH the numbers in the report (no claims beyond them).

Verify (the gate): the Phase-1 evidence line-by-line (binding source cited),
Phase-2 staleness = M15-B/E conventions per mutation type, reduced-motion +
segment fallback paths, fmt/clippy/tests (baseline 779→grow; ONE full
workspace run at the end) — crate-scoped tests meanwhile.

## Do NOT

- No engine parameter changes that alter runtime inference behaviour beyond
  reading timing data; no new dependencies; no CI edits; no Android work;
  no git commit/push; no sudo. If ffmpeg exists on this VPS, it may generate
  a small speech fixture ONCE for the Phase-1 measurement (document the
  commands + sha256 like M15-A did).
- Report: `docs/reports/M15d-alignment.md` (binding evidence, error stats if
  any, the go/stop decision, honest boundaries).
