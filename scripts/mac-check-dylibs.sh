#!/usr/bin/env bash
# Suramak — assert every @rpath dependency of a bundle's binary resolves.
#
# Why this exists: v0.3.0 shipped a macOS app that aborted at launch with
#   Library not loaded: @rpath/libswift_Concurrency.dylib
# because the release binary links the Swift bridge (`speech-transcriber`) but
# the bundle's only LC_RPATH was `@executable_path/../Frameworks`. The CI smoke
# launched a bundle built *without* that feature, so nothing noticed. This check
# is static: it fails the build when a dependency cannot resolve, before anyone
# downloads the artifact.
#
# Usage: scripts/mac-check-dylibs.sh [path/to/Suramak.app]   (default dist/Suramak.app)
set -euo pipefail

APP="${1:-dist/Suramak.app}"
BIN_DIR="$APP/Contents/MacOS"

[ -d "$BIN_DIR" ] || { echo "error: no $BIN_DIR — run make-app.sh first" >&2; exit 1; }

status=0
checked=0

for bin in "$BIN_DIR"/*; do
  [ -f "$bin" ] || continue
  checked=$((checked + 1))
  abs_dir="$(cd "$(dirname "$bin")" && pwd)"

  # LC_RPATH entries (one per line). macOS ships bash 3.2: no mapfile here.
  rpaths="$(otool -l "$bin" | awk '
    /cmd LC_RPATH/ { getline; getline; sub(/^[ \t]*path /, ""); sub(/ \(offset .*/, ""); print }
  ')"

  # Every dependency recorded in the load commands.
  while IFS= read -r dep; do
    case "$dep" in
      @rpath/*)
        suffix="${dep#@rpath/}"
        resolved=""
        while IFS= read -r rp; do
          [ -n "$rp" ] || continue
          expanded="${rp//@executable_path/$abs_dir}"
          expanded="${expanded//@loader_path/$abs_dir}"
          case "$expanded" in
            # macOS ships its Swift runtime in the dyld shared cache, not as
            # on-disk files: /usr/lib/swift/libswift_Concurrency.dylib does NOT
            # exist on disk, yet dyld resolves it there (v0.3.1's first release
            # run proved it — the runner also had no such file, and every
            # absolute /usr/lib/swift dep resolves at launch on real Macs). So
            # an rpath at /usr/lib/swift counts as resolved by construction.
            /usr/lib/swift | /usr/lib/swift/*)
              resolved="$expanded/$suffix (dyld shared cache)"
              ;;
          esac
          if [ -z "$resolved" ] && [ -e "$expanded/$suffix" ]; then
            resolved="$expanded/$suffix"
          fi
          [ -n "$resolved" ] && break
        done <<< "$rpaths"
        if [ -n "$resolved" ]; then
          echo "ok:   $(basename "$bin") -> $dep"
        else
          echo "FAIL: $(basename "$bin") -> $dep cannot be resolved" >&2
          echo "      LC_RPATH entries: ${rpaths:-<none>}" >&2
          status=1
        fi
        ;;
    esac
  done < <(otool -L "$bin" | tail -n +2 | awk '{print $1}')
done

if [ "$checked" -eq 0 ]; then
  echo "error: no binaries under $BIN_DIR" >&2
  exit 1
fi

if [ "$status" -ne 0 ]; then
  echo "error: unresolved @rpath dependencies — this bundle would abort at launch" >&2
  exit 1
fi

echo "dylibs-ok: every @rpath dependency of $APP resolves"
