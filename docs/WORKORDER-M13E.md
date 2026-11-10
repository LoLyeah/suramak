# WORKORDER — M13E: import & background transcription (the core Android loop)

Plan source: `docs/PLAN-M13-ANDROID.md` §3 M13e (size M). Tree: `main` post
`3f61e5f` (M13D landed: models library + dataSync JobService + tiers; M16B
landed). **This is the slice that makes the phone useful**: share a voice note
→ it transcribes with the screen off → the library shows it.

## Deliverables

1. **`ACTION_SEND` / `ACTION_SEND_MULTIPLE` intent filters** on the app
   (`audio/*`, `application/ogg`, the WhatsApp/Telegram Opus mime variants —
   document each accepted type): the fixture targets are WhatsApp voice notes
   (raw Ogg-Opus) and Telegram `.opus`/webm.
2. **Saf import**: `ACTION_OPEN_DOCUMENT` import (any audio/`video mime scheme
   the decode path accepts) — fd-based copy into app storage with progress
   events over the M13c-class bridge (a bounded progress channel; no per-byte
   JNI spam).
3. **Transcription jobs under the foreground service** (type `mediaProcessing`
   API 35+, `dataSync` on 34, untyped below — per the plan): a partial wake
   lock, a progress notification (percent for the current file + the queue
   count), a **cancel action on the notification**, and `onTimeout` → clean
   stop (the job resumable on the next launch). One inference slot (the M16.1
   queue conventions — the same discipline the desktop runs).
4. **Interrupted-job recovery on launch**: a job cut by process death before
   completion is offered to be resumed or discarded (state must be recorded
   BEFORE the audio phase is gone; document exactly what state persists — the
   source file + the model choice + the partial progress, never the
   transcript's own text).
5. **Completion**: a notification tap opens the transcript in the app (the
   compact Library for now; the M13G compact UI replaces the spike screens —
   the notification landing keeps one path, invisible if the UI target screen
   does not exist yet — document precisely what happens today).
6. **Audio decode** on the phone goes through the SHARED decode interface (the
   fixture parity from M15.2 — the same PCM-hash fixtures run against the
   Android decode path in-host as an android-compile-gated test or a
   host-crate test with the cfg targeting the same code; the contract is the
   M15.2 one — reuse it, do not fork it).
7. **The four checks stay untouched**: the spike screens remain until M13G.

## Verification

- Host: unit tests for the import state machine (share → copy → transcribe →
  library row), cancellation mid-decode/mid-transcribe, recovery-after-kill
  semantics at the library layer, the intent-filter type matrix (declared
  types verified via aapt), JobService type selection per API level
  (compile-verified + unit-tested selection logic).
- `cargo ndk -t arm64-v8a --platform 29 check -p suramak-android --locked`;
  debug build-apk.sh with aapt assertions (both .so, manifest intent filters,
  service types+permissions).
- Kotlin compiles warning-free (`allWarningsAsErrors`); fmt/clippy -D
  warnings; workspace tests green (938 baseline → grow; ONE full run at the
  end; crate-scoped tests meanwhile; disk: clean if < 10 GB).
- **Honest boundary (owner device)**: the share→transcribe loop under a
  foreground service and the screen-off behavior are only provable on the
  owner's phone; the report names the exact steps (share a WhatsApp voice
  note → expect the progress notification → completion → the library row).
- Report: `docs/reports/M13e-import-transcribe.md`.

## Do NOT

- No mic recording (M13f), no compact UI (M13g — the spike screens stay), no
  release packaging (M13h), no new dependencies (rustls feature flips of the
  existing graph are allowed if a new crate is unavoidable for SAF — justify),
  no destructive migrations; M15 suites untouched. The engine stays whisper
  tiny/base/small only (M13d's tiers); Java timeouts are handled, not
  suppressed.
- No git commit/push; no sudo.
