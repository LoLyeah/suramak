# M26 — the memory design (why the caps were wrong, and what replaces them)

Owner's question: *"It is killed because the emulator only have 2gb of ram,
isn't it? Meanwhile real android have more than 4gb ram. Also why desktop was
limited? You ask carbon high for a solution."*

Carbon high (gemini-4-carbon, thinking 8192) answered; this document is the
design, with the parent's verification of every claim it depends on.

## The short answer

**The caps were artifacts.** They came from a 3.7 GiB 2-core VPS and an assumed
1 GiB process budget — not from any device the app runs on. A static 45-minute
cap on a 12 GB phone and a 60-minute cap on a 16–64 GB Mac are indefensible.

The real constraint differs per platform:

| Platform | Real binding constraint |
|---|---|
| Android | LMKD + ZRAM, **not** the Java heap. Native allocations bypass ART; what kills the app is anonymous RSS under memory pressure — and it is far tighter while the app is a background service (screen off) than in the foreground. |
| macOS | No per-process cap; dynamic NVMe swap. Metal's working-set limit affects model weights, not PCM. |
| Windows | Commit limit (RAM + pagefile), generous unless the pagefile was disabled. |
| Linux | Overcommit + earlyoom; ~60 % of `MemAvailable` is the sane ceiling. |

## Why `mmap` alone does not fix it

The measured slope is 9.19 MB/min. Decomposed: 3.84 MB/min PCM, ~3.84 MB/min
transient resample/vector slack, ~1.92 MB/min mel (`n_mel = 80`; **128 for
turbo**, i.e. 3.07 MB/min). `whisper.cpp` converts the **whole** passed slice
to mel up front into an anonymous `std::vector<float>` — that allocation cannot
be `mmap`-ed without forking whisper. Mapping the PCM file makes the samples
OS-reclaimable (real, but partial); the mel vector is still anonymous.

## What replaces the caps

1. **Slice 1 (now): device-derived limits.** Replace the constants with
   `compute_device_limits(total, available, low_ram, model, free_disk)` →
   `max_minutes` + a **model-fit gate**. Desktop and the 12 GB Vivo get their
   real ceiling today; a 4 GB phone is newly *protected* from loading
   turbo-q5-0 (which needs ~1,040 MB before one minute of audio).
2. **Slice 2: stream decode to a temp `.pcm32`, read windows through `mmap`.**
   Removes the full-file decode and resample copies; output must stay
   byte-identical.
3. **Slice 3: 15-minute silence-aligned macro-windows** with prompt carry-over
   and deterministic overlap dedupe, keeping whisper's own 30 s seek loop
   inside each window. Projected peak: ~395 MB for *any* duration on tiny,
   ~1,141 MB on turbo — flat. Gated on |ΔWER| ≤ 0.5 pt on natural long-form and
   zero repetition loops on the repeated-clip stress fixture.

## Parent verification (checked against this tree)

- `whisper-rs 0.16` exposes everything the design needs:
  `set_no_context`, `set_tokens`, `set_temperature_inc`, `set_entropy_thold`,
  `set_no_speech_thold`, `set_n_max_text_ctx` (verified in the vendored crate).
- Android already reports `totalBytes` / `availableBytes` / `lowMemory`
  (`DeviceMemory.kt`) and the tier rule lives in Rust — the inputs for the
  formula exist.
- `libc` is already a dependency of `suramak-core` (used for `statvfs`), so
  `mmap`/`madvise` and the macOS `sysctlbyname` provider need no new crate.
- Windows needs a 15-line `extern "system" fn GlobalMemoryStatusEx` — no crate.
- The gate's own quality numbers stay caveated: the long fixtures repeat one
  clip, so the chunking WER deltas are "not proven safe", which is exactly why
  Slice 3 is gated on a *natural* long-form corpus as well as the stress clip.

---

# Appendix — Carbon high, verbatim

### DECISION

