# M0b Report — `suramak-asr` + `suramak-cli` + CI (completes M0)

**Date**: 2026-10-13
**Executor**: Hermes subagent (deepseek-v4.1-flash)
**Scope**: `crates/suramak-asr`, `crates/suramak-cli`, workspace members, `.github/workflows/ci.yml`, full M0b acceptance run.

---

## What was implemented

### `crates/suramak-asr` (per PLAN §9/§10 + WORKORDER)

| File | Summary |
|---|---|
| `src/lib.rs` | Module exports: `capture`, `pipeline`, `whisper`, `worker` + re-exports (`AsrError`, `ContextCache`, `Job`, `Worker`, `WorkerEvent`, `parse_lang`, `transcribe_file`, `model_id_from_path`) |
| `src/whisper.rs` | `AsrError` (thiserror; `#[from] SuramakError` for core errors); `ContextCache` (`HashMap<PathBuf, WhisperContext>`, one context per model path); `transcribe()` builds `FullParams` exactly per PLAN-NOTES §3 (`Greedy{best_of:1}`, `set_n_threads`, `set_language` from `LangMode`, `set_initial_prompt` when set, all `set_print_*` false, `set_token_timestamps(true)`); progress via `set_progress_callback` + user-data pointer (PLAN §2); segments via `state.as_iter()`, timestamps **centiseconds × 10 = ms**; `to_str()` with lossy fallback only on invalid UTF-8; `default_threads()` = parallelism clamped to 2..=8 |
| `src/pipeline.rs` | `transcribe_file()`: core `decode` → core `resample` to 16 kHz mono → whisper → core `Transcript`; `parse_lang()` (`auto`/`en`/`id`/… → `LangMode`); `model_id_from_path()` (`ggml-tiny.bin` → `tiny`) |
| `src/worker.rs` | One named thread `suramak-worker`; std mpsc; `Job{id,input,model_path,lang,initial_prompt}` in; `WorkerEvent::{Progress,Segments,Finished,Error}` out; context cache lives in the worker thread; `install_logging_hooks()` once at startup; `Worker::submit/try_recv/recv`, `Drop` joins the thread |
| `src/capture.rs` | Feature `capture` (OFF by default): cpal default input → per-block mono mixdown into `Vec<f32>` + live RMS (`AtomicU32`); `Recorder::{device_name,sample_rate,rms,stop}` (`stop()` returns a core `AudioBuffer` ready for resampling); `list_input_devices()`. Feature off ⇒ stub returning `AsrError::CaptureUnsupported` |
| `tests/pipeline.rs` | 4 non-ignored unit tests (LangMode parsing, model-id derivation, cache miss error, worker error-event flow) + 2 `#[ignore]` release-only e2e tests (en "fox" assertion, id non-empty assertion) that skip with a message when the tiny model is absent unless `SURAMAK_REQUIRE_MODEL=1` |

`Cargo.toml` per PLAN §10: `suramak-core, whisper-rs, thiserror, tracing, serde, serde_json`; `capture = ["dep:cpal"]` with `cpal = "0.15.3"` optional; macOS-only `whisper-rs` with `features = ["metal"]`.

### `crates/suramak-cli` (per PLAN §9 + WORKORDER)

| File | Summary |
|---|---|
| `src/main.rs` | clap derive; `anyhow` only in main/cmd layer; non-zero exit + `error: …` on stderr on failure; tracing-subscriber with `RUST_LOG` (default `warn`) |
| `src/cmd/mod.rs` | Shared `ExportFormat` (ValueEnum, renders via core exporters), `parse_formats()`, `resolve_model()` (manifest id or `.bin` path), `human_size()`, `load_transcript()` |
| `src/cmd/models.rs` | `list` / `download <id>` (downloads, **re-verifies sha256 on the final file** before claiming success) / `remove <id>` (also removes stale `.part`) |
| `src/cmd/transcribe.rs` | Worker-driven transcription with progress on stderr; writes each requested format to `--out-dir` using the input basename; `--db` opt-in for library save |
| `src/cmd/export.rs` | `export <id> --format F --out FILE` from the library |
| `src/cmd/library.rs` | `library list` (id, created, model, lang, title) |
| `src/cmd/polish.rs` | `polish <id> [--base-url U] [--model M]`; flags > `settings.toml` > defaults (`https://api.openai.com/v1`, `gpt-4o-mini`); upserts polished segments |
| `src/cmd/doctor.rs` | Prints data dir, db path, models dir + installed models (size-checked against the manifest), settings path; creates missing dirs; exit 0 |
| `src/cmd/devices.rs` | `capture` feature ⇒ lists input devices; otherwise prints "not available in this build" (exit 0) |

`Cargo.toml` per PLAN §10: `clap, anyhow, tracing, tracing-subscriber, serde_json, suramak-core, suramak-asr` (+ `capture` feature forwarding to `suramak-asr/capture`).

### Workspace + CI

