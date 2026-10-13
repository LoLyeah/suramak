# PLAN-NOTES — verified corrections & environment facts

Empirically verified on the dev VPS, 2026-10-13. **Where these notes conflict with PLAN.md, these notes win.**

## Environment (all verified)

- rustc/cargo **1.99.0**; clang, cmake, ninja, pkg-config present. Targets installed: `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`.
- `cargo check --target aarch64-apple-darwin` **works from Linux** (verified with the real objc2 crates: 32 s).
- crates.io reachable; whisper.cpp builds here (whisper-rs 0.16.0, release build 1 m 13 s) and transcribes (verified end-to-end, below).
- GitHub Actions runner labels verified as available: `macos-14`, `macos-15`, **`macos-26`, `macos-26-arm64`** → the CI macos job uses **`macos-26-arm64`** (the real `NSGlassEffectView` class exists at runtime there).

## Corrections to PLAN.md

1. **HF repo name is `ggerganov/whisper.cpp`** (PLAN §2/§10 says "ggeraganov" — wrong). Files verified present (range-GET → 206): `ggml-tiny.bin`, `ggml-base.bin`, `ggml-small.bin`, `ggml-large-v3-turbo.bin`, `ggml-large-v3-turbo-q5_0.bin`, `ggml-large-v3-turbo-q8_0.bin`. (`ggml-small-q5_0.bin` does NOT exist — don't reference it.)
2. **objc2 versions:** latest (and what cargo resolves for `"0.3"`/`"0.6"`) is **objc2 0.6.5, objc2-app-kit 0.3.2, objc2-foundation 0.3.2** — newer than PLAN's 0.3.1/0.6.2 pins. objc2-app-kit 0.3.2 **does ship typed wrappers**: `NSGlassEffectView`, `NSGlassEffectContainerView`, `NSGlassEffectViewStyle` (feature flags named exactly `NSGlassEffectView`, `NSGlassEffectContainerView`; the structs are also gated on features `NSView` + `NSResponder`). → Use the typed wrapper **behind a runtime `AnyClass::get("NSGlassEffectView")` presence check** (macOS 26+), falling back to `NSVisualEffectView`. Raw `msg_send!` only for what the wrapper lacks (check cornerRadius/tintColor/style availability at compile time — if the wrapper has them, use them).
3. **whisper-rs 0.16 segment API (verified against crate source — PLAN §2 names some functions that don't exist):**
   - `WhisperContext::new_with_params(path: impl AsRef<Path>, WhisperContextParameters::default())`, `ctx.create_state()`.
   - `state.full(FullParams, &[f32]) -> Result<()>`.
   - Segments: `for seg in state.as_iter() { seg.start_timestamp(), seg.end_timestamp() /* centiseconds → ms ×10 */, seg.to_str()?, seg.n_tokens(), seg.no_speech_probability() }`. There is **no** `full_get_segment_text` / `full_get_segment_tstamp`.
   - `state.full_n_segments() -> i32`, `state.full_lang_id_from_state() -> i32`.
   - Params: `FullParams::new(SamplingStrategy::Greedy { best_of: 1 })`, `set_language(Option<&str>)`, `set_n_threads(i32)`, `set_initial_prompt(&str)`, `set_print_special/progress/realtime/timestamps(bool)`, `set_token_timestamps(bool)`.
   - Crate also ships `whisper_vad` (`WhisperVadContext`) — not needed for MVP (record-then-transcribe), note for later.
4. **ASR behavior findings (measured VS VPS: tiny/base/small/turbo, en+id fixtures):**
   - EN: tiny and base transcribe the fox sentence exactly.
   - ID with language auto-detect: `small` **TRANSLATED to English** (auto-detect went wrong). With `set_language(Some("id"))` → correct Indonesian. → Language is a first-class setting; for Indonesian content, forced language is effectively required. UI: Auto / English / Indonesian selector, and remember the last choice.
   - Brand word "Suramak" is only spelled correctly with a matching `set_initial_prompt` (e.g. "Nama saya Suramak, aplikasi transkripsi lokal.") — verified. Ship a sensible default prompt including "Suramak"; make it user-editable in settings.
   - `large-v3-turbo` is near-perfect for both en and id (CPU: ~10× realtime on 2 cores; Apple Silicon + Metal will be far faster). Default model `turbo-q5-0` (574 MB) endorsed; `small` = fast option; `tiny` = CI only; full `turbo` = accuracy option.
5. **Fixtures already exist and are committed**: `fixtures/audio/en.wav`, `fixtures/audio/id.wav`, ground-truth text in `fixtures/audio/manifest.tsv` — generated with edge-tts **neural** voices (far more realistic than espeak-ng; espeak-ng is not installed — do NOT try to regenerate fixtures with it; PLAN §7's espeak commands are superseded). Use these files in tests/CI exactly as committed. `en.wav` contains the fox sentence + a "Suramak" sentence; `id.wav` contains Indonesian sentences including "Suramak".
6. **Toolchain pin: `stable` (currently 1.99.0)** — do not pin 1.88.0 as PLAN §9 says; avoids forcing a second toolchain download on the VPS and the user's Mac. `edition = "2021"` fine.
7. **`pin-model-hashes.sh` scope**: exactly the six verified files above. Manifest entries recommended: `tiny` (CI), `small` (fast), `turbo-q5-0` (default), `turbo` (accuracy). SHA256 values are provided in the M0 workorders (pre-computed by the orchestrator from the downloaded files) — paste those; do not re-download.
8. **M0 is split for execution**: M0a = workspace + `suramak-core` (+ its tests). M0b = `suramak-asr` + `suramak-cli` + CI + end-to-end acceptance. Workorders `docs/WORKORDER-M0a.md` / `docs/WORKORDER-M0b.md` are authoritative for scope and acceptance.
9. Copy detail: `NSMicrophoneUsageDescription` string (PLAN §7) — keep as planned.
10. **mojibake guard**: whisper output for non-ASCII may include replacement chars — `to_str()` is fine for en/id; use `to_str_lossy()` only if a test shows an issue.

## Known non-issues (verified — do not chase)

- **macOS 27 + winit 0.30.13 IME** — two latent crashes, both first-hit on the owner's Mac (2026-10-13): (1) `-[NSView inputContext]` is nil on macOS 27, so the first text-field focus panicked (`set_ime_cursor_area` → `.expect("input context")`, view.rs:901) — fixed by the `LoLyeah/winit@63f21f8` fork pin (nil-tolerant guards; drop the pin when upstream handles nil). (2) `set_ime_allowed(false)` (text field loses focus) SIGSEGVs in `objc_release` releasing the marked-text cell — released object is the literal `0x3` (crash `48A5EFA3-…`); root cause not yet pinned. Workaround shipped: on macOS the shell forces `platform_output.ime = None` every frame (`shell.rs`), so egui-winit never calls `set_ime_allowed` at all; en/id typing is unaffected (no IME needed). Revisit the winit 0.31 IME rework before re-enabling IME.
- Build-end warning *"block v0.1.6 … code that will be rejected by a future version of Rust"*: the frozen `block 0.1.6` comes from the **`metal` crate inside wgpu/wgpu-hal** (our renderer), not from the audio stack; the flagged construct is `static _NSConcreteStackBlock: Class;` (uninhabited static, rust-lang/rust#74840). Harmless today; disappears when wgpu/metal migrate to `block2`/objc2 upstream. Do not vendor or patch it pre-1.0.
- **macOS 27 SDK**: the CoreAudio aggregate-device description keys (`kAudioAggregateDeviceUIDKey`, `…NameKey`, `…IsPrivateKey`, `…TapListKey`, `kAudioSubTapUIDKey`) are **string-literal macros** with no linkable symbols and no `dlsym` hits; `suramak-taps` builds the dictionary keys as CFStrings from the literal values (`"uid"`, `"name"`, `"private"`, `"taps"`, `"uid"` — from the SDK header). Keep the literal-CFString approach if a future SDK shifts again: the HAL matches dictionary keys by string content.
- On macOS 27 machines `ld` prints *"object file … was built for newer 'macOS' version (27.0) than being linked (11.0)"* for C deps: benign deployment-target noise (rustc's default `-mmacosx-version-min=11.0` vs SDK-built objects); the app's real minimum is `LSMinimumSystemVersion` in `Info.plist`.
