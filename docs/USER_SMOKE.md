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

## 17. Multi-source recording (M6): mic + system audio, and "Only an app…"

- Settings → **Audio sources** → tick **two** entries, e.g. `Default microphone` **and** `System audio (all processes except Suramak)` (any combination works: two mics, a mic + a loopback device, …). The hint line shows `pick one or more — everything ticked is recorded together into one 16 kHz recording`; untick everything and press Save — expected: a red `select at least one input source` line and **nothing is written** (the previous list stays). Save with two entries ticked.
- Start a call (Zoom/Meet/whatever) so **both sides** have audio, press the floating **Record** control, speak ~5 s, press **Stop**.
  Expected: the record view shows a `sources` card with **one row per source** (name, `mute`, gain slider ×1.00), the level meter moves, and after Stop the transcript contains **both sides of the call** (your voice and the remote side), in one `Recording <date>` item.
  - While recording, drag one source's gain slider down (or tick `mute`) — expected: it stays visibly changed; the final recording reflects it (gain/mute apply to the whole recording — the mix happens at Stop). A source that is not producing audio contributes silence, never an error.
- **(b) "Only an app…"** — Settings → Audio sources → tick **`Only an app…`**. Expected: the process picker opens below it; start playback in the app you want (e.g. a Zoom call or Music) and press **Refresh** — the app appears as `us.zoom.xos (pid 1234) — playing`. Tick it (and only it), untick every other source, Save.
  - Record ~5 s while that app plays and another app plays something else (e.g. a browser video). Expected: the transcript contains **only the ticked app's audio**; the other app's audio is absent; Suramak's own playback never appears.
  - With the ticked app **not running**, press Record — expected: a red, actionable line (`no running audio process matches 'us.zoom.xos' — start the app (and play audio), refresh the list, and retry`) instead of a silent fallback to full system audio.
  - Note: the picker needs a macOS build (14.2+ process taps); on Linux it shows `needs a macOS build with capture …` and the list stays read-only.

## 18. Live transcription while recording (M7)

- Press the floating **Record** control and speak a sentence or two.
  Expected: after a few seconds the record view's **live transcript** card starts filling in with your words, under the label `live transcript — provisional · the final pass runs on Stop`; the list auto-scrolls as new words arrive.
- Keep talking, then press **Stop** (or the floating control).
  Expected: the provisional list is discarded the moment you press Stop and the app runs the usual final pass — `transcribing on this machine…`, then the editor with the **final** transcript (the one saved to the library; the provisional text never reaches it, and words may differ slightly from what the live pass showed).
- On a slower machine, or with a big model (`turbo-q5-0`), a note like `N window(s) skipped to keep up — live updates may lag; the final pass is unaffected` may appear in the card and live words may fall behind the speech — **that is fine**: live updates degrade, the recording and the final transcript never do.
- The live pass uses the same model / language / initial prompt as the final one and never uploads anything (like everything else in the app). The first live words appear after ~4 s of speech (the window cadence) plus the model load.

## 19. Quit is clean — no ggml assert / `zsh: abort` (HOTFIX-QUIT)

Quit behavior is **owner-verified on the Mac** — the ggml-metal teardown abort only exists there (Linux has no Metal). Launch the app from a terminal so its logs are visible: `./dist/Suramak.app/Contents/MacOS/suramak` (or `cargo run -p suramak-ui` for a dev build).

- **Quit after a session**: record ~5 s, press **Stop**, wait for the editor/transcript, then quit with **Cmd-Q**. Expected: the app vanishes with **no** `zsh: abort`, **no** `GGML_ASSERT([rsets->data count] == 0)` and no `ggml_metal_device_free` line in the terminal, and **no new Suramak crash report** in Console.app.
- **Menu Quit and window close**: repeat the same sequence using the **Suramak → Quit Suramak** menu item, and once more closing the window with the red button. Expected: identical — clean, silent exit every way.
- **Quit while recording**: start a recording (floating Record control), speak a few seconds, then Cmd-Q **without** pressing Stop. Expected: same clean exit (no abort/assert); the recording is discarded — quitting is not Stop, nothing is saved.
- **Quit while a job runs**: import a file and Cmd-Q during `transcribing on this machine…`. Expected: same clean exit (no abort/assert); the half-done job is not saved, the app is gone within a moment (the quit waits at most ~¼ s for workers, never for the whole job).
- **Quit after live transcription**: record >5 s so the live transcript card fills in, Stop, wait for the final transcript, then Cmd-Q. Expected: same clean exit.

