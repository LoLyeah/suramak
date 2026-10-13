# Product

<!-- impeccable:product-schema 1 -->

## Platform

adaptive

Desktop-first in practice: macOS 26 (Apple Silicon) is the primary target and stays first priority; Windows and Linux ports follow (Linux doubles as the dev-preview surface today and becomes a release target later). One product, OS-native design language: Apple Liquid Glass on macOS; frosted glass (DWM backdrop) on Windows; on Linux, a calm native look without OS glass until a real system material exists to adopt. Mobile references are intentionally not applicable.

## Stack

User-mandated: Rust, fully native (no webview/Electron). Cargo workspace; egui (winit + wgpu) custom-drawn UI; objc2 AppKit glass integration; whisper-rs (whisper.cpp) on-device ASR; rusqlite storage. Authoritative engineering plan: `docs/PLAN.md` + verified corrections in `docs/PLAN-NOTES.md`.

## Users

Primary: the owner — a MacBook (Apple Silicon) user who records and collects audio in English and Indonesian and wants transcripts without sending audio to any cloud.

*Assumption note (labeled): the init interview question was skipped, so audience facts beyond "me first, English + Indonesian, macOS" are inferences from the original brief, not confirmed answers.* Secondary (assumed, unconfirmed): Indonesian users with the same local-first need.

## Product Purpose

Transcribe audio fully on-device: import audio/video files or record from the microphone, get an editable timestamped transcript, export it (Markdown / TXT / JSON, plus SRT/VTT). An optional, user-configured "polish" step can tidy results through the user's own OpenAI-compatible endpoint; the app is fully functional with no LLM at all. Success = daily-usable, private, fast transcription for en+id.

## Positioning

Everything runs locally: audio never leaves the machine unless the user explicitly exports it or opts into a polish call to an endpoint they own. A native Liquid Glass macOS app written in Rust — no webview, no cloud ASR, no telemetry.

## Operating Context

Daily capture/import of en+id speech on macOS; transcripts exported to md/txt/json and often handed to the user's preferred LLM for cleanup outside the app. A Windows port follows the macOS release. Development happens on a headless Linux box + CI; the owner smoke-tests builds on the Mac.

## Capabilities and Constraints

Capabilities (MVP scope): file import (wav/mp3/flac/ogg/m4a), microphone recording (record-then-transcribe), on-device transcription with explicit language selection (Auto / English / Indonesian, plus any custom whisper language code the user types — offered, not officially supported) and a default prompt that teaches the brand word, editable timestamped segments, transcript library (sqlite, with delete), exports md/txt/json/srt/vtt, model manager (download with checksum + resume).

Constraints: Rust, native rendering, macOS Liquid Glass as a hard requirement; no telemetry; network only for model downloads and opt-in polish. Polish API keys live in the OS keychain (env-var fallback on Linux dev).

Undecided (do not invent): distribution model (private / open source / paid), UI localization beyond English.

## Brand Commitments

Name: **Suramak** (user-given). No meaning, voice, logo, or palette has been confirmed — do not invent an origin story or identity. The brand word must be spelled correctly by the transcriber (default initial_prompt includes it).

## Evidence on Hand

No testimonials, benchmarks, or screenshots. Measured, verifiable facts exist in `docs/PLAN-NOTES.md` §4 (VPS transcription measurements) — use those; fabricate nothing else.

## Product Principles

1. Local by default — network is used only for model downloads and opt-in polish calls.
2. Native fidelity — real OS materials (Liquid Glass, DWM backdrop) and real system behaviors; never fake chrome in a way the system would not.
3. Export is first-class — the user's data always leaves in open, portable formats.
4. Indonesian is a first-class language, not an afterthought — forced-language support, sensible id defaults, quality-first model tier.
5. Standalone by design — every cloud/LLM feature is optional and user-owned.
6. Best local engine per language — different languages may run different on-device engines (English can be faster than Indonesian; both stay on-device). Engine routing is a product feature, not an accident.

## Roadmap (post-MVP, agreed direction)

- **Video files (M5)** — this completes the original brief ("import audio/video files"): import `.mp4 / .mov / .m4v / .mkv` (screen recordings, downloaded videos) and transcribe the audio track. Native-first decode: macOS uses **AVFoundation** (objc2, zero new deps — handles everything QuickTime does, incl. `.mov` screen recordings); cross-platform and Matroska go through symphonia (`isomp4`/`mkv` features). WebM/Opus gets a clear, actionable error (convert, or install ffmpeg), with optional **ffmpeg sidecar** detection (never bundled) as the power-user escape hatch. Same library/editor/export flow — SRT/VTT exports become video subtitles.
- **File associations** — declare supported audio+video document types (CFBundleDocumentTypes) so double-click / "Open With…" opens Suramak; Open Recent; macOS Services later.
- **Recordings** — optional "save compressed (FLAC)" toggle (symphonia already decodes FLAC); kept audio stays 16 kHz mono 16-bit WAV, transient temp file by default.
- **Capture 2.0 — multi-source + per-app (M6)** — source selection moves from a single-source dropdown to a **checkbox list** so sources combine: N concurrent inputs (e.g. two mics; **mic + system audio** — the meeting-transcription combo) mixed into the single 16 kHz mono pipeline with proper clock-drift handling and per-source levels. **Per-app capture** ("transcribe only this Zoom call") rides the same milestone: it is one parameter away in `suramak-taps` (`ProcessSelection::Only(processes)`), plus a picker of running audio processes. Virtual loopback devices (BlackHole etc.) already work today via the input-device picker. Rule: the checkbox UI ships together with the mixing engine — never a single-choice checkbox list.
- **Live transcription (M7)** — partial transcripts *while recording* (user-requested 2026-10-13; today the pipeline is record-then-transcribe, so results appear only after Stop). Shape: chunked partial decodes during capture (VAD-gated windows through the same whisper path, transcript view growing live), clearly labeled as provisional; a final full-file pass on Stop keeps quality. Gating questions: engine cost during capture (CPU/GPU contention with the audio path) and graceful degradation on slower Macs. Until then, record-then-transcribe remains the product.
- **Icons / brand** — macOS 26 layered/tinted icon variants via Icon Composer; optional wordmark optical pass.
- **Windows / Linux** — bring-up per the platform roadmap (the DWM frosted-glass stub is already compile-checked).
