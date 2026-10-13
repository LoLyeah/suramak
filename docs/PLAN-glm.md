# Suramak — Implementation Plan

Target: fully-local transcription app. Rust, native rendering, macOS-first (Apple Silicon, macOS 26), Windows-ready architecture. Executors work headless on Linux; the end user smoke-tests on a real Mac. Every milestone has automatable acceptance checks runnable on Linux CI.

---

## 1. Stack decision

**Primary: egui (immediate mode) driven manually via `egui-winit` + `egui-wgpu` on `winit` — NOT `eframe`.**
**Fallback: iced 0.13 with the identical glass technique (same winit/wgpu foundation).**

### Why

1. **Linux dev preview + native macOS**: egui renders via wgpu (Metal on macOS, GL/Vulkan on Linux). The exact same UI code runs on the headless VPS under `xvfb` and natively on macOS. No webview anywhere.
2. **Glass compositing**: we own the `winit` window, so we can reach the `NSWindow` via `raw-window-handle` and perform AppKit surgery. `eframe` is rejected specifically because it does not expose the underlying `winit` window (long-standing limitation), which blocks the glass integration. iced shares the winit/wgpu stack, so the fallback inherits the same glass code path.
3. **Custom drawing**: egui's `Painter` gives per-widget custom drawing (waveform, custom titlebar, floating panels, animations via `ctx.request_repaint`) with zero framework resistance. Immediate mode is ideal for AI executors: no retained-mode state graphs to fight.
4. **Text editing**: `egui::TextEdit::multiline` is mature enough for per-segment transcript editing (selection, undo, IME). iced's multi-line editing is weaker — a major reason it is the fallback, not the primary.
5. **Maintainability**: egui is boring, heavily documented, single-crate mental model, no async runtime required.

Rejected: **GPUI** (not on a stable release cadence for external users, sparse docs, Linux support immature — too risky for autonomous executors); **raw objc2/AppKit** (zero Linux preview — fails the hard dev-environment constraint).

### Glass crates (exact lines)

```toml
# crates/suramak-glass/Cargo.toml
objc2 = "0.6.2"
objc2-foundation = { version = "0.3.1", features = ["NSGeometry", "NSObject"] }
objc2-app-kit = { version = "0.3.1", features = ["NSApplication", "NSColor", "NSResponder", "NSView", "NSVisualEffectView", "NSWindow"] }
raw-window-handle = "0.6"
winit = { version = "0.30", default-features = false, features = ["rwh_06"] } # rwh_06 exposes ns_window

[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.62", features = ["Win32_Foundation", "Win32_Graphics_Dwm", "Win32_UI_WindowsAndMessaging"] }
```

UNCERTAIN: whether `objc2-app-kit 0.3.1` (generated from an SDK ≤ macOS 15) exposes `NSGlassEffectView`. **Fallback (planned as the primary path anyway): runtime class lookup + raw `msg_send!`**, shown below. `NSGlassEffectContainerView` (for floating panels) is post-MVP, same technique.

### macOS glass integration — exact call sequence

Executed once, on the main thread, immediately after `winit` creates the window and **before** the first frame is presented:

1. `window.window_handle()` (rwh 0.6) → `AppKitWindowHandle::ns_window` (`*mut c_void`).
2. `unsafe { Id::retain(ptr as *mut NSWindow) }` → `Retained<NSWindow>`.
3. Make the window non-opaque: `setOpaque(false)`, `setBackgroundColor(Some(&NSColor::clearColor()))`.
4. Custom chrome: `setTitlebarAppearsTransparent(true)`, `setStyleMask(styleMask | FullSizeContentView)`, `setTitleVisibility(Hidden)` — native traffic lights stay, our egui titlebar draws under/around them.
5. Build the effect view sized to `contentView().bounds()`:
   - macOS 26+: `Class::get("NSGlassEffectView")` → `Some` → instantiate via raw `msg_send![cls, new]`, `msg_send![&obj, setFrame: rect]` (treat as `Retained<NSObject>`; no wrapper type needed).
   - else: `NSVisualEffectView::new()`; `setBlendingMode(BehindWindow)`, `setMaterial(UnderWindowBackground)`, `setState(Active)`, `setFrame(bounds)`.
