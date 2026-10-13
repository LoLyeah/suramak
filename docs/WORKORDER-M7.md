# WORKORDER-M7 — Live transcription (partials while recording)

Base: master after M6. Order: … M5 → M6 → **M7** → M9 → M10.
Owner ask: *“why when the app recording, there’s no real time transcription? So
I only need to wait for the recording to end?”* — today the pipeline is
record-then-transcribe; M7 adds the live view.

## Goal

While recording, show a growing **provisional** transcript of the audio so
far. On Stop, the existing full-file pass ALWAYS runs and **replaces** the
provisional text with the final quality version (the final artifact stays
exactly what today's flow produces).

## Design

1. **Chunked partial decodes during capture** — a decoder worker consumes
   fixed windows of the incoming audio (e.g. every ~4–6 s of new audio, with
   a ~1–2 s context overlap) through the SAME whisper path (same model /
   language / initial prompt as the session) and appends provisional segments.
2. **Bounded cost / graceful degradation** (the PRODUCT.md gating question —
   engine cost during capture): the worker must never starve capture.
   Requirements: bounded queue with **drop-oldest**; a backlog guard — if the
   previous window's decode is still running when the next closes, **skip and
   merge** into the next window; a per-session count of skipped windows,
   logged (and surfaced subtly in the UI). Slower Macs may lag live updates —
   acceptable; final correctness is not affected.
3. **Multi-source (M6)**: live partials run on the **mixed** stream when
   several sources are active; single-source sessions behave as today.
4. **Where**: a new `live` module in `suramak-asr` — the scheduler / windowing
   / provisional-segment merging logic must be **pure and Linux-testable**
   (values in, values out; no devices, no cargo feature); decode rides the
   existing whisper session machinery. Capture-gated wiring stays minimal.
5. **UI (record view)**: a growing provisional transcript list with
   auto-scroll and a clear **“provisional — the final pass runs on Stop”**
   label; show the skipped-windows note when relevant; on Stop the view
   transitions to the existing job flow unchanged.

## Tests

- Scheduler (pure): window cadence + overlap; drop-oldest backlog behaviour;
  skip-and-merge rule; session bounds; merge of provisional segments.
- UI state: provisional content is ALWAYS discarded on Stop and the job flow
  takes over; skipped-counter surface.
- Linux E2E (ignored pattern, tiny model): feed a fixture through the live
  scheduler on a simulated clock — provisional segments appear — then run the
  full pass — the final replaces the provisional. Show the actual words.

## Docs

- `docs/USER_SMOKE.md` item 18: record speech on the Mac — words appear while
  recording (labeled provisional); on Stop the final transcript replaces it;
  live updates may lag on slower machines — that is fine.
- `README.md`, `CHANGELOG.md` [Unreleased], `docs/PLAN-NOTES.md` (the backlog
  policy), `docs/reports/M7.md`.

## Acceptance (from /home/ubuntu/suramak; report exact outputs)

1. `cargo fmt --all --check` — clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` — clean.
3. `cargo test --workspace` — all green + the new tests.
4. The Linux live E2E with its exact provisional and final words shown.
5. xvfb smoke — ok.
6. No new dependencies (`cargo tree` unchanged).

## Constraints

- Only modify files inside /home/ubuntu/suramak. No sudo. No git commit/push.
- Do not touch `suramak-glass`, `suramak-playback`, `suramak-taps`, the winit
  fork, `scripts/`, or `crates/suramak-ui/src/instance.rs`.
- The final (Stop) pass must remain byte-identical to today for the same
  input; provisional machinery must be fully skippable/free when disabled.
- No new dependencies. Linux capture stays OFF.
