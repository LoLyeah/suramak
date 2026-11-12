# M23B — can the phone diarize? A gate, not a promise

**Verdict: BORDERLINE.** The engine **builds and links for `arm64-v8a` today** —
no from-source build, no NDK CMake work, no new crate. What stops a plain GO is
not the build: it is (1) a 4 KB-page-aligned `libonnxruntime.so` in the pinned
1.12.9 prebuilt, which will not load on 16 KB-page Android 15+ devices, and
(2) the plan's own 60-minute gate being arithmetically out of reach for the
current in-memory pipeline (see §7). Nothing in this report ships: no product
code, no Cargo feature flip on `main`, no CI edit, no commit.

Scratch worktree: `/home/ubuntu/.hermes/cache/scratch/m23b-work` (branch
`scratch/m23b`, never pushed). Probe crates live outside the repo under
`/home/ubuntu/.hermes/cache/scratch/`.

---

## 1. The question, and the assumption it overturned

`docs/reports/M23-speaker-tagging.md` assumed:

> `sherpa-rs` ships prebuilt natives per target and Android is not one of its
> advertised targets. If it needs a from-source NDK build, that is a multi-day
> detour…

**That assumption is wrong.** `sherpa-rs-sys 0.6.8`'s `dist.json` has an
`android` entry, and its `targets` map names `aarch64-linux-android`
explicitly:

```json
"android": {
  "archive": "sherpa-onnx-{tag}-android.tar.bz2",
  "is_dynamic": true,
  "targets": {
    "aarch64-linux-android": [
      "jniLibs/arm64-v8a/libsherpa-onnx-c-api.so",
      "jniLibs/arm64-v8a/libonnxruntime.so"
    ],
    ...
  }
}
```

`dist.json`'s `tag` is `v1.12.9` — the same sherpa-onnx version desktop already
bundles (M16F). The archive is 34,257,502 B compressed and extracts to 94 MB
for all four ABIs; only the two `arm64-v8a` files matter here.

---

## 2. Attempt A — `sherpa-rs 0.6.8` + `download-binaries`, `aarch64-linux-android`

### 2.1 Probe crate (outside the repo)

`/home/ubuntu/.hermes/cache/scratch/m23b-probe` — a `cdylib` with the exact
dependency line `suramak-asr` uses for its `parakeet` feature:

```toml
sherpa-rs = { version = "0.6.8", default-features = false, features = ["download-binaries"] }
```

`src/lib.rs` constructs `sherpa_rs::diarize::Diarize` from two paths, i.e. it
references exactly the API `crates/suramak-asr/src/diarization.rs` calls.

### 2.2 First failure — and it is not sherpa's

The bare `cargo build --target aarch64-linux-android` (NDK env set, no bindgen
help) failed in `sherpa-rs-sys`'s build script:

```
error: failed to run custom build command for `sherpa-rs-sys v0.6.8`
  --- stderr
  /usr/include/stdint.h:26:10: fatal error: 'bits/libc-header-start.h' file not found

  thread 'main' panicked at .../sherpa-rs-sys-0.6.8/build.rs:330:14:
  Failed to generate bindings: ClangDiagnostic("/usr/include/stdint.h:26:10: fatal error: ...")
```

Cause: `build.rs` maps the Rust target to `--target=armv8-linux-androideabi`
for bindgen but supplies no sysroot, so clang picked the host's `/usr/include`.
Not an Android-natives problem. Two fixes, both verified:

* raw cargo: `BINDGEN_EXTRA_CLANG_ARGS_aarch64-linux-android="--sysroot=$NDK/sysroot --target=aarch64-linux-android29"`;
* **the path `scripts/android/build-apk.sh` actually uses** —
  `cargo ndk -t arm64-v8a --platform 29 build` — works with **no** extra
  bindgen arguments, because cargo-ndk sets them itself. Verified separately.

### 2.3 Result: it links

```
   Compiling sherpa-rs-sys v0.6.8
   Compiling sherpa-rs v0.6.8
   Compiling m23b-probe v0.1.0 (/home/ubuntu/.hermes/cache/scratch/m23b-probe)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.20s
```

Produced `libm23b_probe.so` (6,377,512 B debug), `NEEDED`:

```
libsherpa-onnx-c-api.so
libdl.so
libc.so
```

Both prebuilt natives were hard-linked next to it by `sherpa-rs-sys`'s build
script, exactly as on desktop.

### 2.4 What the cache carries

Path: `~/.cache/sherpa-rs/aarch64-linux-android/7e139d98…/jniLibs/arm64-v8a/`

