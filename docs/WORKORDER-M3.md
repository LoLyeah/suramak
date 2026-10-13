# WORKORDER M3 — recording + model manager + settings

Authoritative milestone: `docs/PLAN.md` §6 "M3". Read first: `docs/PLAN-NOTES.md` (corrections win), `docs/reports/M2.md` (current app; note its deviation 1: the `capture` feature was removed from `suramak-ui`'s target dep in M2 — M3 re-enables it), and the sources you'll call (`suramak-asr` worker + capture stub, `suramak-core` settings/download/keyring paths, M1's chrome-strip code in `app/views.rs`).

## Goal

The app gets its last functional pillar: **microphone recording (record-then-transcribe), an in-app model manager, and a settings screen** — within the M1 look. Recording: the chrome strip's red record dot (already drawn, M1) becomes live on platforms where capture is compiled; record → live RMS meter + rolling waveform painter → stop → wav (or temp+delete per `keep_audio`) → the existing worker pipeline → editor.

## Scope — do exactly this

1. **`suramak-asr::capture` (feature `capture`, OFF by default)** — cpal-backed: input device enumeration, stream start/stop, frame delivery. Pure helpers unit-tested headless (no device): mono mixdown, resample to 16 kHz (reuse core's rubato path), rolling RMS window, wav writer path. Device enumeration test behind `#[cfg(feature = "capture-test")]` (macOS/Windows only — Linux has no device in CI; the test must compile everywhere). `capture` must stay OFF in default builds so Linux needs no audio runtime — but it MUST compile on Linux (`libasound2-dev` is installed on this VPS now).
2. **Record flow in `suramak-ui`**: re-enable the target-gated `capture` feature dep (macOS/Windows). Record start/stop via the chrome-strip dot (visuals unchanged) + a recording view: auto title `Recording <date>`, elapsed time, live RMS meter and a simple rolling waveform painter (calm M1 style), Stop button. On stop: save to `<data>/recordings/` when `settings.keep_audio`, else temp dir + delete after the job completes (whichever way — document the choice). Then run the existing transcribe job flow (progress → editor). Record dot disabled while a job runs; on Linux default builds the dot stays hidden/disabled with a note (no capture compiled).
3. **Model manager view** (reachable from home): table from the manifest — id, size, installed / partial / missing, encoder availability (M1.5 helpers), default marker; actions: download (progress — read the real core download API for its progress callback), delete (respecting the shared-encoder rule from M1.5), set default (writes `settings.model_id`). All selection/action logic in egui-free modules with unit tests.
4. **Settings view**: engine (`auto|coreml|metal|cpu`), default model, default language (`auto|en|id`), keep_audio toggle, recordings folder display, polish section: base URL, model name, API key (keyring on macOS/Windows — `keyring` is already a target-gated dep in core; on Linux the key row is hidden with a note), Save + Reset to defaults. Persist via core `Settings` (toml), round-trip tested; toggling `keep_audio` must be observable in the settings file (PLAN's acceptance greps it).
5. **`suramak-cli devices`**: with `--features capture` lists input devices (name + default marker, exit 0); in default builds prints a clear "built without recording support (feature `capture`)" and exits 0.
6. **Tests** (suite is at 85): +capture mixer/resample/RMS, +record state machine headless (start/stop/timer/failure paths), +model-manager logic (install state, delete-with-shared-encoder, set-default persists), +settings round-trip/defaults. All headless-testable things get tests; device-dependent things are cfg-gated and are macOS CI/user territory.
7. **CI**: linux job — apt step gains `libasound2-dev`; new step `cargo check -p suramak-asr --features capture` (+ its unit tests: `cargo test -p suramak-asr --features capture`). macOS job — add `cargo test -p suramak-asr --features capture,capture-test` (device enumeration; do NOT open a stream — CI runners have no mic; the test asserts enumeration returns a list, possibly empty).
8. **Report** `docs/reports/M3.md` — build notes, deviations, full acceptance outputs, honest blockers (recording hardware E2E is the USER smoke on the Mac — that stays a user action).

## Do NOT

- Polish (LLM tidy) UI, VAD, live/chunked transcription, Windows work.
- Change M1 visuals or the M2 app structure beyond additive views/routes.
- Change core/asr public APIs except strictly-additive (document any).

## Environment (Linux VPS)

- `export PATH="$HOME/.cargo/bin:$PATH"`; 2 cores / 3.6 GB; batch cargo, generous timeouts, background+notify for long steps; debug builds for xvfb.
- **No microphone hardware here and none can be emulated** — recording runtime is macOS-only in practice; make everything that CAN be headless (mixer, resample, RMS, state machine) headless-tested, and say exactly what could not be exercised.
- `libasound2-dev` is installed (parent action) so `--features capture` compiles locally; `capture-test` enumeration on Linux may return an empty list — assert "returns a list", not "has devices".
- Per-command env prefixes only (`env XDG_DATA_HOME=... cmd`) — never session exports (leak rule from M2).
- Models dir has `ggml-tiny.bin` + encoder; smoke modes must keep working (plain + job).

## Acceptance (run all; paste outputs into the report)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                                   # grew from 85; show the counts
cargo check -p suramak-asr --features capture            # compiles on Linux now (ALSA)
cargo test -p suramak-asr --features capture             # pure capture tests
cargo run --release -p suramak-cli -- devices            # default build: clear message, exit 0
cargo run --release -p suramak-cli --features capture -- devices   # lists (empty ok), exit 0
cargo tree -p suramak-ui --target aarch64-apple-darwin | grep -m1 cpal   # present again (macOS target)
cargo check -p suramak-glass --target aarch64-apple-darwin && cargo check -p suramak-glass --target x86_64-pc-windows-msvc
SURAMAK_SMOKE_SECS=3 xvfb-run -a cargo run -p suramak-ui           # plain smoke regression
rm -f build/smoke_job_ok.txt && SURAMAK_SMOKE_SECS=120 SURAMAK_SMOKE_JOB=fixtures/audio/en.wav xvfb-run -a cargo run -p suramak-ui   # job smoke regression
# keep_audio observability (headless): toggle in a unit test's temp dir, then:
#   grep keep_audio <temp>/settings.toml  → matches the new value
```

Constraints: only files inside this repo; no sudo; **no git commit/push**. A truthful blocker beats a false green.

---

## Addendum A — audio source selection (user request, during build)

Settings gain **audio source selection**; recording uses it.

1. **Settings**: `Audio source` dropdown with entries: `Default microphone` (default), **each enumerated input device** (cpal; on macOS this includes virtual loopback devices like BlackHole — "other audio sources"), and `System audio` (macOS builds only; hidden elsewhere). Persist the choice; resolve at record time: default → system default input; named device → match enumerated devices by display name (case-insensitive; fall back to default with a visible warning); system audio → the tap path below. Unit-test the resolution logic (all branches, fallback, Linux error path for `SystemAudio`).
2. **System audio via CoreAudio process taps** (macOS 14.2+; macOS 26 fully supports; NO kernel drivers involved): new crate **`crates/suramak-taps`** — pure Rust (objc2 / CoreAudio externs; system-framework linkage only, NO new C build deps), mac-only code behind `cfg`, no-op stubs elsewhere so **`cargo check -p suramak-taps --target aarch64-apple-darwin` passes from Linux** (same checkability story as `suramak-glass`). API: create a system-audio tap (all processes except Suramak itself) + private aggregate device; return the CoreAudio device UID; `stop()`/Drop tears down aggregate-then-tap exactly once (idempotent). `suramak-asr` capture (feature `capture`) uses it when source = SystemAudio: open the tapped device through cpal by UID. Investigate `objc2-core-audio`-class bindings first; raw `objc2::msg_send`/`extern "C"` is acceptable. Document the exact API calls used with their availability annotations.
3. **Permissions/plist**: add the audio-capture usage description (`NSAudioCaptureUsageDescription`) to `packaging/Info.plist`; document the expected TCC prompt in the report (cannot be verified here — user smoke on the Mac; if the tap fails, the recording view must show a clear, actionable error, unit-tested on the Linux error path).
4. **Acceptance additions**: `cargo check -p suramak-taps --target aarch64-apple-darwin` (from Linux, must pass); `cargo check --workspace`; the source-resolution unit tests; CI linux job gains the taps darwin check step (pure Rust, like the glass one). Existing acceptance block unchanged.
5. **Do NOT** attempt ScreenCaptureKit in this milestone (process taps are the chosen route; SCK stays a documented fallback). Per-app capture ("only this Zoom call") is explicitly a future milestone — but design the tap API so a process-list variant can slot in later (one parameter, not a reshape).
