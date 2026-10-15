# Transcription engines on macOS — landscape & plan

Status: 2026-10. This is the decision record for acceleration/engine work; the milestone workorder is `docs/WORKORDER-M1.5.md`.

## Landscape (verified via research, Oct 2026)

| Engine | Hardware path | Speed class (Apple Silicon) | Indonesian | Rust-native? | Verdict |
|---|---|---|---|---|---|
| whisper.cpp (current) | CPU / Metal GPU | good | ✅ | ✅ (whisper-rs) | core family |
| **whisper.cpp + Core ML encoder** | **Neural Engine (ANE)** | ~2–3× faster than CPU; the fast Whisper path | ✅ | ✅ (whisper-rs `coreml` feature) | **implemented in M1.5** |
| Parakeet TDT v2/v3 (FluidAudio CoreML / MLX) | ANE / GPU | fastest overall (~110–300× realtime) | ❌ (25 European langs) | ❌ (Swift/Python) | parked (Swift shim); **sherpa-rs path implemented in M10A** |
| **Parakeet TDT v3 int8 via sherpa-onnx (sherpa-rs)** | CPU everywhere; **Core ML EP on macOS** | fast (CPU-verified on the VPS; ANE numbers = owner-verify) | ❌ (25 European langs) | ✅ (Rust + prebuilt ONNX Runtime) | **implemented in M10A** (opt-in) |
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

## Status (M10A — the Parakeet fast engine, implemented opt-in)

Phase 2 is implemented as an **opt-in** engine (`docs/WORKORDER-M10A.md`): the
`parakeet` cargo feature (default OFF) **and** the model installed **and** the
Settings "Fast engine (experimental)" toggle ON. Without all three, routing is
exactly the pre-M10A behaviour (builds without the feature link no sherpa code at
all).

What it is:

- NVIDIA **Parakeet TDT 0.6B v3 int8** (`sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8`,
  4 pinned files, 670 609 772 bytes) through **sherpa-rs 0.6.8** (sherpa-onnx
  1.12.9 — Rust-native ONNX Runtime; prebuilt native libraries fetched once at
  build time and cached under `~/.cache/sherpa-rs`).
- Execution provider: `cpu` everywhere; `coreml` on macOS builds (the prebuilt
  macOS libraries carry the Core ML EP; sherpa-onnx logs and falls back to CPU per
  node, and the module retries on `cpu` if a Core ML session cannot be created at
  all — the engine stays Parakeet either way).
- Routing (pure function, `suramak-asr::engine::resolve_engine(setting, fast,
  language, availability)`): the fast engine applies only while the whisper
  `engine` setting is `Auto`; only a **forced, covered** language (25 EU
  languages, pinned in `models::PARAKEET.languages`) routes to Parakeet;
  **Indonesian never** (hard guard + the routing-matrix tests); language
  auto-detect and everything else stay Whisper.
- Result mapping: sherpa-onnx decodes the whole clip in one pass and returns
  **token-level timestamps** (seconds) — there are no whisper-style segment
  boundaries. Segments are reconstructed by splitting the token stream at pauses
  ≥ 0.5 s; a punctuation token that would start a split folds back into the
  sentence it terminates. If a model ever returns no timestamps, the whole result
  maps to one segment spanning the audio (the honest fallback). The transcript
  records `model: parakeet`; the engine line shows `parakeet (fast)`.
- `engine_used` reporting matches the whisper tiers: the result carries the engine
  that actually ran, and a request that cannot be honoured falls back to Whisper
  with a warning (`fast engine requested but … — using …`) — never a hard failure.

Measured on the VPS (Linux, 2 cores, **CPU provider — not ANE numbers**),
`fixtures/audio/en.wav` (9.00 s), release build:

| engine | model load | decode + resample | realtime | text |
|---|---|---|---|---|
| parakeet (TDT v3 int8, cpu) | 6.37 s | 2.16 s | 4.2× | "The quick brown fox jumps over the lazy dog. Surimat keeps every recording on this machine and never sends audio to the cloud." |
| whisper tiny (cpu) | 0.10 s | 2.62 s | 3.4× | "The quick brown fox jumps over the lazy dog. Surramat keeps every recording on this machine and never sends audio to the cloud." |

Word-overlap against the known fixture text: **0.96** (the brand word is misheard
by both engines on this clip). So on this 2-core VPS the Parakeet decode is
~1.2× faster than whisper tiny end-to-end, while its model load is ~60× slower
(652 MB of ONNX). On Apple silicon the Core ML EP should change that picture —
that is **owner-verify**; no ANE numbers are claimed here.

