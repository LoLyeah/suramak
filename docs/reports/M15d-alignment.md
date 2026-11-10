# M15-D — word alignment feasibility → playback UI (M15.6)

Tree: `main` at `304596c` + this slice (uncommitted). Baseline suite: 779 passed.
This run: **807 passed, 0 failed, 10 ignored** (`RUST_TEST_THREADS=1 cargo test
--workspace`, log `build/m15d/workspace-tests.log`).

**Decision: GO.** The pinned whisper-rs binding exposes per-token start/end
spans through the public API the crate already calls, the mapping to words is
byte-exact and refuses anything it cannot reproduce, and the measured error on
the reviewed fixtures is 11 ms median (EN) / 112 ms median (ID) for word starts.
Phase 2 was therefore implemented, not deferred. Two words of caution belong in
the same sentence as the decision: this is **estimated** alignment at whisper's
own 10 ms granularity with a proportional-interpolation fallback inside long
segments (one ID word was 361 ms early), and it only exists for text the engine
itself produced — an edited segment has none until it is re-transcribed.

---

## 1. Phase 1 — what the shipped binding actually exposes

### 1.1 The timing surface (whisper-rs 0.16.0 / whisper-rs-sys 0.15.0)

Read from the exact pinned sources in `~/.cargo/registry/src/index.crates.io-*/`:

| Fact | Citation |
|---|---|
| `WhisperTokenData` is the raw C struct | `whisper-rs-0.16.0/src/lib.rs:45` — `pub type WhisperTokenData = whisper_rs_sys::whisper_token_data;` |
| The struct carries `id`, `tid`, `p`, `plog`, `pt`, `ptsum`, `t0`, `t1`, `t_dtw`, `vlen` | `whisper-rs-sys-0.15.0/src/bindings.rs:4928-4939` |
| The header documents the units and the precondition | `whisper-rs-sys-0.15.0/whisper.cpp/include/whisper.h:131-150` — `int64_t t0; // start time of the token`, `t1`, and *"do not use if you haven't computed token-level timestamps"* |
| Per-token data is reachable from safe Rust | `whisper-rs-0.16.0/src/whisper_state/token.rs:42-50` — `WhisperToken::token_data()` → `whisper_full_get_token_data_from_state` |
| …plus token id, probability and **raw bytes** | `.../token.rs:25-33` (`token_id`), `59-67` (`token_probability`), `95-97` (`to_bytes`) |
| Segment token access | `.../whisper_state/segment.rs:72-74` (`n_tokens`), `166-170` (`get_token`, bounds-checked) |
| The switch that makes whisper.cpp compute the spans | `whisper-rs-0.16.0/src/whisper_params.rs:189-191` — `set_token_timestamps` |
| The anchor thresholds (defaults 0.01) | `whisper_params.rs:198-208` (`set_thold_pt`, `set_thold_ptsum`) |
| The C accessor behind `token_data()` | `whisper.cpp/src/whisper.cpp:8041-8043` — returns `state->result_all[i_segment].tokens[i_token]` verbatim |
| Spans are computed **after** decoding, per segment | `whisper.cpp:7649-7652` (and the second call site at `7694-7697`) — guarded by `if (params.token_timestamps)` |
| How the numbers are produced | `whisper.cpp:8420-8530` — `whisper_exp_compute_token_level_timestamps` |

The last row is the honest core. whisper.cpp does **not** run a forced aligner:
it snaps a token to `t_beg + 2·(tid − token_beg)` centiseconds when that token
carried a forced timestamp token above both probability thresholds
(`whisper.cpp:8473-8483`), fixes the segment ends (`8486-8488`), and then
**fills every remaining gap proportionally to `vlen` (the token's voice
length)** (`8492-8525`). So:

- The spans are a *heuristic estimate*, 10 ms quantised, anchored on timestamp
  tokens when the decoder emitted them and interpolated when it did not.
- In a long segment with few timestamp tokens, the interpolation is what you
  get. That is exactly where the measured ID error comes from (§3.3).
- Nothing in the binding reports *which* tokens were anchors, so a caller
  cannot tell an anchored word from an interpolated one. The product therefore
  never claims better than "estimated".

**What the crate already had:** `transcribe()` has called
`params.set_token_timestamps(true)` since M2 (`whisper.rs:664`), so the spans
were already being computed on every run — this slice only *reads* them. No
inference parameter changed, no threshold was touched, and the text output is
byte-identical to before.

### 1.2 The Parakeet side

