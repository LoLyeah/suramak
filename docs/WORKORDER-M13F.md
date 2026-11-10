# WORKORDER — M13F: mic recording on Android (locked-screen included)

Plan source: `docs/PLAN-M13-ANDROID.md` §3 M13f (size M). Tree: `main` post
`943d7b2` (M13E landed: share-target import + background transcription under
the FGS, 961 tests). This slice adds RECORD: the phone records 16 kHz mono
PCM16 WAV identical to desktop output, screen locked, with interruptions
handled.

## Deliverables

1. **Permissions**: the `RECORD_AUDIO` runtime flow (request once, honest copy
   in Settings; denial = the recording button disabled with the reason, never
   a silent refusal).
2. **The `microphone`-typed foreground service** started only from the
   foreground (Android 14+ while-in-use rule honored): a persistent
   notification with elapsed time + Stop; the M13E FGS conventions
   (progress/cancel pattern) carry over. The service stops when the recording
   stops — never lingers.
3. **Capture in Rust via cpal/AAudio** through the existing `suramak-asr`
   capture path (the M6/M14B2 lineage): device-native rate → resample to
   16 kHz mono (the capture path already resamples — verify on Android's
   48 kHz sources via a host-mocked AAudio callback test).
4. **Interruptions** (per the plan): incoming call, audio-focus loss,
   Bluetooth headset disconnect — each stops the recording CLEANLY (what is
   recorded stays; the session records a journal noting the interruption kind),
   never continues silently into a call.
5. **Record → transcribe hand-off**: Stop lands the WAV exactly where an
   imported file lands (the same inbox path as M13E), so the transcription +
   library + notification flow is literally the M13E one.
6. **Locked-screen recording**: the FGS keeps the capture alive with the
   screen locked (the plan's verification case); a Doze/kill attempt during
   capture must be surfaced (the notification's last-known state) and the
   journal notes the abrupt end.
7. **Quit button invariant** (owner requirement): Quit stops any capture
   first, then the service, then exits.

## Verification

- Host: unit tests for the interruption state machine (call/focus/bt cases),
  the resample path at Android rates (48 kHz→16 kHz fixture), the journal
  replay after a forced stop, and the hand-off equivalence with the M13E
  import path (same WAV bytes as an import for the same fixture).
- `cargo ndk -t arm64-v8a --platform 29 check -p suramak-android --locked`;
  debug build-apk.sh with aapt assertions (RECORD_AUDIO permission, FGS type,
  notification channel), Kotlin warning-free (`allWarningsAsErrors`).
- fmt/clippy -D warnings; workspace tests green (961 baseline → grow; ONE
  full workspace run at the end; crate-scoped tests meanwhile; disk: clean if
  < 10 GB). Keep every M15 suite green; the desktop capture tests must not
  regress (the Android capture code is cfg-gated).
- **Honest boundary (owner device)**: real locked-screen capture, a real
  incoming call, and headset hot-swaps are phone-only facts; report §"owner
  checklist" = record 30 min locked + interrupt with a call + a Bluetooth
  swap, then verify the WAV via the desktop doctor.
- Report: `docs/reports/M13f-mic-android.md`.

## Do NOT

- No compact UI (M13g — the button lives on the existing spike-era screens;
  M13G reflows it), no release packaging (M13h), no VAD/chunking (M16.4), no
  new dependencies (cpal's oboe/audio-aaudio feature selection is a cargo
  feature flip — verify what exists before reaching for a crate), no
  destructive changes to the desktop capture path (cfg-gated additions only).
- No git commit/push; no sudo.
