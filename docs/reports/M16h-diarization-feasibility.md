# M16.6-H — Offline diarization: feasibility and interface decision

**Decision: GO for the integration workorder, conditional on slice 0 — the real
EN+ID fixture gate. The shippable scope is 1–2 speakers until that gate passes;
3+ speakers stay provisional and manual-correctable.**
The sherpa-onnx pipeline already in the graph produces stable, cheap, accurate
labels on a 2-speaker fixture (DER **0.27 %**, count 2/2, identical across three runs)
and on a 3-speaker fixture with long turns (DER **0.00 %**). It fails on rapid
3-way turn-taking: two male voices collapse into one identity for most of the file
(DER **20.6 %**) with both embedding models, so three-person meetings cannot be
sold as automatic until they pass real EN+ID audio. The interface decision below
is built for that: diarization is a derived, replaceable annotation; names and
colours are first-class user edits; the editor shows restrained badges and one
thin lane, never a timeline editor.

**The short version**

- **API surface is real but narrow.** `sherpa-rs 0.6.8` exposes
  `diarize::Diarize` with the full segmentation → embedding → fast-clustering
  path (§2), but no speaker-count getter, no `SetConfig`, no sample-rate getter;
  both model sessions are hard-wired to `num_threads = 1`; `DiarizeConfig`'s
  default forces **4 clusters** and makes `threshold` inert; the progress
  callback's return value is ignored by the C++ pipeline, so **there is no
  mid-run cancellation**.
- **Cost is not a problem.** **0.150 s CPU per audio-second** on 41–44 s fixtures
  and **0.175 s** on a 167 s file on this 2-vCPU VPS (≈6.7× and 5.7× real time,
  one core). Peak RSS **135–152 MB**; model load **0.75–0.91 s**.
- **Models are small and clean.** pyannote segmentation-3.0 **5 992 913 B, MIT**;
  CAM++ zh-en **28 281 164 B, Apache-2.0** → **34 274 077 B** added download.
  NeMo TitaNet-large (101 405 493 B, CC-BY-4.0) was measured and rejected:
  2.4× CPU, 2.3× RSS, and **34.0 %** DER on the same 2-speaker fixture.
- **Turn boundaries are deterministic.** Three fresh model loads × three runs
  produced **identical segment lists** (start, end, speaker) in every 3-run case:
  0 ms boundary movement, same label numbering.
- **Speaker count is the failure axis, not boundaries.** Auto count is 2/2 on
  the 2-speaker fixture; on the rapid 3-speaker fixture it reports 3 labels but
  the two male voices share one cluster (DER 20.6 %); long turns are 3/3 clean.
- **Silence and missing models are errors today.** A silent WAV returns
  `Err("No segments found or invalid pointer.")`; a missing model returns a
  generic init error with no path. Production must map both before the UI.
- **Interface:** additive schema **v7** — `diarization_runs`, `speakers`,
  `speaker_turns`; turns are anchored by **time** (M15.3 split/merge renumbers
  `segments.idx`, so a stored segment range would go stale exactly when the user
  edits text); segment ranges are a read-time projection. No segment text,
  `source_text` or `derived` is ever written.

---

## 1. Machine-named numbers

```text
probe_date=2026-11-11
m16h.decision=GO_NARROWED
host=VM-0-5-ubuntu
cpu=AMD EPYC 7K62 48-Core, 2 vCPU
ram_total_mib=3718
kernel=7.0.0-14-generic
rustc=1.99.0
git_head=b81339f

binding=sherpa-rs 0.6.8 (MIT) -> sherpa-rs-sys 0.6.8 -> sherpa-onnx 1.12.9 (Apache-2.0)
probe_crate=build/m16h-probe (gitignored, not a workspace member)
build_cmd=cargo build --release
models_dir=/tmp/m16h-models
fixtures_dir=/tmp/m16h-fixture

# cost, CAM++ + pyannote fp32 (median of 3 runs unless noted)
two_speakers.audio_s=41.12
two_speakers.cpu_s_per_audio_s=0.1503        # 0.1496-0.1654
two_speakers.wall_x_realtime=0.151
two_speakers.model_load_ms=801               # 744-905 across all runs
two_speakers.rss_peak_mib=152
three_speakers.cpu_s_per_audio_s=0.1503      # 0.1497-0.1515
three_speakers.audio_s=43.77
three_long.audio_s=78.61
three_long.cpu_s_per_audio_s=0.1837          # 78.61 s audio
long_167s.audio_s=167.47
long_167s.cpu_s_per_audio_s=0.1745
long_167s.rss_peak_mib=150
overlap.audio_s=11.44
overlap.cpu_s_per_audio_s=0.0481

# accuracy (frame DER, 10 ms frames, 250 ms collar, optimal injective mapping)
two_speakers_c2.der=0.0027
two_speakers_auto.der=0.0027
two_speakers_c4.der=0.1367                   # binding default: 4 clusters
three_speakers_c3.der=0.2064
three_speakers_auto.der=0.2064
three_speakers_c4.der=0.2064
three_long_c3.der=0.0000
three_rms_c3.der=0.1313
two_male_c2.der=0.0916
two_speakers_titanet.der=0.3401
three_speakers_titanet.der=0.2064

# determinism: 3 fresh Diarize instances, same process, same params
two_speakers_c2.runs_identical_segments=true  # 10/10/10 segments, exact floats
three_speakers_c3.runs_identical_segments=true # 12/12/12
boundary_max_delta_ms=0
label_numbering_identical=true

# cancellation surface (callback returns 1 from its first call)
cancel.callbacks=66                           # two-speaker fixture
cancel.aborted=false
cancel.compute_ms=6132                        # full run, same as without callback
```

## 2. The binding, verified from source

Crate source read at `~/.cargo/registry/src/index.crates.io-*/sherpa-rs-0.6.8/`
and its vendored `sherpa-onnx/sherpa-onnx/`.

### 2.1 What the safe API exposes

