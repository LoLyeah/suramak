# Workorder M10C — native Opus import (webm, .opus voice messages, mkv-opus)

**Date**: 2026-10-15
**Author**: orchestrator (Hermes)
**Owner-requested** (2026-10-15): "does the app have ffmpeg? … why not auto conversion" → the answer landed on **native decoding**, not conversion.
**Refs**: `docs/ENGINES.md` (container support), `docs/PLAN-NOTES.md` §"Decode path — container support & the ffmpeg-sidecar escape hatch", PRODUCT.md (M10C bullet), the committed fixtures `fixtures/video/sample-en-opus.webm` + `sample-en-opus.mkv`.

## Goal

WebM/Opus files and `.opus` voice messages (WhatsApp/Discord class) import and transcribe **natively** — no ffmpeg, no conversion recipe for the common case. Genuinely unsupported leftovers (WMA/AIFF/…) keep the existing actionable recipe; the ffmpeg sidecar stays **never-bundled** (the documented escape hatch).

## Design (verified facts, 2026-10-15)

- symphonia 0.5.5 ships **no first-party Opus decoder** (a placeholder crate). The community adapter **`symphonia-adapter-libopus` 0.2.x** (MIT/Apache-2.0; targets symphonia 0.5) fills it: it **compiles and bundles libopus** (default `bundled` feature; needs `cc` — the same class of bundled-C as whisper.cpp; no runtime binary, nothing shipped separately; libopus is BSD). 0.3.x targets symphonia 0.6 — stay on **0.2.x** for this tree.
- The mkv reader already demuxes webm/mkv (M5); the ogg reader covers raw **`.opus`** (Ogg-Opus) voice notes; both need only the decoder registered.
- Registration happens in `crates/suramak-core/src/decode.rs` (the single decode path): build a `CodecRegistry`, register the adapter's `OpusDecoder` alongside the default codecs (mirror the adapter README for the pinned 0.2.x's exact call — `register_all` style for 0.2.x). Do **not** enable symphonia's `opus` feature (no-op placeholder).
- `crates/suramak-core/src/error.rs` keeps the recipe message for genuinely unsupported codecs; webm/opus must never reach it again.
- UI: add `webm` + `opus` to the import dialog's extension filter in `crates/suramak-ui/src/app/home.rs` (the existing list; drops work by content — no other UI change).

## Tests (update + add)

- `crates/suramak-core/tests/decode.rs`: `webm_opus_is_rejected_with_a_conversion_hint` becomes **positive** (decodes the known speech; 16 kHz). Keep a negative recipe test on a genuinely unsupported codec; keep the garbage-bytes test.
- Add decode coverage for `sample-en-opus.mkv` and — if a raw `.opus` fixture can be made with the VPS's ffmpeg (`ffmpeg -i sample-en-opus.webm -c:a copy sample-en-opus.opus`) — the raw Ogg-Opus path (+ a manifest.tsv row).
- E2e: a real transcribe of the webm fixture with the tiny model asserting the "quick brown fox" text (the orchestrator chain re-runs it; mirror it as a test if cheap).

## Do NOT

Touch whisper/M1.5 tiers, `live.rs`, capture/mix, playback, glass, taps, guard, the M9 look; no new fonts; no `git commit`/`push`; nothing outside `/home/ubuntu/suramak`. Do NOT remove the recipe path for other formats. Do NOT feature-gate opus (it ships in the default build — document the size delta instead).

## Verification (all gates; evidence in the report)

- fmt / clippy `-D warnings` / workspace tests green (the updated decode suite incl. the positive opus reads).
- The **real transcribe e2e** on `fixtures/video/sample-en-opus.webm` and `sample-en-opus.mkv` with the tiny model — record transcript + wall times in the report.
- Release-binary size delta (± 0.1 MB) + `cargo tree` showing the adapter/libopus in the default build; licenses noted (libopus BSD; adapter MIT/Apache-2.0).
- Darwin/windows cross-checks: attempt the usual documented ones; note honestly what cannot run from Linux (cc-built C for foreign targets — the pre-existing class).
- Report `docs/reports/M10c.md`; USER_SMOKE **§30** (a webm file + a real voice-note `.opus`; confirm no ffmpeg anywhere in the flow); ENGINES.md container-table update; PLAN-NOTES decision entry (native decode chosen; the sidecar stays never-bundled).

## After this

Owner Mac pass (M9 19–27 + §28 + §29 + §30, one session) → **M11** (audio-aware polish) next in queue.
