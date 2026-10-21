# AUDIT-BATCH-3 — audit pass 2 closed out

**Date**: 2026-10-21
**Workorder**: `docs/WORKORDER-AUDIT-BATCH-3.md` (authoritative). Spec per finding: `docs/reports/AGY-AUDIT-2-2026-10-21.md`.
**Tree**: `main` at `e21545a` (post-`e71b47b`), modified in place — no commit, no push.
**Rail check**: no new dependencies (no `Cargo.toml` dependency edits at all), nothing outside `/home/ubuntu/suramak`, `.github/workflows/ci.yml`, the workspace `Cargo.toml` and the Android Gradle build untouched.

## Verdicts at a glance

| # | Finding | Severity | Verdict | Test |
|---|---|---|---|---|
| 1 | SUR2-NEW-03 — in-process encoder-hide race | Medium | **FIXED** | `concurrent_hides_of_the_same_encoder_are_mutually_exclusive` |
| 2 | SUR-MED-05 — non-atomic `settings.toml` | Medium | **FIXED** | `settings_save_replaces_the_file_atomically_and_leaves_no_temp_behind` |
| 3 | SUR-MED-02 — `_exit` skips the WAL checkpoint | Medium | **FIXED** | `checkpoint_drains_the_wal_into_the_database_file` |
| 4 | SUR2-NEW-04 — non-cancellable `LiveDecoder` window | Medium | **FIXED** | `teardown_trips_the_token_the_live_window_inference_honours`, `dropping_the_decoder_also_trips_the_inference_token` |
| 5 | SUR-MED-01 — unbounded unzip, PID temp collision, prefix detection | Medium | **FIXED** | `deep_nesting_extracts_but_an_oversized_expansion_is_refused`, `a_macos_junk_first_entry_does_not_become_the_extraction_prefix`, `an_archive_with_two_top_level_directories_is_rejected`, `temp_siblings_are_unique_within_one_process` |
| 6 | SUR-MED-03 — Parakeet FFI `len as i32` | Medium | **FIXED** | `the_ffi_sample_count_is_bounds_checked_not_wrapped` |
| 7 | SUR-MED-04 — playback resampler zero rate / 32-bit overflow | Medium | **FIXED** | `a_zero_rate_is_refused_instead_of_pretending_the_audio_was_resampled`, `resample_capacity_survives_empty_and_integer_extreme_lengths` |
| 8 | SUR-LOW-05 — asymmetric signed PCM normalization | Low | **FIXED** | `signed_pcm_normalization_is_symmetric_at_full_scale` |
| 9 | SUR2-NEW-05 — standalone `upsert_segments` autocommit / stale `updated_at` | Low | **FIXED** | `a_standalone_upsert_touches_updated_at_and_keeps_the_insert_stamp`, `a_failed_standalone_upsert_rolls_back_to_the_previous_segments` |
| 10 | SUR-LOW-06 — unchecked Win32 SID parse | Low | **FIXED** | Windows probe compile (`IsValidSid` path); no host-runnable unit test |
| 11 | SUR-LOW-02 — unescaped `LIKE` wildcards | Low | **FIXED** | `search_treats_wildcards_as_literal_text` |
| 12 | SUR-LOW-01 — resampler error typed as `Polish` | Low | **FIXED** | `an_impossible_rate_is_reported_as_a_resampler_error` |
| 13 | SUR-LOW-04 — silence appended to the resampled tail | Low | **FIXED** | `the_resampled_length_is_exactly_the_input_duration` |
| 14 | SUR2-NEW-06 — `mac-check-dylibs.sh` scope + whitespace split | Low | **FIXED** | `mac_check_dylibs_scans_frameworks_and_keeps_paths_with_spaces_whole` |
| 15 | SUR2-NEW-07 — texture-cache leak on surface error | Low | **FIXED** | Android-only source; not host-compiled (see §15) |
| 16 | SUR-LOW-03 — unqualified `explorer.exe` | Low | **FIXED** | `the_explorer_executable_is_qualified_with_the_system_root` |
| 17 | SUR-LOW-08 — brittle plist key match | Low | **FIXED** | `plist_string_tolerates_whitespace_inside_the_key_tags` |
| 18 | SUR-LOW-09 — `codesign --deep` | Low | **FIXED** | `make_app_embeds_and_signs_the_framework_only_when_the_feature_is_on` (extended) |
| 19 | SUR-MED-06 — fixed `/tmp` payload in `ask_glm.py` | Medium (report: Low) | **FIXED** | tooling: `py_compile` + static check (no harness — see §19) |
| 20 | SUR-LOW-07 — external `winit` git patch | Low | **WONTFIX** | — (reason in §20) |

