# M16.6-H-S0 — Diarization slice 0: the EN+ID multi-speaker gate

**Decision: GO for production. The shippable promise is 1–2 speakers automatic;
3 speakers stay provisional and manual-correctable; 4+ is not promised.**
The gate passes on both primary EN+ID fixtures: 2-speaker DER **3.29 %**
(speech-active reference; 12.12 % under M16H's utterance-span convention, all
of it silence inside TTS sentences and **0.00 s** of speaker confusion), count
**2/2**, boundaries identical **3/3**; rapid 3-way EN+ID DER **2.11 %**, count
**3/3**, identical **3/3**. The 20.6 % male-pair collapse did **not** reproduce
with this trio: every true voice is 100 % attributed to one label. The one new
failure is automatic speaker count on long 3-speaker turns (4 labels at the
binding's default threshold 0.5, DER 10.00 %); threshold 0.6 returns 3 labels
and 0.18 % on these files, but that is a candidate, not a pin, until real audio
confirms it. Real meeting audio remains the owner's device-pass contribution
and the largest unknown in the decision.

**The short version**

- **Fixtures are the product's language shape, not English with an Indonesian
  word.** Three edge-tts WAVs: `id-ID-ArdiNeural` (male), `id-ID-GadisNeural`
  (female), `en-US-GuyNeural` (male); 12 + 12 + 6 turns; 1.6 MB each; hashes in
  §2. Indonesian speakers code-switch into English inside a turn and English
  speakers into Indonesian, which is how the audio arrives in the product.
- **Identity is no longer the failure axis on this set.** With the correct
  count, confusion is **0.00 s** in every case (§3.3). The two male voices do
  not merge. The residual DER is boundary-collar miss: there is not a single
  missed-speech run outside the 250 ms collar in either primary fixture.
- **The English-only ceiling is not broken by Indonesian voices.** 2-speaker
  EN+ID lands at **3.29 %** vs M16H's **0.27 %** English-only TTS (12× the
  ceiling, 3× under the 10 % gate). Rapid 3-way EN+ID lands at **2.11 %** vs
  **20.64 %** English-only: 9.8× lower, because this trio does not collapse.
- **Count is the remaining product risk.** `Auto` (threshold 0.5) returns 3/3
  on the rapid 3-way and 4/3 on long turns; `c=4` — the binding's default —
  returns 4 labels on a 2-person file. The count selector must default to an
  explicit 2/3, not Auto, and the UI must repair an over-split (35/65 % of one
  voice across two labels) in one merge.
- **Stability is total on this box.** 13 cases × 3 fresh model loads produced
  identical segment lists, identical boundary counts and **0 ms** maximum
  boundary delta. Boundary accuracy: median 6.5–17.5 ms, every boundary after
  the first-speech onset within **48 ms** of a true turn start.
- **Cost is unchanged.** **0.157–0.167 s CPU per audio-second**, peak RSS
  **135–152 MiB**, model load **741–826 ms** on the 2-vCPU VPS.
- **Real meeting audio is still missing.** Every fixture is TTS: no room tone,
  no spontaneous crosstalk, no far-field channel, three voices. GO is for the
  1–2 speaker integration; the 3+ claim and real-audio DER remain slice-6
  gates on the owner's device pass.

---

## 1. Machine-named numbers

