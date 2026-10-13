# Suramak

<img src="assets/logo/icons/readme-logo.png" alt="Suramak" width="360">

Local-first transcription app. Fully native, written in Rust. **macOS first; Windows and Linux ports to follow.**

- On-device transcription (whisper.cpp via whisper-rs) — English + Indonesian first.
- Fast path on Apple Silicon: the Whisper encoder runs on the Neural Engine via the official Core ML encoder package (`--engine coreml`; `auto` picks it when installed).
- Import files + microphone recording, timestamped segments, editable transcript.
- **Multi-source recording** (M6): the record source picker is a **checkbox list** — tick two mics, or **mic + system audio** (the meeting-transcription combo), and the inputs are mixed into the one 16 kHz recording by a pure mixing engine (per-source gain/mute at record time; measured-clock drift correction keeps the sources aligned — never clicks, bounded over long runs). **Per-app capture**: "Only an app…" records just one running app's audio (CoreAudio process-tap mixdown), picked from a running-audio-processes list.
- **Live transcription while recording** (M7): a clearly-labeled **provisional** transcript grows in the record view during capture — the incoming stream is decoded in ~4 s windows (1.5 s context overlap) through the same whisper path as the session, behind a bounded drop-oldest queue with a skip-and-merge backlog guard (skipped windows are counted, logged and surfaced; live updates may lag on slow machines, the recording never does). On Stop the existing full-file pass always runs and **replaces** the provisional text — the saved transcript is byte-identical to the record-then-transcribe flow.
- Import **video** (`.mp4` / `.mov` / `.m4v` / `.mkv`): the first audio track is transcribed through the identical pipeline — library row → transcribe → editor → exports (SRT/VTT become video subtitles). Unsupported audio (e.g. WebM/Opus) fails with an actionable conversion hint (`ffmpeg -i in.webm out.mp4`), never a raw decoder error (M5).
- In-app playback with segment follow: play/pause on the waveform strip, click-to-seek, and the active segment highlighted and kept in view while playing; a clean “no audio / no output device” state when either is missing.
- Floating record control: a bottom-center Liquid Glass pill on macOS 26+ (a real `NSGlassEffectView` behind the egui-drawn glyph and live `mm:ss` clock), an honest painted pill on Linux/Windows/older macOS; it is the record entry point — click to start, click to stop — and stays bottom-centered on resize (M8.5).
- Any whisper language code: `auto` / `en` / `id` presets plus a free-text **Custom…** entry in Settings (loose validation, keeps the last good value).
- Delete a transcript from the library: a trash button in the editor header with an inline confirm (segments go with it).
- Export Markdown / TXT / JSON (+ SRT / VTT) — take it to any LLM you like.
- Optional in-app "polish" via your own OpenAI-compatible endpoint — review side-by-side, accept/undo/discard; the app works fully without it.
- No telemetry. Network only for model downloads and opt-in polish.
- Liquid Glass on macOS 26+ (fallback for older); DWM Mica/Acrylic backdrop on Windows (compile-checked; runtime unverified).

## Status

- ✅ `suramak-core` — types, storage (sqlite), export, model download w/ sha256, decode, resample, polish client — fully tested
- ✅ `suramak-asr` — whisper-rs pipeline + worker thread (record-then-transcribe), verified end-to-end; multi-source mixing engine (M6) compiled and tested on every platform; live partials (M7) — pure windowing/backlog scheduler + bounded queue + provisional merging on every platform, decode on its own thread through the same whisper path
- ✅ `suramak-cli` — `models` / `transcribe` / `export` / `library` / `polish` / `doctor`
- ✅ engine tiers — CPU / Metal / Core ML-ANE (`--engine`, `doctor` reports the stack; M1.5)
- ✅ `suramak-ui` — the app: import/record → transcript → edit → export, model manager, settings (incl. the M6 checkbox source list + per-app picker), live provisional transcript while recording (M7; the Mac-only live feel is a user-smoke item), in-app polish (M4; polish round-trip + all Mac-only behavior are user-smoke items)
- ✅ `suramak-taps` — macOS system-audio taps (CoreAudio 14.2+, incl. the M6 per-app `Only` mixdown + process listing); pure Rust, darwin-checked from Linux
- ✅ `suramak-glass` — AppKit materials (macOS) + real DWM backdrop (Windows, compile-checked from Linux) + Linux no-op
- 📋 release handoff — `scripts/mac-smoke.sh` + `docs/USER_SMOKE.md` (run by the owner on the Mac)

## Layout

```
crates/suramak-core   portable domain logic (no UI, no whisper)
crates/suramak-asr    whisper-rs pipeline + worker (feature-gated mic capture) + pure multi-source mixer
crates/suramak-taps   macOS system-audio taps (CoreAudio 14.2+; per-app mixdown + process listing)
crates/suramak-glass  AppKit glass integration (objc2; incl. the M8.5 floating record-control surface) + DWM backdrop (Windows) + Linux no-op
crates/suramak-ui     the app window (winit + egui, no webview)
crates/suramak-playback  in-app playback engine (cpal output + transport state; M8)
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

## License

GPL-3.0-or-later — see [LICENSE](LICENSE).