Parakeet **does** expose token timing, and it is a different shape:
`ParakeetResult { tokens, timestamps_s }` — one timestamp **point** per
subword token (`crates/suramak-asr/src/parakeet.rs:61-76`), consumed by
`segments_from_result` to split at pauses (`parakeet.rs:88-170`). It is not a
`(t0, t1)` span per token, and this slice does **not** wire it: the engine is
behind a default-OFF feature with a 670 MB model that is not present on this
VPS, so any number published for it would be unmeasured. It is named as the
next increment in §7, not claimed.

### 1.3 Token → word mapping (implemented, `crates/suramak-asr/src/words.rs`)

`words_for_segment(segment_idx, text, start_ms, end_ms, tokens)`
(`words.rs:71-138`):

1. Token **bytes** are concatenated (never per-token `&str`: whisper can split a
   UTF-8 character across tokens, and Indonesian hits that), recording each
   token's byte range and span.
2. The concatenation is trimmed of ASCII whitespace and compared with the
   segment text **byte for byte**. Any difference → **no words for that
   segment**. This is the no-fake-alignment rule: an edited, truncated or
   lossily-decoded segment cannot be word-timed by guessing where words "would"
   be.
3. Words are ASCII-whitespace runs (a space byte never appears inside a
   multi-byte UTF-8 sequence, so this cannot split a character). Punctuation
   stays attached to its word.
4. A word's start is the start of the **first token touching it**, its end the
   end of the **last**; times are clamped into the segment span and word starts
   never go backwards. A negative token time (`-1` = "unknown" in whisper.cpp)
   refuses the whole segment rather than substituting a boundary.

Eleven unit tests pin this, including a token split inside `é`
(`C2 A9` across two tokens), a whole "word" that is one token, several words
inside one token, punctuation, tabs/newlines, and the three refusal cases.

---

## 2. Phase-1 measurement setup

### 2.1 Fixtures (generated once, hashes pinned)

`fixtures/align/` — synthesized **one word at a time** with ffmpeg 8.0.1's
`flite` source (voice `slt`), each clip trimmed to its first/last sample above
−40 dBFS and joined with exactly 300 ms of silence (250 ms lead-in, 400 ms
tail). Word boundaries are therefore sample-accurate by construction — no
annotation, and no forced aligner that could share whisper's bias.

```sh
ffmpeg -hide_banner -loglevel error -y \
  -f lavfi -i "flite=text='<word>':voice=slt" \
  -ar 16000 -ac 1 -c:a pcm_s16le <word>.wav
# trim to |sample| > 0.01 (-40 dBFS), concatenate s16le frames with 300 ms of
# zeros between words, write the wav; the timeline is written next to it.
```

| file | contents | duration | sha256 |
|---|---|---|---|
| `en-words.wav` | the quick brown fox jumps over the lazy dog | 6.315 s | `8dadc603ef8b247c9e1202f6e51dde5a66f0cbfaef977de44396e9ff999a4395` |
| `id-words.wav` | selamat pagi nama saya suramak aplikasi lokal | 6.254 s | `0f9cd7708e07a9b356b86756db800ad0c419040237e26de1c60ad20d98b013d5` |

`fixtures/align/SHA256SUMS` pins all four files (`sha256sum -c` verified);
`fixtures/align/README.md` carries the commands, the construction rule and the
same table. These are measurement fixtures, not product audio; only
`crates/suramak-asr/tests/alignment.rs` reads them.

### 2.2 The run

```sh
SURAMAK_REQUIRE_MODEL=1 cargo test -p suramak-asr --release --test alignment \
  -- --ignored --nocapture
```

Model: the pinned tiny `ggml-tiny.bin`
(`be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21`,
`~/.local/share/suramak/models/`), CPU engine, 2 threads, release build. The
same test also runs the natural fixtures (`fixtures/audio/en.wav`,
`id.wav`) for invariants only (§3.4).

---

## 3. Measured error

Reference = the construction timeline. Error = estimate − reference, in ms.
"Within" = share of absolute start errors at or below the bound.

### 3.1 English (9 words, isolated by 300 ms silences)

| word | ref start | est start | Δ start | Δ end |
|---|---|---|---|---|
| the | 250 | 250 | +0 | −36 |
| quick | 686 | 710 | +24 | −110 |
| brown | 1350 | 1380 | +30 | −22 |
| fox | 2102 | 2140 | +38 | −7 |
| jumps | 2717 | 2710 | −7 | −29 |
| over | 3549 | 3550 | +1 | −13 |
| the | 4163 | 4160 | −3 | −29 |
| lazy | 4599 | 4610 | +11 | −15 |
| dog | 5435 | 5480 | +45 | −76 |