## 20. The remade editor — the Deck (M9a)

The shell has been rebuilt (M9 slice 1). The editor now reads as **one** surface system: a **control deck** band under the titlebar — compact transport capsule (skip-to-start, play/pause), the waveform strip, a `mm:ss.mmm / mm:ss.mmm` readout, and one **Export…** button; below it the title line with a quiet **Polish**; the transcript as clean rows (mono timecode gutter; the active segment carries a thin blue edge bar and a faint tint).

- Open a transcript from the library. Expected: **no black bars behind any text anywhere** (that was the old look), the deck spans the pane, the readout ticks as playback runs, and **Export…** opens a small sheet anchored under itself (format chips + destination + Save as…). Clicking outside or leaving the editor closes it.
- Press play: the playhead moves along the strip and the active-row bar follows. Click or drag the strip to seek.
- Check the floating pill: it should float over the transcript area in every state — **never** covering the export sheet or any button.
- The delete affordance sits on the title line (quiet, far right); its in-place confirm is now painted check/cross glyphs (no emoji).

## 21. Appearance: System / Light / Dark (M9a)

- Open **Settings → Appearance**. The Theme control offers *System / Light / Dark* (default **System**).
- Pick **Light**, press **Save**. Expected: the app flips to light **without a restart** — window material, text, controls and the glass all re-resolve; same with **Dark**.
- Set it back to **System**, then flip your Mac's appearance (System Settings → Appearance) with the app open. Expected: the app follows **live**, both directions.
- The choice persists across relaunches.

## 22. Motion (M9a)

- Move between screens (Home ⇄ Editor ⇄ Settings ⇄ Record). Expected: a quick fade with a small rise — no teleports.
- Start and stop a recording: the floating pill **morphs** between idle and recording forms (label/color crossfade, gentle lift on press).
- Hover and press controls (buttons, glyphs, transport): subtle fast transitions everywhere; nothing animates forever.
- With **Reduce Motion** enabled (System Settings → Accessibility → Display), the same actions snap instantly instead of animating.

## 23. The waveform is real + the heat budget (M9b)

- Open `en.wav` (or any imported file): the waveform strip should show a **real** envelope — tall where there's speech, flat through pauses, a flat tail at the end. Play it and compare the shape to what you hear.
- Open the recording whose audio wasn't kept (keep-audio off): the strip shows a **flat line + a hint text** — no invented waveform, ever.
- The heat budget: the app no longer redraws continuously. Sanity check in Activity Monitor (search "suramak"): idle ≈ 0% CPU; while recording, the meter/timer tick without the app chewing CPU; a long transcription runs far quieter than before. (Linux-measured: idle repaints −99%, recording ~10 fps instead of ~19, job ≈ 1.5 fps.)

## 24. Keyboard + focus (M9b)

- In the editor: **Space** play/pause · **⌘E** export sheet · **⌘R** start/stop recording (from anywhere) · **↑/↓** jump between segments · **⌘⌫** arm the delete confirm · **Esc** closes the sheet / leaves the editor.
- All of those must stay silent while you're typing inside a transcript row (Esc excepted).
- Press **Tab** repeatedly: every control gets a visible blue focus ring (sidebar rows → transport → waveform strip → Export… → trash…).

## 25. Your two fixes — the title-line buttons + the header line (M9c)

