# M16.4-F — Silero VAD chunking: feasibility report

**Decision: GO — but only into a gated integration slice, not straight into capture.**
Chunk-only transcription (no final full pass) is **not** approved by this probe: the
join-quality gate below must pass first.

- **The VAD is cheap enough to be a non-issue.** On `VM-0-5-ubuntu` (2 vCPU EPYC
  7K62), Silero v5 costs **4.17 ms of CPU per audio-second** in 32 ms blocks
  (p50 121–146 µs per block across 9 runs; cost 3.83–4.83 ms) — **0.42 % of one
  core**, 240× realtime, and **0.53 %** of the CPU a whisper `base` full pass spends
  on the same audio. It fits
  the 4 GB-phone budget with ~100× headroom against the transcription's own CPU (§6).
- **The binding is already in the graph.** `sherpa-rs 0.6.8` / `sherpa-onnx 1.12.9`
  (Apache-2.0) is the M10A Parakeet dependency and exposes `SileroVad` with the whole
  segment state machine. `ort` 2.0.0-rc.15 would be a *second* ONNX binding, has no
  stable release, and is **larger** on every platform measured (§2).
- **The mechanism is deterministic and pause-aligned.** 3/3 runs produced identical
  segment lists; the feed granularity must be **exactly 512 samples** (1 s blocks merge segments and shift
  starts by up to 960 ms); room tone at −55 dBFS shifts boundaries by ≤ 32 ms; cuts
  land **116 ms after** the last voiced frame and **906–3916 ms before** the next.
- **The cost that matters is chunk cadence, not VAD.** 10–14 s chunks cost
  **1.93×** the CPU of one full pass even though they decode 13 s less audio
  (≈2.5× like-for-like; whisper pads every call to 30 s); 30 s chunks cost **1.15×**. The production policy must target **20–30 s chunks**, cut only at
  pauses ≥ 500–700 ms.
- **The risk is text at joins, and it is unproven.** Whisper on a 30 s chunk of the
  mixed EN/ID fixture emitted bracketed hallucination tags and dropped a word; 10–14 s
  chunks were clean. M7's skip-and-merge is safe only because its provisional list is
  discarded on Stop — a chunk-only pipeline has no such backstop (§5.3).

### Production slices (in order, each independently stoppable)

| # | slice | gate to pass before the next |
|---|---|---|
| 1 | **Join-quality harness** (pure, no capture, no UI): replay the three fixtures through VAD chunker → whisper, compare against the full-pass transcript on reviewed EN+ID text | **zero dropped phrases, ≤ 1 duplicated phrase, WER delta ≤ 1.0 pt** vs the full pass, across clean + room-tone variants; otherwise stop chunk-only and keep M7's cadence |
| 2 | **Queue compose**: `Job.chunk { session, offset_ms }` + session-owned assembly + save-once, reusing M16.1's 1-slot invariant and `Job.samples` | mixed chunk counts, cancellation mid-session, one saved transcript per recording, bounded pending queue (≤ 2 chunks) |
| 3 | **Capture integration** behind `vad-chunking` (default OFF): VAD on the capture consumer thread, fed exactly 512 samples/call from a ring, never in the audio callback | device removal, long capture, stop mid-chunk, CPU ≤ 1 % of the capture process, no unbounded buffers |
| 4 | **Model manager**: pin `silero_vad.onnx` (2 327 524 B, sha256 `2623a295…`) + MIT notice in the licenses surface | download corruption/resume, missing model → feature degrades to M7 cadence, no silent engine change |
| 5 | **Settings + measurement**: chunk target 20–30 s, `min_speech` 0.10 s, `min_silence` 0.5 s, threshold 0.5; report chunk count/latency in the diagnostics | named-hardware latency + CPU table, including one real 4 GB Android device |

No production capture-path code, no new product dependency, and no CI change were made
in this pass. The probe is two throwaway crates under gitignored `build/` and will not
be merged (§8).

**If slice 1 fails its gate, the decision becomes STOP for chunk-only capture**: keep
M7's cadence for provisional partials, keep the final pass as the only transcript
authority, and re-visit only when a streaming-capable engine (Parakeet streaming or a
whisper.cpp streaming build) is in the graph — VAD-driven boundaries cannot fix a
decoder that loses words at a cut.

---

## 1. Candidate, license, and pins