start: **median 11 ms**, p90 45 ms, max 45 ms, mean 17.7 ms, **100 % within
100 ms**; signed mean +15.4 ms (the estimate runs marginally late).
end: median 29 ms, p90 110 ms, max 110 ms, 89 % within 100 ms.
Lexical mismatches: 0 of 9.

### 3.2 Indonesian (7 words, same construction)

| word | ref start | est start | Δ start | Δ end |
|---|---|---|---|---|
| selamat | 250 | 270 | +20 | −61 |
| pagi | 1211 | 850 | **−361** | −40 |
| nama | 1930 | 1590 | **−340** | −38 |
| saya | 2618 | 2280 | **−338** | −12 |
| suramak | 3382 | 3270 | −112 | +369 |
| aplikasi | 4091 | 4160 | +69 | −23 |
| lokal | 5303 | 5300 | −3 | +145 |

start: **median 112 ms**, p90 361 ms, max 361 ms, mean 177.6 ms, 43 % within
100 ms / 57 % within 250 ms / **100 % within 500 ms**; signed mean −152.1 ms
(the estimate runs early). end: median 40 ms, p90 369 ms.
Lexical mismatches: 3 of 7 (the tiny model mis-hears `nama` → "nom",
`suramak` → "Srimag", `lokal` → "Local"; timing is compared positionally, so
recognition quality does not move these numbers).

### 3.3 Reading the numbers

- **The EN case is the good case, and it is not the typical case.** Every word
  is separated by real silence, so whisper's forced timestamp tokens land on
  those silences and the spans are near-exact. This is an *upper bound*.
- **The ID case shows the interpolation.** The 6.2 s clip became a **single
  segment**, so the anchors are at the segment ends only and the interior is
  filled by `vlen` proportions (`whisper.cpp:8492-8525`). The three ~340-360 ms
  early starts are the lag of that proportional fill; the word *order* and
  *containment* are still right, which is why the feature is safe to ship as an
  estimate with a segment fallback.
- **Word ends are noisier than starts** (EN p90 110 ms vs 45 ms): a word's end
  is the end of the last token touching it, and tokens frequently span a
  following pause.
- **Practical shape:** word-level karaoke is comfortable in EN and usable in ID
  for coarse highlighting; clicking a word can be up to ~0.4 s off in a long
  single-segment utterance. The UI copy says "estimated", and nothing in the
  product (exports, seek maths) depends on sub-100 ms precision.

### 3.4 Natural fixtures (no ground truth, invariants only)

`fixtures/audio/en.wav`: 2 segments, 23 words, every segment carries words.
`fixtures/audio/id.wav`: 3 segments, 18 words, same. In both, every word lies
inside its segment span, `end >= start`, and starts are non-decreasing — the
invariants the store and the UI rely on. **No error figure is claimed for these
files**: they are contiguous reading-style speech with no annotation, and
inventing a reference (energy VAD, "pause midpoints") would be a weaker number
dressed as the same thing.

---

## 4. Phase 2 — persistence

### 4.1 Schema v6, additive only

`word_timings` (`store.rs:81` bumps `SCHEMA_VERSION` to 6; DDL at
`store.rs:276-288`, migration at `store.rs:489-518`):

```sql
CREATE TABLE word_timings (
    transcript_id TEXT    NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
    segment_idx   INTEGER NOT NULL,
    word_idx      INTEGER NOT NULL,
    start_ms      INTEGER NOT NULL,
    end_ms        INTEGER NOT NULL,
    text          TEXT    NOT NULL,
    aligned_text  TEXT    NOT NULL,   -- the segment text this row was computed from
    stale         INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (transcript_id, segment_idx, word_idx)
);
```

No table is rebuilt, no row is rewritten; a v5 library opens with an empty
table and behaves exactly as before (`a_v5_database_upgrades_to_v6_additively`).
Deleting a transcript cascades to its rows (`deleting_a_transcript_cascades_to_its_word_rows`).

Writes go through `insert_transcript_with_words` (same transaction as the
segments, used by the CLI's `--db` and the UI's save) or
`replace_word_timings` (one savepoint, whole-set replacement). Both call
`write_word_rows`, which **refuses any group whose words do not spell its
segment** (`text.split_whitespace()` compared with the word list) — the store
is the second line of the same no-fake-alignment rule
(`a_word_group_that_does_not_spell_its_segment_is_refused`).

