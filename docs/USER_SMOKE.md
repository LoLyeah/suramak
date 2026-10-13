# Suramak — user smoke (macOS)

For the owner, on the Mac, from the repo root. ~15 minutes.
Every item lists the expected observable result. Anything else: see **Report** at the end.

Prep (once — prerequisites: Xcode command line tools, Rust via rustup, CMake via `brew install cmake`):

```bash
bash scripts/mac-smoke.sh          # release build + app bundle + CLI e2e; must end "mac smoke complete."
```

The app is `dist/Suramak.app` (or `cargo run -p suramak-ui` for a dev build).
Data lives in `~/Library/Application Support/dev.suramak.Suramak/` (db, settings.toml, models, recordings).

## 1. Core ML engine engaged (ANE tier)

```bash
./target/release/suramak-cli models download tiny --encoder
./target/release/suramak-cli doctor
./target/release/suramak-cli transcribe fixtures/audio/en.wav --model tiny --lang en \
    --engine coreml --formats json --out-dir build/out
```

- Expected: `doctor` prints `coreml: available` and `pairing ok` for tiny (see `docs/ENGINES.md` §Mac manual check).
- Expected: transcribe stderr ends `engine: coreml`; first run is slow (Core ML compiles the model once, then caches).
- If ANE init regresses (whisper.cpp #3702 class): run still succeeds with `engine: metal`/`cpu` + a warning — note it in Report.

## 2. Record 5 s from the microphone

- Click the red dot in the title bar. Expected: record view, `Recording <date>` title, running `mm:ss.t` clock, live waveform/RMS bars.
- Speak ~5 s, press **Stop**. Expected: `transcribing on this machine…` progress, then the editor with your words; sidebar shows the new `Recording <date>` row.

## 3. System audio source (tap + permission prompt)

- Settings → Audio source → Input → `System audio (all processes except Suramak)` → Save.
- Play any audio (Music/YouTube), then record ~5 s and Stop.
- Expected on first use: a macOS permission prompt for audio recording — wording varies by macOS version, in the class of "“Suramak.app” would like to record this computer's audio." → click **Allow**.
- Expected after Allow: transcript of the system audio appears (the recording line shows the tap device name).
- If you click **Don't Allow**: recording fails with a message like `the system-audio tap … is not visible to the audio layer yet — grant the audio-capture permission and retry`. To fix: System Settings → Privacy & Security → Screen & System Audio Recording → enable Suramak, restart the app, retry.
- Note: the tap excludes Suramak itself (no feedback loop).

## 4. Import an mp3

- Drag an mp3 onto the window (or Home → `Choose file…`).
- Expected: transcribe view with progress; then the editor with timestamped segments; new sidebar row titled with the file name.

## 5. Edit + autosave

- Type in any segment. Expected: header shows `unsaved changes`, then `saved` within ~1 s of stopping.
- Quit and reopen the app, click that row. Expected: your edit is still there.

## 6. Export Markdown

- Editor → export panel → `Markdown` → destination `~/<name>.md` → **Export**.
- Expected: `wrote …/name.md`; the file opens in any editor with `[hh:mm:ss.mmm → …] text` lines, including your edits.

## 7. Polish (only if you configure it)

- Settings → Polish (LLM tidy-up): base URL (e.g. `https://api.openai.com/v1`), model (e.g. `gpt-4o-mini`), API key → Save. (Key goes to the OS keychain, never to settings.toml.)
- Editor → **Polish**. Expected: `polishing N segment(s)…`, then a review with `original | tidied` columns per segment.
- **Accept** → text replaced, `saved`, and an **Undo** button appears. Expected: Undo restores the previous text.
- **Discard** → transcript unchanged.
- If the endpoint errors or returns a malformed reply: a red line with the raw error, transcript unchanged. Note it in Report.
- Not configured: the button stays disabled with a note on where to configure it — expected.

## 8. Model manager

- Home → **Models**. Expected: table with sizes and `installed`/`missing` per model; `● default` on one row.
- **Download** e.g. `small`. Expected: row progress `x% — done/total`, then `installed`.
- **Set default** `small` → expected `small is now the default model` and the `●` moves.
- **Delete** it again (keep the one you use). Expected: `removed N file(s)`; the shared Core ML encoder survives while another turbo model still needs it.

## 9. Settings roundtrip

- Settings → Default language → `id` → Save. Quit, reopen. Expected: still `id`.
- **Reset to defaults** alone changes nothing on disk until you press Save — expected.

## 10. App icon (new since M1)

- Expected: `dist/Suramak.app` shows the Suramak icon in Finder; the Dock shows it while running.
- If Finder shows a generic icon: quit/reopen the app or re-run `scripts/make-app.sh` (icon cache).

## 11. keep_audio on/off

- Settings → Keep recordings ✓ → Save. Record 5 s, Stop, wait for the transcript.
  Expected: `<data>/recordings/Recording <date>.wav` exists (16 kHz mono, playable).
- Keep recordings ✗ → Save. Record 5 s, Stop, wait for the transcript.
  Expected: no new file in `recordings/`; the temp copy is deleted once the transcript is saved.

## Report

Anything that deviates from an expected result, with: what you did, what happened, what you expected.
Worth a note even if not fatal:

- engine line actually printed (`coreml`/`metal`/`cpu`) and rough speed per tier;
- the TCC prompt text you saw, and whether denial produced the actionable message;
- any recording that came back silent or garbled (device name shown);
- polish: endpoint used, error text if it failed;
- window chrome/glass: does the window read as the M1 design (dark, rounded, translucent titlebar band);
- anything about the app icon.
