# M25B — long sessions, honestly capped, split at pauses

**Status:** implemented. No commits, no pushes, no CI changes.
**Box:** 2 vCPU AMD EPYC 7K62, 3.7 GiB RAM, Linux 7.0.0-14, rustc 1.99.0.
**Date:** 2026-11-13.
**Gate:** `docs/reports/M25a-long-recording-gate.md` (45 min Android / 60 min desktop,
tiny, single pass; 90 min and both chunking mechanisms are STOPs).

## 1. The cap, named and enforced

`crates/suramak-core/src/session.rs` owns the one number both hosts read:

| host | `SESSION_CAP_MS` | measured budget |
|---|---:|---:|
| Android (`target_os = "android"`) | **2,700,000 ms (45:00)** | 722 MB |
| desktop / CLI / tests | **3,600,000 ms (60:00)** | 859 MB |

* `session_cap_ms()` returns the compile-time constant for this host;
  `session_cap_ms_for(SessionPlatform::{Android,Desktop})` exposes both values
  to the platform tests; `session_peak_mb()` carries the measured number into
  the refusal.
* Exactly at the cap is accepted; **1 ms over is refused** by
  `QueueDriver::start_inference` **before the inference slot opens** (and
  before the decode-ahead request), on recorded parts and imports alike.
* The named reason is `session_too_long` and carries the duration, the cap and
  the measured budget, e.g.

  ```text
  session_too_long: this session is 46:02, over the 45:00 cap; 45 minutes measures
  722 MB peak on a 4 GB phone — split it into shorter parts
  ```

  The journal row (when the driver owns one) is marked failed with the same
  reason; the queue row shows it verbatim.
* A deliberate `Part N of M` is allowed up to the cap **plus the 120 s rotation
  grace** (the recorder wrote on to the next real pause); any other file keeps
  the hard cap.

## 2. The split rule

`session::plan_split` / `split_audio_file` / `write_split_wavs`:

* a silence is a run of ≥ **600 ms** whose 10 ms RMS is at or below −40 dBFS
  (`SILENCE_RMS = 0.01`);
* at the cap, the next qualifying pause is the cut:
  * a pause that straddles the cap cuts **exactly at the cap** (the part stays
    at 45:00, not over);
  * a pause that starts after the cap cuts at the pause's first sample (the
    recorder kept writing until the pause);
* if no pause arrives inside the **120 s grace**, the cut is the lowest-energy
  100 ms window in the last two seconds before the cap and the plan records
  `SplitReason::NoPause`; the row says `split at 45:00 (no pause found)`;
* parts are half-open sample ranges that cover the input exactly once — no gap,
  no overlap, and each part is `≤ cap` for the no-pause path; a pause-aligned
  part can use the grace.
* part names are `{stem} — Part N of M.wav`; `PartInfo::from_name` parses the
  label from a title or file name (`PartInfo::base_name` strips it for the
  library title).

## 3. Recording

* The runtime finalises the authoritative mixed buffer once and writes it
  through `write_split_wavs`; a session that fits the cap is still one
  `{stem}.wav` (no re-encode, no `Part 1 of 1` noise). A longer one is written
  as `Part N of M` slices of that same buffer, so **decoding the parts in order
  reproduces the uninterrupted capture sample for sample** (the PCM16
  conversion is per-sample and deterministic).
* The record view carries the live half: `LiveSplitTracker` watches the same
  16 kHz mixed chunks, and on the pause (or the grace edge) the part counter
  resets with a one-line note (`Part 2 · started at a pause`, 6 s, nothing
  modal).
* `AppState::on_recording_stopped_parts` puts every part into the ordinary
  import batch; each part is a normal job and its own temp-cleanup entry.
* Quit-mid-recording Stop & Save writes every part into the pending entry
  (`part=` lines, format stays v1 and forward-compatible), and the resume
  enqueues all of them.

## 4. Imports

