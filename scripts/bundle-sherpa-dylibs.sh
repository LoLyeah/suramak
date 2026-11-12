#!/usr/bin/env bash
# M16F — bundle the sherpa-onnx shared libraries into Contents/Frameworks.
#
# Called by scripts/make-app.sh **only** when the `parakeet` feature is in
# SURAMAK_FEATURES. macOS only: it needs otool + install_name_tool.
#
# What it guarantees, in order:
#   1. every file in packaging/sherpa-dylibs.tsv is verified (byte size + sha256)
#      against the *pristine* copy sherpa-rs-sys left in target/release, before
#      anything enters the bundle;
#   2. the two libraries the app links (sherpa-onnx's C API and its ONNX Runtime)
#      are copied into Contents/Frameworks — never the cxx-api library, which
#      sherpa-rs does not link;
#   3. each bundled copy carries the bundle's own LC_RPATH,
#      `@executable_path/../Frameworks`, so `otool -l` on the dylib names the
#      Frameworks directory (the gate in scripts/mac-check-dylibs.sh asserts it);
#   4. every @rpath dependency of the bundled C API resolves inside the bundle.
#
# The rpath edit is deliberately packaging-only: the prebuilt dylibs arrive with
# install names of the form @rpath/… but no LC_RPATH of their own, and a
# self-contained copy is what makes the dylib valid independent of the loading
# image's rpath stack. install_name_tool rewrites load commands, so the bundled
# bytes are not byte-identical to the pinned upstream artifact — the pins cover
# the upstream artifact, and make-app.sh ad-hoc signs the patched copy afterwards
# (inside-out, before the app).
#
# Usage: scripts/bundle-sherpa-dylibs.sh dist/Suramak.app
#        SHERPA_DYLIB_DIR=/path/to/lib scripts/bundle-sherpa-dylibs.sh …
set -euo pipefail
cd "$(dirname "$0")/.."

APP="${1:-}"
if [ -z "$APP" ] || [ ! -d "$APP/Contents/MacOS" ]; then
  echo "error: usage: scripts/bundle-sherpa-dylibs.sh path/to/Suramak.app" >&2
  exit 1
fi
for tool in otool install_name_tool shasum; do
  command -v "$tool" >/dev/null || {
    echo "error: $tool is missing — sherpa dylib bundling is macOS-only" >&2
    exit 1
  }
done

MANIFEST="packaging/sherpa-dylibs.tsv"
SRC_DIR="${SHERPA_DYLIB_DIR:-target/release}"
FW="$APP/Contents/Frameworks"
RPATH='@executable_path/../Frameworks'

[ -f "$MANIFEST" ] || { echo "error: no $MANIFEST" >&2; exit 1; }
[ -d "$SRC_DIR" ] || {
  echo "error: $SRC_DIR does not exist — build with the parakeet feature first" >&2
  exit 1
}

# The LC_RPATH entries of a Mach-O file, one per line (macOS bash 3.2: no mapfile).
rpaths_of() {
  otool -l "$1" | awk '
    /cmd LC_RPATH/ { getline; getline; sub(/^[ \t]*path /, ""); sub(/ \(offset .*/, ""); print }
  '
}

# Pass 0 — recover a missing pristine dylib from sherpa-rs-sys's own download
# cache. A restored `target/` counts the build script as fresh, so it never
# re-copies the runtime next to the binaries (CI run 43186421484 failed exactly
# here: `libsherpa-onnx-c-api.dylib is missing from target/release`). The pin
# verification in pass 1 still decides whether the recovered bytes are the real
# thing — this step only decides where to look.
cache_dir="${SHERPA_CACHE_DIR:-$HOME/Library/Caches/sherpa-rs}"
while IFS=$'\t' read -r name bytes sha; do
  case "$name" in ''|'#'*) continue ;; esac
  [ -f "$SRC_DIR/$name" ] && continue
  [ -d "$cache_dir" ] || continue
  found="$(find "$cache_dir" -type f -name "$name" 2>/dev/null | head -n 1)"
  [ -n "$found" ] || continue
  cp -f "$found" "$SRC_DIR/$name"
  echo "sherpa: recovered $name from $found (a restored target/ skipped the build-script copy)"