### 4.2 Staleness = the M15-B/E convention, one statement

```sql
UPDATE word_timings SET stale = 1
WHERE transcript_id = ?1
  AND NOT EXISTS (SELECT 1 FROM segments s
                  WHERE s.transcript_id = word_timings.transcript_id
                    AND s.idx = word_timings.segment_idx
                    AND s.text = word_timings.aligned_text)
```
(`store.rs:1571-1594`, called from `write_segments` and `replace_rows`.)

| mutation | path | effect on word rows |
|---|---|---|
| text edit (`update_segment_text`) | savepoint → `write_segments` | that row's words go stale; **other rows keep theirs** |
| no-op save (same text) | same | nothing goes stale (`a_no_op_text_save_keeps_the_alignment_current`) |
| split | `transact_rows` → `replace_rows` | the split row and every row whose text/index moved go stale; untouched rows keep theirs |
| merge | same | the merged row is stale; unrelated rows keep theirs |
| undo/redo (structural) | `restore_segment_rows` → `replace_rows` | **sticky**: a row that was stale stays stale even when the restored text matches again; a fresh alignment is the only thing that clears it (`a_split_marks_the_alignment_stale_and_only_realignment_clears_it`) |
| re-polish (`upsert_segments`) | `write_segments` | every rewritten row goes stale |
| transcript delete | FK cascade | rows are removed |

Reads use `word_alignment` (`store.rs:1173-1219`): a row is **current** only
when `stale = 0` *and* the live segment text equals `aligned_text`; anything
else lands in `stale` and is never handed to the UI as usable. Because the
check runs per row, an edited segment cannot borrow its neighbour's timing even
if indices shift.

### 4.3 Where alignment is produced

`whisper::transcribe` already had the token spans; `segment_token_spans`
(`whisper.rs:756-789`) now converts each text token (`id < token_eot`) to a
`TokenSpan`, and the same loop that builds the `Segment` calls the mapper
(`whisper.rs:727-735`). The words ride the existing event flow:

`TranscribeOutcome.words` → `PipelineOutcome.words` → `WorkerEvent::Finished { words }`
→ `Effect::SaveTranscript { transcript, words }` → `Store::insert_transcript_with_words`
→ `Editor.words`.

Parakeet/SpeechTranscriber paths carry an empty list, explicitly
(`pipeline.rs:190`, `pipeline.rs:246`).

---

## 5. Phase 2 — playback UI

### 5.1 Word click-to-seek

The active row gets a **word strip** under its text (`editor.rs`, inside the
row loop): one chip per word (`Button` with `frame(true)` +
`frame_when_inactive(true)` so the accent tint paints without hover), tooltip
`"<word> · <mm:ss.mmm> — estimated word timing; click to play from here"`.
A click calls `state.seek_playback(word.start_ms)` — the same transport path
the segment timestamp and the waveform strip use (M8/M15-C), so the pending-seek
machinery and the live-engine addressing are unchanged.

### 5.2 Karaoke highlight, spring, reduced motion

- The current word is the last word whose estimate started at or before the
  playhead (`word_at`, `editor.rs:874-884`); it holds through the pause after
  the word instead of blinking off, and it is only drawn for the segment the
  playhead is inside.
- Highlight = accent tint + accent ink + a 2 pt underline whose **width grows
  from the word's centre on a spring** (`Motion::highlight_spring`,
  `motion.rs:188-196`, which reuses the press spring's constants:
  `SPRING_DURATION` 240 ms, `SPRING_OVERSHOOT` 2 % — "gentle spring, no
  bounce").
- **Reduced motion removes only the spring**: `highlight_spring` returns the
  state's own value immediately (1.0 for the current word, 0.0 otherwise), so
  the highlight and its underline are still there, snapped. Pinned by
  `the_karaoke_highlight_springs_to_its_state_and_snaps_under_reduce_motion`
  (spring in, bounded overshoot, ends exactly, ramps out, reduced = exact
  state on both flips).
- The highlight is gated on `is_playing()`; a paused playhead shows the strip
  without a current word (the state is "which word is being spoken", and
  nothing is).

### 5.3 Explicit fallback when alignment is missing or stale

- No current words for a row → **no strip**; the row keeps its existing seek
  affordances (timestamp in the gutter, waveform strip, keyboard segment
  steps), i.e. segment seeking, unchanged.
