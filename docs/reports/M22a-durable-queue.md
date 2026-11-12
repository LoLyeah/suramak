# M22-A — a queue that survives the OS: segment checkpoints, journal, resume

Status: complete, gates green. Scope from `docs/WORKORDER-M22A.md`; roadmap
`docs/reports/M21-roadmap.md` #1. No app version bump, no commit, no new crate,
no engine/STT change, no CI edit, no UI behaviour change.

## Result in one line

The queue is now durable: a `jobs` journal (schema **v8**, additive) records every
accepted file and the segment prefix it has durably committed; whisper streams
each finalized segment out of the decoder so the worker can checkpoint **during**
a long transcription (`≤ 4` segments or `~5 s` of audio); the transcript write and
the journal update share **one SQLite transaction**; a relaunch reads the journal
and continues at `last_committed_segment` by re-feeding the model only the
remaining window; missing inputs become `failed(resume_unavailable: …)`; a cancel
keeps its real partial text. The crash proof is a child process killed with
`SIGKILL` mid-job that resumes in a fresh process to a **byte-equal** transcript.

## The contract

### Schema v8 — `jobs` (additive)

| column | meaning |
|---|---|
| `id TEXT PRIMARY KEY` | job id (UUID), stable across the crash |
| `path TEXT` | the exact source path the job was accepted with |
| `model TEXT` | the exact model path the job ran with |
| `status TEXT` | `queued` / `decoding` / `transcribing` / `done` / `failed` / `cancelled` |
| `created_at` / `updated_at TEXT` | RFC 3339; `created_at` never moves |
| `transcript_id TEXT` | the row the committed prefix lives in (NULL before the first checkpoint) |
| `last_committed_segment INTEGER` | **count** of segments durably committed (the next `idx` is exactly this number) |
| `last_committed_ms INTEGER` | `end_ms` of the last committed segment (0 when none) — the audio boundary a resume re-feeds from |
| `resumed INTEGER` | the resume marker (set when a fresh process continues the row) |
| `failure TEXT` | the named failure (`resume_unavailable: …` for a refused resume) |

Indexes on `status` and `path`. `upgrade_to_v8` is one additive transaction; a
fresh database already has the table from the `init` DDL, and a v7 library keeps
every row. No existing table is rebuilt.

`last_committed_segment` is a **count**, not the last index: the next segment's
`idx` is the count itself, which removes the empty-job/one-segment off-by-one a
resume would otherwise have to guess at. Both columns are updated together.

### Checkpointing

`crates/suramak-asr/src/queue/journal.rs`:

* `CheckpointPolicy { every_segments: 4, every_ms: 5_000 }` — a checkpoint is due
  after **4 finalized segments or ~5 s of produced audio**, whichever comes first
  (`CheckpointPolicy::due`; pure, unit-tested).
* `CheckpointWriter` accumulates the job's segments (seeded with the committed
  prefix on a resume) and, when due, commits the **whole prefix** through
  `Store::commit_job_checkpoint`.
* `Store::commit_job_checkpoint(job_id, &transcript)` is the whole rule in one
  place: a single `unchecked_transaction` that
  1. writes the transcript header (first checkpoint only) and the segment prefix
     with the same `write_segments` the final save uses,
  2. updates `jobs.transcript_id`, `last_committed_segment`,
     `last_committed_ms` and flips the status to `transcribing`,
  3. commits.

  Statement order inside the transaction is segments → journal. The journal count
  is monotonic: a checkpoint that would move it backwards is refused rather than
  allowed to delete committed text.

