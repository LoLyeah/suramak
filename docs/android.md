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