- Root `Cargo.toml`: `members = ["crates/suramak-core", "crates/suramak-asr", "crates/suramak-cli"]`.
- `.github/workflows/ci.yml`: triggers `push` + `pull_request`; **linux** job (ubuntu-latest): apt `cmake` → checkout → `Swatinem/rust-cache@v2` → `cargo fmt --all --check` → clippy `-D warnings` → `cargo test --workspace` → `cargo build --release -p suramak-cli` → `actions/cache@v4` tiny model (key includes `be07e048e1e5`) → download → CLI e2e (en+id, python3 asserts, `grep -qi fox`) → `SURAMAK_REQUIRE_MODEL=1 cargo test --workspace --release -- --ignored`. **macos** job (`runs-on: macos-26-arm64`): checkout → rust-cache → `cargo build --release --workspace` → tiny model cache/download → `SURAMAK_REQUIRE_MODEL=1 cargo test --workspace --release -- --ignored`. No `suramak-glass`/xvfb steps. YAML validated with `yamllint` (exit 0; only the optional document-start style warning).

---

## Deviations from PLAN / WORKORDER (with reasons)

1. **`whisper-rs` feature `raw-api` enabled** in `suramak-asr` (feature flag, *not* a new dependency). The WORKORDER-preferred progress mechanism (`set_progress_callback` + user-data pointer per PLAN §2) requires naming `whisper_context`/`whisper_state` in the trampoline signature, which whisper-rs re-exports only behind `raw-api`. The safe alternative (`set_progress_callback_safe`) was rejected: it requires a `'static` closure and leaks a boxed closure per call in whisper-rs 0.16.0 (verified in crate source: `progress_callback_safe` is set to `None` while a raw `Box::into_raw` pointer is installed). The raw pointer path with a stack-local `ProgressContext` is sound: whisper.cpp invokes the callback synchronously on the decoding thread during `full()`.
2. **Progress is whisper's real progress callback** (0–100 forwarded over the worker channel), not the per-segment fallback — no fallback needed to report.
3. **CI model-cache paths corrected to reality**: `directories 5.0.1` lowercases the app name on Linux → the real data dir is `~/.local/share/suramak/` (not `~/.local/share/Suramak/` as the WORKORDER parenthetical says; verified empirically from the download and the crate source), and on macOS the project path is the reverse-DNS bundle id → `~/Library/Application Support/dev.suramak.Suramak/`. CI caches those real paths. Caching is best-effort only — the CLI and tests both use `paths::models_dir()`, so correctness is unaffected either way.
4. **CLI defaults chosen** (unspecified in the WORKORDER): `--model turbo-q5-0` (matches core `Settings` default), `--formats md`, `--lang auto`. All documented in `--help`.
5. **No direct `uuid` dependency in the CLI** (kept to the exact WORKORDER dependency list): `export`/`polish` resolve the `<id>` by case-insensitive match against `library list` output instead of parsing a `Uuid`.
6. **`whisper_rs::install_logging_hooks()`** called once in the worker: `set_print_*` only silence decode output; whisper.cpp's C-level model-load chatter still prints to stderr otherwise. With no log/tracing backend enabled this silences it cleanly; failures still surface via return codes/`AsrError`.
7. **`Transcript.model`** is derived from the model file path (`ggml-tiny.bin` → `tiny`) because the WORKORDER fixes `Job` to `{id, input, model_path, lang, initial_prompt}` (no separate model id).
8. **`devices` without the `capture` feature exits 0** with an explanatory message (it is a status report, like `doctor`); with the feature it lists devices and exits 0 (M3 flips the feature on).
9. **`suramak-core` unchanged** — `git diff` over `crates/suramak-core` is empty; the only pre-existing file modified is the root `Cargo.toml` (members line) plus `Cargo.lock`.

### Dependency resolution (from `Cargo.lock`)

| Dep | Requested | Resolved |
|---|---|---|
| `whisper-rs` | `0.16` (+`raw-api`, `metal` on macOS) | `0.16.0` (sys `0.15.0`) |
| `cpal` | `0.15.3` (optional) | `0.15.3` |
| `clap` | `4` | `4.6.7` |
| `anyhow` | `1` | `1.0.104` |
| `serde_json` | `1` | `1.0.154` |
| `tracing-subscriber` | `0.3` | `0.3.23` |
| `thiserror` | `2` | `2.0.21` (1.0.69 present transitively) |

No dependencies were added beyond PLAN §10 + NOTES (the `raw-api` addition is a feature of an already-listed dependency).

---

## Acceptance command outputs (all run from the repo root)

### 1. `cargo fmt --all` → exit 0, `cargo fmt --all --check` → exit 0

```
(no output)
```

### 2. `cargo clippy --workspace --all-targets -- -D warnings` → exit 0

```
    Checking suramak-cli v0.1.0 (/home/ubuntu/suramak/crates/suramak-cli)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.21s
```

### 3. `cargo test --workspace` → exit 0

