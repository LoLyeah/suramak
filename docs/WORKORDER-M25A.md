# WORKORDER — M25A: the long-recording gate (can Suramak hold a 90-minute meeting?)

**This is a measurement gate, not a feature.** Its deliverable is a report with
numbers and a GO / STOP verdict; no product code ships from it. The next
feature slice is written from its answer.

## Why it exists

`decode_file` builds the whole recording as `Vec<f32>` (16 kHz mono = 64 KB per
audio-second). That is fine for a voice note and a 40-minute lecture, and it is
the reason the Android diarization gate came back BORDERLINE: a 30-minute file
projects to ~265 MB of samples on top of the ONNX sessions. Meetings are
45–120 minutes. Before promising meeting capture, measure where this path
actually breaks and whether the existing chunking machinery can carry it.

## Questions the gate must answer with numbers

1. **Where is the ceiling today?** Decode + transcribe a 5, 30, 60 and
   90-minute 16 kHz file (synthesised by repeating a real fixture with silence
   padding between repeats so timestamps stay meaningful). Record peak RSS
   (RSS sampled every 250 ms), wall clock, RTF, and the exact failure if any —
   on this 2-core VPS, with the tiny model and with turbo-q5-0.
2. **Does the existing chunk path hold quality?** The repo already has
   `vad-chunking` (VAD cuts + join). Run the 60-minute file through it and
   compare against the single-pass transcript: segment count, total text
   similarity (normalised WER on the concatenated text), and timestamp drift at
   10 sampled positions. State the delta, not a vibe.
3. **What does a disk-backed decode actually buy?** Measure a variant that
   decodes to a temp 16 kHz f32 file and streams windows from it (no new
   crates: `std::fs` + the existing decode loop). Report peak RSS for the same
   files and whether the transcript is byte-identical to the in-memory run.
4. **What is the honest cap to ship?** From the numbers, name the duration
   that fits a 1 GB budget on a 4 GB phone and on the VPS, and whether the
   chunked path is required or merely nice.

## Method

- Fixtures: a real multi-speaker clip from `fixtures/` repeated to length, with
  0.8 s silence between repeats; record the exact build command and sha256.
- Peak RSS: `\time -v` or `/proc/<pid>/status VmHWM` sampling; state which.
- Quality metric: the repo's own text normaliser if present, else a documented
  word-level Levenshtein over lowercased alphanumeric tokens.
- Disk: `cargo clean` before starting if free space < 12 GB; the 90-minute
  fixtures are ~350 MB each, delete them at the end.

## Deliverable

`docs/reports/M25a-long-recording-gate.md`: a decision block first
(GO with mechanism X / STOP with reason), then the tables (duration × RSS ×
RTF × verdict), the chunk-quality delta, and the recommended cap. Report only;
no source changes.

## Do NOT

- No product code, no Cargo feature flips on `main`, no CI edits, no new
  crates, no commits, no sudo. Experiments live in a scratch worktree under
  `/home/ubuntu/.hermes/cache/scratch/`.
