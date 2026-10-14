# HOTFIX-QUIT Report — clean quit (no ggml-metal abort on exit)

**Date**: 2026-10-14
**Executor**: Hermes subagent (deepseek-v4.1-flash)
**Scope**: `crates/suramak-ui` (`src/shutdown.rs` — new; `src/main.rs` — the quit funnel `App::exiting`/`App::quit`/`App::shutdown`; `src/runtime.rs` — `Runtime::shutdown` + `Bridge::shutdown_and_wait`), `crates/suramak-asr` (`src/live.rs` — `LiveDecoder::shutdown_and_wait`; `src/capture.rs` — the stop-time stream-error downgrade), `CHANGELOG.md`, `docs/PLAN-NOTES.md`, `docs/USER_SMOKE.md` (item 19), this report.
**Workorder**: `docs/WORKORDER-HOTFIX-QUIT.md` (base master post-M7, 619aa0d + the workorder commit).
**Untouched**: `crates/suramak-glass`, `crates/suramak-playback`, `crates/suramak-taps`, the winit fork, `scripts/`, `crates/suramak-ui/src/instance.rs` — **zero diffs**; `Cargo.toml`/`Cargo.lock`/all crate manifests — **zero diffs** (no new dependencies); Linux capture stays OFF (the app build's feature set is unchanged; only CI's existing `--features capture` test step exercises the capture-side change). No `git commit`/`git push`; no `sudo`; no files outside `/home/ubuntu/suramak`.

---

## 1. The bug, and what the sources actually say

Owner evidence: run11 log — quitting (menu Quit / Cmd-Q / window close) **after any whisper session** aborts with

```text
GGML_ASSERT([rsets->data count] == 0) failed   (ggml-metal-device.m:608)
ggml_metal_device_free ← __cxa_finalize_ranges ← exit ← [NSApplication terminate:]
zsh: abort
```

**Root cause (verified against the linked sources, not assumed).**

