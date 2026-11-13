# M26A — limits from the device, not from my laptop

The M25B caps (45 min on Android, 60 min on desktop) were artifacts of the
3.7 GiB, 2-core VPS the measurements were taken on. This slice deletes those
constants and replaces them with a **computed** cap and a **model-fit gate**:

* a 12 GB phone and a 16–64 GB desktop stop hitting a wall that belonged to a
  different machine;
* a 4 GB phone is newly *protected* from `turbo-q5-0`, which needs ~1,039 MB
  before one minute of audio and would be killed by LMKD;
* a job the OS kills twice is quarantined instead of resumed forever.

Design: `docs/reports/M26-memory-design.md` §D. Work order:
`docs/WORKORDER-M26A.md`.

## The rule (one implementation, pure)

`suramak_core::limits::compute_device_limits(platform, total, available,
low_ram, model_base, model_slope, free_disk)` is a pure function; every input
is explicit, so the design's table is a unit test on any host.

```text
Android: low_ram || total <= 3 GiB -> reserve 1800 MB, frac 0.25
         total <= 4.5 GB           -> reserve 2100 MB, frac 0.30
         else                      -> reserve 2800 MB, frac 0.42
         B = min(total*frac, total-reserve); if available known, min(B, avail*0.65)
macOS    B = (total - 4096 MB) * 0.60
Linux    B = min(total*0.55, available*0.75)
Windows  B = min(total*0.55, avail_pagefile*0.65)
B_safe   = B * 0.85
max_minutes = clamp(floor((B_safe - model_base) / model_slope), 0, 480)
model_fits  = B_safe >= model_base + 15 * model_slope
```

Byte counts are converted to **decimal MB** (`bytes / 1,000,000`), the unit the
design's worked table and `/proc/meminfo` use. Free temp disk adds the design's
Phase-2 bound, `floor((free - 500 MB) / 3.84 MB per minute)`; when it is the
tighter bound the reason is `disk`.

`Limits` is exactly the work-order shape: `{ max_minutes, model_fits,
budget_mb, reason }`, with `reason ∈ {memory, disk, ceiling, model too large,
fallback}`.

## The model table (one table, each row's measurement named)

| model | baseline MB | slope MB/min | provenance |
|---|---|---|---|
| `tiny` | 310 | 9.2 | measured 308.4 MB intercept (M25A), measured 9.19 MB/min slope |
| `base` | 380 | 9.2 | design §D.1 table, 80-mel slope (no direct intercept measured) |
| `small` | 700 | 9.2 | design §D.1 table, 80-mel slope (no direct intercept measured) |
| `turbo-q5-0` | 1,039 | 10.5 | measured 1,038 MB intercept on the 574 MB q5_0 file; 128-mel slope |
| `turbo` | 1,900 | 10.5 | design §D.1 full-precision `large-v3-turbo` pair (128-mel) |

`MODEL_FOOTPRINTS` in `crates/suramak-core/src/limits.rs` is the only copy; the
session cap, the queue gate, the recorder rotator and the Android picker all
read it.

## The computed device × model table

Measured by the code on this box (`cargo test -p suramak-core --lib
limits::tests::the_design_device_table_is_exact -- --nocapture`). Phones are
byte totals (4,000,000,000 / 12,000,000,000); Macs are 16 GiB / 64 GiB. Free
disk is 64 GiB so the disk rule does not bind. Available memory is unwired for
the phone rows, which is how the design's arithmetic (`min(5040, 9200)`) is
written; production wires `availMem` and takes the tighter `avail * 0.65`
bound.

| device | total | model | budget MB | max_minutes | model_fits | reason |
|---|---|---|---|---|---|---|
| 4 GB low-RAM | 4,000 MB | tiny | 850 | 58 | true | memory |
| 4 GB low-RAM | 4,000 MB | turbo-q5-0 | 850 | 0 | false | model too large |
| 4 GB | 4,000 MB | tiny | 1,020 | 77 | true | memory |
| 4 GB | 4,000 MB | turbo-q5-0 | 1,020 | 0 | false | model too large |
| 12 GB Vivo X300 | 12,000 MB | tiny | 4,284 | 431 | true | memory |
| 12 GB Vivo X300 | 12,000 MB | turbo-q5-0 | 4,284 | 309 | true | memory |
| 16 GB Mac | 17,179 MB | tiny | 6,672 | 480 | true | ceiling |
| 16 GB Mac | 17,179 MB | turbo-q5-0 | 6,672 | 480 | true | ceiling |
| 64 GB Mac | 68,719 MB | tiny | 32,957 | 480 | true | ceiling |
| 64 GB Mac | 68,719 MB | turbo-q5-0 | 32,957 | 480 | true | ceiling |