| file | bytes | needed? |
|---|---:|---|
| `libonnxruntime.so` | 16,035,920 | yes (always) |
| `libsherpa-onnx-c-api.so` | 4,485,608 | yes (this binding) |
| `libsherpa-onnx-cxx-api.so` | 68,336 | no |
| `libsherpa-onnx-jni.so` | 4,642,224 | no (Java path only) |

ELF facts:

```
libsherpa-onnx-c-api.so: ELF 64-bit LSB shared object, ARM aarch64, ...
  for Android 21, built by NDK r28c (13676358), stripped
  NEEDED libandroid.so, liblog.so, libonnxruntime.so, libm.so, libdl.so, libc.so
  LOAD ... R E 0x4000        ← 16 KB aligned

libonnxruntime.so: ELF 64-bit LSB shared object, ARM aarch64, ...
  for Android 21, built by NDK r26b (10909125), stripped
  NEEDED libdl.so, liblog.so, libm.so, libc.so
  LOAD ... R E 0x1000        ← 4 KB aligned  (see §6)
```

No `libc++_shared.so` dependency in either — the C API is statically linked
against libc++, so the APK needs exactly these two files.

### 2.5 Diarization is really in the prebuilt

`llvm-nm -D --defined-only` finds 12 diarization symbols, including:

```
T SherpaOnnxCreateOfflineSpeakerDiarization
T SherpaOnnxOfflineSpeakerDiarizationProcessWithCallback
T SherpaOnnxOfflineSpeakerDiarizationResultSortByStartTime
T SherpaOnnxOfflineSpeakerDiarizationResultGetNumSpeakers
```

So this is not a stripped ASR-only build; the pyannote + CAM++ path desktop uses
is compiled in.

---

## 3. Attempt B — from-source sherpa-onnx 1.12.9: **NOT ATTEMPTED**

Reason: **not needed.** 1(b) was conditioned on 1(a) lacking Android natives;
1(a) succeeded. A source build would also not fix the one real defect found
(§6) — sherpa-onnx's CMake links the *prebuilt* onnxruntime for Android, so a
from-source sherpa still ships a 4 KB ORT unless ORT itself is rebuilt.

Cost estimate had it been needed (2-core VPS, 8.8 GB free at the end of this
run): sherpa-onnx source + third_party ≈ 500 MB, a Ninja shared-lib build tree
≈ 2–4 GB, 20–40 min wall clock. That is inside the disk budget only if nothing
else runs; the honest stop rule was never triggered because the prebuilt worked.

---

## 4. The real crate compiles for Android — no code changes

Not just a toy probe. In the scratch worktree, with the NDK env:
 `cargo build -p suramak-asr --features parakeet --target aarch64-linux-android --release`

```
   Compiling suramak-core v0.5.0 (.../m23b-work/crates/suramak-core)
   Compiling suramak-asr v0.5.0 (.../m23b-work/crates/suramak-asr)
    Finished `release` profile [optimized] target(s) in 1m 03s
EXIT=0
```

`libsuramak_asr.rlib` 4.4 MB, 163 sherpa/diarization symbols inside.
`crates/suramak-asr/src/lib.rs` compiles `pub mod diarization;`
unconditionally; only the engine inside it is `#[cfg(feature = "parakeet")]`,
so the phone's `Unavailable` path is already the same shape desktop had before
M16I.

One environment stumble worth recording because it will bite CI the same way:
`opusic-sys` (Opus decoding, not sherpa) runs `cmake --install` and tried to
write `/usr/local/lib/libopus.a` → `Permission denied`. Fixed for the probe by
`CMAKE_INSTALL_PREFIX=<scratch>`. This is a cross-build hygiene issue that the
existing `build-apk.sh` never hit because it only builds the app crates, whose
dependency graph presumably resolved Opus through pkg-config on the host. It is
**not** a sherpa blocker, but the first Android build that compiles
`suramak-asr` for the phone will hit it.

---

## 5. Sizes and APK delta

| item | bytes | MiB |
|---|---:|---:|
| `libsherpa-onnx-c-api.so` (arm64-v8a) | 4,485,608 | 4.28 |
| `libonnxruntime.so` (arm64-v8a) | 16,035,920 | 15.29 |
| **native payload** | **20,521,528** | **19.57** |
| pyannote segmentation-3.0 `model.onnx` | 5,992,913 | 5.72 |
| CAM++ `…campplus_sv_zh_en_16k-common_advanced.onnx` | 28,281,164 | 26.97 |
| **models** | **34,274,077** | **32.69** |
| **total on-device addition (libs + models)** | **54,795,605** | **52.26** |

