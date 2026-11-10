#!/usr/bin/env bash
# Suramak — M13b/M13d/M13e: build the installable debug APK for the shell.
#
#   scripts/android/build-apk.sh
#
# What it does, in order (one source of truth; CI runs this exact file):
#   1. discovers the pinned toolchain (NDK r27d, a full JDK 17, the Android SDK);
#   2. cross-builds both Rust cdylibs for arm64-v8a with cargo-ndk, dropping
#      libsuramak_android_spike.so (the M13b shell) and libsuramak_android.so
#      (the M13d model host + M13e import/transcription host) into
#      android/app/src/main/jniLibs/;
#   3. runs ./gradlew assembleDebug (Kotlin shim + manifest + jniLibs → APK);
#   4. asserts the APK exists and really contains both .so files, the right
#      version, the crash-log FileProvider, the M13d download service, the M13e
#      share matrix / SAF / transcription service and both bridges' JNI symbols,
#      then prints its path, size and sha256.
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
SPIKE_CRATE="suramak-android-spike"
SPIKE_LIB="libsuramak_android_spike.so"
APP_CRATE="suramak-android"
APP_LIB="libsuramak_android.so"

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

# --- 1. Rust cdylibs ----------------------------------------------------------
# RUSTFLAGS is scoped to this invocation on purpose: the desktop crates never
# inherit an Android linker flag from an ambient environment.
echo "== cargo ndk build ($SPIKE_CRATE, release) =="
RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" \
  cargo ndk -t "$ABI" --platform "$PLATFORM" \
    -o android/app/src/main/jniLibs \
    build --release -p "$SPIKE_CRATE"

# M13d: the real app crate. Its `libsuramak_android.so` carries the model-host
# JNI symbol the JobService calls. Both libraries share `target/`, so this is a
# link — the whisper/ggml archives are built once.
echo "== cargo ndk build ($APP_CRATE, release) =="
RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" \
  cargo ndk -t "$ABI" --platform "$PLATFORM" \
    -o android/app/src/main/jniLibs \
    build --release -p "$APP_CRATE"

SO="android/app/src/main/jniLibs/$ABI/$SPIKE_LIB"
[ -f "$SO" ] || fail "cargo-ndk did not produce $SO"
echo "   $SO ($(stat -c %s "$SO") bytes)"
FILE_INFO="$(file "$SO")"
case "$FILE_INFO" in
  *"ELF 64-bit"*) ;;
  *) fail "$SO is not an ELF64 object: $FILE_INFO" ;;
esac

APP_SO="android/app/src/main/jniLibs/$ABI/$APP_LIB"
[ -f "$APP_SO" ] || fail "cargo-ndk did not produce $APP_SO"
echo "   $APP_SO ($(stat -c %s "$APP_SO") bytes)"
FILE_INFO="$(file "$APP_SO")"
case "$FILE_INFO" in
  *"ELF 64-bit"*) ;;
  *) fail "$APP_SO is not an ELF64 object: $FILE_INFO" ;;
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
  *"lib/$ABI/$SPIKE_LIB"*) ;;
  *) fail "APK does not contain lib/$ABI/$SPIKE_LIB" ;;
esac
# M13d: the JobService's JNI symbols live in this library; an APK without it
# would crash the moment a download starts.
case "$APK_LISTING" in
  *"lib/$ABI/$APP_LIB"*) ;;
  *) fail "APK does not contain lib/$ABI/$APP_LIB" ;;
esac
case "$APK_LISTING" in
  *"AndroidManifest.xml"*) ;;
  *) fail "APK does not contain AndroidManifest.xml" ;;
esac