**Leftover: 0 unresolved.** 19 FIXED, 1 WONTFIX with a stated reason. Two pass-2 findings were already closed by `e71b47b` before this batch started and were re-verified in source, not re-fixed: **SUR2-NEW-01** (`return Err` on HTTP 416 + `a_stale_complete_part_is_removed_on_416_not_locked_out_forever`) and **SUR2-NEW-02** (`[i16]` capture normalization `/32768.0` + clamp + its test).

---

## Mediums

### 1. SUR2-NEW-03 — in-process encoder-hide race → FIXED

**Re-derived**: the report's scenario is real. `hidden_encoder_path`'s `SEQ` makes concurrent *hides* disjoint, but nothing serialises the directory both hides target, so thread B's Core ML state init can run while thread A has the package hidden (silent downgrade to Metal/CPU for the session), and a second hide started in that window stashes nothing, so A's restore lands under B.

**Change** — `crates/suramak-asr/src/whisper.rs`:
- new `encoder_fs_lock()` (line 294): `static Mutex<()>` with poison recovery, since the guarded work is `rename` + an RAII restore.
- `with_encoder_hidden` (line 305) takes the guard **first** — declared before the `StashedEncoder`, so the restore (which drops first) still runs inside the lock.
- `create_state_for_plan`'s Core ML branch (line 443) holds the same lock around `create_state_probed`, in its own scope so the fallback retry can take it again through `with_encoder_hidden` (no re-entrancy, no deadlock).

**Test**: `concurrent_hides_of_the_same_encoder_are_mutually_exclusive` — two threads on one model file, an `AtomicUsize` peak-concurrency counter (must stay 1) and, in each closure, "the encoder is still hidden for my whole critical section" (the assertion that fails without the lock, because the other thread restores mid-closure).

### 2. SUR-MED-05 — non-atomic `settings.toml` → FIXED

**Change** — `crates/suramak-core/src/settings.rs:323`: `save` serialises first, writes `<name>.<pid>.tmp` **in the same directory** (`temp_sibling`, line 341), renames over the target, and removes the temp if the rename fails. `fsync` is deliberately skipped and the choice documented: the gap closed is a torn write at process death, not power-loss durability, and cross-filesystem renames are not atomic — hence the sibling, not the system temp dir.

**Test**: `settings_save_replaces_the_file_atomically_and_leaves_no_temp_behind` — two saves over an existing file, the result parses as TOML with the new values, and the directory contains exactly `settings.toml`.

### 3. SUR-MED-02 — `_exit` skips destructors/WAL checkpoint → FIXED

**Scope discipline**: `libc::_exit` stays (deliberate ggml-teardown workaround). Only the checkpoint/close gap is fixed.

**Change**:
- `Store::checkpoint()` (`store.rs:183`) runs `PRAGMA wal_checkpoint(PASSIVE)` through `query_row` (stepping the row is what executes it) and returns a `Checkpoint { busy, log_frames, checkpointed_frames }` (`store.rs:16`) so the outcome can be asserted and logged. `PASSIVE` never blocks on readers — right for a bounded teardown; it is a no-op success for in-memory/non-WAL databases.
- `Runtime::shutdown` (`runtime.rs:1185`) takes the store, checkpoints it, warns on failure and drops the connection **before** `hard_exit` — so SQLite's own close-time checkpoint also runs instead of being skipped by `_exit`.

**Test**: `checkpoint_drains_the_wal_into_the_database_file` — a file-backed store, a committed transcript, a non-empty WAL, then `checkpointed_frames == log_frames` for the explicit call and for a repeat, data readable from a fresh connection, and the in-memory no-op path exercised.