Model numbers are the pins in `crates/suramak-asr/src/diarization.rs`
(`SEGMENTATION_BYTES`, `EMBEDDING_BYTES`); the files are not downloaded on this
VPS, so they are quoted from the code, not re-measured.

**APK delta, measured** (scratch worktree, existing `android/app`, same Gradle
task, adding only the two `.so` to `jniLibs/arm64-v8a/`):

| build | APK bytes |
|---|---:|
| baseline, build 1 | 38,580,033 |
| baseline, build 2 (identical inputs) | 41,601,969 |
| with the two sherpa `.so` | 66,484,193 |

Delta: **+24.9 to +27.9 MB** depending on baseline, against an exact stored
payload of **+20,521,528 B**. The spread is AGP/zip page-alignment padding, not
content — the two new entries are `Stored` at exactly 16,035,920 and 4,485,608
bytes. For a shipping build with `useLegacyPackaging = false`, budget
**~20.5 MB APK growth** and treat the rest as packaging noise.

Models should **not** be bundled: the phone already has the M13d model-download
`JobService` (`crates/suramak-android/src/model_host.rs`), so 34.3 MB is a
first-run data download, not APK weight. A GO slice adds two model entries to
that manifest; no new machinery.

---

## 6. The blocker that is real: 16 KB pages

The app already promises 16 KB support: `scripts/android/build-apk.sh` links
every Rust `.so` with `-Wl,-z,max-page-size=16384`, `app/build.gradle.kts`
stores jniLibs uncompressed for exactly this reason, and `targetSdk = 35`.

The 1.12.9 prebuilt does not keep that promise:

* `libsherpa-onnx-c-api.so` — `LOAD … 0x4000` ✅
* `libonnxruntime.so` — `LOAD … 0x1000` ❌

On a 16 KB-page device (Android 15+ with a 16 KB kernel — a growing share of
new hardware, and the configuration Play requires for new apps from Nov 2025),
`dlopen("libonnxruntime.so")` fails. The app would not crash at install; it
would fail the first time the phone tried to diarize, which is worse.

**Cheap fix, verified:** sherpa-onnx **1.13.8** (latest 1.x) ships an Android
archive whose `libonnxruntime.so` is `for Android 27` and **16 KB aligned**
(`LOAD … 0x4000`), with the same 12 diarization symbols and the same C API. The
probe was rebuilt with the 1.13.8 pair *in place of* the 1.12.9 cache files:
`sherpa-rs 0.6.8` links cleanly, and the `.so` copied next to the artifact has
the 1.13.8 BuildID (`179fc10f5b1b6dda529097b23dbf3c9df20c7426`, 4,465,168 B),
so the crate's symbol set really is satisfied by 1.13.8, not by a stale 1.12.9.

Cost of the bump: `libonnxruntime.so` 16.0 → 22.2 MB, `libsherpa-onnx-c-api.so`
4.49 → 4.47 MB ⇒ payload 20.5 → **26.7 MB** (+6.2 MB).

**But the mechanism needs care:** `sherpa-rs-sys`'s build script *overwrites*
`SHERPA_LIB_PATH` for any target in its `dist.json`, so simply exporting it does
not redirect the link — that was tested and fails silently (it rebuilds from its
own cache). A product bump therefore needs one of:

* a vendored/patched `sherpa-rs-sys` (or `[patch.crates-io]` fork) whose
  `dist.json` points at 1.13.8 — the honest route for a shipping app;
* or ship 1.13.8's two `.so` in `jniLibs` while Rust links the 1.12.9 cache
  copy: ABI-compatible per the test above (same SONAME, superset symbols), but a
  deliberate link-vs-ship mismatch that must be documented and re-tested on
  every bump.

It is still a supply-chain decision (a newer native than the crate pins), not a
free win.

The alternative — build sherpa-onnx from source with NDK r27d — produces a
16 KB C API but **not** a 16 KB ORT, because CMake consumes the same prebuilt
AAR. It fixes half the problem for 2–4 GB and half an hour. Not recommended.

---

## 7. The gate numbers do not survive arithmetic

`M23` names a **60-minute** recording and `RSS delta ≤ 180 MB`. Two hard facts
in the current engine:

1. `MAX_AUDIO_MS = 30 min` is enforced in `execute()`
   (`DiarizationError::TooLong`) — a 60-minute file is **refused before any
   inference**. The gate as written cannot run against today's code.