- The app's whisper contexts — and the Metal **residency sets** of their buffers — are owned by the worker thread (`suramak-worker`, behind `suramak-ui`'s bridge) and the live-decode thread (`suramak-live-decode`, M7). Nothing freed them before exit.
- ggml's Metal device is a **function-local static**: `ggml-metal-device.cpp` — `static ggml_metal_device_ptr ctx { ggml_metal_device_init() };` inside `ggml_metal_device_get()`. Its `unique_ptr` deleter calls `ggml_metal_device_free`, which runs `ggml_metal_rsets_free` → `GGML_ASSERT([rsets->data count] == 0)` (whisper-rs-sys 0.15.0 vendored whisper.cpp 1.8.3, `ggml/src/ggml-metal/ggml-metal-device.m:602..608`; the buffer free path does call the symmetric `ggml_metal_device_rsets_rm`, line 1492, so the array is empty **iff** every context's buffers were freed).
- **The load-bearing Darwin fact** (this is why the workorder's "hard `std::process::exit`" is not sufficient as literally written): on macOS *every* `exit()` runs the process's C++ static destructors. Apple libc, `Libc/stdlib/FreeBSD/exit.c`:

  ```c
  void exit(int status) {
      _tlv_exit();          // C++11 requires thread_local objects to be destroyed before global objects
      __cxa_finalize(NULL); // ← runs ggml's static Metal-device destructor
      if (__cleanup) _cleanup();
      __exit(status);
  }
  ```

  `std::process::exit` is `libc::exit`, so it still runs that destructor — and still aborts whenever a whisper context is alive at exit (exactly the "quit while a job runs / while recording" cases). The crash trace itself shows the chain (`exit → __cxa_finalize_ranges → ggml_metal_device_free`). The only termination that cannot run it is **`_exit(2)`** (the raw syscall; `_Exit` is a thin wrapper over it, `Libc/stdlib/FreeBSD/_Exit_.c`).

**Where each quit flavor goes** (verified in the pinned winit fork): menu Quit / Cmd-Q → `[NSApp terminate:]` → winit's delegate `applicationWillTerminate:` → `notify_windows_of_exit` + `internal_exit()` → dispatches winit `LoopExiting` → `ApplicationHandler::exiting` — **before** AppKit's own `exit()`; window close and a smoke run's `event_loop.exit()` reach the same `exiting` via the run-loop's exit handling. So a single hook covers every flavor, and exiting the process *inside that hook* preempts AppKit's `exit()` entirely.

## 2. What was implemented

### The quit funnel (`crates/suramak-ui/src/main.rs`)

- `impl ApplicationHandler for App` gained **`exiting()`** → `App::quit()`.
- `App::quit(&mut self) -> !` = `self.shutdown()` (best-effort teardown, idempotent) + `shutdown::hard_exit(self.exit_code())`; `exit_code()` is 1 after a failed start / job / smoke run, else 0 — **the smoke modes' existing codes are unchanged** (0 on success, 1 on failure; the guard's exit 2 happens before the loop and is untouched).
- `App::shutdown()` runs once (`shutdown_done` flag) and delegates to `Runtime::shutdown(TEARDOWN_GRACE)`; no shell/runtime → clean no-op. The window/surface is left to process teardown (dropping it inside AppKit's terminate notification would be a risk for no gain — the process ends immediately after).
- `main()`: `run_app` errors are printed and mapped to exit 1, then `app.quit()` is the belt-and-braces tail (the loop normally never returns, since `exiting` exits).

### The hard exit (`crates/suramak-ui/src/shutdown.rs`, new)

- `TEARDOWN_GRACE = 250 ms`, `TEARDOWN_POLL = 2 ms` (the workorder bound, pinned by a unit test).
- `hard_exit(code) -> !`: flush stdout/stderr by hand, then `libc::_exit(code)` on unix (Windows fallback: `std::process::exit`) — **documented in code with the full WHY**: `std::process::exit` would run `__cxa_finalize` and with it the ggml destructor; `_exit` skips all finalization, deterministically, on every path. The comment names the codebase's existing `std::process::exit` sites (smoke paths, instance guard) and why the quit path must differ.

### The teardown, through the existing paths (`crates/suramak-ui/src/runtime.rs`)

`Runtime::shutdown(grace) -> bool` (all steps share **one** deadline, total ≤ grace; never panics; idempotent):

1. **live (M7)**: `live.decoder.shutdown_and_wait(remaining)` — the existing signal-and-detach teardown, plus a bounded wait so an *idle* decode thread exits immediately and drops its whisper context (and its residency sets).
2. **capture**: `session.stop()` — the existing stop path (each stream, then any tap); the recorded buffers are discarded (quitting is not Stop; nothing is saved).
3. **playback**: the existing transport teardown (`Stop` releases the engine).
4. **worker bridge**: `Bridge::shutdown_and_wait(remaining)` — closes the job channel, then waits; the bridge thread joins the worker and drops its context cache as it exits. **A job in flight is never interrupted** — the wait is bounded, then the hard exit covers it.

`LiveDecoder::shutdown_and_wait(grace)` (`crates/suramak-asr/src/live.rs`) and `Bridge::shutdown_and_wait(grace)` (`runtime.rs`) share the same shape: signal (reusing the existing teardown), poll `JoinHandle::is_finished()` until the deadline, join (instant) when finished, detach otherwise — exactly like `Drop` leaves it today.

### Capture stop-time log noise (piggyback, cleanly separable — implemented)

`crates/suramak-asr/src/capture.rs`: `Recorder::stop` marks a per-recorder `teardown` flag **before** dropping the stream; the cpal stream-error callback (`imp::stream_error`) then logs the benign stop-time race (`The requested device is no longer available`, run11 ×3) at **debug** level, while genuine mid-recording errors keep the byte-identical legacy line (`audio capture error: {err}`) — classification is pure and unit-tested (`stream_error_line`). Capture semantics are untouched (no data-flow change; only the logging of an error that arrives while the session is being torn down). The quit path also routes capture through `MultiRecorder::stop`, so the same downgrade applies to quitting while recording.

### Docs

- `CHANGELOG.md` → `[Unreleased] → Fixed` (the clean quit + the capture-noise line).
- `docs/PLAN-NOTES.md` → new section **"Quit path — the ggml-metal teardown abort & our hard exit (verified, HOTFIX-QUIT)"**: the abort class, the Darwin `__cxa_finalize` fact, where the quit runs (and that Cmd-Q is intercepted before AppKit's `exit()`), the "do not simplify back to `std::process::exit`" rule, the smoke/guard note, and the capture-noise note.
- `docs/USER_SMOKE.md` → **item 19**: record ~5 s → Stop → Cmd-Q (no `zsh: abort`, no ggml assert, no crash report), plus menu Quit, window close, quit **while recording**, quit **while a job runs**, and quit after live transcription — all owner-verified on the Mac (Linux has no Metal).

---

## 3. Tests

**Suite: 251 passed, 0 failed, 6 ignored** (`cargo test --workspace`) — **+6 new tests** in the default run, **+1** under `--features capture`:

| Test | What it pins |
| --- | --- |
| `shutdown::tests::the_teardown_grace_stays_within_the_workorder_bound` | the ≤250 ms bound (and a usable poll interval) |
| `runtime::tests::bridge_shutdown_is_quick_idempotent_and_closes_the_channel` | an idle bridge exits within the grace; a second shutdown is a no-op; a shut-down bridge refuses jobs |
| `runtime::tests::the_remaining_budget_shrinks_and_clamps_at_zero` | steps share one grace; an expired deadline never yields a negative wait |
| `live::tests::the_live_decoder_shutdown_is_quick_idempotent_and_final` | an idle decode thread exits within the grace; idempotent; a stopped decoder refuses further windows |
| `tests::shutdown_without_a_shell_or_runtime_is_a_no_op_and_idempotent` | `App::shutdown` is a no-op with nothing running and never changes the exit code |
| `tests::the_exit_code_follows_the_failure_flag` | the smoke modes' exit codes (0 / 1) are preserved |
| `capture::tests::stop_time_stream_errors_are_quiet_but_live_ones_stay_loud` (`--features capture`) | teardown-time stream errors are quiet; live ones keep the exact legacy line |

Existing tests: **all green, unchanged** — including the release-only ignored e2e (`RUST_TEST_THREADS=1 SURAMAK_REQUIRE_MODEL=1 cargo test -p suramak-asr --release -- --ignored` → 6 passed, 0 failed: `e2e_en_fixture_contains_fox`, `e2e_id_fixture_has_non_empty_segment`, **`e2e_live_partials_then_the_final_pass_replaces_them`** (the M7 live session still ends in a byte-identical final pass), `e2e_video_fixture_matches_the_source_mp3_words`, `e2e_coreml_encoder_engaged_macos`, `mix_demo_two_synthetic_sources_to_a_wav`).

---

## 4. Acceptance (from `/home/ubuntu/suramak`, exact commands)

### 4.1 `cargo fmt --all --check` — clean

```text
$ cargo fmt --all --check
fmt exit=0
```

### 4.2 `cargo clippy --workspace --all-targets -- -D warnings` — clean

```text
$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.86s
clippy exit=0
```

(re-verified after `touch`ing every changed source file, so the changed crates were re-analysed: `Checking suramak-asr … Checking suramak-cli … Checking suramak-ui … Finished` — no warnings.)

### 4.3 `cargo test --workspace` — all green

```text
$ cargo test --workspace
… 19 test binaries …
test result: ok. 46 passed; 0 failed; 1 ignored; … (suramak-asr lib)
test result: ok. 95 passed; 0 failed; 0 ignored; … (suramak-ui bin)
test result: ok. 22 passed; 0 failed; 0 ignored; …
…
TEST-EXIT=0
passed=251 failed=0 ignored=6
```

New tests in the final run (exact lines):

```text
test live::tests::the_live_decoder_shutdown_is_quick_idempotent_and_final ... ok
test runtime::tests::bridge_shutdown_is_quick_idempotent_and_closes_the_channel ... ok
test runtime::tests::the_remaining_budget_shrinks_and_clamps_at_zero ... ok
test shutdown::tests::the_teardown_grace_stays_within_the_workorder_bound ... ok
test tests::shutdown_without_a_shell_or_runtime_is_a_no_op_and_idempotent ... ok
test tests::the_exit_code_follows_the_failure_flag ... ok
```

Capture feature (CI's existing step; the capture-side change and its test):

```text
$ cargo check -p suramak-asr --features capture
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.86s
$ cargo test -p suramak-asr --features capture
test capture::tests::stop_time_stream_errors_are_quiet_but_live_ones_stay_loud ... ok
test result: ok. 47 passed; 0 failed; 1 ignored; …
CAPTURE-TEST-EXIT=0
```

### 4.4 xvfb smoke — ok, and it exercises the quit path

Every smoke run now terminates **through the quit path itself** (`event_loop.exit()` → `exiting()` → teardown → hard exit), so the harness does exercise it; the Metal assert itself is macOS-only and owner-verified (USER_SMOKE item 19).

Plain smoke (CI's exact shape):

```text
$ SURAMAK_SMOKE_SECS=3 xvfb-run -a cargo run -p suramak-ui
suramak: smoke ok — 85 frames, wrote build/smoke_ok.txt
SMOKE-EXIT=0
$ cat build/smoke_ok.txt
suramak ui smoke ok
frames=85
secs=3
adapter=llvmpipe (LLVM 21.1.8, 256 bits) (Cpu, driver llvmpipe)
backend=vulkan
alpha_mode=PreMultiplied
size=1280x800
pixels_per_point=1
```

Scripted job smoke (real whisper context loaded in the worker — the teardown's worker step with a live context):

```text
$ SURAMAK_SMOKE_SECS=120 SURAMAK_SMOKE_JOB=fixtures/audio/en.wav xvfb-run -a cargo run -p suramak-ui
suramak: SMOKE-JOB-OK — en.wav (2 segments, model tiny) saved to the library
SMOKE-JOB-EXIT=0
$ cat build/smoke_job_ok.txt
suramak ui smoke job ok
job=fixtures/audio/en.wav
transcript_id=c49af6fc-0f42-4c1b-9117-8e5085a85d5e
segments=2
model=tiny
engine=cpu
secs=21.0
```

Idle-quit teardown evidence (`RUST_LOG=suramak=debug`):

```text
DEBUG suramak: quit: teardown finished workers_stopped=true
```

**Quit while a job runs** (headless proof of the bounded behavior — watchdog at 5 s while the tiny job needs ~21 s):

```text
$ RUST_LOG=suramak=debug SURAMAK_SMOKE_SECS=5 SURAMAK_SMOKE_JOB=fixtures/audio/en.wav xvfb-run -a cargo run -p suramak-ui
suramak: smoke job FAILED — watchdog expired before the job completed
DEBUG suramak: quit: teardown finished workers_stopped=false
EXIT=1 (expect 1)
WALL=6s (expect ~6-8s, NOT ~21s)      ← the quit did not wait for the job
file present? no
```

i.e. the worker was still transcribing, the teardown gave up after the 250 ms grace (`workers_stopped=false`), and the hard exit ended the process anyway — exit code and file semantics unchanged. On macOS that same case is exactly where the ggml assert used to fire; `_exit` cannot run it.

Startup-failure exit code (1) through the same funnel:

```text
$ SURAMAK_WGPU_BACKENDS=dx12 SURAMAK_SMOKE_SECS=3 xvfb-run -a cargo run -p suramak-ui
suramak: cannot start: no wgpu adapter on any backend:
EXIT=1 (expect 1)
```

### 4.5 No new dependencies

```text
$ git diff --name-only -- Cargo.toml Cargo.lock crates/*/Cargo.toml | wc -l
0
```

Working tree (nothing committed, nothing pushed):

```text
$ git status --short
 M CHANGELOG.md
 M crates/suramak-asr/src/capture.rs
 M crates/suramak-asr/src/live.rs
 M crates/suramak-ui/src/main.rs
 M crates/suramak-ui/src/runtime.rs
 M docs/PLAN-NOTES.md
 M docs/USER_SMOKE.md
?? crates/suramak-ui/src/shutdown.rs
```

---

## 5. Notes & deviations

1. **The hard exit is `_exit(2)`, not `std::process::exit` (documented deviation from the workorder's literal text).** The workorder's rationale — "so the fragile ggml static destructor never runs" — is only achievable with `_exit` on Darwin: `std::process::exit` → `exit(3)` → `__cxa_finalize(NULL)` → the destructor still runs, and still aborts whenever a context is alive (proven by the crash trace itself plus Apple's libc source quoted in §1). The workorder's approach block is explicitly "constraints, not a full spec"; the goal ("clean and silent in every path … including quitting while recording or while a job runs") and the teardown-first structure are implemented exactly as asked, and the "established pattern" is honoured for the sites it fits (the instance guard and the smoke *codes* are unchanged; the smoke runs simply end through the same funnel now).
2. **The teardown is still the primary mechanism, not decoration**: on a quiescent quit it frees the contexts (verified: `workers_stopped=true`), which is what makes the process state clean; `_exit` is what makes the mid-job/mid-record cases deterministic.
3. **Owner-verification boundary**: the ggml-metal abort is macOS-only (no Metal on Linux), so the quit behavior itself is owner-verified — **docs/USER_SMOKE.md item 19** (Cmd-Q / menu Quit / window close, after a session, while recording, while a job runs, after live transcription; expected: no `zsh: abort`, no `GGML_ASSERT` line, no new crash report). Linux verifies the mechanics: every smoke run exits through `exiting()` → teardown → hard exit with unchanged codes, the bounded mid-job behavior above, and the whole workspace suite is green. The capture-session teardown on quit (quitting while recording) is likewise mac-verified — Linux builds have no capture in the app, so only `cargo check/test -p suramak-asr --features capture` covers that code here.
4. **Not in scope / unchanged**: the worker is never interrupted mid-job (no cancellation path was added — "do not contort for a full join"); detached download/polish threads own no whisper contexts; the window/surface is left to process teardown; a recording in progress is discarded on quit (quitting is not Stop); the instance guard's `std::process::exit(2)` is untouched.
5. **`build/` logs** quoted above: `build/fmt.log`, `build/clippy.log`, `build/test-workspace.log`, `build/test-capture.log`, `build/smoke-*.log`, `build/e2e-ignored.log` (all gitignored).
