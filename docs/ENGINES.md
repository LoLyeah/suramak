# Transcription engines on macOS — landscape & plan

Status: 2026-10. This is the decision record for acceleration/engine work; the milestone workorder is `docs/WORKORDER-M1.5.md`.

## Landscape (verified via research, Oct 2026)

| Engine | Hardware path | Speed class (Apple Silicon) | Indonesian | Rust-native? | Verdict |
|---|---|---|---|---|---|
| whisper.cpp (current) | CPU / Metal GPU | good | ✅ | ✅ (whisper-rs) | core family |
| **whisper.cpp + Core ML encoder** | **Neural Engine (ANE)** | ~2–3× faster than CPU; the fast Whisper path | ✅ | ✅ (whisper-rs `coreml` feature) | **implemented in M1.5** |
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

## Status (M1.5 — implemented)

Verified **from Linux** (repeatable on any machine):

- Manifest pins for the official encoder packages matching our models — tiny, base, small and `large-v3-turbo` (shared by `turbo`, `turbo-q5-0`, `turbo-q8-0`) — with real sizes + sha256, re-checked against the downloaded artifacts (`scripts/pin-model-hashes.sh` prints both sections).
- `suramak-cli models download <id> --encoder`: download → sha256 verify → extract into `<models-dir>/<encoder-dir>-encoder.mlmodelc` → layout check → archive removed. Verified end-to-end for tiny (15.0 MB, sha256 `c88cbd26…`).
- `doctor` prints the engine stack (`WHISPER : COREML = … | …`, Core ML/Metal availability, encoder pairing per installed model); `transcribe --engine auto|coreml|metal|cpu` prints the engine that actually ran.
- Engine resolution, encoder-directory naming (whisper.cpp's `-qX_X` stripping), zip extraction (path-traversal rejection, `__MACOSX/` skipping, atomic replace) and the encoder hide/restore retry are unit-tested on Linux.

Verified **on macOS only** (real ANE execution): whisper.cpp actually loads the package and encodes through Core ML — the `e2e_coreml_encoder_engaged_macos` test asserts `engine_used == coreml`; it runs in the macOS CI job (tiny encoder downloaded there) and via the manual check below. The "~2–3× faster" speed class is **not** re-measured here — it needs a real Apple Silicon machine.

Facts confirmed against the vendored whisper.cpp (whisper-rs-sys 0.15.0) at implementation time:

- The encoder directory whisper.cpp looks for comes from the *model file name*: strip the extension, strip a trailing `-qX_X`, append `-encoder.mlmodelc` (`whisper_get_coreml_path_encoder`). `ggml-large-v3-turbo-q5_0.bin` therefore pairs with `ggml-large-v3-turbo-encoder.mlmodelc` — one package for all three turbo decoders.
- Core ML is a **compile-time** feature with **no runtime switch**: whisper.cpp uses the package whenever the directory exists *and* loads. `suramak-asr` honours `--engine metal|cpu` (and the Core ML retry) by temporarily hiding the directory during state init; the state keeps the ggml encoder for the whole decode.
- whisper-rs-sys builds whisper.cpp with `WHISPER_COREML_ALLOW_FALLBACK=1`, so a package that fails to load degrades to the ggml encoder instead of aborting — the app reports the engine that actually ran and never fails hard.
- `whisper_coreml_init` uses `MLComputeUnitsAll` (CPU/GPU/ANE): Core ML engagement is testable on any Mac (VM included); the ANE speed-up itself is Apple-Silicon-only.
- Engagement is detected from whisper.cpp's own log lines ("Core ML model loaded" / "failed to load Core ML model from …") captured through a `whisper_log_set` callback — the library's own statement of what it did, not a guess.

## Mac manual check (ANE)

```bash
# on a Mac (Apple Silicon), repo root
cargo build --release -p suramak-cli
./target/release/suramak-cli models download tiny --encoder    # or small/turbo for real use
./target/release/suramak-cli doctor                            # expect: coreml: available … + pairing ok
./target/release/suramak-cli transcribe fixtures/audio/en.wav --model tiny --lang en \
    --engine coreml --formats json --out-dir build/out         # expect stderr: engine: coreml
```

What to expect:

- The **first** Core ML run compiles the model for the device — noticeably slower (tens of seconds for tiny); Core ML caches the compiled model afterwards.
- Then benchmark the tiers on the same file — `--engine coreml` vs `--engine metal` vs `--engine cpu` (`time …`) — and check `engine:` on stderr; Activity Monitor's GPU History / `powermetrics` show the ANE/GPU activity.
- If a macOS/chip combination regresses on ANE init (the whisper.cpp #3702 class), the run still succeeds: the CLI prints `engine: metal`/`cpu` plus a warning — no hard failure.

## Container support — the decode path (M5)

Status: 2026-10-14, test-verified on Linux from the committed fixtures in
`fixtures/video/` (real ffmpeg files built from the TTS speech sample; exact
commands in `fixtures/video/manifest.tsv`). Import decodes the **first audio
track** of a file and ignores video streams; every container below rides the
identical pipeline (decode → resample → whisper) as audio imports.

| container | audio codec | supported | fallback hint (the import error) |
|---|---|---|---|
| wav / mp3 / flac / ogg (vorbis) / m4a | pcm / mp3 / flac / vorbis / aac | ✅ (pre-M5) | — |
| mp4 | aac | ✅ (M5) | — |
| mov | aac | ✅ (M5) | — |
| m4v | aac | ✅ (M5) | — |
| mkv | aac | ✅ (M5) | — |
| mkv | opus | ❌ | `cannot decode clip.mkv: the audio codec (Opus) is not supported — convert it first, for example: ffmpeg -i "clip.mkv" "clip.mp4"` |
| webm (not an offered import extension) | opus | ❌ | same message with the file's own name |
| anything else (wma, aiff, …) | — | ❌ (no reader in this build) | `cannot decode clip.wma: this file's format is not supported — convert it first, for example: ffmpeg -i "clip.wma" "clip.mp4"` |

Notes:

- Decoding is pure-Rust **symphonia** — no ffmpeg dependency and nothing
  bundled. The `ffmpeg` line in the errors is a user-facing conversion recipe.
- Readers enabled: `isomp4` (mp4/m4a/mov/m4v), `mkv` (matroska/webm), `wav`,
  `ogg`, plus the existing audio codecs; `mkv` was already active through
  symphonia's default features and is now pinned explicitly in the workspace
  manifest (no new crates).
- `aiff` is listed in the file picker but has no reader enabled in this build
  (pre-existing); such files now fail with the actionable conversion error
  instead of a raw decoder message.
- The **ffmpeg-sidecar** integration (decode anything by shelling out to a
  bundled ffmpeg) remains the documented future escape hatch — see
  `docs/PLAN-NOTES.md`; it is deliberately not implemented and nothing is ever
  bundled.

## Multi-engine strategy (agreed direction, 2026-10-13)

Verdict: **different engines per language is the right end state** — it is where the mature apps converge (MacWhisper ships Whisper and Parakeet side by side). Phase it:

- **Architecture now (M1.5)**: engine selection is a pure function of `(language, installed engines, platform availability)` → engine id. Adding an engine is a new variant + a routing rule, never a reshape of call sites. The result always records the engine that actually ran.
- **Phase 1 (M1.5, now)**: Whisper engines — CPU / Metal / CoreML-ANE. Covers English **and** Indonesian with the big speed unlock.
- **Phase 2 (after the app works, post-M2)**: add **Parakeet TDT** as the English/EU fast path (~an order of magnitude faster than Whisper on ANE; TDT v2 English-only 1.69% WER; TDT v3 = 25 EU languages at ~5.4% English WER). Two candidate integrations, decide by bake-off: **sherpa-rs** (Rust-native, ONNX Runtime + CoreML EP — no Swift bridge) vs **FluidAudio** (Swift, best measured ANE numbers, needs a small Swift shim). Default routing once present: English → Parakeet if installed, else Whisper-ANE; Indonesian → Whisper-ANE; everything else → Whisper.
- **Watch list**: Apple SpeechTranscriber (add when it gains `id`, or for ja/ko/zh coverage), ANEForge, Parakeet multilingual expansion.
- Every added engine carries: model-asset download/verify, `doctor` reporting, a per-language quality note, and its own acceptance run. One engine at a time.