- Open any transcript. The **trash and Polish controls are now real buttons** — translucent capsule bodies with edges, a clear gap between them, gentle press response (the pill's material family). Hover and press them.
- **The line behind the title**: root-caused — the header's vertical rhythm mixed measured-text placement with fixed offsets, so it only collided under your Mac's real fonts. The header is now built from explicit pixel constants and **exactly one rule** is allowed in that zone. Check the title/caption area: no line should cross any text. **If it's still there, screenshot it** — that pins the last piece.
- Copy: the record-view timer and the pill now read `hh:mm:ss`; next to Stop it says `the meter shows the last 10 seconds`; model names are human everywhere (`Tiny (fastest)`, `Large v3 Turbo (balanced)`).
- The export sheet now fades open/closed (no teleport); Esc closes it.

## 26. The confirmed motion details (M9d)

- **Press and hold the floating pill**: it compresses ~3% and releases with a gentle spring — no bounce. (On macOS the glass frame itself stays fixed; the content compresses inside it — if you'd rather the whole glass capsule spring, say so, it's a small glass-runtime change.)
- **While recording**, a small red dot beside the `recording` word breathes slowly — subtle, and nothing else pulses.
- **Click the waveform** far from the playhead: the playhead eases to the spot (nearby clicks stay instant — no lag while playing). During playback, the transcript's follow-scroll animates.
- **Reduce Motion** (System Settings → Accessibility → Display): everything above snaps — including the spring.

## 27. The three pre-release fixes (M9e)

- **Quit while recording**: close the window mid-recording → a card asks what happens to
  the take. **Stop & Save** keeps the audio and finishes its transcription automatically
  the next time you open the app; **Discard** does what the old behavior did;
  **Keep Recording** (or Esc) stays put. Try all three. On your Mac, **⌘Q** must show the
  same card — that path (the AppKit veto) is the one piece I could only compile-check on
  the VPS, so you’re its first real test.
- **Cancel a running transcription**: start one, then hit the quiet **Cancel** on the job
  card (Esc works too) — it should return you Home with “transcription cancelled”,
  stopping cleanly between stages.
- **Small windows**: drag the window down to its minimum (900×600). Settings scrolls
  end-to-end — Save and Reset are reachable and clear of the Record pill; the Models
  list keeps its columns apart (no mashed headers) and you can scroll to the end.

## 28. The fast engine — Parakeet (M10A, experimental)

Parakeet is **opt-in twice**: this app build must carry the `parakeet` cargo feature,
and the model must be installed. The default build has neither, and nothing about
today's routing changes until you turn the toggle on.

- **Build with it** (the macOS build does not ship it yet): from the repo root,
  `cargo build --release -p suramak-cli --features parakeet` (and
  `cargo build --release -p suramak-ui --features parakeet` for the app). The first
  build downloads the prebuilt sherpa-onnx libraries once and caches them.
  Note (packaging): the build script copies the sherpa-onnx shared libraries next
  to the built binary; if the binary cannot find them at launch, run it with
  `DYLD_LIBRARY_PATH=<target>/release` on macOS (`LD_LIBRARY_PATH=…` on Linux)
  until the app-bundle packaging lands (M10B follow-up — the dylibs go into
  `Contents/Frameworks` with an `@loader_path` rpath).
- **Install the model**: `./target/release/suramak-cli models download parakeet`
  (~670 MB, four sha256-verified files; `models list` shows it as `parakeet`).
  `suramak-cli doctor` prints a `parakeet (fast engine, experimental)` block —
  expect `model: installed … layout ok` and the 25-language coverage line.
- **Turn it on**: Settings → **Fast engine** → tick `(experimental) — Parakeet for
  covered languages` → Save. (Off by default; `settings.toml` gains
  `fast_engine = true`.)
- **English**: transcribe `en.wav` (or any English clip). Expected: the engine line
  says `engine: parakeet (fast)`. Time it against the same clip on `--engine coreml`
  (whisper) — that is the comparison that matters on your Mac; the VPS numbers
  (CPU-only) are in `docs/reports/M10a.md` and are not ANE numbers.
- **Indonesian stays whisper**: with the toggle ON, transcribe `id.wav` with
  language `id` — the engine line must say `engine: Core ML` / `Metal` / `CPU`
  (never parakeet). Auto-detect also stays whisper by design.
- **When it can't run**: with the toggle ON but the model not installed (or a build
  without the feature), the job still runs — on whisper — and the status line shows
  `fast engine requested but … — using …`.
