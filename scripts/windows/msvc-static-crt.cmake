# M14a — Windows static-CRT toolchain file (plan D4).
#
# Reached through the `CMAKE_TOOLCHAIN_FILE` environment variable, which both
# cmake-rs (libopus via opusic-sys) and whisper-rs-sys (whisper.cpp) honour, so
# every CMake-built C dependency of the workspace sees it. A strict no-op on
# non-Windows hosts — Linux and macOS builds are untouched even if the variable
# is set (and `scripts/android/build.sh` exports its own NDK toolchain file,
# which wins over the environment because cargo/CI only set this one there).
#
# Why it exists: Rust is compiled with `-C target-feature=+crt-static`
# (`.cargo/config.toml`), so the C objects must use the static CRT (`/MT`) too.
# A mismatch is a hard `LNK2038: mismatch detected for 'RuntimeLibrary'` at link
# time, and any surviving VCRUNTIME/MSVCP import fails the CI `dumpbin` step.

if(NOT CMAKE_HOST_SYSTEM_NAME STREQUAL "Windows")
  return()
endif()

# CMake's own switch — whisper.cpp + ggml. The CI job also exports
# `CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded` as an environment variable, which
# whisper-rs-sys forwards as a CMake define; this is the same value.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded" CACHE STRING "" FORCE)

# libopus overrides `CMAKE_MSVC_RUNTIME_LIBRARY` from its own option
# (opus/CMakeLists.txt: `if(OPUS_STATIC_RUNTIME) set(CMAKE_MSVC_RUNTIME_LIBRARY
# "MultiThreaded")`), and opusic-sys forwards no `CMAKE_*` environment
# variables, so this file is the only way to reach that switch.
set(OPUS_STATIC_RUNTIME ON CACHE BOOL "" FORCE)

# Ninja + MSVC needs `cl.exe` discoverable by CMake itself (it is not on PATH
# unless the Visual Studio developer environment is active, which the runners
# do not activate). Defining a toolchain file makes cmake-rs skip its own
# `-DCMAKE_C_COMPILER=<cl>` hint (cmake-rs only passes that hint when no
# toolchain file is defined), and opusic-sys picks Ninja whenever `ninja`
# exists — as it does on the GitHub windows runners. So locate cl.exe here:
# first on PATH, then in a standard Visual Studio 2022 install. If neither
# finds it, CMake's own detection runs exactly as it would have without this
# block. `link.exe`/`lib.exe` live next to `cl.exe`, so the MSVC platform
# modules resolve the rest from the compiler path.
if(CMAKE_GENERATOR MATCHES "Ninja" AND NOT CMAKE_C_COMPILER)
  find_program(MSVC_CL NAMES cl.exe)
  if(NOT MSVC_CL)
    file(GLOB MSVC_CL_CANDIDATES
      "$ENV{ProgramFiles}/Microsoft Visual Studio/2022/*/VC/Tools/MSVC/*/bin/Hostx64/x64/cl.exe"
      "$ENV{ProgramW6432}/Microsoft Visual Studio/2022/*/VC/Tools/MSVC/*/bin/Hostx64/x64/cl.exe")
    if(MSVC_CL_CANDIDATES)
      list(LENGTH MSVC_CL_CANDIDATES MSVC_CL_COUNT)
      math(EXPR MSVC_CL_LAST "${MSVC_CL_COUNT} - 1")
      list(GET MSVC_CL_CANDIDATES ${MSVC_CL_LAST} MSVC_CL)
    endif()
  endif()
  if(MSVC_CL)
    message(STATUS "suramak: Ninja + MSVC — using cl.exe at ${MSVC_CL}")
    set(CMAKE_C_COMPILER "${MSVC_CL}" CACHE FILEPATH "" FORCE)
    set(CMAKE_CXX_COMPILER "${MSVC_CL}" CACHE FILEPATH "" FORCE)
  endif()
endif()
