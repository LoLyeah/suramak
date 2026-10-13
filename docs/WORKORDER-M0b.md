# WORKORDER M0b — `suramak-asr` + `suramak-cli` + CI (completes M0)

You are a senior Rust engineer working **inside this repository**. Read first, in order:

1. `docs/PLAN.md` — design. 2. `docs/PLAN-NOTES.md` — verified corrections (**NOTES beat PLAN**). 3. `docs/MODEL-PINS.md` — model manifest facts. 4. `docs/reports/M0a.md` — what already exists in `suramak-core`; use its APIs as-is. Only touch `suramak-core` if acceptance genuinely forces a fix — document any change in your report.

## Task (M0b ONLY)

1. **`crates/suramak-asr`** per PLAN §9: `lib.rs, whisper.rs, pipeline.rs, worker.rs, capture.rs` + `tests/pipeline.rs`.
2. **`crates/suramak-cli`** per PLAN §9: `main.rs` + `cmd/{models,transcribe,export,library,polish,doctor,devices}.rs`.
3. Workspace: add both crates to `members`; `suramak-asr/Cargo.toml` per PLAN §10 **including** the macOS `metal` target-dependency and the `capture` feature (cpal optional, OFF by default).
4. **`.github/workflows/ci.yml`**: linux job + macos job, per PLAN §7 but WITHOUT any `suramak-glass`/xvfb steps (those crates don't exist yet). macOS job: `runs-on: macos-26-arm64` (runner label is verified to exist).

## Requirements

### suramak-asr
- `whisper.rs`: context cache (`HashMap<PathBuf, WhisperContext>`); `FullParams` exactly per NOTES §3 (Greedy{best_of:1}, `set_n_threads`, all `set_print_*` false, `set_token_timestamps(true)`, `set_language` from a `LangMode`, `set_initial_prompt` when set). Segments via `state.as_iter()`; `start_timestamp()/end_timestamp()` are **centiseconds → multiply by 10 for ms**. Error type: define `AsrError` (thiserror) or reuse `SuramakError` — pick one, be consistent, convert cleanly at boundaries.
- `pipeline.rs`: `decode` (core) → `resample` to 16 kHz mono (core) → whisper → core `Transcript`. Progress: prefer whisper's progress callback (`set_progress_callback`, forward 0–100 over the worker channel; user-data pointer per PLAN §2); if the binding makes that impractical, report progress per segment (start 0, +100/segment-count per segment) and say so in the report.
- `worker.rs`: one named thread `suramak-worker`; std mpsc; `Job { id, input, model_path, lang, initial_prompt }` in, `WorkerEvent { Progress{job_id,pct}, Segments{job_id, Vec<Segment>}, Finished{job_id, Transcript}, Error{job_id, message} }` out. Context cache lives in the worker thread.
- `capture.rs`: behind feature `capture`: cpal default input → f32 mono accumulation + live RMS value. No UI. Feature off ⇒ a stub that reports unsupported.
- `tests/pipeline.rs`: non-ignored: `LangMode` parsing / small unit logic. `#[ignore]` + release-only e2e: en + id fixtures (from the standard data dir tiny model); assertions: en contains case-insensitive "fox"; id has ≥1 segment with non-empty text. Missing model ⇒ skip with a message **unless env `SURAMAK_REQUIRE_MODEL=1`** (then fail). CI sets it.

### suramak-cli
- clap derive; subcommands exactly: `models list | download <id> | remove <id>`; `transcribe <input> [--model id|path] [--lang auto|en|id] [--initial-prompt S] [--formats md,txt,json,srt,vtt] [--out-dir D] [--db]`; `library list`; `export <id> --format F --out FILE`; `polish <id> [--base-url U] [--model M]`; `doctor`; `devices` (stub unless `capture` feature).
- `transcribe` writes each requested format to `--out-dir` (basename of input). **Decision: saving to the library is OFF by default; `--db` opts in** (privacy default). Document this in the CLI help text and your report.
- `doctor`: prints data dir, db path, models dir + installed models, settings path; creates missing dirs; exit 0.
- `anyhow` only in `main.rs`/cmd layer; exit non-zero with stderr message on error.

### CI (`.github/workflows/ci.yml`)
- Triggers: push + pull_request.
- **linux** (ubuntu-latest): apt `cmake`; checkout; `Swatinem/rust-cache@v2`; steps: `cargo fmt --all --check` → `cargo clippy --workspace --all-targets -- -D warnings` → `cargo test --workspace` → `cargo build --release -p suramak-cli` → cache+download tiny model (`actions/cache@v4` on `~/.local/share/Suramak/models`, key includes `be07e048e1e5` from MODEL-PINS) → CLI e2e (en+id transcribe + python3 JSON assertions + `grep -qi fox`) → `SURAMAK_REQUIRE_MODEL=1 cargo test --workspace --release -- --ignored`.
- **macos** (`runs-on: macos-26-arm64`): checkout; rust-cache; `cargo build --release --workspace`; download tiny (same cache approach); `SURAMAK_REQUIRE_MODEL=1 cargo test --workspace --release -- --ignored`.

## Constraints
- Only modify files inside this repository. No `sudo`. No `git commit` / `git push` — leave changes in the working tree.
- No dependencies beyond PLAN §10 + NOTES: asr gets `whisper-rs, cpal(optional), serde, serde_json, thiserror, tracing, suramak-core`; cli gets `clap, anyhow, tracing, tracing-subscriber, serde_json, suramak-core, suramak-asr`.
- Do NOT rename or regenerate `fixtures/audio/{en,id}.wav`; do not add espeak or UI code; no objc2/windows crates.

## Acceptance (run ALL yourself; paste exact outputs in the report)

```bash
cargo fmt --all
cargo fmt --all --check                                                    # exit 0
cargo clippy --workspace --all-targets -- -D warnings                      # exit 0
cargo test --workspace                                                     # green
cargo build --release -p suramak-cli
cargo run --release -p suramak-cli -- models download tiny                 # exit 0, sha256 verified
cargo run --release -p suramak-cli -- transcribe fixtures/audio/en.wav --model tiny --lang en --formats json,md --out-dir build/out
python3 -c "import json;d=json.load(open('build/out/en.json'));assert len(d['segments'])>=1;print('en segments',len(d['segments']))"
grep -qi fox build/out/en.json && echo EN-OK
cargo run --release -p suramak-cli -- transcribe fixtures/audio/id.wav --model tiny --lang id --formats json --out-dir build/out
python3 -c "import json;d=json.load(open('build/out/id.json'));assert len(d['segments'])>=1 and d['segments'][0]['text'].strip();print('id segments',len(d['segments']))"
SURAMAK_REQUIRE_MODEL=1 cargo test --workspace --release -- --ignored       # green
cargo run --release -p suramak-cli -- doctor                               # exit 0
```

(Use `python3` for JSON assertions — `jq` may be absent. Data dir on Linux: `~/.local/share/Suramak/`.)

If a "do not fix" line like the M0 acceptance still applies: debug-profile whisper is ~50× slow — e2e stays release-only and `#[ignore]`d; do not "fix" that.

## Report
`docs/reports/M0b.md`: implemented, deviations (with reasons), full acceptance outputs, blockers. Then stop.
