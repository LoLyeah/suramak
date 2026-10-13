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

Capabilities (MVP scope): file import (wav/mp3/flac/ogg/m4a), microphone recording (record-then-transcribe), on-device transcription with explicit language selection (Auto / English / Indonesian) and a default prompt that teaches the brand word, editable timestamped segments, transcript library (sqlite), exports md/txt/json/srt/vtt, model manager (download with checksum + resume).

Constraints: Rust, native rendering, macOS Liquid Glass as a hard requirement; no telemetry; network only for model downloads and opt-in polish. Polish API keys live in the OS keychain (env-var fallback on Linux dev).

Undecided (do not invent): distribution model (private / open source / paid), UI localization beyond English, live/streaming transcription.

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
