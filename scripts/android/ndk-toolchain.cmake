# Suramak — CMake toolchain shim for the Rust Android cross-build (M13a).
#
# Why this file exists: `whisper-rs-sys` drives CMake through the `cmake`
# crate, which knows only a toolchain file. The NDK's own
# `android.toolchain.cmake` takes the ABI and platform from CMake variables
# (`ANDROID_ABI` / `ANDROID_PLATFORM`) — and with them unset it silently
# defaults to armeabi-v7a. `cargo-ndk` exports both in the environment, one
# correct value per target, but neither the `cmake` crate nor whisper-rs-sys
# forwards them into CMake. This shim bridges that gap: it reads the values
# cargo-ndk exported and then includes the NDK's official toolchain file.
#
# `scripts/android/build.sh` exports CMAKE_TOOLCHAIN_FILE pointing here.
# Included by whisper.cpp's CMake configure only; the Rust-side link uses the
# NDK clang cargo-ndk configures directly.

if(NOT DEFINED ANDROID_ABI)
  set(ANDROID_ABI "$ENV{ANDROID_ABI}")
endif()
if(NOT ANDROID_ABI)
  message(FATAL_ERROR
    "suramak ndk-toolchain.cmake: ANDROID_ABI is not set. Build through "
    "scripts/android/build.sh — cargo-ndk exports ANDROID_ABI per target.")
endif()

if(NOT DEFINED ANDROID_PLATFORM)
  set(ANDROID_PLATFORM "android-$ENV{ANDROID_PLATFORM}")
endif()
if(NOT ANDROID_PLATFORM OR ANDROID_PLATFORM STREQUAL "android-")
  message(FATAL_ERROR
    "suramak ndk-toolchain.cmake: ANDROID_PLATFORM is not set. Build through "
    "scripts/android/build.sh — cargo-ndk exports ANDROID_PLATFORM.")
endif()

if("$ENV{ANDROID_NDK_HOME}" STREQUAL "")
  message(FATAL_ERROR
    "suramak ndk-toolchain.cmake: ANDROID_NDK_HOME is not set. Build through "
    "scripts/android/build.sh.")
endif()

include("$ENV{ANDROID_NDK_HOME}/build/cmake/android.toolchain.cmake")