* `AppState::begin_imports` probes each file's header (no decode); over the cap
  it decodes once, resamples to 16 kHz mono, and writes `Part N of M` files in
  a scratch session dir, then queues every part. The status line carries the
  plan summary (`3 parts · split at 45:00 (pause)`).
* `QueueDriver::enqueue_import` / `enqueue_import_with_cap` is the same rule at
  the queue layer (used by the tests); a file at or under the cap stays one
  job, and a split that cannot be produced leaves the original for the named
  refusal.
* There is **no chunked decode for transcription**: an import is decoded once
  to find pauses; each part is then decoded normally by its own job, exactly
  like a user-imported file.

## 5. Library and diarization

* Compact library rows show `Part 2 of 3` at the start of the meta line (beside
  the title); the desktop sidebar prefixes the same label. Part titles are the
  base name (`Meeting`, not `Meeting — Part 2 of 3.wav`).
* The desktop Speakers action checks the audio's duration before submitting. A
  transcript over diarization's existing 30-minute `MAX_AUDIO_MS` gets the
  existing named refusal plus the honest route:

  ```text
  this recording is 44.1 min; diarization is capped at 30 min — split into
  shorter parts to label speakers
  ```

  No diarization behaviour was changed.

## 6. The 50-minute split run (measured)

Test: `crates/suramak-asr/tests/m25b_session_split.rs`, run explicitly with
`--ignored --nocapture`. Fixture: synthesised 50:00, 16 kHz mono PCM16,
96,000,044 bytes, 48,000,000 samples, a real 1 s pause straddling 45:00.

| part | samples | length | file bytes |
|---|---:|---:|---:|
| `Part 1 of 2` | 43,200,000 | **45:00** (2,700,000 ms) | 86,400,044 |
| `Part 2 of 2` | 9,600,000 | **5:00** (300,000 ms) | 9,600,044 |
| **total** | **48,000,000** | **50:00** | 96,000,044 |

* plan: `2 parts · split at 45:00 (pause)`
* every part is at or under the 45-minute Android cap;
* the parts' sample count equals the input's;
* decoding the two parts in order equals the input WAV sample for sample;
* **peak RSS (VmHWM): 533,048 KB = 520.6 MiB** — below the cap's 722 MB
  expectation (the run stops before inference; the peak is the 16 kHz decode
  copy plus the mono resample copy, ~2 × 192 MB, not a model).

The fixture is deleted by the test; `/tmp`/scratch left no 50-minute file.

## 7. Tests and gates

| gate | result |
|---|---|
| `cargo test -p suramak-core session:: --lib` | **17 passed** |
| `cargo test -p suramak-asr --lib queue::driver::tests::*` | cap boundary, 1 ms over, exact cap, part-grace, 2- and 3-part imports all pass |
| `cargo test -p suramak-asr --test m25b_session_split -- --ignored --nocapture` | **1 passed**, part lengths and peak above |
| `cargo test -p suramak-ui --bin suramak` | **376 passed** (part line, rotation reset/note, library chip, diarization refusal, pending parts) |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| NDK check (`cargo ndk -t arm64-v8a --platform 29 check`) on `suramak-android` + `suramak-android-spike` | clean |
| NDK clippy (`-D warnings`) on both Android crates | clean |
| `cargo test --workspace` | **1,461 passed, 0 failed, 12 ignored** (baseline 1,434; +27) |

Covered by name:

* cap function per platform; exactly at cap accepted, +1 ms refused; refusal
  reason carries `session_too_long` and the measured budget;
* rotation at a synthetic silence with sample-exact concatenation;
* grace split with no pause (`(no pause found)`);
* import 2-part and 3-part splits with dense `Part N of M` metadata;
* a 50-minute synthesised run whose parts are all under the cap and whose
  total sample count is the input's;
* the record view's `Part 1 · 44:12 of 45:00` line only past 80 %;
* the 30-minute diarization refusal wording.

## 8. Limits / notes

