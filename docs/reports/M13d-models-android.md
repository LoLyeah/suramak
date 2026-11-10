# M13d — models on Android

Workorder: `docs/WORKORDER-M13D.md` (plan source `docs/PLAN-M13-ANDROID.md` §3 M13d).
Base tree: `6be35dc` (v0.4.0 post; M16.1 queue landed, M16B workorder on top).
Scope actually delivered: the library API, the rustls confirmation, the phone
tier/recommendation rules, the free-space preflight, the `dataSync` job host and
the resume proof — all host-tested and build-gated. **No emulator and no device
run** (the workorder excludes them); every runtime claim below says what was and
was not executed.

---

## 0. What the tree actually had (two findings that change the workorder's premises)

### 0.1 Deliverable 1 is a no-op — the logic already lives in `suramak-core`

The download / verify / resume implementation is **already a library**:
`crates/suramak-core/src/download.rs` (985 lines before this slice). It owns
the blocking HTTP client, the `Range` resume, the `.part` bookmark, sha256
verification, ZIP extraction and the multi-file path. `suramak-cli`'s
`models` command (`crates/suramak-cli/src/cmd/models.rs`) is presentation only:
it imports `suramak_core::{download, paths}` and calls
`download::download`, `download::download_zip`, `download::download_files`,
`download::verify_sha256`. There is no reqwest, TLS, checksum or resume code in
the CLI.

So **nothing was moved and nothing was duplicated.** The only duplicated rule
was the CLI's private `part_path()` helper; it is now `download::part_path()`
(public, documented) and the CLI calls the library copy. That is the whole of
item 1's code change.

### 0.2 The M13c scaffolding named in the workorder is not in this tree

The workorder says "M13c's JVM-bridge scaffolding (MainActivity/PlatformBridge/
Insets/JobService skeleton) already exists from the spike". It does not:

```
android/app/src/main/java/app/suramak/spike/   MainActivity.kt, CrashFiles.kt, SpikeApp.kt
crates/suramak-android/src/lib.rs              one C symbol (`suramak_android_version`)
```

There is no `PlatformBridge`, no `Insets` class, no `JobService` skeleton, and
no `PageBridge`. What exists is the M13b spike: `MainActivity` with its own JNI
methods (`nativeInit`, `nativeSetInsets`, `nativeBackPressed`, …). M13c was
never landed. The M13d app surfaces were therefore **built from scratch**, in
the existing Gradle module, without editing the spike's classes. The report
calls this out again in §6.

Because there is no real app package yet, the new Kotlin lives in
`app.suramak.models` (fully qualified in the manifest) and the new Rust JNI
symbol lives in `crates/suramak-android` (`libsuramak_android.so`). The existing
`android-spike` APK now packages **both** libraries so the service's symbols
resolve in the artifact the build gate produces.

---

## 1. Model download/verify/resume as a library API — done (no extraction needed)

**Consumable API the app uses** (all in `suramak_core::download`):

