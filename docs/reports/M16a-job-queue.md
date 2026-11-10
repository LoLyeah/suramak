# M16-A — Background job lifecycle: the bounded multi-file queue (M16.1)

Status: complete, gates green. Scope from `docs/WORKORDER-M16A.md`; parent plan
`docs/PLAN-M15-M18.md` §4 (M16.1). No app version bump, no commit, no new
dependency, no engine/STT change, no capture/HUD/hotkey work.

## Result in one line

A single in-memory queue now owns every accepted file — `queued → decoding →
transcribing → done / failed(named) / cancelled` — with exactly one inference
slot, at most two decodes ahead (96 MiB of decoded audio), the existing
cancellation seam wired from the card, Esc, the queue list and the CLI, failures
that never cascade, a compact bounded queue card with up/down/remove, a
quit-confirm that names the files quitting would drop, and a multi-path
`suramak-cli transcribe` with per-file completion lines and `--cancel`.

## Concurrency and memory contract

The shared implementation is `crates/suramak-asr/src/queue/`:

* `queue::JobQueue` — the pure state machine (no threads, no clocks). All rules
  below are pinned by its unit tests.
* `queue::driver::QueueDriver` — the production pump used by the CLI (and the
  template for capture slices): fills the slot, requests decode-ahead, maps
  backend events onto the machine.

| rule | value | why |
|---|---|---|
| running transcriptions | **exactly 1** | `JobQueue::next_inference` returns `None` while another item is running; the slot is released only by `done`/`failed`/`cancelled`. The whisper context cache stays single-owner. The CLI test backend asserts no two submits overlap; the UI bridge has one in-flight job by construction. |
| decode-ahead | **≤ 2 items** (`MAX_DECODE_AHEAD`) | decoding is CPU-only, allocates one 16 kHz mono `f32` buffer per item and never touches the engine context. The two slots count an item from "decode requested" until the inference slot takes it (so a decoded, waiting item still holds its slot); the item the engine is transcribing is outside the count. At most three sample buffers exist at any instant. |
| decoded memory | **≤ 96 MiB** (`MAX_PREFETCH_BYTES`) | 16 kHz mono `f32` = 64 KiB per audio second, so ~26 minutes of decoded audio across the pool. A buffer that would cross the cap is refused: the item returns to `queued` (marked "do not prefetch") and decodes inline in the worker — bounded memory, never a stuck queue. |
| decoded-but-waiting buffers | ≤ 2 items, ≤ 96 MiB | a buffer that arrived keeps its decode-ahead slot until the inference slot takes it; the byte cap bounds the actual memory. When the slot takes the item, the buffer moves into the job and leaves the pool (its slot frees, and the next decode may start). |
| queue rows visible | **4** + `+n more` | `app::transcribe::QUEUE_VISIBLE_ROWS`; fixed row geometry, no scroll area. |

