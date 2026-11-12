# WORKORDER — M22A2: resume wired into the two real apps

M22A (`df38644`) built the durable queue: schema v8 journal, checkpointing,
torn-write reconciliation, `QueueDriver::resume_pending()`. Only the CLI
calls it (`suramak-cli/src/cmd/transcribe.rs --db`). The promise "an OS kill
mid-lecture resumes at the boundary" is not live in the desktop app or on
Android. This slice wires it — no new mechanism.

## Deliverables

1. **Desktop**: the app's queue gets `with_journal(Store::open(db))` and calls
   `resume_pending()` once at startup, before the library's first frame is
   painted. Resumed jobs appear in the app's existing job surface with the
   `resumed` marker; `Unavailable`/`Skipped` outcomes surface as the row's
   named reason (existing failure surface, no new sheet).
2. **Android host**: same two calls in the jobs host's driver creation
   (`crates/suramak-android/src/jobs.rs`), so a process killed by the OEM
   battery saver resumes on the next launch. The journal must use the app's
   private DB path (the store already knows it).
3. **One shared helper** for "open the store, build the driver with a journal,
   resume" so the three hosts cannot drift; the CLI is refactored onto it.
4. **A test per host** that proves wiring, not the mechanism: seed the store
   with an interrupted job (the M22A fixture), construct the host's queue, and
   assert one job runs to completion with `resumed` set. Desktop test in
   `crates/suramak-ui/tests/`; Android test in `crates/suramak-android/tests/`.

## Verification

- fmt/clippy -D warnings; NDK check + clippy on both Android crates; one full
  workspace run (baseline 1281).
- Report: append §"wiring" to `docs/reports/M22a-durable-queue.md`, naming the
  exact call sites.

## Do NOT

- No changes to the journal schema or the resume mechanics; no UI redesign;
  no new crates; no CI edits; no git commit/push; no sudo.