| field | value |
|---|---|
| repository | `https://github.com/snakers4/silero-vad` |
| tag | `v5.1.2` (tag ref → commit `6478567951ae5c9979ad7b234185b5515f4be7a1`) |
| file | `src/silero_vad/data/silero_vad.onnx` |
| bytes | **2 327 524** |
| sha256 | **`2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f`** |
| license | **MIT**, 20 lines, `LICENSE` sha256 `2e63e9a38b6e8fc0c7bc37ce174caca1862870856c6daf5697cfb785e925520b` ("MIT License / Copyright (c) 2020-present Silero Team") |
| model signature | 3 inputs (`input`, `state`, `sr`) / 2 outputs → the v5 path in sherpa-onnx (`silero-vad-model.cc` selects `RunV5` on 3/2 and requires `window_size == 512` at 16 kHz) |

Provenance notes, checked rather than assumed:

- `v5.1.1` resolves to **byte-identical** model bytes (same sha256); `master` does not
  (`1a153a22…` prefix) — pin the tag, never `master`.
- The file sherpa-onnx publishes as `asr-models/silero_vad.onnx` is **not** this model:
  643 854 B, sha256 `9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6`,
  a 4-input v4-class k2-fsa export (`x,h,c` → `prob,new_h,new_c`). It runs (219 µs/block vs 121 µs) but it is not the pinned
  candidate and must not be substituted.
- The repository README's license badge carries stale alt text ("CC BY-NC 4.0") while
  the badge label, its link, and the `LICENSE` file are MIT; the README body says
  "Published under permissive license (MIT)". The LICENSE file governs; re-check at
  pin time rather than trusting the badge.

## 2. Rust/ONNX binding decision: sherpa-rs, not ort

| | **sherpa-rs 0.6.8 → sherpa-onnx 1.12.9** | `ort` 2.0.0-rc.15 |
|---|---|---|
| status in repo | already a dependency (`parakeet` feature, M10A), pinned and documented in `docs/ENGINES.md` | new dependency |
| crate license | MIT (wrapper); **Apache-2.0** (sherpa-onnx, bundled `LICENSE`, 202 lines); **MIT** (ONNX Runtime 1.17.1, bundled `.so`) | MIT OR Apache-2.0; ONNX Runtime 1.22.0 also MIT |
| release maturity | 0.6.8, used in-tree since M10A | **release candidates only** (rc.15, 2026-10-19); no stable version |
| VAD code | `SileroVad` + `SileroVadConfig` already implement windowing, LSTM state, `min_speech`/`min_silence`/`max_speech`/threshold and the segment queue | would be written here: 512-sample windowing, 64-sample context, `state` `[2,1,128]`, `sr` tensor, segmentation state machine (~400–600 LOC + its own tests) |
| Linux x64 payload | **20 825 192 B (19.86 MiB)**: `libsherpa-onnx-c-api.so` 5 216 616 + `libonnxruntime.so` 15 534 400 + `libsherpa-onnx-cxx-api.so` 74 176 | **21 042 416 B (20.07 MiB)**: `libonnxruntime.so.1.22.0` alone |
| macOS universal2 payload | **59 465 435 B (56.71 MiB)**: `libsherpa-onnx-c-api.dylib` 8 907 832 + `libonnxruntime.1.17.1.dylib` 50 396 248 + cxx 161 328 | **70 774 760 B (67.50 MiB)**: `libonnxruntime.1.22.0.dylib` alone |
| Android arm64 payload | **20 521 528 B (19.57 MiB)**: prebuilt `libsherpa-onnx-c-api.so` 4 485 608 + `libonnxruntime.so` 16 035 920 (from `sherpa-onnx-v1.12.9-android.tar.bz2`) | ONNX Runtime AAR/so must be built or extracted for each ABI; not covered by the crate's download path |
| second runtime risk | none — one ORT in the process | a second ORT (1.22) if Parakeet ever ships → two copies (~36 MB) and two symbol families in one app |

The size assumption usually made in `ort`'s favour does not hold on this evidence:
the current ORT release is larger than sherpa's whole native payload on both desktop
platforms measured. The remaining argument for `ort` — no C++ wrapper, a Rust-native
API — is real but does not buy a smaller, simpler, or better-licensed artifact here.

**Measured API caveat (it shapes slice 3).** `sherpa-onnx`'s VAD is a *block-coupled*
API: every `AcceptWaveform` call ORs all windows inside the block before deciding
"speech", so a pause shorter than the block is invisible to the segmentation. Measured
on the same 60 s fixture: 512-sample blocks → 13 segments; 1 600 → 13 but starts shift
(median 32 ms, max 96 ms); 4 800 → median 128 ms, max 288 ms; 16 000 (1 s) → **8
segments**, starts shifted by up to 960 ms, pauses swallowed. Production must feed
**exactly 512 samples per call**, independent of the device callback size.