6. **Re-parent so glass sits BEHIND the render surface** (subviews always composite above their parent's layer, so we cannot just add the effect view below the winit view — we swap the content view):
   ```rust
   let winit_view = ns_window.contentView().unwrap();      // winit's NSView (hosts the CAMetalLayer)
   glass.setAutoresizingMask(ViewWidthSizable | ViewHeightSizable);
   ns_window.setContentView(Some(&glass));                 // glass becomes content view
   glass.addSubview(&winit_view);                          // winit view (wgpu surface) now on top
   winit_view.setAutoresizingMask(ViewWidthSizable | ViewHeightSizable);
   ```
7. Configure the wgpu surface for transparency: pick `CompositeAlphaMode::PostMultiplied` if present in `surface.get_capabilities(&adapter).alpha_modes`, else `PreMultiplied`. Clear color `{0,0,0,0}` every frame.
8. In egui, never paint an opaque root: `CentralPanel::default().frame(egui::Frame::none())`, translucent widget fills (e.g. `Color32::from_white_alpha(24)`), so the OS material shows through.

Window dragging: top 44 px strip in egui; when pointer is down there and not over an interactive widget, call `window.drag_window()` (winit). All AppKit code lives in `suramak-glass`; the UI crate calls `suramak_glass::apply(&winit_window, config)` behind `#[cfg(target_os = "macos")]`.

**Windows (stub now, real later)**: `suramak-glass/src/windows.rs` calls `DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &DWMSBT_TRANSIENTWINDOW /*acrylic*/, 4)`; compile-checked via cross-target `cargo check`, not exercised in MVP.

**Linux dev preview**: `suramak-glass` is a no-op; the UI paints an opaque dark background. Transparency is not validated on Linux — that is the user smoke test's job.

---

## 2. ASR pipeline

**Flow**: `capture/decode → mono mixdown → resample to 16 kHz f32 → whisper-rs → segments → Transcript`.

- **Capture (decision: cpal)**: `cpal = "0.15.3"`. On macOS cpal wraps CoreAudio — native enough, and it keeps the pipeline identical on Windows later. Platform-native capture (AVAudioEngine) buys nothing for MVP and costs a second code path. Default input device, default stream config (f32), samples accumulated in a `Vec<f32>` with a live RMS meter for the UI.
- **File import**: `symphonia = "0.5.4"` (pure Rust, no system deps, runs on Linux CI) with features `["aac","flac","isomp4","mp3","vorbis","wav"]`. Accepted: wav, mp3, flac, ogg/vorbis, m4a/mp4 (AAC). `hound = "3.5"` for wav write (recordings). UNCERTAIN: some MOV/MKV/WebM/Opus files will fail to decode → show a clear "convert to wav/mp3/m4a" error; optional `ffmpeg-sidecar` integration is post-MVP.
- **Resample**: `rubato = "0.16"` (feature `fft_resampler`) to 16 kHz mono. Sinc/FFT quality; deterministic and unit-testable (sine sweep).
- **whisper**: `whisper-rs = "0.16"`, feature `metal` on macOS (whisper.cpp Metal), CPU elsewhere. Build requires `cmake` + a C++ toolchain on CI (noted in §7). Params per job: `SamplingStrategy::Greedy`, `set_language` (user: Auto / en / id), `set_n_threads` (2 on CI, physical cores on Mac), all `set_print_*` off, `set_token_timestamps(true)`, optional `set_initial_prompt`. Segments via `full_n_segments` / `full_get_segment_tstamp(i, start|end)` (ms) / `full_get_segment_text(i)`. Rely on whisper.cpp's built-in entropy temperature fallback (on by default) for Indonesian robustness.
- **VAD (decision: none in MVP)**. Record-then-transcribe makes VAD unnecessary; whisper's internal segmentation handles silence. Post-MVP, if live chunked transcription is added, evaluate whisper.cpp's Silero VAD — UNCERTAIN whether whisper-rs 0.16 exposes it; fallback is fixed 3 s chunks without VAD.
- **Recording strategy (decision: record-then-transcribe for MVP)**. Best accuracy, simplest code, no merge artifacts. Chunked live transcription is post-MVP.
- **Threading**: one dedicated worker thread (`std::thread`, named `suramak-worker`) in `suramak-asr::worker`. UI/CLI → `mpsc::Sender<Job>`; worker → `mpsc::Sender<WorkerEvent>` (`Progress{job_id,pct}`, `Segments{job_id,Vec<Segment>}`, `Finished{job_id,Result<TranscriptId>}`, `Error`). Whisper progress callback (`FullParams::set_progress_callback`) forwards 0–100 through the channel (sender passed as `*mut c_void` user data, reconstructed in the C callback). Whisper contexts cached per model in the worker (`HashMap<ModelId, WhisperContext>`); only the worker thread touches them. UI polls `try_recv()` each frame.
- **Models** (HuggingFace repo `ggeraganov/whisper.cpp`, file `ggml-<id>.bin`):

| id | file | size | role |
|---|---|---|---|
| `tiny` | ggml-tiny.bin | 78 MB | CI + tests only |
| `base` | ggml-base.bin | 148 MB | optional |
| `small` | ggml-small.bin | 488 MB | "fast" user option |
| `turbo-q5-0` | ggml-large-v3-turbo-q5_0.bin | 574 MB | **default on macOS** |
| `turbo` | ggml-large-v3-turbo.bin | 1.6 GB | accuracy option |

  Rationale: large-v3-turbo q5_0 is near-large accuracy at ~8× speed, well within Apple Silicon + Metal realtime. URL: `https://huggingface.co/ggerakanov/whisper.cpp/resolve/main/<file>` — precisely `https://huggingface.co/ggeraganov/whisper.cpp/resolve/main/<file>`. Mirror: same path on `https://hf-mirror.com`, selected via `SURAMAK_HF_MIRROR` env or settings field. Downloader (in `suramak-core`, `reqwest` blocking, rustls): stream to `<dest>.part`, resume via `Range` header if `.part` exists, verify pinned **sha256** (`sha2`), then rename. Manifest pins sha256 per model; `scripts/pin-model-hashes.sh` (curl + `sha256sum`) fills them in the first PR — executors must not invent hashes.

---

## 3. Workspace layout

```
suramak/                        # cargo workspace
├── crates/suramak-core         # pure-Rust domain: types, storage, export, settings, download, decode, polish. No UI, no whisper, no cpal.
├── crates/suramak-asr          # whisper-rs + cpal + worker thread + pipeline. Depends on core.
├── crates/suramak-glass        # objc2/AppKit glass + window chrome; DWM stub. Depends only on objc2/winit/rwh. Pure Rust → cross-checkable.
├── crates/suramak-ui           # egui app (bin: suramak). Depends on core, asr, glass.
└── crates/suramak-cli          # headless CLI (bin: suramak-cli). Depends on core, asr.
```

Dependency direction (arrows = "depends on"): `core ← asr`, `core+asr+glass ← ui`, `core+asr ← cli`. `glass` is referenced by `ui` only via `#[cfg]`. `core` and `glass` contain zero C-compiling dependencies except rusqlite-bundled in core (see §7 for why the aarch64 cross-check targets `glass` only).

CLI surface (clap 4 derive):
`suramak-cli models list|download <id>|remove <id>` · `transcribe <input> [--model id|path] [--lang en|id|auto] [--initial-prompt S] [--formats md,txt,json,srt,vtt] [--out-dir D] [--db]` · `library list` · `export <id> --format F --out FILE` · `polish <id> [--base-url U] [--model M]` · `devices` · `doctor`.

---

## 4. Data model + storage

**Decision: rusqlite (sqlite, `bundled`) over JSON files.** Incremental segment inserts during transcription, WAL autosave without whole-file rewrites, SQL search over the library, one boring proven crate. JSON remains the *export* format.

```rust
pub struct Transcript { pub id: Uuid, pub title: String, pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>, pub source: Source, pub model: String,
    pub language: Option<String>, pub audio_path: Option<PathBuf>, pub segments: Vec<Segment> }
pub struct Segment { pub idx: u32, pub start_ms: i64, pub end_ms: i64, pub text: String }
pub enum Source { File { name: String }, Recording { started_at: DateTime<Utc>, duration_ms: i64 } }

pub struct Settings { pub asr: AsrSettings, pub polish: Option<PolishSettings>,
    pub storage: StorageSettings /* keep_audio: bool */, pub ui: UiSettings /* glass: Auto|Always|Never, font_scale */ }
pub struct AsrSettings { pub model_id: String, pub language: LangMode /* Auto | Fixed(String) */,
    pub initial_prompt: Option<String>, pub translate_to_english: bool }
pub struct PolishSettings { pub base_url: String /* default https://api.openai.com/v1 */, pub model: String }
```

Schema (v1, `store.rs`):

```sql
PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);            -- schema_version=1
CREATE TABLE transcripts(id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, source_json TEXT NOT NULL, model TEXT NOT NULL,
  language TEXT, audio_path TEXT);
CREATE TABLE segments(transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
  idx INTEGER NOT NULL, start_ms INTEGER NOT NULL, end_ms INTEGER NOT NULL, text TEXT NOT NULL,
  PRIMARY KEY(transcript_id, idx));
```

**Paths** (`directories = "5"`, `ProjectDirs::from("dev", "suramak", "Suramak")` → `data_dir`): macOS `~/Library/Application Support/Suramak/`, Windows `%APPDATA%\Suramak\`, Linux `~/.local/share/Suramak/`. Files: `suramak.db`, `settings.toml`, `models/ggml-*.bin`, `audio/<uuid>.wav` (recordings, if `keep_audio`). **Autosave**: transcript row inserted when a job starts; segments upserted on completion; text edits debounced 500 ms (`ctx.request_repaint_after` + dirty flag) → `Store::update_segment_text`.

**Export schemas (exact)** — timestamps `HH:MM:SS.mmm` via `core::time::fmt_*`:

- **JSON**: `{"app":"suramak","schema":1,"id":"<uuid>","title":"…","created_at":"RFC3339","source":{"type":"file","name":"intro.mp3"},"model":"turbo-q5-0","language":"id","segments":[{"id":0,"start_ms":0,"end_ms":2400,"start":"00:00:00.000","end":"00:00:02.400","text":"…"}]}`
- **Markdown**: `# {title}` then list block `- Date: …` `- Source: …` `- Model: …` `- Language: …`, blank line, `## Transcript`, then one line per segment: `[00:00:00.000 → 00:00:02.400] text`, blank line between segments.
- **TXT**: one line per segment: `[00:00:00] text`.
- **SRT**: standard — index, `HH:MM:SS,mmm --> HH:MM:SS,mmm`, text, blank.
- **VTT**: `WEBVTT` header, then `HH:MM:SS.mmm --> HH:MM:SS.mmm`, text, blank.

---

## 5. LLM polish

```rust
pub trait PolishBackend {
    fn polish(&self, segments: &[Segment], cfg: &PolishSettings) -> Result<Vec<Segment>, PolishError>;
}
pub struct OpenAiCompat { http: reqwest::blocking::Client } // timeout 120 s
```

- **Batch (non-streaming) for MVP**; streaming post-MVP. Endpoint: `POST {base_url}/chat/completions`, body `{"model": M, "temperature": 0, "messages":[{"role":"system","content":SYS},{"role":"user","content":"<segments JSON>"}]}`. No `response_format` (max endpoint compatibility); the parser tolerates ```` ```json ```` fences.
- **System prompt (exact)**: `You are a transcript cleanup engine. You receive JSON: {"segments":[{"id":int,"text":string}]}. Return ONLY JSON of the same shape with the same ids in the same order. Fix punctuation, capitalization, spacing, and obvious misrecognitions. Merge words broken mid-word. Do not add, remove, summarize, or translate content. Keep the original language.`
- **Validation**: returned ids must equal input ids (same set, same order); texts non-empty; else `PolishError::BadResponse` carrying the raw body. UI shows a before/after diff with Accept / Revert.
- **API key (decision: OS keychain via `keyring = "3"` on macOS/Windows; env var `SURAMAK_OPENAI_API_KEY` everywhere as fallback and as the only path on Linux)** — avoids dbus/secret-service on headless Linux. Keyring is a target-specific dependency; `core::keychain` is cfg-gated. Service `"Suramak"`, account `"polish"`.
- The app is fully functional with `polish: None` — the settings screen and menu entry are simply hidden.

---

## 6. Milestones M0–M4

### M0 — bare-minimum slice: Linux CLI, wav → transcript → export (first PR)

**Goal**: the entire portable pipeline, proven by tests on Linux, no UI.
**Deliverables**: `suramak-core` (types, time, paths, decode, resample, models manifest, downloader, store, export, settings, polish, keychain stub), `suramak-asr` (whisper.rs, pipeline, worker), `suramak-cli` (all subcommands except `devices`), fixtures, CI.
**Acceptance** (all on Linux):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings        # exit 0
cargo test --workspace                                        # all green; whisper e2e is #[ignore]d (debug whisper is ~50× too slow)
cargo build --release -p suramak-cli
cargo run --release -p suramak-cli -- models download tiny    # exit 0; file at ~/.local/share/Suramak/models/ggml-tiny.bin; sha256 matches manifest
cargo run --release -p suramak-cli -- transcribe fixtures/en_fox.wav --model tiny --lang en --formats json,md --out-dir build/out
test "$(jq '.segments|length' build/out/en_fox.json)" -ge 1   # passes
jq -r '.segments[].text' build/out/en_fox.json | grep -qi fox # passes
cargo run --release -p suramak-cli -- transcribe fixtures/id_fox.wav --model tiny --lang id --formats json --out-dir build/out
jq -r '.segments[].text' build/out/id_fox.json | grep -q .    # non-empty
cargo test --workspace --release -- --ignored                 # whisper e2e (en + id fixtures, tiny model)
```

Fixtures are committed once (see §7); CI never generates speech.
**Do NOT**: any UI, any objc2/glass, cpal, keyring, recording, Windows code.

### M1 — window + Liquid Glass shell

**Goal**: `suramak-ui` opens a transparent egui window over real AppKit glass on macOS; plain opaque window on Linux; `suramak-glass` crate complete with fallbacks.
**Deliverables**: winit 0.30 `ApplicationHandler` loop, manual egui-winit + egui-wgpu wiring, transparent surface config, glass application sequence (§1), custom titlebar + drag strip, smoke mode (`SURAMAK_SMOKE_SECS=N` → run N s, write `build/smoke_ok.txt`, exit 0), `scripts/make-app.sh` + `packaging/Info.plist`.
**Acceptance**:

```bash
rustup target add aarch64-apple-darwin
cargo check -p suramak-glass --target aarch64-apple-darwin     # exit 0 (pure-Rust objc2; no Apple SDK needed for check)
rustup target add x86_64-pc-windows-msvc
cargo check -p suramak-glass --target x86_64-pc-windows-msvc   # exit 0 (DWM stub)
sudo apt-get install -y xvfb libgl1-mesa-dri
SURAMAK_SMOKE_SECS=3 xvfb-run -a cargo run -p suramak-ui       # exit 0
test -f build/smoke_ok.txt                                     # passes
# macOS (CI macos-14 job and/or user):
cargo build --release -p suramak-ui && ./scripts/make-app.sh   # dist/Suramak.app exists
SURAMAK_SMOKE_SECS=3 ./dist/Suramak.app/Contents/MacOS/suramak # exit 0
```

**Do NOT**: transcription in UI, widgets beyond titlebar/home placeholder, chasing NSGlassEffectView *visuals* on CI (impossible — runners are macOS 15; runtime fallback path is what CI exercises).

### M2 — transcription in the UI + library + export

**Goal**: drag-drop/file-dialog import → worker job → progress → editable transcript → sqlite library → export.
**Deliverables**: `app/` modules (home, transcribe, editor, library, export), winit `WindowEvent::DroppedFile` handling, `rfd` save dialogs (macOS/Windows only; Linux uses a path field), segment editor (timestamp label + `TextEdit` per segment, autosave), scripted smoke job (`SURAMAK_SMOKE_JOB=fixtures/en_fox.wav` runs the full in-app pipeline headless and asserts the DB row).
**Acceptance**:

```bash
cargo test --workspace                                  # includes headless UI state-machine tests
SURAMAK_SMOKE_SECS=20 SURAMAK_SMOKE_JOB=fixtures/en_fox.wav xvfb-run -a cargo run -p suramak-ui  # exit 0
cargo run --release -p suramak-cli -- library list | grep -q "$(basename fixtures/en_fox.wav)"
```

Plus user smoke on Mac: import a real mp3, watch progress, edit a segment, export md.
**Do NOT**: recording, polish UI, model-download UI (CLI covers it), Windows.

### M3 — recording + model manager + settings

**Goal**: microphone recording (record-then-transcribe), in-app model download with progress, settings screen persisted to `settings.toml`, keychain key entry.
**Deliverables**: `asr::capture` (cpal), record UI with RMS meter + waveform painter, model manager (list/download/delete/default), settings screen, `suramak-cli devices`.
**Acceptance**: `cargo test --workspace` (capture mixer/resample unit tests; cpal device enumeration behind `#[cfg(feature = "capture-test")]`, run only on macOS); user runs `suramak-cli devices` (lists input device, exit 0) and records 5 s in the app → transcript appears; `grep keep_audio ~/.local/share/Suramak/settings.toml` after toggling.
**Do NOT**: polish UI, VAD, live chunked transcription, Windows.

### M4 — polish + Windows stubs + packaging

**Goal**: LLM polish end-to-end, Windows architecture locked in via compile-checked stubs, shippable .app.
**Deliverables**: polish worker job + diff/accept UI, `polish` CLI, `windows.rs` DWM backdrop implementation (compile-checked only), `scripts/mac-smoke.sh`, `docs/USER_SMOKE.md`.
**Acceptance**: `cargo test --workspace` (polish client tested against a hand-rolled `TcpListener` mock server returning canned OpenAI JSON — zero extra deps); `cargo check -p suramak-glass --target x86_64-pc-windows-msvc`; `cargo run --release -p suramak-cli -- polish <id>` against the mock in a test; full `scripts/mac-smoke.sh` run by the user (exit 0).

---

## 7. Verification & CI

**Test inventory** (all in-crate `tests/` + unit tests):

- `core`: export golden-string tests (exact md/txt/json/srt/vtt output for a fixed 3-segment transcript); time formatting; store roundtrip + cascade delete on `tempfile` dir; settings toml roundtrip; downloader resume + sha256 (hand-rolled `TcpListener` HTTP server with `Range` support, ~40 lines, deterministic); polish client (mock server: success, fenced JSON, 500, timeout, id-mismatch).
- `asr`: resampler (440 Hz sine 44.1 k → 16 k, dominant frequency within ±2%); mono mixdown; whisper e2e `#[ignore]` + `--release` only (tiny model, both fixtures).
- `ui`: headless state-machine tests (job lifecycle, autosave debounce logic, route switching) — UI logic lives in pure modules with no egui types where possible.
- `cli`: `std::process::Command` integration tests invoking the release binary.

**Model in CI**: `ggml-tiny.bin` (78 MB) fetched by `suramak-cli models download tiny` and cached with `actions/cache@v4` (path `~/.local/share/Suramak/models/ggml-tiny.bin`, key = pinned sha256). Never download larger models in CI.

**Cross-check feasibility (answer: yes, narrowly)**: `cargo check --target aarch64-apple-darwin` from Linux works only for crates with no target-C build scripts. `suramak-glass` qualifies (objc2 crates are pure Rust, pregenerated bindings; check does not link). `suramak-core` (rusqlite-bundled compiles sqlite3.c) and `suramak-asr` (whisper.cpp via cmake) do not — they are checked/built on the macOS runner instead. Same logic for `x86_64-pc-windows-msvc` + `windows` crate.

**`.github/workflows/ci.yml`** (outline):

- **job `linux`** (ubuntu-latest): `apt-get install -y cmake xvfb libgl1-mesa-dri`; `rustup target add aarch64-apple-darwin x86_64-pc-windows-msvc`; steps: fmt → clippy (`-D warnings`) → `cargo test --workspace` → `cargo check -p suramak-glass --target aarch64-apple-darwin` → `cargo check -p suramak-glass --target x86_64-pc-windows-msvc` → restore model cache / download tiny → release build + CLI e2e (M0 acceptance block) → `cargo test --workspace --release -- --ignored` → `xvfb` UI smoke. Use `Swatinem/rust-cache@v2`.
- **job `macos`** (macos-14, arm64; UNCERTAIN whether `macos-26` runners exist — switch the label when they do): `cargo build --release --workspace`; `cargo test --workspace --release -- --ignored`; `./scripts/make-app.sh`; `SURAMAK_SMOKE_SECS=3 ./dist/Suramak.app/Contents/MacOS/suramak`; upload `dist/Suramak.app` via `actions/upload-artifact@v4`.

**VPS notes**: 2 cores / 3.6 GB — CI runners are bigger; local VPS runs the same linux job minus release e2e if slow. Disk: shared `target/` ≈ 4–6 GB; run `cargo clean` between milestones if under 10 GB free. No LTO anywhere (`[profile.release]` defaults, `lto = false` explicit) to keep link memory low; if rustc OOMs: `CARGO_BUILD_JOBS=1`.

**Fixtures** (generated once by the orchestrator, committed — CI never synthesizes speech):

```bash
sudo apt-get install -y espeak-ng
espeak-ng -w fixtures/en_fox.wav -s 140 "The quick brown fox jumps over the lazy dog"
espeak-ng -v id -w fixtures/id_fox.wav -s 140 "Seekor rubah cokelat melompat di atas anjing yang malas"
```

**User Mac smoke script** (`scripts/mac-smoke.sh`, exact):

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
git pull --ff-only
cargo build --release --workspace
./scripts/make-app.sh                                   # builds dist/Suramak.app, ad-hoc codesign
SURAMAK_SMOKE_SECS=3 ./dist/Suramak.app/Contents/MacOS/suramak
test -f build/smoke_ok.txt && echo "SMOKE_WINDOW_OK"
./dist/Suramak.app/Contents/MacOS/suramak-cli doctor    # exit 0
open dist/Suramak.app
echo "MANUAL: record 5 s, verify transcript, export .md, then press Enter"
read -r _
ls ~/Library/Application\ Support/Suramak/suramak.db >/dev/null && echo "SMOKE_DB_OK"
```

`packaging/Info.plist` must contain `NSMicrophoneUsageDescription = "Suramak uses the microphone to record audio for on-device transcription."`, `LSMinimumSystemVersion = 13.0`, `CFBundleIdentifier = dev.suramak.app`.

---

## 8. Risks & mitigations (top 8)

1. **`NSGlassEffectView` absent from objc2-app-kit 0.3.1** → primary path is runtime `Class::get` + raw `msg_send!` (works regardless); fallback `NSVisualEffectView` (`UnderWindowBackground`, `BehindWindow`, `Active`). Cannot be visually verified on macOS 15 CI runners → user smoke on macOS 26 is the acceptance gate (M1).
2. **Transparent compositing shows black/opaque window** (alpha-mode mismatch) → `PostMultiplied` (fallback `PreMultiplied`) chosen from `surface.get_capabilities().alpha_modes`; clear alpha 0; `Frame::none()` root; Linux preview cannot validate the Metal path → user smoke. If re-parenting the winit view breaks resize, mitigation: autoresizing masks on both views (in the code); last resort: keep winit view as content view and place glass in a `NSVisualEffectContainerView`-style sibling window — defer, flag in M1.
3. **whisper.cpp build on 2-core/3.6 GB CI** → cmake + rust-cache cached; tiny model only; e2e strictly `--release`; `set_n_threads(2)`; no LTO; `CARGO_BUILD_JOBS=1` if OOM.
4. **Debug-profile whisper is unusably slow** → all whisper-touching tests are `#[ignore]` + `cargo test --release -- --ignored`; documented in test headers so executors don't "fix" it.
5. **espeak fixtures flaky for ASR assertions** → fixtures committed (deterministic bytes); en assertion `grep -qi fox`; id assertion non-empty only. If tiny-model/transcode drift breaks "fox", relax to non-empty + `segments ≥ 1` and record it in the PR — do not chase whisper accuracy in CI.
6. **Mic permission / TCC with unsigned app** → `NSMicrophoneUsageDescription` in Info.plist from M1; ad-hoc `codesign --sign -` in `make-app.sh` (stable per binary); user may see the mic prompt again after each rebuild — accepted; proper Developer ID signing explicitly deferred (post-MVP).
7. **Indonesian transcription quality** → `--lang id` fixed (not auto), Indonesian default `initial_prompt` ("Berikut adalah transkrip audio dalam bahasa Indonesia."), whisper.cpp entropy temperature fallback left enabled, default model `turbo-q5-0` (not small). UNCERTAIN: whisper-rs exposure of temperature params — rely on whisper.cpp defaults; do not hand-roll fallback loops in MVP.
8. **symphonia container gaps (mkv/webm/opus, some MOV)** → accepted-input list surfaced in UI + error message "convert to wav/mp3/m4a"; `ffmpeg-sidecar` optional post-MVP. (Related, folded here: keyring on headless Linux needs dbus → env-var-only on Linux by design.)

---

## 9. MVP file tree + M0 checklist

```
Cargo.toml                        # workspace + [workspace.dependencies] + [profile.release] lto=false
rust-toolchain.toml               # pinned stable (1.88.0; bump only if a dep demands)
fixtures/en_fox.wav               # committed ASR fixture (en)
fixtures/id_fox.wav               # committed ASR fixture (id)
packaging/Info.plist              # .app metadata incl. NSMicrophoneUsageDescription
scripts/make-app.sh               # release build → dist/Suramak.app → ad-hoc codesign
scripts/mac-smoke.sh              # user smoke test (§7)
scripts/pin-model-hashes.sh       # downloads models once, emits sha256 lines for manifest.rs
scripts/gen-fixtures.sh           # espeak-ng commands (§7), run once, outputs committed
docs/USER_SMOKE.md                # step-by-step Mac verification for the end user
.github/workflows/ci.yml          # linux + macos jobs (§7)
crates/suramak-core/Cargo.toml
crates/suramak-core/src/lib.rs
crates/suramak-core/src/error.rs        # SuramakError (thiserror), single error enum
crates/suramak-core/src/paths.rs        # ProjectDirs data/models/audio/config paths
crates/suramak-core/src/time.rs         # ms ↔ "HH:MM:SS.mmm" / srt / vtt formatting
crates/suramak-core/src/transcript.rs   # Transcript/Segment/Source types (serde)
crates/suramak-core/src/models.rs       # ModelManifest: id, hf file, size, sha256, role
crates/suramak-core/src/download.rs     # resume+Range+sha256 downloader (reqwest blocking)
crates/suramak-core/src/decode.rs       # symphonia/hound → AudioBuffer{rate,chans,samples}
crates/suramak-core/src/resample.rs     # rubato → 16 kHz mono f32
crates/suramak-core/src/store.rs        # rusqlite Store: CRUD, search, WAL
crates/suramak-core/src/export.rs       # md/txt/json/srt/vtt writers
crates/suramak-core/src/settings.rs     # Settings + toml load/save
crates/suramak-core/src/polish.rs       # PolishBackend trait + OpenAiCompat client
crates/suramak-core/src/keychain.rs     # cfg: keyring (mac/win) / env var (linux)
crates/suramak-core/tests/export.rs     # golden-string export tests
crates/suramak-core/tests/store.rs      # sqlite roundtrip tests
crates/suramak-core/tests/download.rs   # mock-server resume/checksum tests
crates/suramak-core/tests/polish.rs     # mock-server polish tests
crates/suramak-asr/Cargo.toml
crates/suramak-asr/src/lib.rs
crates/suramak-asr/src/whisper.rs       # context cache, FullParams, segment extraction
crates/suramak-asr/src/pipeline.rs      # decode→resample→whisper→Transcript
crates/suramak-asr/src/capture.rs       # cpal input stream → samples + RMS meter
crates/suramak-asr/src/worker.rs        # Job/WorkerEvent mpsc + worker thread
crates/suramak-asr/tests/pipeline.rs    # #[ignore] release e2e (fixtures, tiny)
crates/suramak-glass/Cargo.toml
crates/suramak-glass/src/lib.rs         # apply(window, config) + cfg dispatch + GlassMode
crates/suramak-glass/src/macos.rs       # objc2 sequence of §1 (glass, titlebar, drag)
crates/suramak-glass/src/windows.rs     # DWMWA_SYSTEMBACKDROP_TYPE (compile-checked stub)
crates/suramak-glass/src/stub.rs        # Linux no-op
crates/suramak-ui/Cargo.toml
crates/suramak-ui/src/main.rs           # winit ApplicationHandler, surface setup, smoke mode
crates/suramak-ui/src/app/mod.rs        # AppState, routes, worker wiring, event pump
crates/suramak-ui/src/app/home.rs       # drop zone, import button, record button, recents
crates/suramak-ui/src/app/transcribe.rs # progress bar + live segments
crates/suramak-ui/src/app/editor.rs     # segment editor, timestamps, autosave
crates/suramak-ui/src/app/library.rs    # transcript list, search, open/delete
crates/suramak-ui/src/app/export.rs     # format picker + save (rfd on mac/win)
crates/suramak-ui/src/app/settings.rs   # model manager, language, polish config, key
crates/suramak-ui/src/app/waveform.rs   # painter waveform widget
crates/suramak-ui/src/theme.rs          # glass palette, fonts (bundled Inter OFL)
crates/suramak-ui/src/platform.rs       # cfg calls into suramak_glass
crates/suramak-ui/assets/fonts/Inter-Regular.ttf, Inter-Medium.ttf
crates/suramak-cli/Cargo.toml
crates/suramak-cli/src/main.rs          # clap derive, subcommand dispatch
crates/suramak-cli/src/cmd/models.rs    # list/download/remove
crates/suramak-cli/src/cmd/transcribe.rs# file → transcript → export/db
crates/suramak-cli/src/cmd/export.rs    # library id → file
crates/suramak-cli/src/cmd/library.rs   # list
crates/suramak-cli/src/cmd/polish.rs    # headless polish
crates/suramak-cli/src/cmd/doctor.rs    # dirs/db/model/config checks
crates/suramak-cli/src/cmd/devices.rs   # cpal input enumeration (M3)
```

**M0 checklist (first PR)**: workspace root + `rust-toolchain.toml`; full `suramak-core` (all modules above) + its 4 test files; `suramak-asr` (whisper/pipeline/worker; `capture.rs` may be an empty `#[cfg(feature="capture")]` stub); `suramak-cli` (models/transcribe/export/library/doctor; `devices` stubbed); both fixtures committed; `scripts/pin-model-hashes.sh` run with its output pasted into `models.rs`; `.github/workflows/ci.yml` linux job. Acceptance = the M0 command block in §6, all green. Nothing else.

---

## 10. Appendix — exact dependency lines

```toml
# root Cargo.toml
[workspace]
members = ["crates/suramak-core", "crates/suramak-asr", "crates/suramak-glass", "crates/suramak-ui", "crates/suramak-cli"]
resolver = "2"
[workspace.package]
edition = "2021"; rust-version = "1.85"; version = "0.1.0"
[profile.release]
lto = false            # 2-core/3.6GB link safety; whisper.cpp has its own CFLAGS

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }            # data model everywhere
serde_json = "1"                                            # export JSON, polish wire format
toml = "0.8"                                                # settings.toml
uuid = { version = "1", features = ["v4", "serde"] }         # transcript ids
chrono = { version = "0.4", features = ["serde"] }           # timestamps, RFC3339
rusqlite = { version = "0.32", features = ["bundled"] }      # storage; bundled = no system sqlite
directories = "5"                                           # per-OS app-data paths
thiserror = "2"                                             # core error enums
anyhow = "1"                                                # CLI error context only
tracing = "0.1"                                             # structured logs
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
reqwest = { version = "0.12", default-features = false, features = ["blocking", "json", "rustls-tls"] } # model download + polish; rustls = no openssl, cross-check friendly
sha2 = "0.10"                                               # model checksums
symphonia = { version = "0.5.4", features = ["aac", "flac", "isomp4", "mp3",
`"vorbis", "wav"] }                                          # pure-Rust decode (wav/mp3/flac/ogg/m4a); zero system deps → runs on headless Linux CI
rubato = { version = "0.16", features = ["fft_resampler"] } # sinc/FFT resample to 16 kHz mono; deterministic, unit-testable
hound = "3.5"                                               # wav writer for recordings (decode goes through symphonia)
clap = { version = "4", features = ["derive"] }             # suramak-cli argument parsing
pollster = "0.4"                                            # block_on for wgpu adapter/surface setup; no async runtime anywhere
egui = "0.33"                                               # UI: immediate mode, Painter custom drawing, mature multiline TextEdit
egui-winit = "0.33"                                         # winit → egui input bridge (manual integration; eframe rejected, §1)
egui-wgpu = "0.33"                                          # egui → wgpu renderer; Metal on macOS, GL/Vulkan on Linux
winit = { version = "0.30", features = ["rwh_06"] }         # window + raw NSWindow handle for glass; defaults keep x11/wayland for the xvfb smoke run
raw-window-handle = "0.6"                                   # rwh 0.6 handle traits (matches winit's rwh_06)
whisper-rs = "0.16"                                         # whisper.cpp bindings; `metal` feature merged in on macOS (see suramak-asr)
```

Rules for the table:

- **Never add `wgpu` as a direct dependency.** Consume it via the `egui_wgpu::wgpu` re-export; this eliminates wgpu version skew between egui-wgpu and our surface code.
- The workspace `winit` line supersedes the §1 sketch (which showed `default-features = false` for glass): defaults are kept so the Linux `xvfb` run gets X11/Wayland backends. Cargo feature unification guarantees a single winit across `suramak-glass` and `suramak-ui`.
- Target-gated and optional deps (`cpal`, `keyring`, `rfd`, `objc2*`, `windows`) cannot live in `[workspace.dependencies]` — they are declared per-crate below.
- Versions are floors (`"0.33"` = latest 0.33.x). Bumps happen only in a dedicated dependency-bump PR; `cargo update` is forbidden inside feature PRs. Toolchain pinned to 1.88.0 via `rust-toolchain.toml`; MSRV 1.85.

### crates/suramak-core/Cargo.toml

```toml
[dependencies]
serde = { workspace = true }             # Transcript/Segment/Source + export JSON
serde_json = { workspace = true }        # export writer, polish wire format
toml = { workspace = true }              # settings.toml load/save
uuid = { workspace = true }              # transcript ids
chrono = { workspace = true }            # timestamps, RFC3339
rusqlite = { workspace = true }          # storage; bundled = no system sqlite, cross-check friendly
directories = { workspace = true }       # per-OS app-data paths (§4)
thiserror = { workspace = true }         # SuramakError
tracing = { workspace = true }           # structured logs
reqwest = { workspace = true }           # model download + polish client; rustls = no openssl anywhere
sha2 = { workspace = true }              # model checksum verification
symphonia = { workspace = true }         # audio/video decode
rubato = { workspace = true }            # 16 kHz resample
hound = { workspace = true }             # wav writer

