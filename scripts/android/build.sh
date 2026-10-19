#!/usr/bin/env bash
# Suramak — Android cross-build (M13a). The single source of truth: CI and
# humans run the same script. It installs nothing; it discovers the pinned
# toolchain and fails loudly when something is missing or unpinned.
#
# Requires (see docs/android.md for the install steps, all sudo-free):
#   - NDK r27d (27.3.13750724) at $ANDROID_NDK_HOME / $ANDROID_NDK_ROOT /
#     $HOME/android-ndk/android-ndk-r27d (first match wins)
#   - cargo-ndk v4.1.2 on PATH (`cargo install cargo-ndk --version 4.1.2`)
#   - rustup targets aarch64-linux-android + x86_64-linux-android
#
# Builds, per ABI:
#   arm64-v8a  release  — the shipping ABI; the M13a gate profile
#   x86_64     debug    — the CI-emulator ABI (PLAN-M13 §4)
# Each build covers suramak-core, suramak-asr and the suramak-android cdylib;
# the resulting libsuramak_android.so is listed with its size at the end.
#
# Usage:  scripts/android/build.sh [abi ...]      (default: both ABIs)
#         SURAMAK_ANDROID_PLATFORM=29 scripts/android/build.sh arm64-v8a
set -euo pipefail

PINNED_NDK_REVISION="27.3.13750724"   # r27d — the pin is recorded in docs/android.md
PINNED_NDK_DIRNAME="android-ndk-r27d"
PLATFORM="${SURAMAK_ANDROID_PLATFORM:-29}"   # minSdk 29 (PLAN-M13-ANDROID §4)

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

# --- NDK discovery (no installation, no SDK manager) --------------------------
find_ndk() {
  local candidate
  for candidate in \
    "${ANDROID_NDK_HOME:-}" \
    "${ANDROID_NDK_ROOT:-}" \
    "$HOME/android-ndk/$PINNED_NDK_DIRNAME" \
    "$HOME/Android/Sdk/ndk/$PINNED_NDK_REVISION" \
    "$HOME/Library/Android/sdk/ndk/$PINNED_NDK_REVISION"
  do
    if [ -n "$candidate" ] && [ -f "$candidate/source.properties" ]; then
      (cd "$candidate" && pwd)
      return 0
    fi
  done
  return 1
}

if ! NDK="$(find_ndk)"; then
  cat >&2 <<EOF
error: no Android NDK found.
  Looked at: \$ANDROID_NDK_HOME, \$ANDROID_NDK_ROOT,
             \$HOME/android-ndk/$PINNED_NDK_DIRNAME,
             \$HOME/Android/Sdk/ndk/$PINNED_NDK_REVISION
  Install NDK r27d ($PINNED_NDK_REVISION) and set ANDROID_NDK_HOME — see docs/android.md.
EOF
  exit 1
fi

NDK_REVISION="$(sed -n 's/^Pkg.Revision *= *//p' "$NDK/source.properties")"
if [ "$NDK_REVISION" != "$PINNED_NDK_REVISION" ]; then
  echo "error: NDK at $NDK is revision $NDK_REVISION; this build pins $PINNED_NDK_REVISION (r27d)." >&2
  echo "       Point ANDROID_NDK_HOME at the pinned NDK (docs/android.md)." >&2
  exit 1
fi

command -v cargo-ndk >/dev/null || {
  echo "error: cargo-ndk not on PATH — run: cargo install cargo-ndk --version 4.1.2" >&2
  exit 1
}

for triple in aarch64-linux-android x86_64-linux-android; do
  if ! rustup target list --installed 2>/dev/null | grep -qx "$triple"; then
    echo "error: rustup target $triple missing — run: rustup target add aarch64-linux-android x86_64-linux-android" >&2
    exit 1
  fi
done

# --- Environment for cargo-ndk + the C build scripts --------------------------
export ANDROID_NDK_HOME="$NDK"
export ANDROID_NDK_ROOT="$NDK"
# whisper-rs-sys drives CMake through the `cmake` crate, which only forwards a
# toolchain file — this shim turns cargo-ndk's per-target ANDROID_ABI /
# ANDROID_PLATFORM env into CMake variables and includes the NDK's official
# android.toolchain.cmake (see the shim's header for the full why).
export CMAKE_TOOLCHAIN_FILE="$REPO_ROOT/scripts/android/ndk-toolchain.cmake"

echo "== Suramak Android cross-build =="
echo "   NDK:      $NDK (r27d, $NDK_REVISION)"
echo "   cargo-ndk: $(cargo ndk --version 2>/dev/null || echo unknown)"
echo "   platform: android-$PLATFORM"

# --- The builds ----------------------------------------------------------------
build_abi() {
  local abi="$1" profile
  case "$abi" in
    arm64-v8a) profile="release" ;;
    x86_64)    profile="debug" ;;
    *) echo "error: unsupported ABI '$abi' (supported: arm64-v8a, x86_64)" >&2; exit 2 ;;
  esac
  echo
  echo "== [$abi] cargo ndk -t $abi --platform $PLATFORM build ($profile) =="
  if [ "$profile" = "release" ]; then
    cargo ndk -t "$abi" --platform "$PLATFORM" build --release \
      -p suramak-core -p suramak-asr -p suramak-android
  else
    cargo ndk -t "$abi" --platform "$PLATFORM" build \
      -p suramak-core -p suramak-asr -p suramak-android
  fi
}

abis=("$@")
if [ "${#abis[@]}" -eq 0 ]; then
  abis=(arm64-v8a x86_64)
fi
for abi in "${abis[@]}"; do
  build_abi "$abi"
done

# --- Artifact summary ----------------------------------------------------------
echo
echo "== artifacts =="
for abi in "${abis[@]}"; do
  case "$abi" in
    arm64-v8a) so="target/aarch64-linux-android/release/libsuramak_android.so" ;;
    x86_64)    so="target/x86_64-linux-android/debug/libsuramak_android.so" ;;
  esac
  if [ ! -f "$so" ]; then
    echo "error: expected artifact missing: $so" >&2
    exit 1
  fi
  size="$(du -h "$so" | cut -f1)"
  echo "  $so ($size)"
  if command -v file >/dev/null 2>&1; then
    file -b "$so"
  fi
done
echo "== android cross-build OK =="
