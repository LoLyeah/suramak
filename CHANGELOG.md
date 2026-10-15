# Changelog

All notable changes to Suramak. Format: [Keep a Changelog](https://keepachangelog.com/); versioning per `docs/VERSIONING.md` (semver).

## [Unreleased]

### Added
- **Parakeet fast engine — opt-in (M10A)**: a "Fast engine (experimental)" toggle (Settings → Transcription; default OFF) and a `--fast` CLI flag route **covered languages** (the 25 European languages of Parakeet TDT 0.6B v3 int8) through **sherpa-onnx** — Rust-native ONNX with the Core ML execution provider on macOS — behind a `parakeet` cargo feature the release builds do not enable yet. **Indonesian never routes to Parakeet** (a hard invariant asserted across the routing matrix and end-to-end); auto-detect and uncovered languages stay whisper; a covered language without the model installed warns and falls back, never fails. The model (4 files, ~670 MB, per-file sha256 pins) installs with `suramak-cli models download parakeet`; `doctor` reports build/model/coverage/setting; the editor and CLI name what ran (`parakeet (fast)`). VPS-measured (CPU-only, the 9 s English fixture): 4.2× realtime vs whisper-tiny's 3.4×; model load ~6 s (cached per worker). Apple-silicon/ANE timing is owner-verify. Report: `docs/reports/M10a.md`

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
