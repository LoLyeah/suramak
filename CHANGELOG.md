# Changelog

All notable changes to Suramak. Format: [Keep a Changelog](https://keepachangelog.com/); versioning per `docs/VERSIONING.md` (semver).

## [Unreleased]

### Added
- On-device transcription (whisper.cpp; CPU / Metal / **Core ML-ANE** tiers with automatic fallback + honest reporting) — English + Indonesian
- Import audio files (wav/mp3/m4a/flac/ogg) via drag-and-drop, file picker, or path
- Microphone recording with live RMS meter + rolling waveform; **system audio via CoreAudio process taps** (macOS 14.2+, no kernel drivers)
- Editable timestamped transcript with debounced autosave; sqlite library; export md/txt/json/srt/vtt
- In-app model manager (download with progress, delete, set default) and full settings screen
- Optional LLM “polish” via your own OpenAI-compatible endpoint (key stored in the OS keychain)
- macOS 26 **Liquid Glass** shell (fallback material on older macOS)

### License
- GPL-3.0-or-later
