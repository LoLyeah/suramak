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

- Click the floating **Record** control (bottom center of the window; real glass pill on macOS 26+). Expected: record view, `Recording <date>` title, running `mm:ss.t` clock, live waveform/RMS bars; the floating control itself now shows the stop glyph + a live `mm:ss`.
- Speak ~5 s, press **Stop** (in the record view) — or click the floating control (its stop affordance) — the result is identical. Expected: `transcribing on this machine…` progress, then the editor with your words; sidebar shows the new `Recording <date>` row.

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

## 12. Custom language code

- Settings → Default language → **Custom…** → type `es` → Save. Quit, reopen the app.
  Expected: Custom… still selected with `es` in the field; transcripts made with it carry `es` in the editor meta line; `grep language` in `settings.toml` shows `es`.
- Type something invalid (e.g. `e!`) and press Save without fixing it.
  Expected: a red inline hint under the field, and the language keeps its previous value (nothing invalid is written).

## 13. Delete a transcript

- Open any transcript (sidebar row) → click the small trash glyph in the editor header (left of `Home`) → confirm with **✔** (the ✖ cancels).
  Expected: you land back on Home with `deleted <title>` in the status line; the sidebar row is gone.
- Quit and relaunch the app.
  Expected: the row is still gone and its segments with it (the delete is permanent).

## 14. Playback with segment follow

- Open a transcript made from an **imported file** (e.g. the mp3 from item 4). In the editor, press the play control left of the waveform strip.
  Expected: audio plays; the playhead moves along the strip; the readout shows `mm:ss / mm:ss`; the segment containing the playhead is highlighted and scrolls into view as playback moves down the transcript.
- Press the control again (it now shows the pause bars).
  Expected: playback pauses; the highlight stays where it is and the list stops following until you play again.
- Click in the middle of the waveform strip.
  Expected: playback jumps there (still playing if it was playing); the highlight follows.
- Click a segment's timestamp (the mono column at the left of a row).
  Expected: playback seeks to that segment's start.
- Press **Home**, switch transcripts, or start a recording.
  Expected: playback stops and the playhead resets; opening that transcript again starts from the beginning.
- **Missing audio**: open a transcript recorded with `Keep recordings` **off** (item 11 ✗). Expected: the strip shows the reason (`keep audio was off — the recording's file is gone …`) instead of the playhead and the play control is dimmed — pressing it does nothing (there is no file to play).
- **No output device**: press play with no output device available (unplug headphones/speakers, or a VM without audio). Expected: the strip shows `no audio output device on this machine`; the app keeps working and the rest of the editor is unaffected. Connect a device and press play again to retry.

## 15. Floating record control (M8.5)

- On the Home view, look at the bottom center of the window. Expected: a floating pill-shaped control — on macOS 26+ **real Liquid Glass** (it blurs/refracts the desktop behind it; resize the window over a wallpaper to see the material move); on older macOS the same pill is egui-painted (translucent fill, hairline, subtle shadow) — with a red dot and the label `Record`.
- Click it. Expected: the record view opens and the recording starts, **and the floating control switches to the stop glyph + a live `mm:ss` clock ticking every frame** (it stays visible on every screen while recording).
- Click the floating control again (or the record view's Stop). Expected: recording stops and finalizes exactly like before — `transcribing on this machine…`, then the editor with your words and the new `Recording <date>` sidebar row. No double-finalize if you click twice.
- **Resize the window** (any size, any screen scale). Expected: the control stays bottom-centered, ~24 pt above the bottom edge, horizontally centered on the window — never stretched, never off the bottom; its glass material follows exactly the painted pill.
- **While a transcription job is running** (or on a build without capture compiled in), the control is dimmed. Expected: clicking it does not start anything; a status line explains why (`recording is disabled while a job is running` / `recording is not compiled into this build (feature \`capture\`)`).
- **The old titlebar red dot is gone** — the floating control is the record entry point. Expected: no red dot in the title bar; the titlebar keeps only the (decorative) play/export glyphs.

## 16. Import a video (M5)

- Drag a real screen recording (`.mov` or `.mp4`) onto the window (or Home → `Choose file…` — the picker now says `Audio or video`).
  Expected: exactly the audio-import flow — transcribe view with progress, then the editor with timestamped segments and the file name in the sidebar; the audio track is transcribed and the video stream is ignored.
- Repeat with an `.mkv` (AAC audio).
  Expected: identical behavior.
- Drag a WebM (Opus audio — e.g. a screen recording made on a Linux box) onto the window.
  Expected: the transcribe view fails with a red, actionable line naming a working alternative, e.g. `cannot decode clip.webm: the audio codec (Opus) is not supported — convert it first, for example: ffmpeg -i "clip.webm" "clip.mp4"` — no crash, no dead end; **Back** returns home.
- Export the video transcript as `SRT`/`VTT`.
  Expected: valid subtitle files with the same timestamps as the audio (the editor and exports are unchanged — a video import is unremarkable).

## Report

Anything that deviates from an expected result, with: what you did, what happened, what you expected.
Worth a note even if not fatal:

- engine line actually printed (`coreml`/`metal`/`cpu`) and rough speed per tier;
- the TCC prompt text you saw, and whether denial produced the actionable message;
- any recording that came back silent or garbled (device name shown);
- polish: endpoint used, error text if it failed;
- window chrome/glass: does the window read as the M1 design (dark, rounded, translucent titlebar band);
- anything about the app icon.