| item | what it gives | source |
|---|---|---|
| `diarize::Diarize::new(segmentation_model, embedding_model, config)` | builds one offline pipeline; errors with `Failed to initialize offline speaker diarization` on a null handle | `src/diarize.rs:43-91`, error at `:87-89` |
| `diarize::DiarizeConfig` | `num_clusters: Option<i32>`, `threshold: Option<f32>`, `min_duration_on`, `min_duration_off`, `provider`, `debug` — **no `num_threads`** | `src/diarize.rs:19-27` |
| `DiarizeConfig::default()` | `num_clusters = Some(4)`, `threshold = 0.5`, durations `0.0` | `src/diarize.rs:29-40` |
| `Diarize::compute(samples: Vec<f32>, progress_callback) -> Result<Vec<Segment>>` | whole waveform by value, 16 kHz assumed; returns `{start, end, speaker}` f32/f32/i32, sorted by start | `src/diarize.rs:93-148`; `Segment` at `:10-15`; sort at `:122-123` |
| progress callback | `Fn(i32 done, i32 total) -> i32`; **the return value is ignored** by the C++ implementation | Rust `:151-158`; C++ `offline-speaker-diarization-pyannote-impl.h:510-512` |
| zero segments | becomes `Err("No segments found or invalid pointer.")`, not `Ok(vec![])` | `src/diarize.rs:139-141` |

Config construction details that matter:

- both sessions are created with `num_threads: 1` hard-coded — embedding at
  `src/diarize.rs:68`, segmentation at `:79`. There is no way to use the second
  core from this binding.
- `num_clusters` is passed as `config.num_clusters.unwrap_or(4)` (`:57`), so
  `None` means **4 speakers**, not "auto". `0` or a negative value is the only
  way to reach threshold-based clustering; the C++ branches on
  `num_clusters > 0` (`fast-clustering.cc:59-65`) and otherwise uses
  `cutree_cdist(threshold)`.
- `provider` defaults to `"cpu"` (`src/lib.rs:31-44`); macOS could pass
  `coreml`, untested here.
- model paths are converted with `to_str().unwrap()` (`:53-54`): a non-UTF-8
  path panics before sherpa sees it. Model Manager paths are UTF-8 today.

### 2.2 What the C API has but the Rust safe wrapper does not

| C API symbol | C header line | exposed by `sherpa-rs`? |
|---|---|---:|
| `SherpaOnnxOfflineSpeakerDiarizationGetSampleRate` | `c-api.h:1670-1671` | **no** |
| `SherpaOnnxOfflineSpeakerDiarizationSetConfig` | `c-api.h:1674-1676` | **no** |
| `SherpaOnnxOfflineSpeakerDiarizationResultGetNumSpeakers` | `c-api.h:1687-1688` | **no** |
| `...ProcessWithCallback` | `c-api.h:1722-1725` | yes (via `compute`) |

The raw `sherpa-rs-sys` bindgen output does contain `...GetNumSpeakers` and
`...SetConfig` (verified in the generated `bindings.rs`, lines 5202 and 5272),
so the production adapter can call them through `sherpa-rs-sys` exactly as the
Parakeet module already does for token timestamps. It needs `sherpa-rs/sys` in
its feature list; no new crate.

Consequences for production:

1. **Sample rate is the caller's contract.** `compute` takes no rate; the
   segmentation metadata says 16 000 Hz. `suramak-core::resample::to_16khz_mono`
   (`crates/suramak-core/src/resample.rs:16`) must run first. Passing 8 kHz audio
   yields timestamps at half scale, silently.
2. **Speaker count must be counted by the app** (distinct `speaker` values) or
   read through the sys API. The safe wrapper has no count.
3. **Re-clustering without reload** is possible only through `SetConfig`
   (sys). Without it, changing the count means dropping and rebuilding the
   diarizer: 0.75–0.91 s here, acceptable, but it reloads both ONNX sessions.
4. **Cancellation does not exist mid-run.** The callback is progress-only. A
   production job can mark itself cancelled and discard the result, but the
   worker keeps burning CPU until sherpa returns. On this box a 1 h file is
   ~10.5 min of one core (§6.1), so the UI must not show a cancel button that
   implies immediate stop; either cap duration, isolate in a killable child, or
   label the action "stop after this file" honestly.

### 2.3 Segmentation model shapes (from the pinned ONNX metadata)

Read with `build/m16h-probe/fixtures/onnx_meta.py` from the pinned files:

```text
model.onnx (pyannote-segmentation-3.0)
  model_type=pyannote-segmentation-3.0
  sample_rate=16000   window_size=160000 samples (10 s)
  num_speakers=3      powerset_max_classes=2   num_classes=7
  receptive_field_size=991  receptive_field_shift=270 frames
  input  x: [N, 1, T]
  output y: [N, floor(floor(floor(floor(T/10 - 251/10)/3 - 2/3)/3)/3 - 8/3) + 1, 7]
```

The 7 output classes are the powerset of up to 2 simultaneous speakers among 3
(`sherpa-onnx/csrc/offline-speaker-diarization-pyannote-impl.h:191-225`); frames
are 10 ms after the model's own encoder stacking. This is why the pipeline can
represent overlapping speech as two simultaneous labels, but why **three
speakers inside one 10 s window** is the hard case measured in §6.4.

## 3. Candidate models: files, shapes, licences

Downloaded once to `/tmp/m16h-models` (never the repo), sha256 verified locally.

| role | file | bytes | sha256 | licence | metadata |
|---|---|---:|---|---|---|
| segmentation | `sherpa-onnx-pyannote-segmentation-3-0/model.onnx` | 5 992 913 | `220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079` | **MIT** (CNRS), `LICENSE` 1 061 B sha256 `14d7016ad68e7394d6e6b78d96cc2ae431c905287b89674cfdf021e79e62b8ba` | 16 kHz, 10 s window, 3 speakers max, 7 powerset classes |
| segmentation (small, untested) | `.../model.int8.onnx` | 1 540 506 | `d582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d` | MIT (same archive) | int8 quantisation of the same graph |
| embedding **chosen** | `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx` | 28 281 164 | `aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2` | **Apache-2.0** (ModelScope `iic/speech_campplus_sv_zh_en_16k-common_advanced`, API field `License`) | `framework=3d-speaker`, `output_dim=192`, input `[N,T,80]` fbank, `feature_normalize=global-mean`, `normalize_samples=1` |
| embedding rejected | `nemo_en_titanet_large.onnx` | 101 405 493 | `d51abcf31717ef28162f26acb9d44dd4127c3d44c9b8624f699f3425daca8e77` | **CC-BY-4.0** (NVIDIA HF card) | `framework=nemo`, `output_dim=192`, input `[N,80,T]+length`, English |

Archive provenance:

