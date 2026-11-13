# M25A — the long-recording gate

**Status:** measurement gate, report only. No product code, no commits, no feature flips.
**Box:** 2 vCPU AMD EPYC 7K62, 3.7 GiB RAM, 6 GiB swap, Linux 7.0.0-14, rustc 1.99.0.
**Date:** 2026-11-13.

## 1. Decision

**GO — for a capped meeting, not for the 90-minute meeting.**

* **GO:** in-memory single-pass transcription with **tiny**, capped at **45 minutes on a
  4 GB phone** and **60 minutes on this 2-core VPS**. 60 min measured **859 MB** peak RSS
  on the VPS (fits a 1 GiB process budget with 165 MB headroom); 45 min interpolates to
  **722 MB**, which leaves room for the ~150 MB Android/app baseline on a 4 GB phone.
* **STOP — 90 minutes on the current path.** 90 min measured **1,135 MB** peak RSS, 111 MB
  over a 1 GiB budget and +32 % over the 60-minute point. Nothing failed on this 3.7 GiB
  VPS (the job completed in 13.0 min), so the ceiling is the memory budget, not the OS:
  the phone's LMKD kills the app before the file finishes.
* **STOP — turbo-q5-0 as a meeting model on this hardware.** Its 1-minute run peaked at
  **1,047 MB** (already over 1 GiB) and took **308 s** (RTF **5.13**). The 5-minute run was
  killed after 18 min 04 s at 1,100 MB with no transcript; a 90-minute job projects to
  **~7.7 hours** on this box. It is not a duration-cap question; it is a no-go.
* **STOP — the existing VAD chunk path as a quality-preserving mechanism.** At 60 min it
  lowers peak RSS to **533 MB** (−38 %) but costs **+53 % wall time** and **+21.74 pt
  normalised WER** against the single pass (9,890 → 9,321 words, 912 → 794 segments).
  It duplicates/shortens text at chunk boundaries; it is a memory technique, not a
  meeting path.
* **Disk-backed streaming is the right direction but not shippable as measured.** A
  30-minute-window streaming decode cut the 90-minute peak to **600 MB** (−47 %) at the
  same wall time, but the transcript was **not byte-identical** and fixed window cuts
  trigger whisper repetition loops: **+12.12 pt WER at 90 min** (the 30–60 min window
  alone is +27.25 pt) and **+16.88 pt** at 60 min with 15-minute windows. The first 15-minute window is
text-identical (the first 30 minutes of the 30-minute-window run are within 0.04 %); every
cut after it degrades the next window.

**Recommended honest cap to ship:** **45 min (4 GB phone) / 60 min (VPS)** with tiny and
the in-memory single pass. Sessions longer than that need the windowed path with
silence-aligned cuts and prompt/overlap continuity — a separate slice that must re-run
this gate before it is promised.

## 2. Method

The probe reproduces the product front half exactly
(`pipeline::prepare_samples`: `decode::decode_file` → `resample::to_16khz_mono`) and then
calls `whisper::transcribe` with the product parameters (greedy, `n_threads =
default_threads()` = 2, token timestamps on, `from_ms = 0`). Each measurement runs in its
own process so peak RSS is attributable. Peak RSS is **`/proc/<pid>/status VmHWM`**, with
`VmRSS` sampled every 250 ms; both are reported (they agree within 0.2 MB except the
chunk path, where the sampler misses a transient — `VmHWM` is authoritative).

The disk variant streams a temporary 16 kHz f32 file in fixed windows: WAV PCM16 decoded
with `std::fs` (sample conversion identical to `decode.rs`'s
`(i16 as f32 / 32768.0).clamp(-1, 1)`), written to `$TMPDIR`, then read back window by
window with `read_exact` into the only full-window buffer. No new crates; the temp file is
deleted at the end of the run.

### Fixture

No multi-speaker clip exists under `fixtures/` (the committed fixtures are 9.0 s / 11.9 s
single-voice clips). The gate therefore repeats the real multi-speaker M16H conversation
already on the box, `/tmp/m16h-fixture/long_175s.wav` (167.47 s, 16 kHz mono, sha256
`cd076789bbe633ab72ee4a49d938c2f565bc0c55da3f058d143cbbabe0f50f55`), with **0.8 s of
silence between repeats**, truncated/padded to the exact target duration by
`tools/m25a/build_fixtures.py` (sample-exact, no ffmpeg):