Where the segments come from: `whisper::transcribe` now installs whisper.cpp's
**new-segment callback** (`set_new_segment_callback`) with a stack-local
`SegmentContext`, exactly like the existing progress/abort trampolines. Each
finalized segment is read out of the whisper state and sent to the worker as
`WorkerEvent::Checkpoint`; the driver feeds `CheckpointWriter` and owns the only
SQLite connection. The callback never touches the store (no I/O inside
whisper's decoding loop) and never unwinds.

For Parakeet and SpeechTranscriber the engine hands back the whole clip at once;
those jobs still stream nothing mid-run, are checkpointed only at their end, and
a resume simply re-runs the clip (the full segment set replaces the prefix — no
duplicates, no missing text, but no mid-run durability either). This is stated
rather than papered over.

### The ordering rule and the crash-between-writes case

A crash can never leave a transcript that *reads* as longer than its journal:

* **Normal path** — segments and journal are one transaction, so they land
  together or not at all.
* **Simulated torn write, dangerous direction** (segments committed, journal not):
  `Store::open` runs `recover_job_checkpoints`. For every resumable job it reads
  the real contiguous prefix and compares it to `last_committed_segment`. If the
  transcript is longer, the unacknowledged tail is **deleted** (through
  `write_segments`, so the search index follows) before any reader can see it.
  After that, transcript ≤ journal, and a resume cannot duplicate text.
* **Simulated torn write, reverse direction** (journal committed, segments not):
  the journal count is **rewound** to the prefix that actually exists, and
  `last_committed_ms` is taken from those rows. The resume then re-feeds that
  audio instead of skipping it — no missing text.

Both directions are pinned by store tests that commit each half on its own, drop
the connection, and re-open the file.

### Resume

`QueueDriver::resume_pending()` reads every `decoding`/`transcribing` row
(oldest first) and for each:

| case | result |
|---|---|
| source file gone | `failed(resume_unavailable: the source file is gone (…))` — the row stays, the reason is readable |
| model gone | `failed(resume_unavailable: the model is gone (…))` |
| different model than this launch | left **pending**, reported as skipped (a later launch with the right model can continue it; two models are never spliced) |
| committed prefix present | enqueued; the row gets `resumed = 1` and one `tracing::info!` line (`resuming interrupted job at its last committed segment`, with `from_ms`/`next_idx`) |
| nothing committed | enqueued and runs from zero under the same row identity |

`plan_resume` also refuses a journal that claims a prefix the library does not
have (`the committed text is missing …`) and a zero-ms boundary that would
re-feed the whole file — both named, never guessed.

The pipeline (`pipeline::transcribe_file`) takes a `CheckpointRun { committed,
from_ms, next_idx, on_segment }`:

* it slices the decoded 16 kHz buffer at `from_ms * 16` samples and hands only the
  tail to whisper; every produced timestamp is offset by `from_ms`, every index by
  `next_idx`;
* it returns `committed ++ tail`, so the worker's `Finished` transcript is the
  complete job text;
* an `initial_prompt` is **not** replayed on a mid-file resume (a start-of-file
  hint invites the model to re-emit committed words);
* when the crash landed after the last segment (`from_ms` at or past the end),
  the engine is not loaded at all: the committed prefix is the transcript.

Word timing (M15.6) for a resumed job covers the segments produced after the
resume; the committed prefix keeps no word rows, so karaoke falls back to segment
seeking there. Honest, and recorded here rather than silently fabricated.

### Cancellation

Unchanged seam: the token is set, the worker answers `Cancelled` at its next
stage boundary. The driver now **flushes the writer's pending segments first**,
then sets the row to `cancelled`. A cancelled job's partial segments stay in the
library (`jobs.last_committed_segment` matches the rows) and the row is not
resumable. A checkpoint that cannot be written is not swallowed: it stops the
job and the terminal event is a `failed(checkpoint: …)`, never a fake `done`.

## Wiring

* `QueueDriver::with_journal(store)` — additive builder; `new()` is byte-for-byte
  today's in-memory behaviour (all existing queue/driver tests unchanged). The
  driver is the journal's **single writer**: queued rows at enqueue, the
  checkpoint transaction on the worker's stream, terminal statuses.
* `QueueDriver::enqueue` reuses a pending row for the same path+model (a relaunch
  continues the job instead of twinning it), but never lets two live items share
  one row.
* CLI `suramak-cli transcribe … --db`: opens the library once, attaches it to the
  driver, calls `resume_pending()` before the arguments are queued, prints one
  line per resume/refusal, and skips the old per-file `insert_transcript` because
  the driver already wrote the transcript + words + `done` row in one
  transaction. `--db` remains opt-in; without it nothing is journaled.
* `WorkerEvent::Checkpoint` and `QueueEvent::Resumed` are new. The desktop
  runtime matches `Checkpoint` (its queue is in-memory, so the event is a no-op
  there) and the Android host matches `Resumed` (it keeps its own job journal).
  Those arms are compile-only integration — no UI text, layout or flow changed.

## Verification

| gate | command | result |
|---|---|---|
| store journal unit tests | `cargo test -p suramak-core --lib store::tests` | **54 passed** (9 new: round-trip, checkpoint transaction, monotonic count, both torn-write directions, cancel keeps partials, named refusal, finish+words, v7→v8 upgrade) |
| checkpoint/resume unit tests | `cargo test -p suramak-asr --lib queue::journal::tests` | **6 passed** (policy, 30-segment resume from 12, missing source, missing model, named prefix, cancel flush) |
| driver journal tests | `cargo test -p suramak-asr --lib queue::driver::tests` | **11 passed** (4 new: done row, cancel partials, resume boundary + marker, launch refusal) |
| **SIGKILL crash proof** | `cargo test -p suramak-asr --test durable_resume` | **2 passed** — a child process transcribes 30 synthetic segments at 40 ms/segment, is `SIGKILL`ed after ≥ 4 committed segments, and a fresh child resumes from the journal. Asserted: the interrupted transcript has exactly `last_committed_segment` rows (never more); the resumed row is `done` with `resumed = 1`; the final segment JSON is **byte-equal** to an uninterrupted run's; indices are one contiguous 0..29 run. Repeated 3×, stable. |
| real-model checkpoint path (ignored e2e) | `cargo test -p suramak-asr --test pipeline -- --ignored e2e_checkpoint_stream_and_resume_match_the_full_run` | **1 passed** (tiny model, debug): the mid-run stream equals the final segment list; a resume at the first segment's `end_ms` preserves the prefix byte-for-byte, keeps indices contiguous, and the resumed stream equals the tail with offset indices and absolute times. |
| fmt | `rustfmt --edition 2021 <changed files>` | clean |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings` (and the same on `-p suramak-core -p suramak-asr -p suramak-cli`) | clean, exit 0 |
| NDK cross-check | `cargo ndk -t arm64-v8a --platform 29 check -p suramak-android --locked` and `cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-android -p suramak-android-spike --all-targets -- -D warnings` | both clean (exit 0) |
| workspace tests | `cargo test --workspace` | **exit 0 — 1280 passed, 0 failed** (one full run at the end; the count includes M22B's concurrent tests in the same tree) |

## What this slice deliberately does not do

* **The desktop UI's queue and the Android host do not call `resume_pending` yet.**
  The desktop queue is in-memory by design and the Android service keeps its own
  JSON job journal (`crates/suramak-android/src/jobs.rs`); adopting the store
  journal there is a behaviour change in those hosts, and this order forbids UI
  changes. The launch entry point is implemented, tested and wired in the CLI, so
  a host only has to call `driver.resume_pending()`.
* **No cross-process writer guard.** The driver is the single writer *within* a
  process; two processes writing one library is already outside the store's
  contract. `recover_job_checkpoints` only touches resumable rows and can only
  observe committed states.
* **Queued rows from a killed process are re-bound when the same path is
  enqueued again**, but `resume_pending` does not resurrect them on its own
  (the order resumes `decoding`/`transcribing`; a queued job has no text).
* **Word timing across a resume boundary** is absent for the committed prefix,
  as described above.

## Files

* `crates/suramak-core/src/store.rs` — schema v8 (`jobs`), `JobStatus`,
  `JobRecord`, `insert_job`, `job`, `jobs`, `resumable_jobs`, `resumable_job_for`,
  `pending_job_for`, `set_job_status`, `mark_job_resumed`,
  `fail_job_unavailable`, `commit_job_checkpoint`, `finish_job`,
  `committed_segments`, `recover_job_checkpoints`; 9 tests.
* `crates/suramak-core/src/lib.rs` — re-export `chrono::{DateTime, Utc}` for the
  store's public journal types (no new dependency edge).
* `crates/suramak-core/tests/persistence.rs` — the v1→current upgrade guard now
  reaches v8.
* `crates/suramak-asr/src/queue/journal.rs` — `CheckpointPolicy`,
  `CheckpointWriter`, `DurableJob`, `Resume`, `ResumePlan`, `plan_resume`; 6 tests.
* `crates/suramak-asr/src/queue/driver.rs` — journal ownership, enqueue rows,
  `resume_pending`, `begin_durable`, checkpoint commits, terminal statuses,
  `QueueEvent::Resumed`; 4 new tests.
* `crates/suramak-asr/src/whisper.rs` — the new-segment callback,
  `from_ms`/`next_idx` offsets and word-span shifting. `worker.rs` — `Job.durable`,
  `WorkerEvent::Checkpoint`, the checkpoint stream. `pipeline.rs` —
  `CheckpointRun`, tail slicing, prefix reassembly.
* `crates/suramak-asr/tests/durable_resume.rs` — the SIGKILL parent/child test.
* `crates/suramak-asr/tests/pipeline.rs` — the ignored real-model checkpoint e2e;
  the existing call sites pass `None` for the new checkpoint stream.
* `crates/suramak-cli/src/cmd/transcribe.rs` — `--db` attaches the journal,
  launch resume, resume line, no double insert.
* `crates/suramak-android/src/jobs.rs`, `crates/suramak-ui/src/desktop/{app/mod.rs,
  runtime.rs}` — one match arm / `durable: None` field each, required by the new
  event and job field; no behaviour change.
* `crates/suramak-ui/src/deck/compact/chrome.rs` — one `#[allow(clippy::too_many_arguments)]`
  on the existing ten-argument dock builder, required for the workspace `-D
  warnings` gate; no code change.

## Evidence: the crash proof in detail

`tests/durable_resume.rs` spawns its own test binary
(`current_exe --exact sigkill_child_entry`) with a mode environment. The child
runs the production `QueueDriver` with a deterministic fixture engine that
produces `segment NN` at `[NN s, NN+1 s)` from absolute audio time, so a resume
that starts at the wrong millisecond or index changes the bytes. The parent:

1. runs the child to completion in `reference.db`;
2. runs it at 40 ms/segment in `crash.db` and polls the journal until
   `last_committed_segment >= 4`, then `Child::kill()` (SIGKILL on Unix — no
   destructors, no clean WAL close);
3. re-opens `crash.db` and asserts the transcript has exactly
   `last_committed_segment` rows (the ordering rule, observed from a fresh
   process);
4. runs the child again in `resume` mode: it calls `resume_pending`, gets a
   `Resumed { from_ms, next_idx }`, and finishes;
5. compares `serde_json` segment bytes with the reference run — equal — and
   checks `done` / `resumed = 1` / 30 contiguous segments.

The run completed in ~0.25 s wall clock and passed three consecutive times on
this 2-core host.
