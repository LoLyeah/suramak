# M26C — 15-minute macro-windows: measured, gated, **STOPPED**

## Decision

**STOP. The macro path is not enabled, and the duration cap stays where it is.**

Three of the work order's four gates are missed: **|ΔWER|**, **segment-count
stability**, and the **absolute** memory ceiling. The failure is not a crash
and not a repetition loop — the mechanism works and the memory is flat — but
the quality criteria and the absolute ceiling are missed, and the work order
makes that a STOP.

| # | Gate (work order) | Limit | Measured | Result |
|---|---|---|---|---|
| 1 | Natural corpus, \|ΔWER\| vs single-pass | ≤ 0.5 pt | **−0.62 pt** (single 7.89, macro 7.27) | **FAIL** |
| 2 | Natural corpus, segment count | within 2 % | **−7.12 %** (702 → 652) | **FAIL** |
| 3 | Repeated 167 s clip, 4-gram loops | none > 4 consecutive | **0** (max run 1) | **PASS** |
| 4 | 90-minute tiny peak | ≤ 450 MB and ±15 MB of 30 min | **552.8 MB**, −0.6 MB vs 30 min | **FAIL** on the ceiling; flatness PASS |
| (5) | ≤ 15-minute file byte-identical (preserved-path check) | exact digest | **`bd62aedf6f932c46`** before and after, 84 segments | **PASS** |

What "stays" means concretely:

* `macro_pipeline::MACRO_WINDOWS_ENABLED` is **`false`**; production takes the
  single `whisper_full` call for every duration, exactly as before this slice.
* `Limits::max_minutes` **is not changed**: it remains the M26A Phase-1
  memory-derived cap (77 min on a 4 GB phone/tiny, 431 min on a 12 GB
  phone/tiny, 8 h ceiling on a 16 GB Mac; 40 min for `turbo-q5-0` on this
  2-core box at its live 1,459 MB budget). The Phase-2 "disk-only" cap and the
  `M_peak(15 min)` model-fit gate are **not** shipped, because the path that
  would make them safe is not shipped.
* The implementation, its unit tests and the measurement harness stay in the
  tree so a later slice can retry without rebuilding the mechanism.

The one production-neutral piece that remains is `PcmSlice::advise_dontneed`
(an unused public helper unless the macro path is enabled) — it changes no
transcript and no cap.

## 1. What was built

| Piece | Where |
|---|---|
| Cut selection (C1/C2), prompt firewall, seam stitch — pure, deterministic, 10 unit tests | `crates/suramak-asr/src/macro_chunk.rs` |
| The decode driver: window plan, one reused `WhisperState`, `madvise(MADV_DONTNEED)` per window, prompt carry, seam application, progress by samples, cancellation | `crates/suramak-asr/src/macro_pipeline.rs` |
| `WindowSession` (one state for the whole job) + `TextToken` (ids/probabilities for the firewall); the single-call `transcribe` params are untouched | `crates/suramak-asr/src/whisper.rs` |
| `PcmSlice::advise_dontneed(start, end)` — page-aligned `MADV_DONTNEED`, clamped, a hint that never fails a job | `crates/suramak-core/src/pcm.rs` |
| Wiring: `if macro_pipeline::applies(len)` → macro, else the unchanged single call (now guarded by the disabled flag) | `crates/suramak-asr/src/pipeline.rs` |
| Acceptance harness: natural WER/segments, stress loops, 30/90-minute VmHWM children | `crates/suramak-asr/tests/m26c_macro_windows.rs` |

The rules are the work order's, as constants in one `MacroConfig`:
window target 900 s, search `[cursor+840 s, cursor+930 s]`, 20 ms RMS frames,
threshold `max(1e−4, percentile15(E) × 1.8)`, C1 = midpoint of the longest
silent run ≥ 400 ms with 1.5 s overlap, C2 = minimum-energy 200 ms in
`[cursor+870 s, cursor+915 s]` with 4.0 s overlap and `cut_without_pause`.
Decoder guards on the window call only: `no_context = true` + explicit
`set_tokens`, `temperature_inc = 0.2`, `entropy_thold = 2.4`,
`logprob_thold = −1.0`, `no_speech_thold = 0.6`, `max_tokens = 0`,
`n_max_text_ctx = 224`, token timestamps on.

