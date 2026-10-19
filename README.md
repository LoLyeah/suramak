# Suramak

<img src="assets/logo/icons/readme-logo.png" alt="Suramak" width="360">

Local-first transcription app. Fully native, written in Rust — **no webview, no ffmpeg, no telemetry**.

[![CI](https://github.com/LoLyeah/suramak/actions/workflows/ci.yml/badge.svg)](https://github.com/LoLyeah/suramak/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/LoLyeah/suramak)](https://github.com/LoLyeah/suramak/releases/latest)

## Platforms

- ✅ **macOS (Apple Silicon)** — shipping since v0.1.1: `.dmg` + `.zip` on the [releases page](https://github.com/LoLyeah/suramak/releases/latest), Liquid Glass, Sparkle in-app updates.
- ✅ **Linux (x86_64)** — built and shipped in every release (`.tar.gz`); frosted-material fallback, no fake glass.
- 🚧 **Windows 11 (x86_64)** — the CLI is proven on CI (static CRT, no VC++ redistributable imports, English + Indonesian transcribes green); the GUI shell is in progress (M14).
- 🚧 **Android (arm64)** — the engine cross-compiles on the NDK (M13a green, arm64-v8a + x86_64); the shell spike and Go/No-Go gate is M13b.

English + Indonesian are the first-class languages; any Whisper language code works.

## Features

- On-device transcription (whisper.cpp via whisper-rs) — English + Indonesian first.
- Fast path on Apple Silicon: the Whisper encoder runs on the Neural Engine via the official Core ML encoder package (`--engine coreml`; `auto` picks it when installed).
- Import files + microphone recording, timestamped segments, editable transcript.
- **Multi-source recording** (M6): the record source picker is a **checkbox list** — tick two mics, or **mic + system audio** (the meeting-transcription combo), and the inputs are mixed into the one 16 kHz recording by a pure mixing engine (per-source gain/mute at record time; measured-clock drift correction keeps the sources aligned — never clicks, bounded over long runs). **Per-app capture**: "Only an app…" records just one running app's audio (CoreAudio process-tap mixdown), picked from a running-audio-processes list.
- **Live transcription while recording** (M7): a clearly-labeled **provisional** transcript grows in the record view during capture — the incoming stream is decoded in ~4 s windows (1.5 s context overlap) through the same whisper path as the session, behind a bounded drop-oldest queue with a skip-and-merge backlog guard (skipped windows are counted, logged and surfaced; live updates may lag on slow machines, the recording never does). On Stop the existing full-file pass always runs and **replaces** the provisional text — the saved transcript is byte-identical to the record-then-transcribe flow.
- Import **video** (`.mp4` / `.mov` / `.m4v` / `.mkv` / `.webm`) and **Opus** (`.opus` voice notes): the first audio track is transcribed through the identical pipeline — library row → transcribe → editor → exports (SRT/VTT become video subtitles). Opus (WebM / MKV / Ogg-Opus) decodes **natively** with libopus compiled into the app — no ffmpeg anywhere in the flow (M10C). Genuinely unsupported leftovers (WMA / AIFF / ALAC) keep the actionable conversion hint (`ffmpeg -i in.wma out.mp4`), never a raw decoder error (M5).
- In-app playback with segment follow: play/pause on the waveform strip, click-to-seek, and the active segment highlighted and kept in view while playing; a clean “no audio / no output device” state when either is missing.
- Floating record control: a bottom-center Liquid Glass pill on macOS 26+ (a real `NSGlassEffectView` behind the egui-drawn glyph and live `mm:ss` clock), an honest painted pill on Linux/Windows/older macOS; it is the record entry point — click to start, click to stop — and stays bottom-centered on resize (M8.5).
- Any whisper language code: `auto` / `en` / `id` presets plus a free-text **Custom…** entry in Settings (loose validation, keeps the last good value).
- Delete a transcript from the library: a trash button in the editor header with an inline confirm (segments go with it).
- Export Markdown / TXT / JSON (+ SRT / VTT) — take it to any LLM you like.
- Optional in-app "polish" via your own OpenAI-compatible endpoint — review side-by-side, accept/undo/discard; the app works fully without it. **Audio-aware polish (M11)**: an opt-in toggle (default OFF) that sends the recording itself alongside the transcript — OpenAI-compatible `input_audio` content parts (base64 WAV, no encoder), chunked at segment boundaries into ≤ 4 MB (~2 min) chunks, the corrected outputs concatenated in order — so corrections resolve names, homophones and jargon against what was actually said. Needs an audio-capable model (`gpt-audio` class, Gemini, Voxtral); a missing recording falls back to text-only with a note, and an endpoint that rejects the audio gets one automatic text-only retry with a note naming the reason. Off = byte-identical to the pre-M11 text-only request.
- No telemetry. Network only for model downloads, opt-in polish (the recording uploads only when you turn audio-aware polish on) and — in packaged macOS builds — the update check itself (the appcast on our GitHub Releases; nothing about you is sent).
- **In-app updates (M12, macOS)**: the packaged app checks for a new version in the background (about once a day; toggle "Automatic updates" in Settings, ON by default) and installs it **in place after your confirmation** — Sparkle 2's standard UI shows the release notes (from this repo's CHANGELOG) and asks before installing. A "Check for Updates…" item lives in the app menu. The feed is an EdDSA-signed `appcast.xml` attached to each GitHub release; dev builds (`cargo run`) and Linux/Windows builds have no updater. See **Updates** below.
- Liquid Glass on macOS 26+ (fallback for older); DWM Mica/Acrylic backdrop on Windows (compile-checked; runtime unverified).

## Status

- ✅ `suramak-core` — types, storage (sqlite), export, model download w/ sha256, decode, resample, polish client (incl. the M11 audio-aware request builder + segment-aligned chunker) — fully tested
- ✅ `suramak-asr` — whisper-rs pipeline + worker thread (record-then-transcribe), verified end-to-end; multi-source mixing engine (M6) compiled and tested on every platform; live partials (M7) — pure windowing/backlog scheduler + bounded queue + provisional merging on every platform, decode on its own thread through the same whisper path
- ✅ `suramak-cli` — `models` / `transcribe` / `export` / `library` / `polish` / `doctor`
- ✅ engine tiers — CPU / Metal / Core ML-ANE (`--engine`, `doctor` reports the stack; M1.5)
- ✅ `suramak-ui` — the app: import/record → transcript → edit → export, model manager, settings (incl. the M6 checkbox source list + per-app picker), live provisional transcript while recording (M7; the Mac-only live feel is a user-smoke item), in-app polish (M4; polish round-trip + all Mac-only behavior are user-smoke items)
- ✅ `suramak-taps` — macOS system-audio taps (CoreAudio 14.2+, incl. the M6 per-app `Only` mixdown + process listing); pure Rust, darwin-checked from Linux
- ✅ `suramak-glass` — AppKit materials (macOS) + real DWM backdrop (Windows, compile-checked from Linux) + Linux no-op
- ✅ in-app updates (M12) — Sparkle 2 behind the default-OFF `updates` feature: boot-time start on the main thread, event callbacks into the app log, Settings "Automatic updates" (ON) + a "Check for Updates…" app-menu item; pinned + sha256-verified framework fetch, embedded + ad-hoc-signed in `dist/Suramak.app`, EdDSA-signed appcast built by the release workflow. The whole update cycle is owner-verify (USER_SMOKE §32); the macOS module is type-checked from Linux against the real crate (the `build/probe-updates` probe)
- 📋 release handoff — `scripts/mac-smoke.sh` + `docs/USER_SMOKE.md` (run by the owner on the Mac)
- ✅ **`suramak-android` (M13a)** — the engine as an NDK cdylib: arm64-v8a release + x86_64 debug cross-build green against NDK r27d; `android-build` CI job runs on every push
- ✅ **Windows (M14a)** — platform seams (`cfg(windows)` paths/secrets/updates), `+crt-static` + MSVC static-runtime CMake shim; the `windows-2022` CI job is fully green: clippy, workspace tests, parakeet compile-check, release CLI, **dumpbin CRT check (no VCRUNTIME/MSVCP)**, doctor, English + Indonesian transcribes

## Layout

```
crates/suramak-core   portable domain logic (no UI, no whisper)
crates/suramak-asr    whisper-rs pipeline + worker (feature-gated mic capture) + pure multi-source mixer
crates/suramak-taps   macOS system-audio taps (CoreAudio 14.2+; per-app mixdown + process listing)
crates/suramak-glass  AppKit glass integration (objc2; incl. the M8.5 floating record-control surface) + DWM backdrop (Windows) + Linux no-op
crates/suramak-ui     the app window (winit + egui, no webview)
crates/suramak-playback  in-app playback engine (cpal output + transport state; M8)
crates/suramak-android   Android engine cdylib (NDK cross-build; M13)
crates/suramak-cli    headless CLI (also powers CI end-to-end tests)
```

Technical plan: `docs/PLAN.md` · verified engineering notes: `docs/PLAN-NOTES.md` · milestones: `docs/WORKORDER-*.md` · reports: `docs/reports/`.

## Build (dev)

macOS prerequisites: Xcode command line tools, Rust (rustup), and CMake (`brew install cmake`) — CMake is what builds whisper.cpp.

```bash
cargo build --release -p suramak-cli   # CLI
cargo build --release -p suramak-ui    # the app
./scripts/make-app.sh                  # macOS: dist/Suramak.app
bash scripts/mac-smoke.sh              # macOS: full release smoke → docs/USER_SMOKE.md
```

Models are downloaded on demand (`suramak-cli models download <id>`, sha256-pinned).

## Updates

In-app updates are **macOS-only** and live behind the default-OFF `updates`
cargo feature (the whole updater is compiled out of every other build).

What the packaged app does (once the feature is on and the owner has done the
one-time key setup below): Sparkle 2 checks the feed
(`https://github.com/LoLyeah/suramak/releases/latest/download/appcast.xml`)
in the background — roughly once a day, controlled by Settings → **Updates →
"Automatic updates"** (ON by default) — and, when a newer version exists,
shows its standard dialog with the release notes (the CHANGELOG section of
that release) and **installs in place only after you confirm**. The app menu
also gets a **"Check for Updates…"** item (greyed out while a check runs). The
app log (`RUST_LOG=suramak=debug`) records every step.

Build/release wiring:

```bash
./scripts/fetch-sparkle.sh          # pinned + sha256-verified Sparkle 2.9.6 (framework + tools)
SPARKLE_FRAMEWORK_PATH="$(./scripts/fetch-sparkle.sh)" \
  cargo build --release -p suramak-ui --features updates   # dev build (needs the framework on disk)
SURAMAK_FEATURES=speech-transcriber,updates ./scripts/make-app.sh   # the release bundle (embeds + signs Sparkle.framework)
```

- `scripts/fetch-sparkle.sh` pins the official 2.9.6 archive and its real
  sha256; the release/CI pipelines never download the framework any other way.
- `make-app.sh` copies the framework into `Contents/Frameworks` (symlinks
  preserved) before the existing ad-hoc signing; the
  `@executable_path/../Frameworks` rpath is a link-time argument
  (`crates/suramak-ui/build.rs`).
- The release workflow signs the archive and writes `appcast.xml` with
  Sparkle's `generate_appcast` (`scripts/make-appcast.sh`), using the
  `SPARKLE_ED_KEY` repo secret via `--ed-key-file -` (stdin, the documented CI
  path). Without that secret the release still ships and the appcast step logs
  a loud skip — the updater then finds no feed and stays dormant.

**One-time owner key setup** (no key material ever enters the repo):

1. On a Mac, from the fetched distribution (`build/sparkle/2.9.6`):
   `./bin/generate_keys` — generates the EdDSA keypair into your login
   keychain and prints the public key.
2. Put the printed public key into `packaging/Info.plist` as `SUPublicEDKey`
   (replacing the clearly-marked placeholder).
3. Export the private key and store it as the repo secret — the exported file
   *is* the base64 key string (`generate_keys -x` writes it in exactly the form
   `--ed-key-file -` expects), so its content goes in as-is:
   `./bin/generate_keys -x /tmp/sparkle_ed_key && pbcopy < /tmp/sparkle_ed_key`
   → GitHub → repo → Settings → Secrets and variables → Actions → new secret
   **`SPARKLE_ED_KEY`** → paste. Delete the local file afterwards. (Do **not**
   base64 the file again — a double-encoded value is the one common mistake,
   and the appcast step refuses it with a message saying exactly this.)

Exact commands, the expected first-cycle behavior and the honest boundaries
are in `docs/reports/M12.md`; the owner smoke is `docs/USER_SMOKE.md` §32.

## License

GPL-3.0-or-later — see [LICENSE](LICENSE).