| function | role |
|---|---|
| `install_file(dir, name, url, size, sha256, on_progress) -> Result<()>` | **new** — the app-facing install: verify an existing copy, free-space preflight, resume, size+sha verify |
| `check_space(dir, needed) -> Result<()>` | **new** — the never-half-starts preflight |
| `available_space_bytes(dir) -> Option<u64>` | **new** — `statvfs`, `None` where unknown |
| `part_path(dest) -> PathBuf` | now public (the bookmark's single source of truth) |
| `download_with_progress_checked` | existing transfer with cancel |
| `verify_sha256` | existing checksum |

`install_file` semantics:

1. an existing destination is trusted only after `verify_sha256`; a corrupt copy
   is removed and downloaded again (no permanent lock-out);
2. `check_space` runs **before** the `.part` is created;
3. the transfer resumes from `<file>.part` and the callback can stop it
   (`Cancelled`, bytes kept);
4. the finished file is checked against the pinned size and sha256 before `Ok`.

The CLI keeps its exact command surface and desktop behaviour; it was not moved
onto `install_file` on purpose (desktop must not gain the new free-space
refusal — see the workorder's "no desktop behaviour change").

---

## 2. HTTP stack on Android = rustls — confirmed, no flip needed

* Workspace pin (`Cargo.toml`): `reqwest = { default-features = false,
  features = ["blocking", "json", "rustls-tls"] }`.
* `download.rs` builds its client with `.use_rustls_tls()` explicitly.
* Feature-tree check on the real Android target:

```
$ cargo tree -p suramak-core --target aarch64-linux-android -e features
... reqwest feature "rustls-tls"
... reqwest feature "rustls-tls-webpki-roots"
... rustls feature "ring"
$ cargo tree ... | grep -Eic 'native-tls|openssl'
0
```

No `native-tls`, no `openssl`, no `openssl-sys` anywhere in the Android graph —
nothing to flip. `ring` (the rustls crypto provider) is already built for
`aarch64-linux-android` by the existing CI cross-build.

A CI assertion was added to the `android-build` job (`TLS graph (android
target) is rustls-only`) so a future feature flip fails fast with a named error
instead of a C-toolchain failure deep in a build.

**Boundary:** this proves the *feature graph*, not a TLS handshake on a phone.
No emulator/device run exists in this slice.

---

## 3. Phone model recommendations — additive manifest fields + landing copy

### Manifest (the "store registry" in this tree)

There is no separate JSON model registry; the registry is
`suramak_core::models::MODELS`. `ModelInfo` gained one additive field:

```rust
pub struct PhoneGuidance {
    pub min_ram_bytes: u64,           // 0 = runs below the floor; u64::MAX = never on phones
    pub default_from_ram_bytes: u64,  // 0 = never a default
    pub note: &'static str,           // honest one-liner, also the refusal reason
}
```

Every one of the six models now declares it (tiny 0/0, base floor/floor,
small 5 GiB/6.8 GiB, turbo-q5-0 6.8 GiB/never, turbo-q8-0 and turbo `u64::MAX`).
Desktop ignores the field; no desktop code path reads it. No pins or sizes
changed.

### Rules (`suramak_core::device`)

| device | default | allowed | refused |
|---|---|---|---|
| not arm64 | — | — | everything ("needs a 64-bit ARM phone") |
| arm64 < 4 GB (usable < 3.4 GiB or `isLowRamDevice`) | tiny | tiny | base/small/turbo, each naming "4 GB" |
| 4 GB class | **base** | tiny, base | small ("needs 6 GB"), turbo |
| 6 GB class | **base** | tiny, base, small | turbo-q5-0 ("needs 8 GB"), q8/full |
| 8 GB+ | **small** | tiny, base, small, turbo-q5-0 | turbo-q8-0/full ("not offered on phones") |

* **Indonesian is steered away from tiny** at every tier: `language_note` says
  so, the landing copy says so, and tiny is never a default on a device that
  meets the floor (its `default_from_ram_bytes` is 0). Below the floor it is the
  only runnable model, and the note says exactly that instead of pretending.
* Usable-vs-nominal RAM: Android reports ~3.6 GB on a "4 GB" phone, so the
  thresholds are usable-RAM values (3.4/5.0/6.8 GiB) documented in `device.rs`
  and covered by boundary tests.
* The recommendation is derived from `MODELS`, so a pin and its phone rule
  cannot drift.

### Landing text

`device::LANDING_EN` / `LANDING_ID` (returned by the bridge `landing` op) and
the Android resources `res/values/strings.xml` + `res/values-in/strings.xml`
(`model_landing_*`). Both name base/small as the defaults, tiny as quick-tests
only, and the Indonesian text explicitly says to avoid tiny.

**Boundary:** no compact UI exists yet (M13g), so nothing renders these strings
on a phone in this slice; they are shipped as resources and through the bridge.
The plan's word "quantized" for base/small does not match the manifest: only
`turbo-q5-0`/`turbo-q8-0` are quantized pins. Inventing `base-q5_1`/
`small-q5_1` pins without downloading and hashing the upstream files would put
unverified sha256 values in the manifest, so the recommendations name the
**existing pinned** `base` (148 MB) and `small` (488 MB). Adding quantized
base/small pins is a separate, hash-verified manifest change.

---

## 4. Device floor: arm64 + 4 GB, memory probe over the bridge

* Kotlin `DeviceMemory.read(context)`:
  `ActivityManager.getMemoryInfo(MemoryInfo)` → `totalMem`, `availMem`,
  `lowMemory`, plus `isLowRamDevice` and `Build.SUPPORTED_64_BIT_ABIS` for
  arm64. This is the probe the workorder asks for.
* `ModelsBridge.memory(context, language)` sends it over the one JNI function;
  Rust `DeviceProfile` + `device::tier_for` classify and return the
  recommendation (`default_model`, `allowed`, `refused` with reasons,
  `headline`, `detail`, `language_note`).
* Refusals are explicit and quoted from the same function the download path
  uses (`device::refusal_for`), e.g. `small needs a phone with at least 6 GB of
  RAM; this one has 3.7 GB` and `turbo … not offered on phones`.
* `ModelHost::start` refuses an unprobed device ("device memory has not been
  probed yet") rather than guessing.

**Boundary:** the probe is exercised when a download job starts, not by the
Activity — there is no landing screen yet, so the "steer before download" UI is
M13g's. The mapping itself is host-tested for 4/6/8 GB × EN/ID.

---

## 5. Free-space check before download — refuses before the first byte

`download::check_space(dir, needed)`:

* `statvfs` via `libc` (already in the workspace graph on every target through
  ring/getrandom, chrono, sha2 — `Cargo.lock` gains an edge, not a package);
* requires the file **plus 64 MiB headroom**;
* if `dir` does not exist yet (first launch) it probes the nearest existing
  ancestor, so a fresh install is still checked;
* on non-unix (desktop Windows) `available_space_bytes` returns `None` and the
  check is skipped — desktop behaviour is unchanged;
* the error is one sentence naming both numbers:
  `not enough free space in <models dir>: need 0.6 GB, 0.0 GB available — free
  up space and retry`.

Two call sites, both before any byte is written: `ModelHost::start` (before the
worker thread exists) and `install_file` (before `.part` is created). The
bridge exposes `space` for the UI to preflight with a chosen size.

**Tested:** `a_download_that_cannot_fit_is_refused_before_the_first_byte`
(asserts no `.part`, no destination), `check_space_accepts_what_fits_and_
refuses_what_cannot`, `a_missing_models_dir_still_gets_the_free_space_check`.

**Boundary:** no real full-disk device test; the host test asks for
`u64::MAX` bytes. The Android `filesDir` filesystem is the one statvfs sees at
runtime.

---

## 6. `dataSync` JobService: progress, cancel, timeout, resume

New files (the M13c skeleton did not exist — §0.2):

* `android/.../models/ModelDownloadJobService.kt` — a `JobService` that
  promotes itself to a `dataSync` foreground service
  (`startForeground(..., ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)`),
  builds the channel, polls the Rust host every 500 ms, and updates the
  notification only when the integer percent changes.
* `CancelModelDownloadReceiver.kt` — the notification's Cancel action sets the
  Rust cancel flag and clears the notification.
* `ModelsBridge.kt` — one `external fun nativeCommand(String): String`, JSON
  protocol, `loadLibrary("suramak_android")`.
* `DeviceMemory.kt` — the floor probe.
* Manifest: `INTERNET`, `POST_NOTIFICATIONS`, `FOREGROUND_SERVICE`,
  `FOREGROUND_SERVICE_DATA_SYNC`, the service
  (`exported=false`, `foregroundServiceType="dataSync"`,
  `permission=BIND_JOB_SERVICE`) and the receiver.
* `ModelDownloadJobService.enqueue(context, model, language)` is the scheduling
  entry point (`NETWORK_TYPE_ANY`, `setRequiresStorageNotLow`, extras: model,
  language, `filesDir/models`).

**A fact the workorder's wording gets wrong (verified against the installed
`android-35/android.jar` with `javap`):**

* `JobService` has **no** `onTimeout` in API 35. `Service` has
  `onTimeout(int)` / `onTimeout(int, int)` — the Android 15 six-hour limit is a
  **foreground-service** rule, not a JobScheduler rule.
* `JobInfo.Builder` has **no** `dataSync` setter. The `dataSync` designation is
  the `<service android:foregroundServiceType="dataSync">` attribute (the
  packaged APK shows `foregroundServiceType=0x1`).

So the implementation is: a real `JobService` (survives the Activity, platform
retries) that becomes a `dataSync` foreground service for its duration, and
overrides `Service.onTimeout(startId)` for the 6-hour contract:

* **progress** — deterministic `percent = bytes*100/total` (integer), same
  value on re-read; title/text from resources, `setProgress(100, percent,
  false)`, `setOnlyAlertOnce`;
* **cancel** — broadcast → Rust cooperative cancel → the worker stops at the
  next chunk, `.part` kept, "Download paused" notification;
* **`onTimeout`** — cancel, stop foreground, "Download paused (time limit)",
  `jobFinished(params, true)` → the job is rescheduled and resumes the
  bookmark;
* **`onStopJob`** — cancel, stop foreground, return `true` (retry; the `.part`
  is the bookmark);
* **failure/refusal** — a normal notification ("Download not started" + the
  Rust reason); the job ends without touching the network.

**Boundary:** no emulator in this workorder, so the service/notification path is
compile- and artifact-verified (dex classes present, manifest resolves,
`foregroundServiceType=0x1`), not runtime-exercised. No UI calls `enqueue()`
yet (no compact UI, M13g). The `startForeground` call is wrapped: if a device
refuses the promotion, the job still runs and the notification is posted
normally.

---

## 7. Resume survives a hard process kill — library-layer proof

The on-disk state after a kill is exactly one thing: `<model>.bin.part` with N
bytes. Nothing else survives (the host keeps no journal, no database row, no
in-memory offset that a new process could read). A new process therefore *is*
the "resume" case: `install_file` sees the part, sends `Range: bytes=N-`, and
appends only the rest.

Test `download::tests::resume_after_a_hard_kill_continues_from_the_bookmark`:

1. a 300 000-byte body is served; the first "process" stops mid-stream (the
   callback returns false — the same stop point a SIGKILL leaves the part at);
2. the test reads the bookmark from `.part` and asserts it is partial;
3. a **fresh call with no state** is served a `206` with only the remainder;
4. it asserts the first progress report of the new call equals the bookmark
   (the UI shows continuity), the final file is byte-identical, sha256 verifies,
   and the `.part` is renamed away.

Existing tests already cover: server ignores Range (200 → restart), stale
complete part (416 → removed), cancel keeps the part, multi-file cancel keeps
completed files + current part.

**Boundary:** the kill is simulated at the library seam (callback stop), not a
real `SIGKILL` of a running process, and there is no on-device kill test (no
emulator). The equivalence argument is above and is the honest extent of what a
host test can prove: the code has no cross-call memory to lose.

---

## 8. Verification (commands run in this workspace)

| gate | command | result |
|---|---|---|
| rustls graph | `cargo tree -p suramak-core --target aarch64-linux-android -e features` | `reqwest feature "rustls-tls"`, `rustls feature "ring"`, **0** native-tls/openssl |
| Android compile | `cargo ndk -t arm64-v8a --platform 29 check -p suramak-android --locked` | `Finished dev profile … in 5.53s` |
| APK gate | `bash scripts/android/build-apk.sh` | green; `SIZE: 27219050 bytes (25 MiB)`, `SHA256: c5067d38cd97ed09eab3d2e0cbbd7a975097b6efaca28198bc56276c4c39064e` |
| APK contents | `unzip -l` | `lib/arm64-v8a/libsuramak_android.so` (6 211 080 B), `lib/arm64-v8a/libsuramak_android_spike.so` (13 576 928 B) |
| JNI symbol | `llvm-nm -D --defined-only libsuramak_android.so` | `T Java_app_suramak_models_ModelsBridge_nativeCommand` |
| Manifest | `aapt dump xmltree … AndroidManifest.xml` | `app.suramak.models.ModelDownloadJobService`, `BIND_JOB_SERVICE`, `foregroundServiceType=0x1` (dataSync), receiver, INTERNET/DATA_SYNC permissions |
| Kotlin classes | `dexdump` classes3.dex | `ModelDownloadJobService`, `ModelsBridge`, `DeviceMemory`, `CancelModelDownloadReceiver` |
| Kotlin warnings | `:app:compileDebugKotlin` with `allWarningsAsErrors = true` | `BUILD SUCCESSFUL` |
| Tests | `RUST_TEST_THREADS=1 cargo test --workspace` | **935 passed, 0 failed, 10 ignored**, 29 binaries |
| Clippy (workspace) | `cargo clippy --workspace --all-targets -- -D warnings` | clean (exit 0) |
| Format (workspace) | `cargo fmt --all --check` | clean (exit 0) |

Test growth: 28 new host tests — 9 in `device`, 6 in `download` (free space,
missing dir, resume-after-kill, install verify, corrupt replace), 13 in
`suramak-android` (tier replies, refusals, space, progress, bad requests).
The workspace total is 935 rather than baseline+28 because the **parallel M16B
child** added its own HUD/hotkey tests to `suramak-ui` in the same tree.

Disk: after the gates `/` had 5.5 GB free. `target/debug/incremental` (4.0 GB)
and `target/aarch64-linux-android` (1.1 GB — this slice's Android build cache,
regenerated by `build-apk.sh`) were removed; `/` is back to 11 GB free. The APK
and `jniLibs/*.so` were kept, so the artifact evidence above still resolves.

---

## 9. Known issues / honest boundaries

1. **Pre-existing flaky test, not from this slice.**
   `cmd::transcribe::tests::a_multi_file_batch_continues_after_a_failure`
   (`crates/suramak-cli/src/cmd/transcribe.rs`, M16.1 queue, unchanged by this
   work) intermittently panics at its `debug_assert_eq!(done+failed+cancelled,
   total)` with `0 != 2`. Cause: the first `driver.run_once()` can complete both
   junk-file jobs, so the following `while !driver.is_idle()` never runs and its
   `drain_events()` never consumes the two `Failed` events. It reproduces
   standalone on this 2-core VPS and passed in the final full run; it is
   timing-dependent. It is outside M13d and outside the files this workorder
   allows (`transcribe.rs` is not a models file), so it was reported, not
   patched.
2. **Formatting is green in the final tree.** During the work,
   `cargo fmt --all --check` was red only on the parallel M16B child's
   in-progress `suramak-ui` files; they were formatted by that workstream before
   this report was finished. Workspace-wide `cargo fmt` was never run by this
   slice (it would have rewritten their files).
3. **No emulator/device run.** Everything Android-side is build/artifact
   verified. The service has never been started, the notification never shown,
   and the memory probe never returned a real phone's `totalMem`.
4. **No compact UI.** `enqueue()` has no caller; the landing/refusal copy is
   shipped as resources + bridge data for M13g. The `POST_NOTIFICATIONS`
   runtime request (Android 13+) also belongs to that UI slice — without it the
   job still runs, but the progress notification is silent.
5. **The spike app and the real app share one Gradle module** because M13c
   never split them. Both `.so` files are packaged; the spike's Activity still
   loads only its own library, and the JobService loads
   `libsuramak_android.so` when it starts. Splitting the modules is the real
   app's M13g work.
6. **CI was not executed here** (no GitHub runner in this environment). The
   workflow change is limited to a rustls feature-tree step in `android-build`;
   the local equivalents of `android-build` (arm64) and `android-spike`
   (`build-apk.sh`) both ran green. The `x86_64` half of `android-build` was not
   rebuilt locally (the new Rust code is target-independent and `cargo ndk
   check` covers arm64).
7. **"Quantized base/small"** is not in the manifest (see §3); no unverified
   pins were invented.

---

## 10. Files touched

**Rust**

* `crates/suramak-core/src/device.rs` — new: tiers, recommendations, refusals,
  landing copy, 9 tests.
* `crates/suramak-core/src/models.rs` — `PhoneGuidance` + one additive field per
  model; module doc.
* `crates/suramak-core/src/download.rs` — `install_file`, `check_space`,
  `available_space_bytes`, public `part_path`; 6 new tests.
* `crates/suramak-core/src/error.rs` — `NotEnoughSpace`.
* `crates/suramak-core/src/lib.rs` — `pub mod device`.
* `crates/suramak-core/Cargo.toml` — `libc` (unix targets; already in the graph).
* `crates/suramak-android/src/model_host.rs` — new: the JSON command host, 10
  tests.
* `crates/suramak-android/src/lib.rs` — the module, the process host, the one
  JNI export, 3 tests.
* `crates/suramak-android/Cargo.toml` — `serde_json`, `jni` (android target;
  both already in the lock/graph).
* `crates/suramak-cli/src/cmd/models.rs` — calls `download::part_path`, private
  copy removed (no behaviour change).

**Kotlin / Android resources**

* `android/app/src/main/java/app/suramak/models/` — `ModelsBridge.kt`,
  `DeviceMemory.kt`, `ModelDownloadJobService.kt`,
  `CancelModelDownloadReceiver.kt`.
* `android/app/src/main/AndroidManifest.xml` — permissions, service, receiver.
* `android/app/src/main/res/values/strings.xml`,
  `android/app/src/main/res/values-in/strings.xml` — landing + notification
  copy (EN/ID).
* `android/app/build.gradle.kts` — `allWarningsAsErrors = true`.

**Build / CI / docs**

* `scripts/android/build-apk.sh` — builds `suramak-android` too, asserts both
  libs and the M13d manifest entries.
* `.github/workflows/ci.yml` — rustls-only feature-tree assertion.
* `Cargo.lock` — dependency edges only (`jni`, `serde_json`, `libc`); no new
  package, no version change.
* this report.

**Deliberately untouched:** `crates/suramak-ui`, `crates/suramak-taps` (parallel
M16B child), the M15 suites, and the spike's own Kotlin/Rust files.