done < "$MANIFEST"

# Pass 1 — verify every pin before touching the bundle. A mismatch is a
# supply-chain failure: stop with the expected/actual pair, never copy.
while IFS=$'\t' read -r name bytes sha; do
  case "$name" in ''|'#'*) continue ;; esac
  src="$SRC_DIR/$name"
  [ -f "$src" ] || {
    echo "error: $name is missing from $SRC_DIR" >&2
    echo "       (the bundle must be built with the parakeet feature; see docs/ENGINES.md)" >&2
    exit 1
  }
  actual_bytes="$(stat -f%z "$src")"
  [ "$actual_bytes" = "$bytes" ] || {
    echo "error: $name is $actual_bytes bytes, pinned $bytes — stale or foreign dylib" >&2
    exit 1
  }
  actual_sha="$(shasum -a 256 "$src" | awk '{print $1}')"
  [ "$actual_sha" = "$sha" ] || {
    echo "error: $name sha256 mismatch" >&2
    echo "       expected $sha" >&2
    echo "       got      $actual_sha" >&2
    exit 1
  }
done < "$MANIFEST"

# Pass 2 — copy + give each copy the bundle's rpath.
mkdir -p "$FW"
while IFS=$'\t' read -r name bytes sha; do
  case "$name" in ''|'#'*) continue ;; esac
  cp -f "$SRC_DIR/$name" "$FW/$name"
  chmod u+w "$FW/$name"
  # Capture then match: a live `awk | grep -q` lets grep exit early, awk dies
  # on SIGPIPE, and `set -o pipefail` turns a MATCHING rpath into a failure.
  existing_rpaths="$(rpaths_of "$FW/$name")"
  case "$existing_rpaths" in
    *"$RPATH"*) ;;
    *)
      install_name_tool -add_rpath "$RPATH" "$FW/$name"
      ;;
  esac
  echo "sherpa: bundled $name ($bytes bytes, sha256 verified, rpath $RPATH)"
done < "$MANIFEST"

# Pass 3 — the app binary must carry the bundle rpath at link time: dyld resolves
# @rpath/libsherpa-onnx-c-api.dylib through the main executable's rpaths. That
# entry is a link-time argument (crates/suramak-ui/build.rs, `updates` or
# `parakeet`); patching the executable here would be too late to be trustworthy.
app_bin="$APP/Contents/MacOS/suramak"
if [ -x "$app_bin" ]; then
  # Capture then match (the SIGPIPE/pipefail trap again — see pass 2).
  app_rpaths="$(rpaths_of "$app_bin")"
  case "$app_rpaths" in
    *"$RPATH"*) ;;
    *)
      echo "error: $app_bin has no LC_RPATH $RPATH" >&2
      echo "       (crates/suramak-ui/build.rs must emit it for this feature set)" >&2
      exit 1
      ;;
  esac
fi

# Pass 4 — the copied C API must resolve its own @rpath dependencies inside the
# bundle. This duplicates one slice of mac-check-dylibs.sh on purpose: a failure
# here names the bundling step, before codesign runs over a broken layout.
c_api="$FW/libsherpa-onnx-c-api.dylib"
[ -f "$c_api" ] || { echo "error: $c_api missing after copy" >&2; exit 1; }
while IFS= read -r dep; do
  case "$dep" in
    @rpath/*)
      suffix="${dep#@rpath/}"
      if [ ! -e "$FW/$suffix" ]; then
        echo "error: $c_api -> $dep does not resolve in $FW" >&2
        exit 1
      fi
      ;;
  esac
done < <(otool -L "$c_api" | tail -n +2 | sed -n 's/^[[:space:]]*\(.*\) (compatibility version .*$/\1/p')

echo "sherpa: Contents/Frameworks carries the Parakeet runtime (no dev paths)"
