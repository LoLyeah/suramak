# Workorder M11 — audio-aware polish (the recording as a second data point)

**Date**: 2026-10-16
**Author**: orchestrator (Hermes)
**Owner-requested** (2026-10-15): "how about the LLM API analyzing the audio recording too … two data points … add to future milestones."
**Refs**: PRODUCT.md (M11 bullet), the existing in-app polish implementation (`grep -rn "polish" crates/`), `docs/USER_SMOKE.md`, `docs/PLAN-NOTES.md`.

## Goal

The optional LLM **polish** may send the **recording alongside the transcript**, so corrections resolve homophones, names and jargon against what was actually said — a cross-check, not a second ASR (whisper stays the engine). **Default OFF; explicit opt-in; the privacy copy must state the recording itself uploads** (the app's only upload path stays opt-in-only).

## Design (verified facts, 2026-10-15)

- OpenAI-compatible chat completions accept **`input_audio` content parts** (`{"type":"input_audio","input_audio":{"data":<base64>,"format":"wav"|"mp3"}}`; audio-model class). Gemini takes audio natively; Voxtral-class chat models too. The **user's endpoint/model decides compatibility** — the ladder below must degrade gracefully.
- Our recording is **16 kHz mono 16-bit PCM WAV** — already a valid `wav` payload; **no encoder needed**. Size: 32 KB/s raw → chunk it.
- Reuse the existing polish client/plumbing (endpoint/model settings; API key in the OS keychain; the existing replace + Undo UX). No new network stack.

## Requirements

1. **Settings**: under polish, one new toggle — **"Send the recording too (audio-aware polish)"** (default OFF) + a caption stating exactly what uploads and that it needs an audio-capable model (name examples: `gpt-audio` class, Gemini, Voxtral). Same one-control pattern as the Fast engine row.
2. **Request builder**: toggle ON + recording exists → messages = [text part: the existing polish prompt + transcript] + **`input_audio` part(s)**. **Chunking**: split the WAV at **segment boundaries** into chunks of ≤ 4 MB raw (≈ 2 min) — never split mid-segment when boundaries allow; base64 per chunk; corrected outputs concatenated in order. Pin the **exact JSON shape** in tests.
3. **Fallback ladder** (graceful, never a hard failure): recording missing (keep-audio off / file gone) → text-only + a note; endpoint rejects audio (4xx) → automatic single text-only retry + a note naming the reason. Warnings surface in the UI exactly like today's polish warnings.
4. **Privacy copy**: settings caption + README polish section + USER_SMOKE §31: *"the recording itself is uploaded to the endpoint you configured when this is on."*
5. **Tests (all Linux-runnable)**: request-shape builder (exact content parts); chunker (caps, segment alignment, deterministic order, full coverage — every sample exactly once); a **mock-endpoint test** (std `TcpListener` or the existing test-server pattern in the polish tests) asserting the audio part arrives with right format/bytes, chunk coverage, and the 400→text-only fallback. **Never hit real endpoints in tests.**
6. **Do NOT**: touch engines/capture/mix/playback/glass/taps/M9 look; no new fonts; no `git commit`/`push`; nothing outside `/home/ubuntu/suramak`.

## Verification

- fmt / clippy `-D warnings` / workspace tests green, including the new tests (show counts).
- Report `docs/reports/M11.md`: exact commands, the JSON shape pinned, honest boundaries (**real-endpoint e2e = owner with their key**, USER_SMOKE §31: toggle on → polish an English clip against their endpoint → confirm it works or shows the fallback note; confirm OFF = byte-identical to today's text-only behavior).
- USER_SMOKE §31 items; README polish section updated.

## After this

Owner Mac pass adds §31; **M12** (in-app updates) next in queue.
