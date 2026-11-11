# M13I — the Android emulator smoke: the release APK's launch surface, proven without a device

Executed per `docs/WORKORDER-M13I.md`. The owner reported the v0.4.1/0.4.2
release APK "utterly broken" with no symptom detail, and the compact Library
launch surface (M13G1–G3) had never run on a real Android runtime — only in
Xvfb screenshots. This milestone builds an **x86_64** variant of the same app,
boots an emulator on GitHub's KVM-capable runner, installs it, launches it, and
makes a blank screen, a crash dialog or a dead process a **red build**.

**Run status: the job is landed but has not executed yet.** The workorder
forbids `git commit`/`git push`, and CI only runs on a push — so the first
`android-smoke` run happens on the next push to `main`. The sections below
separate what was verified on this VPS (the whole workflow/script/gate surface,
plus a byte-level audit of the shipped v0.4.2 APK) from what only the run can
prove. No claim below is a substitute for that run.

## 1. What landed

| File | Change |
|---|---|
| `scripts/android/build-apk.sh` | ABI + Rust profile are build inputs (`SURAMAK_ANDROID_ABI`, `SURAMAK_ANDROID_PROFILE`); the release path refuses anything but arm64-v8a + release. x86_64 builds one cdylib (the one `MainActivity` loads) in debug, and the APK assertions are ABI-aware (including "no foreign ABI's libs"). |
| `android/app/build.gradle.kts` | `-PsuramakAbi` sets `ndk.abiFilters`; unknown values fail the build. Default stays `arm64-v8a`, so every existing invocation is byte-identical. |
| `scripts/android/emulator-smoke.sh` | The whole smoke as one script: AVD create, headless boot, install, launch, markers, onboarding dismissal, screencap + uiautomator capture, assertions, monkey canary, artifact dump. |
| `tests/android-smoke/inspect_screencap.py` | Stdlib-only PNG decoder + the Library screencap gate, with `--self-test` (real renders as positive controls; black/white frames as negative controls). |
| `.github/workflows/ci.yml` | New `android-smoke` job (KVM, cached system image, x86_64 APK build, smoke, evidence upload), `continue-on-error: false`. |
| `docs/android.md` | The ABI switch and the smoke documented next to the existing build section, plus the owner-facing note that a release APK cannot install over a debug-signed build ("App not installed") — a candidate for the owner's report that no emulator can reproduce. |
| `docs/reports/M13i-emulator-smoke.md` | This report. |

No Rust source changed. `release.yml` is untouched; the arm64 release job, its
signing step and its asset names are exactly as before.

## 2. The x86_64 variant, and why it cannot touch the release

`build-apk.sh` already had an arm64-v8a/release path; the change is that the
ABI and Rust profile are now explicit inputs instead of constants:

- **arm64-v8a (default, release)** — unchanged: both cdylibs, release Rust,
  `assembleRelease`, the version/FileProvider/JNI/dex assertions. This is what
  `release.yml` calls.
- **x86_64 (CI, debug)** — `SURAMAK_ANDROID_ABI=x86_64` builds
  `libsuramak_android.so` from `cargo ndk -t x86_64 --platform 29` in the debug
  profile and packages it with `assembleDebug -PsuramakAbi=x86_64`. The spike
  cdylib is skipped on purpose: since M13G3 `MainActivity` loads only
  `libsuramak_android.so`, and the spike code is linked into it.

Two guards make the boundary structural rather than a convention:

1. `SURAMAK_ANDROID_RELEASE=1` with any ABI other than `arm64-v8a` (or any
   profile other than `release`) exits 2 before building anything.
2. The APK assertion fails if the x86_64 APK contains `lib/arm64-v8a/` (and
   vice versa), so a stale `jniLibs` tree cannot leak into the artifact.

The job uploads the x86_64 APK as the Actions artifact
`suramak-android-x86_64-debug` with `if-no-files-found: ignore` — a CI-only
diagnostic, never attached to a release. The release path's wall-clock is
unaffected: `android-smoke` is a separate parallel job, and the only thing it
shares with `android-build`/`android-spike` is the pinned NDK cache.

## 3. The emulator job