```text
probe_date=2026-11-11
m16hs0.decision=GO_PRODUCTION_1_2_SPEAKERS_3_PROVISIONAL
host=VM-0-5-ubuntu
cpu=AMD EPYC 7K62 48-Core, 2 vCPU
ram_total_mib=3718
kernel=7.0.0-14-generic
rustc=1.99.0
git_head=7ddd11c
binding=sherpa-rs 0.6.8 (MIT) -> sherpa-rs-sys 0.6.8 -> sherpa-onnx 1.12.9
probe_crate=build/m16h-probe (gitignored, not a workspace member)
fixtures_dir=build/m16h-probe/slice0/audio
results_dir=build/m16h-probe/slice0/results
edge_tts=7.3.1
ffmpeg=8.0.1

# models: unchanged pins, verified by sha256 before the run
seg.model.bytes=5992913
seg.model.sha256=220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079
emb.campplus.bytes=28281164
emb.campplus.sha256=aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2

# fixtures (see §2)
two_speakers_enid.bytes=1674202
two_speakers_enid.duration_ms=52317
two_speakers_enid.turns=12
two_speakers_enid.sha256=7d296597d90586d07d324cd852c39b9140142f5bd1357a63d2c2f36e495c44fc
three_speakers_enid.bytes=1559134
three_speakers_enid.duration_ms=48721
three_speakers_enid.turns=12
three_speakers_enid.sha256=2c01c605175ddcddf0db5a7a6b417d94256514f63386f176c1f1ab4fc2dabaaf
three_long_enid.bytes=1578144
three_long_enid.duration_ms=49315
three_long_enid.turns=6
three_long_enid.sha256=c172a8c2e386d3811183305a349c282d6fb81105774426e15f127c7296bbe45c
manifest.sha256=78c01bb8f6bbc8b1b6ca1908b433dc403ab5b5987bc8b19e5e93b312761f9f5e

# gate, primary fixtures, CAM++ + pyannote fp32, threshold 0.5 unless named
# der_span = M16H publication convention; der_active = speech-active reference (§3.1)
two_enid.auto.der_span=0.1212
two_enid.auto.der_active=0.0329
two_enid.auto.der_active.miss_s=1.72
two_enid.auto.der_active.fa_s=0.00
two_enid.auto.der_active.conf_s=0.00
two_enid.auto.count=2/2
two_enid.auto.stability_3of3=true
two_enid.auto.segments=19/19/19
three_enid.auto.der_span=0.0764
three_enid.auto.der_active=0.0211
three_enid.auto.der_active.miss_s=1.01
three_enid.auto.der_active.fa_s=0.02
three_enid.auto.der_active.conf_s=0.00
three_enid.auto.count=3/3
three_enid.auto.stability_3of3=true
three_enid.auto.segments=16/16/16
three_long_enid.c3.der_span=0.0896
three_long_enid.c3.der_active=0.0018
three_long_enid.c3.count=3/3
three_long_enid.c3.stability_3of3=true
three_long_enid.auto_thr050.count=4/3
three_long_enid.auto_thr050.der_active=0.1000
three_long_enid.auto_thr060.count=3/3
three_long_enid.auto_thr060.der_active=0.0018
two_enid.c4.count=4/2
two_enid.c4.der_active=0.1690

# cost, median of 3 runs (the main matrix, no concurrent build)
two_enid.cpu_s_per_audio_s=0.1581        # 0.1573-0.1581 across cases
three_enid.cpu_s_per_audio_s=0.1630      # 0.1621-0.1665
three_long_enid.cpu_s_per_audio_s=0.1591
two_enid.load_ms=746-789
three_enid.load_ms=741-826
three_long_enid.load_ms=754-799
two_enid.rss_peak_mib=138
three_enid.rss_peak_mib=135
three_long_enid.rss_peak_mib=152

# determinism: 3 fresh Diarize instances per case, same process, same params
cases_with_identical_segments=13
boundary_max_delta_ms=0
label_numbering_identical=true
```

## 2. Fixtures: provenance, commands, hashes

Generated once with **edge-tts 7.3.1** (the repo's committed-fixture voice
generator, `scripts/gen_fixtures.py`) through the Edge read-aloud endpoint,
decoded with ffmpeg 8.0.1. Voices:

| speaker role | voice | locale | gender |
|---|---|---|---|
| Indonesian male | `id-ID-ArdiNeural` | id-ID | Male |
| Indonesian female | `id-ID-GadisNeural` | id-ID | Female |
| English male | `en-US-GuyNeural` | en-US | Male |

Per-line synthesis (the generator calls the same API; the CLI equivalent is):

```bash
/home/ubuntu/.venvs/tts/bin/edge-tts --voice id-ID-ArdiNeural \
         --text "Setuju. The pump needs a new seal before the dry season, jadi kita pesan lebih awal." \
         --write-media line.mp3
ffmpeg -v error -y -i line.mp3 -ar 16000 -ac 1 -f s16le line.raw
```