```text
segmentation_archive=https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2
segmentation_archive.bytes=6958444
segmentation_archive.sha256=24615ee884c897d9d2ba09bb4d30da6bb1b15e685065962db5b02e76e4996488
segmentation_source=https://huggingface.co/pyannote/segmentation-3.0 (MIT; converted by k2-fsa, archive README)
embedding_url=https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx
embedding_license_url=https://www.modelscope.cn/models/iic/speech_campplus_sv_zh_en_16k-common_advanced

# pins as machine-named keys
seg.model.bytes=5992913
seg.model.sha256=220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079
seg.model_int8.bytes=1540506
seg.model_int8.sha256=d582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d
seg.license.bytes=1061
seg.license.sha256=14d7016ad68e7394d6e6b78d96cc2ae431c905287b89674cfdf021e79e62b8ba
emb.campplus.bytes=28281164
emb.campplus.sha256=aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2
emb.campplus.license=Apache-2.0
emb.titanet.bytes=101405493
emb.titanet.sha256=d51abcf31717ef28162f26acb9d44dd4127c3d44c9b8624f699f3425daca8e77
emb.titanet.license=CC-BY-4.0
```

**Why CAM++ and not TitaNet.** On the same 2-speaker fixture: DER **0.27 %** vs
**34.01 %**, CPU **0.150** vs **0.362 s/audio-s**, peak RSS **152** vs **351 MiB**,
and TitaNet emitted NaN embeddings for some short turns (the diagnostic in
`build/m16h-probe/src/bin/emb.rs` showed `within A mean=NaN`). It is also
English-only and CC-BY-4.0, a heavier notice obligation than Apache-2.0.
TitaNet on the 3-speaker fixture produced exactly the same DER as CAM++
(0.2064) — the failure there is upstream of the embedding, which is the
important diagnostic in §6.4.

**Languages.** Both models run on waveform/features only — no text, no tokenizer,
no language id. pyannote's model metadata has no language field; CAM++'s
`language=Chinese-English` describes its *training data*, not a runtime
restriction. Diarization is therefore architecturally language-agnostic and
EN+ID audio is covered by the same graph. That is not the same as *measured* on
Indonesian: this box has no Indonesian multi-speaker recording, and every
fixture here is synthetic English TTS (§5). The M16.6 verify list still owns
real EN+ID fixtures; §9 makes them the first gate.

**Package impact.** 34 274 077 B (32.69 MiB) added download for the tested pair,
or 29 821 670 B with the int8 segmentation model (not probed). Models would be
runtime downloads through Model Manager, not bundled into the app; the sherpa
dylibs themselves were already bundled and gated by M16F.

## 4. The alternative direction (one at a time)

The constraint is **no second runtime**: the macOS release already bundles
sherpa-onnx's C API dylibs for Parakeet (`packaging/sherpa-dylibs.tsv`, M16F),
and the Rust binding is in the dependency graph behind `parakeet`/`vad-chunking`.

| candidate | verdict | why |
|---|---|---|
| **sherpa-rs `diarize`** (chosen) | GO for the probe | same runtime and dylibs already bundled/gated; safe wrapper exists; models 34 MB, MIT + Apache-2.0 |
| direct `ort` + pyannote ONNX | reject | would add a second ONNX Runtime binding/version next to the one sherpa links, and require reimplementing the powerset decoding, sliding-window stitching, chunk-speaker embedding extraction and fast clustering — the ~750 lines in `offline-speaker-diarization-pyannote-impl.h` |
| Python `pyannote.audio` | reject | a Python runtime is not shippable inside the native app; also a second model stack |
| whisper.cpp `tinydiarize` | reject | turn detection for a 2-speaker pattern, no speaker labels, no N-speaker clustering; not a diarization pipeline |
| cloud diarization | reject | violates local-first/zero-telemetry; network is limited to user-initiated model downloads and the opt-in Polish endpoint |
| another Rust diarization crate | not pursued | every plausible one links `ort` or its own runtime, which is the constraint this direction exists to avoid |

One direction only, as the workorder asked: the sherpa path is the only one that
reuses the runtime already in the release.

## 5. Fixtures: provenance, commands, hashes

Generated once on this VPS with ffmpeg 8.0.1's libflite filter. Three distinct
voices: `kal` (US male), `awb` (Scottish male), `slt` (US female). Each line is
synthesized separately, trimmed to its speech span (amplitude threshold 250,
30 ms margin), peak-normalised to 0.7, then laid on a 16 kHz mono timeline with
explicit gaps. The generator is `build/m16h-probe/fixtures/generate_fixtures.py`
(primary fixtures) and `gen_variants.py` (diagnostic variants); the manifest
records every utterance's exact start/end and is the scoring ground truth.

Command per utterance:

```bash
ffmpeg -v error -f lavfi -i "flite=textfile=<line>.txt:voice=<kal|awb|slt>" \
       -ar 16000 -ac 1 -f s16le -
```

Assembly: concatenate `silence(gap) + utterance` in Python, write 16-bit mono
WAV; the manifest's `start_ms`/`end_ms` come from the same sample counts, so the
ground truth cannot drift from the audio.

| fixture | turns | speakers | duration | bytes | sha256 |
|---|---:|---|---:|---:|---|
| `two_speakers.wav` | 10 | A=kal, B=slt | 41 116 ms | 1 315 786 | `9fe8435405320245da22b610895bd72314f71ec4b2d42b931d9ad8eb18688dc8` |
| `three_speakers.wav` | 12 | A=kal, B=awb, C=slt | 43 768 ms | 1 400 644 | `756ae6114e6a7780429c604c2205b3e4aef170d5a3c148fe033d6bab0b9f6ad4` |
| `overlap.wav` | 3 | kal, awb, slt | 11 441 ms | 366 156 | `90a785eeaa7c708bec63575748509a85c315022a023dbf33b275a9304f14cead` |
| `two_male.wav` (diagnostic) | 10 | kal, awb | 40 510 ms | — | `8632fe0899bc50caec97af480ad137a46b4c3031e5bb81789a44e47841c8e041` |
| `three_long.wav` (diagnostic) | 6 | kal, awb, slt, ~13 s turns | 78 612 ms | — | `f1d8e97d57ce1b675ceae30b34e5e0ef9ac9f93465230c7bb96e314e62010a31` |
| `three_rms.wav` (diagnostic) | 12 | kal, rms, slt | 45 391 ms | — | `0522a03f9ada3ab04572d98be1ddd0e46e67eb12e4a848016254bf95f3a30b7c` |

Gap pattern: 650–700 ms between different speakers; the 2-speaker fixture has
one 1.5 s same-speaker gap (turn 6→7) and one 250 ms inter-speaker gap (8→9) to
exercise close boundaries. The overlap fixture places B 0.7 s after A ends and C
1.0 s before B ends.