1. **Verdict**: The static 45m / 60m caps are measurement artifacts from the 3.7 GiB VPS (**KNOWN**). Remove them.
2. **Architecture**: Do **not** rely solely on `mmap` to remove the memory slope—`whisper.cpp` allocates an anonymous mel spectrogram for the entire slice passed to `whisper_full` (**INFERRED** from `whisper.cpp` 1.7/1.8 internals + **KNOWN** 9.19 MB/min slope). Instead, ship **streaming decode to a temporary 16 kHz `f32` file + stateful macro-chunked `whisper_full` (15-minute silence-aligned windows with prompt carryover and timestamp-gated overlap)**.
3. **Cap Policy**: Replace duration caps with a dynamic **disk-space + model-fit check** once macro-chunking lands. As an immediate Phase 1 bridge (before chunking), ship a **RAM-derived dynamic duration cap** that immediately unblocks desktop and 12 GB phones while protecting 4 GB Android devices.

---

### A. IS THE CAP WRONG? (YES — BINDING CONSTRAINTS BY PLATFORM)

A 45-min cap on a 12 GB Vivo X300 and a 60-min cap on a 16–64 GB Mac are **indefensible artifacts** of the 3.7 GiB VPS (**KNOWN**). On `tiny`, 90 minutes takes 1,135 MB (**KNOWN**); on `turbo-q5-0`, 90 minutes single-pass takes ~1,865 MB (**INFERRED**: $1047 - 9.19 + 90 \times 9.19$, adjusted for 128-mel). Both fit effortlessly in 12–64 GB.

**Actual Binding Constraints (**INFERRED** from OS internals):**
* **Android (LMKD + ZRAM Anonymous RSS, NOT Java Heap)**: Native Rust/`whisper.cpp` allocations bypass the ART heap (`dalvik.vm.heapgrowthlimit`, typically 256–512 MB) and allocate directly via Scudo/jemalloc `mmap(MAP_ANONYMOUS)`. There is no hard per-process native heap cap on 64-bit Android, **but** Android has **no disk swap**—only RAM-backed ZRAM (typically 50% of `totalMem`, ~3:1 compression on sparse floats, ~1.1:1 on high-entropy audio/mel floats).
  * **Foreground App (`oom_score_adj = 0`)**: LMKD kills at extreme PSI (`psi_complete` stall) when system `MemAvailable` drops below ~150–250 MB. On a **4 GB phone** (~2.2 GB taken by kernel/SystemUI/GPU/GMS), safe process RSS ceiling is **~1.1–1.3 GB**. On the **12 GB Vivo X300**, foreground ceiling is **~6.5–7.5 GB**.
  * **Background Service (`oom_score_adj = 200–250`, screen off / user switches apps)**: LMKD hunts aggressively when free RAM < 10–15%. Safe process RSS ceiling drops to **~650–800 MB on 4 GB** and **~3.5–4.5 GB on 12 GB**. **This is the true mobile binding constraint.**
* **macOS 13+ Apple Silicon (Unified Memory + Dynamic NVMe Swap)**: Zero arbitrary process RSS cap. `whisper.cpp` uses Metal (`ggml-metal` unified memory wired/shared pages) + `malloc`. Binding constraint is **SSD free space** (macOS dynamically grows `/System/Volumes/VM/swapfile*`) and **Metal `device.recommendedMaxWorkingSetSize`** (~66–75% of RAM, i.e., 10.6 GB on a 16 GB Mac—exceeding this only matters for GPU *model weights/KV*, not CPU-side PCM/mel). A 2 GB job on a 16 GB Mac does not even trigger memory compression.
* **Windows x86_64 (System Commit Limit)**: `VirtualAlloc` fails (`ERROR_COMMITMENT_LIMIT` / Rust OOM abort) strictly when `System Committed Bytes` hits `Physical RAM + Pagefile`. On a 16 GB PC with default pagefile, commit limit is ~20–32 GB, leaving 8–16 GB easily available. Binding constraint is commit exhaustion only if the user manually disabled the pagefile on an 8 GB machine.
* **Linux Desktop**: Overcommit/earlyoom/systemd-oomd. Safe up to ~60% of `MemAvailable`.

---

### B. THE MECHANISM QUESTION: WHY `mmap` ONLY HALF-SOLVES IT