edge-tts **7.3.1** is required for a working read-aloud token; the venv's
previous 7.2.8 returned HTTP 403 from the synthesis endpoint on this box
(`--list-voices` still worked).

Assembly (`build/m16h-probe/fixtures/edge_tts_fixtures.py`): every line is
trimmed to its speech span (amplitude threshold 250, 30 ms margin), peak-
normalised to 0.7, and laid on a 16 kHz mono timeline with explicit gaps. The
manifest records each utterance's exact `start_ms`/`end_ms` from the same
sample counts, so the ground truth cannot drift from the audio. Synthesis
results are cached in `slice0/audio/synth/`; deleting the cache re-synthesizes
from the service and is not expected to reproduce the bytes.

| fixture | speakers / voices | turns | duration | bytes (MiB) | sha256 |
|---|---|---:|---:|---:|---|
| `two_speakers_enid.wav` | A=Ardi, B=Gadis | 12 | 52 317 ms | 1 674 202 (1.60) | `7d296597d90586d07d324cd852c39b9140142f5bd1357a63d2c2f36e495c44fc` |
| `three_speakers_enid.wav` | A=Ardi, B=Guy, C=Gadis | 12 | 48 721 ms | 1 559 134 (1.49) | `2c01c605175ddcddf0db5a7a6b417d94256514f63386f176c1f1ab4fc2dabaaf` |
| `three_long_enid.wav` (control) | A=Ardi, B=Guy, C=Gadis | 6 | 49 315 ms | 1 578 144 (1.51) | `c172a8c2e386d3811183305a349c282d6fb81105774426e15f127c7296bbe45c` |

Design of each file:

- **Two speakers, alternating.** Gaps 650 ms normally; one rapid exchange at
  180 / 180 / 200 ms (turns 4–6) and another at 180 / 200 ms (turns 11–12).
  Code-switching: turn 3 is one Indonesian turn with an English sentence inside
  it ("The pump needs a new seal before the dry season") and turn 9 is a full
  English turn in an Indonesian voice. Turn durations 2.2–5.7 s.