2. The runner hands sherpa the whole waveform as `Vec<f32>`: 16 kHz × 4 B =
   **64 KB per audio-second**. Measured desktop peak RSS with both ONNX
   sessions live is **135–152 MB** (M16H). Therefore:

   | audio | waveform | + sessions | projected peak |
   |---:|---:|---:|---:|
   | 10 min | 38 MB | 135–152 MB | **173–190 MB** |
   | 30 min (the cap) | 115 MB | 135–152 MB | **250–267 MB** |
   | 60 min (the gate) | 230 MB | 135–152 MB | **365–382 MB** |

   The **180 MB delta gate is reachable only for roughly 7–12-minute
   recordings**. At 30 minutes it is over by ~1.4×; at 60 minutes by ~2.1×.
   This is architecture, not phone speed: no ARM64 device can beat it, and
   desktop's own M16H report already projected ~370 MB for 1 h and flagged it
   "not measured".

Consequence for the gate: a 60-minute, screen-off, unplugged run on the owner's
device is the right test for **RTF, battery and thermals**, but its **RSS
criterion must be restated** (or the input capped) before the result means
anything. A phone with 8 GB RAM will probably survive 250–380 MB; Android's
LMKD is the thing the 180 MB line was protecting, and that line is not met by
the current design at any length above ~12 min.

`RTF` itself is plausible: desktop measured **0.150–0.167 s/audio-s**; the
plan's 2–3× ARM penalty gives 0.30–0.50, straddling the 0.45 pass line. That is
a projection, not evidence.

---

## 8. The runtime gate is OPEN (and this box cannot close it)

No ARM64 Android device is attached to this VPS, no emulator/AVD is installed,
and an x86_64 emulator timing is **not** evidence for ARM thermals, battery or
LMKD (M23 says so explicitly). So:

* build + link + symbols + ABI + minSdk: **proven here**;
* load + run + RTF + RSS + battery + thermals on `arm64-v8a`: **unmeasured**,
  owner's device required.

This report deliberately does not convert that gap into a GO.

---

## 9. Verdict

**BORDERLINE**, in one paragraph: the phone *can* build the engine today —
`sherpa-rs 0.6.8`'s own cache carries `arm64-v8a` natives built for Android 21,
they link against the exact API `suramak-asr` uses, and the real crate compiles
for the target with one feature flag. But it should not ship as-is: the pinned
1.12.9 `libonnxruntime.so` is 4 KB-page aligned and will fail to load on 16 KB
devices, and the plan's 60-minute / 180 MB RSS gate cannot pass with an
in-memory waveform that costs 64 KB per audio-second and a 30-minute hard cap.
Both are fixable — a 1.13.8 prebuilt bump (+6.2 MB) and either a shorter cap or
chunked diarization — but neither is this slice.

**If the owner's device is a 4 KB-page phone** (most devices before Android 15
with 16 KB kernels), the only remaining unknown is measured runtime; the build
path below is ready.

---

## 10. If this becomes GO: the exact product slice

Condition: 16 KB ORT settled (§6) and a device run passes the restated gate.
Order matters; each step is small.

1. **Feature, no new crate.** In `crates/suramak-asr/Cargo.toml` add
   `diarization = ["dep:sherpa-rs", "sherpa-rs/sys"]` (today the engine rides
   `parakeet`, which would also switch on the Parakeet ASR engine on the phone —
   an unrelated surface). In `crates/suramak-android/Cargo.toml`'s
   `cfg(target_os = "android")` block:
   `suramak-asr = { path = "../suramak-asr", features = ["capture", "diarization"] }`.
2. **Packaging.** In `scripts/android/build-apk.sh`, after each `cargo ndk`
   invocation, copy `libsherpa-onnx-c-api.so` and `libonnxruntime.so` from
   `target/aarch64-linux-android/<profile>/` into
   `android/app/src/main/jniLibs/arm64-v8a/`, then assert each is ELF64, has the
   expected size, and `llvm-readelf -l | grep LOAD` shows `0x4000`. `cargo ndk`
   copies only the crate's own cdylib; without this step the APK links at build
   time and dies at `dlopen` on the phone.
3. **Models.** Add the two files to the existing model-download manifest
   (`model_host.rs` + `models::ModelInfo`, sizes/hashes already pinned in
   `diarization.rs`). Preflight with `download::check_space`; a missing model is
   a named refusal, not a crash — `SherpaDiarizer::load` already verifies size
   and sha256 before sherpa sees a path.
4. **FGS step after transcription.** `TranscriptionJobService` already polls the
   Rust `QueueDriver`; `JobStage` today is `Decoding | Transcribing`. Add
   `Diarizing` after `Done`, driven by the same one-slot queue so a second note
   cannot start inference while the first is tagging. Progress comes free:
   `SherpaDiarizer::run` bridges sherpa's `(done,total)` callback to 0–100, and
   the notification reuses `Notify.progress`. Cancel keeps the M16H contract —
   the C++ pipeline ignores the callback's return, so a running job is
   *discarded at the next await point*, never aborted; the existing
   `CANCEL_CONTRACT` sentence is the one to show.
