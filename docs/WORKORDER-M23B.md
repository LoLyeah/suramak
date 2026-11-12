# WORKORDER — M23B: can the phone diarize? A gate, not a promise

Plan: `docs/reports/M23-speaker-tagging.md`. Desktop diarizes with
sherpa-onnx (pyannote-seg + CAM++); Android compiles `suramak-asr` without the
features that pull sherpa. This slice answers one question with evidence:
**can the engine be built and run on `arm64-v8a`, and if so, at what cost?**
No product code lands from this workorder — its deliverable is a report and a
go/no-go.

## Steps

1. **Build feasibility (the hard part).** Try, in order, and record exactly
   what fails and why:
   a. `sherpa-rs 0.6.8` with `download-binaries` for
      `--target aarch64-linux-android` (does its cache carry Android
      natives? which ABI/API level?);
   b. if not: build sherpa-onnx 1.12.9 from source with the NDK toolchain
      (cmake + ninja against `arm64-v8a`, `ANDROID_PLATFORM=29`,
      `BUILD_SHARED_LIBS=ON`, onnxruntime for Android as the prebuilt AAR's
      `.so`) and link `sherpa-rs/sys` against it via `SHERPA_ONNX_LIB_DIR`;
   c. record the artifact sizes (sherpa `.so` + onnxruntime `.so` + the two
      models: pyannote 5.99 MB, CAM++ 28.3 MB) and the APK delta.
2. **If it links**: run the measured gate the plan names — a 60-min,
   multi-speaker recording on the OWNER'S DEVICE (never the emulator), screen
   off, unplugged: RTF ≤ 0.45, RSS delta ≤ 180 MB, battery drop ≤ 10%,
   thermal slowdown < 25%, and Whisper weights unloaded before sherpa
   allocates. State the pass/borderline/fail verdict with the numbers.
   If the owner's device is not available to this slice, run what CI can (an
   x86_64 emulator timing is NOT evidence — say so) and leave the device gate
   explicitly open for the owner.
3. **If it does not link**: stop. Document the blockers, the two paths
   (vendored from-source build in CI vs. an ORT-Android integration), their
   costs, and the recommendation. Do not attempt a partial integration.

## Deliverable

`docs/reports/M23b-android-diarization-gate.md`: the attempt log with real
command output, the artifact/APK numbers, the verdict, and — if GO — the exact
product slice that follows (FGS step after transcription, progress, unload
order, the `Tag speakers` action that replaces manual creation). If NO-GO,
the phone keeps manual tagging and the report says so in one line.

## Do NOT

- No product code, no Cargo feature flips on `main`, no CI edits (build
  experiments happen in a scratch dir or a branch that is never pushed); no
  voiceprint storage; no cloud; no new crates on `main`; no git commit/push;
  no sudo.