- **Three speakers, rapid 3-way.** Twelve turns of 1.9–5.5 s with 320–380 ms
  gaps, so two or three voices share most 10 s segmentation windows — the shape
  that collapsed in M16H §6.4. Two Indonesian voices plus one native English
  voice; turn 8 is an Indonesian sentence inside an English turn ("Ada juga
  retak di pipa dekat dapur. It is small, but the wall is damp.").
- **Three speakers, long turns (diagnostic control).** Same three voices, six
  turns of 6.4–9.6 s and 900 ms gaps. Separates "three identities" from "three
  identities inside one window".

```text
two_speakers_enid.sha256=7d296597d90586d07d324cd852c39b9140142f5bd1357a63d2c2f36e495c44fc
three_speakers_enid.sha256=2c01c605175ddcddf0db5a7a6b417d94256514f63386f176c1f1ab4fc2dabaaf
three_long_enid.sha256=c172a8c2e386d3811183305a349c282d6fb81105774426e15f127c7296bbe45c
```

**Honest boundary on the fixtures.** These are neural TTS, not people in a
room: no room tone, no spontaneous overlap, no laughter, no far-field or phone
channel, one voice per speaker and only three voices. They are the right
instrument for language coverage, identity separation, count and determinism;
they are not evidence that the same DER holds on a real meeting. The
code-switched lines are read by one voice, so the switch is lexical, not a
change of speaker — which is exactly the product case.

## 3. The gate

### 3.1 Metric and the two references

Frame DER, 10 ms frames, 250 ms collar around ground-truth boundaries, optimal
injective mapping (`build/m16h-probe/fixtures/score_slice0.py`). As in M16H,
the denominator is total file duration rather than reference-speech duration,
so the two reports are directly comparable; miss, false-alarm and confusion
seconds are reported separately because they carry the interpretation. Two
references are reported because the fixture audio forced the distinction:

- **`der_span` (M16H publication convention).** Every synthesized utterance
  counts as speech from its first to its last sample, including pauses inside
  it. This is what M16H scored.
- **`der_active` (gate number).** A 10 ms frame below **-46 dBFS RMS** is
  non-speech; active runs separated by less than 400 ms are merged, so word
  gaps stay inside a turn while a sentence pause splits it. Silence is not
  scored as missed speech; a prediction over silence is a false alarm.

Why the second reference is not a metric dodge: edge-tts inserts 0.5–1.0 s
pauses inside sentences, and 19 % of the two-speaker fixture's utterance-span
frames are digital silence. The diarizer correctly returns no speaker there,
which the span reference scores as 6.34 s of "missed speech". Re-scoring the
M16H fixtures with the speech-active reference reproduces the published numbers
exactly — **0.27 %**, **20.64 %**, **0.00 %** (`fixtures/rescore_m16h_active.py`,
output in `slice0/m16h_rescore_active.txt`; the male-pair variant moves 9.16 →
9.13 %, the only change). The reference therefore does not move the English
ceiling and the comparison in §4 is like-for-like.

### 3.2 Gate table

`labels` is distinct labels returned; `eff` is labels owning ≥ 10 % of the
audio. `count` is shown as labels/true and marked ✅ only when both the
distinct-label count and the effective count equal the true number. `3/3` means
three fresh `Diarize` instances produced identical segment lists (start, end,
label).

| fixture | true | case | labels (eff) | count | DER span | **DER active** | miss / FA / conf (active) | segments ×3 | max Δ | 3/3 |
|---|---:|---|---:|---:|---:|---:|---|---:|---:|---|
| two_speakers_enid | 2 | **auto (thr 0.5)** | 2 (2) | **2/2** | 12.12 % | **3.29 %** | 1.72 / 0.00 / **0.00 s** | 19/19/19 | 0 ms | ✅ |
| two_speakers_enid | 2 | **c=2** | 2 (2) | **2/2** | 12.12 % | **3.29 %** | 1.72 / 0.00 / **0.00 s** | 19/19/19 | 0 ms | ✅ |
| two_speakers_enid | 2 | c=4 (binding default) | 4 (3) | 4/2 ❌ | 26.13 % | 16.90 % | 1.72 / 0.00 / 7.12 s | 20/20/20 | 0 ms | ✅ |
| three_speakers_enid | 3 | **auto (thr 0.5)** | 3 (3) | **3/3** | 7.64 % | **2.11 %** | 1.01 / 0.02 / **0.00 s** | 16/16/16 | 0 ms | ✅ |
| three_speakers_enid | 3 | **c=3** | 3 (3) | **3/3** | 7.64 % | **2.11 %** | 1.01 / 0.02 / **0.00 s** | 16/16/16 | 0 ms | ✅ |
| three_speakers_enid | 3 | c=4 | 3 (3) | **3/3** | 7.64 % | **2.11 %** | 1.01 / 0.02 / **0.00 s** | 16/16/16 | 0 ms | ✅ |
| three_speakers_enid | 3 | c=2 (under-cluster) | 2 (2) | 2/3 ❌ | 35.22 % | 29.68 % | 1.01 / 0.02 / 13.43 s | 13/13/13 | 0 ms | ✅ |
| three_long_enid | 3 | **c=3** | 3 (3) | **3/3** | 8.96 % | **0.18 %** | 0.07 / 0.02 / **0.00 s** | 11/11/11 | 0 ms | ✅ |
| three_long_enid | 3 | auto (thr 0.5) | 4 (4) | 4/3 ❌ | 19.04 % | 10.00 % | 0.07 / 0.02 / 4.84 s | 12/12/12 | 0 ms | ✅ |
| three_long_enid | 3 | auto (thr 0.6) | 3 (3) | **3/3** | 8.96 % | **0.18 %** | 0.07 / 0.02 / **0.00 s** | 11/11/11 | 0 ms | ✅ |
| three_long_enid | 3 | auto (thr 0.7) | 3 (3) | **3/3** | 8.96 % | **0.18 %** | 0.07 / 0.02 / **0.00 s** | 11/11/11 | 0 ms | ✅ |

Gate verdict per the workorder's per-fixture criteria (same thresholds as
M16H §9):

