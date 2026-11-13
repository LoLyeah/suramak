# WORKORDER — M26A: limits that come from the device, not from my laptop

Design: `docs/reports/M26-memory-design.md` (Carbon high + parent
verification). The 45/60-minute caps are replaced by a **computed** limit and a
**model-fit gate**, so desktop and the owner's 12 GB phone stop hitting a wall
that belonged to a 3.7 GiB VPS, while a 4 GB phone is newly protected from a
model it can never load.

## Deliverables

1. **`compute_device_limits`** in `suramak-core` (pure, host-tested):
   inputs `total_bytes`, `available_bytes`, `low_ram`, `model_baseline_mb`,
   `model_slope_mb_per_min`, `free_disk_bytes`; outputs
   `Limits { max_minutes: u32, model_fits: bool, budget_mb: u32, reason: CapReason }`.
   ```text
   Android: low_ram || total <= 3 GiB -> reserve 1800 MB, frac 0.25
            total <= 4.5 GB           -> reserve 2100 MB, frac 0.30
            else                      -> reserve 2800 MB, frac 0.42
            B = min(total*frac, total-reserve); if available known, min(B, avail*0.65)
   Desktop: macOS  B = (total - 4096) * 0.60
            Linux  B = min(total*0.55, available*0.75)
            Windows B = min(total*0.55, avail_pagefile*0.65)
   B_safe = B * 0.85
   max_minutes = clamp(floor((B_safe - model_base) / model_slope), 0, 480)
   model_fits  = B_safe >= model_base + 15 * model_slope
   ```
   Model table: `tiny` base 310 MB / slope 9.2; `base` 380 / 9.2; `small` 700 /
   9.2; `turbo-q5-0` 1039 / 10.5; `turbo` 1900 / 10.5 (the last two are the
   measured/interpolated pairs from the design — keep them in ONE table with a
   comment naming the measurement).
2. **Platform providers** (no new crates): Android passes the existing
   `DeviceMemory.Report`; macOS `libc::sysctlbyname("hw.memsize")`; Linux parses
   `/proc/meminfo` (`MemAvailable`, fallback `MemTotal`); Windows a 15-line
   `GlobalMemoryStatusEx` FFI. A provider that fails returns `None` and the
   caller falls back to the conservative desktop default (60 min) **with a log
   line naming the failure** — never a silent guess.
3. **Wiring**: the queue's `session_too_long` refusal and the recording
   rotator's threshold both read `Limits.max_minutes`; the cap carries its
   reason and the numbers (`"this device budgets 4,284 MB; tiny needs 310 MB +
   9.2 MB per minute"`). Rotation still applies at the computed cap.
4. **Model-fit gate**: selecting or starting a model whose baseline exceeds
   the budget is refused **before** the job, with the exact numbers and one
   sentence naming the fix (`turbo-q5-0 needs ~1,039 MB; this device budgets
   1,020 MB — choose small or base`). No silent downgrade, ever.
5. **No infinite crash loop**: the job journal records `attempt_n` before the
   inference slot opens; a job that dies from an OS kill and is re-seen on
   launch with `attempt_n >= 2` is quarantined as `killed_by_os` instead of
   being resumed forever.

## Verification

- The design's table as unit tests: {4 GB low-RAM, 4 GB, 12 GB Vivo, 16 GB Mac,
  64 GB Mac} × {tiny, turbo-q5-0} — exact `max_minutes` and `model_fits`.
- Linux provider test against a fixture `/proc/meminfo` string; a failing
  provider yields the documented fallback and a log line.
- Queue tests: over-limit refusal carries the computed cap and reason; a
  recording at the computed cap rotates; the model-fit gate refuses before any
  decode; `attempt_n` quarantine after two OS kills.
- fmt/clippy -D warnings; NDK check + clippy on both Android crates (env
  `ANDROID_NDK`/`_HOME`/`_ROOT` + `CMAKE_GENERATOR=Ninja`); one full workspace
  run (baseline 1492).
- Report: `docs/reports/M26a-device-limits.md` with the table as measured by
  the code on this box and the three device classes.

## Do NOT

- No streaming decode, no mmap, no macro-chunking (Slices 2–3); no new crates;
  no changes to the decode/whisper path; no silent model downgrade; no CI
  edits; no git commit/push; no sudo.
