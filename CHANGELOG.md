# Changelog

All notable changes to Suramak. Format: [Keep a Changelog](https://keepachangelog.com/); versioning per `docs/VERSIONING.md` (semver).

## [Unreleased]

### Added
- Import video files (`.mp4` / `.mov` / `.m4v` / `.mkv`): the first audio track is transcribed through the identical pipeline as audio imports (library row → transcribe → editor → exports; SRT/VTT become video subtitles), with a test-verified container matrix from real ffmpeg fixtures. Unsupported audio (WebM/Opus, or any container this build has no reader for) fails with an actionable conversion hint — `cannot decode clip.webm: the audio codec (Opus) is not supported — convert it first, for example: ffmpeg -i "clip.webm" "clip.mp4"` — instead of a raw decoder error (M5)
- In-app playback with segment follow: play/pause + moving playhead on the waveform strip, click-to-seek (the strip and any segment's timestamp), a `mm:ss / mm:ss` readout, and the active segment highlighted and scrolled into view while playing. Missing audio and a missing output device are explicit states, never a dead control (M8)

### Changed
- The floating record control replaces the titlebar red dot: bottom-center, a real Liquid Glass pill on macOS 26+ (`NSGlassEffectView`, egui-painted glyph/label/live `mm:ss` on top) and an honest painted pill on Linux/Windows/older macOS; click starts, click stops (the same finalize flow as the record view's Stop), and it stays bottom-centered on resize (M8.5)

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
