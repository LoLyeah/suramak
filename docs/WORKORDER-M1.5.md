# WORKORDER M1.5 — macOS fast engine: Core ML / ANE encoder (`whisper-coreml`)

You are a senior Rust engineer working **inside this repository**. Read first: `docs/ENGINES.md` (the decision record for this work), `docs/PLAN-NOTES.md`, `docs/reports/M0b.md`, `docs/WORKORDER-M0b.md` (CLI conventions), and `docs/reports/M1.md` if it exists.

## Task (M1.5 ONLY — do not touch `suramak-ui` / `suramak-glass`)

1. **`suramak-core`**
   - Model manifest (`models.rs`): add encoder-package entries (`encoder_zip` file name, byte size, sha256) for the models that have official encoders: tiny, base, small, medium, large-v3, large-v3-turbo (see HF `ggerganov/whisper.cpp` — names like `ggml-large-v3-turbo-encoder.mlmodelc.zip`). **Fetch the real sizes/hashes at implementation time** (extend `scripts/pin-model-hashes.sh` to hash encoder zips too; paste results); do not invent hashes.
   - Downloader: support `.zip` artifacts — download → sha256 verify → extract keyed directory `<model-stem>-encoder.mlmodelc` next to the model file in the models dir. Add the `zip` crate (pure Rust) with a one-line justification in the report; no other new deps.
   - Settings (`settings.rs`): add `engine: EngineSetting { Auto, CoreML, Metal, Cpu }` (serde default `Auto`) + TOML roundtrip test.
2. **`suramak-asr`**
   - Passthrough feature `coreml = ["whisper-rs/coreml"]`, enabled only for the macOS target in the workspace (feature may exist on all targets but must not affect non-macOS builds).
   - `whisper.rs`: resolve the encoder directory (`<models-dir>/<model-stem>-encoder.mlmodelc`) when engine is CoreML/Auto; construct `WhisperContextParameters` accordingly (**check whisper-rs 0.16's actual knob for CoreML/use_gpu — read the crate source before coding; PLAN-NOTES §3 discipline**). If init fails with CoreML → log a warning, retry without it, and record the engaged engine. Expose `engine_used` in the pipeline result / worker diagnostics.
   - Engine resolution order for `Auto` on macOS: CoreML (only if the encoder dir exists) → Metal → CPU; on other OSes: CPU. Pure function + unit tests.
3. **`suramak-cli`**
   - `models download <id> [--encoder]` (downloads the encoder package when asked or when `--encoder`); `models list` shows encoder presence per model; `models remove` cleans encoder dirs too.
   - `transcribe ... [--engine auto|coreml|metal|cpu]`, and print the engaged engine at completion (stderr line).
   - `doctor` prints an "engines" section: compiled backends, CoreML available (macOS only), encoder pairing status for installed models.
4. **Tests / verification**
   - Portable unit tests: engine resolution, encoder path resolution, zip extraction (build a tiny synthetic zip in the test), downloader zip path (mock server).
   - `cargo check -p suramak-asr --features coreml --target aarch64-apple-darwin` **must pass from Linux**; also plain `cargo check --target aarch64-apple-darwin`.
   - From Linux you CAN fully verify the download/extract path: `cargo run -p suramak-cli -- models download tiny --encoder` → files at `~/.local/share/suramak/models/ggml-tiny-encoder.mlmodelc/` + `doctor` shows pairing. Do it.
   - Real ANE execution is macOS-only: add an `#[ignore]` e2e + a short "Mac manual check" section in `docs/ENGINES.md` (exact commands + what to expect: first run compiles, then benchmark).
5. **Docs**: update `docs/ENGINES.md` (status/precision confirmed), README gets one line about ANE acceleration; CI: linux job adds the two `cargo check ... --target aarch64-apple-darwin` steps for the asr crate with `--features coreml`.

## Constraints
- Only files inside this repo; no `sudo`; no `git commit`/`git push`.
- Do not modify `suramak-ui`/`suramak-glass` (parallel workstream); `core`/`asr`/`cli` only as scoped.
- New dependency: `zip` only (pure-Rust), justified. Everything else per existing manifests.
- Honest verification: if the encoder pairing cannot be validated from Linux for a given model, say exactly that in the report.
- Engine selection is a pure function of `(language, installed engines, platform availability)` → engine id, even though only Whisper variants exist now; future engines (Parakeet via sherpa-rs/FluidAudio, Apple SpeechTranscriber) must slot in as a new variant + routing rule without reshaping call sites. Diagnostics always expose the engine that actually ran.

## Acceptance

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p suramak-asr --target aarch64-apple-darwin
cargo check -p suramak-asr --features coreml --target aarch64-apple-darwin
cargo run -p suramak-cli -- models download tiny --encoder   # exit 0; sha256 verified; mlmodelc dir exists
cargo run -p suramak-cli -- doctor                            # engines section + tiny encoder pairing OK
cargo run --release -p suramak-cli -- transcribe fixtures/audio/en.wav --model tiny --lang en --formats json --out-dir build/out   # still green
```

## Report
`docs/reports/M1.5.md`: implemented, deviations (with reasons), full acceptance outputs, blockers. Then stop.
