# M16I — diarization production integration (schema v7 + badges/lane)

**Status: implemented and verified on this Linux VPS; the macOS/owner-device
execution gates are named, not run.** The contract is the M16H interface
decision and the M16I workorder: diarization is a **derived, replaceable**
annotation stored in three additive tables; the runner is a one-slot job with
the **discard-at-the-next-await-point** cancellation contract; the editor shows
restrained badges and ONE 8 px lane, with rename-in-place, delete-restores and a
stale/re-run state. Real-meeting audio quality remains the owner-device pass, and
3+ speakers remain provisional and manually correctable by design.

```text
workorder=docs/WORKORDER-M16I.md
parents=M16h-diarization-feasibility.md, M16h0-diarization-slice0.md
schema=6 -> 7 (additive: diarization_runs, speakers, speaker_turns)
runner_slot=1 (DiarizationLane; the M16.1 discipline, same CancelToken)
cancel_contract=discard at the next await point; the binding has no mid-run abort
max_audio_ms=1800000 (30 min)
models=pyannote-segmentation-3.0 (MIT) + CAM++ (Apache-2.0), pinned by size+sha256
engine=sherpa-onnx 1.12.9 via sherpa-rs 0.6.8 (the `parakeet` linkage / M16F dylibs)
editor=18 px badge (3 px bar) per row + one 8 px lane above the waveform
```

## 1. What shipped

### 1.1 Schema v7 (additive, `suramak-core`)

`SCHEMA_VERSION` 6 → 7. One `upgrade_to_v7` transaction creates
`diarization_runs`, `speakers`, `speaker_turns` and the
`speaker_turns_span` index; the base DDL creates the same objects for a fresh
database. Nothing is rebuilt, dropped or rewritten. No `segments` or
`transcripts` column changes.

* **`diarization_runs`** — transcript id, monotonic `run_id`, `created_at`,
  segmentation/embedding file names + sha256s, `engine_version`, clustering
  parameters (`num_clusters` where `0` = auto, `threshold`, min durations),
  `audio_ms`, and the sticky `stale` marker. A row exists only for a
  **completed** run — a failed or cancelled run writes nothing — so `stale` is
  the row's status flag ("current" vs "the transcript changed since"), and the
  transient queued/running/cancelling/done states live in the job lane, not in
  the table.
