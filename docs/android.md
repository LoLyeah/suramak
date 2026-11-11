# Android (M13) — toolchain pins and the cross-build

Status: M13a (engine-on-NDK gate) landed 2026-10-19. This file records the
toolchain pins and how to run the cross-build; the milestone plan is
`docs/PLAN-M13-ANDROID.md`, the slice report `docs/reports/M13a.md`.

## Pins

| Piece | Pin | Where recorded |
|---|---|---|
| Android NDK | **r27d** — revision `27.3.13750724` | `scripts/android/build.sh` (`PINNED_NDK_REVISION`), the CI `android-build` job, this table |
| NDK artifact | `android-ndk-r27d-linux.zip` from `https://dl.google.com/android/repository/android-ndk-r27d-linux.zip` | below |
| NDK sha256 | `601246087a682d1944e1e16dd85bc6e49560fe8b6d61255be2829178c8ed15d9` (663,956,036 bytes) | the CI job verifies it before unpacking |
| NDK sha1 | `22105e410cf29afcf163760cc95522b9fb981121` | cross-checked against Google's own `repository2-1.xml` (`<remotePackage path="ndk;27.3.13750724">`) |
| cargo-ndk | **v4.1.2** (`cargo install cargo-ndk --version 4.1.2`) | the CI job installs exactly this |
| rustup targets | `aarch64-linux-android`, `x86_64-linux-android` | `scripts/android/build.sh` checks both are present |
| Android platform level | `android-29` (minSdk 29, `PLAN-M13-ANDROID.md` §4) | `scripts/android/build.sh` (`SURAMAK_ANDROID_PLATFORM`, default 29) |
| ABIs | `arm64-v8a` (release; the shipping ABI) + `x86_64` (debug; the CI-emulator ABI) | `scripts/android/build.sh` |

r27d is the newest r27.x line release (`r27e` and later do not exist on
`dl.google.com`); the sha256 is measured from the official zip, and the sha1
matches Google's published repository metadata for `ndk;27.3.13750724`.

## Install the toolchain (no sudo, user home only)

```bash
# 1. NDK r27d into ~/android-ndk/android-ndk-r27d
mkdir -p ~/android-ndk
curl -L -o /tmp/android-ndk-r27d-linux.zip \
  https://dl.google.com/android/repository/android-ndk-r27d-linux.zip
sha256sum /tmp/android-ndk-r27d-linux.zip   # must match the pin above
unzip -q /tmp/android-ndk-r27d-linux.zip -d ~/android-ndk/
rm /tmp/android-ndk-r27d-linux.zip

# 2. cargo-ndk + the Rust targets
cargo install cargo-ndk --version 4.1.2
rustup target add aarch64-linux-android x86_64-linux-android
```

`scripts/android/build.sh` finds the NDK through `$ANDROID_NDK_HOME`,
`$ANDROID_NDK_ROOT`, `~/android-ndk/android-ndk-r27d`, then the standard SDK
locations — and refuses to build against a different revision.

## Build

```bash
scripts/android/build.sh                  # both ABIs (arm64-v8a release, x86_64 debug)
scripts/android/build.sh arm64-v8a        # one ABI while iterating
```

Artifacts land under `target/<triple>/<profile>/`:
`libsuramak_android.so` (the cdylib) plus the `suramak-core` / `suramak-asr`
rlibs for the same target. The script prints each `.so` with its size.

CI runs the same script in the `android-build` job (`.github/workflows/ci.yml`),
which downloads the pinned NDK with the sha256 check above and caches it keyed
on the revision.

## The installable APK and the x86_64 emulator variant (M13I)

`scripts/android/build-apk.sh` builds the APK. Its ABI is a build input:

```bash
bash scripts/android/build-apk.sh                          # arm64-v8a, release Rust (the shipping path)
SURAMAK_ANDROID_ABI=x86_64 bash scripts/android/build-apk.sh  # x86_64, debug Rust — CI emulator only
```

The release path is pinned: `SURAMAK_ANDROID_RELEASE=1` refuses any ABI other
than `arm64-v8a`, and the release job in `release.yml` is unchanged. The
x86_64 variant exists only for the `android-smoke` CI job: a debug-signed APK
uploaded as a CI artifact (`suramak-android-x86_64-debug`), never a release
asset. Gradle gets the ABI through `-PsuramakAbi`, so `abiFilters` packages
exactly the `.so` cargo-ndk produced for that ABI.

The emulator smoke itself is one script — CI and humans run the same file:

```bash
# needs /dev/kvm, the emulator package and the android-35 google_apis x86_64 image
bash scripts/android/emulator-smoke.sh
```