### 4. SUR2-NEW-04 — non-cancellable `LiveDecoder` window → FIXED

**Change** — `crates/suramak-asr/src/live.rs`:
- `LiveDecoder` owns a `CancelToken` (line ~575) and a clone rides to the decode thread (`run_decoder(..., cancel)`).
- `cancel_token()` (line 611) hands out clones for tests/callers.
- `decode_window(..., cancel)` (line 759) passes that token to `whisper::transcribe` instead of `&CancelToken::new()`; whisper checks it on its abort callback between chunks.
- `shutdown_and_wait` (line 684) and `Drop` (line 714) both trip it *before* waiting/detaching, so the quit path's 250 ms grace is enough for a window mid-inference to return and drop its context.

**Tests**: `teardown_trips_the_token_the_live_window_inference_honours` (fresh decoder uncancelled, every handle shares the one flag, `shutdown_and_wait` trips it) and `dropping_the_decoder_also_trips_the_inference_token` (the Stop path does too).

### 5. SUR-MED-01 — zip bomb cap + PID collision + prefix detection → FIXED

**Change** — `crates/suramak-core/src/download.rs`:
- `MAX_EXTRACTED_BYTES = 5 GiB` (line 288), documented against the real archive set (encoder packages, 1–2 GB expanded) and enforced across the **whole** archive, not per entry.
- `extract_entries(..., budget_bytes)` (line 379) reads each entry through `Read::take(remaining + 1)` — the archive's declared sizes are not trusted — and fails as soon as an entry wants more than the remaining budget.
- `archive_prefix` (line 342) replaces "first entry wins": it ignores `__MACOSX/…` and root-level files and requires **one** shared top-level directory. An archive with several roots is rejected rather than half-extracted — `download_zip`'s layout cannot represent two packages, so picking one would silently misroute the install.
- `temp_sibling` (line 461) appends a process-wide `SEQ`, so two concurrent extractions into the same parent cannot share (and delete) each other's staging dir.

**Tests** (all in `download.rs`'s unit module; the integration file's stale-temp assertion was updated to the new shape): a `__MACOSX`-first archive now extracts the real package; a two-root archive is rejected with nothing on disk; a 64 KiB-of-zeros mini-bomb under a 200-deep path is refused under a 32 KiB budget while the same archive extracts fine under 1 MiB; two `temp_sibling` calls differ.

### 6. SUR-MED-03 — Parakeet FFI integer overflow → FIXED

**Change** — `crates/suramak-asr/src/parakeet.rs`: the length is validated **before** any FFI call (`sample_count_i32`, line 496) so an oversized buffer needs no stream cleanup; `samples.len() as i32` is gone.

**Test**: `the_ffi_sample_count_is_bounds_checked_not_wrapped` — exact at `i32::MAX`, `Err` at `i32::MAX + 1` and at `u32::MAX` (the value that used to wrap to −1).

### 7. SUR-MED-04 — playback resampler zero rate / overflow → FIXED

**Change** — `crates/suramak-playback/src/engine.rs`:
- `resample_mono` (line 500) now errors on a zero rate instead of passing the input through as if it had been resampled; the legitimate `from == to` pass-through stays.
- capacity comes from `resample_capacity` (line 537), computed in `u64` with saturating/checked arithmetic and a `usize::try_from` fallback to the input length (the capacity is only a hint), so `len * to` can no longer wrap a 32-bit `usize`.

**Tests**: zero-rate variants all error and `from == to` still copies; `resample_mono(&[], …)` is empty; a 60 M-frame 44.1→16 kHz case equals the u64-computed frame count; integer-extreme input lands on one of the two documented outcomes instead of panicking.

---

## Lows

### 8. SUR-LOW-05 — asymmetric signed PCM normalization → FIXED

**Change** — `crates/suramak-core/src/decode.rs`: `normalize_pcm(sample, full_scale)` (line 240) divides by the **negative** extreme's magnitude and clamps to `[-1, 1]`; applied to all four signed widths (S8 128, S16 32 768, S24 2^23, S32 2^31) — fixing only S16 would have left the same defect in the other three. Unsigned paths already used the symmetric constant.