```text
two_speakers.sha256=9fe8435405320245da22b610895bd72314f71ec4b2d42b931d9ad8eb18688dc8
three_speakers.sha256=756ae6114e6a7780429c604c2205b3e4aef170d5a3c148fe033d6bab0b9f6ad4
overlap.sha256=90a785eeaa7c708bec63575748509a85c315022a023dbf33b275a9304f14cead
two_male.sha256=8632fe0899bc50caec97af480ad137a46b4c3031e5bb81789a44e47841c8e041
three_long.sha256=f1d8e97d57ce1b675ceae30b34e5e0ef9ac9f93465230c7bb96e314e62010a31
three_rms.sha256=0522a03f9ada3ab04572d98be1ddd0e46e67eb12e4a848016254bf95f3a30b7c
```

**Honest boundary:** these are TTS voices, not human conversation. They have no
room tone, no crosstalk beyond the planted 1 s overlap, no accent variety beyond
the three flite voices, no phone/far-field channel and no Indonesian. They are
the right instrument for runtime, determinism and gross-accuracy questions; they
are **not** evidence that the same DER holds on real meetings. The generator
itself is deterministic: re-running it reproduces the same bytes and hashes.

## 6. Probe results

All runs: `Diarize::new` then `compute` on the full waveform, fp32 segmentation,
CAM++ unless named otherwise, `min_duration_on=0`, `min_duration_off=0`.
"DER" is frame-level with 10 ms frames, a 250 ms collar around ground-truth
boundaries, and the best injective mapping between predicted and true labels
(`build/m16h-probe/fixtures/score.py`). The unmodified output segments are in
`build/m16h-probe/results/`.

### 6.1 Cost, memory, load

| fixture | audio | CPU s / audio s | wall / realtime | load ms | peak RSS |
|---|---:|---:|---:|---:|---:|
| two_speakers, c=2 | 41.12 s | 0.1503 (0.1496–0.1654) | 0.151 | 801 | 152 MiB |
| three_speakers, c=3 | 43.77 s | 0.1503 (0.1497–0.1515) | 0.150 | 757 | 136 MiB |
| three_long, c=3 | 78.61 s | 0.1837 | 0.184 | 796 | 147 MiB |
| long_167s (4× two_speakers, 1 s gaps) | 167.47 s | 0.1745 | 0.175 | 810 | 150 MiB |
| overlap, c=3 | 11.44 s | 0.0481 | 0.049 | 793 | 129 MiB |

One core is used (both sessions hard-wired to 1 thread), and wall time tracks CPU
within 1 %. The short-fixture number is the one to quote for UX: a 10-minute
recording costs about **90–105 s** on this VPS; a 1-hour recording costs about
**10.5 min** (projection from 0.175 s/s, not measured). Memory is dominated by
the two ONNX sessions (~136 MB); the waveform is 64 KB per audio-second, so a
1-hour file adds 230 MB of `Vec<f32>` — a projected ~370 MB peak, not measured.

### 6.2 Turn-boundary stability (3/3)

Three fresh `Diarize` instances, same model files, same parameters, same
waveform:

| case | segments per run | boundary max delta | raw label sequence | exact segment lists |
|---|---|---:|---|---|
| two_speakers c=2 | 10 / 10 / 10 | 0 ms | identical | **identical** |
| two_speakers auto | 10 / 10 / 10 | 0 ms | identical | **identical** |
| two_speakers c=4 | 10 / 10 / 10 | 0 ms | identical | **identical** |
| three_speakers c=3 | 12 / 12 / 12 | 0 ms | identical | **identical** |
| three_speakers c=4 | 12 / 12 / 12 | 0 ms | identical | **identical** |
| three_speakers auto | 12 / 12 / 12 | 0 ms | identical | **identical** |

"Identical" means the JSON segment objects compare equal, including f32 start/end
and speaker id. There is no run-to-run jitter to design around: a re-run on the
same file and parameters is reproducible, so the UI does not need to reconcile
shifting boundaries. Different *cluster counts* do change labels (c=2 vs c=4) and
can change the post-processing's label assignment, so the run's parameters
belong in its provenance row.

### 6.3 Speaker count and DER, 2- and 3-speaker fixtures

| case | true | labels | segments | DER | miss | false alarm | confusion |
|---|---:|---:|---:|---:|---:|---:|---:|
| two_speakers c=2 | 2 | 2 | 10 | **0.0027** | 0.11 s | 0.00 s | 0.00 s |
| two_speakers auto | 2 | 2 | 10 | **0.0027** | 0.11 s | 0.00 s | 0.00 s |
| two_speakers c=4 (binding default) | 2 | 4 | 10 | 0.1367 | 0.12 s | 0.00 s | 5.50 s |
| three_speakers c=3 | 3 | 3 | 12 | 0.2064 | 0.00 s | 0.00 s | 9.03 s |
| three_speakers auto | 3 | 3 | 12 | 0.2064 | 0.00 s | 0.00 s | 9.03 s |
| three_speakers c=2 | 3 | 2 | 12 | 0.4365 | 0.00 s | 0.00 s | 19.10 s |
| three_speakers c=4 | 3 | 3 | 12 | 0.2064 | 0.00 s | 0.00 s | 9.03 s |
| three_long c=3 | 3 | 3 | 6 | **0.0000** | 0.00 s | 0.00 s | 0.00 s |
| three_rms c=3 | 3 | 3 | 13 | 0.1313 | 0.00 s | 0.00 s | 5.96 s |
| two_male c=2 | 2 | 2 | 11 | 0.0916 | 0.11 s | 0.00 s | 3.60 s |

Speaker-count accuracy, read literally as "number of labels", is perfect on
every 3-speaker case — and that is exactly why the literal metric is not enough.
The count is right while the identity is wrong (§6.4). For the 2-speaker fixture
the count is 2/2 for auto and c=2, and 4/2 for the binding's default, which is a
real trap: **the default path forces 4 clusters on this 2-person recording**
(DER 13.7 %). Production must never call `DiarizeConfig::default()` unchanged.

### 6.4 The 3-speaker failure, precisely

The rapid 3-speaker fixture (3–4 s turns, 0.65 s gaps) returns 12 segments that
line up with the 12 utterances — all boundaries within **190 ms** of ground
truth (most ≤ 100 ms; the largest is the first speech onset, 0.49 s vs 0.30 s) —
and then assigns the two male voices to the same identity. Time-weighted
attribution of each true speaker's speech to the returned labels:

| true speaker | predicted label 0 | predicted label 2 | predicted label 1 |
|---|---:|---:|---:|
| A (kal, male) | 67 % | 33 % | 0 % |
| B (awb, male) | 81 % | 19 % | 0 % |
| C (slt, female) | 0 % | 0 % | **100 %** |

So there are **three labels but two acoustic identities**: the male pair is one
cluster for the first 32 s, then the model splits that cluster in the last two
turns (33.2–39.9 s) into label 2 — a temporal split, not a speaker split. The
female voice is clean throughout.

The same collapse happens with **TitaNet** (identical 12 segments, identical
confusion 9.03 s, DER 0.2064), and with c=2/c=3/c=4 and auto. The embedding is
not the cause; the segmentation model's per-window speaker tracking (or the
C++ `FinalizeLabels`/`ReLabel` path that consumes it) is. Long turns do not
trigger it: `three_long` with ~13 s turns and the same three voices is **DER
0.0000**, six segments for six turns. A second rapid 3-speaker variant with
`kal/rms/slt` improves to DER 0.1313 but still has a wrong turn split, so the
voice pair matters but does not remove the failure.

**Interpretation:** the honest capability line is *"reliable for two voices; a
third voice is only reliable when turns are long enough that all three are
rarely inside one 10 s segmentation window."* The product must not promise
automatic 3+ speaker accuracy from this model set. Manual correction can fix a
merged pair only by reassigning each turn, which is usable but not "one click".

### 6.5 API edges: silence, missing model, cancellation

| case | result | source |
|---|---|---|
| 5 s digital silence | `Err("No segments found or invalid pointer.")`; the job must map this to "no speech" not "failed" | `diarize.rs:139-141` |
| missing segmentation file | `Err("Failed to initialize offline speaker diarization")`, no path in the message; pre-flight existence + size/sha check needed for a named error | `diarize.rs:85-89` |
| callback returns `1` on first call | callback invoked 66 times, **no abort**, full 6.13 s compute, 10 segments returned | C++ `offline-speaker-diarization-pyannote-impl.h:510-512` ignores the return |
| 8 kHz input | the safe API has **no sample-rate parameter**: 8 kHz samples would be read as 16 kHz (doubled-speed audio, half-scale times). Not run; `to_16khz_mono` is mandatory | `diarize.rs:93-97`, `c-api.h:1670-1671` not wrapped |

The cancellation finding is the most consequential API gap. A progress callback
that looks cancellable (`-> i32`) is not: sherpa calls it and drops the value.
The production job lifecycle (M16.1) must not expose a cancel control that
promises immediate stop. Options, in order of preference for the integration
workorder: (a) cap input duration and call it "stop after this recording";
(b) run diarization in a killable child process; (c) upstream a real abort flag.
(a) is the only one that fits this slice.

### 6.6 Overlap

`overlap.wav` has A 0.3–3.8 s, B 4.5–8.5 s, C 7.5–11.1 s (B/C overlap 1.04 s).
The pipeline returns three segments: A 0.32–3.81 (label 0), B 4.49–8.55 (label 0),
C 7.51–11.08 (label 2). Two things follow:

- **Overlapping speech is representable**: B and C overlap for 1.04 s and the
  result keeps two segments with different labels over that interval. The UI
  must therefore handle a segment/turn map where one time range has two speakers;
  a single "speaker per row" badge is a simplification, not the data model.
- **Identity in overlap is not fixed by it**: A and B are again merged (both
  label 0), the same male-pair collapse as §6.4. Do not claim overlap-aware
  diarization; claim "overlapping segments are preserved, identities are
  provisional".

### 6.7 Long audio

`long_167s.wav` is `two_speakers.wav` repeated four times with 1 s silences:
167.47 s, 40 segments, 2 labels, CPU 29.23 s (0.1745 s/audio-s), RSS 150 MiB.
The cost scales linearly within measurement noise; memory does not grow with the
number of segments (it grows with the waveform, which is one `Vec<f32>`). A 1 h
file is a projection, not a measurement; the production slice must measure it on
the target Mac before promising a duration cap.

### 6.8 TitaNet comparison (why the rejected model is rejected)

| case | DER | CPU s/audio-s | peak RSS | notes |
|---|---:|---:|---:|---|
| two_speakers c=2, CAM++ | **0.0027** | 0.150 | 152 MiB | 10 segments |
| two_speakers c=2, TitaNet | 0.3401 | 0.362 | 351 MiB | 11 segments, effective 1 label; NaNs on some short turns |
| three_speakers c=3, CAM++ | 0.2064 | 0.150 | 136 MiB | same failure |
| three_speakers c=3, TitaNet | 0.2064 | 0.365 | 336 MiB | identical failure |

TitaNet is 2.4× the CPU, 2.3× the RSS, 3.5× the download, CC-BY-4.0, and worse
on the easy case. CAM++ is the pin.

## 7. What this probe did not establish

- **No human speech.** Every fixture is flite TTS. Real room tone, overlaps,
  laughter, pauses and accents will move DER; the 2-speaker number is a ceiling,
  not an expectation.
- **No Indonesian fixture.** The pipeline is language-agnostic by construction
  (§3), but EN+ID coverage is unmeasured. The plan's M16.6 verify list still
  requires it; §9 makes it gate 0.
- **No macOS/Windows/Android run.** This is Linux x86_64 only. The M16F dylibs
  are the macOS packaging gate; CoreML provider behaviour is untested (the
  binding accepts `provider`, but the diarization path was probed on CPU only).
- **No 1-hour file.** 167 s is measured; 1 h is arithmetic (0.175 s/audio-s,
  230 MB waveform). The production cap needs a real measurement on the owner's
  Mac.
- **No transcript-alignment test.** No ASR transcript was produced for these
  fixtures. Turn boundaries were scored against the audio's own ground truth,
  not against whisper segments; the segment↔turn mapping in §8.2 is designed
  but unmeasured. Whisper segments are typically 5–15 s and will often straddle
  a turn, so the split-badge/export path is the first thing the integration
  slice must test on a real transcript.
- **No corrupt-file test.** A *missing* model returns the generic init error
  (§6.5); a truncated/corrupt ONNX file was not probed. It will fail in
  `Diarize::new` with the same null-handle error, so the pre-flight size/sha
  check is the only named-error path.
- **No clean-machine dylib run.** The M16F packaging gate covers the bundled
  dylibs on macOS; this probe used the Linux prebuilt bundle from
  `sherpa-rs-sys`. Re-running the M16F clean-machine gate with the diarization
  feature on is a slice-6 item, not repeated here.
- **No confidence scores.** The binding exposes none; the UI cannot show a
  per-turn confidence, only the run's model/params and the "provisional" state.