It boots an AVD headless (`-gpu swiftshader_indirect`), installs the APK,
launches `app.suramak.spike/.MainActivity`, waits for the app's own logcat
markers (`android_main entered`, `first frame ... presented`), dismisses the
one-time onboarding with BACK, and inspects the Library screencap with
`tests/android-smoke/inspect_screencap.py` (stdlib PNG decoder; the gate's
positive controls are the deterministic renders in `tests/ui-compact/shots/`).
A bounded monkey pass follows as a crash canary. Artifacts land in
`build/android-smoke/` (screencaps, uiautomator dumps, full logcat).

What this proves and what it does not: `docs/reports/M13i-emulator-smoke.md`.

## Why the CMake shim exists

`whisper-rs-sys` builds whisper.cpp through the `cmake` crate, which forwards
only a toolchain file. The NDK's `android.toolchain.cmake` reads the ABI and
platform from CMake variables (`ANDROID_ABI` / `ANDROID_PLATFORM`) and, with
them unset, silently defaults to `armeabi-v7a`. `cargo-ndk` exports both in the
environment (per target) but nothing in the whisper-rs → cmake-rs chain reads
them. `scripts/android/ndk-toolchain.cmake` bridges exactly that gap — it maps
the environment values into CMake and then includes the NDK's own toolchain
file. It is set via `CMAKE_TOOLCHAIN_FILE` by `build.sh`; nothing else changes.

Two notes on C++ linkage, recorded so M13b+ does not re-derive them:

- the NDK's toolchain defaults `ANDROID_STL` to `c++_static`, so whisper.cpp's
  objects carry the static libc++ (the plan's preference);
- `whisper-rs-sys` nonetheless emits `cargo:rustc-link-lib=dylib=c++_shared`
  for Android, so the final link also records a `libc++_shared.so` dependency.
  Making that consistent (patch the emit or ship `libc++_shared.so`) is
  packaging work for M13b+; it does not affect the M13a compile/link gate.

## What M13a proves — and what it does not

Proves: `suramak-core`, `suramak-asr` (whisper.cpp, bundled libopus via
`symphonia-adapter-libopus`, symphonia) and the `suramak-android` cdylib
cross-compile and link for `aarch64-linux-android` and `x86_64-linux-android`,
and CI keeps proving it on every push.

Does not prove (next gates): anything about runtime behaviour — no Android
binary is executed here. Runtime verification is the CI emulator smoke and the
owner's device at M13b.

---

## For the owner (the M13h release): install, first-run, permissions

Install, no adb:

1. Open the release page on the phone: `https://github.com/LoLyeah/suramak/releases`
   — pick the newest release, download `suramak-<version>-android-arm64-v8a.apk`.
2. Open the downloaded file; Android asks once to install from this source
   (Chrome/Files). Tap **Install**. The APK is signed with Suramak's own key
   (not a store key): Play Protect may note "unknown developer" the first
   time — proceed; on-device only, no telemetry; the sha256 is published
   beside the APK (check with any file-hash app).

First run:

1. The app opens on the **Library** with a **model download card** — the
   recommendation is language-aware (Indonesian steers you away from `tiny`),
   free-space-checked, downloaded over the dataSync foreground service with a
   percent notification that survives being backgrounded and resumes after
   the app dies.
2. Onboarding explains the three paths once: **share a voice note**
   (WhatsApp/Telegram → share → Suramak), **open a file** (Ogg/WebM/MKV/m4a/
   mp3/wav via the picker), **record in-app** (mic).
3. A transcription's progress lives in the notification (percent + waiting
   count, cancel action). With the screen off it keeps running (the
   foreground service holds a partial wake lock); killing the app mid-job
   offers **resume or discard** on the next launch.

Permissions — what they are for:

- **Microphone**: only when you press record. The recording is local; the
  foreground notification shows whenever the mic is open — no hidden
  recording.
- **Notifications**: the transcription progress/completion and the
  model-download progress. Denied, the job still runs — you just won't get
  the banner.
- **Files (SAF)**: only what you pick or share into the app; nothing else is
  read.
- **Battery optimization** (optional): Vivo/Xiaomi/Oppo-class systems kill
  backgrounded apps aggressively. Suramak resumes work after being killed,
  but for long jobs allow "run in background" for Suramak in the battery
  settings.

Diagnostics: Settings → Diagnostics keeps the shell spike's four-check
counters and the last-crash banner — if anything misbehaves on a new device,
that card names it; the crash file (shareable) sits in the app's external
files dir.

Updates ride the same release page (a newer APK with a higher versionCode
installs over the old one; the library data stays). Uninstall removes the
library database — back up via the export sheet first (the library itself is
app-private).

**If a debug build was ever installed** (any M13x spike APK), the release APK
will not install over it: debug and release builds carry different signing
keys, and Android refuses the upgrade with **"App not installed"** — before any
of the app's code runs. Uninstall the old Suramak first (that removes the
app-private library; export anything worth keeping), then install the release
APK. Every later release upgrades in place.