Verified from Linux (this tree):

- The real end-to-end (`cargo test -p suramak-asr --release --features parakeet
  --test parakeet -- --ignored`): download → per-file sha256 verify → load →
  transcribe → approximate text match (table above); the routing test pins
  `en → parakeet` and `id → never`, against the real install.
- Routing-matrix unit tests (language × installed × setting, incl. the Indonesian
  invariant over the whole availability matrix); model-manager tests (pins,
  per-file sha256 + size, layout, idempotency, corruption rejection); `doctor`
  prints the engine block (build / provider / model + layout / coverage /
  setting).
- `cargo check -p suramak-ui --target aarch64-apple-darwin` cannot pass from
  Linux (pre-existing: `ring`, rusqlite-bundled and whisper-rs-sys need an Apple
  C toolchain — M1.5 deviation 10; the exact failure is in
  `docs/reports/M10a.md`). Instead: `cargo tree -e features --target
  aarch64-apple-darwin` resolves `sherpa-rs` + `sherpa-rs-sys` (with the `sys`
  feature) for the darwin target, and a throwaway probe crate
  (`build/probe-darwin` — the same sherpa feature set and a trimmed copy of the
  module's C-API calls) compile-checks for `aarch64-apple-darwin` from Linux.

Boundaries (honest):

- macOS **Core ML EP engagement and ANE speed are owner-verify** (no Apple
  hardware on the VPS); the darwin code path is compile-checked only.
- On Linux the linked sherpa-onnx shared libraries are copied next to the
  binary by the sherpa-rs build script; runs need
  `LD_LIBRARY_PATH=<target>/<profile>` (`…/deps` for test binaries). Packaging
  the dylibs into the macOS app bundle (`Contents/Frameworks` + an
  `@loader_path` rpath) is a follow-up for the Mac build (M10B / owner).
- sherpa-rs' `static` feature (self-contained binaries) is available but requires
  `RUSTFLAGS="-C relocation-model=dynamic-no-pic"` on Linux x86_64; not used here.
- `live` transcription stays whisper by design; the fast engine runs on file jobs
  only. The UI Models screen does not list the Parakeet model (install/remove via
  `suramak-cli models download|remove parakeet`); the Settings toggle is the only
  UI addition.
- Quality note: 25 European languages (no Indonesian); the acceptance is a
  text-match, **not** a WER benchmark — no quality claims beyond the overlap
  figure above.
- The bake-off note (sherpa-rs vs FluidAudio) stays open pending real Mac ANE
  numbers: sherpa-rs was chosen for the Rust-native constraint, FluidAudio
  remains the parked Swift-shim fallback.

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

- **Architecture now (M1.5)**: engine selection is a pure function of `(setting, fast engine, language, installed engines, platform availability)` → engine id. Adding an engine is a new variant + a routing rule, never a reshape of call sites. The result always records the engine that actually ran.
- **Phase 1 (M1.5, now)**: Whisper engines — CPU / Metal / CoreML-ANE. Covers English **and** Indonesian with the big speed unlock.
- **Phase 2 (M10A, implemented opt-in)**: **Parakeet TDT v3 int8** as the English/EU fast path, through **sherpa-rs** (Rust-native ONNX Runtime + the Core ML execution provider on macOS — no Swift bridge). Decision record: **sherpa-rs chosen** for the Rust-native constraint; **FluidAudio stays parked** as the Swift-shim fallback should real Mac ANE numbers ever disappoint (the bake-off note stays open pending those numbers — owner-verify). Routing (see “Status (M10A — implemented)” below): with the “Fast engine (experimental)” setting ON, covered languages → Parakeet if installed, else Whisper; **Indonesian → Whisper, always**; everything else → Whisper.
- **Phase 3 (M10B, next)**: Apple SpeechTranscriber — the default path for its covered languages on macOS once it is proven (owner pass with measured numbers). Spike first: the Rust `speech` crate wrapper (docs.rs/speech, analyzer/SpeechTranscriber, macOS 26+) before any Swift shim.
- **Watch list**: Apple SpeechTranscriber (add when it gains `id`, or for ja/ko/zh coverage), ANEForge, Parakeet multilingual expansion.
- Every added engine carries: model-asset download/verify, `doctor` reporting, a per-language quality note, and its own acceptance run. One engine at a time.