| gate | threshold | 2-speaker result | 3-speaker result |
|---|---|---|---|
| DER | 2-spk ≤ 10 %; 3-spk ≤ 20 % | **3.29 %** ✅ | **2.11 %** ✅ (rapid); 0.18 % (long, c=3) |
| speaker count | correct | **2/2** ✅ | **3/3** ✅ (auto; long needs 0.6 or c=3) |
| boundary stability | 3/3 identical | **3/3** ✅ | **3/3** ✅ |

The M16H 5-file sample criterion (`≥ 4/5` for 2-speaker, `≥ 3/5` for
3-speaker) is **not** satisfied by this slice: slice 0 has one file per variant,
not five. That remains open evidence (§7), not a passed criterion.

### 3.3 Where each voice lands

Time-weighted share of each true speaker's speech per predicted label, auto
cases:

| fixture | true speaker | label → share |
|---|---|---|
| two_speakers_enid | A (Ardi, ID male) | label 0 **100 %** |
| two_speakers_enid | B (Gadis, ID female) | label 1 **100 %** |
| three_speakers_enid | A (Ardi, ID male) | label 1 **100 %** |
| three_speakers_enid | B (Guy, EN male) | label 0 **100 %** |
| three_speakers_enid | C (Gadis, ID female) | label 2 **100 %** |
| three_long_enid c=3 | A / B / C | labels 0 / 1 / 2, **100 %** each |
| three_long_enid auto 0.5 | A / B | labels 0 / 1, **100 %** |
| three_long_enid auto 0.5 | C (Gadis) | label 2 **34.7 %**, label 3 **65.3 %** |

There is **no identity confusion** in any correct-count case. The failure that
defined M16H (two male voices sharing one label for 32 s) does not occur here:
Ardi and Guy are cleanly separated for the whole 48.7 s file, including the
rapid 320–380 ms exchanges. The only wrong outcome is the auto over-split of
Gadis on long turns, which is a count/threshold failure, not a merge.

### 3.4 Boundary stability and boundary accuracy

Three runs per case, each with a fresh `Diarize` instance:

| fixture | boundary counts | segment counts | max boundary Δ | identical segment lists |
|---|---|---|---:|---|
| two_speakers_enid (auto, c2) | 12 / 12 / 12 | 19/19/19 | 0 ms | ✅ |
| two_speakers_enid (c4) | 13 / 13 / 13 | 20/20/20 | 0 ms | ✅ |
| three_speakers_enid (auto, c3, c4) | 12 / 12 / 12 | 16/16/16 | 0 ms | ✅ |
| three_speakers_enid (c2) | 9 / 9 / 9 | 13/13/13 | 0 ms | ✅ |
| three_long_enid (c3, auto 0.6/0.7) | 6 / 6 / 6 | 11/11/11 | 0 ms | ✅ |
| three_long_enid (auto 0.5) | 7 / 7 / 7 | 12/12/12 | 0 ms | ✅ |

Against ground truth, predicted speaker-change starts are close: median error
**6.5–17.5 ms**, and every boundary after the first-speech onset is within
**48 ms** of a true turn start. The single 269 ms outlier in each 3-speaker
file is the first speech onset (predicted 0.03 s vs annotated 0.30 s), a
leading-silence convention, not a turn-tracking error.

The segment count exceeds the turn count on the 12-turn files (19 and 16) but
not because of speaker errors: the diarizer splits one utterance into separate
speech runs at long sentence pauses. That is correct speech activity and it has
a UI consequence: **one transcript segment can map to more than one speaker
turn**. The M16H design already anchors turns by time and projects the segment
range at read time, so no schema change is needed; the badge must pick the
dominant turn and the lane must draw the gap, not assume one turn per row.

## 4. Where Indonesian voices land vs the M16H English-only TTS ceiling