- **No cancellation.** Deliberately: it does not exist in this version (§6.5).
- **No re-import stability test.** Determinism is proven across fresh model loads
  in one process; identical bytes decoded by a different ffmpeg version could
  differ, but the app decodes once and stores the waveform for the job.

## 8. Interface decision: what `suramak-core` would store

The rule this design follows is M15's: **derived data is separate, replaceable,
and never overwrites the source text; manual correction is user data, not a
cache.** Diarization adds no column to `segments` and no field to `Transcript`.

### 8.1 Additive schema v7 (DDL)

`SCHEMA_VERSION` 6 → 7, one `upgrade_to_v7` transaction, `CREATE TABLE IF NOT
EXISTS` only. No table rebuild, no row rewritten, no column dropped. A fresh
database gets the same objects from the base DDL; an older library starts with
no rows and behaves exactly as today.

```sql
-- One diarization attempt per transcript. The latest run is the active one;
-- a re-run replaces it in one transaction (derived data, not history).
CREATE TABLE IF NOT EXISTS diarization_runs (
    transcript_id       TEXT    NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
    run_id              INTEGER NOT NULL,          -- monotonic per transcript
    created_at          TEXT    NOT NULL,
    segmentation_model  TEXT    NOT NULL,          -- file name
    segmentation_sha256 TEXT    NOT NULL,
    embedding_model     TEXT    NOT NULL,
    embedding_sha256    TEXT    NOT NULL,
    num_clusters        INTEGER NOT NULL,          -- 0 = threshold/auto
    threshold           REAL    NOT NULL,
    min_duration_on     REAL    NOT NULL,
    min_duration_off    REAL    NOT NULL,
    audio_ms            INTEGER NOT NULL,
    PRIMARY KEY (transcript_id, run_id)
);

-- App-level speakers, stable across re-runs. Manual names/colours live here.
CREATE TABLE IF NOT EXISTS speakers (
    transcript_id TEXT    NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
    speaker_id    INTEGER NOT NULL,                -- dense from 0, app-owned
    label         TEXT    NOT NULL,                -- "Speaker 1" or a user name
    color_slot    INTEGER NOT NULL,                -- 0..5, theme palette index
    source        TEXT    NOT NULL,                -- 'auto' | 'manual'
    created_at    TEXT    NOT NULL,
    updated_at    TEXT    NOT NULL,
    PRIMARY KEY (transcript_id, speaker_id)
);

-- The derived turns. Time is the identity; a turn is one contiguous span
-- assigned to one speaker. Overlaps are allowed (two rows may overlap).
CREATE TABLE IF NOT EXISTS speaker_turns (
    transcript_id TEXT    NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
    run_id        INTEGER NOT NULL,
    turn_idx      INTEGER NOT NULL,
    start_ms      INTEGER NOT NULL,
    end_ms        INTEGER NOT NULL,
    speaker_id    INTEGER NOT NULL,
    source        TEXT    NOT NULL DEFAULT 'auto', -- 'auto' | 'manual'
    PRIMARY KEY (transcript_id, run_id, turn_idx),
    FOREIGN KEY (transcript_id, run_id)
        REFERENCES diarization_runs(transcript_id, run_id) ON DELETE CASCADE,
    FOREIGN KEY (transcript_id, speaker_id)
        REFERENCES speakers(transcript_id, speaker_id) ON DELETE CASCADE,
    CHECK (end_ms > start_ms)
);
CREATE INDEX IF NOT EXISTS speaker_turns_span
    ON speaker_turns(transcript_id, run_id, start_ms, end_ms);
```

No `segments` change. No `transcripts` change. No trigger. The cascade from
`transcripts` already removes all three tables, exactly like `segments` and
`word_timings`. Label + colour live once per speaker (`speakers`), not duplicated
per turn: a turn carries `speaker_id`, and a rename/colour change is one row
update that every turn and export sees immediately. Backup/restore needs no
change: `Store::backup_to` is a `VACUUM INTO` of the whole file (new tables
included), and the existing schema-version guard refuses a newer database rather
than guessing.

### 8.2 Why turns are time-anchored, and where "segment ranges" live

The workorder's phrase is "transcript id, segment ranges, a speaker label +
colour". The store keeps **time spans**, not a stored `(first_segment_idx,
last_segment_idx)` pair, for a concrete M15 reason: `segments.idx` is rewritten
by split/merge (`crates/suramak-core/src/segment_edit.rs`: a split inserts a row
and shifts every later idx; a merge shifts them down). A stored segment range
would silently point at different text after the first edit — the one operation
labels must survive. A time span is invariant under text edits.

The segment range is a **read-time projection**, implemented in the store:

- `speaker_for_segments(transcript_id) -> Vec<Option<SpeakerId>>`: for each
  segment row, the turn with the largest temporal overlap (tie → earliest turn).
  This is what the editor badge uses; a segment that overlaps two turns with
  ≥ 25 % each is marked `split` so the UI can show two colours.
- `speaker_turn_ranges(transcript_id) -> Vec<(turn_idx, first_idx, last_idx)>`:
  the segment rows whose **midpoint** falls inside the turn. This is the "segment
  range" for list operations such as "assign these rows".
- The lane renders directly from `speaker_turns.start_ms/end_ms`; no segment
  join is needed for the strip.

If profiling later shows the join is hot, a rebuildable cache column can be added
in v8; the contract above is what the UI and exporters consume.

### 8.3 Store/API surface (what the integration workorder implements)

```text
Store::replace_diarization_run(id, run: &DiarizationRun, speakers: &[Speaker],
                               turns: &[SpeakerTurn]) -> Result<()>
    -- one transaction: delete the old run (cascade turns), insert run,
       upsert speakers, insert turns. Counts and lengths validated first.

Store::diarization_run(id) -> Option<DiarizationRun>
Store::speaker_turns(id) -> Vec<SpeakerTurn>
Store::speaker_for_segments(id) -> Vec<Option<u32>>
Store::speaker_turn_ranges(id) -> Vec<(u32, u32, u32)>

