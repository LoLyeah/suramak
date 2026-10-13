#!/usr/bin/env bash
# Full Mac smoke (M4 release handoff) — run on the Mac, from the repo root:
#
#   bash scripts/mac-smoke.sh
#
# release build -> make-app.sh (icon included) -> bundle smoke -> doctor ->
# CLI e2e on fixtures/audio/en.wav -> pointer to docs/USER_SMOKE.md -> open the app.
#
# Plain bash, no sudo, fails loudly: every step must exit 0 or the script stops
# (set -euo pipefail). Prerequisites: Xcode command line tools (codesign/iconutil),
# Rust (rustup), CMake (`brew install cmake`), and network for the one-time tiny
# model download.
set -euo pipefail
cd "$(dirname "$0")/.."

# Preflight: fail fast with a fix hint instead of a cryptic build-script panic.
for tool in cmake cargo; do
  command -v "$tool" >/dev/null || {
    echo "error: '$tool' not found on PATH." >&2
    echo "  cmake  -> brew install cmake   (or https://cmake.org/download)" >&2
    echo "  cargo  -> https://rustup.rs" >&2
    exit 1
  }
done
xcode-select -p >/dev/null 2>&1 || {
  echo "error: Xcode command line tools missing. Run: xcode-select --install" >&2
  exit 1
}

CLI="target/release/suramak-cli"
APP="dist/Suramak.app/Contents/MacOS/suramak"

step() { printf '\n=== %s ===\n' "$1"; }

step "1/7 release build (app + CLI)"
cargo build --release -p suramak-ui -p suramak-cli

step "2/7 package dist/Suramak.app (icon included)"
./scripts/make-app.sh

step "3/7 model check (tiny — short-circuits when already installed)"
"$CLI" models download tiny

step "4/7 bundle smoke (3 s)"
mkdir -p build
SURAMAK_SMOKE_SECS=3 "$APP"

step "5/7 doctor (engine stack + encoder pairing)"
"$CLI" doctor

step "6/7 CLI e2e on fixtures/audio/en.wav"
"$CLI" transcribe fixtures/audio/en.wav --model tiny --lang en --formats json,md --out-dir build/out
grep -qi fox build/out/en.json
echo "CLI e2e OK — build/out/en.json"

step "7/7 handoff"
echo "Full checklist for the owner: docs/USER_SMOKE.md"
open dist/Suramak.app

echo
echo "mac smoke complete."
