# Planning request: Suramak — native, local-first transcription app

You are the planning engineer. Produce a complete, decisive engineering plan in Markdown for the project below. The plan will be executed by autonomous coding agents (Google Antigravity CLI, strong coding models) with no further context, coordinated by an orchestrating agent. Be concrete: exact crate names + versions + feature flags, exact file paths, exact commands. No hedging, no marketing language, no questions back. Where something is genuinely uncertain, mark it UNCERTAIN with a fallback. Output ONLY the plan document.

## Product

**Suramak** — a fully local (on-device) transcription app.

- Transcribe audio: import audio/video files (drag & drop + file dialog) AND record from microphone.
- Languages: English + Indonesian primarily (Whisper is multilingual; UI copy in English for now).
- Editable transcript view with segment timestamps; transcript library (history of past transcripts).
- Export: Markdown, plain text, JSON (and SRT/VTT as a cheap bonus) — users can then take exported files to any LLM they prefer to tidy up.
- Optional in-app "polish": user configures an OpenAI-compatible endpoint (base URL, API key, model) and the app can send the transcript for cleanup/formatting. The app must work fully without any LLM configured.
- No telemetry. Network used only for: whisper model download, and the optional user-configured LLM polish call.

## Hard requirements

- Written in Rust, native rendering — NOT a webview/Electron/Tauri app.
- macOS first (Apple Silicon). Windows port immediately after (same codebase); Windows implementation is NOT in MVP but the architecture must not preclude it (cfg stubs allowed).
- macOS UI must adopt Apple's official Liquid Glass: `NSGlassEffectView` / `NSGlassEffectContainerView` (macOS 26+), graceful fallback to `NSVisualEffectView` on older macOS. Real OS material, with the app's own Rust-rendered content on top (transparent window, non-opaque layer).
- Windows (later): real frosted glass via DWM backdrop (`DWMWA_SYSTEMBACKDROP_TYPE` acrylic/mica), custom client area.
- A design process will review the UI, so the architecture must support high-polish custom UI (custom window chrome, animations, drag regions) — do not pick a framework that fights custom design.

## Environment facts (critical for architecture + verification strategy)

- Dev/CI machine: headless Linux VPS, 2 cores, 3.6 GB RAM, 30 GB free disk. GitHub Actions available (ubuntu + macOS runners). No physical Mac for the dev team: the END USER builds and smoke-tests on their Mac (Apple Silicon, macOS 26); CI compiles macOS targets too.
- Therefore: the portable core must compile and pass tests on Linux. Platform-specific UI code must be cfg-gated so Linux builds never need the Apple SDK.
- Executors are autonomous agents working headlessly; every milestone must have automatable acceptance checks (commands + expected output). Prefer boring, proven crates.
- ASR: use whisper-rs (bindings to whisper.cpp) 0.16+ with `metal` feature on macOS; state model choices (e.g. ggml-large-v3-turbo vs medium vs small), download UX, and whether/how to use VAD (whisper.cpp built-in VAD, Silero).
- Audio capture: evaluate cpal vs platform-native capture; pick one for MVP.
- Consider whether `cargo check --target aarch64-apple-darwin` from Linux is feasible for syntax-checking macOS code (needs `rustup target add`; linking not required for check) — if feasible, include it in CI/local verification.

## Key decision to make (with rationale): UI stack

Choose ONE primary + ONE fallback from: egui (immediate-mode; transparent window + AppKit glass behind content), iced, GPUI, direct objc2/AppKit (fully hand-rolled AppKit UI via objc2 + objc2-app-kit), or a hybrid. Criteria:

1. Can render on Linux for dev preview (the only machine the team can run UI on) while being fully native on macOS (no webview).
2. Can place its content over a real `NSGlassEffectView`/`NSVisualEffectView` (window transparency + material behind rendered surface must work reliably) — spell out the exact call sequence (NSWindow configuration, layer backing, objc2 crates and which classes, fallback path).
3. Enough custom-drawing control for an "impeccable" glass UI: custom titlebar, floating panels, waveform, smooth scrolling lists, transcript text editing.
4. Text editing maturity (transcript editing is core).
5. Long-term maintainability for a small team of AI executors.

## Deliverables (sections)

1. **Stack decision** (primary + fallback + why; windows/rendering/glass crates with exact Cargo.toml lines; the macOS glass integration call sequence).
2. **ASR pipeline**: capture → resample → whisper-rs → segments; VAD; threading (dedicated worker thread, progress channel); recording strategy (chunked live transcription vs record-then-transcribe — pick for MVP); model management (which models, where stored, resume/checksum, HuggingFace URLs + mirrors).
3. **Workspace layout**: cargo workspace, crate names, module files with one-line responsibilities, dependency direction (core ← platform ← ui), plus a CLI binary (`suramak-cli`) for headless pipeline testing.
4. **Data model + storage**: Rust structs for Transcript/Segment/Settings; storage backend (rusqlite vs JSON files — pick); per-OS app-data paths; autosave policy; export schemas: exact shape of .md, .txt, .json (field names), srt/vtt if included.
5. **LLM polish**: trait + OpenAI-compatible client (reqwest); settings (base_url/model in config, API key in OS keychain via `keyring` crate? — decide), prompt template, error handling, streaming vs batch for MVP.
6. **Milestones M0–M4**: each with goal, deliverables, exact acceptance checks (commands + expected results), and what NOT to do. Point out the true bare-minimum slice (M0): Linux CLI wav → transcript → export, verified by tests.
7. **Verification & CI**: test list (unit + integration; tiny-model end-to-end test strategy and where CI gets the model); GitHub Actions workflow outline (jobs, runners); remote smoke-test script for the user's Mac (exact commands).
8. **Risks & mitigations**: top 8, concrete (e.g. NSGlassEffectView availability via objc2 — maybe needs a raw `class!` declaration; whisper.cpp build on CI; 2-core build times; OOM; mic permission strings (NSMicrophoneUsageDescription); code signing deferred; Indonesian transcription quality knobs such as initial_prompt/temperature fallback; transparent window + renderer compositing pitfalls).
9. **MVP file tree** (every file, one-line purpose) + **M0 checklist** (first PR).
10. **Appendix**: exact Cargo.toml dependency lines for every crate in the workspace (name, version, features, why).

Assume the reader knows Rust and can look up APIs, but knows nothing else about this project. Keep it under ~6000 words. Markdown only.