[target.'cfg(any(target_os = "macos", target_os = "windows"))'.dependencies]
keyring = { version = "3", features = ["apple-native", "windows-native"] } # OS keychain for the polish API key; Linux uses SURAMAK_OPENAI_API_KEY (§5)

[dev-dependencies]
tempfile = "3"                           # throwaway data dirs for store/download/polish tests
```

Mock HTTP servers in `tests/download.rs` / `tests/polish.rs` are hand-rolled `std::net::TcpListener` loops (~40 lines each) — deliberately zero extra dev-dependencies.

### crates/suramak-asr/Cargo.toml

```toml
[dependencies]
suramak-core = { path = "../suramak-core" }
whisper-rs = { workspace = true }        # base (CPU) build; Linux CI + fallback
thiserror = { workspace = true }
tracing = { workspace = true }
serde = { workspace = true }             # Job / WorkerEvent wire types
serde_json = { workspace = true }

[features]
capture = ["dep:cpal"]                   # mic capture; OFF by default so Linux builds never need ALSA headers (libasound2-dev)

[dependencies.cpal]
version = "0.15.3"
optional = true                          # CoreAudio on macOS / WASAPI on Windows through one API

[target.'cfg(target_os = "macos")'.dependencies]
whisper-rs = { workspace = true, features = ["metal"] } # whisper.cpp Metal backend; features merge with the base declaration
```

Build requirements: `cmake` + a C++ toolchain (whisper.cpp is compiled from source by whisper-rs's build script). CI installs `cmake` on Linux; the macOS runner has clang via Xcode CLT. No ALSA is needed on Linux because `capture` is off there.

### crates/suramak-glass/Cargo.toml

```toml
[dependencies]
winit = { workspace = true }             # Window type for apply(); never creates windows itself
raw-window-handle = { workspace = true } # ns_window / HWND extraction

