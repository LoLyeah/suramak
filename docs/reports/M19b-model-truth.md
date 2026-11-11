# M19B — the model registry tells the truth (and picks the phone's default)

Executed per `docs/WORKORDER-M19B.md` (slice WO-2 of
`docs/reports/M19-android-audit-plan.md`).

**Run status.** Implemented and verified on this VPS with real commands: the
full workspace suite (**1172 passed / 0 failed**, baseline 1148 + 24 new),
`clippy --workspace --all-targets -D warnings`, `fmt --check`, and a
positive/negative exercise of the new smoke assertion block. **The emulator
smoke did not run here** — this host has no `/dev/kvm`, no `emulator` package
and no Android SDK — so §5 names exactly what its first CI run will assert.
The new screenshot was rendered by the phone-viewport suite (and the
regenerated ones inspected) before this report was written.

---

## 1. The defect, in three places

| Where | What it did | Why it lied |
|---|---|---|
| `settings.rs` (compact) | `selected = row.id == settings.model_id`, then `selected → "In use"` and the accent border | selection is a setting, not a fact about disk |
| `Settings::default()` + Android host | `asr.model_id = "turbo-q5-0"`; the host loads `Settings::load(..).unwrap_or_default()` on every start | the phone inherited the desktop default and never applied the tier it already knew |
| `compact_host::installed_models` | listed `files/models` **filenames** | `path.exists()` is not verification: a 3-byte file or a `.part` counted as installed |

The owner's screenshot follows directly: on a fresh 8 GB phone the configured
model was `turbo-q5-0`, so its row was selected — and the selected row said
**In use** — while no file existed. The same row said `default (macOS)`, and
the Library offered no model while the Record button stayed enabled.

---

## 2. First run and migration (Android only)

### 2.1 The schema change

`suramak_core::settings::AsrSettings` gains:

```rust
/// M19B: the model was chosen by the user (an explicit "Use" tap), so the
/// Android host must never migrate it. Missing in older files → `false`.
#[serde(default)]
pub model_explicitly_chosen: bool,
```

`Settings::default()` still names `turbo-q5-0`
(`models::DESKTOP_DEFAULT_MODEL_ID`, now a shared constant so the migration
cannot drift from it) and the flag defaults to `false`. **Desktop behaviour is
unchanged**: the desktop never consults the flag and never calls the migration.

### 2.2 The rule

`suramak_core::device::model_choice_for_phone(model_id, explicitly_chosen,
profile, verified)` returns `Some(new_id)` only when **all** hold:

1. `!model_explicitly_chosen`, and
2. `model_id == "turbo-q5-0"`, and
3. that model is not verified on disk.

The replacement is `device::highest_tier_verified(profile, verified)` — the
verified, *allowed* model with the highest manifest `min_ram_bytes` — else
`recommend(profile, ..).default_model_id`:

| Usable RAM | Tier | Fresh-install default | Below floor |
|---|---|---|---|
| 4 GB (3.7 report) | Low | `base` | — |
| 6 GB (5.5) | Mid | `base` | — |
| 8 GB (7.4) | High | `small` | — |
| 12 GB | High | `small` | — |
| 16 GB | High | `small` | — |
| < 3.4 GB / low-RAM flag | BelowFloor | — | `tiny` (unchanged steer) |

`CompactHost::device` (the first device report, which `UiBridge.device` sends in
`MainActivity.onCreate` before the surface starts) scans disk, runs the rule,
persists `settings.toml` when it changes, and only then seeds the rows. A
first run and a legacy install therefore take the *same* path: a missing file
reads as `Settings::default()` → turbo → migrate; an old file with turbo behaves
identically. The log line names what happened:

```
SURAMAK-MODEL migrate from=turbo-q5-0 to=small tier=High first_run=true verified=[] 
```

**An explicit choice is never overridden.** The `setting` op's `model` arm is
the only writer that sets `model_explicitly_chosen = true` (the compact "Use"
action). Two other writers deliberately leave it false: the migration itself,
and `refresh_models`' download adoption (a download is not a "Use" tap).

### 2.3 The migration matrix, as tested

| Stored state | Disk | Result |
|---|---|---|
| old file, turbo, flag absent (`false`) | nothing | tier default (`small` at 8 GB, `base` at 4–6, `tiny` below floor), persisted |
| old file, turbo | `base` verified | `base` (highest installed, beats the tier default) |
| old file, turbo | `base` + `small` verified | `small` |
| old file, turbo | `turbo-q5-0` verified | kept — it is a real file |
| old file, turbo | `small` verified on a 4 GB phone | `base` (small is refused there) |
| old file, `small` (a choice from before the flag) | anything | kept |
| `model_explicitly_chosen = true`, turbo | nothing | kept; the header says `turbo-q5-0 · not downloaded` |
| explicit "Use" tap on `base` | `base` verified | `base`, flag written `true`; survives relaunch |

---

## 3. Disk verification and the five-state row machine

### 3.1 Verification (`suramak_core::models`)

`ModelFileState` is `Absent | Partial | WrongSize | Verified`, read by
`model_file_state(models_dir, model)`:

* **Verified** = the file is present, its length is exactly the pinned
  `size_bytes`, and no `.part` sits beside it;
* **Partial** = a `.part` exists (an interrupted or in-flight transfer), or a
  complete file still has a `.part` beside it until the sweep removes it;
* **WrongSize** = a file exists at any other length (truncated/corrupt);
* **Absent** = nothing.

The sha256 is **not** recomputed on every scan: M13D verified it when the file
landed (`download::install_file`), and hashing 500 MB on each device report
would stall the first frame. Size + `.part` is the contract the work order
names and catches the states that actually occur. `sweep_completed_parts`
deletes a `.part` beside a complete pinned-size file (debris that can never be
resumed — the server would 416 it), and `remove_model_files` is the
Re-download's unlink (dest + part).

`CompactHost::model_disk_scan` performs the sweep + scan; the old
filename-only `installed_models` is gone. `download_state` (the resume offer)
and the post-download `refresh_models` use the same truth.

### 3.2 The machine (`compact::model`)

```rust
pub enum ModelState { NotDownloaded, Downloading, StaleCorrupt, Installed, InUse }
```

`ModelRow::state(selected, live_download)`:

| State | Condition | Row action | Subtitle |
|---|---|---|---|
| `NotDownloaded` | no file, no `.part` | `Get` (`Download` when it is the configured model, primary fill) | `· not downloaded` |
| `Downloading` | a live job, or a `.part` | `Cancel` (live) / `Resume` (no job) + `% · bytes of bytes` | `· downloading` |
| `StaleCorrupt` | wrong-sized file | `Re-download` (host unlinks dest + part before the transfer) | `· stale — re-download` |
| `Installed` | verified, not selected | `Use` (primary) | `· installed` |
| `InUse` | **verified AND selected** | `In use` (inert) | `· installed` |

Only `InUse` paints `tokens.accent_tint` + the accent border and the "In use"
word. The accent path is unreachable without `row.installed`, which is exactly
`disk == Verified`. `InUse` cannot be produced by a download view, by a
selection, or by any combination that lacks the file — the exhaustive test
walks 4 disk states × 2 selections × 4 job views and asserts the equivalence.

**Configured-but-missing** is the case the owner hit: the configured row shows
`· not downloaded` and a **primary Download**, never "In use".

---

## 4. Honest copy

| Surface | Before | After |
|---|---|---|
| Settings → Model (TRANSCRIPTION) | `turbo-q5-0` (plain, always) | `small` when verified; `small · not downloaded` in `tokens.warn` when not |
| Catalog role | `default (macOS)` | `desktop default` in the manifest, **omitted from phone rows** (`ModelRow.role` is empty and `size_role()` drops the separator) |
| Settings row subtitle | `574 MB · default (macOS) · installed` | `574 MB · not downloaded` (the disk truth replaces the platform note) |
| Library card free-space line | `Needs 148 MB · 9.0 GB free on this phone` | `Needs 148 MB · 9.0 GB free` |
| Record button | always enabled | disabled (`ink_faint`, no action, a11y `enabled=false`) with an inline warning: `Download a model before recording.`; `Choose a model that is on this phone.` when a verified model exists but is not the configured one |

`can_record()` requires the **configured** model to be verified, because that is
the id the job loads; "some model is installed" is not enough. Before the shell
reports memory (`models` empty) it does not block. The shell also refuses a
stray `Record` action with the same reason, so a stale tap cannot start a job
that is guaranteed to fail.

The Library card already used the M13D recommendation; it now inherits the
verified `installed` flag, so a `.part` or a truncated file keeps the card up
instead of hiding it.

---

## 5. The emulator smoke's new assertion

`scripts/android/emulator-smoke.sh` gains one block after the Library
screenshot and before M19A's Settings steps (logcat was cleared before launch,
so the whole run's app log is in `logcat-app.txt`):

1. `adb shell ls /sdcard/Android/data/app.suramak.spike/files/models` must be
   empty — otherwise the fresh-install claim would be vacuous (`-wipe-data`
   guarantees it, the check proves it).
2. At least one `SURAMAK-MODEL` line must exist.
3. **No** row line may contain `state=in_use` or `in_use=1`.
4. The last `header="…"` line must contain `not downloaded`.

The witness is the shell's own marker (egui paints pixels, so neither
uiautomator nor a screencap diff can read a row's action word), one line per
state change, plus a header line:

```
SURAMAK-MODEL header="small · not downloaded" model_id=small missing=1 can_record=0
SURAMAK-MODEL row=tiny state=not_downloaded in_use=0
SURAMAK-MODEL row=small state=not_downloaded in_use=0
```

The migration event is logged separately (`SURAMAK-MODEL migrate …`).

**Not run here.** No `/dev/kvm`/`emulator`/SDK on this host. What was
exercised: `bash -n` on the script, and the Python block extracted and fed
three synthetic logs — a correct fresh-install log passes; a log with
`state=in_use in_use=1` fails with the row named; a header without
`not downloaded` fails. That proves the assertion is load-bearing in both
directions, not that a phone ran.