The design's rows for 4 GB (77 / 0), 12 GB (431 / 309) and both Macs
(480 / 480) reproduce exactly. Two honest deltas, both noted rather than
hidden:

* **4 GB low-RAM.** The design's table cell says "60 min" in parentheses, but
  its own formula gives `floor((850 - 310) / 9.2) = 58`. The code follows the
  formula; the 60 was the old static cap the slice exists to delete.
* **16 GB Mac budget.** The design's table prints 7,373 MB, which is
  `(16,384 - 4,096) * 0.60` before the 0.85 safety factor and uses MiB for the
  total. The work order's formula block applies `B_safe = B * 0.85` to every
  platform; the code does that, in decimal MB, so the budget is 6,672 MB. The
  cap is 480 either way (the 8-hour ceiling binds).

## This box, measured

`cargo test -p suramak-core --lib limits::tests::the_live_probe -- --nocapture`:

```text
M26A_HOST platform=linux total_bytes=3899420672 available_bytes=Some(2503565312)
low_ram=false free_disk_bytes=Some(9830338560) budget_mb=1596 max_minutes=139
model_fits=true reason=memory
```

`cargo test -p suramak-core --lib session::tests::the_cap_is_computed -- --nocapture`
(the shipping path, with the default configured model `turbo-q5-0`):

```text
M26A_SESSION model=turbo-q5-0 cap_model=turbo-q5-0 cap_ms=3660000
limits=Limits { max_minutes: 61, model_fits: true, budget_mb: 1688, reason: Memory }
```

The old desktop constant was 60 minutes. This 3.9 GB box computes 139 minutes
on `tiny` and 61 on `turbo-q5-0` at that moment; the 60-minute wall was its own
artifact. `MemAvailable` moves between processes, so the two numbers differ by
the memory the box had free when each test ran — the cap is resolved once per
process by design, so a recording's threshold and its part sizes cannot
disagree.

When the configured model cannot fit at all (for example the full `turbo` on
this box), the **job** is refused by the model-fit gate; the recording cap is
then computed with the largest model that does fit, so a phone still rotates
its parts at a usable size instead of 0 minutes. The substitution is logged
with both model ids.

## Wiring

| where | what it reads |
|---|---|
| `session::session_limits()` / `session_cap_ms()` | the host probe (or the Android report), the configured model, free temp disk — resolved once per process |
| queue `start_inference` | `session::cap_refusal_for(duration, Limits, model)` → `session_too_long` with the budget and the model's cost |
| queue `enqueue_import` | splits at the same computed cap |
| recorder rotator (`LiveSplitTracker`, `RotationPolicy::phone()`, `write_split_wavs`) | `session_cap_ms()` — rotation at the computed cap (plus the 120 s grace at a real pause) |
| queue `refuse_model_fit` (both `start_decode` and `start_inference`) | `limits::model_fit_refusal` — **before any decode** |
| Android `device::refusal_for` / `budget_refusal` | the same fit gate on model selection and download |
| `jobs.attempt_n` (schema v14) | incremented before the inference slot; `attempt_n >= 2` on launch → `killed_by_os` quarantine |

### The refusal copy, verbatim

```text
session_too_long: this session is 77:00, over the 77:00 cap (memory); this
device budgets 1,020 MB; tiny needs 310 MB + 9.2 MB per minute — split it into
shorter parts

model_too_large: turbo-q5-0 needs ~1,039 MB; this device budgets 1,020 MB —
choose small or base
```

The model gate never swaps the model: it refuses and names the largest
models that fit. The session refusal carries the rule that set the cap
(`memory`, `disk`, `ceiling`, `model too large`, `fallback`).

### Crash-loop quarantine

