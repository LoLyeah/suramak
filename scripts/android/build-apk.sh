#!/usr/bin/env bash
# Suramak — M13b: build the installable debug APK for the Android shell spike.
#
#   scripts/android/build-apk.sh
#
# What it does, in order (one source of truth; CI runs this exact file):
#   1. discovers the pinned toolchain (NDK r27d, a full JDK 17, the Android SDK);
#   2. cross-builds the Rust cdylib for arm64-v8a with cargo-ndk, dropping
#      libsuramak_android_spike.so into android/app/src/main/jniLibs/;
#   3. runs ./gradlew assembleDebug (Kotlin shim + manifest + jniLibs → APK);
#   4. asserts the APK exists and really contains the .so, the right version
#      and the crash-log FileProvider, then prints its path, size and sha256.
#
# It installs nothing and needs no sudo. Environment (all optional):
#   ANDROID_HOME / ANDROID_SDK_ROOT   SDK root (default ~/android-sdk)
#   ANDROID_NDK_HOME                  NDK r27d (default ~/android-ndk/android-ndk-r27d)
#   JAVA_HOME                         JDK 17 with javac (default: ~/android-sdk/jdk/*,
#                                     then /usr/lib/jvm/java-17-openjdk-amd64)
#   SURAMAK_ANDROID_PLATFORM          API level for cargo-ndk (default 29 = minSdk)
#
# 16 KB page alignment: Android 15+ devices with 16 KB pages require the LOAD
# segments of the .so to be 16 KB aligned. The flag below is the linker's;
# Gradle additionally stores the library uncompressed (see app/build.gradle.kts).
set -euo pipefail

PINNED_NDK_REVISION="27.3.13750724"   # r27d — docs/android.md
PINNED_NDK_DIRNAME="android-ndk-r27d"
PLATFORM="${SURAMAK_ANDROID_PLATFORM:-29}"   # minSdk 29 (PLAN-M13-ANDROID §4)
ABI="arm64-v8a"
CRATE="suramak-android-spike"
LIB_NAME="libsuramak_android_spike.so"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

fail() {
  echo "error: $*" >&2
  exit 1
}

# --- SDK ---------------------------------------------------------------------
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/android-sdk}}"
[ -d "$SDK" ] || fail "no Android SDK at $SDK (set ANDROID_HOME)"
export ANDROID_HOME="$SDK"
export ANDROID_SDK_ROOT="$SDK"

# --- JDK (needs javac: Kotlin/AGP compile, a JRE is not enough) ---------------
if [ -z "${JAVA_HOME:-}" ]; then
  for candidate in "$SDK"/jdk/* /usr/lib/jvm/java-17-openjdk-amd64; do
    if [ -x "$candidate/bin/javac" ]; then
      JAVA_HOME="$candidate"
      break
    fi
  done
fi
[ -n "${JAVA_HOME:-}" ] && [ -x "$JAVA_HOME/bin/javac" ] \
  || fail "no JDK with javac found (set JAVA_HOME); a JRE-only install cannot build Kotlin"
export JAVA_HOME
export PATH="$JAVA_HOME/bin:$PATH"

# --- NDK ---------------------------------------------------------------------
# Prefer the pinned install at its canonical path when present: CI runner images
# export ANDROID_NDK_HOME for a *different* NDK revision, and silently building
# against that one would fail (or worse, succeed with the wrong toolchain).
if [ -f "$HOME/android-ndk/$PINNED_NDK_DIRNAME/source.properties" ]; then
  NDK="$HOME/android-ndk/$PINNED_NDK_DIRNAME"
else
  NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-$HOME/android-ndk/$PINNED_NDK_DIRNAME}}"
fi
[ -f "$NDK/source.properties" ] || fail "no NDK at $NDK (set ANDROID_NDK_HOME)"
NDK_REVISION="$(sed -n 's/^Pkg.Revision *= *//p' "$NDK/source.properties")"
[ "$NDK_REVISION" = "$PINNED_NDK_REVISION" ] \
  || fail "NDK at $NDK is $NDK_REVISION; this build pins $PINNED_NDK_REVISION (r27d)"
export ANDROID_NDK_HOME="$NDK"
export ANDROID_NDK_ROOT="$NDK"

command -v cargo-ndk >/dev/null || fail "cargo-ndk not on PATH (cargo install cargo-ndk --version 4.1.2)"
command -v rustup >/dev/null || fail "rustup not on PATH"
rustup target list --installed | grep -qx aarch64-linux-android \
  || fail "rust target aarch64-linux-android is missing (rustup target add aarch64-linux-android)"

[ -d "$SDK/platforms/android-35" ] || fail "SDK platform android-35 missing in $SDK"
[ -d "$SDK/build-tools/35.0.0" ] || fail "SDK build-tools 35.0.0 missing in $SDK"