AAPT="$SDK/build-tools/35.0.0/aapt"
if [ -x "$AAPT" ]; then
  # Same SIGPIPE reasoning as above: match in the shell, not through a pipe.
  BADGING="$("$AAPT" dump badging "$APK")"
  case "$BADGING" in
    *"package: name='app.suramak.spike' versionCode='7' versionName='0.4.0-m13e.1'"*) ;;
    *)
      # Fall back to a narrower match so a future versionCode bump does not
      # hide a real versionName regression.
      case "$BADGING" in
        *"package: name='app.suramak.spike'"*"versionName='0.4.0-m13e.1'"*) ;;
        *) fail "APK versionName is not 0.4.0-m13e.1 (see aapt output below)" ;;
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
  # M13d: the model-download job and its dataSync type. A missing service or
  # permission turns every download into a silent no-op on the phone.
  case "$MANIFEST_TREE" in
    *"app.suramak.models.ModelDownloadJobService"*) ;;
    *) fail "APK manifest declares no app.suramak.models.ModelDownloadJobService" ;;
  esac
  case "$MANIFEST_TREE" in
    *"BIND_JOB_SERVICE"*) ;;
    *) fail "APK manifest does not protect the JobService with BIND_JOB_SERVICE" ;;
  esac
  case "$MANIFEST_TREE" in
    *"FOREGROUND_SERVICE_DATA_SYNC"*) ;;
    *) fail "APK manifest does not declare FOREGROUND_SERVICE_DATA_SYNC" ;;
  esac
  case "$MANIFEST_TREE" in
    *"android.permission.INTERNET"*) ;;
    *) fail "APK manifest does not declare INTERNET — no model could download" ;;
  esac
  case "$MANIFEST_TREE" in
    *"app.suramak.models.CancelModelDownloadReceiver"*) ;;
    *) fail "APK manifest declares no cancel receiver for the download notification" ;;
  esac
  # M13e: the share matrix. A missing action or mime type means the app never
  # appears in the share sheet for that voice note — the one place the owner
  # cannot see a log line.
  case "$MANIFEST_TREE" in
    *"android.intent.action.SEND_MULTIPLE"*) ;;
    *) fail "APK manifest declares no ACTION_SEND_MULTIPLE filter" ;;
  esac
  case "$MANIFEST_TREE" in
    *"android.intent.action.SEND"*) ;;
    *) fail "APK manifest declares no ACTION_SEND filter" ;;
  esac
  for MIME in "audio/*" "application/ogg" "application/x-ogg" "audio/opus" "audio/ogg" "audio/webm" "video/webm" "video/mp4" "video/x-matroska"; do
    case "$MANIFEST_TREE" in
      *"mimeType(0x01010026)=\"$MIME\""*) ;;
      *) fail "APK share matrix is missing $MIME" ;;
    esac
  done
  case "$MANIFEST_TREE" in
    *"android.intent.action.OPEN_DOCUMENT"*) ;;
    *) fail "APK manifest declares no ACTION_OPEN_DOCUMENT (SAF) filter" ;;
  esac
  # M13e: the transcription service, both foreground types and the wake lock.
  case "$MANIFEST_TREE" in
    *"app.suramak.transcribe.TranscriptionJobService"*) ;;
    *) fail "APK manifest declares no app.suramak.transcribe.TranscriptionJobService" ;;
  esac
  case "$MANIFEST_TREE" in
    *"foregroundServiceType"*"0x2001"*) ;;
    *) fail "the transcription service does not declare mediaProcessing|dataSync (0x2001)" ;;
  esac
  case "$MANIFEST_TREE" in
    *"FOREGROUND_SERVICE_MEDIA_PROCESSING"*) ;;
    *) fail "APK manifest does not declare FOREGROUND_SERVICE_MEDIA_PROCESSING" ;;
  esac
  case "$MANIFEST_TREE" in
    *"android.permission.WAKE_LOCK"*) ;;
    *) fail "APK manifest does not declare WAKE_LOCK — the screen-off job would stall" ;;
  esac
  case "$MANIFEST_TREE" in
    *"app.suramak.transcribe.CancelTranscriptionReceiver"*) ;;
    *) fail "APK manifest declares no cancel receiver for the transcription notification" ;;
  esac
  case "$MANIFEST_TREE" in
    *"android.app.shortcuts"*) ;;
    *) fail "APK manifest does not register the launcher shortcuts (SAF import entry)" ;;
  esac

  # M13e: the JNI symbols both JobServices call. A missing symbol is an
  # UnsatisfiedLinkError the first time a job starts, on the phone only.
  LLVM_NM="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-nm"
  if [ -x "$LLVM_NM" ]; then
    SYMBOLS="$("$LLVM_NM" -D --defined-only "$APP_SO")"
    case "$SYMBOLS" in
      *"Java_app_suramak_models_ModelsBridge_nativeCommand"*) ;;
      *) fail "$APP_SO exports no ModelsBridge.nativeCommand symbol" ;;
    esac
    case "$SYMBOLS" in
      *"Java_app_suramak_transcribe_TranscribeBridge_nativeCommand"*) ;;
      *) fail "$APP_SO exports no TranscribeBridge.nativeCommand symbol" ;;
    esac
  else
    fail "no llvm-nm at $LLVM_NM (cannot verify the JNI symbols)"
  fi
  printf '%s\n' "$BADGING" | grep -E "^(package|sdkVersion|targetSdkVersion|launchable-activity|native-code)" || true
fi

SIZE="$(stat -c %s "$APK")"
SHA="$(sha256sum "$APK" | cut -d' ' -f1)"

echo
echo "APK:     $APK"
echo "SIZE:    $SIZE bytes ($((SIZE / 1024 / 1024)) MiB)"
echo "SHA256:  $SHA"
echo "INSTALL: adb install -r $APK"