| file | bytes | sha256 |
|---|---:|---|
| `m25a-5m.wav` | 9,600,044 | `18091d9192566ccdb78255b96c9b9e8fe2dffdaa46ce69a6a2719e67d5ddf03d` |
| `m25a-30m.wav` | 57,600,044 | `2492bb905b3251c8edc526af7c0b113798b762d09282fb18b020ece21cb825c9` |
| `m25a-60m.wav` | 115,200,044 | `65cfd0f5c5ed22fa326b25fd9c92a0cb37deb4f43e1f324448f0413d07404e3f` |
| `m25a-90m.wav` | 172,800,044 | `b022443f9332b26ab1525c47e08ab39ba64fdb1fff5f7bb4be69318c6f7ccb30` |

Models: tiny `ggml-tiny.bin` sha256 `be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21`;
turbo-q5-0 `ggml-large-v3-turbo-q5_0.bin` sha256
`394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2` (matches the repo's
pinned sha256). VAD: `silero_vad.onnx` sha256
`9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6`.

`cargo test -p suramak-asr --release --features vad-chunking --test m25a_gate -- --ignored
--nocapture` with `M25A_MODE/M25A_INPUT/M25A_MODEL/M25A_OUT`; `M25A_WINDOW_MS` selects the
disk window. Raw JSON per completed run is in
`/home/ubuntu/.hermes/cache/scratch/m25a-results/`; the killed turbo 5-minute attempt
produced no JSON (its peak is the `VmHWM` read before the kill).

## 3. Where the in-memory path breaks — tiny

`decode_file` holds the whole file as `Vec<f32>` (64 KB per 16 kHz mono audio-second) and
`to_16khz_mono` briefly holds a second copy. The transcription state then keeps a mel
spectrogram for the whole buffer. The measured peak is linear in duration:

| audio | wall | RTF | peak RSS (sampled) | VmHWM | decode | transcribe | segments | words | vs 1 GiB |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 5 min | 50.9 s | 0.170 | 354.6 MB | **354.3 MB** | 0.04 s | 50.9 s | 85 | 814 | +669 MB free |
| 30 min | 281.2 s | 0.156 | 583.8 MB | **583.7 MB** | 0.22 s | 280.9 s | 486 | 5,051 | +440 MB free |
| 60 min | 525.7 s | 0.146 | 858.9 MB | **859.4 MB** | 0.46 s | 525.1 s | 912 | 9,890 | **+165 MB free** |
| 90 min | 779.0 s | 0.144 | 1,134.3 MB | **1,135.0 MB** | 0.69 s | 778.1 s | 1,341 | 14,735 | **−111 MB over** |

A least-squares fit through the four points is **peak = 308.4 MB + 9.19 MB per audio-minute**
(residual < 0.5 MB at every point). Decode and resample are noise: **0.69 s** to decode 90
minutes at 16 kHz mono, 0.86 s including resample. Wall time is **0.144–0.170× realtime** and
improves slightly with duration (fixed per-call overhead amortises).

**Exact failure observed: none on this VPS.** The 90-minute job completed. The failure on a
4 GB phone is the app's RSS budget: 1,135 MB native + the Android/UI baseline exceeds a
1 GiB budget and the LMKD reclaim path, not a decode error. Diarization is already refused
above 30 min by `MAX_AUDIO_MS` (`DiarizationError::TooLong`), so a 60-minute meeting cannot
get speaker labels at all today.

### 3.1 Decode is not the long-file problem at 16 kHz — but it is at phone rates

A 5-minute 48 kHz stereo WAV measured **354.9 MB** peak, essentially the same as the 16 kHz
mono 5-minute run (354.3 MB): at short durations the transcription state dominates and the
decode buffers are freed before it. That changes with duration. At 48 kHz stereo,
`decode_file` holds 384 KB per audio-second (23 MB/min) and the mono copy adds 11.5 MB/min
during the resample; a 90-minute phone recording would be **~2.1 GB of source buffer +
~1.0 GB of mono** before whisper starts, against 3.7 GiB total and ~2 GiB available. The
16 kHz fixture understates the real decode peak by 6× per audio-second. This is arithmetic
from the measured 5-minute point, not a measured 90-minute 48 kHz run.