5. **Unload order (the memory rule).** Drop the Whisper engine — the
   `WhisperContext` inside the worker — and join the transcribe thread **before**
   `DiarizationWorker` constructs `SherpaDiarizer`, i.e. before ~135–152 MB of
   ONNX sessions allocate on top of the still-resident Whisper model (whichever
   ggml size the phone downloaded). The diarization worker
   loads models per job and drops them when the job ends, so peaks do not stack;
   the queue's single-inference slot is what makes this enforceable rather than
   hopeful.
6. **The `Tag speakers` action.** Bottom bar today: `Speakers` opens the manual
   sheet (M23A). Untagged → label `Tag speakers`; a run exists → `N speakers`.
   Tapping it opens a compact sheet `How many people spoke? [Auto] [2] [3] [4+]`
   (default Auto → `num_clusters = 0`, threshold clustering) with a `Tag`
   button; then the FGS runs and the editor re-reads the run. No new store work:
   `store.replace_diarization_run` is the desktop call
   (`crates/suramak-ui/src/desktop/runtime.rs:558`) and Android already renders
   `diarization.speakers/turns` and speaker-aware exports
   (`compact_host.rs:730`, `:1016`). Zero embeddings are persisted — the run row
   stores models, hashes and counts, never vectors.
7. **Cap, honestly.** With today's in-memory design, enable the action only for
   notes ≤ ~10 minutes, or restate the RSS budget at 30 min (~265 MB projected).
   Chunked diarization with global re-clustering is a separate slice, not a
   tweak.

Acceptance, mirroring M23's: on the owner's phone, `Tag speakers` on a 2-person
recording completes in the FGS with progress, the editor shows coloured
`Speaker 1`/`Speaker 2`, rename propagates to the notification prefix, and a
DB dump shows no embedding table/rows.

**If NO-GO** (device fails the restated gate, or 16 KB cannot be settled): the
phone stays manual — the M23A Speakers sheet is already the shipping answer, and
this report is the evidence for that sentence.

---

## 11. What this slice did not do

* No product code, no Cargo feature flip on `main`, no CI edit, no commit, no
  push. Experiments ran in `scratch/m23b` and in `/…/scratch/m23b-probe`.
* No from-source sherpa-onnx build (not needed; see §3).
* No device measurement (no device; see §8). No emulator run — an x86_64 timing
  would be a false signal.
* No model download (the two files are absent on this VPS; sizes are the code
  pins).
* The 1.13.8 test swapped its two `.so` into sherpa-rs' cache temporarily; the
  cache was restored to the 1.12.9 pair afterwards (`libonnxruntime.so`
  16,035,920 B, `LOAD … 0x1000`), so a later build reproduces §2, not §6.

## Appendix — exact commands that produced the evidence

```bash
# 1. probe link (prebuilt cache, 1.12.9)
export CC_aarch64_linux_android=$NDK/bin/aarch64-linux-android29-clang
export CXX_aarch64_linux_android=$NDK/bin/aarch64-linux-android29-clang++
export AR_aarch64_linux_android=$NDK/bin/llvm-ar
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$NDK/bin/aarch64-linux-android29-clang
export LIBCLANG_PATH=/home/ubuntu/.hermes/cache/scratch/libclang   # libclang.so symlink
env "BINDGEN_EXTRA_CLANG_ARGS_aarch64-linux-android=--sysroot=$NDK/sysroot --target=aarch64-linux-android29" \
  cargo build --target aarch64-linux-android

# 1b. the build-apk.sh path, no bindgen env needed
ANDROID_NDK_HOME=$NDK_ROOT cargo ndk -t arm64-v8a --platform 29 build

# 2. the real crate
cargo build -p suramak-asr --features parakeet --target aarch64-linux-android --release

# 3. APK delta
./gradlew assembleDebug          # baseline, then again with the two .so in jniLibs

# 4. 16 KB check
llvm-readelf -l libonnxruntime.so | grep LOAD     # 0x1000 in 1.12.9, 0x4000 in 1.13.8
llvm-nm -D --defined-only libsherpa-onnx-c-api.so | grep -c Diariz   # 12
```

Desktop reference numbers used above (M16h): CPU 0.150–0.167 s/audio-s, peak RSS
135–152 MB, model load 0.75–0.91 s, 1 h → ~370 MB projected.