**Look at your measured slope (**KNOWN**): `9.19 MB / audio-minute` (`tiny` model).**
Let's decompose that exact number against `whisper.cpp` 1.7.x/1.8 internals (**INFERRED**):
1. **Input PCM (`16 kHz mono f32`)**: $16,000 \times 4 \times 60 = 3,840,000\text{ bytes} = \mathbf{3.84\text{ MB/min}}$ (**KNOWN**). (Note: your transient resample copy of $2 \times 3.84 = 7.68\text{ MB/min}$ peaks *before* `whisper_full` runs if freed beforehand, or adds to peak if held).
2. **`whisper_pcm_to_mel` (`state->mel.data`)**: Inside `whisper_full_with_state`, `whisper.cpp` immediately scans 100% of `samples[0..n_samples]` via STFT threads and allocates `std::vector<float>` of size `n_mel * n_len` (where `n_len` = 100 frames/sec + 3000 frames of zero-padding).
   * For `tiny` / `base` / `small` / `medium` (`n_mel = 80`): $80 \times 100 \times 60 \times 4\text{ bytes} = 1,920,000\text{ bytes} = \mathbf{1.92\text{ MB/min}}$ (actually **3.84 MB/min** briefly during `log_mel_spectrogram_worker_thread` stage-1/stage-2 buffer resize/doubling or if `std::vector` capacity grows geometrically, plus `1.92 MB/min` steady!).
   * **CRITICAL TRAP for `turbo-q5-0` (`large-v3-turbo`)**: `turbo` uses **`n_mel = 128`**, NOT 80! Its mel vector alone is $128 \times 100 \times 60 \times 4 = \mathbf{3.07\text{ MB/min}}$ steady!
   * Plus `result_all` (`whisper_segment` structs + token vectors + logits/alignments): ~**0.2–0.5 MB/min**.
   * Sum if resample buffer isn't dropped cleanly or vector over-allocates: $3.84\text{ (PCM)} + 3.84\text{ (resample/vec slack)} + 1.92\text{ (mel)} \approx \mathbf{9.6\text{ MB/min}}$ (matching your **9.19 MB/min** **KNOWN** VmHWM slope within MiB vs MB unit conversion: $9.6\text{ MB} = 9.155\text{ MiB}$!).

**What happens if you only `mmap` the temp PCM file and call `whisper_full` once on 90 minutes?**
1. `whisper_pcm_to_mel` linearly reads every single page of the `mmap` slice in <0.5s, faulting **all 345.6 MB** into the OS page cache (`VmHWM` counts file-backed RSS!). Under LMKD pressure, Android *can* evict clean `MAP_PRIVATE`/`MAP_SHARED` file pages (`posix_madvise(MADV_SEQUENTIAL)` helps), so those 345.6 MB become **reclaimable** instead of fatal anonymous ZRAM pages.
2. **HOWEVER**, `state->mel.data` is a heap `std::vector<float>` (`MAP_ANONYMOUS`). You **cannot** `mmap` it without forking `whisper.cpp`. For 90 min on `turbo-q5-0` (`n_mel = 128`), `state->mel.data` allocates **276.5 MB** of un-evictable anonymous RAM (and ~553 MB if `std::vector` geometric growth triggers during resize, though `whisper.cpp` calls `.resize(n_mel * n_len)` directly).
3. Furthermore, **do NOT use `whisper_full_parallel`**: it clones `whisper_state` *N* times (multiplying KV cache + compute buffers ~250–400 MB $\times N$) and splits audio blindly on fixed sample boundaries, causing the exact cut-point hallucinations you measured (**INFERRED**).
4. Calling `whisper_full` in tiny **30-s sub-calls** fixes RAM completely, **but** disables `whisper.cpp`'s internal timestamp-guided seek loop (`seek += delta`), forcing hard 30.00s cuts mid-word and causing severe WER degradation (**INFERRED**).

**The Winning Mechanism**:
Stream-decode + resample in 64 KB chunks directly to a temporary `16 kHz mono f32` file on disk (eliminating **both** full-file PCM and full-file resample RAM copies!), `mmap` (Unix/Android via `libc::mmap`) or `pread`/`File::read_exact_at` a **15-minute silence-aligned window**, and pass that ~15-min slice to `whisper_full`. Inside each 15-min window, `whisper.cpp` runs its native 30-s seek loop untouched.

**90-Minute Peak RAM Arithmetic Under This Design (15-min macro-windows, `madvise(MADV_DONTNEED)` after each window):**
* **Streaming decode + resample buffer**: **~2 MB** (replaces $345.6\text{ MB PCM} + 345.6\text{ MB resample}$).
* **Active 15-min `mmap` PCM window**: **57.6 MB** file-backed (reclaimable by OS; `MADV_DONTNEED` drops RSS to 0 between windows).
* **Active 15-min `state->mel.data`**:
  * `tiny` (80-mel): $15 \times 1.92 = \mathbf{28.8\text{ MB}}$ anonymous.
  * `turbo-q5-0` (128-mel): $15 \times 3.07 = \mathbf{46.1\text{ MB}}$ anonymous.