| case | M16H English-only (flite) | S0 EN+ID (edge-tts) | delta |
|---|---:|---:|---:|
| 2 speakers, auto/correct count | **0.27 %** | **3.29 %** | +3.02 pp |
| 3 speakers, rapid 3-way | **20.64 %** | **2.11 %** | **−18.53 pp** |
| 3 speakers, long turns | **0.00 %** | **0.18 %** | +0.18 pp |
| 2 speakers, same-gender male pair | 9.13 % | not built | — |
| 3 speakers, alternative voices (`kal/rms/slt`) | 13.13 % | not built | — |

Both numbers use the speech-active reference; the M16H column reproduces the
published values exactly under it (§3.1). Reading:

- **On the easy 2-speaker case, Indonesian voices cost 3.0 pp.** 3.29 % is 12×
  the English ceiling but a third of the 10 % gate. The entire residue is
  boundary-collar miss (1.72 s), with **0.00 s** confusion and **0.00 s** false
  alarm; there is no missed-speech run outside the collar. In product terms
  the labels are right and the edges are within a quarter second.
- **On the hard rapid 3-way case, the EN+ID trio is 9.8× better than the
  English-only pair that collapsed.** This is not evidence that Indonesian is
  easier. It is evidence that M16H's 20.6 % was **voice-pair specific**: the
  flite `kal`/`awb` male pair shared an embedding identity, while Ardi/Guy do
  not. Language never enters the model; voice timbre does.
- **The honest ceiling statement:** 0.27 % is a synthetic-English,
  three-voice, no-noise ceiling. 3.29 % is the EN+ID synthetic equivalent.
  Neither is a real-meeting expectation. The gate's 10 % exists precisely to
  absorb that gap; the measured value leaves 6.7 pp of headroom.

## 5. The 3-speaker finding, precisely

M16H concluded that "three speakers inside one 10 s window" collapsed because
the segmentation model's per-window speaker tracking merged the two male
voices. This slice tested the same shape with a different trio and did not
reproduce it: 12/12 turns, 3 labels, 100 % attribution per voice, DER 2.11 %,
stable across three runs. What follows is what can and cannot be inferred.

- **The collapse is pair-dependent, not a universal 3-speaker limit.** A second
  trio passing is not proof that any trio passes; M16H already had one trio
  fail and one partially fail (`three_rms`, 13.13 %). Two data points on each
  side.
- **Long turns introduce a different failure: over-clustering.** At threshold
  0.5, `three_long_enid` returns four labels: Gadis's first 9.6 s turn is split
  60/40 into labels 2 and 3, and her second turn joins label 3 (34.7 / 65.3 %
  of her speech overall). DER is 10.00 % (confusion 4.84 s), count
  4/3. Threshold 0.6 and 0.7 both return 3 labels and 0.18 %, and 0.6 leaves
  the rapid 3-way and the 2-speaker file unchanged (3/3 and 2/2, same DER).
  This is the first evidence that the binding's default `threshold = 0.5` is
  wrong for this model pair on long turns, but three TTS files are not enough
  to change the production pin; slice 2 must reproduce it on real audio.
- **`c=4` remains a trap.** On the 2-speaker file the binding's default
  (`DiarizeConfig::default()` → 4 clusters) returns 4 labels and 16.90 % DER;
  production must never call the default unchanged. This is unchanged from
  M16H and is now re-confirmed on EN+ID audio.
- **Count selector policy.** The measured safe default is an explicit count
  (2 or 3) chosen by the user or inferred from the job's expected speakers;
  `Auto` is a convenience that is right on the rapid fixture and wrong on the
  long one at 0.5. If Auto ships, ship it with 0.6 and a one-click "merge
  speakers" repair, and label it as a suggestion rather than a fact.

## 6. Cost, memory, load

Main matrix only, median of three runs per case; no concurrent build.

