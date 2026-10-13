# Transcription engines on macOS — landscape & plan

Status: 2026-10. This is the decision record for acceleration/engine work; the milestone workorder is `docs/WORKORDER-M1.5.md`.

## Landscape (verified via research, Oct 2026)

| Engine | Hardware path | Speed class (Apple Silicon) | Indonesian | Rust-native? | Verdict |
|---|---|---|---|---|---|
| whisper.cpp (current) | CPU / Metal GPU | good | ✅ | ✅ (whisper-rs) | core family |
| **whisper.cpp + Core ML encoder** | **Neural Engine (ANE)** | ~2–3× faster than CPU; the fast Whisper path | ✅ | ✅ (whisper-rs `coreml` feature) | **add in M1.5** |
| Parakeet TDT v2/v3 (FluidAudio CoreML / MLX) | ANE / GPU | fastest overall (~110–300× realtime) | ❌ (25 European langs) | ❌ (Swift/Python) | not primary; no `id` |
| Apple SpeechAnalyzer / SpeechTranscriber (macOS 26) | ANE | very fast, long-form tuned | ❌ (10 langs as of 26.6) | ❌ (Swift-only API) | revisit if Apple adds `id` |
| WhisperKit (Argmax) | ANE via CoreML | excellent (2.2% WER turbo, streaming) | ✅ | ❌ (Swift; needs a Swift shim) | defer; same silicon path as our CoreML option |
| ANEForge (whisper.cpp backend) | direct ANE (private APIs) | ~2× faster than CoreML encoder | ✅ | partial (pip package + dylib) | research watch-list |
| MLX / faster-whisper / Python stacks | GPU/CPU | varies | ✅ | ❌ | excluded (native constraint) |

Key facts:
- MacWhisper's speed reputation comes from this same trick: whisper.cpp with the **Core ML encoder on the ANE**.
- The whisper.cpp HF repo ships **official pre-converted encoder packages** (`ggml-<model>-encoder.mlmodelc.zip`, 12 of them incl. `ggml-large-v3-turbo-encoder.mlmodelc.zip`) — no local Python conversion needed.
- ANE init can regress on some macOS/chip combos (whisper.cpp #3702) → the app must fall back to Metal cleanly and report which engine actually ran.
- First CoreML run compiles for the device (slow once, cached after).

## Decision

- **Whisper (whisper.cpp) remains the one core engine family** — it is the only family with reliable Indonesian plus the widest model range.
- **Add a macOS acceleration tier: `whisper-coreml` (ANE encoder)** — M1.5:
  - `suramak-asr` gets a `coreml` feature (macOS-only) wired to `whisper-rs/coreml`.
  - Model manager downloads, sha256-verifies and extracts `<model>-encoder.mlmodelc` packages next to their ggml files (renamed to match the selected model's stem).
  - Engine selection is explicit and honest: `--engine auto|coreml|metal|cpu` (settings field `engine`, default `auto` = coreml → metal → cpu); the result records which engine actually ran; `doctor` reports the acceleration stack and encoder pairing.
  - Auto-fallback: if CoreML/ANE init fails, retry without it and surface a warning (never a hard failure).
- **Deferred engines** (documented, not built): Apple SpeechTranscriber shim (no `id`, Swift-only), Parakeet/FluidAudio (no `id`), WhisperKit shim, ANEForge (private APIs).
- Re-check this table each macOS release; Apple adding Indonesian to SpeechTranscriber is the trigger to revisit the native-Apple engine.

## Implementation notes

- Encoder packages are fp16 and external to the ggml decoder, so `turbo` and `turbo-q5-0` decoders can both pair with the `large-v3-turbo` encoder package (verify pairing at implementation; `doctor` exposes it).
- The encoder zip's sha256 + size get pinned in the model manifest the same way ggml files are (extend `scripts/pin-model-hashes.sh`).