The UI path is **stricter** than the shared bound: it has no decode-ahead pool,
so its queue is 0 decodes ahead and 1 inference (the bridge's job loop). The
decode-ahead machinery is exercised end-to-end by the CLI driver and remains the
reusable path for M16.4's capture pipeline. This is a ceiling, not a target.

Decoded-ahead buffers are dropped on cancellation, on over-budget admission and
when the slot consumes them; there is no unbounded staging buffer anywhere.

## Per-item state machine

| state | meaning | reached by |
|---|---|---|
| `Queued` | accepted, not started | `enqueue` |
| `Decoding { ready: false }` | a decode is in flight, or the worker is decoding inline | `next_decode`, inline `next_inference` |
| `Decoding { ready: true }` | decoded and waiting for the slot (a buffer is held) | `mark_decoded` |
| `Transcribing` | holds the single inference slot | prefetched `next_inference`, or the worker's `Stage::Transcribing` event |
| `Done` | transcript delivered (engine + warnings recorded) | `WorkerEvent::Finished` |
| `Failed { reason }` | named failure, shown in the card | decode failure, submit failure, `WorkerEvent::Error` |
| `Cancelled` | removed while queued, or the worker confirmed the running cancel | `cancel`, `WorkerEvent::Cancelled` |

States live **in memory only**. The store keeps today's per-transcript semantics:
nothing is written for a queued item, and a finished job saves its transcript
exactly as a single job does today (one `SaveTranscript` per `Finished`). The
queue never invents placeholder rows or a job table.

`WorkerEvent::Stage { Decoding | Transcribing }` is new and emitted by the shared
pipeline front half, so the card says "decoding the audio…" only while that is
true. A queue item with a decoded-ahead buffer goes straight to `Transcribing`;
the stage event still arrives and is a no-op for it.

## Cancellation

| where | what happens |
|---|---|
| Cancel control / Esc on the transcribe card | existing seam: `Effect::CancelJob` sets the job's `CancelToken`; the item stays `Transcribing` with `cancelling` set, the caption reads `cancelling…`, and the slot stays occupied until the worker answers `Cancelled`. A second press is inert. |
| queue row **Remove** (or `cancel_queued(id)`) | a waiting item is removed outright — its decoded buffer (if any) is dropped and it never reaches the engine. Cancelling the running row routes to the seam above. |
| `suramak-cli transcribe … --cancel N` | applies at the stable 1-based position: waiting files are removed before the batch starts; position 1 (the first to run) has its token set right after it takes the slot, and the worker stops at its next stage boundary. |
| worker answers `Cancelled` mid-batch | the item is terminal, the slot frees, and **the next item starts** — the queue never wedges (`cancelling_the_running_job_continues_the_queue`). |

`queue.next_inference` refuses while the slot is occupied even if the token was
already set, so a second engine run can never start on top of a half-stopped one.

## Failures

One item's failure is terminal for that item only: reason recorded on the item,
mirrored in `AppState::queue_failure`, and the next item starts immediately. The
card shows the last failure as `failed earlier: <name> — <reason>` while the next
job runs; when the batch drains, the existing single failure state (`Job::Failed`)
takes over as before. Decode failures caught by the prefetch pool are named the
same way and never reach the engine. No cascade, no retry storm.

## Shutdown and the quit confirm

The queue is deliberately **not persisted**. Quitting with waiting items now
shows the existing in-app confirm card with queue copy instead of closing
silently:

* title `N files still queued`, body "Not started yet — quitting drops them
  without a transcript." / "Finished transcripts are saved; the running file
  stops as before.";
* actions **Keep Working** (Esc) and **Quit and Drop**;
* quitting discards the waiting items, logs their names (`quit: dropped queued
  transcriptions`) and exits through the unchanged teardown/hard-exit funnel.

The card **is** the visible note — there is no cross-launch resume, and nothing
is dropped unless the user answers Quit and Drop; Keep Working (or Esc) returns
to the queue intact. The drop happens in `answer_quit` (before the frame loop
starts teardown), so the queue cannot be half-abandoned by a later error.

The running item keeps today's behavior: the bounded teardown does not interrupt
it, and the hard exit covers it if it is still working. A live recording's confirm
still wins when both conditions exist (recording locks imports, so this is only
theoretical; the recording prompt is unchanged).

## UI surface

* **Queue card** below the progress card, only when ≥1 item is pending:
  `Up next · N files`, up to four rows (`position. name`, state word) with
  painted `Up`, `Down`, `Remove` controls, then `+n more`. The running item is the
  card's subject and is never a row — it can't be reordered.
* Rows are painted at fixed geometry; names end-elide so the state word and the
  controls never leave the row. All three controls are the house painted
  tertiary buttons (24 pt hit areas, accesskit labels, hover/press crossfades only
  on input).
* **No idle redraws**: the list adds no `request_repaint` of its own; a row only
  repaints when input or a worker event changes it. Pinned by
  `the_queue_card_adds_no_idle_repaint` (repaint delay > 1 s with a static job).
* Importing while a job runs appends to the queue (today's import keeps the single
  slot). The macOS/Windows "Choose files…" dialog accepts a multi-selection and
  enqueues in dialog order; the Linux path field stays single-path.
* A batch continuation never steals the editor: its transcript is saved to the
  library and the status line reports `engine: … — N of M done`; only the user's
  own import opens its result as before.
* The transcribe caption uses the real stage: "decoding the audio…" until the
  engine starts, then "transcribing on this machine…".

## CLI

```
suramak-cli transcribe <input>... [--cancel N[,N]] [--model …] [--lang …] [--formats …] [--out-dir …] [--db]
```

* Paths are validated up front (one missing path refuses the whole batch), then
  enqueued in command-line order.
* Per-file completion lines go to **stderr** (stdout stays parseable):
  `[1/3] done: a.wav — 42 segments, engine cpu`,
  `[2/3] failed: b.wav — decode: …`, `[3/3] cancelled: c.wav`.
* `--cancel N` is 1-based over command-line order. Waiting files are removed
  before the batch starts; position 1 is the file that runs first, so its cancel
  is applied right after it takes the slot (the cooperative token stops it at its
  next stage boundary). It is meaningful with **two or more paths** — a
  single-path batch has nothing left to continue with (the CLI prints a note).
  Duplicates are ignored; an out-of-range position is an error before any work.
* A failed file never stops the batch; the exit status is non-zero only when at
  least one file failed (cancellations are requested outcomes). A one-line summary
  (`queue: 1 done, 1 failed, 1 cancelled of 3`) closes every run.
* Output names are input stems; duplicate stems in one batch become `-2`, `-3`, …
  so a batch cannot overwrite its own earlier export.
* The one-slot rule is visible: only one `[i/n] transcribing…` line moves at a
  time, and the decode-ahead line states the 96 MiB cap.

## Tests (all deterministic; no model, no audio device)

`crates/suramak-asr` — queue core (14): state walk; 1-slot invariant; decode-ahead
bound; over-budget inline fallback; cancel queued; cancel mid-decode (late buffer
dropped) and mid-transcribe (slot held until the worker answers); no cascade;
decode failure; shutdown drop; up/down ordering incl. pinned running row;
queue-order-wins-over-decode-order; prefetch bytes release; bounded display rows.

`crates/suramak-asr` — driver (7): mixed outcomes in order on one slot (backend
asserts no overlap); decode-ahead request bound; `--cancel` position removes a
queued item; position cancel sets the running token and holds the slot; mid-decode
cancel drops the late buffer; decode failure reported + batch continues; queue
continues after a failure.

`crates/suramak-ui` — state (9): second import queues instead of replacing;
next item starts on finish and continuations don't steal the view; cancel queued;
up/down + pinned running row; named failure + continuation + final failure state;
cancel-running continues the queue; stage event leaves `decoding`; quit confirm
and drop; a batch with one missing path starts nothing.

`crates/suramak-ui` — views (4): rows/controls/`+n more`; failure reason stays
visible; no idle repaint; name elision. Quit card (1): queue copy, both actions,
accesskit labels, Keep keeps the queue.

`crates/suramak-cli` (4: 2 batch + 2 pure): multi-file batch continues after a
failure and reports the count; `--cancel` reports the cancelled file and keeps
going; position validation; duplicate-stem naming.

Every M15 suite (split/merge undo, FTS search, karaoke word chips, bilingual
export) still passes untouched.

## Gates

| gate | result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **846 passed / 0 failed / 10 ignored** (baseline 807/0/10; +39 new tests) |
| disk | `cargo clean` at start (13 GB target → 22 GB free); final `target/` 6.6 GB, 15 GB free on `/` |

## Honest boundary

The queue's **logic** is covered headlessly: state transitions, the 1-slot
invariant, decode-ahead/memory bounds, cancellation at every stage, shutdown
drops, failure continuity, CLI ordering/cancel/exit codes and the card's paint
tree. The queue's **UX** is not judged here: this machine has no audio device and
no owner-Mac pass, so row density, the four-row limit, the `+n more` cut, the
quit-card copy and the multi-select dialog need the owner's Mac review with real
files.

One live check was run on this VPS with the installed `tiny` model and the
committed `en.wav` fixture, twice under two names: both files transcribed in order
(2 segments each, `engine cpu`, 9.3 s total) and both exports landed; the same
batch with `--cancel 1` reported `cancelled: one.wav`, wrote only `two.txt` and
finished the second file (4.6 s). That proves the real worker path, ordering,
prefetch hand-off, per-file lines and the running-cancel seam end to end — it is
**not** a quality or UX judgement (debug-build CPU whisper, no audio device, no
owner-Mac window).

Known limits, stated rather than hidden:

* the queue is in-memory: quitting (after the confirm) loses waiting items by
  design; there is no resume of a previous batch at startup;
* the UI does not decode ahead (0 of the allowed 2) — the shared driver does, and
  the CLI exercises it;
* a same-stem batch export gets `-2`/`-3` suffixes instead of a directory per
  file (`--out-dir` remains the caller's choice);
* reordering is one step at a time (no drag), matching the brief;
* `--db` writes each finished transcript in its own transaction, as before.

## Files

* `crates/suramak-asr/src/queue/mod.rs` — new: `JobQueue`, item states, bounds,
  cancellation/reorder/shutdown, 14 tests.
* `crates/suramak-asr/src/queue/driver.rs` — new: `QueueDriver`, backend/decoder
  traits, threaded prefetch, 7 tests.
* `crates/suramak-asr/src/{lib,worker,pipeline}.rs` — queue exports, `Job.samples`,
  `JobStage`/`WorkerEvent::Stage`, prepared-samples front half.
* `crates/suramak-asr/tests/{pipeline,alignment,parakeet}.rs` — call-site updates
  for the prepared-samples/stage parameters.
* `crates/suramak-cli/src/cmd/transcribe.rs` — multi-path batch, per-file lines,
  `--cancel`, unique stems, tests.
* `crates/suramak-ui/src/app/mod.rs` — queue state, enqueue/continue/cancel/
  reorder, quit prompt kind, `queue_failure`, tests.
* `crates/suramak-ui/src/app/transcribe.rs` — stage caption, queue card, failure
  line, tests.
* `crates/suramak-ui/src/app/quit_confirm.rs` — queue confirm variant, test.
* `crates/suramak-ui/src/app/home.rs` — multi-select import.
* `crates/suramak-ui/src/{runtime,main,framestats}.rs` — job construction, submit
  failure continuation, quit action handling, test literal.
