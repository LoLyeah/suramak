# WORKORDER-HOTFIX-QUIT — clean quit (no ggml-metal abort on exit)

Base: master post-M7 (619aa0d). Owner evidence: run11 log + macOS behavior
(user's MacBook Air M4, macOS 27).

## The bug

Quitting the app (Cmd-Q / menu Quit / window close) **after any whisper
session aborts** with:

```
GGML_ASSERT([rsets->data count] == 0) failed   (ggml-metal-device.m:608)
ggml_metal_device_free ← __cxa_finalize_ranges ← exit ← [NSApplication terminate:]
zsh: abort
```

The window is already gone when it fires, so it's cosmetic-ish — but it
produces "zsh: abort" + crash-report noise and it must go. Root cause: the
process exits by normal `main` return → C static destructors run → ggml's
global Metal-device destructor asserts because whisper contexts (and their
Metal residency sets) are still alive; contexts are owned by worker/bridge
threads and nothing frees them before exit. This is a known ggml-metal
teardown class (ggml's own message cites llama.cpp#17869).

## Goal

Quitting is **clean and silent in every path** (menu quit, Cmd-Q, window
close), after any amount of whisper / live-transcription / playback activity,
including quitting **while recording** or while a job is running.

## Approach (constraints, not a full spec)

- All quit flavors funnel through the winit event loop returning; add an
  explicit shutdown step in `crates/suramak-ui/src/main.rs` after the loop:
  best-effort teardown (stop capture/live/playback if active via the existing
  teardown paths — never block, never panic; stop workers where quick so
  contexts drop), flush logs, then a **hard `std::process::exit(0)`** so the
  fragile ggml static destructor never runs. Comment WHY at the exit (ggml
  teardown assert; hard exit is the established pattern in this codebase —
  the smoke paths and the single-instance guard already exit this way).
- Bounded grace only (≤ ~250 ms): if a worker thread owns a context and cannot
  be joined quickly, the hard exit still covers it — do not contort for a full
  join.
- Smoke/exit codes: unchanged (they end inside the loop with their own codes).
- Minor piggyback IF cleanly separable: capture stop currently prints
  `audio capture error: The requested device is no longer available …` on
  every Stop (teardown race, benign, seen in run11 three times). Downgrade
  just the shutdown-time variant (debug-level / annotated), without hiding
  genuine mid-recording stream errors. Skip + document if it risks capture
  semantics.

## Tests / docs

- Keep everything headless green; add teardown unit tests where feasible
  (e.g. teardown is idempotent, no-op when nothing runs).
- `docs/USER_SMOKE.md` item 19: record ~5 s, Stop, quit (Cmd-Q) — expected:
  the app vanishes with **no** `zsh: abort` and no ggml assert in the
  terminal; also quit *while recording* and quit while a job runs.
- `CHANGELOG.md` [Unreleased] Fixed; `docs/PLAN-NOTES.md` known-issues entry
  (ggml-metal teardown assert + our hard-exit rationale);
  `docs/reports/HOTFIX-QUIT.md`.

## Acceptance (from /home/ubuntu/suramak; exact outputs in the report)

1. `cargo fmt --all --check` — clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` — clean.
3. `cargo test --workspace` — all green (+ any new tests).
4. xvfb smoke — ok (plus one smoke run exercising the quit path if the smoke
   harness can; otherwise state that the quit path is mac-verified by the
   owner).
5. No new dependencies.

## Constraints

- Only modify files inside /home/ubuntu/suramak. No sudo. No git commit/push.
- Do not touch `suramak-glass`, `suramak-playback`, `suramak-taps`, the winit
  fork, `scripts/`, or `crates/suramak-ui/src/instance.rs`.
- The `capture` feature stays OFF on Linux; do not restructure capture
  semantics for the log-noise item.
