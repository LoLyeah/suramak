# WORKORDER — M14B2: lock-free audio capture (SUR-HIGH-03)

Tree: current `main` (post-`ad61cb9`). **Read first:** `docs/reports/AGY-AUDIT-2026-10-21.md` §SUR-HIGH-03 (the finding + suggested shape), `crates/suramak-asr/src/capture.rs` (whole file — the callback path, `accumulate`, `RollingRms`, `take_live_chunk`, and the stop/finalize handoff), and the M7 live-decode wiring in `crates/suramak-asr/src/live.rs` that consumes the chunks.

## Goal

The OS audio callback runs on a real-time thread. Today it takes an OS mutex (`std::sync::Mutex`) and heap-allocates (`mono_mixdown`'s `Vec`, `buffer.push` reallocation) on every delivery — the classic priority-inversion/dropout pattern. Make the callback **lock-free and allocation-free**: an SPSC ring buffer between the RT callback and a non-RT consumer thread that owns the shared state.

## Scope

1. **Add `rtrb` (pinned version) to `suramak-asr`** (or `ringbuf` — pick one, justify in the report; the API must allow the producer to try-push and never block).
2. **Callback path becomes:** mono-mixdown **into a stack buffer or iterator without heap allocation** (rewrite `mono_mixdown` as an iterator adapter or fixed-capacity stack buffer sized from `data.len()`), then `try_push` each sample into the SPSC ring. No locks, no allocation, no syscalls. If the ring is full, **drop the incoming samples** and increment an atomic drop counter (drop-oldest on the consumer side is NOT possible lock-free; document the chosen policy — with the ring sized generously (≥ 1 s of samples), drops only happen if the consumer thread is dead, which is already a fatal state).
3. **A consumer thread** (owned by the capture session) drains the ring and does what `accumulate` did: push into the shared `Vec<f32>` (under the existing mutex, now only touched by non-RT threads), update `RollingRms`, service `take_live_chunk` boundaries exactly as today. The consumer must keep the **same observable behavior**: `rms_bits` updates, live chunks, and the final buffer on stop.
4. **Ring sizing:** at the session's sample rate, ≥ 1 second of mono samples (e.g. 48 000 for 48 kHz); document the arithmetic. Ring creation must happen at session start (allocation is fine there — not on the RT thread).
5. **Tests (host-runnable, no device):**
   - a unit test that drives the new callback path directly (produce N buffers as the RT thread would, assert the consumer produces the identical sample sequence and the same rms values as the old implementation);
   - a test that overflows the ring (consumer paused) and asserts the drop counter increments and the callback still returns in bounded time (no blocking);
   - the existing capture tests must keep passing unchanged.
6. **Do NOT change:** device enumeration, the public `capture` API shape (the `CameraTaker`-style session handle may gain a private thread, but public signatures stay), resampling, the whisper/live pipeline, `keep_audio` semantics.
7. **Report** `docs/reports/M14b2-audio-rt.md`: the before/after callback path, why rtrb vs ringbuf, the drop policy, measured numbers (a stress test: consumer delayed randomly up to 50 ms, assert zero blocking in the producer — e.g. via a deadline check), and the honest boundary (unit-tested here; real-device dropout check = owner lane on macOS).

## Do NOT

- No `git commit`/`push`; no sudo; only files inside `/home/ubuntu/suramak`; no new dependencies beyond the chosen ring-buffer crate; no changes to `suramak-taps` (its tap path is macOS-internal and out of scope here).

## Verification

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (490 baseline + your new tests).
- `cargo test -p suramak-asr --features capture,capture-test` still green.
- The audit finding's three complaints each have a code-level answer in the report (no mutex in callback / no allocation in callback / documented drop policy).
