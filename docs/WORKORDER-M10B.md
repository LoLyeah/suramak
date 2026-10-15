# Workorder M10B — Apple SpeechTranscriber (the macOS fast path)

**Date**: 2026-10-15
**Author**: orchestrator (Hermes)
**Refs**: `docs/ENGINES.md` (Phase 3 + the engine landscape), `docs/WORKORDER-M10A.md` §Design-for-the-follow-up, `docs/PLAN-NOTES.md` (conventions), `docs/MODEL-PINS.md`.
**Owner direction (2026-10-15)**: "For mac, speechtranscriber is the default choice as it's fast and cheap (resource wise) for mac." + "Finish M10B first before my mac testing" → **land it opt-in in this workorder**; the **auto-default flip is explicitly deferred** to a follow-up tree after the owner's measured pass (their Mac session covers M9 items + M10A §28 + this workorder's §29 in one go).

## Goal

Add Apple's on-device **SpeechTranscriber** (the SpeechAnalyzer pipeline, macOS 26+) as the **macOS fast path** — via the Rust **`speech` crate** (v0.8.x; `speech::analyzer::SpeechTranscriber` + `AssetInventory`), NOT a hand-written Swift shim. It hangs off the existing M10A machinery: the "Fast engine (experimental)" setting (default OFF), the pure `resolve_engine` routing, the engine line, doctor, `--fast`.

## Facts that shape the design

- **Coverage**: ~10 languages / 30 locales as of macOS 26.6 — **no Indonesian**. Indonesian + everything else stay Whisper, forever, for this engine.
- **Assets are OS-managed**: the locale model ships via the system (the crate wraps `AssetInventory` / `AssetInstallationRequest`). The app does **not** pin, download or host any model file for this engine — `models`/MODEL-PINS stay untouched. `doctor` reports availability instead.
- **The crate has a Swift bridge** (`swift-bridge/`): building it requires Xcode/Swift on a Mac. Linux **cannot** compile it — the Linux surface here is the refusal stub + routing tests (see Verification). The `speech` crate is macOS-26+; whole-file input only (`transcribe_in_path`) — fine, our call shape is one clip → result.
- M10A's `resolve_engine(setting, fast, language, avail)` is the single routing point — extend it; do not fork it.

## Design

1. **Feature + gating**: new cargo feature **`speech-transcriber`** on `suramak-asr` (default OFF; `suramak-cli` / `suramak-ui` propagate it). The `speech` dependency lives under `[target.'cfg(target_os = "macos")'.dependencies]` (optional). Without the feature **or** off-macOS the module exists with the same API shape as the parakeet stub: `compiled()`, honest refusals — so every routing/refusal/serialization test runs on Linux.
2. **Engine variant**: `EngineId::SpeechTranscriber` (`as_str` = `speech-transcriber`; display `speech-transcriber (fast)`). `EngineAvailability` gains `speech_transcriber_compiled` + `speech_transcriber_available` (runtime: macOS 26+ AND the locale is supported by the OS; the pure function takes it as an input so tests pin the matrix).
3. **Routing (pure, in `resolve_engine`)** — with the toggle ON and `setting == Auto`:
   - macOS + language covered by SpeechTranscriber + available → **SpeechTranscriber**;
   - else fall through to the M10A Parakeet check (covered → Parakeet if ready);
   - else Whisper (with the existing honest warning when a fast engine was requested but blocked — extend the message to name the blocker).
   - **`id` never routes to either fast engine** (keep the hard guard + matrix tests). Non-macOS → SpeechTranscriber is never selected. Explicit `coreml|metal|cpu` continues to mean "whisper on that backend" (M10A deviation 4 holds).
4. **The engine module** (`crates/suramak-asr/src/speech_transcriber.rs`, macOS+feature): shared `prepare_samples` front half; transcribe 16 kHz mono f32; map `SpeechTranscriptionResult`s onto the existing `Segment` shape (use the crate's time-range attributes when present; else one whole-clip segment — document); `engine_used` reporting; the same two honest progress milestones (50/100).
   - Asset handling: check availability first; if the locale asset is missing, attempt the crate's wrapped installation request; any failure degrades to **Whisper with an actionable message** — never a hard failure.
5. **Settings/UI**: no new controls. The existing "Fast engine (experimental)" row caption gains the engine order for macOS: `SpeechTranscriber → Parakeet → Whisper (covered languages only; Indonesian always Whisper)`. Engine line names what ran.
6. **CLI / doctor**: doctor gains a `speech-transcriber (fast engine, experimental)` block: build (feature on/off), macOS 26+ availability, supported-locale list, asset state, and the routing note. `--fast` unchanged. No `models` entries (OS-managed; say why in the block).
7. **CI**: extend the macOS CI job with a **compile step for the feature** so the Swift bridge is validated on a real Mac without waiting for the owner (`cargo build -p suramak-asr --features speech-transcriber` — keep the job bounded; do not touch the smoke step).
8. **This build must not touch**: the whisper/M1.5 tiers' behavior, `live.rs` (live stays whisper), capture/mix, playback, glass, taps, instance guard; M10A's parakeet internals beyond the `engine.rs` routing extension + shared plumbing; no UI redesign; no new fonts. Do NOT flip the auto-default. Do NOT add streaming ST. Do NOT attempt FluidAudio. Do NOT pin ST assets in our manifest.

## Do NOT

Only modify files inside `/home/ubuntu/suramak`; no `sudo`; no `git commit`/`push`; nothing outside the repo.

## Verification (all gates; evidence in the report)

- fmt/clippy `-D warnings`; workspace tests green — including the extended routing matrix: ST beats Parakeet on macOS when both are available; ST never off-macOS; ST never for `id`; missing asset → Whisper fallback with the warning; feature-off builds identical (default features unchanged).
- The Linux `--features speech-transcriber` build compiles (the refusal stub path) and its tests pass; assert the stub refusals like M10A did.
- `cargo check -p suramak-ui --target aarch64-apple-darwin` — expect the pre-existing C-dependency failure (ring class, M1.5 deviation 10) — document as before. The ST module itself builds **on a Mac only** (Swift bridge): the CI compile step (Design 7) + the owner's build are its real validation. Write that module small, conservative, and pure-Rust-visible; every ST runtime behavior is **owner-verify** and must be labeled so.
- Report `docs/reports/M10b.md`: item-by-item, exact commands, deviations with whys, honest boundaries, and the owner list. USER_SMOKE gains **§29**: build on the Mac with the feature, tick the toggle, transcribe `en.wav` (engine line must say `speech-transcriber (fast)`; note the first-run asset download if any), confirm `id.wav` still runs whisper, and record the **measured comparison** (Core ML whisper vs SpeechTranscriber on the same clip) — those numbers gate the later auto-default flip.

## After this

Owner Mac pass (M9 19–27 + M10A §28 + M10B §29, one session) → auto-default flip decision (owner numbers + ENGINES.md status update) → **M10C** (native Opus import) next in queue.