| fixture | audio | CPU s / audio s | wall / realtime | load ms | peak RSS |
|---|---:|---:|---:|---:|---:|
| two_speakers_enid auto | 52.32 s | 0.1581 | 0.158 | 750–789 | 137 MiB |
| two_speakers_enid c=2 | 52.32 s | 0.1573 | 0.157 | 746–789 | 138 MiB |
| three_speakers_enid auto | 48.72 s | 0.1621 | 0.162 | 752–826 | 135 MiB |
| three_speakers_enid c=3 | 48.72 s | 0.1630 | 0.163 | 741–780 | 135 MiB |
| three_speakers_enid c=4 | 48.72 s | 0.1665 | 0.175 | 763–783 | 135 MiB |
| three_long_enid c=3 | 49.32 s | 0.1590 | 0.161 | 754–786 | 143 MiB |
| three_long_enid auto | 49.32 s | 0.1592 | 0.160 | 776–799 | 152 MiB |

Same class as M16H (0.150–0.175 s/audio-s, 135–152 MiB, 0.75–0.91 s load). A
10-minute recording costs ~95–100 s of one core on this VPS; the 1-hour
projection (~9.6 min) is arithmetic, not measured. Both ONNX sessions remain
hard-wired to one thread; the second vCPU does not help.

## 7. What slice 0 did not establish

- **No human meeting audio.** Room tone, spontaneous crosstalk, laughter,
  overlapping turns, accents, far-field/phone channels and unknown microphones
  are all absent. The 3.29 % / 2.11 % numbers are TTS best cases for this voice
  set, not expectations for the owner's recordings. This is the largest open
  risk and stays the owner's contribution at the next device pass.
- **One trio, one file per variant.** The M16H 5-file gate (4/5 for 2-speaker,
  3/5 for 3-speaker) is not met; a single passing trio does not establish a
  3-speaker product claim. A second and third voice pair could still collapse.
- **No 4+ speakers.** The segmentation model's powerset covers at most three
  simultaneous speakers; nothing above three was measured.
- **No macOS/Windows/Android run.** Linux x86_64 only; CoreML untested.
- **No 1-hour file.** 49–52 s is measured; 1 h is arithmetic.
- **No transcript alignment.** No ASR transcript was produced; turn↔whisper
  segment mapping is designed (M16H §8.2) but still unmeasured. The 16-segments-
  for-12-turns result makes this more urgent: one segment can straddle two
  turns even within one utterance.
- **No real overlap.** The fixtures are sequential; M16H's planted-overlap
  finding is unchanged and not re-tested.
- **No re-synthesis guarantee.** The fixtures are pinned by sha256, but edge-tts
  is a hosted service: regenerating from an empty cache can produce different
  bytes (voice version, prosody). Treat the WAVs as the evidence, not the
  command output.

## 8. Updated decision and the integration slice list

**GO for production**, with M16H §9's slice list unchanged and the following
scope statement:

| scope | promise | evidence |
|---|---|---|
| 1–2 speakers | automatic labels, count 2/2, DER 3.29 % (TTS) | gate passed on EN+ID |
| 3 speakers | labels shown as **provisional**; auto count must be confirmed; merge repair is first-class | rapid 3-way 2.11 %, but one trio and auto over-split on long turns |
| 4+ speakers | not offered | not measured |

The slice list (same as M16H §9, with slice-0 results attached):

| # | slice | scope | gate before the next slice |
|---|---|---|---|
| 0 | **EN+ID fixture gate** | **done**: three edge-tts fixtures, CAM++ pair, same scorer | **passed** for 2-speaker and rapid 3-way; long-turn Auto count is the named caveat |
| 1 | Schema v7 + types + store | `DiarizationRun`, `Speaker`, `SpeakerTurn`; additive `upgrade_to_v7`; replace/read/rename/colour/reassign/merge/split/delete | migration from populated v6 is additive and byte-preserving; split/merge then read mapping correct; transcript delete cascades; default export unchanged |
| 2 | Inference adapter (`suramak-asr`) | `to_16khz_mono`; model pins + Model Manager install; count selector; silence → empty; missing/corrupt → named errors; duration cap; progress; **no fake cancel** | missing model names the file; silence returns zero turns; 30-min file bounded; progress 100 %; **Auto threshold decision (0.5 vs 0.6) measured on real audio** |
| 3 | Persistence wiring | apply/active-run replacement, manual-name matching by overlap, re-run confirmation, delete labels | renamed speaker keeps name/colour on re-run; unmatched manual names survive; manual turn edit triggers confirmation; `segments` rows byte-identical |
| 4 | Editor UI | badges + 8 px lane + popover + count selector + states | keyboard-only rename/reassign; six slots pass contrast on both ladders; no badge without a run; **a 3-speaker over-split is repairable in ≤ 2 actions per turn; one segment may show two turns** |
| 5 | Speaker-labelled export | opt-in SRT/VTT prefix / `<v>`; straddling cue split; bilingual composition | labels never enter `segments.text`; default output byte-identical; split does not change stored segment |
| 6 | Verification and docs | real EN+ID meeting audio re-run, 5-file sets per variant, 1 h measurement on the owner's Mac, clean-machine dylib load (M16F), corrupt/resume download, diagnostics, licence notices | every M16.6 verify item named; package/licence surface updated; 3+ claim matches measured gate |