Store::rename_speaker(id, speaker_id, label) -> Result<()>
Store::set_speaker_color(id, speaker_id, slot: u8) -> Result<()>
Store::reassign_turn(id, turn_idx, speaker_id) -> Result<()>       -- source='manual'
Store::merge_speakers(id, from, into) -> Result<()>                -- move turns, delete from
Store::split_speaker(id, new_label, color_slot, turn_idxs: &[u32]) -> Result<()>
Store::delete_diarization(id) -> Result<()>                        -- run + turns; speakers with it
```

All mutating methods are single transactions; the read methods take `&self` and
never write. `run_id` is `MAX(run_id) + 1`, read *before* the old row is deleted,
so the sequence stays monotonic even though only one run is kept.
`reassign_turn` / `split_speaker` set `speaker_turns.source = 'manual'`; that is
how a re-run knows manual work exists.

### 8.4 How the M15 provenance rules apply

1. **Derived, not destructive.** A run writes only v7 tables. `segments.text`,
   `segments.source_text`, `segments.derived` and `word_timings` are untouched.
   Test: snapshot every `segments` row (all columns) before a diarization
   apply + delete, compare byte-for-byte after — must be equal.
2. **Never overwrite the source text.** There is no speaker prefix in `text`.
   Speaker labels are joined at render/export time. Test: export with labels on
   and off, assert the transcript rows are unchanged and the default SRT/VTT
   output is byte-identical to today's.
3. **Manual correction is first-class.** A rename or colour change is a user
   row in `speakers` (`source='manual'`), not a derived cache. A re-run:
   - matches new clusters to existing speakers by maximum temporal overlap
     (greedy; ≥ 20 % of the smaller side's speech keeps the name/colour);
   - keeps unmatched `manual` speakers in `speakers` (they simply have no turns
     in the active run) so a name is never silently deleted;
   - requires confirmation when any `speaker_turns.source='manual'` row exists,
     because replacing turns would discard those edits;
   - never renames a manual speaker automatically.
4. **Provenance.** The run row pins file names, sha256s and all clustering
   parameters, plus `created_at` and `audio_ms`. The UI shows this in the
   speaker popover ("pyannote-segmentation-3.0 + CAM++ · 3 speakers · auto"),
   not in the transcript body.
5. **Staleness.** Text edits do not stale turns (time-anchored). A segment that
   straddles a turn boundary is a *view* problem: the badge shows the dominant
   speaker plus a split marker; the speaker-labelled export splits the cue at
   the turn boundary in its own output, leaving the stored segment intact.
   Replacing a transcript's audio is not a path that exists today (import
   creates a new transcript); if a future edit path replaces it, that path must
   delete the run in the same transaction — turns never cross audio.
6. **Deletion.** "Remove speaker labels" is one transaction: delete the run
   (turns cascade) and the transcript's `speakers` rows. It is an explicit,
   confirmed action; the text is never affected.

### 8.5 Re-run and correction semantics

- One active run per transcript. A re-run is explicit (the "Re-run" button),
  never automatic on open or on text edit.
- The speaker-count control offers **Auto** (threshold) and **2–6**. The
  measured default must be Auto or 2 for a 2-person file; **never** the binding's
  `None`/4 default (§6.3).
- Re-run while a job is already diarizing the same transcript is refused (one
  job per transcript, the M16.1 lifecycle).
- Manual operations available in the first slice: rename, colour, reassign one
  turn, reassign a contiguous selection, merge two speakers, split a speaker by
  selecting turns. These are enough to repair the failure in §6.4 (merged pair)
  without a timeline editor.
- Undo: each operation is a store transaction and must be reversible by the
  editor's existing undo path or a single-step "Revert" in the popover; the
  integration workorder picks the mechanism. No operation is silently
  destructive except merge (which deletes the source speaker row) and delete
  labels, both confirmed.

### 8.6 The editor form: restrained badges and one lane

The plan's decision is "restrained badges/tints after diarization; no full
timeline editor" (`docs/PLAN-M15-M18.md:71`). The form:

- **Row badge.** A 18×18 px rounded chip at the start of each segment's text
  row, preceded by a 3 px colour bar, with a 11 px numeral (or the initial of a
  renamed speaker). Fill = the slot colour at 12 % over the card; numeral =
  `ink`. Hover reveals the label; click opens the speaker popover. No badge at
  all when the transcript has no run — no placeholder "Speaker ?" rows.
- **Lane strip.** One 8 px strip between the time ruler and the waveform, full
  editor width: contiguous bands per turn at 30 % alpha, 1 px separators. Two
  simultaneous speakers (the overlap case) draw as two 4 px half-height bands,
  not as a blended colour. No labels on the strip; hover tooltip only. No
  dragging, no zoom-specific redraw beyond the existing waveform repaint.
- **Status chip.** "Speakers · 2" plus a quiet "auto" tag; the popover footer
  says "Labels describe voices in this recording, not people." This is the
  plan's session-local rule, visible at the point of use.
- **Popover.** 280 px wide: speaker rows (swatch, editable name, turn count),
  colour cycle, "Merge into…", "Re-run diarization" with the count selector,
  "Remove labels". The run provenance line sits at the bottom in `ink_faint`.
- **Colour is never the only channel.** Every badge carries a numeral/initial,
  and the lane's bands are separated by hairlines. The six slots are chosen per
  theme and are asserted by the existing `contrast_ratio` test
  (`crates/suramak-ui/src/deck/theme.rs:414-441, 749-847`): 3:1 against the card
  for the bar, 4.5:1 for the numeral on the 12 % tint, both ladders.
- **Motion.** At most a 120 ms fade when the lane first appears; suppressed by
  the existing reduced-motion path. No idle animation.
- **Scope guards.** No per-word colours, no speaker column in the library list,
  no speaker labels in search snippets, no badges in the compact HUD, no
  timeline editor. Exports get an explicit "Include speaker labels" toggle,
  default **off**.

Suggested palette (to be validated by the contrast test, not taken on faith):
slot 0 = the existing accent blue, 1 = warn amber, 2 = teal, 3 = plum,
4 = olive, 5 = rose; each has a light and a dark ladder value. The implementation
must not ship a slot that fails the contrast assertions.

### 8.7 Export

- Default SRT/VTT output is unchanged, byte for byte.
- A new opt-in path prefixes cues with `"<label>: "` (SRT) or uses the VTT
  `<v Label>` voice span; it splits a cue at a turn boundary only when the cue
  straddles two speakers and the toggle is on. It reads `speaker_for_segments`
  + `speaker_turns`; it never writes.
- Bilingual export composes with it (speaker prefix applies to both lines), but
  the first slice may ship monolingual only if the composition risks the M15.8
  invariants; the integration workorder decides and tests.

### 8.8 Non-goals for this integration

No known-person recognition, no cross-session voice matching, no speaker
directory (M18.1), no per-turn confidence, no realtime labels, no automatic
re-run, no speaker-aware search, no timeline editing, no labels for audio that
has no stored transcript.

## 9. Decision block: GO, narrowed, with the slice list

**GO** for production integration in this order. Slice 0 is a hard gate; if it
fails, the decision becomes STOP for the feature (the probe and the schema
design stay as the record).

| # | slice | scope | gate before the next slice |
|---|---|---|---|
| 0 | **Real EN+ID fixture gate** (probe-only) | record/collect 2-speaker and 3-speaker real audio in EN and ID (the M16.6 verify fixtures), run the pinned CAM++ pair, score with the same script | 2-speaker: DER ≤ 10 % and count correct on ≥ 4/5 files; 3-speaker: DER ≤ 20 % and count correct on ≥ 3/5. If 2-speaker fails → **STOP**. If 3-speaker fails → continue **2-speaker only**, hide the 3+ promise |
| 1 | **Schema v7 + types + store** | `DiarizationRun`, `Speaker`, `SpeakerTurn`; `upgrade_to_v7`; replace/read/rename/colour/reassign/merge/split/delete; cascade and provenance tests | migration from a populated v6 library is additive and byte-preserving for all existing rows; split/merge then read mapping is correct; deleting a transcript removes all three tables; default export unchanged |
| 2 | **Inference adapter** (`suramak-asr`, behind the existing sherpa feature) | 16 kHz conversion via `to_16khz_mono`; model pins in `models.rs` + Model Manager install (`segmentation.onnx`, `embedding.onnx`); count selector (Auto/2–6); silence → empty, missing/corrupt → named errors; duration cap; progress events; **no fake cancel** | missing/corrupt model names the file; silence returns zero turns; a 30-min file completes with bounded memory; progress reaches 100 %; cancel discards and does not claim to stop the worker |
| 3 | **Persistence wiring** | apply a run, active-run replacement, manual-name matching by overlap, re-run confirmation when manual turns exist, delete labels | re-run keeps a renamed speaker's name and colour on the same voice; unmatched manual names survive; a manual turn edit triggers the confirmation; segment rows byte-identical before/after every path |
| 4 | **Editor UI** | badges + lane + popover + count selector + states (none/running/empty/failed); accessibility and contrast tests | keyboard-only rename/reassign; six slots clear the contrast assertions on both ladders; no badge when no run; a 3-speaker merged pair can be repaired in ≤ 2 actions per turn; reduced motion honoured |
| 5 | **Speaker-labelled export** | opt-in SRT/VTT prefix / `<v>`; straddling cue split; bilingual composition | labels never enter `segments.text`; default output byte-identical; straddling cue split does not change the stored segment |
| 6 | **Verification and docs** | EN+ID real fixtures re-run, long-file measurement on the owner's Mac, clean-machine dylib load (M16F gate), corrupt/resume download, diagnostics line, licence notices | all M16.6 verify items have a named result; package/licence surface updated with pyannote MIT + 3D-Speaker Apache-2.0; 3+ claim matches the measured gate |

**What is deliberately out of slice 1–6:** realtime, cross-session identity,
speaker directory, search integration, per-word labels, timeline editing,
automatic re-runs, any change to `segments`/`transcripts`.

## 10. Re-visit conditions

- **3+ speakers**: re-run the §9 gate when a newer sherpa-onnx (or a different
  pinned segmentation model) claims better multi-speaker tracking. The specific
  failure to retest is the rapid 3-way fixture: two male voices sharing one
  label for 32 s. Do not re-open the 3+ claim on a model bump alone.
- **Cancellation**: re-open when sherpa-onnx exposes an abort that the C++
  pipeline honours, or when the job runner can isolate diarization in a killable
  child. Until then, the duration cap + discard semantics are the contract.
- **Indonesian**: re-open the language question only with real ID fixtures; the
  architecture is language-agnostic, the measurement is not.
- **Long files**: if the owner's Mac measures > 15 min CPU for a 1 h file or
  RSS > 600 MB, reduce the cap rather than ship a job that looks hung.

## 11. Verification of the untouched workspace

```text
git_status=clean apart from this report (build/ is gitignored)
cargo fmt --all --check        = clean
cargo clippy --workspace --all-targets -- -D warnings = clean (finished in 1.51 s on a warm target)
cargo test --workspace        = 1079 passed / 0 failed / 10 ignored (36 test binaries), exit 0
```

The probe is a throwaway crate at `build/m16h-probe` (its own `[workspace]`,
`build/` is gitignored); its models live in `/tmp/m16h-models`, its fixtures in
`/tmp/m16h-fixture`, and its result JSONs are copied to
`build/m16h-probe/results/` for traceability. No production file, dependency,
CI file or test was touched.

## Appendix — reproduction

```bash
# 1. models (once; /tmp)
curl -L -o /tmp/m16h-models/seg.tar.bz2 \
  https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2