- Some rows stale → one honest caption above the rows:
  *"word timing stale on N segments — those rows seek by segment"*
  (only rendered when `stale` is non-empty).
- A transcript from an older library (no rows at all) simply never shows a
  strip: nothing is promised, nothing is faked.

Refresh points: on load (`Effect::LoadTranscript`), after a fresh job (from the
event, before the store round-trip), after every successful autosave, and after
split/merge/undo/redo (the runtime re-reads `word_alignment` in the same arm
that already re-reads translations). `on_word_alignment_loaded` ignores reads
for a transcript that is not the open one.

---

## 6. Verification (what was actually run)

| gate | result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy -p suramak-core -p suramak-asr -p suramak-ui -p suramak-cli --all-targets -- -D warnings` | clean |
| `RUST_TEST_THREADS=1 cargo test --workspace` (final full run) | **807 passed, 0 failed, 10 ignored** (baseline 779; +28 new tests) |
| Phase-1 measurement, release, real tiny model | §3 tables; the gate test asserts median ≤ 250 ms and p90 ≤ 500 ms per fixture, and equal word counts |
| natural-fixture invariants, release, real model | pass (§3.4) |
| CLI end-to-end (`--db`, scratch `XDG_DATA_HOME`) | `saved to library`, DB at v6, 9 `word_timings` rows with the measured times, `stale = 0` |
| Xvfb job smoke (`SURAMAK_SMOKE_JOB=fixtures/align/en-words.wav`) | exit 0, `SMOKE-JOB-OK`, 17 frames, no panic with the strip in the editor |
| Xvfb screenshot of the opened transcript | the 9 word chips render under the segment line, plain (no current word) while paused — `build/m15d/editor.png` (scratch) |

New tests (28): 11 `words::` mapping, 9 store word-timing/migration, 1 motion
spring, 2 editor (`word_at`, alignment adoption), 1 app-state edit invalidation,
4 alignment-harness units — plus the 2 `#[ignore]`d release measurements (not
counted in 807).

---

## 7. Honest boundaries

1. **Estimated, not forced.** The numbers are whisper.cpp's own token spans:
   anchored on timestamp tokens where the decoder emitted them, interpolated by
   voice length elsewhere. Worst measured word start: **361 ms early** (ID,
   one long segment). No claim beyond §3 is made anywhere in the UI; the strip
   tooltip says "estimated".
2. **Produced at transcription time only.** There is no re-alignment path in
   this slice: an edited segment, a split/merge, or a library upgraded from v5
   has no current words and falls back to segment seeking (with the stale
   caption where rows exist). Reconstructing timing for *edited* text needs a
   different tool (word-level forced alignment, or a re-transcribe-and-match
   diff), which this slice deliberately does not fake.
3. **Parakeet is not wired.** It has per-token points
   (`parakeet.rs:61-76`), not spans, and its model is not on this VPS — wiring
   it unmeasured would be a claim without evidence. Same for the macOS
   SpeechTranscriber path, which exposes no tokens here.
4. **ASCII whitespace segmentation.** Correct for English and Indonesian;
   scripts without spaces (CJK, Thai) would get one "word" per run. Stated, not
   papered over.
5. **A token stream that does not spell the stored text gets nothing** — that
   includes the rare lossily-decoded segment. Refusal over substitution, by
   design and by test.
6. **The playing highlight was not captured visually.** The chip row itself was
   screenshotted on Xvfb (9 chips under the segment, none current while paused),
   but the Xvfb box has **no audio output device**, so playback never starts
   there and the tinted current word could not be shot. That state is covered by
   the motion test and `word_at` tests, not by a picture; a real-display pass of
   the playing state remains an owner check.
7. **The ID fixture uses flite's English voice** (`slt`) on Indonesian words —
   the timing is what matters here, but the text is mis-heard, so the lexical
   mismatch count (3/7) is an artifact of that voice plus the tiny model, not a
   claim about Indonesian ASR quality.
8. **`thold_pt`/`thold_ptsum` stay at their 0.01 defaults.** Lowering them
   would accept more timestamp anchors and might shrink the ID interpolation
   error; that is a parameter experiment with a quality/text risk and belongs to
   a measured follow-up, not to this slice.

## 8. Suggested next increment (not in scope here)

Wire Parakeet's token points through the same mapper (span = point → next
point), then add a re-alignment path for edited text (word-level forced
alignment against the stored audio). Both build on this table and its staleness
rule; neither changes it.