`jobs.attempt_n` is written in the same SQLite journal, before the inference
slot opens. A process killed by the OS leaves its row `decoding` /
`transcribing` with the attempt counted. On the next launch
`QueueDriver::resume_pending` sees `attempt_n >= 2`, writes
`failed / killed_by_os: … after 2 attempts without a checkpoint; quarantined
instead of resumed`, and does **not** enqueue the job. The row is terminal, so
no launch can pick it up again; the checkpointed text stays in the library.

## Providers (no new crates)

* **Linux / Android** — `/proc/meminfo`: `MemTotal` and `MemAvailable`
  (falling back to `MemTotal` on kernels without it). Parsed by
  `parse_meminfo`, pinned by a fixture test and exercised live.
* **Android** — the existing `DeviceMemory.Report` (`totalMem`, `availMem`,
  `isLowRamDevice`) is handed to `session::set_device_probe` from both Android
  hosts; the Java `low_ram` flag wins over the byte count, exactly as the
  design says.
* **macOS** — `libc::sysctlbyname("hw.memsize")`.
* **Windows** — a raw `extern "system" GlobalMemoryStatusEx` over the
  `MEMORYSTATUSEX` layout; `ullTotalPhys` + `ullAvailPageFile`, the design's
  commit-limit bound.
* **Failure** — the provider returns `Err(cause)`;
  `limits::limits_from_probe` logs
  `device memory probe failed; using the conservative 60-minute cap` with the
  platform, the model and the cause, and returns
  `Limits { 60, model_fits: true, budget_mb: 0, reason: fallback }`. Never a
  silent guess.

## Verification

* `crates/suramak-core/src/limits.rs` — 12 tests: the design table exactly,
  the model-fit sentence, the Linux fixture (with and without
  `MemAvailable`), a missing file's named error, the fallback **and its log
  line** (captured through a test subscriber), the disk rule, the 8-hour
  ceiling, the one-table pin, the largest-fitting model, a zero-byte probe, the
  copy's thousands separators, and the live provider on this host.
* `crates/suramak-core/src/session.rs` — the cap is per device, exactly at the
  computed cap is accepted / 1 ms over is refused with the budget, the refusal
  carries the rule and the model's cost, the session refusal steps aside when
  the model cannot fit, and a recording at the computed cap rotates there
  (both the file plan and the live tracker).
* `crates/suramak-asr/src/queue/driver.rs` — over-limit refusal carries the
  computed cap (77 min on a 4 GB phone), at-cap acceptance, the model gate
  refusing **with the prefetch pool never reading the file**, and
  `attempt_n` quarantine after two OS kills (two launches against one journal).
* `crates/suramak-core/src/store.rs` — `attempt_n` round-trip, increment,
  never reset by a re-bind, and the `killed_by_os:` quarantine ends
  resumability.
* `crates/suramak-asr/src/capture_stream.rs` — the streaming rotator's policy
  is the computed cap.

No streaming decode, no `mmap`, no macro-chunking, no new crates, no CI edits,
no model downgrade.

| check | result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `cargo ndk -t arm64-v8a --platform 29 check -p suramak-android -p suramak-android-spike --locked` | exit 0 |
| `cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-android -p suramak-android-spike --all-targets -- -D warnings` | exit 0 |
| `cargo test --workspace` | **1,509 passed, 0 failed, 13 ignored** (36 binaries; baseline 1,492) |

The UI snapshot tests rewrite their PNGs on every run; the restored files
carry no content change (the settings screen's model list is identical).

## Files

* `crates/suramak-core/src/limits.rs` — new: the formula, the model table, the
  providers and the refusal copy.
* `crates/suramak-core/src/session.rs` — the computed cap and its refusal.
* `crates/suramak-core/src/device.rs` — the phone-level budget gate.
* `crates/suramak-core/src/store.rs` — schema v14 `jobs.attempt_n`,
  `record_job_attempt`, `quarantine_job_killed_by_os`.
* `crates/suramak-asr/src/queue/driver.rs` — the model gate, the computed
  session cap, the attempt counter and the quarantine.
* `crates/suramak-asr/src/capture_stream.rs` — `RotationPolicy::phone()`.
* `crates/suramak-android/src/{model_host,compact_host}.rs` — the Android
  report feeds `session::set_device_probe`.
* UI/CLI resume surfaces — the `Quarantined` outcome is reported.