## 4. turbo-q5-0

| audio | wall | RTF | VmHWM | segments | words | note |
|---:|---:|---:|---:|---:|---:|---|
| 1 min | 307.7 s | **5.13** | **1,047.2 MB** | 15 | 161 | completed; baseline after model load 561.6 MB |
| 5 min | >1,084 s | >3.61 | ≥1,100 MB | — | — | killed after 18 min 04 s; no transcript |
| 30 min | — | — | — | — | — | not run: 5.13 RTF projects ~2.6 h |
| 60 min | — | — | — | — | — | not run: ~5.1 h |
| 90 min | — | — | — | — | — | not run: ~7.7 h |

The 1-minute transcript is clean (161 words, no loop), so the 5-minute kill was slowness,
not a hang. The process sits at **561.6 MB** after model load, before any audio; the 5-minute
attempt reached 1,100 MB. turbo-q5-0 cannot satisfy a 1 GiB budget at **any** duration and
is a 5× realtime job on this 2-core box. The 30/60/90 rows are deliberately not run: the
gate is already closed at 1 minute, and each row would cost 2.6–7.7 hours.

## 5. Does the existing chunk path hold quality? (60 min, tiny)

`vad::chunk_pcm` with `VadConfig::default()` (10 s min chunk, 14 s max, 700 ms pause, 1.5 s
pre-roll), each chunk transcribed with the same whisper parameters, joined with
`vad::join_decoded` (M7's rule).

| metric | single pass | VAD chunked | delta |
|---|---:|---:|---:|
| peak RSS (VmHWM) | 859.4 MB | **533.0 MB** | −326.4 MB (−38.0 %) |
| wall | 525.7 s | 804.7 s | +279.0 s (+53.1 %) |
| RTF | 0.146 | 0.224 | +0.078 |
| chunks | 1 pass | 286 | — |
| chunk audio decoded | 3,600 s | 3,725.7 s | +125.7 s (pre-roll re-decode) |
| segments | 912 | 794 | −118 |
| words | 9,890 | 9,321 | **−569 (−5.8 %)** |
| normalised WER vs single | — | **21.74 %** | **+21.74 pt** |

The VAD itself costs 20.2 s; the remaining 783.8 s is whisper. The delta is not a timestamp
artefact: chunk output contains boundary hallucinations and duplicated pre-roll words, e.g.
at 0:15 the chunked text emits an extra `summary.` before `"We can, I will read…"`, and at
0:58 it emits `notes. Thank you…` while the single pass has the separate sentence `Friedly
at nine in the small room by the stairs.` At 5:30–6:00 it adds a stray `This there's.` and
duplicates `to follow.`. Whisper without the full-pass
context picks different words (`launch` vs `lunch`, `branded` vs `printed`) and, at 12–14 s
of audio per call, hallucinates at the edges.

**Timestamp drift at 10 positions** (position-anchored: the reference token nearest the
instant, the same normalised token in the candidate within ±5 s; a global alignment is
unusable on this fixture because the 167 s conversation repeats and matches a different
repetition):

| # | target | word | reference start | chunk start | delta |
|---:|---:|---|---:|---:|---:|
| 1 | 3:00 | did | 179,906 ms | 179,557 ms | −349 ms |
| 2 | 9:00 | follow | 539,926 ms | — | unmatched |
| 3 | 15:00 | summary | 899,896 ms | 900,569 ms | +673 ms |
| 4 | 21:00 | the | 1,260,029 ms | 1,261,466 ms | +1,437 ms |
| 5 | 27:00 | for | 1,620,029 ms | 1,620,630 ms | +601 ms |
| 6 | 33:00 | the | 1,980,260 ms | 1,979,728 ms | −532 ms |
| 7 | 39:00 | down | 2,340,260 ms | 2,340,311 ms | +51 ms |
| 8 | 45:00 | can | 2,700,260 ms | 2,705,256 ms | +4,996 ms |
| 9 | 51:00 | thank | 3,060,260 ms | 3,059,952 ms | −308 ms |
| 10 | 57:00 | can | 3,420,260 ms | 3,420,581 ms | +321 ms |

Matched 9/10, median |drift| **532 ms**, max **4,996 ms** (the 45:00 match is at the ±5 s
search edge, i.e. the word moved by about a chunk). The chunk path is not quality-neutral.

## 6. What a disk-backed decode buys

Temporary 16 kHz f32 file; windows read back with `std::fs`. 30-minute windows unless noted.

| variant | audio | window | peak RSS (VmHWM) | vs in-memory | wall | RTF | temp file | WER vs single | text identical |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| in-memory (reference) | 60 min | — | 859.4 MB | — | 525.7 s | 0.146 | — | — | — |
| disk, 30-min windows | 60 min | 30 min | **598.6 MB** | −260.8 MB (−30.3 %) | 531.5 s | 0.148 | 219.7 MiB | **12.68 %** | no |
| disk, 15-min windows | 60 min | 15 min | **491.3 MB** | −368.1 MB (−42.8 %) | 537.6 s | 0.149 | 219.7 MiB | **16.88 %** | no |
| in-memory (reference) | 90 min | — | 1,135.0 MB | — | 779.0 s | 0.144 | — | — | — |
| disk, 30-min windows | 90 min | 30 min | **599.8 MB** | −535.2 MB (−47.1 %) | 784.2 s | 0.145 | 329.6 MiB | **12.12 %** | no |

The RSS win is real and lands exactly where the in-memory path is worst: the window buffer
is 115 MB (30 min) or 57.6 MB (15 min), the mel adds about half that again, and the decode
copy is never materialised. Disk I/O is not the cost — the 60-minute decode + write takes
0.18–0.26 s and the 90-minute one 0.29 s (page cache); wall time is unchanged within ~1 %.

**The transcript is not byte-identical, and the failure is at the cuts, not in the decode.**
With 15-minute windows the first 15 minutes are **text-identical** to the single pass
(0.00 % WER); after the 15:00 cut the windows degrade: 15–30 min **14.09 %**, 30–45 min
**39.13 %**, 45–60 min **15.99 %**. The 30-minute-window 90-minute run shows the same shape:
0–30 min **0.04 %**, 30–60 min **27.25 %**, 60–90 min **9.92 %**. The decoder latches into a repetition loop
after a mid-sentence cut; the 30–60 window emits
`I will read the recording from yesterday and the first 10 minutes.` repeatedly instead of
the conversation. This is a decoder-context failure, not a PCM bug: the same samples at the
start of the file transcribe identically.

Byte-identity check: both `text_identical` and `segment_arrays_identical` are **false** for
every disk run (segment counts 984 vs 912 at 60 min/30-min windows; 1,101 vs 912 at 60
min/15-min windows; 1,415 vs 1,341 at 90 min). A proper windowed design needs silence-aligned
cuts + prompt carry (the M7 join rule alone does not prevent the loop) and its own quality
gate.

## 7. The honest cap

Measured tiny peak, and the 1 GiB arithmetic (fit: 308.4 MB + 9.19 MB/min):

| audio | measured / projected peak | 1 GiB process budget | 4 GB phone (+150 MB app baseline) |
|---:|---:|---|---|
| 30 min | 583.7 MB | fits (440 MB free) | fits (290 MB free) |
| 45 min | 721.7 MB (interpolated) | fits (302 MB free) | **fits (152 MB free) — recommended phone cap** |
| 60 min | **859.4 MB measured** | **fits (165 MB free) — recommended VPS cap** | 1,009 MB — 15 MB free, too tight |
| 75 min | 997.3 MB (projected) | 26 MB free — do not ship | over |
| 90 min | **1,135.0 MB measured** | **111 MB over** | over |

* **4 GB phone: 45 minutes.** 45 min is 722 MB native; adding an assumed 150 MB Android/app
  baseline (not measured on a phone in this gate) leaves ~150 MB of headroom under 1 GiB. 60 min at 859 MB + baseline is over.
* **2-core VPS: 60 minutes.** 859 MB measured, 16 % headroom under 1 GiB; 90 min is 1,135 MB
  and would need a ~1.2 GB (1.1 GiB) budget. If the VPS budget is the machine's 3.7 GiB rather than
  1 GiB, 90 min fits and completes in 13 min — but the product promise is the phone, so the
  ship cap is the phone's.
* **Chunking is not the answer.** It buys 326 MB at 60 min but costs 21.74 pt WER and 53 %
  wall. It cannot be sold as the long-meeting path in its current form.
* **Disk streaming is the only *measured* 90-minute mechanism that fits 1 GiB** (600 MB), but as
  measured it is not quality-safe. Fixing the cuts is the next slice; until it passes a
  chunk-vs-single gate at 60/90 min, 90-minute meetings are a **STOP**.
* **turbo-q5-0 has no cap under 1 GiB** (1,047 MB at 1 minute) and is 5.13× realtime.

## 8. Limitations

* The fixture is a 167 s conversation repeated; it is real multi-speaker speech but the
  repetition makes whisper's decoder repetition-sensitive. That is a fair stress for the
  cut/window mechanisms (it exposes them), but absolute WER on a non-repeating meeting will
  differ. The tiny single-pass transcript is stable across repeats and across the 60/90
  runs (first divergence at word 9,817 of 9,890, i.e. at the 60-minute boundary).
* The gate measures 16 kHz mono files, as the workorder specifies. Phone recordings are
  usually 44.1/48 kHz stereo; the decode buffers are 6× larger per audio-second (see §3.1),
  so the in-memory cap on real phone files is lower, not higher.
* One process per point; no warm-model reuse. Model load is outside the wall clock but its
  RSS is inside the peak (tiny 93 MB, turbo 562 MB).
* The WER is the repo's own normaliser (lowercase alphanumeric tokens, `'` and `-` kept)
  over the concatenated transcript; the distance is an exact word-level Levenshtein. The
  disk/chunk drift uses position-anchored same-word matching because global alignment is
  ambiguous on repeated audio; it is stated at the ±5 s search bound.

## 9. Reproduce

```bash
# fixture
M25A_BASE=/tmp/m16h-fixture/long_175s.wav \
  python3 tools/m25a/build_fixtures.py . /tmp/m25a-fixtures 5 30 60 90

# one point (tiny, in-memory)
M25A_MODE=single M25A_INPUT=/tmp/m25a-fixtures/m25a-60m.wav \
M25A_MODEL=$HOME/.local/share/suramak/models/ggml-tiny.bin \
M25A_OUT=/tmp/m25a-60m-single.json M25A_LANG=en \
LD_LIBRARY_PATH=target/release:target/release/deps \
cargo test -p suramak-asr --release --features vad-chunking --test m25a_gate \
  -- --ignored --nocapture

# chunk / disk
M25A_MODE=chunk M25A_VAD_MODEL=/tmp/silero_vad.onnx ...
M25A_MODE=disk M25A_WINDOW_MS=1800000 ...
```

The report's raw JSON, timeline and analysis files are under
`/home/ubuntu/.hermes/cache/scratch/m25a-results/` (scratch; not committed).

---

## Parent verification (added by the orchestrator)

Read against the code before adopting the decision:

- **`decode_file` really does hold the whole file**: `AudioBuffer.samples: Vec<f32>`
  (`crates/suramak-core/src/decode.rs`), and `prepare_samples` resamples into a second
  full buffer. The 64 KB/audio-second figure is arithmetic, not an estimate.
- **`MAX_AUDIO_MS = 30 min` is real** (`crates/suramak-asr/src/diarization.rs:56`,
  enforced in `execute()`), so the report's note that a 60-minute meeting gets **no
  speaker labels today** is a product gap, not a caveat.
- **turbo-q5-0 in this box's memory budget**: the pinned model file is ~574 MB, so a
  1,047 MB peak at one minute is consistent with model + state, and the 5.13 RTF is this
  2-core VPS (Metal on the owner's Mac is a different machine).
- **Fixture caveat, recorded deliberately:** the long fixtures repeat one 167 s
  multi-speaker clip with 0.8 s of silence. Whisper is known to enter repetition loops on
  repeated audio, so the **quality** verdicts (VAD chunk +21.74 pt, windowed +12–17 pt)
  are "not proven safe on this fixture", not "proven broken in general". The **memory**
  verdicts are fixture-independent and stand as measured.
- **What is adopted:** the cap (45 min phone / 60 min VPS, tiny, single pass) and the
  STOP on 90 min / turbo / both chunking mechanisms *as quality-safe paths*. Any future
  long-meeting design must pass a gate on **diverse** 60–90-minute material, not a
  repeated clip.