* **Recording files are materialised at Stop, not switched mid-stream.** The
  desktop recorder is buffer-then-stop by design, and the authoritative mix
  (per-source gain/mute + clock-drift correction) exists only at finalise. The
  split is applied to that exact buffer, so the sample-exact guarantee is
  against the capture the product actually keeps; `LiveSplitTracker` drives the
  live counter/note from the same pause rule. A mid-stream file switch would
  have to re-create the resampler at the cut and would break the byte-for-byte
  tail contract the desktop/Android writers currently share.
* **Android's streaming capture** (`AndroidRecorder` → one `StreamingWav`) is
  closed by §10 (M25B2): the writer now rotates **after** the resampler, so a
  take longer than 45 minutes lands as `Part N of M` and is never refused by
  the cap. Resampler state does not cross the cut.
* A pause-aligned part can run into the 120 s grace (at most cap + 120 s); the
  queue accepts a `Part N of M` up to that bound. The no-pause fallback stays
  at or under the cap.
* The 50-minute test is debug, so wall time (26.7 s) is not a product number;
  peak RSS is allocation-dominated and valid for the split path. Inference was
  not run in that test (no model), which is why 520.6 MB is below the 722 MB
  inference budget rather than equal to it.

## 9. Reproduce

```bash
# cap + splitter + concatenation + imports
cargo test -p suramak-core session:: --lib

# queue refusal, exact-cap acceptance, part grace, 2/3-part imports
cargo test -p suramak-asr --lib queue::driver::tests

# 50-minute measured run (prints part lengths and VmHWM)
cargo test -p suramak-asr --test m25b_session_split -- --ignored --nocapture

# UI line/rotation/library/diarization
cargo test -p suramak-ui --bin suramak
```

## 10. Phone rotation (M25B2)

**Status:** implemented. No commits, no pushes, no CI changes.
**Mechanism:** `PartRotator` in `crates/suramak-asr/src/capture_stream.rs`,
between the resampler output and the WAV files.

### 10.1 How the cut works

* The Android consumer pushes device-rate mono blocks through the unchanged
  `FftFixedIn` resampler; its **16 kHz output** goes to the rotator. A cut is a
  sample index on that one continuous stream: the open part is truncated at the
  cut and the already-written tail bytes are copied into the next part. PCM16 is
  per-sample, so the parts concatenate to exactly the uninterrupted 16 kHz
  take — **bit for bit by construction**. No resampler state ever crosses a cut.
* The rule is the desktop rule of `suramak_core::session`, applied live:
  * at the phone cap (45:00) a rotation becomes pending; the writer keeps
    writing while it looks for the cut;
  * the cut is the first ≥ 600 ms silence at or after the cap;
  * a pause that straddles the cap cuts **exactly at the cap**;
  * if no pause completes inside the ≤ 120 s grace, the cut is the
    least-energetic 50 ms window **after the cap** in the last five seconds, and
    the part is flagged `no_pause` (`cut_without_pause()`);
  * every cut is clamped to the grace edge, so every part is ≤ cap + grace. A
    take that stops in the sub-frame gap past the grace edge is split once at
    stop for the same reason.
* Silence is an RMS gate over 50 ms windows. The threshold is **three times the
  take's own noise floor** (the quietest 50 ms frame of the first two seconds),
  clamped to 0.0005–0.01 RMS (≈ −66…−40 dBFS), and logged once per part
  (`tracing` target `suramak::capture::parts`, part number, start clock and the
  threshold used).
* Stop flushes the rotator. A take that fits keeps its reserved name, exactly
  as before; a rotated take is renamed to `{stem} — Part N of M.wav`. The mic
  host adopts **each part as its own ordinary job** (part 1 keeps the reserved
  key, parts 2…N get `{id}-pN`), and the foreground notification reads the
  live part counter from the host status (`Recording · part 2`).