The safe wrapper also takes `Vec<f32>` (`accept_waveform(chunk.to_vec())`), i.e. one
allocation + copy per 32 ms. At 121 µs per call the copy is not the bottleneck, but
slice 3 should call `sherpa-rs/sys`'s
`SherpaOnnxVoiceActivityDetectorAcceptWaveform` with a borrowed slice — `sherpa-rs/sys`
is already enabled by the existing `parakeet` feature.

## 3. Probe: machine, fixtures, method

**Machine** `VM-0-5-ubuntu`: AMD EPYC 7K62, **2 vCPU**, 3 GB RAM, Linux
7.0.0-14-generic, rustc 1.99.0; load average **1.1–1.9** during runs (shared VPS, a
second build running). All CPU numbers are process CPU (`/proc/self/stat`
utime+stime), so co-tenant scheduling inflates wall time but not the CPU column. Run
date 2026-11-10/11 UTC.

**Probe** (throwaway, `build/m16df-probe`, release, sherpa-rs 0.6.8 with the same
prebuilt sherpa-onnx 1.12.9 the product feature links): reads 16 kHz mono PCM16,
constructs `SileroVad` once, feeds fixed-size blocks, drains segments after every
call, `flush()` at the end, and prints one JSON object per run with per-call
percentiles, process CPU, RSS, and every segment (`start_ms`, `end_ms`,
`emitted_at_ms`). `num_threads = 1`, `window_size = 512`, `threshold = 0.50`,
`min_silence = 0.300 s`, `min_speech = 0.250 s`, `max_speech = 30 s`,
`buffer = 60 s` unless a row says otherwise.

**Fixtures**

| fixture | bytes | sha256 | provenance |
|---|---|---|---|
| `fixtures/audio/en.wav` | 288 078 | `318bcc707bcc0a60f5238383f8af90e66b5e4d2307bd44b16e9ca895385b38a6` (committed) | 9.000 s, 16 kHz mono PCM16, repo TTS sample |
| `fixtures/audio/id.wav` | 381 006 | `f630e8a473df1883bd63e31ec6cbcc6f1c75524decbd7a19ee034b59510cfbd1` (committed) | 11.904 s, same |
| `m16df-60s-clean.wav` | 1 920 078 | `b443105ded6bbfe800746e570f7b0cc769eb0958e8d044fd04e1126663e637d4` | generated once with system ffmpeg 8.0.1 from the two committed files: 9 cues (one deliberately 0.300 s), 9 pauses 0.5–3.5 s, 60.000 s exactly, all `atrim` in samples |
| `m16df-60s-room.wav` | 1 920 078 | `e05e1de5364e4836ed2908fca59e657f509e46fd1c8ce5659fd90e562d32370b` | the same timeline + pink noise at −55 dBFS (`amplitude=0.002:seed=20261110`, so the bytes are reproducible), a plausible microphone floor |

The 60 s fixtures are **not committed** (this pass lands docs + reports only). They
regenerate deterministically from the two committed fixtures with the command in §8;
the cue table is:

| cue | source | source start (ms) | length (ms) | session start (ms) |
|---|---|---|---|---|
| 1 | en | 0 | 4000 | 1000 |
| 2 | id | 0 | 4500 | 7000 |
| 3 | en | 4000 | 5000 | 15000 |
| 4 | id | 4500 | 7404 | 22500 |
| 5 | en | 500 | 2500 | 33000 |
| 6 | id | 8000 | 3904 | 38000 |
| 7 | id | 400 | **300** | 45000 |
| 8 | en | 0 | 6000 | 47500 |
| 9 | id | 2000 | 3500 | 56000 |

Layout verified independently by a 32 ms RMS gate: onsets at 1.184, 7.264, 14.976,
22.496, 32.992, 37.984, 44.992, 47.680, 56.320 s — the cue starts plus 0.1–0.3 s of
soft onset.

## 4. Measurements

### 4.1 Cost per audio-second