* **`speakers`** — dense app-owned `speaker_id`, the label (`"Speaker 1"` or
  the user's name), `color_slot` (0–5), `source` (`auto`/`manual`), timestamps.
  The user-set name **is** the label with `source='manual'`; the two never
  diverge in the surface, so a second `name` column would be a second source of
  truth (M16H's DDL is label + colour + source).
* **`speaker_turns`** — `(run_id, turn_idx)`, `start_ms`/`end_ms`,
  `speaker_id`, `source` (`auto`/`manual`). Overlaps are allowed; a `CHECK`
  refuses an empty span and both foreign keys cascade.

**Turns are time-anchored; segment ranges are derived on read.** The store
exposes the M16H surface:

```text
replace_diarization_run(id, run, speakers, turns) -> run_id   (one transaction)
diarization_run(id) / speakers(id) / speaker_turns(id) / diarization(id)
speaker_for_segments(id) -> Vec<Option<SegmentSpeaker>>      (largest overlap;
                                                              >=25 % each = split)
speaker_turn_ranges(id) -> Vec<(turn_idx, first_idx, last_idx)>  (midpoint rule)
rename_speaker / set_speaker_color / reassign_turn
merge_speakers / split_speaker / delete_diarization
```

Provenance rules enforced by tests: applying a run and then deleting it leaves
every `segments` row and every `transcripts` row **byte-for-byte** as it was;
`source_text` is never touched; a re-run preserves a manual name on the same
speaker id and keeps an unmatched manual speaker (with no turns) instead of
deleting it; unused *auto* speakers are dropped. Deleting the transcript
cascades all three tables.

**Staleness (M15-B/E machinery).** `diarization_runs.stale` is set in the same
transaction as `update_segment_text`, a split, a merge, a restore/undo or a
whole-set upsert, and is cleared only by a fresh run. The turns themselves are
never rewritten — they stay time-correct; the badge/lane view is what becomes
provisional. This was a deliberate reconciliation of the workorder's
"edit/split/merge marks stale" with M16H's "text edits do not stale turns":
the time data does not go stale, the **segment projection** does, and the UI
says so.

A **newer-schema database is refused before any DDL is written** (the version
marker is read first), and a non-numeric marker is a refusal — both pinned by
tests.

### 1.2 The runner (`suramak-asr::diarization`)

* **One slot.** `DiarizationLane` is a pure state machine
  (`Idle → Queued → Running → Done/Failed/Cancelled`); a second submit while the
  slot is held is `Busy`, and a transcript already queued/running is
  `Duplicate`. The thread wrapper (`DiarizationWorker`) owns the model run; the
  runtime owns the lane and the token.
* **The same cancellation seam** (`suramak_asr::CancelToken`). The contract,
  shown verbatim by the Stop tooltip:

  > Stopping discards this run's result; the model cannot be interrupted
  > mid-file, so the labels disappear only when it finishes.

  A **queued** job cancels immediately and never starts. A **running** job's
  token is set and the state shows `cancelling…`; the slot stays occupied until
  sherpa returns; then `execute` returns `Cancelled` at its second await point
  and `DiarizationLane::admit` returns `Discarded` — **the turns are never
  normalised, written or shown**. A cancel that lands as `compute` finishes is
  surfaced as `Cancelled { discarded: true }`, never as a "done" with nothing
  written.
* **Bounded input.** `MAX_AUDIO_MS` = 30 min; longer audio is refused with the
  two numbers before the engine runs. Progress is 0–100 through the binding's
  progress callback (progress-only: the callback deliberately returns 0).
* **Model pins & named errors.** Segmentation and embedding files are pinned
  by byte size and sha256 (M16H §3, unchanged). The UI probe checks size only
  (34 MB of hashing must not hitch a frame); the worker's load runs the full
  sha256 check and names the failing file. Silence (the binding's
  `Err("No segments found…")`) maps to **zero turns**, not a failure.
* **Availability is a designed state, never a missing control.** The pure rule:

  | build | platform | models | state |
  |---|---|---|---|
  | no sherpa linkage | any | any | unavailable: "compiled without the sherpa-onnx engine" |
  | linked | macOS (M16F dylibs) | ready | **ready** |
  | linked | Windows/Linux/Android | ready | unavailable: the speech libraries are bundled in the macOS app only today; the platform follows when its packaging ships them |
  | linked | macOS | missing/wrong size | unavailable, naming the file and the expected bytes |

  Diarization rides the **same `parakeet` feature** that links sherpa-rs and
  whose M16F dylib bundle ships in the macOS app — no second runtime, no second
  feature, no new dependency.

* 16 kHz mono conversion is mandatory (`decode_file` →
  `to_16khz_mono`); the binding would otherwise read 8 kHz audio at double
  speed.

### 1.3 The editor surface (`suramak-ui`)

Shared Deck components (`deck/speakers.rs`) — the desktop editor and the
compact (phone) editor call the same `badge()`/`lane()` functions; there is no
Android-specific rendering.

* **Row badge:** a 3 px colour bar + an 18 px chip at the start of each row,
  numeral for `"Speaker N"` / initial for a renamed speaker. The row reserves
  the slot **only when a run exists** — the plain view keeps its exact
  geometry and has no placeholder. Click = rename in place; right-click =
  cycle the colour slot (a manual edit, one row).
* **Rename-in-place:** the badge becomes a text field; Enter/blur commits,
  Escape cancels; the store updates one `speakers` row and every badge, the
  lane and any export read the new name through `speaker_id`.
* **One lane:** 8 px, directly above the waveform, drawn from
  `speaker_turns.start_ms/end_ms` over the audio length. 30 % bands, 1 px
  separators; two overlapping turns draw as two 4 px half-height bands, never
  a blended colour.
* **Speaker line** (top of the transcript, scrolls with it): voice count, run
  provenance (`pyannote-segmentation-3.0 + CAM++ · c=2`/`auto`), the count
  selector (Auto → 2…6 → Auto), `Add labels`/`Re-run`, and `Delete diarization`
  with an in-place confirm. Stale runs add `labels stale — text changed` in the
  warn colour and dim badges/lane to 55 %.
* **Delete** removes run + speakers in one transaction; the transcript rows are
  byte-identical before/after and the plain view returns.
* **A11y:** the badge is a labelled focusable control (`"Budi — click to rename"`)
  with the standard focus ring; the lane is a labelled `Image` node
  (`"Speaker lane: N speaking turns (…)"`). Colour is never the only channel
  (numeral + separators).
* **Motion:** none added. The lane draws immediately (a stored run is not
  "arriving"); there is no idle animation and no per-frame work proportional to
  the run.
* **Capacity / device:** the phone draws the same lane + badges; its host may
  seed `CompactEditor::speakers` (the field defaults to `None`, so a phone
  build without diarization is unchanged).

## 2. Verification

```text
cargo fmt --all --check                                  = clean
cargo clippy --workspace --all-targets -- -D warnings    = clean
cargo test --workspace                                   = 1130 passed / 0 failed / 10 ignored
                                                           (36 test binaries), exit 0
```

The baseline was 1079; this slice adds **51 tests** (core 196 lib + the
integration suites, asr 162 lib, the UI's 92 lib / 356 bin / 13 phone shots).
No M15 suite changed its outcome.

Orchestrator note: the child's pre-cutoff run reported 1125/46; its last five
tests plus the frame-lane mirroring fix were then verified by one more full
run — plus one test corrected by the orchestrator:
`a_malformed_run_is_refused_and_the_previous_run_survives` asserted that an
empty speakers+turns set is refused, but M16I's silent-run contract (a
recording with no speech is a legitimate completed result, pinned by
`a_silent_run_round_trips_with_no_speakers`) made that stale — the assertion
now matches the contract. Final tree: 1130/0/10, fmt/clippy clean.

Unit/integration coverage added (all named by the workorder):

| what | where |
|---|---|
| turn persistence round-trip + projections | `store::tests::a_diarization_run_round_trips…` |
| rename propagation + manual source | `store::tests::renaming_a_speaker_propagates…` |
| stale on edit **and** split **and** merge, turns untouched | `store::tests::an_edit_split_or_merge_marks…` |
| delete restores the plain view; transcript delete cascades | `store::tests::deleting_the_diarization_restores…` |
| malformed run refused, previous survives | `store::tests::a_malformed_run_is_refused…` |
| reassign / merge / split / colour | `store::tests::speaker_corrections_reassign_merge…` |
| unused auto speaker dropped, manual name kept | `store::tests::a_rerun_drops_unused_auto_speakers…` |
| v1…v6 populated fixtures upgrade, rows value-for-value | `store::tests::every_populated_fixture_upgrades_to_v7…` |
| this workspace's populated `build/` databases | `store::tests::the_build_directories_populated…` |
| corrupt / newer schema refused, nothing written | `store::tests::a_corrupt_or_newer_schema…` |
| cancel while queued never starts | `diarization::tests::cancelling_a_queued_job…` |
| cancel during compute discards at the await point | `diarization::tests::a_cancel_during_compute_discards…` |
| progress 100, success path, failure frees the slot | asr `diarization::tests` |
| duration cap | asr `the_duration_cap_refuses…` |
| platform-without-dylibs unavailable state | asr `every_platform_without_the_dylibs…` |
| size probe cheap, full check catches a wrong sha | asr `the_size_probe_is_cheap…` |
| six slots clear 3:1 bar + 4.5:1 numeral on both ladders | `theme::tests::every_speaker_slot…` |
| desktop golden shots (light/dark/stale) | `tests/ui-desktop/shots/editor-speakers-*.png` |
| phone inherits badges + lane | `tests/ui-compact/shots/editor-speakers.png` |
| lane geometry (8 px, above the waveform) | `editor::tests::the_speaker_lane_is_one_eight_pixel_strip…` |

**Upgrade-in-place, evidence.** The unit test builds a populated database at
each of v1…v6 (three segments with provenance appropriate to that version, plus
glossary/prefs/word rows where the version had them), upgrades each, and
compares every existing row value-for-value plus zero invented diarization
rows. A second test walks this workspace's gitignored `build/` tree and, for
every populated pre-v7 `*.db`, copies it, upgrades the copy and compares the
`(idx, start_ms, end_ms, text)` projection of every segment (plus the segment
count of the first transcript). On this box that is **17 populated fixtures**:
16 v1 libraries from the M9b–M9e, M10a, M4.5 and reshoot acceptance runs, and
the v6 M15d smoke library — every one upgrades to v7 with its rows intact. The
tests are skipped gracefully when `build/` is absent (CI), where the synthetic
v1…v6 fixtures are the guarantee.

**Golden shots.** `tests/ui-desktop/shots/` — `editor-speakers-light.png`,
`editor-speakers-dark.png`, `editor-speakers-stale.png` (1280 × 800, the real
egui shape stream via the deterministic rasteriser). They show the badges
(1 / B / 1, blue/amber/blue), the 8 px lane with three bands, and the stale
variant's dimmed badges + stale line. `tests/ui-compact/shots/editor-speakers.png`
is the phone proof: the same components, one lane above the list and a badge on
every row.

## 3. The honest boundary

* **Real meetings are the owner-device pass.** Every accuracy number in the
  parent reports is TTS (M16H flite, S0 edge-tts EN+ID). Room tone, spontaneous
  crosstalk, laughter, far-field/phone channels and unknown microphones are
  still unmeasured; the 2-speaker DER 3.29 % is a ceiling, not an expectation.
  The feature ships as **1–2 speakers automatic; 3 provisional; 4+ not offered**,
  exactly the S0 scope.
* **3+ speakers are provisional and manually correctable by design.** The
  editor exposes rename, colour, delete and re-run; the store already provides
  reassign/merge/split (tested) for the repair workflow, but the first slice's
  surface does not yet expose per-turn reassignment. A merged pair is repaired
  by re-running with an explicit count or by the store-level operations, not by
  a timeline editor (which the plan forbids).
* **No model install path in this slice.** The runner reads
  `<models>/diarization/{segmentation,embedding}` with the M16H names and pins;
  the Model Manager entry/download for the 34 MB pair is the packaging
  follow-up (M16H slice 2). A missing install is a named unavailable state, not
  a crash.
* **macOS dylibs are the M16F gate.** This box has no Apple toolchain; the
  clean-machine dylib load with the diarization models is the owner-Mac/CI
  gate, named by M16F. Windows/Linux/Android are the designed unavailable state
  until their packaging ships the same libraries.
* **No 1-hour measurement.** 30 min is the cap; the 1 h projection remains
  arithmetic on the S0 cost (0.157–0.167 s/audio-s), to be measured on the
  owner's Mac before the cap is raised.
* **No cross-session identity.** Nothing here matches voices across
  transcripts, stores embeddings, or names people. The lane's tooltip and the
  status line say the labels describe voices in this recording.
* **Export with speaker labels is not in this slice.** No `segments.text`
  change, no SRT/VTT prefix; the default export is untouched. The interface's
  slice 5 remains open.
* **The runner is not the file `JobQueue`.** Diarization is one job over one
  already-stored transcript, not a file in the import queue; it mirrors the
  M16.1 discipline (one slot, terminal events, the same `CancelToken`) in a
  sibling lane rather than reusing the decode-ahead queue whose states are
  file/decode-shaped.
* **No real-meeting overlap test.** The 8 px lane's half-height overlap path is
  driven by the stored-turn model and the synthetic fixture; M16H's planted
  overlap remains the only overlap evidence.

## 4. Files

New:
- `crates/suramak-core/src/diarization.rs` — types, projection, validation,
  in-memory correction helpers.
- `crates/suramak-asr/src/diarization.rs` — pins, model checks, availability,
  params, lane, worker, sherpa adapter, tests.
- `crates/suramak-ui/src/deck/speakers.rs` — the shared badge + lane renderer.
- `crates/suramak-ui/src/desktop/app/diarization.rs` — editor state, rename
  draft, job mirror, actions.
- `tests/ui-desktop/shots/editor-speakers-{light,dark,stale}.png`
- `tests/ui-compact/shots/editor-speakers.png` (new)
- `docs/reports/M16i-diarization-integration.md` (this report)

Modified:
- `crates/suramak-core/src/store.rs` — v7 DDL + upgrade, the diarization API,
  stale hooks, tests.
- `crates/suramak-core/src/lib.rs`, `src/error.rs` — module, `Uuid` re-export,
  `SuramakError::Diarization`.
- `crates/suramak-core/tests/persistence.rs` — the schema constant.
- `crates/suramak-asr/src/lib.rs` — module.
- `crates/suramak-ui/src/deck/{theme,mod}.rs` — six speaker slots + contrast
  test, module.
- `crates/suramak-ui/src/deck/compact/{model,editor,mod}.rs` — the optional
  speaker view and the shared renderer call.
- `crates/suramak-ui/src/desktop/app/{mod,editor}.rs` — effects + state field,
  badges, lane, rename, status row, golden shots.
- `crates/suramak-ui/src/desktop/{runtime,desktop}.rs` — the effect handlers,
  the worker poll, the startup availability seed.
- `crates/suramak-ui/tests/ui_compact.rs` — the inheritance shot.

No commit, no push, no CI edit, no new dependency (sherpa-rs was already an
optional workspace dependency), no engine change, no Android-specific code.
