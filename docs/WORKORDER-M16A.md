# WORKORDER — M16.1: background job lifecycle (bounded multi-file queue)

Plan source: `docs/PLAN-M15-M18.md` §4 M16.1 (size M). Tree: `main` v0.4.0
(`de37dca`, 807 tests, all gates green). M16's goal: capture + speakers without
entangling permissions with diarization readiness. The queue is slice 1 — its
worker conventions are what capture slices land on top of.

## Deliverable

**A bounded multi-file transcription job queue** replacing the current
one-job-at-a-time seams where applicable in the UI + CLI:

- Per-item state machine: `queued → decoding → transcribing → done /
  failed(named) / cancelled`; states persisted IN MEMORY only (the store keeps
  the already-existing per-transcript job row semantics — a queued item becomes
  an ordinary transcript row exactly when its transcription begins — the
  current behavior for a single job).
- **Bounded inference concurrency: exactly 1 running transcription at a time**
  (the model worker already serializes; document that the queue's single-slot
  inference is intentional — decode/import may run concurrently, bounded**
  e.g. ≤2 decodes ahead**, memory bounds documented).
- Cancellation semantics per slice: cancel a queued item = remove it; cancel
  the RUNNING item = the existing job cancellation seam (ESc + the Cancel
  control already exist in the editor/job card); queue does not wedge when the
  running job is cancelled from anywhere (tests assert this).
- Shutdown behavior: on app close mid-queue, the queued-but-not-started items
  are LOST silently unless the queue's order is durable in the session — keep
  it in-memory and drop with a visible note before refocus (document); the
  already-recorded/started ones behave like today's Stop & Save path.
- Error handling: a failed item moves to `failed(named)` and the queue continues
  with the next one — no cascade; the per-item reason surfaces in the job card
  (the existing UI surface).
- UI: the job card grows a compact queue list (≥1 item pending shows the list;
  drag-free reorder via up/down controls; bounded visible rows with a "+n
  more" line). CLI: `suramak-cli transcribe` accepts multiple paths, enqueues
  in order, prints per-file completion lines, `--cancel` cancels by position
  (documented; the one-slot inference is CLI-visible).
- Motion conventions: the queue rows follow the Deck tokens (no continuous
  idle redraw; the existing repaint budget rules apply — the job-card updates
  ride the existing event-driven frames).

## Tests (the gate)

- Mixed success/failure imports; cancel-queued; cancel-running mid-decode and
  mid-transcribe; app-close with items queued; bounded decode-ahead (no
  unbounded buffer growth); the 1-slot inference invariant (two transcription
  activities never run concurrently — asserted); queue continues after a
  failure; UI interactions cancel/raise as specified.
- fmt/clippy workspace -D warnings/tests (baseline 807; grow; ONE full
  workspace run at the end; crate-scoped tests meanwhile; disk: clean if <
  10 GB).
- **Honest boundary**: the queue's UX is judged on the owner's Mac pass (no
  audio device here for real recordings); the queue logic itself is covered by
  tests.

## Do NOT

- No engine changes, no new STT features, no `suramak-taps` work, no new
  dependencies, no CI edits, no Android work; do not touch the refusing
  surfaces outside the queue's scope (capture/HUD/hotkeys are SEPARATE slices);
  tests must keep every M15 surface green (split/merge, FTS, bilingual).
- No git commit/push, no sudo. Report: `docs/reports/M16a-job-queue.md`.