| fixture | blocks | calls | p50 per call | p95 | p99 | max | CPU for audio | per audio-s | × realtime (CPU) |
|---|---|---|---|---|---|---|---|---|---|
| clean 60 s | 512 | 1875 | **122 µs** | 168 µs | 201 µs | 240 µs | 250 ms | **4.17 ms** | 240× |
| room 60 s | 512 | 1875 | 120 µs | 145 µs | 165 µs | 238 µs | 240 ms | **4.00 ms** | 250× |
| en 9 s | 512 | 282 | 125 µs | 146 µs | 158 µs | 245 µs | 30 ms | 3.33 ms | 300× |
| id 11.9 s | 512 | 372 | 123 µs | 144 µs | 153 µs | 253 µs | 50 ms | 4.20 ms | 238× |
| clean 60 s | 1600 | 600 | 371 µs | 526 µs | 592 µs | 781 µs | 240 ms | 4.00 ms | 250× |
| clean 60 s | 4800 | 200 | 1 121 µs | 1 279 µs | 1 401 µs | 1 404 µs | 230 ms | 3.83 ms | 261× |
| clean 60 s | 16 000 | 60 | 3 792 µs | 4 495 µs | 5 086 µs | 6 139 µs | 230 ms | 3.83 ms | 261× |

Read: the cost is **per 512-sample window**, not per call — 32 × 512-sample calls cost
the same as one 16 000-sample call. Run-to-run spread is real (ORT's session pool and
co-tenant scheduling): **9 fresh 60 s runs gave 230–290 ms, median 250 ms = 4.17 ms
per audio-second**, with one 350 ms outlier while load average was ~1. Process CPU is
read in 10 ms ticks, so each 60 s point carries ±0.17 ms/s of quantisation on top.
Cold-start outliers exist (first run: 1.70 ms max; occasional 0.97–2.26 ms maxima
under co-tenant load) and are irrelevant at 32 ms cadence. Model construction
(session + model read) measured **108–113 ms** warm, 128 ms cold.

### 4.2 Stability

**Run-to-run:** 3/3 runs of the clean fixture produced **identical** segment lists to
the millisecond (13 segments); same for room tone. Determinism is exact, not
statistical.

**Block-size sensitivity** (the same audio, only the `accept_waveform` size changed):

| block | segments | start shift vs 512 (median / max) | verdict |
|---|---|---|---|
| 512 (32 ms) | 13 | — | native resolution |
| 1 600 (100 ms) | 13 | 32 ms / 96 ms | ends drift +64…+300 ms |
| 4 800 (300 ms) | 13 | 128 ms / 288 ms | boundaries visibly quantised |
| 16 000 (1 s) | **8** | 336 ms / **960 ms** | pauses swallowed, segments merged |

**Threshold** (vs 0.50 baseline): 0.40 and 0.60 keep every boundary within 128/96 ms
(median 0); 0.30 adds up to 320 ms of pre-roll on quiet onsets; **0.70 clips quiet
speech onsets by up to 448 ms**. 0.4–0.6 is the stable band; 0.5 is the pin.

**Short utterances:** at `min_speech = 0.25 s` the deliberately 0.300 s cue is
**dropped entirely** (1 of 9 cues, 0.8 % of the fixture's speech time — a real
"dropped text" mode for interjections). At `min_speech = 0.10 s` it is recovered at
45 140–45 396 ms, at the cost of two extra fragments (15 vs 13 segments). Slice 5
should pin 0.10 s and merge sub-second fragments into their neighbour chunk.

**Pause length:** `min_silence` 0.5/0.7 s merge sentence pauses (13 → 11 segments) with
boundaries within 8–32 ms of the 0.3 s run. A longer `min_silence` is therefore a safe
chunk-count lever.