echo "== M13b spike build =="
echo "   sdk:        $SDK"
echo "   ndk:        $NDK ($NDK_REVISION)"
echo "   java:       $JAVA_HOME ($("$JAVA_HOME/bin/java" -version 2>&1 | head -1))"
echo "   cargo-ndk:  $(cargo-ndk --version)"
echo "   abi:        $ABI (platform $PLATFORM)"

# --- 1. Rust cdylib -----------------------------------------------------------
# RUSTFLAGS is scoped to this invocation on purpose: the desktop crates never
# inherit an Android linker flag from an ambient environment.
echo "== cargo ndk build ($CRATE, release) =="
RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" \
  cargo ndk -t "$ABI" --platform "$PLATFORM" \
    -o android/app/src/main/jniLibs \
    build --release -p "$CRATE"

SO="android/app/src/main/jniLibs/$ABI/$LIB_NAME"
[ -f "$SO" ] || fail "cargo-ndk did not produce $SO"
echo "   $SO ($(stat -c %s "$SO") bytes)"
FILE_INFO="$(file "$SO")"
case "$FILE_INFO" in
  *"ELF 64-bit"*) ;;
  *) fail "$SO is not an ELF64 object: $FILE_INFO" ;;
esac

# --- 2. APK ------------------------------------------------------------------
echo "== gradle assembleDebug =="
(
  cd android
  # `clean` on purpose: an incremental packageDebug that repackages a *changed*
  # stored (uncompressed) .so leaves a ~13 MB zero-filled hole in the APK on
  # AGP 8.7 — the artifact then reports a bogus size. A clean package is 25 s
  # and the numbers in the report are the numbers you install.
  ./gradlew --no-daemon --console=plain clean assembleDebug
)

APK="android/app/build/outputs/apk/debug/app-debug.apk"
[ -f "$APK" ] || fail "Gradle did not produce $APK"

# --- 3. Assert the APK really carries what the checks need -------------------
echo "== APK assertions =="
# Capture once: `unzip -l | grep -q` under `pipefail` reports unzip's SIGPIPE
# (141) when grep exits at the first match, which would look like a missing .so.
APK_LISTING="$(unzip -l "$APK")"
case "$APK_LISTING" in
  *"lib/$ABI/$LIB_NAME"*) ;;
  *) fail "$APK does not contain lib/$ABI/$LIB_NAME" ;;
esac
case "$APK_LISTING" in
  *"AndroidManifest.xml"*) ;;
  *) fail "$APK does not contain AndroidManifest.xml" ;;
esac

AAPT="$SDK/build-tools/35.0.0/aapt"
if [ -x "$AAPT" ]; then
  # Same SIGPIPE reasoning as above: match in the shell, not through a pipe.
  BADGING="$("$AAPT" dump badging "$APK")"
  case "$BADGING" in
    *"package: name='app.suramak.spike' versionCode='3' versionName='0.3.0-m13b.2.1'"*) ;;
    *)
      # Fall back to a narrower match so a future versionCode bump does not
      # hide a real versionName regression.
      case "$BADGING" in
        *"package: name='app.suramak.spike'"*"versionName='0.3.0-m13b.2.1'"*) ;;
        *) fail "APK versionName is not 0.3.0-m13b.2.1 (see aapt output below)" ;;
      esac
      ;;
  esac
  case "$BADGING" in
    *"sdkVersion:'29'"*) ;;
    *) fail "APK minSdk is not 29" ;;
  esac
  case "$BADGING" in
    *"launchable-activity: name='app.suramak.spike.MainActivity'"*) ;;
    *) fail "APK has no launchable app.suramak.spike.MainActivity" ;;
  esac
  # v2.1: the crash banner's "Share log" route has to be in the *packaged*
  # manifest — a missing provider turns the button into an exception on the
  # phone, which is the one place it cannot be debugged.
  MANIFEST_TREE="$("$AAPT" dump xmltree "$APK" AndroidManifest.xml)"
  case "$MANIFEST_TREE" in
    *"androidx.core.content.FileProvider"*) ;;
    *) fail "APK manifest declares no FileProvider (Share log would fail on the phone)" ;;
  esac
  case "$MANIFEST_TREE" in
    *"app.suramak.spike.fileprovider"*) ;;
    *) fail "APK manifest has no app.suramak.spike.fileprovider authority" ;;
  esac
  # v2.1: back is intercepted through the platform dispatcher (see the back
  # handler); without this flag Android 16 keeps the gesture on the system path.
  case "$MANIFEST_TREE" in
    *"enableOnBackInvokedCallback"*) ;;
    *) fail "APK manifest does not enable the OnBackInvoked (predictive back) dispatcher" ;;
  esac
  printf '%s\n' "$BADGING" | grep -E "^(package|sdkVersion|targetSdkVersion|launchable-activity|native-code)" || true
fi

SIZE="$(stat -c %s "$APK")"
SHA="$(sha256sum "$APK" | cut -d' ' -f1)"

echo
echo "APK:     $APK"
echo "SIZE:    $SIZE bytes ($((SIZE / 1024 / 1024)) MiB)"
echo "SHA256:  $SHA"
echo "INSTALL: adb install -r $APK"
