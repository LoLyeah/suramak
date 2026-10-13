# WORKORDER M4 — polish UI + Windows DWM + release handoff

Authoritative milestone: `docs/PLAN.md` §6 "M4". Read first: `docs/PLAN-NOTES.md` (corrections win), `docs/reports/M2.md` + `docs/reports/M3.md` (current app + deviations), `docs/ENGINES.md` (the "Mac manual check" section you will reference from the handoff docs), the existing core polish module and its mock-server tests, `crates/suramak-glass/src/windows.rs` (the M1 stub you will make real), and `scripts/make-app.sh`.

## Goal

Three closers, all that remains before the MVP finish line:
1. **LLM "polish" usable in-app** — worker job + review/accept UI (the core client + CLI command already exist since M0).
2. **Windows frosted glass locked in** — real DWM backdrop code in `suramak-glass`, compile-checked only (no runtime claims).
3. **Release handoff** — `scripts/mac-smoke.sh` + `docs/USER_SMOKE.md` so the owner can run the full Mac smoke end-to-end.

## Scope — do exactly this

### 1. Polish in-app
- Editor gains a **Polish** action (enabled only when polish is configured — base URL + key present, key via `keyring` on macOS/Windows / `SURAMAK_OPENAI_API_KEY` on Linux, per M3); otherwise a note explaining how to configure it in Settings.
- Run the polish call on a **background thread** (core's client is blocking HTTP; do not use the ASR worker). Report running/failure via the existing event/bridge pattern.
- **Review UI**: original vs tidied transcript side-by-side per segment; actions **Accept** (replace segment text, persist via the store, keep the previous text for a one-step **Undo**), **Discard**. Batch only — no streaming.
- egui-free state machine + headless tests: request lifecycle, failure surfaces, accept → persist → undo, discard, polish with n segments where the reply drops/merges segments (define + test the rule: fall back to keeping original segments and showing the raw reply error).
- The UI polish flow must be testable headless against a mock endpoint (reuse the pattern from core's polish tests; no new deps).

### 2. Windows DWM in `suramak-glass`
- Implement `windows.rs` for real: `DwmSetWindowAttribute` — `DWMWA_SYSTEMBACKDROP_TYPE` (acrylic `DWMSBT_TRANSIENTWINDOW` / mica `DWMSBT_MAINWINDOW`), `DWMWA_USE_IMMERSIVE_DARK_MODE`, plus the minimal transparency/titlebar flags winit needs. Public `apply()` API stays identical to the macOS path; cfg-gating unchanged.
- Must pass `cargo check -p suramak-glass --target x86_64-pc-windows-msvc` **from Linux** (pure-Rust `windows` crate — same checkability story as the darwin side). Document exact DW attribute values in comments. Compile-check only; no runtime claims.

### 3. Release handoff
- `scripts/mac-smoke.sh` (runs on the Mac from repo root; plain bash, no sudo): release build → `make-app.sh` (icon included) → bundle smoke (`SURAMAK_SMOKE_SECS=3`) → `suramak-cli doctor` → CLI e2e on `fixtures/audio/en.wav` → print a pointer to `docs/USER_SMOKE.md` → `open dist/Suramak.app`. Fails loudly on any step failing. `bash -n` clean.
- `docs/USER_SMOKE.md` — numbered checklist for the owner on the Mac, every item with expected observable result: Core ML engine engaged (reference `docs/ENGINES.md` §Mac manual check), record 5 s from mic → transcript appears, **system audio source** (tap creation + the TCC prompt path; expected prompt text; behavior if denied), import an mp3, edit + autosave persists, export md, polish (if configured), model manager (download/delete/set default), settings roundtrip, **the app icon shows in Finder/Dock** (new since M1), keep_audio on/off behavior, and a "things to report" section. Short lines, no prose.
- README: status block reflects M4 (polish usable in-app, Windows path compile-checked, handoff docs); keep it one screen.

### 4. Tests / CI / report
- Suite grows from 123 (paste the real counts). All existing steps stay green, including: smokes (plain + job), capture check/tests, taps darwin check, glass darwin+windows checks.
- CI: no new jobs needed unless the polish UI joins an existing step; keep the YAML valid.
- `docs/reports/M4.md`: what was built, deviations with reasons, full acceptance outputs, honest blockers (polish round-trip against a REAL endpoint and every Mac-only behavior remain user-smoke items).

## Do NOT

- Video import, file associations, per-app capture, FLAC recording toggle, Icon Composer layered icons (all M5+; recorded in PRODUCT.md roadmap).
- UI localization, VAD, streaming.
- Change M1 visuals / M2 structure beyond additive (polish view, actions, status lines).
- Change core/asr public APIs except strictly-additive (document any).

## Environment (Linux VPS)

- `export PATH="$HOME/.cargo/bin:$PATH"`; 2 cores / 3.6 GB; batch cargo, generous timeouts, background+notify for long steps; debug for xvfb.
- **Per-command env prefixes only** (`env XDG_DATA_HOME=... cmd`) — never session exports (the shell is shared with the parent).
- No Mac, no mic, no tap runtime here — compile checks + headless tests are the acceptance; say exactly what remains user-smoke.
- Models + fixtures as before; smoke modes must keep working.

## Acceptance (run all; paste outputs into the report)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                                   # grew from 123
cargo check -p suramak-glass --target aarch64-apple-darwin
cargo check -p suramak-glass --target x86_64-pc-windows-msvc      # now the REAL DWM code
cargo check -p suramak-taps --target aarch64-apple-darwin
cargo check -p suramak-asr --features capture && cargo test -p suramak-asr --features capture
bash -n scripts/mac-smoke.sh
SURAMAK_SMOKE_SECS=3 xvfb-run -a cargo run -p suramak-ui
rm -f build/smoke_job_ok.txt && SURAMAK_SMOKE_SECS=120 SURAMAK_SMOKE_JOB=fixtures/audio/en.wav xvfb-run -a cargo run -p suramak-ui && test -f build/smoke_job_ok.txt
# polish: headless UI test against the mock, plus the CLI against the same mock (core's test pattern)
```

Constraints: only files inside this repo; no sudo; **no git commit/push**. A truthful blocker beats a false green.
