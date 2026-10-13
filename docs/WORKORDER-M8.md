# WORKORDER-M8 — Playback + segment follow

Base: master at the v0.1.0 tag. First post-release milestone (owner-agreed order:
M8 → M8.5 → M5 → M6 → M7 → M9). Owner tests real audio on the Mac afterwards;
the VPS is the build/test surface here.

## Goal

Play the transcript's source audio inside the editor:
- play/pause control on the waveform strip + a moving playhead (position line);
- click anywhere on the strip to seek; time readout (current / total, `mm:ss`);
- clicking a segment seeks to its start;
- the **active segment** (containing the playhead) is highlighted and
  auto-scrolled into view while playing.

## Architecture (follow the established patterns)

- **New crate `suramak-playback`** (workspace member; deps: `suramak-core` for
  decode + transcript types, `cpal` for output, `rubato` if resampling is
  needed — it is already in the workspace). Linux is fine: `cpal`'s ALSA backend
  builds with `libasound2-dev` (CI + VPS have it); no capture feature involved.
- **Engine thread** owns the `cpal` output stream. Commands from the UI thread:
  `Play { from_ms }`, `Pause`, `Resume`, `Seek(ms)`, `Stop` (channel). Position
  reported via `Arc<AtomicU32>` (millis), polled per frame — exactly the
  capture-RMS pattern (`runtime.capture_rms()`).
- **Decode**: reuse `suramak_core::decode` (16 kHz mono f32 — the samples
  whisper already consumes). If the output device doesn't accept 16 kHz,
  resample to its config with `rubato`.
- **Failure paths must be clean, never panic**: no output device (the headless
  VPS has none) → UI shows a "no audio output device" state; missing source
  file → unavailable state (below), engine not opened.
- **Source resolution** — pure function, unit-tested:
  `resolve_playback_source(&Transcript) -> PlaybackSource` (in `suramak-core`
  or the playback crate, wherever it reads cleanest):
  - imported file → `transcript.source` if the path still exists;
  - recording with kept audio → its kept path;
  - gone (recording with `keep_audio` off — temp deleted) →
    `Unavailable { hint: "keep audio" }`.
  Prefer NO new DB field; if the kept-recording path cannot be derived
  reliably, add the minimal field + migration and note it as a deviation.
- **UI** (editor):
  - waveform strip: play/pause glyph, playhead line (`palette.accent`),
    click-to-seek, `mm:ss / mm:ss` readout;
  - segment list: active-row highlight + `scroll_to_rect` auto-follow while
    playing (do not yank the scroll when paused);
  - click a segment → seek (keep playing if playing);
  - position → active segment: binary search over segment starts — pure fn,
    unit tests for the boundary cases (before first / inside / gap / after);
  - leaving the editor or deleting the transcript stops playback and resets
    position; starting a recording stops playback first;
  - the event loop's `busy()` predicate includes "playback active" so frames
    keep coming while the playhead moves.

## Tests

- source-resolution matrix (exists / missing / keep_audio off);
- position → segment mapping boundaries;
- UI state machine transitions (idle → playing → paused → stop-on-leave;
  unavailable path).
Audio output itself is macOS/manual — the VPS has no device.

## Also

- `docs/USER_SMOKE.md`: item 14 — playback: open an imported transcript, play,
  verify highlight follows + seek works; the missing-audio case shows the hint.
- `README.md`: one bullet (in-app playback with segment follow).
- `CHANGELOG.md` [Unreleased] → Added: in-app playback with segment follow.
- `docs/reports/M8.md`: changes, evidence, deviations.

## Acceptance (from /home/ubuntu/suramak; report exact outputs)

1. `cargo fmt --check` — clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` — clean.
3. `cargo test --workspace` — all green + the new tests.
4. `cargo build -p suramak-ui && SURAMAK_SMOKE_SECS=3 xvfb-run -a -s "-screen 0 1024x768x24" ./target/debug/suramak`
   — smoke ok; with no audio device the app must still start and the editor
   must render the unavailable state gracefully.
5. `cargo check -p suramak-playback --target aarch64-apple-darwin` — required
   to pass (cpal's coreaudio backend is pure Rust + frameworks). If it cannot
   pass from Linux, document exactly why in the report.

## Constraints

- Only modify files inside /home/ubuntu/suramak. No sudo. No git commit/push.
- Do not touch `suramak-glass`, `suramak-taps`, the winit fork, `scripts/`,
  or `crates/suramak-ui/src/instance.rs`.
- Keep every existing test green; Linux capture stays OFF (playback must not
  pull the capture feature in).
- No on-disk format changes without an explicit deviation note in the report.
