# WORKORDER-M5 — video import (transcribe the audio track)

Base: master after M8.5. Owner order: M8 → M8.5 → **M5** → M6 → M7 → M9 → M10.
Per `PRODUCT.md` M5 — this completes the original brief ("import audio/video
files").

## Goal

Import **`.mp4 / .mov / .m4v / .mkv`** and transcribe their **audio track**
through the *same* pipeline as audio imports: library row → transcribe →
editor → exports; SRT/VTT become video subtitles. Fixtures with a real speech
track prove it end-to-end on Linux.

## Design

1. **Containers via symphonia first** (the existing decode path in
   `suramak-core`): enable the needed symphonia reader features (`isomp4`,
   `mkv` — exact feature names/versions as the pinned symphonia offers; do not
   bump the crate beyond what the workspace already pins, and if a feature
   does not exist for `.mov`/`.mkv`, say so in the matrix instead of guessing).
   The importer must pick the **first audio track** and ignore video streams.
2. **Honest container matrix**: generate real fixtures with ffmpeg on the VPS
   and record which of the four containers actually decode (test each, don't
   assume): `.mp4/.m4v` (AAC), `.mov` (AAC), `.mkv` (AAC and Opus). Whatever
   does not decode gets a *clear, actionable* import error naming a working
   alternative (e.g. `ffmpeg -i in.webm out.mp4`). **WebM/Opus**: not
   supported — same actionable error. No ffmpeg sidecar in this milestone;
   note it in `docs/PLAN-NOTES.md` as the known escape hatch for later (never
   bundle anything).
3. **Fixtures**: build each video fixture from the existing speech sample (the
   TTS sample mp3 under `build/share/` — real words, tiny model transcribes
   it) **plus** a `testsrc` video stream, so container+video coexistence is
   real: `ffmpeg -f lavfi -i testsrc=... -i <sample>.mp3 -c:v libx264 -c:a aac
   ...`. Keep committed fixtures small (≤ 100 KB each) under the repo's
   existing test-fixture convention (check what the repo already does for
   test media and follow it; if it has none, `crates/suramak-core/tests/
   fixtures/` is fine). Record the exact ffmpeg commands in the report.
4. **UI + import surfaces**: extend every place that knows the accepted
   extensions (file picker, drag-drop filter, CLI import validation — grep for
   the existing audio extension list and extend all of them; missing one is
   the classic bug here). Visible copy: importing a video is unremarkable —
   the editor already shows duration/segments; only the picker copy may say
   "audio or video". No new views.
5. **Tests**:
   - decode tests per container from the committed fixtures (assert sample
     rate/length/audio present; container matrix as documented);
   - an end-to-end: `suramak-cli transcribe <fixture.mp4>` with the tiny model
     (follow how the existing ignored whisper pipeline tests are gated and run
     them enabled here) → the transcript text must match transcribing the
     source mp3 directly (same words);
   - the unsupported-container error path (construct a `.webm`/fake file and
     assert the actionable message, not a panic).

## Also

- `docs/ENGINES.md` (or the decode docs section): the container matrix table
  (container / codec / supported / fallback hint).
- `docs/USER_SMOKE.md`: item 16 — import a real screen recording (.mov/.mp4)
  and a .mkv on the Mac; also the WebM error message.
- `README.md` bullet; `CHANGELOG.md` [Unreleased] Added; `docs/reports/M5.md`.

## Acceptance (from /home/ubuntu/suramak; report exact outputs)

1. `cargo fmt --all --check` — clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` — clean.
3. `cargo test --workspace` — all green + the new tests.
4. The enabled end-to-end video transcribe (acceptance 5 of Tests above) with
   its exact output shown (the transcript words).
5. xvfb smoke — `cargo build -p suramak-ui && SURAMAK_SMOKE_SECS=3 xvfb-run -a
   -s "-screen 0 1024x768x24" ./target/debug/suramak` — ok.
6. `cargo tree -p suramak-core --target x86_64-unknown-linux-gnu` — show the
   symphonia feature set actually enabled (no new crates beyond what the
   workspace already has).

## Constraints

- Only modify files inside /home/ubuntu/suramak. No sudo. No git commit/push.
- Do not touch `suramak-glass`, `suramak-taps`, `suramak-playback`, the winit
  fork, `scripts/`, or `crates/suramak-ui/src/instance.rs`.
- No new dependencies; symphonia feature flags only (plus existing crates).
- Do not change the on-disk format or the pipeline's behavior for
  audio imports; video must ride the identical path.
- Fixtures: small, committed, exact ffmpeg provenance in the report.
