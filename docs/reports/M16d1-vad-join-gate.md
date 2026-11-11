# M16.4-S1 — VAD join-quality gate

**Decision: STOP for chunk-only capture.** The gate fails on the 60 s
speech+silence fixture with both whisper models, clean and room tone (WER delta
**39.0–45.1 pt** against a 1.0 pt budget, with 1–4 dropped phrases per run).
The join *rule* is not what fails — a fixed 30 s cut control moves the same
fixture by 27.5–32.4 pt, and the monolingual control still moves 8.0 pt (tiny)
and 33.3 pt (base) — the per-chunk decoder is. Per the workorder: **M7's cadence
stays, and production slices 2–5 are cancelled.**

The chunker itself is sound and is kept: deterministic 3/3, 512-sample feed,
cuts 84 ms after the last voiced frame and 908–3 916 ms before the next, no cut
inside speech. What it cannot do is make whisper's chunked decode equal to a
full pass.

| | |
|---|---|
| gate rows | 12 gated (6 fixtures × 2 models); **8 pass, 4 fail** |
| failures | `60s-clean` and `60s-room`, tiny and base — every code-switched row |
| monolingual 60 s control | tiny 8.00 pt, base 33.33 pt (1 dropped phrase, 1 duplicated phrase) |
| cut-position control | fixed 30 s cuts: tiny 32.39 pt, base 27.54 pt — same magnitude as VAD cuts |
| duplication | 0 duplicated phrases in every gated row; **1** in the base monolingual control (`"lazy dog"`, a segment straddling the frontier) |
| determinism | chunk spans identical 3/3 on the 60 s fixture |

---

## 1. What landed

| path | what |
|---|---|
| `crates/suramak-asr/src/vad.rs` | the pure chunker, the join rule, the 512 feed — `#[cfg(feature = "vad-chunking")]`, 16 unit tests, no model needed |
| `crates/suramak-asr/tests/vad_join.rs` | the join-quality harness (7 metric unit tests + 2 `#[ignore]`d model tests: boundary/determinism, and the gate matrix) |
| `crates/suramak-asr/Cargo.toml` | `vad-chunking = ["dep:sherpa-rs"]`, default **OFF**; reuses the existing `sherpa-rs 0.6.8` optional dependency (the M10A Parakeet pin, sherpa-onnx 1.12.9, Apache-2.0/MIT chain verified in M16DF). No new package. |
| `crates/suramak-asr/src/lib.rs` | the gated `pub mod vad;` |

No capture path, no service, no settings, no UI, no Android, no CI file was
touched. A default build does not compile the module at all, so every shipped
surface is byte-identical.

The model file is **not committed**: the host tests read
`SURAMAK_VAD_MODEL_PATH` and skip (or fail, with `SURAMAK_REQUIRE_VAD=1`) when
it is absent — the alignment test's model gate. On Linux the prebuilt
sherpa-onnx libraries need to be on the loader path, as `docs/ENGINES.md`
already documents for `parakeet`:

```sh
SURAMAK_VAD_MODEL_PATH=/tmp/silero_vad_v5.1.2.onnx \
SURAMAK_VAD_FIXTURES=/tmp/m16df \
LD_LIBRARY_PATH=target/release \
cargo test -p suramak-asr --release --features vad-chunking \
  --test vad_join -- --ignored --nocapture
```

## 2. The chunker

### 2.1 Invariants

| invariant | how it is held | test |
|---|---|---|
| exactly **512 samples per `accept_waveform` call** | `feed_schedule(total)` returns `(start, 512)` pairs; the tail block is zero-padded rather than shortened, because sherpa-onnx ORs every window inside one call and a short call is a different window | `the_feed_is_exactly_512_samples_per_call`, `the_padded_tail_is_zero_filled_…` |
| a cut **never lands inside speech** | a chunk closes only at a VAD segment end; `chunks_from_segments` cannot produce another cut point | `a_long_monologue_is_one_chunk_not_a_cut_through_speech` |
| the chunk **starts at its own first word − pre-roll** | silence between the previous cut and the pre-roll is never decoded | `every_chunk_starts_at_its_own_first_word_…` |
| **determinism** | the VAD is exact at 512 (M16DF 3/3), and the chunk policy is a pure function of the segment list | `chunking_is_deterministic_across_three_runs` + the live 3/3 run below |