[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6.2"                          # runtime Class::get + msg_send! for NSGlassEffectView
objc2-foundation = { version = "0.3.1", features = ["NSGeometry", "NSObject"] } # NSRect/NSMakeRect for setFrame:
objc2-app-kit = { version = "0.3.1", features = ["NSApplication", "NSColor", "NSResponder", "NSView", "NSVisualEffectView", "NSWindow"] } # fallback material + window surgery (§1)

[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.62", features = ["Win32_Foundation", "Win32_Graphics_Dwm", "Win32_UI_WindowsAndMessaging"] } # DWMWA_SYSTEMBACKDROP_TYPE acrylic/mica (stub, §1)
```

Notes:

- The objc2 lines from §1 are consolidated here under the macOS target table — they cannot compile on Linux, and this placement is what makes `cargo check -p suramak-glass --target aarch64-apple-darwin` from Linux work (objc2 crates are pure Rust with pregenerated bindings; `check` does not link).
- `NSGlassEffectView` needs no objc2-app-kit feature flag because it is reached via runtime `Class::get("NSGlassEffectView")` + raw `msg_send!` (Risk 1).
- objc2-app-kit features are per-header. If a symbol (e.g. a `NSWindowStyleMask` constant) is missing at compile time, add its feature flag — the cross-target `cargo check` in CI catches this within one commit; never work around it with raw `msg_send!` for symbols that have wrappers.

### crates/suramak-ui/Cargo.toml

```toml
[dependencies]
suramak-core = { path = "../suramak-core" }
suramak-asr = { path = "../suramak-asr" }
suramak-glass = { path = "../suramak-glass" }
egui = { workspace = true }
egui-winit = { workspace = true }
egui-wgpu = { workspace = true }         # wgpu consumed via egui_wgpu::wgpu re-export only
winit = { workspace = true }
pollster = { workspace = true }          # block_on for surface creation
tracing = { workspace = true }
tracing-subscriber = { workspace = true }

[target.'cfg(any(target_os = "macos", target_os = "windows"))'.dependencies]
suramak-asr = { path = "../suramak-asr", features = ["capture"] } # mic capture enabled only where cpal is platform-native
rfd = "0.15"                                                       # native save dialogs; Linux export uses a path field (avoids zbus/portal deps)
```

Fonts are compiled in via `include_bytes!("assets/fonts/Inter-Regular.ttf")` / `Inter-Medium.ttf` and registered as `egui::FontData` in `theme.rs` — no system-font dependency, identical metrics on Linux preview and macOS.

### crates/suramak-cli/Cargo.toml

```toml
[dependencies]
suramak-core = { path = "../suramak-core" }
suramak-asr = { path = "../suramak-asr" }
clap = { workspace = true }
anyhow = { workspace = true }            # CLI-only error context; core uses thiserror
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
serde_json = { workspace = true }        # pretty-printed transcript output
```

### System-package recap (CI + VPS)

Linux needs exactly: `cmake` (whisper.cpp), `xvfb` + `libgl1-mesa-dri` (wgpu software GL for the UI smoke run). It never needs: ALSA (capture gate), openssl (rustls), sqlite (bundled), dbus/secret-service (keyring is macOS/Windows-only). macOS runners need nothing beyond the default toolchain.

### Open UNCERTAIN items (both have committed fallbacks)

1. `objc2-app-kit 0.3.1` predates macOS 26, so `NSGlassEffectView` has no wrapper — mitigated by design: runtime `Class::get` + `msg_send!` is the primary path (§1), `NSVisualEffectView` is the runtime fallback, and visual verification is the user smoke test on macOS 26 (M1).
2. GitHub may not offer `macos-26` runners yet — CI uses `macos-14` (arm64) for build/test/artifact; switch the label to the macOS 26 runner the week it appears. Nothing else changes.

— End of plan —