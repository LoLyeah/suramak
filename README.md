# Suramak

<img src="assets/logo/icons/readme-logo.png" alt="Suramak" width="360">

Local-first transcription app. Fully native, written in Rust. **macOS first; Windows and Linux ports to follow.**

- On-device transcription (whisper.cpp via whisper-rs) — English + Indonesian first.
- Fast path on Apple Silicon: the Whisper encoder runs on the Neural Engine via the official Core ML encoder package (`--engine coreml`; `auto` picks it when installed).
- Import files + microphone recording, timestamped segments, editable transcript.
- Export Markdown / TXT / JSON (+ SRT / VTT) — take it to any LLM you like.
- Optional in-app "polish" via your own OpenAI-compatible endpoint — the app works fully without it.
- No telemetry. Network only for model downloads and opt-in polish.
- Liquid Glass on macOS 26+ (fallback for older); frosted glass on Windows later.

## Status

- ✅ `suramak-core` — types, storage (sqlite), export, model download w/ sha256, decode, resample, polish client — fully tested
- ✅ `suramak-asr` — whisper-rs pipeline + worker thread (record-then-transcribe), verified end-to-end
- ✅ `suramak-cli` — `models` / `transcribe` / `export` / `library` / `polish` / `doctor`
- ✅ engine tiers — CPU / Metal / Core ML-ANE (`--engine`, `doctor` reports the stack; M1.5)
- 🚧 `suramak-ui` + `suramak-glass` — the native window & Liquid Glass shell (in progress)

## Layout

```
crates/suramak-core   portable domain logic (no UI, no whisper)
crates/suramak-asr    whisper-rs pipeline + worker (feature-gated mic capture)
crates/suramak-glass  AppKit glass integration (objc2) + Windows DWM stub + Linux no-op
crates/suramak-ui     the app window (winit + egui, no webview)
crates/suramak-cli    headless CLI (also powers CI end-to-end tests)
```

Technical plan: `docs/PLAN.md` · verified engineering notes: `docs/PLAN-NOTES.md` · milestones: `docs/WORKORDER-*.md`.

## Build (dev)

```bash
cargo build --release -p suramak-cli   # CLI
cargo build --release -p suramak-ui    # the app
./scripts/make-app.sh                  # macOS: dist/Suramak.app
```

Models are downloaded on demand (`suramak-cli models download <id>`, sha256-pinned).

## License

GPL-3.0-or-later — see [LICENSE](LICENSE).