```
     Running tests/pipeline.rs (target/debug/deps/pipeline-219bfafb4606c02d)
test e2e_en_fixture_contains_fox ... ignored, release-only e2e: debug whisper is ~50x too slow (run with --release -- --ignored)
test e2e_id_fixture_has_non_empty_segment ... ignored, release-only e2e: debug whisper is ~50x too slow (run with --release -- --ignored)
test context_cache_reports_missing_model ... ok
test model_id_strips_ggml_prefix_and_extension ... ok
test parse_lang_auto_and_fixed ... ok
test worker_reports_missing_model_as_error_event ... ok
test result: ok. 4 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/lib.rs (suramak_core)
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s
     Running tests/download.rs (suramak_core)  → ok. 5 passed
     Running tests/export.rs   (suramak_core)  → ok. 5 passed
     Running tests/polish.rs   (suramak_core)  → ok. 8 passed
     Running tests/store.rs    (suramak_core)  → ok. 8 passed
```

(33 core tests + 4 asr tests green, 2 asr e2e ignored; 10 × `test result: ok`, exit 0.)

### 4. `cargo build --release -p suramak-cli` → exit 0

```
real    4m12.700s
user    7m32.228s
sys     0m23.619s
```

### 5. `cargo run --release -p suramak-cli -- models download tiny` → exit 0

```
downloading tiny (77.7 MB)
  from https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin
  to   /home/ubuntu/.local/share/suramak/models/ggml-tiny.bin
tiny downloaded, sha256 verified (77.7 MB): /home/ubuntu/.local/share/suramak/models/ggml-tiny.bin
```

### 6. en transcribe + assertions → exit 0

```
$ cargo run --release -p suramak-cli -- transcribe fixtures/audio/en.wav --model tiny --lang en --formats json,md --out-dir build/out
transcribing…   0%… 100%… done
wrote build/out/en.json (2 segments, json)
wrote build/out/en.md (2 segments, md)
library save off by default (privacy) — pass --db to opt in

$ python3 -c "import json;d=json.load(open('build/out/en.json'));assert len(d['segments'])>=1;print('en segments',len(d['segments']))"
en segments 2

$ grep -qi fox build/out/en.json && echo EN-OK
EN-OK
```

`build/out/en.md` content (segment 1 is the exact fox sentence):

```
# en.wav

- Date: 2026-10-13 03:39:14 UTC
- Source: en.wav
- Model: tiny
- Language: en

## Transcript

[00:00:00.000 → 00:00:03.000] The quick brown fox jumps over the lazy dog.

[00:00:03.000 → 00:00:08.280] Surramat keeps every recording on this machine and never sends audio to the cloud.
```

### 7. id transcribe + assertion → exit 0

```
$ cargo run --release -p suramak-cli -- transcribe fixtures/audio/id.wav --model tiny --lang id --formats json --out-dir build/out
wrote build/out/id.json (3 segments, json)

$ python3 -c "import json;d=json.load(open('build/out/id.json'));assert len(d['segments'])>=1 and d['segments'][0]['text'].strip();print('id segments',len(d['segments']))"
id segments 3
```

### 8. `SURAMAK_REQUIRE_MODEL=1 cargo test --workspace --release -- --ignored` → exit 0

```
test e2e_en_fixture_contains_fox ... ok
test e2e_id_fixture_has_non_empty_segment ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 16.95s
```

### 9. `cargo run --release -p suramak-cli -- doctor` → exit 0

```
suramak doctor
  data dir:   /home/ubuntu/.local/share/suramak
  db:         /home/ubuntu/.local/share/suramak/suramak.db (not created yet)
  models dir: /home/ubuntu/.local/share/suramak/models
  settings:   /home/ubuntu/.local/share/suramak/settings.toml (built-in defaults)
installed models:
  - /home/ubuntu/.local/share/suramak/models/ggml-tiny.bin (77.7 MB, id tiny, size ok)
ok
```

### Extra verification (not in the acceptance block)

- `models list` → 6 models, `tiny` marked installed (exit 0).
- `transcribe … --db` → `saved to library: id 2c36b84d-…`; `library list` → 1 row; `export <id> --format md --out build/out/export-test.md` → exit 0, file matches the transcript.
- `polish <id>` without `SURAMAK_OPENAI_API_KEY` → `error: API key not set — set SURAMAK_OPENAI_API_KEY or configure keychain`, exit 1 (expected negative path).
- `devices` (feature off) → "device enumeration is not available in this build…", exit 0.
- `cargo check -p suramak-asr --features capture` → exit 0 (cpal 0.15.3 + alsa built; the capture path compiles, though it is not exercised in CI per the WORKORDER CI scope).
- `cargo run --release -p suramak-cli -- transcribe --help` documents `--db`: *"Also save the transcript to the local library database. OFF by default for privacy: nothing is persisted unless you opt in"*.

---

## Blockers

**None.** All acceptance commands are green.

---

## Toolchain / environment

- rustc 1.99.0 (b940084d7 2026-09-28), cargo 1.99.0 — `stable` per `rust-toolchain.toml`.
- Platform: x86_64-unknown-linux-gnu, 2 cores / 3.6 GB; whisper.cpp compiled via cmake in ~1.5 min (debug) / ~2 min (release).
- Acceptance logs kept under `build/logs/` (gitignored), export artifacts under `build/out/`.