- **Install size note**: the Parakeet model adds ~670 MB under the models dir (plus
  the one-time sherpa-onnx native libraries in the build cache). `models remove
  parakeet` deletes it.

## 29. The macOS fast path — Apple SpeechTranscriber (M10B, experimental)

SpeechTranscriber is **opt-in** the same way as Parakeet: the app build must carry
the `speech-transcriber` cargo feature (macOS only — the `speech` crate builds a
Swift bridge), and the Settings toggle must be on. It needs **macOS 26+** and one
of its 10 covered languages (`en, es, fr, de, it, pt, ja, ko, zh, yue` —
**Indonesian is never covered**). Its models are **managed by macOS** (nothing to
install or pin; the system ships and updates them), so there are no `models`
entries for it — `doctor` reports the state instead.

- **Build with it** (the macOS build does not ship it yet; needs Xcode/Swift —
  the crate compiles its Swift bridge with `swift build` on first build): from the
  repo root, `cargo build --release -p suramak-cli --features speech-transcriber`
  (and `cargo build --release -p suramak-ui --features speech-transcriber` for the
  app). First build takes a few minutes (the bridge compiles once).
- **Check `doctor`**: `./target/release/suramak-cli doctor` — expect a
  `speech-transcriber (fast engine, experimental)` block with
  `build: compiled in …`, `os: macOS 26+ SpeechTranscriber available`,
  `auth: …`, `locales: N supported by this OS (…)`, the 10-language coverage line
  and `asset: …` for your default language.
- **Turn it on**: Settings → **Fast engine** → tick
  `(experimental) — fast engines for covered languages` → Save. On macOS 26+ the
  order is `SpeechTranscriber → Parakeet → Whisper`.
- **First run**: transcribe `en.wav` (or any English clip). Expected: a one-time
  macOS prompt — “Suramak would like to access speech recognition” class → click
  **Allow** (if it never appears, check System Settings → Privacy & Security →
  Speech Recognition; a denial still transcribes — on whisper — with an
  actionable warning). The first run may also download the locale model
  (OS-managed; a few tens of MB, once) and may be slower than later runs.
  Expected engine line: **`engine: speech-transcriber (fast)`**.
- **The measured comparison (this gates the later auto-default flip)**: on the
  **same clip**, time `--engine coreml` (whisper on the ANE encoder) vs the fast
  path (`--fast`), e.g.
  `time ./target/release/suramak-cli transcribe fixtures/audio/en.wav --model small --lang en --engine coreml --formats json --out-dir build/out` vs
  `time ./target/release/suramak-cli transcribe fixtures/audio/en.wav --model small --lang en --fast --formats json --out-dir build/out`
  — record both wall times, the engine lines, and compare the texts (a couple of
  runs each; note the first-run Core ML compile and the first-run asset download
  separately). **These numbers decide whether SpeechTranscriber becomes the
  auto-default for its covered languages** — put them in the Report.
- **Indonesian stays whisper**: with the toggle ON, transcribe `id.wav` with
  language `id` — the engine line must say `engine: Core ML` / `Metal` / `CPU`
  (never `speech-transcriber`). Auto-detect also stays whisper by design.
- **A covered language the OS lacks** (e.g. a locale your Mac does not list in
  doctor): the job still runs — on Parakeet if installed, else whisper — and the
  status line shows `fast engine requested but … — using …` naming the blocker.
- **When it can't run**: no macOS 26, feature not compiled, or authorization
  denied → the job still runs on whisper with an actionable warning; nothing
  fails hard.
- **Note (packaging)**: the `speech` Swift bridge links Apple frameworks only —
  no dylibs to copy; unlike Parakeet there is nothing to bundle for this engine.

## Report

Anything that deviates from an expected result, with: what you did, what happened, what you expected.
Worth a note even if not fatal:

- engine line actually printed (`coreml`/`metal`/`cpu`) and rough speed per tier;
- the TCC prompt text you saw, and whether denial produced the actionable message;
- any recording that came back silent or garbled (device name shown);
- polish: endpoint used, error text if it failed;
- window chrome/glass: does the window read as the intended glass shell (rounded, translucent), and does it follow your Mac's light/dark (or the Appearance override you set);
- anything about the app icon.