**What would flip this decision.** 2-speaker real-audio DER > 10 % or wrong
count on ≥ 2 of 5 files → **STOP** for the feature, as M16H §9 says. 3-speaker
real-audio DER > 20 % or wrong count on ≥ 2 of 5 → keep 2-speaker only and hide
the 3+ promise. A second voice trio collapsing in the same rapid-3-way shape →
demote 3 speakers to manual-only.

**Missing evidence, named.** Real meeting audio (owner device pass); four more
2-speaker and three more 3-speaker files to satisfy the 5-file gate; a second
and third voice trio; 4+ speakers; macOS/Windows and CoreML; 1-hour runtime and
RSS; transcript-alignment behaviour; spontaneous overlap. None of these blocks
slices 1–5, which is why the decision is GO rather than DEFER.

## 9. Verification of the untouched workspace

```text
git_head=7ddd11c
git_status=clean apart from this report (build/ is gitignored)
cargo fmt --all --check                                  = clean
cargo clippy --workspace --all-targets -- -D warnings    = clean
cargo test --workspace                                   = 1079 passed / 0 failed / 10 ignored (36 binaries), exit 0
```

The first cold-build test run had one flake in a pre-existing timing-sensitive
test, `suramak-asr::worker::tests::a_cancel_queued_with_the_job_is_observed_before_the_model_load`
(149 passed, 1 failed under the compile load); it passes alone and the clean
full re-run above is green with the baseline counts. No test was changed. The
UI snapshot tests rewrote two tracked PNGs during the run; both were restored
with `git checkout`, so the tree is clean apart from this report.

No production file, dependency, CI file or test was touched. The probe is the
same throwaway crate at `build/m16h-probe` (its own `[workspace]`, gitignored);
models remain in `/tmp/m16h-models`, fixtures and results in the gitignored
`build/m16h-probe/slice0/`.

## 10. Reproduction

```bash
# 0. model pins (unchanged; verify before running)
sha256sum /tmp/m16h-models/sherpa-onnx-pyannote-segmentation-3-0/model.onnx \
          /tmp/m16h-models/campplus.onnx
# expect 220ad67c... and aa3cfc16...

# 1. fixtures (edge-tts 7.3.1 + ffmpeg 8.0.1)
/home/ubuntu/.venvs/tts/bin/python build/m16h-probe/fixtures/edge_tts_fixtures.py
# writes build/m16h-probe/slice0/audio/{*.wav,manifest.json}

# 2. probe binary
cd build/m16h-probe && cargo build --release

# 3. gate matrix (9 cases x 3 runs) and score
./run_matrix_slice0.sh /tmp/m16h-models
python3 fixtures/score_slice0.py slice0/audio slice0/results

# 4. prove the metric is M16H-comparable
python3 fixtures/rescore_m16h_active.py
```

`run_matrix_slice0.sh` is the exact matrix behind §3: auto/2/4 on the
2-speaker file, auto/2/3/4 on the rapid 3-way, auto/3 on the long control. The
threshold diagnostics (§5) are four extra invocations with `--num-clusters 0`:
`--threshold 0.6` on `two_speakers_enid.wav` and `three_speakers_enid.wav`,
and `--threshold 0.6` / `0.7` on `three_long_enid.wav`.
