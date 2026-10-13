# WORKORDER M2 — transcription in the UI + library + export

Authoritative milestone: `docs/PLAN.md` §6 "M2". Read first: `docs/PLAN-NOTES.md` (corrections win over PLAN), `docs/reports/M1.md` + `docs/reports/M1.5.md` (current state of the shell and the asr worker), and the sources you'll call (`suramak-asr` worker API, `suramak-core` store/export/settings API — read them, don't guess signatures).

## Goal

The app becomes functional: **drop a file (or pick one) → transcribe in-app with live progress → edit the transcript → it lives in the sqlite library → export to a file.** All inside the existing M1 Liquid-Glass shell (chrome strip, sidebar, main pane composition stays; the static placeholder content is replaced by real views).

## Scope — do exactly this

1. **`crates/suramak-ui/src/app/` modules** (replace/split the placeholder `view.rs` as needed; keep `shell.rs`/`theme.rs`/glass flow intact):
   - `app/mod.rs` — app state, routes (home / transcribe-editor), wiring.
   - `app/home.rs` — drop-zone view (idle state): import button (rfd on macOS/Windows; on Linux a path text field + Enter — see deps), recent recordings list (from store), clear "drop a file here" affordance matching the C composition's calm style.
   - `app/transcribe.rs` — running-job view: file name, progress bar (`WorkerEvent` progress), engine line (from M1.5's outcome reporting), completion → editor.
   - `app/editor.rs` — transcript editor: per-segment row = mono timestamp label + editable text (`egui::TextEdit`), selection follows playback position if trivial to do (else skip playback), **autosave**: edits mark dirty → debounced (~1 s) persist to store; "saved" indicator.
   - `app/library.rs` — sidebar list bound to the real store (replaces placeholder rows); selecting a recording loads it into the editor.
   - `app/export.rs` — format picker (md / txt / json / srt / vtt — use core's export module) + destination (rfd save dialog on macOS/Windows; Linux: output path field), writes the file, shows result/error line.
2. **Import paths**: winit `WindowEvent::DroppedFile` (works all platforms) + the import affordance above.
3. **Worker wiring**: `suramak-ui` gains path deps on `suramak-core` **and** `suramak-asr` (default features; `capture` OFF). One background worker thread, std channel for events; call `egui::Context::request_repaint()` from the UI side when events arrive (Context is Send+Sync). In-app transcribe **always saves to the library DB** (unlike CLI's opt-in default) — that is the product behavior. UI logic (job lifecycle, autosave debounce, route switching, export path/format selection) lives in **pure modules with no egui types** so it is unit-testable headless.
4. **Scripted smoke job**: env `SURAMAK_SMOKE_JOB=<audio path>` → on startup the app runs the full in-app pipeline (import → transcribe with the resolved default model → save to DB), then asserts the DB row exists, prints `SMOKE-JOB-OK`, writes `build/smoke_job_ok.txt`, exits 0. `SURAMAK_SMOKE_SECS` is the watchdog: if the job hasn't completed when it elapses → exit 1. Plain `SURAMAK_SMOKE_SECS` smoke (no job) must keep working exactly as in M1.
5. **Dependencies**: only these additions — `rfd = "0.15"` under `[target.'cfg(any(target_os = "macos", target_os = "windows"))'.dependencies]` (Linux deliberately excluded: avoids zbus/portal deps; Linux uses the path-field affordance) and the two internal path deps. Nothing else new without justification in the report.
6. **Tests** (extend the suite; current total is 67 passing): headless state-machine tests for job lifecycle, autosave debounce (fake clock), route switching, export selection; UI modules that can't be egui-free get thin seams instead. `cargo test --workspace` must stay green.
7. **CI** (`.github/workflows/ci.yml`): linux job — after the M0 e2e block, add the scripted smoke job (`SURAMAK_SMOKE_SECS=120 SURAMAK_SMOKE_JOB=fixtures/audio/en.wav xvfb-run -a cargo run -p suramak-ui` in the same debug build the M1 smoke uses; assert `build/smoke_job_ok.txt`). macOS job — add the same scripted job after the existing smoke (the model cache is already there).
8. **Report**: `docs/reports/M2.md` — what was built, deviations with reasons, the full acceptance block with real outputs, honest blockers. The exact `library list` assertion must reflect the real CLI output format (document the command you ran).

## Do NOT

- Recording, polish UI, model-download UI, Windows work, settings screen (M3/M4).
- Touch `suramak-glass` internals; change `suramak-core`/`suramak-asr` public APIs except strictly-additive (document any change in the report).
- Change the M1 shell visuals (chrome/titlebar/glass/smoke infra) — the composition is approved.

## Environment (Linux VPS)

- `export PATH="$HOME/.cargo/bin:$PATH"`; 2 cores / 3.6 GB — batch cargo calls, generous timeouts, background+notify for long steps; debug builds for xvfb runs.
- Models: `ggml-tiny.bin` + its encoder package are installed in `~/.local/share/suramak/models/`; `xvfb-run` + LAVAPIpe are configured (M1 learned `LIBGL_ALWAYS_SOFTWARE=1` needs are handled by `scripts/xvfb-shot.sh` — read it).
- Fixtures: use `fixtures/audio/en.wav` (fox sentence) / `fixtures/audio/id.wav` — verify names before referencing.
- Known pre-existing limitation: `cargo check --target aarch64-apple-darwin` only works for pure-Rust crates (`suramak-glass`); the C-dep chain (rusqlite/whisper-rs-sys/ring) is built on the macOS CI runner. Do not fight it (see M1/M1.5 reports).

## Acceptance (run all, paste outputs into the report)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                                   # grew from 67; show the counts
cargo check -p suramak-glass --target aarch64-apple-darwin
cargo check -p suramak-glass --target x86_64-pc-windows-msvc
rm -f build/smoke_job_ok.txt
SURAMAK_SMOKE_SECS=120 SURAMAK_SMOKE_JOB=fixtures/audio/en.wav xvfb-run -a cargo run -p suramak-ui   # exit 0
test -f build/smoke_job_ok.txt && echo SMOKE-JOB-OK
cargo run --release -p suramak-cli -- library list        # MUST show the smoke job's recording (assert it)
SURAMAK_SMOKE_SECS=3 xvfb-run -a cargo run -p suramak-ui  # M1 plain smoke regression — still exit 0
SURAMAK_SMOKE_SECS=3 ./scripts/xvfb-shot.sh               # screenshot flow still works
```

Constraints: only files inside this repo; no sudo; **no git commit/push** (the parent verifies and commits). A truthful blocker beats a false green.