* **Base runtime + model + KV/compute buffers** (derived from **KNOWN** intercepts: $308.4\text{ MB}$ for `tiny`; $1047 - 10 \approx 1037\text{ MB}$ for `turbo-q5-0`):
  * **90-min `tiny` Peak VmHWM**: $308.4 + 28.8\text{ (mel)} + 57.6\text{ (file RSS)} \approx \mathbf{395\text{ MB}}$ (down from **1,135 MB**; anonymous RSS $\approx \mathbf{337\text{ MB}}$, flat for *any* duration—2 hours or 10 hours!).
  * **90-min `turbo-q5-0` Peak VmHWM**: $1037 + 46.1\text{ (mel)} + 57.6\text{ (file RSS)} \approx \mathbf{1,141\text{ MB}}$ (anonymous RSS $\approx \mathbf{1,083\text{ MB}}$, flat for any duration!).

---

### C. THE QUALITY-PRESERVING ALTERNATIVE TO SHIP

Why did your existing VAD-cut (+21.74 pt WER) and fixed 15-min chunking (+16.88 pt WER) trigger repetition loops (**KNOWN**)?
Three known Whisper failure modes (**INFERRED**):
1. **Cold decoder state at cuts** (`prompt_tokens` empty or carrying already-looping tokens into the next chunk).
2. **Hard cuts through speech or micro-pauses** creating truncated phonemes at `t = 0.0s` of the new chunk, which blows up Whisper's `compression_ratio` / `logprob` heuristics or traps greedy decoding when `temperature_inc == 0`.
3. **Feeding long pure-silence tails** without `no_speech_thold` protection (note: your test fixture had repeated 167s clips with 0.8s silence—repetitive source text specifically poisons Whisper's self-attention KV cache if `n_max_text_ctx` is unbounded across repetitive phrases!).

#### Exact Cut, Prompt & Dedup Specification (`MacroChunkPipeline`)

**1. Unified Decode Stage (One Path for All Inputs)**
* Every input (recording, desktop file, import, Android `content://` share stream) decodes + resamples incrementally into `{cache_dir}/suramak_job_{id}.pcm32` (`16_000` Hz, `f32le`, mono). Check `statvfs` upfront: require `free_bytes >= est_duration_sec * 64_000 + 100_000_000`.
* For recordings ≤ **20 minutes**, if RAM budget permits (see Section D), just run 1 macro-window (`0..EOF`) — **bit-for-bit identical to single-pass**.

**2. Macro-Window Boundary Selection (Target `W = 15 min = 900s`, Search `[840s, 930s]`)**
* Do **not** micro-chop with VAD (that caused your +21.74 pt WER by destroying Whisper's 30s acoustic context). Keep long 15-min contiguous windows.
* Scan only the PCM energy in `[cursor + 840s, cursor + 930s]` using 20 ms frames (320 samples): compute RMS $E_i = \sqrt{\frac{1}{320}\sum x^2}$.
* Adaptive silence floor: $E_{\text{thresh}} = \max\left(10^{-4},\ \text{percentile}_{15}(E) \times 1.8\right)$.
* **Rule C1 (Clean Silence Cut)**: Pick the **midpoint** of the longest contiguous run of frames with $E_i < E_{\text{thresh}}$ lasting **$\ge 400\text{ ms}$** inside `[840s, 930s]`. Overlap = **`1.5 s`** (`cursor_next = cut_point - 1.5s`).
* **Rule C2 (Fallback Minimum-Energy Cut — non-stop speech)**: If no $\ge 400\text{ ms}$ silence exists, pick the 200 ms window with minimum mean energy inside `[870s, 915s]`, and set Overlap = **`4.0 s`** (`cursor_next = cut_point - 4.0s`).

**3. `whisper_full_params` Loop-Prevention & Prompt Continuity**
* **`no_context = true`** at the start of each `whisper_full` macro-call, then explicitly inject sanitized prompt tokens via **`prompt_tokens` / `prompt_n_tokens`** (via `whisper_tokenize` or `FullParams::set_tokens` in `whisper-rs`). Why? `no_context = false` across independent macro-buffers carries stale positional timing biases; explicit `prompt_tokens` populates only the SOT-prev text context (`<|startofprev|>`).
* **Prompt Sanitizer (CRITICAL — stops loop propagation)**: Take the last **$\le 160$ tokens** (never full 224) from the *accepted, non-overlapped* end of chunk $k-1$, **ONLY IF** the trailing segment had:
  1. Compression ratio (UTF-8 length / zlib-or-unique-4gram ratio; without new crates, compute `unique_byte_4grams / total_byte_4grams > 0.45`), AND
  2. Average token probability $\bar{p} \ge 0.45$ (`whisper_full_get_token_p`), AND
  3. No contiguous 1–6 word phrase repeated $\ge 3$ times.
  *If any check fails, pass **zero** prompt tokens (`prompt_n_tokens = 0`) to chunk $k$ to firewall any hallucination.*
* **Anti-Loop Decoder Guards inside `whisper_full_params`** (must be enabled if currently disabled):
  * `entropy_thold = 2.4` (Whisper default; aborts repetitive high-entropy/low-compression 30s windows and retries with higher temperature).
  * `temperature_inc = 0.2` (up to `1.0`—essential fallback when greedy `0.0` loops at a boundary!).
  * `no_speech_thold = 0.6`, `logprob_thold = -1.0`.
  * `max_tokens = 0`, `single_segment = false`, `token_timestamps = true` (or segment timestamps + proportional token interpolation).

**4. Deterministic Overlap Deduplication (`join_seam(prev_segs, next_segs, cut_point_sec, overlap_sec)`)**
Let seam zone $S = [\text{cut\_point} - \text{overlap},\ \text{cut\_point}]$, midpoint $M = \text{cut\_point} - \frac{\text{overlap}}{2}$.
1. **Timestamp Gate**: Keep all words/segments from chunk $k-1$ whose midpoint $\frac{t_0 + t_1}{2} \le M$. Keep all words/segments from chunk $k$ (shifted by `cursor_next`) whose midpoint $> M$.
2. **Boundary LCS Stitch (handles +/- 600ms timestamp jitter at $M$)**: Extract the last 12 normalized words (lowercase, strip ASCII/Unicode punctuation) emitted by chunk $k-1$ before $M$, and the first 12 normalized words emitted by chunk $k$ after $M$. Run exact + fuzzy Longest Common Substring (word equality or Levenshtein $\le 1$ for words $\ge 5$ chars) searching for a suffix-of-prev matching a prefix-of-next of length $L \in [1..12]$ (for $L=1$, require exact match and timestamp gap $< 0.8\text{s}$). Drop the $L$ prefix words from chunk $k$.

**5. Acceptance Test (Quality + Memory)**
* **Corpus**: Do **not** evaluate only on the synthetic repeated-167s clip (keep it solely as a *stress test* for loop firewalling). Build a 90-minute real long-form benchmark (concatenated LibriSpeech long chapters / Earnings22 / real podcast with natural prosody) + the 90-min synthetic repeat fixture.
* **Pass Criteria**:
  1. Normalized WER delta vs single-pass 90-min `whisper_full`: **`|ΔWER| ≤ 0.50 percentage points`** on natural speech, and **`ΔWER ≤ +1.0 pt` with 0 repetition loops** (0 occurrences of any 4-gram repeated $>4\times$ consecutively unless in ground truth) on the 167s-repeat stress fixture.
  2. Peak `VmHWM` on 90-min `tiny` **$\le 450\text{ MB}$** (flat vs 30-min within $\pm 15\text{ MB}$).

---

### D. RIGHT-SIZING THE CAP AT RUNTIME (FORMULAS & ARITHMETIC)

#### 1. Model Baseline Table (`M_base` & `S_slope`)
From **KNOWN** measurements + **INFERRED** `n_mel` scaling:
* **Single-Pass Mode (Phase 1, before macro-chunking)**:
  * `M_base(model)` = `max(310 MB, model_file_mb * 1.81)` → `tiny` = **310 MB** (measured 308.4), `turbo-q5-0` (574 MB) = **1,039 MB** (measured 1,038 MB intercept).
  * `S_slope(model)` = **9.2 MB/min** (`n_mel = 80`) or **10.5 MB/min** (`n_mel = 128`, e.g., `turbo` / `large-v3`). *(Drops by 3.84 MB/min immediately once streaming PCM decode replaces in-memory resample).*
* **Macro-Chunked Mode (Phase 2, `W = 15 min` windows)**:
  * Peak RAM is **constant wrt total file duration**: `M_peak(model) = M_base(model) + 15 * S_window(model)` where `S_window` (with `mmap` + `MADV_DONTNEED`) $\approx 5.8\text{ MB/min}$ (`tiny`) or $6.9\text{ MB/min}$ (`turbo`) $\implies$ **`~397 MB` (`tiny`)** and **`~1,143 MB` (`turbo-q5-0`)**.

#### 2. Safe Memory Budget Formula (`B_safe`)

**Android (`DeviceMemory` from `ActivityManager.MemoryInfo`):**
Query both `totalMem` (at startup) and `availMem` (at job start). Because Suramak runs a job queue that must survive the user locking the phone or switching apps (`oom_score_adj` rising to background service tier), budget conservatively:
```text
if isLowRamDevice || totalMem <= 3_221_225_472 (3 GiB):
    reserve_os = 1_800 MB
    total_cap_frac = 0.25
elif totalMem <= 4_500_000_000 (~4 GB tier):
    reserve_os = 2_100 MB
    total_cap_frac = 0.30
else (> 4 GB tier):
    reserve_os = 2_800 MB
    total_cap_frac = 0.42

B_from_total = min(totalMem * total_cap_frac, totalMem - reserve_os)
B_from_avail = availMem * 0.65   // if availMem is wired up; else omit
B_safe       = min(B_from_total, B_from_avail) * 0.85  // 0.85 safety factor for egui/wgpu/SQLite spikes
```

**Desktop (`libc`, zero new crates):**
* **macOS**: `libc::sysctlbyname(c"hw.memsize")` $\to$ `total_mem`. Since macOS has fast NVMe dynamic swap + memory compression, `B_safe = (total_mem - 4_096 MB) * 0.60`.
* **Linux**: Parse `/proc/meminfo` (`MemAvailable`, fallback `MemTotal`) $\to$ `B_safe = min(MemTotal * 0.55, MemAvailable * 0.75)`.
* **Windows** (`windows-sys` already pulled in by `wgpu`/`egui`, or raw `extern "system" fn GlobalMemoryStatusEx` in 15 lines of FFI—zero new crates): `ullTotalPhys`, `ullAvailPageFile` $\to$ `B_safe = min(ullTotalPhys * 0.55, ullAvailPageFile * 0.65)`.

#### 3. Cap Computation
* **Phase 1 (Single-Pass Duration Cap)**:
  $$\text{max\_minutes} = \left\lfloor \frac{B_{\text{safe}} - M_{\text{base}}(\text{model})}{S_{\text{slope}}(\text{model})} \right\rfloor$$
  *If $B_{\text{safe}} < M_{\text{base}}(\text{model}) + 15 \times S_{\text{slope}}$, block model selection with a clear "Model too large for this device's RAM" error!*
  *Practical UX ceiling*: clamp `max_minutes` to `min(computed, disk_free_cap, 480 min [8 hrs])`.
* **Phase 2 (After 15-min Macro-Chunking Ships)**:
  * Check **Model Fit**: require $B_{\text{safe}} \ge M_{\text{peak}}(\text{model}, 15\text{m})$.
  * Check **Temp Disk Space** (`libc::statvfs` on Unix/Android, `GetDiskFreeSpaceExW` on Windows):
    $$\text{max\_minutes\_disk} = \left\lfloor \frac{\max(0,\ \text{free\_bytes} - 500\text{ MB})}{3.84\text{ MB/min}} \right\rfloor$$
  * Duration is otherwise **uncapped** (or soft-capped at e.g. 12 hours to guard corrupt container headers).

#### 4. Worked Arithmetic Table (Using Decimal MB to match `/proc` measurements)

| Device | `totalMem` | `B_safe` (after 0.85 safety) | Model | **Phase 1 Cap** (Single-Pass, `S=9.2` / `10.5`) | **Phase 2 Cap** (15m Chunks + `mmap`) |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **4 GB Android** (`isLowRam=false`) | 4,000 MB | $\min(1200, 1900) \times 0.85 = \mathbf{1,020\text{ MB}}$ | `tiny` (310 MB) | $\lfloor(1020 - 310)/9.2\rfloor =$ **77 min** *(or **60 min** if `isLowRam=true`, $B_{\text{safe}}=850$)* | **Uncapped by RAM** (needs 397 MB $\le 1020$ MB; disk-bound) |
| **4 GB Android** | 4,000 MB | **1,020 MB** | `turbo-q5-0` (1,039 MB) | **0 min — Blocked upfront!** *(Prevents guaranteed LMKD crash!)* | **Blocked upfront** (needs 1,143 MB $> 1,020$ MB; tell user to pick `small`/`base`) |
| **12 GB Vivo X300** | 12,000 MB | $\min(5040, 9200) \times 0.85 = \mathbf{4,284\text{ MB}}$ | `tiny` (310 MB) | $\lfloor(4284 - 310)/9.2\rfloor =$ **431 min (7.1 hrs)** | **Uncapped by RAM** (397 MB peak; disk-bound) |
| **12 GB Vivo X300** | 12,000 MB | **4,284 MB** | `turbo-q5-0` (1,039 MB) | $\lfloor(4284 - 1039)/10.5\rfloor =$ **309 min (5.1 hrs)** *(warn on RTF battery time)* | **Uncapped by RAM** (1,143 MB peak; disk-bound) |
| **16 GB Mac** (M1/M2/M3) | 16,384 MB | $(16384 - 4096) \times 0.60 = \mathbf{7,373\text{ MB}}$ | `tiny` (310 MB) | $767\text{ min} \to$ **480 min (8h ceiling)** | **Uncapped by RAM** (disk-bound) |
| **16 GB Mac** (M1/M2/M3) | 16,384 MB | **7,373 MB** | `turbo-q5-0` (1,039 MB) | $603\text{ min} \to$ **480 min (8h ceiling)** | **Uncapped by RAM** (disk-bound) |

*(Notice what the formula just caught that the static 45-min cap completely missed: **`turbo-q5-0` on a 4 GB phone OOMs at Minute 1** (`1,047 MB > 1,020 MB`), whereas on the 12 GB Vivo it safely runs for hours!)*

---

### E. SEQUENCING (SMALLEST SAFE SLICES)

#### Slice 1: Dynamic RAM + Disk Cap Policy (Desktop & 12 GB Phone Unblocked Today — ~1 day)
* **Change**: Replace hardcoded `45 min` / `60 min` constants with `compute_device_limits(DeviceMemory, ModelInfo, free_disk_bytes)` from Section D (Phase 1 formula). Wire `sysctlbyname("hw.memsize")` on macOS, `/proc/meminfo` on Linux, `GlobalMemoryStatusEx` on Windows, and existing `DeviceMemory` on Android. Add upfront model-fit gate (`B_safe < M_base + 15*S_slope`).
* **Failure mode avoided**: Desktop and 12 GB Android users stop hitting fake 45/60m walls immediately, with **zero** changes to the audio/whisper pipeline (zero WER risk), while 4 GB phones are newly protected from loading `turbo-q5-0`.
* **Acceptance Test**: Unit test `compute_device_limits` across synthetic `{4GB LowRam, 4GB, 12GB Android, 16GB Mac, 64GB Mac} × {tiny, turbo-q5-0}` + integration test transcribing a **120-minute file on macOS** single-pass to completion with zero truncation.

#### Slice 2: Streaming Temp-File Decode + `mmap` Reader (Unified Input Path — ~2 days)
* **Change**: Refactor the single decode+resample stage to stream in 64 KB chunks to `{temp_dir}/job_{id}.pcm32` (deleting the in-memory full-file decode `Vec<f32>` + full-file resample `Vec<f32>`). Wrap read access in a safe Rust `PcmSliceReader` backed by `libc::mmap(..., MAP_PRIVATE, fd, 0)` + `posix_madvise(..., MADV_SEQUENTIAL)` on Unix/Android (with `Read + Seek` window buffer fallback on Windows, or `CreateFileMappingW`), and RAII `Drop` that `munmap`s + `unlink`s on job finish/panic/startup-orphan-sweep.
* **Failure mode avoided**: Eliminates the transient $2 \times 3.84 = 7.68\text{ MB/min}$ anonymous decode+resample spike and turns the PCM buffer into OS-reclaimable file pages. Keeps ONE path for files, mic recordings, imports, and Android shares.
* **Acceptance Test**: Transcribe a 90-minute file on `tiny` single-pass; verify output text is **100% byte-identical** to before Slice 2, peak anonymous RSS drops by $\ge 340\text{ MB}$, and zero `.pcm32` files remain in `temp_dir` after success or simulated job cancellation.

#### Slice 3: 15-Minute Silence-Aligned Macro-Chunker + Prompt Firewall + Overlap Stitch (~3–4 days)
* **Change**: Implement `MacroChunkPipeline` (Section C) over `PcmSliceReader`, issuing `madvise(MADV_DONTNEED)` on completed windows, reusing one `WhisperState` across windows, and switching `compute_device_limits` from Phase 1 (linear slope) to Phase 2 (flat `M_peak` + `statvfs` disk cap). On Android, ensure the job queue holds a Foreground Service (`FOREGROUND_SERVICE_TYPE_MEDIA_PROCESSING` / `SHORT_SERVICE` or user-visible notification on Android 14–16) while transcribing so `oom_score_adj` stays $\le 200$.
* **Failure mode avoided**: Caps `state->mel.data` anonymous RAM forever (making 3-hour recordings work even on 4 GB phones with `tiny`/`base`) without triggering cut-point repetition loops.
* **Acceptance Test**: Run the 90-minute natural long-form + 90-minute 167s-repeat benchmark suite: assert `|ΔWER| ≤ 0.50 pt` (natural), `0` repetition loops (synthetic), and `VmHWM` on 120-minute `tiny` is within $\pm 15\text{ MB}$ of 30-minute `tiny` ($\le 450\text{ MB}$).

---

### F. WHAT NOT TO DO

1. **DO NOT "just remove or raise the cap" globally without the model-fit check**: On a 4 GB Android phone, a 90-min `tiny` single-pass (1,135 MB) or a 1-min `turbo-q5-0` job (1,047 MB) will trigger a silent `SIGKILL` from Android LMKD. Because SQLite marks the job in-progress, if the app auto-resumes the queue on restart, you create an **infinite crash-on-launch loop**. *(Also: mark a job `attempt_count += 1` in SQLite **before** calling `whisper_full` so an OS kill is detected on next boot and quarantined).*
2. **DO NOT silently downgrade the user's chosen model** (e.g., swapping `turbo-q5-0` $\to$ `tiny` behind their back when a file is long): Users pick `turbo` for non-English accuracy or domain jargon; silent model swaps destroy trust. Block incompatible `(device_ram, model)` pairs upfront in the UI with exact memory numbers.
3. **DO NOT use `whisper_full_parallel(ctx, params, samples, n_samples, n_processors)`**: It allocates `n_processors` full `whisper_state` structs (multiplying RAM instead of cutting it) and slices PCM on blind arithmetic boundaries with zero overlap or prompt carryover—maximizing both peak RSS and boundary WER.
4. **DO NOT chop audio into short VAD segments (5–30s) before Whisper**: Whisper's encoder expects dense 30-second acoustic windows and uses relative timestamps inside each 30s frame to self-align; pre-cutting with VAD creates hundreds of cold boundaries and pads every short utterance to 30 seconds of mel frames (slowing RTF by 2–4x **and** causing the +21.74 pt WER regression you already measured).
5. **DO NOT pass unvalidated `prompt_tokens` across chunks or set `temperature_inc = 0.0`**: Once chunk $k$ hallucinates `"Thank you. Thank you. Thank you."` at a noisy cut, feeding those tokens as `initial_prompt` into chunk $k+1$ guarantees the entire remainder of a 2-hour transcript collapses into that loop. Always run the 4-gram / token-probability firewall in Section C.3.
6. **DO NOT hold Android `content://` share streams in `Vec<u8>` before decoding**: Stream directly from the JNI `ParcelFileDescriptor` (`libc::dup(fd)` passed to Rust `File::from_raw_fd`) straight into the streaming decoder $\to$ `.pcm32` temp file. Holding a 200 MB `.m4a`/`.wav` in RAM before decoding re-introduces an OOM vector outside `whisper.cpp`.