The prompt firewall carries at most 160 tokens and requires **all** of:
unique byte-4-gram ratio > 0.45, mean token probability ≥ 0.45, and no 1–6 word
phrase emitted ≥ 3 times. "Repeated" is operationalised as *consecutive*
repetition — what a decoder loop looks like (`"Thank you. Thank you. Thank
you."`); counting non-consecutive occurrences would reject every natural
English tail because `"the"` appears more than three times in 160 tokens. This
is the one interpretive liberty in the firewall, and it is stated here rather
than hidden: on the corpus it passed all four cuts and carried 160 tokens each
time.

## 2. The natural corpus (sources and checksums)

Built from [LibriSpeech](https://www.openslr.org/12) `dev-clean`
(OpenSLR SLR12, CC BY 4.0), **eight chapters from eight distinct speakers**,
concatenated in this order with ffmpeg at 16 kHz mono PCM16 (each chapter is a
continuous read passage; no clip is repeated):

| # | Speaker / chapter | Minutes | Utterances | sha256 of the 16 kHz mono chapter WAV |
|---|---|---|---|---|
| 1 | 1919 / 142785 | 8.17 | 64 | `fe47577864a80bc6c3f65ae549dd533160e5a785eaad30427b3c4bf5bffc7d49` |
| 2 | 777 / 126732 | 8.11 | 82 | `9959d4eff8dd4c7c29bf333c5409b9a01371bb9429971354b5fe3bff0b9d98f6` |
| 3 | 3853 / 163249 | 8.05 | 57 | `7cd1231a981af8d13b11dab9095b2e4311291302e4cd977896c82b6382b0cd73` |
| 4 | 3170 / 137482 | 8.04 | 49 | `6b6e3b3306f10a2dd96e5695632d7c02e7938cf88684c25c2e40cca1c161dfbf` |
| 5 | 3000 / 15664 | 8.03 | 47 | `4c86b7553fe3af0840c063b4d54196feb27fcbea0401d2b944c46b2f55feb7a0` |
| 6 | 3081 / 166546 | 8.00 | 90 | `ccab580f0525c3c27d31f0b076f4d7333c7a6526d33eef92f5a50f2fd46a1340` |
| 7 | 3576 / 138058 | 7.99 | 41 | `6e535f516708bf82ab46d62aef3220d755dbaddf359557461d877b20bc4da51d` |
| 8 | 422 / 122949 | 7.98 | 36 | `c5b6195bb8ce20ea8eb0a0d9aafef1fe4d83ed667f31b8310a80572cbcc8c304` |

| Artifact | Value |
|---|---|
| LibriSpeech `dev-clean.tar.gz` sha256 | `76f87d090650617fca0cac8f88b9416e0ebf80350acb97b343a85fa903728ab3` |
| Concatenated corpus `natural-64min.wav` | **3888.00 s = 64.80 min**, 124,415,924 bytes, sha256 `2c9986a7a2f04ac2468bd6e066a5801bc77855aeeffe0009842f7e79475791b0` |
| Reference text | the chapters' `.trans.txt`, concatenated in the same order: **10,634 words** |
| Model | `ggml-tiny.bin`, sha256 `be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21` |

Reference and hypotheses are normalised identically (lowercase, apostrophes
kept inside words, every other character a separator) before WER; the Rust
harness and an independent Python/difflib pass agree on both numbers.

## 3. WER

`SURAMAK_REQUIRE_MODEL=1 … --ignored --exact natural_corpus_holds_wer_and_segment_count`
(tiny, CPU, 2 threads, release).

| Run | Segments | WER | Words in hypothesis |
|---|---:|---:|---:|
| Single call, 64.80 min | 702 | **7.8898 %** | 10,644 |
| Macro windows, 64.80 min | 652 | **7.2691 %** | 10,673 |
| Δ (macro − single) | −7.12 % | **−0.6207 pt** | +29 |

The macro path is *better* on this corpus, not worse — and that is still a
gate failure, because the limit is symmetric (`|ΔWER| ≤ 0.5 pt`). The
difference is spread across the file, not concentrated at cuts: of the 361
word positions where the two transcripts disagree, 342 (94.7 %) are more than
±1 % of the file away from any cut; 19 are within ±0.5 % and the other 6 are
between ±0.5 % and ±1 %. The per-decile WERs (macro vs single, in %) are
`7.3/11.3, 11.9/13.3, 8.0/8.3, 9.7/8.6, 4.1/4.6, 6.5/6.0, 5.2/5.8, 8.1/8.9, 6.6/7.1, 5.2/4.9`
— ordinary decode variance in both directions, not seam damage.

The independent Python check: 839 vs 773 word errors over 10,634 reference
words = 7.890 % vs 7.269 %, Δ = −0.621 pt. A 20-minute slice showed the same
structural behaviour with the opposite sign on segmentation: 304 macro
segments vs 281 single (+8.2 %).

No segment collapsed into a repeated phrase: the longest consecutive run of
any 4-gram in the 64.8-minute macro transcript is **1** (the single pass is
also 1).

## 4. Repetition loops

Stress fixture: the corpus's first **167.47 s** repeated six times with 0.8 s
of silence between repeats (the M25A/M16H stress shape), 16.81 min,
sha256 `bd9ea915d8bf5a940a7b8c8f76330e6a168f95ddfc22b9adc46ab3096ee6d834`.

| Metric | Value |
|---|---|
| Windows | 2 (cut at 841,230 ms, C1 silence, 1.5 s overlap) |
| Segments | 276 |
| Longest consecutive run of one 4-gram | **1** (no repetition at all) |
| Result | **PASS** — zero loops |

The firewall carried 160 tokens at the cut; the stress text is repetitive by
construction and the carry still passed the checks (its byte-4-gram ratio and
phrase-repetition test are computed on the *tail*, and the repeated clip's tail
within a window is ordinary prose). No loop survived.

## 5. Memory

Each measurement is a fresh child process of the test binary, tiny model, CPU,
2 threads; it decodes the fixture to a `.pcm32` and runs the macro path, then
prints `VmHWM` from `/proc/self/status`.

| Fixture | Windows | Peak VmHWM | |
|---|---:|---:|---|
| 30-minute natural | 2 | 527.8 MiB | 553.4 MB |
| 90-minute natural | 7 | 527.2 MiB | 552.8 MB |
| Difference | | **−0.6 MiB / −0.6 MB** | flat |

The **flatness** target (±15 MB) passes decisively: duration has stopped
mattering, and the `MADV_DONTNEED` per window is doing its job (without it the
90-minute run would carry four more 57.6 MB windows). The **absolute** ceiling
does not: 552.8 MB > 450 MB. The 450 MB figure in the design is
`308.4 MB (tiny base) + 28.8 MB mel + 57.6 MB window`; this build/host does not
reproduce that base — the M26B report's own 30-minute single-pass measurement
on this box was 578 MiB *including* 115 MB of mapped PCM and ~58 MB of mel, and
the macro path's saving over it is only the difference between the whole-file
and per-window mel/PCM. The macro path adds just the active window and its mel;
the ceiling is base-dominated. It is recorded as a failure rather than
redefined, and the gate that matters for duration — flatness — passed at
−0.6 MB.

For reference, the 5-minute single-pass run peaks at 351.7 MiB
(360,172 KB) in the CLI — the same base plus 19 MB of PCM and 9.6 MB of mel.

## 6. Cut positions chosen on the corpus

| Window | Start (ms) | End (ms) | Cut (ms) | Overlap | Rule | `cut_without_pause` | Prompt carried |
|---:|---:|---:|---:|---:|---|---|---:|
| 0 | 0 | 891,220 | 891,220 | 1.5 s | C1 silence | false | 160 tokens |
| 1 | 889,720 | 1,744,760 | 1,744,760 | 1.5 s | C1 silence | false | 160 tokens |
| 2 | 1,743,260 | 2,647,600 | 2,647,600 | 1.5 s | C1 silence | false | 160 tokens |
| 3 | 2,646,100 | 3,553,110 | 3,553,110 | 1.5 s | C1 silence | false | 160 tokens |
| 4 | 3,551,610 | 3,887,995 | — | — | final | — | — |

Planned before decode by `plan_windows` and identical to the executed cuts.
All four boundaries landed in a real pause (C1), none needed C2. The prompt
firewall accepted every tail (no `LowDiversity`, `LowProbability` or
`Repetition` rejection).

## 7. Why it failed, and what a future slice must change

* **Segment counts move by 7–8 %, deterministically.** Each window restarts
  whisper's 30 s processing grid and its rolling text context; sentence
  boundaries near a chunk edge fall differently, so a transcript with the same
  words gets a different number of segments. The 2 % criterion is not met by
  independent windows with these lengths. Fixing it means either preserving
  the 30 s grid across cuts (which the "cut in the middle of a pause" rule
  precludes) or merging segments at seams — a new design, not a parameter.
* **WER moves by 0.62 pt, in the better direction.** The windows decode
  slightly differently from the single call everywhere (94.7 % of the
  disagreement is away from cuts). The gate is symmetric, so this is a failure.
  A future attempt should either widen the accepted band with the owner's
  sign-off or make the windows reproduce the single call more closely.
* **The absolute memory ceiling is host/base-bound.** Flatness is achieved;
  553 MB is the tiny base on this build plus one window. If 450 MB is a real
  product requirement, the base has to come down (smaller context/threads or a
  different runtime), not the window.

## 8. Reproduction

```bash
# corpus (LibriSpeech dev-clean, 8 chapters, 64.80 min) — see §2 for hashes
# the fixture builders are ffmpeg + the wave module; the exact commands are in
# the run log of this slice. Then:
cd /home/ubuntu/suramak
TMPDIR=/path/with/space \
M26C_CORPUS=/path/natural-64min.wav \
M26C_REFERENCE=/path/natural-64min.txt \
SURAMAK_REQUIRE_MODEL=1 \
cargo test -p suramak-asr --release --test m26c_macro_windows -- \
  --ignored --exact natural_corpus_holds_wer_and_segment_count --nocapture

# stress (expects a > 15 min file of the 167.47 s clip repeated)
M26C_STRESS=/path/repeat-167s-x6.wav … --exact repeated_clip_has_no_repetition_loop

# memory (30- and 90-minute fixtures)
M26C_RSS_30=/path/natural-30min.wav M26C_RSS_90=/path/natural-90min.wav \
  … --exact rss_90_minutes_is_flat
```

The 60–90 minute fixtures are 124–173 MB each and are deleted after the run;
they are reproducible from the sources above. The natural test **fails by
design** (it encodes the gate that stopped the slice) and is `#[ignore]`d with
that reason in its attribute.

## 9. Build gates

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| NDK r27d, `arm64-v8a`, API 29: `cargo ndk … clippy -p suramak-android --all-targets -- -D warnings` | clean |
| same for `-p suramak-android-spike` | clean |
| Full workspace test suite | **1532 passed, 0 failed, 22 ignored** (48 binaries; baseline 1522 + 10 `macro_chunk` unit tests; ignored 17 + the 5 M26C harness tests) |
| 5-minute byte-identical | digest `bd62aedf6f932c46` / 84 segments, pre-slice and post-slice binaries |

The five M26C tests are `#[ignore]`d like the rest of the whisper e2e suite
(they need the tiny model and 60–90 minutes of audio); the natural-corpus one
is ignored **because it fails**, and its attribute says so.