* The 45-minute cap **never refuses a phone take now**: it rotates. The
  `session_too_long` refusal remains for imports that cannot be split and for
  the **12-part ceiling** (`MAX_PARTS = 12`, 12 × 45 min = 9 h):
  `session_too_long: this take is longer than 9 hours — a recording stops at 12
  parts of 45:00 each; start a new recording`. The twelve parts written before
  the ceiling are repaired and handed off anyway; the stop note carries the
  sentence into the status surface.

### 10.2 The measured 47-minute take

Test: `crates/suramak-asr/tests/m25b2_phone_rotation.rs`, run with
`--ignored --nocapture`. Fixture: synthesised 47:00 at 16 kHz, one 1 s pause at
45:10 (the first real pause after the cap), 45,120,000 samples.

| part | samples | length | file bytes |
|---|---:|---:|---:|
| `Part 1 of 2` | 43,360,000 | **2,710,000 ms (45:10)** | 86,720,044 |
| `Part 2 of 2` | 1,760,000 | **110,000 ms (1:50)** | 3,520,044 |
| **total** | **45,120,000** | **47:00** | 90,240,044 |

* exactly **one** rotation, at the first silence after 45:00 (45:10.000);
* all 45,120,000 samples (90,240,000 data bytes) compared against the same
  take written as one WAV: **identical byte for byte**;
* **peak RSS (VmHWM): 6.6 MiB** (6,580–6,796 KB = 6.4–6.6 MiB across runs) —
  the streaming rotator never holds the take. (M25B's buffer-then-split 50-minute run peaked at 520.6 MiB; this
  path removes that copy entirely.)
* wall time in the debug test: 6.8 s (not a product number — the fixture is
  generated and pushed in the same process).

### 10.3 Tests and gates

| gate | result |
|---|---|
| `cargo test -p suramak-asr --lib capture_stream::` | **21 passed** (9 pre-M25B2 + 12 rotation) |
| `cargo test -p suramak-android --test mic_host` | **11 passed** (2 new: multi-part hand-off, ceiling keeps 12 parts) |
| `cargo test -p suramak-asr --test m25b2_phone_rotation -- --ignored --nocapture` | **1 passed**, parts and peak above |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| NDK check + clippy (`cargo ndk -t arm64-v8a --platform 29`, both Android crates) | clean |
| `cargo test --workspace` | **1,478 passed, 0 failed, 13 ignored** (baseline 1,461; +17) |

Covered by name: a 47-minute take rotates once at the first silence after
45 minutes; a pause after the cap cuts there; a long **pre-cap** silence does
not rotate early; a no-pause take cuts at the least-energetic 50 ms window and
is flagged; three parts are named `Part N of 3` and the provisional name is
gone; no rotation ever emits a zero-length part; a take ending exactly at the
cap stays one file; a sub-frame stop past the grace edge still lands inside
cap + grace; the 12-part ceiling refuses with the one sentence and keeps the
twelve parts; the threshold is 3 × the first-two-seconds floor; `repair_take`
finds every provisional part of a killed take; a rotated take hands off every
part as its own job.

### 10.4 Limits / notes

* **A process death mid-rotated-take recovers part 1 through the existing
  single-path mic journal.** Later parts are already playable WAVs in
  `files/media` (their headers are repaired on demand), but the launch replay
  does not yet adopt them automatically; that needs a parts list in the mic
  journal (a separate slice, and the queue journal/resume is untouched).
* The notification's part line is Kotlin (`RecordingService` → `Notify`), built
  only in the app build, which this box does not run; the Rust status field it
  reads (`MicStatus.part`, fed by the capture thread's `Meter`) is host-tested.
* A pause-aligned part may use the 120 s grace (≤ 45:02:00), exactly like the
  desktop parts; the queue accepts a `Part N of M` up to that bound. The
  no-pause fallback is clamped to the same bound, including at stop.
* Part 1 is renamed at stop; if the process dies before stop, the provisional
  name (`{stem} — Part 2.wav`, no `of M`) is what remains on disk. `PartInfo`
  needs both numbers, so a recovered part's label is only reconstructed by
  `repair_take`, not by the library row.
