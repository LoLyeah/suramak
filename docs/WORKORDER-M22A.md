# WORKORDER — M22A: a queue that survives the OS (checkpoint + resume)

Roadmap: `docs/reports/M21-roadmap.md` #1 (Carbon high). Today the M16.1
queue is in-memory: an OS kill mid-lecture restarts from zero. This slice
makes the queue durable and resumable, using the existing SQLite/WAL store —
no new dependencies.

## Deliverables

1. **A job journal** (additive schema, next version): per job — id, path,
   model, status (queued/decoding/transcribing/done/failed/cancelled),
   created/updated, and `last_committed_segment` (index + ms) per transcript.
2. **Segment checkpointing**: during transcription, every N segments (or every
   ~5 s of audio decoded) the worker commits the segments produced so far to
   the transcript row (the same store call the final write uses) and updates
   the journal in one transaction. A crash never leaves a transcript with
   segments past the journal.
3. **Resume on launch**: the app reads the journal; any job in
   decoding/transcribing resumes from `last_committed_segment` — decode may
   restart (cheap), transcription continues at that segment boundary by
   re-feeding the model the remaining window (never duplicating committed
   text, never skipping uncommitted audio). A "resumed" marker lands on the
   row and one logcat/log line says so.
4. **Honesty**: a job whose source file disappeared or whose model is gone
   becomes `failed(resume_unavailable: reason)` — never silently dropped.
5. **Cancellation** still rides the existing token; a cancelled job's partial
   segments stay (they are real) with status `cancelled`.

## Verification

- Unit tests: journal round-trip; checkpoint commits before journal (and the
  reverse on a simulated crash between the two — the transcript must never
  claim more than the journal); resume from segment k produces no duplicate
  and no missing text on a synthetic 30-segment job; missing-file/model paths
  fail with the named reason; cancel keeps partials.
- A host test that spawns the worker, kills it mid-job (SIGKILL in a child
  process), and a fresh worker resumes to the same final text as an
  uninterrupted run (byte-equal transcript).
- fmt/clippy -D warnings; NDK cross-check on both Android crates; workspace
  tests green (one full run at the end).
- Report: `docs/reports/M22a-durable-queue.md`.

## Do NOT

- No UI changes (the row's resume marker is a log + store field only); no new
  crates; no engine changes; no CI edits; no git commit/push; no sudo.
