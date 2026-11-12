# Changelog

All notable changes to Suramak. Format: [Keep a Changelog](https://keepachangelog.com/); versioning per `docs/VERSIONING.md` (semver).

## [Unreleased]

## 0.6.0 - 2026-11-13

### Added
- **The watched inbox**: point Suramak at a folder once — a phone sync folder, a recorder's output, a colleague's drop — and every audio file that finishes landing there imports itself. A 5 s poll (3 levels deep, 200 entries per scan), a 2 s size-settle gate so half-written files wait, `.part`/`.crdownload` blocked by name and sibling, and sha256 dedupe through a new ledger, so the same file copied twice transcribes once. It rides the existing queue, and the Library says `Watching · 3 imports` — or names the failure when the folder disappears.
- **Citation-verified notes**: Minutes, Action items and a bilingual EN↔ID brief, generated through your own OpenAI-compatible endpoint from the transcript's segments. Every `[mm:ss]` is checked against a real segment (±1.5 s): valid ones are rewritten to canonical times, and fabricated, past-the-end, malformed or duplicate citations are **removed and counted** — the sheet reads `12 citations checked · 1 removed (no matching audio)` instead of quietly shipping a timestamp that isn't there. Stored per transcript and preset; export as Markdown.
- **Open Transcribe (Android)**: a documented intent contract — `app.suramak.action.TRANSCRIBE` with a `content://` uri and your own `PendingIntent` — so another app can hand Suramak audio and receive the transcript back as a broadcast. One queue, no new permission, no NotificationListener, no media-folder access. Caller docs in `docs/OPEN-TRANSCRIBE.md`.

### Fixed
- A removed citation no longer leaves its separator behind: "approved [13:00]." becomes "approved.", not "approved ." — pinned for end-of-line, mid-line and sentence-final cases.

## 0.5.0 - 2026-11-12

### Added
- **The UI refresh (M20A–E)** — the five slices the owner asked for ("dated, no motion, too barebones"), with Liquid Glass kept and real AppKit glass used on macOS:
  - **Type**: bundled Inter, Newsreader (serif titles, the empty state) and Geist Mono, on a written scale with tabular numerals; the phone's timecodes and transport counters now sit in fixed-width cells, so playback stops jittering.
  - **Material**: seven frost tokens per ladder with WCAG-asserted contrast (lowest 6.23:1), `frosted_panel`/scrim/dock/card primitives, deterministic grain, a shadow budget, and 11 painted round-cap icons at 44 pt hit boxes.
  - **Library**: serif title, gear action, frosted search, one honest model banner (absent when a verified model is in use), type-scaled rows with hairlines and press motion, the Newsreader empty state, and a floating frosted dock (Import secondary, Record primary in record red).
  - **Settings**: one grouped model card with human tier names (Quick test / Compact / Balanced / Highly accurate), a real Recommended badge, a full-bleed accent selection, RAM-named state-aware copy, no raw ids or engine strings on the primary surface, Quit hidden on Android.
  - **Editor and sheets**: body type scale, kit-drawn speaker headers with a pencil, "Line length" instead of CPL on the phone, every sheet on the Sheet plane with slide + scrim motion, pressed-spring buttons, Newsreader onboarding.
- **A queue that survives the OS (M22A/A2)**: schema v8 job journal + segment checkpointing. Committed text is durable before the journal records it, torn writes reconcile in both directions on open, and a job killed mid-file resumes at the boundary. Proven by a SIGKILL test whose resumed transcript is byte-equal to an uninterrupted run. Resume is wired into all three hosts (CLI, desktop, Android) through one shared helper.
- **The voice note comes back as a transcript (M22B)**: share audio → a 1.2 s frosted sheet → the existing FGS queue transcribes headlessly → a paged notification with honest "Transcribing… N%" progress, then the transcript with Copy / Next page / Open editor / Share quote card. Speaker prefixes only when diarization ran; the sender's name only when the intent carried it. No new permissions, no NotificationListener, no media scraping.
- **Library-wide search (M22C)**: schema v9 FTS5 segment index; the Library field searches titles *and* spoken text, with phrases, AND terms and `speaker:` filters; hits show a bracketed snippet and tap through to the editor at that segment. Audit gate as a test: 8,000 segments / 400k words, p95 **16.05 ms** (bar 50 ms). Rows gained a mini-waveform, speaker pips (+N) and tabular durations.
- **Speaker tagging on the phone (M23A)**: `Speaker 1…N` by hand, per recording — add, assign ("this turn" or "from here down"), rename, colour, merge into…, remove label, clear all, with undo on every verb. Exports can include speakers (SRT/VTT/Markdown) and never rewrite a file silently: the sheet says when speakers changed since the last export.

### Fixed
- **BACK on the Android share sheet now refuses the share**, not just closes the sheet — the pending URIs are dropped with it (the emulator smoke caught the offer staying armed).
- **macOS CI**: a restored `target/` made the sherpa build script count as fresh and skip copying its dylibs, so bundling failed; the bundler now recovers the pinned files from sherpa-rs's own cache and still verifies every sha256.
- The smoke's clearance assertion now asks the right question: the Quit row must sit above the viewport edge (which already excludes the gesture inset), not have its own trailing padding visible.

## 0.4.3 - 2026-11-11

### Fixed
- **Android: the compact Library comes up.** The library host was registered in one `OnceLock` while the shell read a second, never-set one, so the app opened and stayed on the "Opening the library…" splash forever. One host accessor now backs both sides — the Library, its model card, its empty state and the Import/Record bar draw on a cold launch (proven on a real Android 15 emulator in CI, screenshot-inspected).
- **Android: relaunching after Back no longer crashes.** winit allows one event loop per process; the finish path now ends the process cleanly (`_exit`, recorded as a voluntary exit — no SIGABRT/SIGSEGV), so the next launch is a fresh process with a fresh loop and GPU. The relaunch screen paints and reaches the Library again.
- **Android: Back on the splash leaves** instead of being swallowed (the gesture used to do nothing while the host was still starting).
- **CI: an Android launch smoke** now boots an x86_64 emulator on every push and asserts the chain the owner reported broken: install → cold launch → Library renders → Back → clean exit → fresh-process relaunch → Library renders → a bounded monkey pass with the app still alive. Screenshots, logcat, uiautomator dumps and the platform's exit-info are uploaded as artifacts on every run.
- CI: the apt-dependent jobs retry with backoff (GitHub's mirror DNS outages were failing jobs before they started); the emulator job no longer touches apt (cmake/ninja ship with the runner image).

## 0.4.2 - 2026-11-11

### Added
- **Speaker diarization (M16.6)** — the editor can label who spoke, entirely offline:
  - **Run diarization** from the transcript menu: the recording is clustered into session-local speakers (no identity claims, nothing leaves the device), rendered as color badges on segment rows and a single 8 px speaker lane above the waveform.
  - **Manual correction first-class**: rename a speaker in place (the name propagates through the run), delete a run to restore the plain view, re-run after edits (edits mark the run stale).
  - Verified quality on real EN+ID multi-speaker fixtures: 2-speaker DER 3.29% (2/2 speakers), rapid 3-way 2.11% (3/3) — deterministic across runs; 3+ speakers stay provisional and manually correctable by design.
  - Runs as a job through the same one-slot queue discipline; platforms without the bundled engine show the feature as unavailable.
- **Windows joins the release page (M14x)**: `suramak-<version>-windows-x86_64.zip` (app + CLI) with a published sha256 — the first Windows artifact on a release. Windows may show a SmartScreen note on first launch (unsigned build); More info → Run anyway.

### Fixed
- The dylib-resolution gate's rpath checks no longer mis-report matching rpaths as failures (a SIGPIPE + pipefail trap caught three times across the packaging scripts).

## 0.4.1 - 2026-11-11

### Added
- **Android is a product (M13c–h)** — the phone app the milestone plan promised, release-signed:
  - **Share a voice note** (WhatsApp/Telegram's 9 voice-note media types) or **open a file** (Ogg/WebM/mkv-opus through the same decode path the desktop uses — fixture-parity, sha256-pinned) → transcribed **entirely on the phone** with the model downloaded on first use (checksummed, resumable across app kills, free-space checked).
  - **Recording**: mic capture through the desktop's capture path (16 kHz mono WAV identical to desktop bytes), foreground-service notification with elapsed time, interruption handling (calls, audio focus, Bluetooth), locked-screen recording.
  - **Compact phone UI**: the launch surface is the Library; transcript editor with split/merge + undo, search, playback-synced word highlighting, exports (SRT/VTT/ASS/MD/JSON/TXT) byte-identical to the desktop, onboarding sheet, honest diagnostics behind Settings.
  - **Background safety**: one inference slot (the desktop queue discipline), kill-proof job journaling with resume-or-discard, foreground-service progress/cancel notifications, a Quit action (capture → services → exit).
  - Device floor: arm64 + Android 10+; below 4 GB the larger models are refused with clear messages. release-signed APK on the release page (`suramak-<version>-android-arm64-v8a.apk` + sha256); install steps + permissions explained in docs/android.md.
- **M16.1 — the multi-file job queue** (desktop + CLI): batch imports transcribe one-by-one with per-item state, cancel-anything-from-anywhere, failures never cascade.
- **M16B (macOS) — global hotkey + floating dictation HUD**: Carbon hotkey (zero permission prompts), six-state HUD card, Accessibility-gated insertion with a Copy-first fallback.
- **M16.4 feasibility**: Silero VAD chunked long-recording capture is a GO gated into five slices (VAD ≈ 0.53% of transcription CPU; report `docs/reports/M16d-vad-feasibility.md`) — integration starts post-release.

## 0.4.0 - 2026-11-10

### Added
- **M15 Transcript Workbench (all eight slices)** — the transcript correction/search/export surface becomes a real workbench:
  - **Source text is immutable** (schema v2 `segments.source_text`): any edit, polish or rewrite preserves the model's original inference text (the only text that could be permanently lost was editable segments overwriting it — fixed with an additive migration; older libraries upgrade in place, losing nothing).
  - **Split/merge as real transactions** (M15.3): word-boundary/caret-validated splits with bounded undo/redo (20 steps, reset per transcript), Cmd+Shift+S/M + Cmd+Shift+Z / Cmd+Z (Ctrl equivalents on Linux/Windows), zero-duration handled per the export policy; search stays consistent after every mutation.
  - **Subtitle diagnostics + basic ASS export** (M15.4): grapheme-aware CPL/CPS advisory badges (configurable 37/42 + 17/20 defaults) on every segment row, and the ASS serializer joins SRT/VTT as the sixth export format (documented escaping; zero-duration cues become one frame or skip, configurable).
  - **Measured glossary + reversible rules** (M15.5): a 64-term glossary is fed into the Whisper initial prompt against a token cap **measured from your actual model at runtime** (`suramak-cli prompt-budget` — e.g. ggml-tiny = 224 tokens; overflow shows exactly how many terms were dropped); a separate deterministic post-correction dictionary applies to a derived layer only (idempotent, reversible); seven opt-in Indonesian normalization rules guarded against code-switching; Clean/Verbatim export with a previewed diff and an explicit apply.
  - **FTS5 search** (M15.7): ranked results over the full corpus (+timestamps and snippet text), unicode61 tokenizer — no stemming anywhere, Indonesian retrieval stays exact; every mutation path updates the index in the same transaction; a corrupted index rebuilds itself on open; the migration backfills in chunks and resumes cleanly.
  - **Opt-in bilingual export** (M15.8): explicit per-transcript translation through the configured Polish endpoint (bounded batches, per-segment failure naming, retry, cancellation), results stored editable with full provenance, staleness tracked through every edit/split/merge, stacked bilingual SRT/VTT/ASS output with per-line CPL/CPS advisories; **zero endpoint traffic unless you enable it** — asserted by tests.
  - **Word-level timings + karaoke playback** (M15.6): word timing comes from whisper's token timestamps (read-only — no inference change); measured start errors on the pinned tiny model: **EN median 11 ms, ID median 112 ms**. Word chips are click-to-seekable, the current word highlights with a spring during playback (Reduce Motion removes only the spring), and any edit marks the timing stale (sticky across undo) with a segment-seek fallback when stale.
- **Decode parity** (M15.2): the WebM/Ogg-Opus/mkv-opus import path is pinned by sha256-tagged fixtures (EN + Indonesian) and fails cleanly on truncated containers; the Android import path will reuse this same interface (M13c).

### Changed
- Schema v1 → v6 additive migrations; every mutation keeps the original inference text; corpus of 10k segments searches at <16 ms worst case on a 2-core machine.

## [0.3.1] - 2026-10-21

### Fixed
- **macOS launch crash in v0.3.0 — `@rpath/libswift_Concurrency.dylib` not found** — the app aborted at launch (`Termination Reason: Namespace DYLD, Code 1, Library missing`) on every machine. The release binary links the Swift bridge (`speech-transcriber`, M10B), and the concurrency runtime is the one Swift dylib macOS loads through `@rpath` rather than an absolute path; the bundle's only `LC_RPATH` was `Contents/Frameworks`, which has no such file. Fixed by adding `/usr/lib/swift` to the rpath of both the app and the CLI binaries. Two gates now prevent a repeat: CI packages the bundle with the **same features as the release** (`speech-transcriber,updates` — it previously built `updates` only, so the Swift linkage never entered the smoke), and `scripts/mac-check-dylibs.sh` statically fails the CI job and the release when any `@rpath` dependency cannot resolve.
- **UI: the macOS cross-thread repaint deadlock is gone** — off-thread (worker/bridge) repaint wakes now go through winit's user-event channel (`EventLoopProxy::send_event`) instead of a synchronous main-thread dispatch made while egui's context lock was held; a CI stack sample caught the cycle on the runner (`take_egui_input → lock_shared_slow` on main vs `request_repaint → dispatch_sync(main)` on the bridge). Also unblocks the macOS CI smoke run.

## [0.3.0] - 2026-10-16

### Added
- **In-app updates (M12, owner-requested)**: the packaged macOS app now checks for a new version in the background and installs it **in place after your confirmation** — Sparkle 2.9.6's standard UI shows the release notes (this changelog's section for the release) and asks before installing; a **"Check for Updates…"** item sits in the app menu, and Settings gains an **Updates** section with "Automatic updates" (**ON** by default; `settings.updates.auto_check`). The feed is an **EdDSA-signed `appcast.xml`** attached to each GitHub release, generated by the release workflow from the packaged zip + a same-named `.md` of the changelog section (`generate_appcast --ed-key-file -` from the `SPARKLE_ED_KEY` secret; absent secret → a loud skip and the release ships without a feed). The whole updater is behind the default-OFF `updates` cargo feature (macOS-only) and the pinned, sha256-verified Sparkle 2.9.6 distribution (`scripts/fetch-sparkle.sh`); `make-app.sh` embeds `Sparkle.framework` into `Contents/Frameworks` with the `@executable_path/../Frameworks` rpath and ad-hoc signs the nested code. One-time owner step: generate the keypair (`generate_keys`), put the public key into `packaging/Info.plist` (a clearly-marked placeholder ships until then — the app logs a loud reminder), store the exported private key as the repo secret `SPARKLE_ED_KEY`. The full update cycle is owner-verify (`docs/USER_SMOKE.md` §32); report: `docs/reports/M12.md`
- **Audio-aware polish (M11, owner-requested)**: the optional LLM polish gained a second data point — the **recording** — behind a new Settings toggle, **"Send the recording too (audio-aware polish)"** (default OFF; `settings.polish.send_audio`). When on and a recording is available, the recording is sent as OpenAI-compatible **`input_audio`** content parts (base64 WAV — the 16 kHz mono PCM16 recordings need **no encoder**; imported files are decoded + resampled through the pipeline's own path), **chunked at segment boundaries into ≤ 4 MB (~2 min) raw chunks** — one request per chunk, the corrected outputs concatenated in order. The ladder degrades, never fails hard: a **missing recording** is text-only + a note; an endpoint that **rejects the audio (4xx)** gets **one automatic text-only retry** + a note naming the status and the endpoint's reason; the notes surface with the review like every polish message. With the toggle **off**, the request is **byte-identical** to the pre-M11 text-only one (pinned by tests at the builder level and on the wire). The privacy copy (Settings caption, README, USER_SMOKE §31) states that the recording itself is uploaded when the toggle is on — it stays the app's one opt-in upload path. Tests: exact request shape, chunker coverage/order/segment-alignment/caps, and mock-endpoint runs (audio part shape + coverage + 400 fallback + OFF-identical) — no real endpoints anywhere. Report: `docs/reports/M11.md`

## [0.2.0] - 2026-10-15

### Added
- **Native Opus import — WebM, `.opus` voice notes, mkv-opus (M10C)**: Opus audio decodes **in-process** through the single decode path (a `symphonia-adapter-libopus` decoder registered on the codec registry; libopus compiled into the binary via `cc` — BSD, the whisper.cpp-class bundled C, nothing shipped separately, **no ffmpeg anywhere in the flow**). `.webm`/`.opus` joined the import dialog's filter, and the old actionable conversion recipe is now only for genuinely unsupported leftovers (WMA/AIFF/ALAC). Decode + real tiny-model transcribe of the committed webm, mkv and raw Ogg-Opus fixtures are pinned by tests (`docs/reports/M10c.md`)
- **Parakeet fast engine — opt-in (M10A)**: a "Fast engine (experimental)" toggle (Settings → Transcription; default OFF) and a `--fast` CLI flag route **covered languages** (the 25 European languages of Parakeet TDT 0.6B v3 int8) through **sherpa-onnx** — Rust-native ONNX with the Core ML execution provider on macOS — behind a `parakeet` cargo feature the release builds do not enable yet. **Indonesian never routes to Parakeet** (a hard invariant asserted across the routing matrix and end-to-end); auto-detect and uncovered languages stay whisper; a covered language without the model installed warns and falls back, never fails. The model (4 files, ~670 MB, per-file sha256 pins) installs with `suramak-cli models download parakeet`; `doctor` reports build/model/coverage/setting; the editor and CLI name what ran (`parakeet (fast)`). VPS-measured (CPU-only, the 9 s English fixture): 4.2× realtime vs whisper-tiny's 3.4×; model load ~6 s (cached per worker). Apple-silicon/ANE timing is owner-verify. **Not shipped in release artifacts yet** — the sherpa-onnx shared libraries need app-bundle packaging first; source builds enable it with `--features parakeet`. Report: `docs/reports/M10a.md`
- **Apple SpeechTranscriber fast engine — opt-in (M10B)**: on macOS 26+ the "Fast engine" toggle prefers Apple's on-device **SpeechTranscriber** (the SpeechAnalyzer pipeline, ANE-based; ~10 languages / 30 locales) via the Rust `speech` crate — locale resolution, the one-time speech-recognition authorization (plus `NSSpeechRecognitionUsageDescription`), **OS-managed assets** (nothing pinned, downloaded, or bundled by the app), and a conservative span → segment mapping. On macOS the order is **SpeechTranscriber → Parakeet → Whisper**; elsewhere Parakeet → Whisper; **Indonesian never routes to either fast engine** (hard invariant, tested end-to-end). Any fast-engine failure **degrades to Whisper** with the blocker named, never a hard failure. Landed opt-in; the auto-default flip happens after the owner's measured pass. **Shipped enabled in the macOS release artifact** (the setting stays OFF by default); Linux/Windows builds don't compile it. Report: `docs/reports/M10b.md`

## [0.1.1] - 2026-10-15

### Added
- **Live transcription while recording** (M7): the record view now grows a clearly-labeled **provisional** transcript while capture runs — the incoming audio (the same 16 kHz stream the recording uses, mixed by the M6 engine when several sources are active) is decoded in ~4 s windows with 1.5 s of context overlap through the **same whisper path** as the session (same model / language / initial prompt), on its own decode thread behind a bounded **drop-oldest** queue. A **skip-and-merge backlog guard** keeps capture unblocked: if a window closes while the previous decode still runs it is skipped and merged into the next one, with a per-session skipped count that is logged and surfaced subtly in the record view (live updates may lag on slower machines; the recording never does). The provisional text is discarded on **Stop** — the existing full-file pass always runs and **replaces** it, byte-identical to the record-then-transcribe flow for the same input (pinned by a tiny-model e2e and by the final pipeline being untouched)
- **Multi-source recording** (M6): the record source picker is now a **checkbox list** — two mics, or **mic + system audio** (the meeting-transcription combo) — recorded concurrently and mixed into the one 16 kHz mono WAV by a pure, Linux-testable mixing engine (`suramak-asr::mix`). Per-source gain/mute while recording; a documented clipping policy (per-source gain before the sum, hard saturation only at the PCM boundary, counted and reported); and measured-clock **drift correction** (each source resampled at its measured rate — never clicks; the drift tests show naive mixing sliding 35 ms over 3 minutes at 250 ppm while the corrected mix stays within a sample)
- **Per-app capture** (M6): "Only an app…" records just one running app's audio — `suramak-taps` gained `ProcessSelection::Only` (`initStereoMixdownOfProcesses:`) and a running-audio-processes listing; the Settings picker ticks apps (bundle-id keys, resolved to live pids at record time — a missing app fails with an actionable message, never a silent widening)
- Source list persisted in `settings.toml` (`[[recording.sources]]`, additive; the legacy `audio_source` field follows the first entry and remains the fallback for older files)
- Import video files (`.mp4` / `.mov` / `.m4v` / `.mkv`): the first audio track is transcribed through the identical pipeline as audio imports (library row → transcribe → editor → exports; SRT/VTT become video subtitles), with a test-verified container matrix from real ffmpeg fixtures. Unsupported audio (WebM/Opus, or any container this build has no reader for) fails with an actionable conversion hint — `cannot decode clip.webm: the audio codec (Opus) is not supported — convert it first, for example: ffmpeg -i "clip.webm" "clip.mp4"` — instead of a raw decoder error (M5)
- In-app playback with segment follow: play/pause + moving playhead on the waveform strip, click-to-seek (the strip and any segment's timestamp), a `mm:ss / mm:ss` readout, and the active segment highlighted and scrolled into view while playing. Missing audio and a missing output device are explicit states, never a dead control (M8)
- **Full UI remake (M9)** — the shell was remade as one coherent pass through `impeccable`, not patched screen-by-screen: the **Deck** composition (title band, painted transport capsule, waveform strip, mono readout), one painted control system (three tiers × five states; the title-line trash/Polish/Undo controls are real translucent capsule buttons), a single material ladder (real Liquid Glass on macOS → DWM frosted on Windows → frosted treatment on Linux), and an **Appearance** setting (system-following, with a Light/Dark override applied live). Transcript rows gained a mono `hh:mm:ss` gutter, serif measure-capped text, and an active-segment edge bar; exports moved to an anchored sheet (⌘E)
- **Motion, first-class (M9)** — a motion grammar with shared tokens, the floating pill's gentle spring (no bounce), a living dot while recording, eased waveform seeks + animated transcript follow-scroll, and Reduce-Motion support throughout (contract: `.impeccable/motion-brief.md`)
- **Real waveform peaks + repaint budget + accessibility floor (M9b)** — playback draws the recording's actual peak envelope; repaint heat went event-driven (idle: 873 → 9 frames / 30 s; job: ~19 → ~1.5 fps) with an exit-time frame report (`SURAMAK_FRAME_STATS=1`); every painted control carries a label + focus ring, with a central keymap (Space / ⌘R / ⌘E / Esc / ↑↓ / ⌘⌫) guarded against text fields
- **Quit-safety + job control (M9e)** — quitting mid-recording now asks (Stop & Save / Discard Recording / Keep Recording; Esc keeps); **Stop & Save** preserves the recording and finishes its transcription automatically on the next launch; an in-flight transcription job gets a quiet **Cancel** (Esc too); Settings/Models scroll correctly at the app's 900×600 minimum window

### Changed
- Settings → **Audio sources**: the single-source dropdown became the checkbox list (≥1 source required to save; capture-less builds show it read-only with the existing copy)
- The floating record control replaces the titlebar red dot: bottom-center, a real Liquid Glass pill on macOS 26+ (`NSGlassEffectView`, egui-painted glyph/label/live `mm:ss` on top) and an honest painted pill on Linux/Windows/older macOS; click starts, click stops (the same finalize flow as the record view's Stop), and it stays bottom-centered on resize (M8.5)
- Copy sweep + readability: exactly two time formats (elapsed `hh:mm:ss`, transport `mm:ss.mmm`), human model names everywhere (`Tiny (fastest)`, …), count-aware pluralization; the record red is text-safe for AA contrast; the editor header is built from explicit pixel constants (the macOS-font-metric-only overlapping-rule class is structurally gone)
- Brand: the app mark updated to **A3 “Speak to Text + Live Record Dot”** — all vector masters + icon sets (macOS/Windows/Linux/web) + the README lockup regenerated and probe-verified

### Fixed
- **Quitting is clean and silent in every path** — menu Quit, Cmd-Q and window close, after any amount of whisper / live-transcription / playback activity, including quitting **while recording** or **while a job runs**: every quit flavor now runs one explicit shutdown step (capture stopped through the existing stop path, the live decode thread and the worker bridge signalled and waited for at most ~250 ms, playback `Stop`ped — a job in flight is never interrupted) and then exits hard, so ggml's macOS Metal-device teardown assert (`GGML_ASSERT([rsets->data count] == 0)` in `ggml_metal_device_free`, the llama.cpp#17869/#19137 class) can never fire after a whisper session. No more `zsh: abort` or crash-report noise on quit; smoke exit codes are unchanged. (Owner-verified: docs/USER_SMOKE.md item 19; rationale in docs/PLAN-NOTES.md.)
- The benign stop-time `audio capture error: The requested device is no longer available …` line no longer prints on every Stop (it is the teardown race of the stream being dropped); genuine mid-recording capture errors still print exactly as before, and the teardown-time variant is a debug-level line
- Settings and Models no longer share egui scroll state (scrolling Settings then opening Models landed mid-table) — the scroll areas are id-isolated with a regression test

## [0.1.0] - 2026-10-13

### Added
- On-device transcription (whisper.cpp; CPU / Metal / **Core ML-ANE** tiers with automatic fallback + honest reporting) — English + Indonesian
- Import audio files (wav/mp3/m4a/flac/ogg) via drag-and-drop, file picker, or path
- Microphone recording with live RMS meter + rolling waveform; **system audio via CoreAudio process taps** (macOS 14.2+, no kernel drivers)
- Editable timestamped transcript with debounced autosave; sqlite library; export md/txt/json/srt/vtt
- In-app model manager (download with progress, delete, set default) and full settings screen
- Optional LLM “polish” via your own OpenAI-compatible endpoint (key stored in the OS keychain)
- macOS 26 **Liquid Glass** shell (fallback material on older macOS)
- Custom language codes in Settings — presets plus any whisper code you type (unofficial codes are best-effort)
- Delete transcripts from the library (trash affordance with an in-place confirm)

### Fixed
- macOS 27 hardening (all found through owner smoke testing): the Liquid Glass re-parenting could abort when a modal panel (open/save dialog) ran; two winit IME crashes around text-field focus (nil input context; `set_ime_allowed` release) — fixed via a pinned winit fork plus keeping the app IME-free on macOS; system-audio taps now translate pids to process objects and never probe the HAL immediately after aggregate creation

### Changed
- Linux is a shipping target from this release — the release page gains a Linux app tarball next to the macOS build
- A second app instance exits with a clear message instead of fighting over the library (single-instance guard)

### License
- GPL-3.0-or-later
