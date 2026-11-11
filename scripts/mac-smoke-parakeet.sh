#!/usr/bin/env bash
# M16F — clean-machine dynamic-load smoke for the Parakeet fast engine.
#
# What this proves, and what it does not:
#   - the packaged bundle is copied OUT of the repo into a fresh directory, then
#     `suramak-cli doctor` runs with a scrubbed environment (`env -i`: no
#     DYLD_LIBRARY_PATH, no PATH into cargo, no inherited HOME);
#   - dyld's own loader trace names BOTH sherpa libraries inside
#     `Contents/Frameworks` of the copied bundle and no cargo/sherpa-rs/target
#     path anywhere in the loaded images;
#   - the build reports the `parakeet` feature compiled in.
# It does NOT load the 670 MB Parakeet model or run a decode: model loading and
# Core ML/ANE engagement on real Apple Silicon are the owner-Mac pass (the work
# order's honest boundary). A CI run with a pre-installed model can add
# `--model-dir` later without changing this contract.
#
# Usage: bash scripts/mac-smoke-parakeet.sh [dist/Suramak.app]
set -euo pipefail
cd "$(dirname "$0")/.."

APP="${1:-dist/Suramak.app}"
[ -d "$APP/Contents/Frameworks" ] || {
  echo "error: no $APP/Contents/Frameworks — package a parakeet bundle first" >&2
  exit 1
}

fail() { echo "error: $*" >&2; exit 1; }

app_bin="$APP/Contents/MacOS/suramak"
cli_bin="$APP/Contents/MacOS/suramak-cli"
[ -x "$cli_bin" ] || fail "no suramak-cli in the bundle (make-app.sh copies it when built)"
[ -x "$app_bin" ] || fail "no suramak binary in the bundle"

# The app binary must link the engine — otherwise the feature is not in the
# artifact that ships, even if some helper still is.
otool -L "$app_bin" | grep -q '@rpath/libsherpa-onnx-c-api.dylib' ||
  fail "$app_bin does not link @rpath/libsherpa-onnx-c-api.dylib (parakeet feature missing?)"

SMOKE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/suramak-parakeet-smoke.XXXXXX")"
trap 'rm -rf "$SMOKE_ROOT"' EXIT INT TERM
# macOS: mktemp returns the /var/... alias of TMPDIR, but dyld prints the
# RESOLVED /private/var/... path. Match the trace against the canonical path
# or the check compares two spellings of the same directory and "fails".
SMOKE_REAL="$(cd "$SMOKE_ROOT" && pwd -P)"
SMOKE_APP="$SMOKE_ROOT/Suramak.app"
mkdir -p "$SMOKE_ROOT/home" "$SMOKE_ROOT/tmp"
ditto "$APP" "$SMOKE_APP"

out="$SMOKE_ROOT/doctor.txt"
libs="$SMOKE_ROOT/dyld.txt"
# `env -i` is the clean machine: only what a fresh macOS user session has.
if ! env -i \
  HOME="$SMOKE_ROOT/home" \
  TMPDIR="$SMOKE_ROOT/tmp" \
  PATH=/usr/bin:/bin:/usr/sbin:/sbin \
  DYLD_PRINT_LIBRARIES=1 \
  "$SMOKE_APP/Contents/MacOS/suramak-cli" doctor >"$out" 2>"$libs"; then
  cat "$out" >&2 || true
  cat "$libs" >&2 || true
  fail "suramak-cli doctor failed inside the copied bundle"
fi

grep -q "compiled in (sherpa-onnx linked" "$out" ||
  fail "doctor does not report the parakeet feature compiled in"

c_api="$SMOKE_APP/Contents/Frameworks/libsherpa-onnx-c-api.dylib"
onnx="$(ls "$SMOKE_APP/Contents/Frameworks"/libonnxruntime.*.dylib 2>/dev/null | head -1 || true)"
[ -f "$c_api" ] || fail "no libsherpa-onnx-c-api.dylib in the copied bundle"
[ -n "$onnx" ] || fail "no libonnxruntime.*.dylib in the copied bundle"

# dyld prints the path it used; @executable_path expansion can leave a
# `Contents/MacOS/../Frameworks` segment in it, so match on "a line naming this
# library that also lives under the copied bundle" rather than a canonical path.
loaded_from_bundle() { # <basename> <human label>
  grep -F "$1" "$libs" >/dev/null 2>&1 || {
    cat "$libs" >&2
    fail "dyld did not load the $2 ($1)"
  }
  grep -F "$1" "$libs" | grep -F "$SMOKE_REAL/Suramak.app/" >/dev/null || {
    cat "$libs" >&2
    fail "the $2 ($1) was loaded from outside the copied bundle"
  }
}
loaded_from_bundle libsherpa-onnx-c-api.dylib "sherpa c-api"
loaded_from_bundle "$(basename "$onnx")" "onnxruntime"

# The v0.3.0 class, inverted: no loaded image may come from the build tree.
# (The smoke bundle itself lives under $TMPDIR, which may be under /Users on a
# CI runner — only cargo/target/sherpa-cache paths are dev paths.)
if grep -E '\.cargo/|sherpa-rs|/target/(release|debug)/|/build/m16df' "$libs" >/dev/null; then
  grep -E '\.cargo/|sherpa-rs|/target/(release|debug)/|/build/m16df' "$libs" >&2
  fail "dyld loaded a library from a dev path — the bundle is not self-contained"
fi

echo "parakeet-smoke-ok: dyld loaded $(basename "$c_api") + $(basename "$onnx") from Contents/Frameworks"
echo "                   (clean env, copied bundle, no dev paths)"