`.github/workflows/ci.yml` → `android-smoke` (`ubuntu-latest`, 75 min,
`continue-on-error: false`):

1. **KVM** — installs the standard udev rule (`MODE="0666"` on `/dev/kvm`) and
   prints `ls -l /dev/kvm`. GitHub's ubuntu runners expose the device; without
   it the emulator falls back to unusable software emulation and the job fails
   at boot with a clear harness error.
2. **Disk** — removes preinstalled toolchains (`dotnet`, `ghc`, `boost`, the
   SDK's bundled NDKs, the hosted tool cache) so the NDK + Gradle + cargo
   build has room; `df -h` is printed before and after.
3. **Toolchain** — JDK 17, `cmake` + `ninja-build` (whisper.cpp's CMake build),
   the pinned NDK r27d (same sha256 + cache key as the other Android jobs),
   cargo-ndk 4.1.2, `x86_64-linux-android`.
4. **SDK packages** — `sdkmanager` installs `platform-tools`,
   `platforms;android-35`, `build-tools;35.0.0`, `emulator` and
   `system-images;android-35;google_apis;x86_64`. The **system image directory
   is cached** (`android-system-image-35-google-apis-x86_64-v1`) *before*
   `sdkmanager` runs, so a warm run does not re-download ~1.5 GB;
   `emulator -accel-check` proves KVM is live.
5. **Screencap gate self-test** — the gate must pass three real compact renders
   and reject a black and a white frame before it is allowed to judge the
   emulator.
6. **Build** — `SURAMAK_ANDROID_ABI=x86_64 scripts/android/build-apk.sh`, then
   upload the APK.
7. **Smoke** — `scripts/android/emulator-smoke.sh`.
8. **Evidence** — `build/android-smoke/` uploaded `if: always()`.

### What the smoke does, step by step

- Creates `suramak-m13i` (Pixel 6 profile) if absent, then boots it with
  `-no-window -no-audio -no-boot-anim -no-snapshot -wipe-data -gpu
  swiftshader_indirect -accel on -memory 2048`. `-wipe-data` guarantees the
  first-run state; swiftshader is the deterministic software Vulkan/GL the app's
  `Backends::VULKAN | GL` list can always acquire.
- Waits for a real boot: `adb devices` → `sys.boot_completed == 1` **and**
  `service check package` found, bounded by `SURAMAK_BOOT_TIMEOUT` (600 s).
  `ANDROID_SERIAL` is pinned from `adb devices` (the port is not always 5554).
- Disables window/transition/animator scales, unlocks, captures the **launcher**
  screencap (the baseline).
- `adb install -r -t` (fails unless `Success`), grants `POST_NOTIFICATIONS` and
  `RECORD_AUDIO` so the monkey cannot park the app in a permission dialog.
- `am start -W -n app.suramak.spike/.MainActivity`, then waits on the app's own
  markers (below).
- Captures `launch-onboarding.png` (the one-time sheet over the Library), sends
  `KEYCODE_BACK` (the compact stack treats back-on-the-intro as **Skip** and
  persists `first_run_seen`), re-issues `am start` in case that back instead
  left the Library root, then captures `library.png`.
- Runs the screencap gate on `library.png` with `launcher.png` as the baseline.
- Runs the monkey canary, captures `after-monkey.png`, and greps the full
  logcat for fatal signatures.
- Always: full logcat, uiautomator dumps, `dumpsys window`,
  `dumpsys activity activities`, the app's own `spike-last-crash.txt`, and
  `emulator.log` land in the artifact directory; the emulator is killed.

## 4. The launch assertions (what makes it red)

| # | Assertion | Failure it catches |
|---|---|---|
| 1 | `adb install` prints `Success` | an APK that cannot be installed (signing/alignment/manifest) |
| 2 | logcat `SuramakSpike`: **`android_main entered (entry #…, activity generation …)`** within 90 s | `System.loadLibrary` / JNI / `GameActivity` / native-thread failure — the "opens then closes" class |
| 3 | logcat `SuramakSpike`: **`first frame of this gpu generation presented`** | no adapter, no surface, a GPU-init dead end — the black-screen class |
| 4 | `dumpsys window` `mCurrentFocus` is `app.suramak.spike/…MainActivity` | a crash dialog, an ANR dialog or the launcher in front |
| 5 | uiautomator dump names the package and contains no "has stopped"/"isn't responding" | the system's own crash/ANR surface |
| 6 | no `FATAL EXCEPTION`, `ANR in app.suramak.spike`, `Force finishing activity` in the app logcat | a Java/native crash the markers can race past |
| 7 | **the screencap gate** on `library.png` (§5) | a blank/black/white screen, the launcher, a system dialog, a flat fill |
| 8 | monkey: no fatal signature, and the pid survives **or** the app exited through its own `SuramakQuit` path | a crash under interaction; the app's deliberate Quit is not a failure |

`continue-on-error: false` is explicit in the job: any of 1–8 fails the build.
That is the point — the owner's failure mode must be a red run, not a warning
buried in an artifact.

## 5. The screencap gate — and the surface it was calibrated on

There is no OCR and no image library on the runner, so
`tests/android-smoke/inspect_screencap.py` decodes the PNG with `zlib` and
asserts properties only the app's painted surface has:

| Check | Threshold | Why |
|---|---|---|
| portrait phone resolution | ≥ 480×800, height > width | a landscape/letterboxed/wrong-device frame is not the launch surface |
| compact theme is the dominant surface | modal 4-bit colour within 14 of a `theme.rs` window/panel tone (light **or** dark) and ≥ 20 % of pixels | the app's material ladder, not a wallpaper or a system dialog |
| not a flat fill | ≥ 24 distinct 4-bit colours | a blank frame, a solid crash background |
| luminance spread | stddev ≥ 10 | a flat frame |
| top band carries text/controls | ≥ 0.4 % high-contrast pixels in the top 10 % | the "Library" / "Settings" app bar |
| bottom band carries text/controls | ≥ 0.4 % high-contrast pixels in the bottom 10 % | Import / Record |
| layered chrome | ≥ 2 distinct band tones across 20 horizontal bands | app bar vs content vs bottom bar; a flat fill yields 1 |
| app replaced the launcher | mean \|ΔL\| ≥ 8 vs the pre-launch screencap | the app is actually in front and painting |

The compact shell installs the **dark ladder unconditionally** at GPU setup
(`theme::install(&egui_ctx, AppTheme::Dark, …)`), so the expected emulator
modal is ≈`#0E1526`; the light tones remain accepted because the gate also
serves the light reference render and system-drawn surfaces.

**Calibration is on the real product surface, not on invented numbers.** The
deterministic compact renders in `tests/ui-compact/shots/` are produced by the
same `suramak_ui::deck::compact` code the phone runs. The gate's `--self-test`
requires:

```
positive control first-run-download.png: PASS
positive control launch-library.png: PASS
positive control library-light.png: PASS
negative control black frame: FAIL
negative control white frame: FAIL
```

Measured on the references (1216×2640, the phone viewport):

| Shot | modal colour | share | distinct colours | luma σ | top contrast | bottom contrast |
|---|---|---|---|---|---|---|
| `first-run-download.png` (dark, first run) | `(8,24,40)` ≈ `#0E1526` | 0.605 | 85 | 30.6 | 0.0088 | 0.2143 |
| `launch-library.png` (dark, populated) | `(8,24,40)` | 0.850 | 81 | 25.3 | 0.0088 | 0.2143 |
| `library-light.png` (light ladder) | `(232,232,248)` ≈ `#E9EBF0` | 0.878 | 67 | 23.2 | 0.0080 | 0.2141 |

**Emulator screencap: PENDING.** No `library.png` exists for this tree because
the job has not run (no push from this workorder). What is inspected here is
the deterministic reference surface, plus the gate that will judge the
emulator artifact; the gate — not a human eye — is what makes the CI run red or
green. On the first run, `library.png` and `screencap.json` from
`suramak-android-smoke-evidence` are the artifact to read, and `logcat-full.txt`
plus `exit-info.txt` name any failure.

The three positives are the three states a fresh install can be in (onboarding
over the first-run Library, the first-run Library itself, and the light
ladder); the two negatives are the two ways a screenshot can lie (all black,
all white). A gate that cannot fail is not a gate, so `--self-test` runs in CI
before the emulator and would fail the job if the thresholds ever stopped
biting.

**What was inspected here.** The reference renders were opened and read
([`tests/ui-compact/shots/launch-library.png`](../../tests/ui-compact/shots/launch-library.png),
`first-run-download.png`, `onboarding.png`): the Library app bar reads
"Library" with "Settings" at the right, the search field says "Search
transcripts", the first-run card reads "Get a model for this phone" with the
recommended row and a Download button, the empty state explains share/import/
record, and the bottom bar carries Import and Record. No black frame, no
dialog, no missing chrome. The emulator's own `library.png` is the artifact this
same gate judges; **that artifact does not exist yet** because the job has not
run (see the run status at the top). The first run's `screencap.json` carries the
measured numbers and `library.png` is the visual read; the gate, not this
document, is the pass/fail.

## 6. The shipped v0.4.2 APK, audited byte-for-byte

Without a device, the next best evidence is the artifact the owner actually
installed. `gh release download v0.4.2` was inspected on this VPS:

| Property | Measured |
|---|---|
| asset | `suramak-0.4.2-android-arm64-v8a.apk`, 37,382,418 bytes |
| sha256 | `88e52f93c094b69ad1071e2acfb9acda2e8e7387b90fe0db0cf917aca8640929` |
| package / version | `app.suramak.spike`, versionCode 402, versionName 0.4.2 |
| SDK | minSdk 29, targetSdk 35, compileSdk 35 |
| ABIs | `native-code: 'arm64-v8a'`; `lib/arm64-v8a/libsuramak_android.so` (27,931,344 B) + `libsuramak_android_spike.so` |
| packaging | `extractNativeLibs=false`, both `.so` **stored** (0 % compression) |
| 16 KB readiness | `zipalign -c -P 16 -v 4` → **Verification successful**; ELF `LOAD` align **0x4000** |
| dynamic deps | only `libdl/liblog/libandroid/libOpenSLES/libm/libc` — no `libc++_shared.so` gap |
| JNI | `android_main`, `Java_app_suramak_ui_UiBridge_nativeCommand`, `Java_app_suramak_spike_MainActivity_nativeInit` all exported |
| dex | `app/suramak/ui/UiBridge`, `app/suramak/spike/MainActivity`, the compact verb constants — the compact build, not a stale spike |
| signature | v3 scheme only (correct for minSdk 29; v1/v2 unnecessary), CN=Suramak |

So the released bytes are structurally launchable: aligned for 16 KB devices,
uncompressed and mapped from the APK, all JNI symbols present, the compact
classes packaged, a signature Android 10+ accepts. Nothing in the artifact
explains "utterly broken" — which is exactly why the smoke has to run.

One release-surface defect was found in the audit and is **not** fixed here
(it is not a launch bug and this workorder forbids unrelated production
edits): `res/values/strings.xml` still sets `app_name` to **"Suramak Spike"**,
so the installed release shows a developer name under the icon. One-line fix,
owner/orchestrator call.

## 7. Diagnosing "utterly broken" — what the first run separates

The smoke is designed so its artifacts answer the question the owner could not:

| Candidate | How the run distinguishes it |
|---|---|
| **Install-over-debug-signature** — the first release-signed install over an M13x debug build fails with `App not installed` | The emulator installs into a **clean, wiped** userdata, so it passes. If the owner's failure was at install time, the smoke is green and the fix is the documented uninstall-first step (`docs/android.md`). |
| **Native load / JNI / GameActivity** | Assertion 2 (`android_main entered`) never appears; logcat carries the `UnsatisfiedLinkError`/`dlopen` line. |
| **GPU/surface init (black screen)** | Assertion 3 (`first frame … presented`) never appears; logcat carries `request_adapter`/surface errors. |
| **A panic on the first draw** | Assertion 1/6 + the app's own `spike-last-crash.txt` (pulled into the artifact) name the panic, message and location. |
| **A Kotlin crash in `onCreate`** | `AndroidRuntime` in logcat + the crash file; assertion 6 fails. |
| **The compact Library not drawn (host missing → "Opening the library…")** | Assertions 2/3 pass but the screencap gate fails the app-palette check (`#0B0B0E` is not a theme window tone), and `library.png` shows the "Opening the library…" screen. |

What it cannot separate — and what stays owner-only — is in §8.

## 8. The honest boundary (x86_64 vs arm64, and the OEM layer)

- **The emulator is x86_64; the owner's phone is arm64.** The smoke proves the
  shared code path: the Kotlin shim, the JNI contract, `android_main`, the
  wgpu/egui surface, the compact Deck render, the app's own startup and frame
  markers. It does **not** execute a single arm64 instruction.
- **The GPU is SwiftShader, not the phone's GPU.** Adreno/Mali Vulkan driver
  behaviour, Metal (macOS) and the vendor's surface compositor are not
  exercised. A driver-specific black screen on the Vivo would pass this job and
  still fail on hardware.
- **The Vivo OEM layer is untouchable from CI**: OriginOS background killing,
  the OEM's permission prompts, its share-sheet routing, its 120 Hz scheduling.
  Those stay owner-verify.
- **16 KB pages**: the emulator image uses 4 KB pages. The APK's 16 KB
  readiness is proven statically (§6: `zipalign -P 16`, `LOAD` align 0x4000),
  not at runtime.
- **The x86_64 device is not arm64, so the product says so.** `DeviceMemory`
  reports `arm64=false`, `PhoneTier::Unsupported`, and the first-run card shows
  the honest "needs a 64-bit ARM phone" guidance. That is correct behaviour,
  but it means the smoke does **not** exercise model download, transcription,
  sharing or recording — those need an arm64 device (the owner's).
- **Install-over-previous-signature is not reproducible** on a wiped emulator;
  the release-signature change is a distribution event, not a code path.
- **`app.suramak.spike`** remains the application id (and the launcher label
  currently reads "Suramak Spike"). Renaming the id would orphan every
  installed library; that decision is the owner's, not this workorder's.

## 9. Verification ledger

Run on this VPS against the final tree:

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `RUST_TEST_THREADS=1 cargo test --workspace` | 1129 passed / 0 failed / 10 ignored, exit 0 |
| `bash -n scripts/android/build-apk.sh scripts/android/emulator-smoke.sh` | clean |
| `python3 tests/android-smoke/inspect_screencap.py --self-test` | 3 positives PASS, 2 negatives FAIL as required |
| PNG decoder cross-check | ImageMagick-re-encoded render (all four PNG filter types: Sub 30, Up 507, Average 10, Paeth 103 rows) decoded pixel-identically on a 500-pixel random sample |
| `yaml.safe_load(.github/workflows/ci.yml)` | parses; 6 jobs, `android-smoke` has 17 steps and `continue-on-error: false` |
| shipped v0.4.2 APK audit | §6 (aapt/apksigner/zipalign/llvm-readelf/dexdump) |

Not run here (and cannot be): the emulator job itself — it needs a KVM host and
a push. Also not rebuilt here: the x86_64 cdylib itself (the `android-build` job
proves the engine stack for x86_64; this VPS kept its disk budget — `target/`
was 13 GB with 5 GB free, so a second full NDK target was not built). The
first run on `main` is the acceptance gate; its artifact
`suramak-android-smoke-evidence` carries `library.png`, `launch-onboarding.png`,
`window-library.xml`, `logcat-full.txt`, `exit-info.txt` (the platform's
crash/ANR/LMKD record) and `screencap.json`.

### 9.1 Workspace tests

`RUST_TEST_THREADS=1 cargo test --workspace` → **1129 passed / 0 failed / 10
ignored**, exit 0 (the suite runs serially by design: whisper sessions are not
concurrency-safe in-process). No Rust source changed; the run is the
regression check for the tree, not for this diff.

## 10. Reproduce

```bash
# local, KVM host, SDK + system image installed (docs/android.md)
SURAMAK_ANDROID_ABI=x86_64 bash scripts/android/build-apk.sh
bash scripts/android/emulator-smoke.sh

# gates
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
RUST_TEST_THREADS=1 cargo test --workspace
python3 tests/android-smoke/inspect_screencap.py --self-test
```