Policy defaults (`VadConfig`): threshold 0.50, `min_silence` 0.30 s,
`min_speech` **0.10 s** (0.25 s drops the fixture's deliberate 0.300 s cue),
`max_speech` 30 s, buffer 31 s, `min_chunk` 10 s, soft `max_chunk` 14 s,
`pause` 700 ms, `pre_roll` **1 500 ms**.

The pre-roll was 500 ms first. The harness measured the difference on the
monolingual 60 s control: at 500 ms a chunk head lost *"the quick"* (tiny:
"Pick brown fox jumps…", 1 dropped phrase, 20.00 pt); at 1 500 ms the head is
clean (0 dropped phrases, 8.00 pt). 1 500 ms is also exactly M7's window
overlap, so the join rule is exercised the way M7 exercises it.

### 2.2 Measured on `m16df-60s-clean.wav`

15 VAD segments (the same count M16DF measured at `min_speech = 0.10`), four
chunks, **92.7 % of the file decoded** (the rest is silence the VAD resolved):

| chunk | audio | core starts | length |
|---|---|---|---|
| 0 | 0.000 – 11.604 s | 0.000 | 11.604 s |
| 1 | 13.592 – 26.196 s | 11.604 | 12.604 s |
| 2 | 25.624 – 41.076 s | 26.196 | 15.452 s |
| 3 | 43.640 – 59.572 s | 41.076 | 15.932 s |

Cuts: **84 ms after the last energy-gated voiced frame** (−42 dBFS, 100 ms
hangover, 32 ms frames) and **3 372 / 908 / 3 916 ms before the next** — the
workorder's margin is ≥ 90 ms, so every cut clears it by an order of magnitude.
The 84 ms (rather than M16DF's 116 ms) is the same property measured on the
32 ms frame grid; the cut is still inside the VAD's own end, never on a
phoneme.

Two chunks exceed the 14 s soft maximum (15.45 and 15.93 s). That is
structural: a cut may only land at a VAD segment end. From 25.6 s the candidate
segment ends are at 9.8 s (below the 10 s floor) and then at 15.45 s; from
43.6 s the 9.95 s candidate misses the floor by 50 ms and the next pause is the
end of the file. No cut was moved into speech to hit the target. Chunks decode
11.6–15.9 s, well inside whisper's 30 s window.

Determinism: three fresh VAD runs over the same PCM produced **identical chunk
spans** (asserted, not eyeballed).

## 3. The join rule

M7's skip-and-merge, unchanged in shape (`live.rs::MERGE_TOLERANCE_MS = 50`):

```text
for each chunk k (audio start_ms … cut_ms; core_start_ms = previous cut, 0 for k = 0):
    for each decoded segment s of chunk k:
        start = k.start_ms + s.start_ms
        end   = k.start_ms + s.end_ms
        if s.text is empty or end <= start:                                  drop
        if end <= k.core_start_ms + MERGE_TOLERANCE_MS:                      drop   # re-decode
        else: append with idx = session.segments.len()
```

The difference from M7 is what the tolerance protects: M7 drops a re-decoded
window head; here it drops the pre-roll of the next chunk. Because the pre-roll
is 1.5 s and a chunk normally closes on a pause ≥ 700 ms, adjacent chunks
overlap by up to 800 ms — real re-decoded audio, which the rule must drop.

**The rule leaks when a decoded segment straddles the frontier.** A segment
that begins in the pre-roll and ends after `core_start + 50` is kept whole, so
its head duplicates the previous chunk's tail. Measured: base on the
monolingual control emitted `"lazy dog"` twice (`duplicated_phrases: ["lazy
dog"]`). M7 tolerates this because the final pass replaces the provisional list
byte-for-byte; a chunk-only pipeline has no such backstop.

## 4. The harness

### 4.1 Fixtures

| fixture | bytes | sha256 | note |
|---|---|---|---|
| `fixtures/audio/en.wav` | 288 078 | `318bcc70…38a6` | committed, 9.000 s |
| `fixtures/audio/id.wav` | 381 006 | `f630e8a4…fbd1` | committed, 11.904 s |
| `m16df-60s-clean.wav` | 1 920 078 | `b443105d…37d4` | M16DF, regenerable |
| `m16df-60s-room.wav` | 1 920 078 | `e05e1de5…370b` | M16DF, −55 dBFS pink |
| `en-room.wav` | 288 078 | `8175a3528bd21c19106ec703c35afce2502449f4f4fa2ffc62d9f58b3ef3f5ec` | **new**: en.wav + the same pink-noise mix |
| `id-room.wav` | 381 006 | `c500743d9b88a0860663d6e3ba4d19538dd4c7770f845c81b640b04bae766533` | **new**: id.wav + the same mix |
| `m16df-60s-enonly.wav` | 1 920 078 | `edd503d5b6c97e799b67e9cc2d14d392341dca433eecbb94d2feec52078e0fe4` | **control**: the 60 s timeline with the ID cues muted |

The room variants of the two short fixtures are new, because the workorder asks
for clean **and** room tone per fixture; they use M16DF's exact recipe
(`anoisesrc=color=pink:amplitude=0.002:sample_rate=16000:seed=20261110`,
`amix … normalize=0:duration=first`). The enonly control is a diagnostic, not
part of the gate: it isolates join quality from whisper's code-switch failure.

```sh
# the two room variants
for n in en id; do
  D=$(ffprobe -v error -show_entries format=duration -of csv=p=0 fixtures/audio/$n.wav)
  ffmpeg -y -hide_banner -loglevel error -i fixtures/audio/$n.wav \
    -f lavfi -t "$D" -i "anoisesrc=color=pink:amplitude=0.002:sample_rate=16000:seed=20261110" \
    -filter_complex "[0:a][1:a]amix=inputs=2:normalize=0:duration=first[a]" \
    -map "[a]" -ar 16000 -ac 1 -c:a pcm_s16le -map_metadata -1 /tmp/m16df/$n-room.wav
done

# the monolingual control: mute the ID cue spans (M16DF cue table)
ffmpeg -y -hide_banner -loglevel error -i /tmp/m16df/m16df-60s-clean.wav \
  -af "volume=enable='between(t,7.0,11.504)+between(t,22.5,29.904)+between(t,38.0,41.904)+between(t,45.0,45.3)+between(t,56.0,59.5)':volume=0" \
  -ar 16000 -ac 1 -c:a pcm_s16le -map_metadata -1 /tmp/m16df/m16df-60s-enonly.wav
```

Models: `tiny` `ggml-tiny.bin` sha256 `be07e048…1b21` (the repo pin) and `base`
`ggml-base.bin` sha256 `60ed5bc3…2efe` (the M16DF cache), both with the
product's parameters (`whisper::transcribe`: greedy, 2 threads, token
timestamps, language pinned per fixture).

### 4.2 What a row measures

1. **Full pass** over the whole file — the authority.
2. **Chunked run**: VAD chunks, each decoded with the same parameters, joined
   by the rule above.
3. **Word alignment** (unit-cost Levenshtein, deterministic tie-breaks) between
   the two texts, from which:
   - **dropped phrase** = a maximal run of ≥ 2 reference words with no
     counterpart (single missing words are counted in `dropped_words` but are
     not a "phrase");
   - **duplicated phrase** = an inserted run of ≥ 2 words that also occurs in
     the reference (text re-decoded across the frontier); insertions that occur
     nowhere in the reference are reported as `extra` (hallucination);
   - **WER delta** = `WER(chunked, reference) − WER(full, reference)` where a
     reviewed reference exists (`fixtures/audio/manifest.tsv` for EN/ID); for
     the 60 s fixtures the full pass is the reference, so delta = WER(chunked,
     full).

Punctuation and case never count as edits, so a difference must be a word to
move the numbers.

### 4.3 Why language is pinned

The 60 s fixture is **code-switched** (5 EN cues, 4 ID cues). Whisper is not a
code-switching decoder. With `auto` the full pass detects Indonesian for the
whole file and the chunks flap per chunk; the gate would then measure language
detection, not joins. Every gate row therefore pins one language per session
(`en` for the EN fixtures and the mixed fixture, `id` for ID) — the most
favourable configuration for chunking, because the full pass and the chunks at
least agree on the language. `auto` is measured separately, as a diagnostic.

## 5. Results

### 5.1 The gate table

`PASS`/`FAIL` are the gate rows; `info` rows are controls and diagnostics.
Full run: `cargo test -p suramak-asr --release --features vad-chunking --test
vad_join -- --ignored --nocapture`, 486 s, models `tiny` and `base`, 2026-11-11.

| verdict | model | fixture | lang | chunks | audio | words full/chunk | dropped phrases (words) | duplicated | WER Δ |
|---|---|---|---|---|---|---|---|---|---|
| PASS | tiny | en | en | 1 | 8.2 s | 23 / 23 | 0 (0) | 0 | +0.00 |
| PASS | tiny | en-room | en | 1 | 8.2 s | 23 / 23 | 0 (0) | 0 | +0.00 |
| PASS | tiny | id | id | 1 | 11.1 s | 18 / 18 | 0 (0) | 0 | +0.00 |
| PASS | tiny | id-room | id | 1 | 11.1 s | 18 / 18 | 0 (0) | 0 | +0.00 |
| **FAIL** | tiny | 60s-clean | en | 4 | 55.6 s | 71 / 47 | **4 (24)** | 0 | **+45.07** |
| **FAIL** | tiny | 60s-room | en | 4 | 55.6 s | 77 / 59 | **3 (19)** | 0 | **+38.96** |
| PASS | base | en | en | 1 | 8.2 s | 23 / 23 | 0 (0) | 0 | +0.00 |
| PASS | base | en-room | en | 1 | 8.2 s | 23 / 23 | 0 (0) | 0 | +0.00 |
| PASS | base | id | id | 1 | 11.1 s | 17 / 17 | 0 (0) | 0 | +0.00 |
| PASS | base | id-room | id | 1 | 11.1 s | 17 / 17 | 0 (0) | 0 | +0.00 |
| **FAIL** | base | 60s-clean | en | 4 | 55.6 s | 69 / 49 | **1 (20)** | 0 | **+39.13** |
| **FAIL** | base | 60s-room | en | 4 | 55.6 s | 69 / 49 | **1 (20)** | 0 | **+40.58** |

The eight passes are single-chunk rows: both short fixtures are shorter than
the 10 s chunk floor, so the "chunked" run is one decode of (almost) the whole
file and the join is trivially exact. They prove the harness path and the
pre-roll trim; they are **not** join evidence. The join evidence is the 60 s
rows and the controls.

For context, against the reviewed manifest text the full pass itself scores
4.35 % (en) and 27.78 % (id) with tiny — the chunked run reproduces it word for
word, which is why the delta is exactly 0.00. Absolute accuracy is the model's;
the gate measures only what chunking changes.

### 5.2 Controls — what actually moved the text

| control | model | chunks | dropped (words) | duplicated | WER Δ | reading |
|---|---|---|---|---|---|---|
| 60s-clean, **fixed 30 s cuts, no VAD** | tiny | 2 | 1 (3) | 0 | +32.39 | the cut positions are not the cause: non-VAD cuts move it as far |
| 60s-clean, fixed 30 s cuts, no VAD | base | 2 | 0 (2) | 0 | +27.54 | same |
| **60s-enonly** (monolingual) | tiny | 3 | 0 (1) | 0 | +8.00 | join clean; residual is decoder variance |
| **60s-enonly** (monolingual) | base | 3 | 1 (8) | **1** (`lazy dog`) | +33.33 | the frontier leak + a repetition loop |
| 60s-clean, `auto` | tiny | 4 | 4 (32) | 0 | +63.64 | per-chunk language flapping |
| 60s-clean, `auto` | base | 4 | 0 (0) | 0 | +142.42 | the full pass itself collapses (33 words) |
| id, 4 s chunk floor (2 chunks) | tiny / base | 2 | 0 | 0 | +0.00 | a real 2-chunk join on monolingual audio is exact |
| id-room, 4 s chunk floor | tiny / base | 2 | 0 | 0 | +5.56 / +0.00 | one word |

### 5.3 The failure, in whisper's own output

`tiny`, 60 s clean, `en`, per chunk (from the harness dump, shipped config):

```text
chunk 0 [ 0.0–11.6 s]  The quick brown fox jumps over the lazy dog.
chunk 1 [13.6–26.2 s]  Permatt keeps every recording on this machine and never sends audio to the cloud.
chunk 2 [25.6–41.1 s]  and buy. Trim a casi su d'hamin choba.
chunk 3 [43.6–59.6 s]  The quick brown fox jumps over the lazy dog. Sermat keeps every recording on this machine.
```

Chunk 0 also contains the Indonesian cue at 7.0–11.5 s and chunk 1 the head
of the Indonesian cue at 22.5 s: **neither produced a single word** for them.
The full pass over the same audio with the same language pin keeps those spans
as pseudo-Indonesian ("Sir. Salama Pagi. Namasaya Surama. Applicasi. …"), badly
but present. Chunking did not move the words — it deleted them, because a
12 s window of English context gives the decoder no reason to emit Indonesian.

`base` is the same story with different noise: its chunk 2 [25.6–41.1 s] —
about 15 s of Indonesian speech — came back as `"Thank you for watching."`

`base`, monolingual control, per chunk:

```text
chunk 1 [31.7–50.4 s]  Pick brown fox jumps over the lazy dog.  (×3 — a repetition loop)
chunk 2 [49.9–53.6 s]  dog. CERAMAT keeps every recording on this machine.
```

The `dog.` at the head of chunk 2 is the previous chunk's tail re-decoded; it
ends past `core_start + 50 ms`, so the join keeps it — the duplication in the
table. The repetition loop is the decoder, not the cut: that chunk is 1.5 s of
speech, 15 s of silence, 2.2 s of speech.

### 5.4 What the numbers mean

- **The VAD cut positions are not the problem.** Fixed 30 s cuts (no VAD at
  all) move the same fixture by 27.5–32.4 pt. Any chunking of this audio does
  this.
- **Code-switching is fatal under one pinned language**, and `auto` is worse
  (63.6 / 142.4 pt, because the full pass and the chunks disagree about the
  language). A chunk-only pipeline has no way to be right on this fixture.
- **Monolingual audio is not clean either.** tiny reaches 8.0 pt with no
  dropped phrase; base reaches 33.3 pt, drops an 8-word phrase and duplicates
  two words at a frontier. The residual is decoder-level (chunk-head
  under-confidence, repetition loops on long silences), not a policy constant
  that can be tuned.
- **Duplication is real but rare**: zero in every gated row, one in the base
  monolingual control. The join rule's leak is the straddling-segment case
  documented in §3.

## 6. Decision

**STOP for chunk-only capture.**

The workorder's rule is explicit: a gate failure makes the decision STOP, keeps
M7's cadence, and cancels production slices 2–5. That is the outcome recorded
here, for these reasons in order:

1. Every code-switched gate row fails by 39–45 pt against a 1.0 pt budget. The
   M16DF fixture that was built to stress speech+silence is exactly the case a
   chunk-only pipeline cannot transcribe.
2. The failure is not fixable by moving the cut. The cut margins are held (84 ms
   after the last voiced frame, ≥ 908 ms before the next) and a non-VAD control
   reproduces the damage.
3. Chunk-only changes the transcript even on clean monolingual audio (tiny 8.0
   pt, base 33.3 pt, one duplicated phrase). A transcript that differs from the
   full pass is a different product artefact; the gate exists to prevent
   shipping it silently.

What this does **not** say: that VAD chunking is useless. M16DF's *preferred*
outcome — VAD chunks replacing M7's fixed cadence for **provisional partials**,
with the final Stop pass remaining the only transcript authority — is untouched
by this gate, because that design keeps the final pass and therefore has M7's
safety net. Nothing in this pass authorises removing that pass.

What would change the decision: a streaming-capable decoder (Parakeet streaming
or a whisper.cpp streaming build) whose partials are conditioned on the session
context, not on a 12 s window; or a code-switch-capable model; or a
per-chunk language decision that is stable across the session. None is in the
graph today.

## 7. Verification

| check | command | result |
|---|---|---|
| formatting | `cargo fmt --all --check` | exit 0 |
| lints (workspace) | `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| lints (feature crate) | `cargo clippy -p suramak-asr --all-targets --features vad-chunking -- -D warnings` | exit 0 |
| unit tests (chunker) | `cargo test -p suramak-asr --features vad-chunking --lib vad::` | 16 passed |
| metric tests | `cargo test -p suramak-asr --features vad-chunking --test vad_join` | 7 passed, 2 ignored |
| boundary/determinism | `… --test vad_join chunk_boundaries -- --ignored --nocapture` | pass, 3/3 identical |
| baseline (current main, before any edit) | `cargo test --workspace --no-fail-fast` | **1 079 passed, 0 failed, 10 ignored** (35 targets) |
| final workspace run | `cargo test --workspace --no-fail-fast` | see §7.1 |
| gate | `… --test vad_join vad_join_gate -- --ignored --nocapture` | **FAIL** on 4 of 12 gated rows (this is the finding, not a defect in the harness) |

The gate test exits non-zero by design: its assertion *is* the decision. It is
`#[ignore]`d and feature-gated, so neither the workspace run nor CI executes it.

### 7.1 Final workspace run

`cargo test --workspace --no-fail-fast`, default features, after all edits:

```text
36 targets, 1 079 passed, 0 failed, 10 ignored   (EXIT 0)
```

Identical to the baseline taken before the first edit (35 targets, 1 079 passed,
0 failed, 10 ignored). The one extra target is the new `tests/vad_join.rs`,
which `#![cfg(feature = "vad-chunking")]` compiles to **zero tests** without
the feature — the default build is untouched, which is also the evidence that
the feature gate is real.

Tree: `git status --porcelain` shows only this pass's files —
`M crates/suramak-asr/Cargo.toml`, `M crates/suramak-asr/src/lib.rs`,
`?? crates/suramak-asr/src/vad.rs`, `?? crates/suramak-asr/tests/vad_join.rs`,
`?? docs/reports/M16d1-vad-join-gate.md`. The two `tests/ui-compact/shots/*.png`
files that the UI snapshot tests rewrite on every run were restored. Nothing was
committed or pushed.

## 8. Reproduction and cleanup

- Gate command: §1, plus `SURAMAK_VAD_ONLY=<labels>` to narrow a run,
  `SURAMAK_VAD_MODELS=id=path,…` to choose models, `SURAMAK_VAD_DUMP=1` to print
  full/chunk/joined texts, `SURAMAK_VAD_PRE_ROLL_MS=` for the pre-roll study.
- The harness writes `target/vad-join-gate.json` (config + every row) for the
  report's tables.
- Fixtures and models live outside the repo (`/tmp/m16df`, `/tmp/silero_vad_v5.1.2.onnx`,
  `~/.cache/m16df-models`); nothing was added to git.
- No `git commit`, no `push`, no Android or CI file, no capture-path change.
