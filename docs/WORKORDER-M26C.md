# WORKORDER — M26C: macro-windows, so duration stops being a memory problem (Slice 3)

Design: `docs/reports/M26-memory-design.md` §C (the cut rules, the prompt
firewall, the seam stitch) and §E slice 3. Slices 1–2 are in: the cap is
device-derived and the audio is streamed to a temp `.pcm32` and mapped. What
still grows with duration is whisper's own **mel spectrogram**, allocated for
the whole slice it is handed (1.92 MB/min tiny, 3.07 MB/min turbo). Hand it
15-minute windows and the peak becomes flat.

**This slice is gated on quality.** If it cannot hold |ΔWER| ≤ 0.5 pt against
single-pass on natural speech, it STOPS and the app keeps the current cap.

## Deliverables

1. **`MacroChunkPipeline`** over the existing `PcmSlice`:
   - window target 15 min, boundary search in `[cursor+840 s, cursor+930 s]`;
   - RMS over 20 ms frames, threshold `max(1e-4, percentile15(E) × 1.8)`;
   - Rule C1: cut at the midpoint of the longest silent run ≥ 400 ms in the
     search band; overlap 1.5 s;
   - Rule C2 (no silence): the minimum-energy 200 ms window in
     `[cursor+870 s, cursor+915 s]`, overlap 4.0 s, and the part is flagged
     `cut_without_pause`;
   - one `WhisperState` reused across windows; `madvise(MADV_DONTNEED)` after
     each window.
2. **Prompt firewall** (this is what stops the repetition loops): at each cut,
   carry at most 160 tokens from the accepted, non-overlapped tail of the
   previous window — and only when the tail passes all three checks: unique
   byte-4-gram ratio > 0.45, mean token probability ≥ 0.45, and no phrase of
   1–6 words repeated ≥ 3 times. Fail any check → carry **zero** tokens.
3. **Decoder guards** (whisper-rs params, verified present in 0.16):
   `no_context = true` plus explicit `set_tokens`, `temperature_inc = 0.2`,
   `entropy_thold = 2.4`, `no_speech_thold = 0.6`, `logprob_thold = -1.0`,
   `n_max_text_ctx` bounded, token timestamps on.
4. **Deterministic seam stitch**: keep words whose midpoint ≤
   `cut - overlap/2` from the left window and > it from the right (times shifted
   by the cursor); then drop the longest suffix-of-previous / prefix-of-next
   common run (up to 12 normalised words, Levenshtein ≤ 1 for words ≥ 5 chars;
   for a single-word match require an exact match and < 0.8 s gap).
5. **Wiring**: when the mapped slice is longer than the window target, the
   pipeline uses the macro path; otherwise the single call is unchanged, so
   existing transcripts stay byte-identical. `max_minutes` becomes the disk/
   safety ceiling from §D instead of the memory slope, and the model-fit gate
   uses `M_peak(15 min)`.
6. **Progress + cancellation** stay exact: progress is by samples consumed
   across windows; cancel mid-window stops within one window.

## Verification (the gate)

- **Natural-speech corpus**: build a ≥ 60-minute file by concatenating distinct
  real clips (not one clip repeated; name the sources and sha256 in the report)
  and compare macro vs single-pass: **|ΔWER| ≤ 0.5 pt**, segment counts within
  2 %, and no segment collapsing to a repeated phrase.
- **Stress fixture** (the repeated 167 s clip): **zero** repetition loops —
  defined as no 4-gram repeated more than 4× consecutively.
- **Memory**: a 90-minute file on tiny peaks **≤ 450 MB** and within ±15 MB of
  the 30-minute run (flat). Report the real VmHWM.
- **Byte-identical path preserved**: a 5-minute file still takes the single
  call and its transcript digest equals the pre-slice build.
- fmt/clippy -D warnings; NDK check + clippy on both Android crates; one full
  workspace run (baseline 1522).
- Report: `docs/reports/M26c-macro-windows.md` — decision block first
  (GO/STOP), then the WER table, the loop count, the memory table, and the
  exact cut positions chosen on the corpus.

## Do NOT

- No `whisper_full_parallel`; no VAD pre-cutting into short segments; no
  silent model downgrade; no new crates; no CI edits; no git commit/push; no
  sudo. If the gate fails, STOP is the deliverable — the cap stays and the
  report says why.
