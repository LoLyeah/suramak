# WORKORDER M0a — workspace skeleton + `suramak-core`

You are a senior Rust engineer working **inside this repository** (current directory). Read these first, in order:

1. `docs/PLAN.md` — implementation plan (authoritative design).
2. `docs/PLAN-NOTES.md` — empirically verified corrections. **WHERE NOTES AND PLAN CONFLICT, THE NOTES WIN.**
3. `docs/MODEL-PINS.md` — exact sha256 + byte sizes for the model manifest (use these values verbatim).

## Your task (M0a ONLY)

Create the cargo workspace skeleton and implement **`suramak-core` completely, with its tests** — the core portion of PLAN.md §9's first-PR checklist, and NOTHING else:

- `Cargo.toml` (workspace root; `members = ["crates/suramak-core"]` for now; `[workspace.dependencies]` exactly per PLAN §10 as corrected by NOTES; `[profile.release] lto = false`).
- `rust-toolchain.toml` → `channel = "stable"` (NOT 1.88.0, per NOTES §6).
- `crates/suramak-core/` with ALL modules from PLAN §9: `lib.rs, error.rs, paths.rs, time.rs, transcript.rs, models.rs, download.rs, decode.rs, resample.rs, store.rs, export.rs, settings.rs, polish.rs, keychain.rs`.
- `crates/suramak-core/tests/`: `export.rs, store.rs, download.rs, polish.rs` (mock servers = hand-rolled `std::net::TcpListener` loops; no extra dev-dependencies beyond `tempfile`).
- `scripts/pin-model-hashes.sh` (for the future; the CURRENT values come from `docs/MODEL-PINS.md` — paste them into `models.rs` verbatim; do NOT re-download models).

## Exact requirements (anywhere ambiguous, follow PLAN then NOTES)

- **Only create/modify files inside this repository.** Never touch `~/.hermes`, `~/.ssh`, or anything outside the repo. No `sudo`. Do **not** run `git commit` or `git push` — leave all changes in the working tree.
- Dependencies: use the exact lines from PLAN §10 + NOTES; versions are caret floors. If a crate line fails to resolve on crates.io, pick the nearest resolvable version and note it in your report. Do not add anything MEMO doesn't list (only `tempfile` dev-dep is allowed extra).
- `error.rs`: single `SuramakError` enum (thiserror); all modules return `Result<T, SuramakError>`.
- `download.rs`: reqwest **blocking** + rustls; resume via `Range` header when `*.part` exists; sha256 (sha2) verify before rename; mock-server test with Range support.
- `decode.rs`: symphonia (features per PLAN §10) → `AudioBuffer { rate, channels, samples: Vec<f32> }`; `hound` is only for writing wav.
- `resample.rs`: rubato `fft_resampler` → 16 kHz mono f32. Unit test: 440 Hz sine at 44.1 kHz → dominant frequency within ±2% after resample (naive DFT or zero-crossing detector; no new deps).
- `store.rs`: rusqlite bundled; schema EXACTLY as PLAN §4 (WAL; `meta`, `transcripts`, `segments` tables); CRUD + search + cascade delete; tests use `tempfile` dirs.
- `export.rs`: md/txt/json/srt/vtt writers; **golden-string tests** with a fixed 3-segment transcript — write the exact expected strings into the tests (formats are specified in PLAN §4).
- `settings.rs`: toml load/save roundtrip test.
- `polish.rs`: `PolishBackend` trait + OpenAI-compatible blocking client; system prompt and validation EXACTLY as PLAN §5; mock-server tests: success, fenced ```json response, HTTP 500, id-mismatch.
- `keychain.rs`: `#[cfg(any(target_os = "macos", target_os = "windows"))]` keyring path (dep is target-gated); on Linux ONLY the env-var fallback `SURAMAK_OPENAI_API_KEY`. Must compile on Linux with zero keyring deps.
- `time.rs`: ms ↔ `HH:MM:SS.mmm` and srt/vtt variants; unit tested.
- `models.rs`: manifest built from `docs/MODEL-PINS.md` (id, hf filename, size, sha256, role). Roles: `tiny`=CI, `small`=fast, `turbo-q5-0`=default, `turbo`=accuracy. HF base URL: `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/`.
- Comments: minimal and factual. One-line rustdoc on public items.

## Acceptance — run ALL of these yourself; paste exact outputs into your report

```bash
cargo fmt --all
cargo fmt --all --check                                          # exit 0
cargo clippy -p suramak-core --all-targets -- -D warnings        # exit 0
cargo test -p suramak-core                                        # all green
```

Do NOT build/test anything else. Do NOT create the other crates. If a genuinely blocking problem appears after honest attempts, document it precisely instead of faking a pass — a truthful blocker beats a false green.

## Report

Write `docs/reports/M0a.md` (create the dir): what you implemented, any deviations from PLAN/NOTES with reasons, the full acceptance command outputs, and anything not done. Then stop.