tar xjf /tmp/m16h-models/seg.tar.bz2 -C /tmp/m16h-models
curl -L -o /tmp/m16h-models/campplus.onnx \
  https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx

# 2. probe
cd build/m16h-probe && cargo build --release

# 3. fixtures + run matrix (writes /tmp/m16h-fixture and results/)
chmod +x run_matrix.sh && ./run_matrix.sh /tmp/m16h-models

# 4. inspect metadata/shapes
python3 fixtures/onnx_meta.py /tmp/m16h-models/sherpa-onnx-pyannote-segmentation-3-0/model.onnx \
  /tmp/m16h-models/campplus.onnx
```

`run_matrix.sh` is the exact matrix behind the cost/accuracy tables. The 167 s
long-file run is `two_speakers.wav` repeated four times with 1 s silences, one
run, the same binary and models. The silence/missing-model probes are one
invocation each:

```bash
ffmpeg -v error -f lavfi -i "anullsrc=r=16000:cl=mono" -t 5 \
  -c:a pcm_s16le /tmp/m16h-fixture/silence.wav -y
./target/release/m16h-probe --wav /tmp/m16h-fixture/silence.wav \
  --seg /tmp/m16h-models/sherpa-onnx-pyannote-segmentation-3-0/model.onnx \
  --emb /tmp/m16h-models/campplus.onnx --num-clusters 2 --runs 1 \
  --out /tmp/m16h-fixture/silence.json     # exits 101: "No segments found…"
./target/release/m16h-probe --wav /tmp/m16h-fixture/two_speakers.wav \
  --seg /tmp/nope.onnx --emb /tmp/m16h-models/campplus.onnx \
  --num-clusters 2 --runs 1 --out /tmp/m16h-fixture/missing.json
                                             # exits 101: "Failed to initialize…"
```