**Noise:** adding −55 dBFS pink room tone changes 4 boundary values (two segment ends, one end + one start pair), all by **≤ 32 ms**;
segment count and every start are otherwise identical. No false speech on the noise
floor (the room file's 22 s of non-speech produced no segment).

**Against an independent energy gate** (−42 dBFS, 100 ms hangover, 512-sample frames):
precision **0.869**, recall **0.931**, IoU **0.817**; over 11 gate runs ≥ 1 s the VAD
start is median **−34 ms** and the end median **+148 ms** (range +84…+244 ms) — i.e. it
consistently keeps ~150 ms of decay after the energy gate's end. Start offsets are
within ±100 ms for 5/11 runs, with a tail to −1.44 s where the VAD includes quiet
speech below the gate's threshold. For joins, the useful property is the consistent
+150 ms tail: a cut at the VAD end sits *inside* the pause, never on the last phoneme.

**Emission latency:** every segment is emitted **332 ms** after its end (= 300 ms
`min_silence` + one 32 ms block), constant across the fixture. That is the algorithmic
floor for "pause → chunk available"; it is not CPU latency.

### 4.3 Memory

| point | RSS |
|---|---|
| process before model load | 13.1 MB |
| after `SileroVad::new` (v5) | **46.4 MB** (+33.3 MB, includes the mapped 20.8 MB of native libs and ORT arenas) |
| after a 60 s run | 47.3 MB (+0.9 MB) |
| v4-class sherpa-hosted model, same probe | 28.2 MB after load |

The VAD's own ring buffer is `buffer_size_in_seconds × 16 000 × 4 B` = **3.84 MB** at
60 s. 10 s and 30 s buffers produced identical segments on this fixture (no cue exceeds
7.4 s), so the buffer only has to exceed the longest expected utterance; slice 3 should
set it to `max_speech` + 1 s.

### 4.4 Chunk formation

Raw VAD output at 300 ms pauses is **13 chunks of 0.50–3.80 s** — too short: whisper
pads every call to 30 s, so this multiplies cost (§4.5). Applying the production
policy — *close at a pause ≥ 700 ms only when the chunk is already ≥ 10 s, hard cap
30 s* — gives **4 chunks: 10.45, 10.77, 13.97, 11.86 s**. Cut margins: **116 ms after**
the last voiced frame, **906–3 916 ms before** the next. With a 5 s minimum the same
fixture yields 6 chunks of 3.3–9.9 s, i.e. the policy is a dial, not a cliff.

### 4.5 Transcription share (same machine, same PCM, `base`, 2 threads, `auto`)

| run | audio | CPU | per audio-s | × realtime |
|---|---|---|---|---|
| `base`, one full pass | 60.0 s | 46.88 s | 781 ms | 1.28× |
| `base`, 2 × 30 s chunks | 60.0 s | 54.04 s | 901 ms | 1.11× |
| `base`, 4 chunks of 10.5–14.0 s | 47.1 s of speech | 90.60 s | 1 925 ms | 0.52× |
| `base`, one 5.5 s window (M7's shape) | 5.5 s | 20.26 s | 3 684 ms | 0.27× |
| `tiny`, one full pass | 60.0 s | 20.35 s | 339 ms | 2.95× |

- **VAD as a share of transcription:** 4.17 ms ÷ 781 ms = **0.53 %** of a `base` full
  pass; 1.23 % of `tiny`; 0.46 % of a 30 s-chunked pass; 0.22 % of a 10 s-chunked pass.
- **Chunk cadence is the cost driver:** per-call cost is ~20–32 s of CPU regardless of
  whether the chunk is 5.5 s or 30 s (whisper encodes a padded 30 s window). The four
  chunks cover only the fixture's **47.1 s of speech** (the pauses between them are
  never decoded) and still cost **1.93×** the CPU of the single 60 s pass — on a
  like-for-like speech basis (781 ms × 47.06 s = 36.8 s) the overhead is **≈2.5×**.
  30 s chunks, which cover the whole file, cost **1.15×**; M7's 5.5 s windows at a 4 s
  step cost ≈ **5× realtime** (one call per 4 s of audio).
- Practical consequence: **chunk at 20–30 s, not 10 s**, unless the user explicitly
  trades CPU for partial latency. This also cuts the number of joins ~3× versus 10 s
  chunks, which is the risk surface that matters (§5).

## 5. Integration argument — M16.1 queue + whisper

### 5.1 What already exists

- `worker::Job` already carries `samples: Option<Vec<f32>>` (M16.1's decoded-ahead
  buffer), and `pipeline::prepare_samples` **skips decode entirely** when it is set.
  A chunk can therefore enter the existing pipeline with zero new decode work.
- `JobQueue` enforces exactly one inference slot and ≤ 2 decodes ahead (≤ 96 MiB);
  `QueueDriver` is explicitly documented as the template for capture slices.
- The pipeline's `Transcript` is built by `transcript_for(input, …)`, which needs a
  path only for the title/source — a chunk job can pass the *session* WAV path and get
  correct provenance for free.
- Cancellation already rides `CancelToken` through the job and the engine, so a
  stopped recording can abort the in-flight chunk without new machinery.

### 5.2 What is missing (slice 2, not this pass)

1. **Item identity is a file.** `JobQueue::enqueue(PathBuf)` and one
   `SaveTranscript` per `Finished` mean a 4-chunk session would create four library
   entries. The queue needs a session-scoped item whose `Finished` events are
   *composed*, and a save-once step at session end.
2. **Chunk offset/provenance.** `Transcript` timestamps come from whisper relative to
   the chunk; the session assembly must add the chunk's session offset and drop the
   overlap context (rule below). A `Job.chunk { session_id, offset_ms, core_start_ms }`
   field is the smallest honest carrier — `samples` already carries the PCM.
3. **Bounded pending queue.** The decoded-ahead pool bounds *file decodes*; a live
   session needs its own bound (≤ 2 pending chunks × ≤ 30 s × 64 KiB/s = ≤ 3.8 MB) and
   a policy for what happens when transcription falls behind — the same "skip and
   merge" question M7 answered, but now the merge must be text-exact, not provisional.

Suggested flow: capture consumer thread → 512-sample ring feed → VAD → chunk closed at
pause (policy §4.4) → `Job { samples: chunk±pad, chunk: Some(span) }` → existing single
inference slot → segments offset-shifted, overlap dropped → session transcript grows →
one `SaveTranscript` at Stop. The audio callback only pushes to the existing SPSC ring
(M14B2); it never touches the VAD.

**Join rule** (the M7 frontier rule, re-used with VAD-known boundaries):

```
for each decoded segment s in chunk k (timestamps shifted by chunk.offset_ms):
    if s.end_ms <= chunk.core_start_ms + MERGE_TOLERANCE_MS: drop   # overlap re-decode
    else: append with idx = session.segments.len()
```

with `MERGE_TOLERANCE_MS = 50` exactly as `live.rs` defines it (whisper timestamps are
centiseconds). The difference from M7 is *what the tolerance protects*: M7 drops a
re-decoded window head; here it drops the padded lead-in of the next chunk, and the cut
is known to be 116 ms past the last phoneme and ≥ 906 ms before the next (§4.4), so the
50 ms tolerance sits well inside silence.

### 5.3 Honest comparison with M7's skip-and-merge

| | M7 live (shipped) | M16.4 chunked capture (proposed) |
|---|---|---|
| boundary source | fixed 4 s cadence, 1.5 s overlap | VAD pause, 300–700 ms `min_silence`, padded ±0.3–0.5 s |
| merge rule | drop segments ending ≤ covered + 50 ms, append-only | same rule, applied per chunk |
| safety net | **the final Stop pass replaces the entire list byte-for-byte** (`pipeline.rs`/`whisper.rs`/`worker.rs` zero-diff in M7) | **none if chunk-only**; the composed list *is* the artifact |
| observed failure mode | a phrase across a boundary can appear twice *in the provisional list only* | the same duplication would be **permanent** |
| CPU per audio-second | 5.07 s (5.5 s window every 4 s, §4.5) | 0.90 s at 30 s chunks, 1.93 s at 10 s |
| dropped-text risk | none (windows are contiguous) | real: a dropped/short cue never reaches whisper (measured at `min_speech = 0.25`, §4.2) |

That table is the reason the decision is **GO to slice 1, not GO to capture**. M7's
merge is correct *because* of the final pass; a chunk-only design inherits the merge
rule without its safety net. Two acceptable outcomes:

- **Preferred:** VAD-chosen chunks *replace M7's fixed cadence for partials*, and the
  existing final pass remains the authority (M7's hard rule untouched). This cuts
  partial-decode CPU ~4–6× (one call per 20–30 s instead of per 4 s) and improves
  boundaries, with **zero** new risk to the saved transcript.
- **Only with slice 1 green:** chunk-only for very long recordings (where a final full
  pass is the thing being avoided). Then the join gate is the ship gate.

Text evidence from this probe, stated plainly: `base` on the 30–60 s chunk emitted
`[Lirik brown fox jump over the lazy dog] [Trimakasi sudah mencoba] …` — bracketed
hallucination tags and a dropped "quick" — while the 10–14 s chunks produced clean
sentences and the single full pass produced none of those tags. One fixture, one model,
one run: this is a warning, not a measurement of join WER, and it is exactly what slice
1 must quantify before any chunk-only claim.

## 6. The 4 GB-phone CPU budget

`device.rs` makes `base` the default on Low/Mid phones (4–6 GB) and allows only `tiny`
below the 4 GB floor; a 4 GB device therefore transcribes with `base` or `tiny`.

| work | CPU per audio-second | share of one core | relative to VAD |
|---|---|---|---|
| Silero VAD v5 (measured) | **4.17 ms** | 0.42 % | 1× |
| whisper `tiny` full pass (measured) | 339 ms | 34 % | 81× |
| whisper `base` full pass (measured) | 781 ms | 78 % | 187× |
| whisper `base`, 30 s chunks | 901 ms | 90 % | 216× |
| whisper `base`, M7 5.5 s windows | 3 684 ms | 368 % | 883× |

**Statement.** The VAD costs **0.53 % of a `base` transcription's CPU** and **1.23 % of
`tiny`'s** for the same audio. Even if a 4 GB phone's small cores run the VAD 3–5×
slower per core than this EPYC vCPU (a pessimistic bound for a 2.3 MB LSTM against
whisper's GEMM workload), the VAD stays **at or under 2.1 % of one core** and **under
2.7 % of the transcription's own CPU** — below the capture pipeline's existing resample/mix cost
and far below the 1-slot inference. The budget question for a 4 GB phone is **chunk
cadence, not VAD**: 10 s chunks roughly double transcription CPU versus 30 s chunks
(§4.5), which is the number that decides whether a long recording keeps up.

No phone was measured in this pass (no Android work allowed); the phone claim is a
ratio plus a scaling bound, and the first Android slice (5) must replace it with a
named-device table.

## 7. What this probe does not prove

- **Join quality.** No WER/diff harness was run; §5.3's hallucination is an anecdote.
  This is the gate on slice 1 and the only thing that can turn GO into STOP.
- **Phone behaviour.** No 4 GB device, no Android capture path, no thermal throttling.
- **Real rooms.** The noise variant is stationary pink noise at −55 dBFS; babble,
  keyboard transients and HVAC are not covered.
- **Long uninterrupted speech.** The longest cue is 7.4 s, so `max_speech = 30 s` and a
  `buffer = 10 s` were never stressed; slice 3 must test a >30 s monologue (the
  forced-split path in `voice-activity-detector.cc`).
- **macOS/Windows runtime.** Library sizes were measured from the pinned artifacts;
  only Linux was executed.
- **`ort` CPU parity.** Not benchmarked (same ORT kernels, different version); the
  decision does not rest on a claimed CPU delta.

## 8. Reproduction

Probe sources (throwaway, **not merged**): `build/m16df-probe/` (VAD) and
`build/m16df-whisper/` (transcription baseline), both under the gitignored `build/`
tree, each its own workspace with its own `Cargo.lock`; delete with
`rm -rf build/m16df-probe build/m16df-whisper`. Models and fixtures live outside the
repo (`/tmp`, `~/.cache/m16df-models`). No workspace file, `Cargo.toml`, `Cargo.lock`
or CI file was modified.

Model fetch:

```sh
curl -sSL -o /tmp/silero_vad_v5.1.2.onnx \
  https://raw.githubusercontent.com/snakers4/silero-vad/v5.1.2/src/silero_vad/data/silero_vad.onnx
sha256sum /tmp/silero_vad_v5.1.2.onnx
# 2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f
```

Fixture generation (from the repository root; the exact command that produced the
pinned bytes above):

```sh
F=fixtures/audio; OUT=/tmp/m16df; mkdir -p "$OUT"
ffmpeg -y -hide_banner -loglevel error -i "$F/en.wav" -i "$F/id.wav" \
  -f lavfi -t 60 -i "anullsrc=r=16000:cl=mono" -filter_complex "\
[2:a]asplit=10[b0][b1][b2][b3][b4][b5][b6][b7][b8][lead];\
[0:a]atrim=start_sample=0:end_sample=64000,asetpts=PTS-STARTPTS[c0];\
[1:a]atrim=start_sample=0:end_sample=72000,asetpts=PTS-STARTPTS[c1];\
[0:a]atrim=start_sample=64000:end_sample=144000,asetpts=PTS-STARTPTS[c2];\
[1:a]atrim=start_sample=72000:end_sample=190464,asetpts=PTS-STARTPTS[c3];\
[0:a]atrim=start_sample=8000:end_sample=48000,asetpts=PTS-STARTPTS[c4];\
[1:a]atrim=start_sample=128000:end_sample=190464,asetpts=PTS-STARTPTS[c5];\
[1:a]atrim=start_sample=6400:end_sample=11200,asetpts=PTS-STARTPTS[c6];\
[0:a]atrim=start_sample=0:end_sample=96000,asetpts=PTS-STARTPTS[c7];\
[1:a]atrim=start_sample=32000:end_sample=88000,asetpts=PTS-STARTPTS[c8];\
[b0]atrim=start_sample=0:end_sample=32000,asetpts=PTS-STARTPTS[g0];\
[b1]atrim=start_sample=0:end_sample=56000,asetpts=PTS-STARTPTS[g1];\
[b2]atrim=start_sample=0:end_sample=40000,asetpts=PTS-STARTPTS[g2];\
[b3]atrim=start_sample=0:end_sample=49536,asetpts=PTS-STARTPTS[g3];\
[b4]atrim=start_sample=0:end_sample=40000,asetpts=PTS-STARTPTS[g4];\
[b5]atrim=start_sample=0:end_sample=49536,asetpts=PTS-STARTPTS[g5];\
[b6]atrim=start_sample=0:end_sample=35200,asetpts=PTS-STARTPTS[g6];\
[b7]atrim=start_sample=0:end_sample=40000,asetpts=PTS-STARTPTS[g7];\
[b8]atrim=start_sample=0:end_sample=8000,asetpts=PTS-STARTPTS[g8];\
[lead]atrim=start_sample=0:end_sample=16000,asetpts=PTS-STARTPTS[g9];\
[g9][c0][g0][c1][g1][c2][g2][c3][g3][c4][g4][c5][g5][c6][g6][c7][g7][c8][g8]\
concat=n=19:v=0:a=1[out]" -map "[out]" -ar 16000 -ac 1 -c:a pcm_s16le \
  -map_metadata -1 "$OUT/m16df-60s-clean.wav"

ffmpeg -y -hide_banner -loglevel error -i "$OUT/m16df-60s-clean.wav" \
  -f lavfi -t 60 -i "anoisesrc=color=pink:amplitude=0.002:sample_rate=16000:seed=20261110" \
  -filter_complex "[0:a][1:a]amix=inputs=2:normalize=0:duration=first[a]" \
  -map "[a]" -ar 16000 -ac 1 -c:a pcm_s16le -map_metadata -1 "$OUT/m16df-60s-room.wav"
```

Probe invocation:

```sh
cd build/m16df-probe && cargo build --release
LD_LIBRARY_PATH="$PWD/target/release" ./target/release/m16df-probe \
  --model /tmp/silero_vad_v5.1.2.onnx --wav /tmp/m16df/m16df-60s-clean.wav \
  --chunk 512 --threshold 0.5 --min-silence 0.3 --min-speech 0.25 --repeat 3
```

Whisper baseline: `build/m16df-whisper --model … --wav … --threads 2 --lang auto`,
same `whisper-rs 0.16` the product uses (`FullParams`: greedy, `n_threads =
default_threads() = 2`, token timestamps on, printing off), `ggml-base.bin`
sha256 `60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe` (the
`MODEL-PINS.md` value) and the repo's `ggml-tiny.bin`.

---

## Verification (this pass)

The shared working tree was **not** used as the test oracle: another workorder
(M13G1) holds `crates/suramak-ui` mid-refactor, so a clean detached worktree at
`HEAD f714176` was tested instead (`git worktree add --detach /home/ubuntu/m16df-baseline HEAD`,
`CARGO_PROFILE_DEV_DEBUG=0`). That is the exact tree this report describes, and it
contains no edit from this pass.

| check | command | result |
|---|---|---|
| formatting | `cargo fmt --all --check` | **exit 0** |
| lints | `cargo clippy --workspace --all-targets -- -D warnings` | **exit 0**, `Finished dev profile in 1m 56s` |
| tests | `cargo test --workspace --no-fail-fast` | **987 passed, 0 failed, 10 ignored** (32 test binaries/doctest targets) |
| tree | `git status --porcelain` | this pass adds only `docs/reports/M16d-vad-feasibility.md`; the probe lives under gitignored `build/`. The other entries in the shared tree (`crates/suramak-ui`, `Cargo.lock`, `tests/ui-compact`) belong to the concurrent M13G1 workorder and were not touched |

One honest note: the **first** full-suite run (machine load average 3.8, two cargo
builds plus a co-tenant) failed exactly one timing-sensitive test,
`worker::tests::a_cancel_queued_with_the_job_is_observed_before_the_model_load`
(10 s poll deadline), and nothing else — 986/1/10. The test then passed **3/3**
individually and in the quiet full re-run that produced **987/0/10**. It is a
pre-existing load-sensitivity in that test's deadline, not a change from this pass
(this pass changes no Rust source in the workspace).

## Cleanup

- Probe crates: `build/m16df-probe/` and `build/m16df-whisper/` (gitignored, not
  workspace members, own lockfiles). **Delete before merge**:
  `rm -rf build/m16df-probe build/m16df-whisper`. Their build artifacts were removed
  after the measurements; the sources are kept only for review of this report.
- Fixtures and models: `/tmp/m16df/` (≈ 8 MB), `/tmp/silero_vad_v5.1.2.onnx`
  (2.3 MB), `~/.cache/m16df-models/ggml-base.bin` (148 MB). None are in the repo.
- The detached test worktree `/home/ubuntu/m16df-baseline` is removed after the run.
