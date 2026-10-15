# WORKORDER — M10A: the Parakeet fast engine (sherpa-rs)

Tree: current `main` (post-v0.1.1 tag). **Read first, in order:** `docs/ENGINES.md` (the whole file — especially “Multi-engine strategy” + “Phase 2”), `docs/MODEL-PINS.md` (pin format), `docs/reports/M1.5*.md` if present (the whisper CoreML tier this must not disturb), and the existing engine code in `crates/suramak-asr/` (`grep -rn "engine" crates/suramak-asr/src | head`).

## Goal

English (plus the languages Parakeet v3 actually covers) gets an **opt-in FAST engine**: NVIDIA **Parakeet TDT 0.6B v3** (int8) through **sherpa-rs** (Rust-native ONNX Runtime; Core ML execution provider on macOS where available — no Swift bridge). **Indonesian must never route to Parakeet.** Whisper stays the default for everything: today's auto-routing is unchanged until the fast engine is explicitly enabled *and* installed.

This is Phase 2 of the multi-engine strategy: engine selection stays a pure function of `(language, installed engines, platform availability, setting)` → engine id, per ENGINES.md. Adding this engine = a new variant + routing rules, never a reshape of call sites.

## Scope

1. **Engine plumbing** (`suramak-asr`): a `parakeet` module + engine variant behind a cargo feature (default: OFF — builds without the feature stay exactly as today). Transcription runs Parakeet TDT v3 int8 through sherpa-rs; results map onto our `TranscriptSegment`/result shape (Parakeet gives token-level timestamps — if segment-level precision is impractical, map honestly and document the granularity in ENGINES.md). `engine_used` reporting works like the whisper tiers (the result records the engine that actually ran).
2. **Model manager**: a new Parakeet model kind — download → sha256 verify → extract → layout check, pinned like the whisper models (extend the pin script / manifest mechanism accordingly; **pins must be real** — download the artifact and take the actual sha256+size). Prefer `parakeet-tdt-0.6b-v3` **int8** (k2-fsa / sherpa-onnx releases; verify the URL and layout yourself). `doctor` reports it (installed / layout / language coverage).
3. **Routing + Settings**: Settings gains a **“Fast engine (experimental)”** control (follow the existing painted-control + settings patterns from M9; default **OFF**). When ON: covered languages → Parakeet if installed, else Whisper; Indonesian and everything else → Whisper. The editor/status engine line names the engine that ran (e.g. `engine: parakeet (fast)`).
4. **Tests + acceptance**: routing unit tests (language × installed × setting matrix — Indonesian can never select Parakeet); model-manager unit tests (manifest, extraction, sha mismatch rejection); a **VPS end-to-end** (ignored/release-only, like the whisper e2e): download the Parakeet model → transcribe `fixtures/audio/en.wav` → assert the text matches the known sentence approximately (normalized case/punctuation) + print wall time (whisper tiny on the same file for the honest comparison note). Run it for real on the VPS; put the numbers in the report.
5. **Honesty + docs**: no speed claims beyond what was measured; update `docs/ENGINES.md` (implemented status, sherpa-rs chosen — FluidAudio parked as the Swift-shim fallback if Mac ANE numbers ever disappoint; Mac ANE verification = owner follow-up), and add the owner smoke items to `docs/USER_SMOKE.md` (enable the toggle; transcribe English; check the engine line; confirm Indonesian still runs Whisper; note install size).
6. **Design for the follow-up**: keep the engine concept generic — **M10B** will be Apple SpeechTranscriber. Owner direction (2026-10-15): it should become the **default path for its covered languages on macOS** (ANE-based; ~2–3× whisper-small throughput at equal-or-better WER per third-party benchmarks; ~85 MB, lives outside app memory, no app-size impact, OS-auto-updated; 10 languages / 30 locales, **no Indonesian**). Spike first: Swift-only API — evaluate the existing Rust `speech` crate wrapper (docs.rs/speech, analyzer/SpeechTranscriber, macOS 26+) before writing our own Swift shim. Policy: covered languages → SpeechTranscriber when available (Settings override to force Whisper), Indonesian + everything else → Whisper; land opt-in first, flip to auto-default after one owner pass with measured numbers.

## Do NOT

- Touch the whisper/M1.5 tiers' behavior, `live.rs` (live transcription stays whisper), capture/mix, playback, glass, taps, the instance guard; no UI redesign beyond the one settings control; no new fonts; no `git commit`/`push`; nothing outside `/home/ubuntu/suramak`.

## Verification (all gates; evidence in the report)

- fmt/clippy `-D warnings`/workspace tests green; the new routing + manager tests included; the feature-off build identical (default features unchanged).
- The **Parakeet E2E actually run on this VPS** with the real downloaded model (network + disk are fine; int8 0.6B on 2 cores is slow but the clip is seconds — budget generously) — record text match + wall times in the report.
- `cargo check -p suramak-ui --target aarch64-apple-darwin` for cfg-gated darwin code paths (sherpa-rs darwin check; the Core ML EP path compiles).
- Report `docs/reports/M10a.md`: item-by-item, exact commands, measured numbers, deviations with whys, and the owner-verification list.

## After this

M10B (Apple SpeechTranscriber) once the owner has had one pass with Parakeet on the Mac; the bake-off note (sherpa-rs vs FluidAudio) stays open pending real ANE numbers.
