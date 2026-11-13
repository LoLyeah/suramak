# WORKORDER — M26B: stream the audio to disk, then map it (Slice 2)

Design: `docs/reports/M26-memory-design.md` §B and §E slice 2. Today the
pipeline holds the whole recording twice: `decode_file` builds `Vec<f32>` and
`prepare_samples` resamples into a second one. This slice removes both full-file
allocations without changing a single output byte.

## Deliverables

1. **One streaming decode path**: `decode_to_pcm16k_mono(input_path, out_path)`
   decodes + resamples incrementally in bounded chunks and writes the result to
   `{temp}/suramak_{job_id}.pcm32` (16 kHz mono, `f32le`) — no full-file
   `Vec<f32>`, no full-file resample buffer. Peak working set must be bounded by
   the chunk size (target ≤ 8 MB of samples held at once), not by duration.
2. **A fast path for our own recordings**: a WAV we wrote is already 16 kHz mono
   PCM16; its `data` chunk is mapped directly (no decode, no re-encode). A test
   asserts the samples from the fast path are **byte-identical** to the decode
   path for the same file.
3. **`PcmSlice` reader**: unix/Android uses `libc::mmap` (`MAP_PRIVATE`,
   `posix_madvise(MADV_SEQUENTIAL)`) over the `.pcm32`, exposing `&[f32]`;
   Windows falls back to `File::read_exact_at` into one window buffer (documented
   as the difference — no new crate). RAII `Drop` unmaps and deletes the temp
   file; a startup sweep deletes orphaned `suramak_*.pcm32` left by a kill.
4. **Whisper gets the same slice it used to**: the transcription call signature
   changes from `&[f32]` to `PcmSlice`, everything else (params, timestamps,
   cancellation, progress) unchanged. Output must be byte-identical to the
   current build on the same input — that is the acceptance test, not a vibe.
5. **Disk preflight**: before writing the temp file, require
   `free_bytes ≥ duration_seconds × 64_000 + 100 MB`; refuse with a named
   reason when it does not fit (and never leave a partial temp file behind).

## Verification

- Byte-identical transcripts: run 3 fixtures (short en, id, a 20-minute
  synthesised file) through both the old in-memory path (kept behind a
  test-only helper) and the new streaming path; assert identical segment text,
  start/end ms, and segment count.
- Peak RSS: a 30-minute file through the new path must peak **≥ 200 MB below**
  the in-memory path on the same box; record the real numbers.
- Temp hygiene: success, cancellation mid-decode, and a simulated panic all
  leave zero `suramak_*.pcm32`; a startup sweep removes a planted orphan.
- Windows fallback: the window reader is host-tested on Linux (the fallback is
  pure `std::fs`), and the platform selection is asserted by cfg tests.
- fmt/clippy -D warnings; NDK check + clippy on both Android crates (env
  `ANDROID_NDK`/`_HOME`/`_ROOT` + `CMAKE_GENERATOR=Ninja`); one full workspace
  run (baseline 1509).
- Report: `docs/reports/M26b-streaming-decode.md` with the RSS table and the
  byte-identical evidence.

## Do NOT

- No macro-chunking / 15-minute windows (Slice 3); no change to whisper params
  or the queue; no new crates; no change to the WAV we write for recordings
  (only read it faster); no CI edits; no git commit/push; no sudo.