---

## 6. Tests added (24)

| Test | What it pins |
|---|---|
| `core::models::a_model_counts_only_at_the_pinned_size_and_without_a_part` | Absent → WrongSize → Partial → (sweep) Verified; labels; unlink |
| `core::models::the_scan_covers_every_manifest_model_in_order` | one state per manifest row |
| `core::models::the_desktop_default_constant_names_the_default_settings_model` | `turbo-q5-0`, role `desktop default`, no `macOS` copy |
| `core::settings::the_model_choice_flag_defaults_to_false_and_round_trips` | old file reads `false`; round-trip; desktop default unchanged |
| `core::device::the_phone_default_follows_the_tier_at_4_6_8_12_16_gb` | the five tier defaults |
| `core::device::below_the_floor_the_default_is_still_tiny` | the existing below-floor steer |
| `core::device::the_migration_matrix` | all eight rows of §2.3 |
| `core::device::the_highest_verified_model_is_the_biggest_that_may_run` | allowed-only, refuses `turbo` files |
| `compact::model::the_tier_default_is_the_recommended_row_at_4_6_8_12_16_gb` | the UI's `recommended` flag follows the same table |
| `compact::model::the_five_row_states_are_reachable_from_disk_truth` | all five states, their actions and labels |
| `compact::model::in_use_is_impossible_without_a_verified_file` | exhaustive 4 × 2 × 4 matrix |
| `compact::model::the_header_row_names_the_configured_model_and_its_disk_truth` | `small` vs `small · not downloaded`; no borrowed file |
| `compact::model::a_partial_or_stale_file_does_not_unlock_recording` | card stays; Record stays off |
| `compact::model::an_unsupported_cpu_says_why_recording_is_blocked` | its own sentence |
| `compact::model::the_desktop_role_is_not_phone_copy` | role omission, subtitles |
| `ui_compact::a_fresh_phone_reads_not_downloaded_and_never_in_use` | a11y labels: `Download base` exists, no "In use"; after verification `In use base` |
| `ui_compact::a_phone_without_a_verified_model_cannot_tap_record` | the inline reason and the disabled control |
| `compact_host::a_fresh_install_takes_the_phone_tier_default_and_persists_it` | 4/6/8/12/16 GB, persisted, flag false, header/can_record honest |
| `compact_host::a_legacy_turbo_file_migrates_to_the_highest_installed_model` | installed beats tier default; survives relaunch |
| `compact_host::a_legacy_turbo_file_with_nothing_installed_takes_the_tier_default` | legacy → `small` at 8 GB |
| `compact_host::an_explicit_model_choice_is_never_overridden` | flag protects; a real pre-flag choice kept; Use sets the flag |
| `compact_host::a_missing_file_never_reads_as_in_use_on_the_host` | fresh, partial, then verified → `in_use` only at the end |
| `compact_host::a_wrong_sized_file_reads_as_stale_and_blocks_recording` | `stale_corrupt`; unlink |
| `compact_host::a_finished_download_becomes_the_model_the_user_asked_for` | adoption of the downloaded model; no switch away from a working one |

Screenshots (regenerated by the phone-viewport suite): `settings.png`,
`settings-tiers.png`, `settings-bottom-inset.png`, `first-run-download.png`,
`download-progress.png`, `download-resume.png`, plus the new
`settings-model-truth.png` (6 GB, configured `base`, no file: amber
`base · not downloaded`, primary `Download`, no "In use").

---

## 7. Verification

| Command | Result |
|---|---|
| `cargo test --workspace` | **1172 passed, 0 failed** (baseline 1148 + 24 new) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo fmt --all -- --check` | clean |
| smoke block, synthetic pass log | exit 0, prints the header and the observed states |
| smoke block, `in_use=1` log | exit 1, names the offending row |
| smoke block, header without `not downloaded` | exit 1 |
| `bash -n scripts/android/emulator-smoke.sh` | clean |

---

## 8. Boundaries and honest limits

- **No downloader rewrite.** `download::install_file` is still the only
  transfer; M13D's `.part` resume, checksum and atomic rename are untouched.
  The only addition is the one-line unlink of a known-bad dest before start.
- **No automatic downloads.** The migration changes a setting, never a file.
- **Desktop untouched.** `Settings::default()` still returns `turbo-q5-0`; the
  desktop never calls `model_choice_for_phone`; the `model_explicitly_chosen`
  key is simply written alongside the others.
- **The scan is size-based, not sha256-based.** A same-size bit-flip would not
  be caught until the next download or a failed load. Re-hashing on every
  device report was judged worse than the exposure; the report names it rather
  than implying cryptographic re-verification.
- **`log` moved from the Android-only dependency table to the crate's general
  dependencies** so the host tests (desktop) can log the same markers. It is
  the same crate and version already in the graph; `Cargo.lock` is unchanged.
- **Shot binaries** are regenerated; they are the suite's output, not edited by
  hand.
- The emulator smoke's first real run is CI's; this host cannot boot one.