**Test**: `signed_pcm_normalization_is_symmetric_at_full_scale` — `MIN → −1.0` and `MAX → just under +1.0` for every width, silence stays 0, overshoot clamps.

### 9. SUR2-NEW-05 — standalone `upsert_segments` autocommit / stale `updated_at` → FIXED

**Change** — `crates/suramak-core/src/store.rs`:
- `write_segments` (line 151) is the row writer (inserts + stale-tail delete); `insert_transcript` calls it directly so the caller's `updated_at` remains authoritative on insert.
- `upsert_segments` (line 125) wraps `write_segments` + `UPDATE transcripts SET updated_at` in a `SAVEPOINT`/`RELEASE` with `ROLLBACK TO` on error. A savepoint, not `BEGIN`, because this is also the path `insert_transcript` can nest inside (it no longer does, but the public method stays safe to nest).

**Tests**: a standalone upsert touches `updated_at` (and insert keeps the caller's stamp); a trigger that aborts the second row leaves both previous rows intact — i.e. the first, already-written row is rolled back.

### 10. SUR-LOW-06 — unchecked Win32 SID parsing → FIXED

**Change** — `crates/suramak-ui/src/instance_windows.rs:120`: `IsValidSid(PSID(user.User.Sid.0))` is called before `read_sid`, returning an `io::Error` on failure; `read_sid`'s safety contract now states it requires a `IsValidSid`-validated pointer (its count is one byte, so the `8 + 4*count` walk cannot overflow).

**Test**: no host-runnable test — the file is `cfg(windows)`. Verified by compiling the real source for `x86_64-pc-windows-msvc` through the M14b probe crate (`build/probe-m14b`, `clippy --all-targets -D warnings`, clean), which is how this file was checked when it was written.

### 11. SUR-LOW-02 — unescaped `LIKE` wildcards → FIXED

**Change** — `store.rs:346`: `\`, `%` and `_` are escaped and both `LIKE`s say `ESCAPE '\'`, so a search is a literal substring.

**Test**: `search_treats_wildcards_as_literal_text` (integration, `crates/suramak-core/tests/store.rs`) — `100%` matches only `100% done` (not `100 items done`), `a_b` only `a_b notes`, and a backslash query still matches.

### 12. SUR-LOW-01 — resampler error typed as `Polish` → FIXED

**Change** — `SuramakError::ResamplerInit(String)` (`error.rs:37`) and `to_16khz_mono` maps the `FftFixedIn` construction error to it (`resample.rs:26`).

**Test**: `an_impossible_rate_is_reported_as_a_resampler_error` — a 0 Hz buffer returns `ResamplerInit`, not `Polish` (also pins that rubato errors instead of panicking on rate 0).

### 13. SUR-LOW-04 — artificial silence at the resampled tail → FIXED

**Change** — `resample.rs`: after the chunk loop the output is truncated to `round(mono.len() * 16 000 / rate)` — the input's real duration.

**Test**: `the_resampled_length_is_exactly_the_input_duration` — a 0.5 s 44.1 kHz sine yields exactly 8 000 samples; a non-chunk-aligned clip never *gains* samples. (Rubato's FFT latency can still leave a very short clip slightly under its ideal length; that is pre-existing and the opposite direction from this finding — noted in the test.)

### 14. SUR2-NEW-06 — `mac-check-dylibs.sh` scope + whitespace splitting → FIXED

**Change** — `scripts/mac-check-dylibs.sh`: discovery now walks `Contents/MacOS` **and** `Contents/Frameworks` (`find -print0` + `read -d ''`), filters to Mach-O via `file -b` (frameworks also hold plists/headers), and parses `otool -L` with `sed 's/^[[:space:]]*\(.*\) (compatibility version .*$/\1/p'` — the trailing annotation is stripped, the path is not split, so a spaced path survives. Output labels are bundle-relative, and the summary reports the number of binaries checked.

**Test**: `mac_check_dylibs_scans_frameworks_and_keeps_paths_with_spaces_whole` — a static contract test (the same pattern as the existing `make-app.sh`/`fetch-sparkle.sh` tests): Frameworks root present, Mach-O filter present, no whitespace-splitting parser, NUL-separated paths.

### 15. SUR2-NEW-07 — texture-cache leak on surface error → FIXED

**Change** — `crates/suramak-android-spike/src/app.rs:595`: the `textures_delta.free` loop moved from after `frame.present()` to immediately after the `textures_delta.set` upload, i.e. before the swapchain acquisition that can `return` early. The free list is one-shot, so the early return used to strand those textures in the renderer for the life of the surface. Freeing there is safe: egui only frees ids no live paint job references.

**Test**: none possible on this host — `app.rs` is `cfg(target_os = "android")` and the Android deps need the NDK (not installed here; the APK was built elsewhere). The change is a 3-line move within one function; `cargo fmt --all` parses the file and the host build of the crate is clean. Flagged as the one fix in this batch with no host-side verification beyond parsing.

### 16. SUR-LOW-03 — unqualified `explorer.exe` → FIXED

**Change** — `crates/suramak-ui/src/reveal.rs:44`: `explorer_exe(SystemRoot)` builds `%SystemRoot%\explorer.exe`; the bare name remains only as the no-`SystemRoot` fallback. The Windows spawn uses it.

**Test**: `the_explorer_executable_is_qualified_with_the_system_root` — qualified path for a system root, bare name for `None`/empty. Also compiled for Windows through the probe.

### 17. SUR-LOW-08 — brittle plist key match → FIXED

**Change** — `crates/suramak-ui/src/updates.rs:96`: `plist_string` scans `<key …>` elements, trims the key text before comparing, tolerates a multi-line key, then still requires the next element to be `<string>` (a key whose value is an array/dict returns `None`, as documented).

**Test**: `plist_string_tolerates_whitespace_inside_the_key_tags` — `<key> CFBundleName </key>` and a multi-line `SUFeedURL` both read; a missing key still returns `None`. The existing compact-spelling test and the shipped-`Info.plist` tests still pass.

### 18. SUR-LOW-09 — deprecated `codesign --deep` → FIXED

**Change** — `scripts/make-app.sh`: nested code under `Contents/Frameworks` is signed explicitly, deepest first (`find -depth` over `*.xpc`, `*.app`, `*.dylib`, `*.framework`), then the app bundle is signed **without** `--deep`. Header comment updated.

**Test**: the existing `make_app_embeds_and_signs_the_framework_only_when_the_feature_is_on` asserted `codesign --force --deep --sign -` was present — it pinned the deprecated form, so it was updated: no `codesign … --deep` invocation, `find … -depth`, the per-nested `codesign --force --sign - "$nested"`, the outer `codesign --force --sign - "$APP"`, and embed-before-sign ordering.

### 19. SUR-MED-06 — fixed `/tmp` payload in `scripts/ask_glm.py` → FIXED

**Change** — the `PAYLOAD = "/tmp/glm_payload.json"` constant and the `open(...)` write are gone; the body is serialised straight to curl on stdin (`--data-binary @-` with `input=json.dumps(body)`), which is the report's first recommended option and removes the predictable path (and the temp file) entirely.

**Verification**: `python3 -m py_compile scripts/ask_glm.py` clean; `grep` confirms no `PAYLOAD` and no `/tmp/glm` reference remain. There is no Python test harness in this repo and the module's import performs the API call, so this tooling fix has a static check rather than a unit test — stated plainly rather than dressed up.

### 20. SUR-LOW-07 — external personal `winit` git patch → WONTFIX

**Reason**: the finding's premise does not hold in this tree. `Cargo.toml:72` pins the patch to an exact commit:

```toml
winit = { git = "https://github.com/LoLyeah/winit", rev = "63f21f8e189d32852f0c0629f866e3292631371a" }
```

`Cargo.lock` records the same rev, so a build is reproducible and a moved branch cannot change it. The remaining risk is the repository disappearing or being force-pushed away from that rev — and the recommended remedy (vendor the fork into `crates/winit-patch`) would add a full winit fork to the tree and requires editing workspace dependency declarations, both of which this batch's rails forbid. The patch itself is load-bearing for the macOS 27 IME nil-context crash (`-[NSView inputContext]` → `.expect("input context")` panic) with no upstream fix in the 0.30 line, so dropping it is not an option either. Re-open if the rails ever allow vendoring, or when upstream covers the nil case.

---

## Verification (real runs, this tree)

```text
$ cargo fmt --all --check
FMT_CLEAN

$ cargo clippy --workspace --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.78s          ← exit 0, zero warnings

$ cargo test --workspace
passed: 532   failed: 0   ignored: 8                                          ← 511 baseline + 21 new

$ cargo test -p suramak-asr --features capture,capture-test
test result: ok. 109 passed; 0 failed; 1 ignored     (lib)
test result: ok. 6 passed; 0 failed; 7 ignored       (integration: worker/pipeline)
                                                                              ← exit 0

$ cargo check -p suramak-glass -p suramak-taps --target x86_64-pc-windows-msvc
Finished `dev` profile … in 11.20s                                            ← exit 0

$ (build/probe-m14b) cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings
Checking probe-m14b v0.0.0 … Finished … in 0.14s                              ← exit 0
   (real instance.rs / instance_windows.rs / reveal.rs, incl. their tests)

$ cargo check -p suramak-android-spike
Finished `dev` profile … in 5.91s                                             ← exit 0 (host; app.rs is Android-only)

$ bash -n scripts/mac-check-dylibs.sh && bash -n scripts/make-app.sh   → SH_SYNTAX_OK
$ python3 -m py_compile scripts/ask_glm.py                             → PY_OK
```

**One observed flake, not a regression**: the first `--features capture,capture-test` run failed
`worker::tests::a_cancel_queued_with_the_job_is_observed_before_the_model_load` (its 10 s bounded wait
expired) while the box was still busy with the workspace suite. It passed 3/3 in isolation immediately
after, and the full feature suite passed on re-run (109/109). `worker.rs` is untouched by this batch and
the test's own comment records that this path has wedged before on this 2-core box.

## Files changed

| File | Findings |
|---|---|
| `crates/suramak-asr/src/whisper.rs` | SUR2-NEW-03 |
| `crates/suramak-asr/src/live.rs` | SUR2-NEW-04 |
| `crates/suramak-asr/src/parakeet.rs` | SUR-MED-03 |
| `crates/suramak-core/src/download.rs` | SUR-MED-01 |
| `crates/suramak-core/src/settings.rs` | SUR-MED-05 |
| `crates/suramak-core/src/store.rs` | SUR-MED-02, SUR2-NEW-05, SUR-LOW-02 |
| `crates/suramak-core/src/error.rs`, `resample.rs` | SUR-LOW-01, SUR-LOW-04 |
| `crates/suramak-core/src/decode.rs` | SUR-LOW-05 |
| `crates/suramak-core/tests/{store,download}.rs` | SUR-LOW-02, SUR-MED-01 (assertion shape) |
| `crates/suramak-playback/src/engine.rs` | SUR-MED-04 |
| `crates/suramak-ui/src/runtime.rs` | SUR-MED-02 (quit wiring) |
| `crates/suramak-ui/src/instance_windows.rs` | SUR-LOW-06 |
| `crates/suramak-ui/src/reveal.rs` | SUR-LOW-03 |
| `crates/suramak-ui/src/updates.rs` | SUR-LOW-08, SUR2-NEW-06 + SUR-LOW-09 contract tests |
| `crates/suramak-android-spike/src/app.rs` | SUR2-NEW-07 |
| `scripts/ask_glm.py` | SUR-MED-06 |
| `scripts/mac-check-dylibs.sh` | SUR2-NEW-06 |
| `scripts/make-app.sh` | SUR-LOW-09 |

Untouched on purpose: `.github/workflows/ci.yml`, workspace `Cargo.toml`/`Cargo.lock` deps, `android/`
(Gradle), `crates/suramak-cli` (its `polish.rs` call site needs no change — the store method now does
the right thing), and every test that was already pinning behaviour unrelated to these findings.
