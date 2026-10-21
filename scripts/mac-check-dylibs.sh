#!/usr/bin/env bash
# Suramak — assert every @rpath dependency of a bundle's binaries resolves.
#
# Why this exists: v0.3.0 shipped a macOS app that aborted at launch with
#   Library not loaded: @rpath/libswift_Concurrency.dylib
# because the release binary links the Swift bridge (`speech-transcriber`) but
# the bundle's only LC_RPATH was `@executable_path/../Frameworks`. The CI smoke
# launched a bundle built *without* that feature, so nothing noticed. This check
# is static: it fails the build when a dependency cannot resolve, before anyone
# downloads the artifact.
#
# Scope (SUR2-NEW-06): **every Mach-O file in the bundle**, not just
# `Contents/MacOS/*`. Embedded code under `Contents/Frameworks` — Sparkle's
# framework, its Autoupdate/Updater.app helpers and the two XPC services — has
# its own LC_RPATH and its own @rpath dependencies; checking only the app's
# executables passed a bundle whose framework would fail to load.
#
# Usage: scripts/mac-check-dylibs.sh [path/to/Suramak.app]   (default dist/Suramak.app)
set -euo pipefail

APP="${1:-dist/Suramak.app}"
BIN_DIR="$APP/Contents/MacOS"

[ -d "$BIN_DIR" ] || { echo "error: no $BIN_DIR — run make-app.sh first" >&2; exit 1; }

status=0
checked=0

# NUL-separated candidates from both roots; `find -print0` + `read -d ''` keeps a
# path with spaces in one piece.
bundle_files() {
  local root
  for root in "$APP/Contents/MacOS" "$APP/Contents/Frameworks"; do
    [ -d "$root" ] || continue
    find "$root" -type f -print0
  done
}

# The dependency paths recorded in the load commands, one per line.
#
# `otool -L` prints "\t<path> (compatibility version …)" and a path may contain
# spaces, so strip the trailing annotation from the end of the line instead of
# splitting each line on whitespace, which truncated such a path and reported a
# false failure (SUR2-NEW-06).
dependency_paths() {
  otool -L "$1" | tail -n +2 | sed -n 's/^[[:space:]]*\(.*\) (compatibility version .*$/\1/p'
}

while IFS= read -r -d '' bin; do
  # Only Mach-O files are handed to otool: a framework also holds plists,
  # headers and .tbd stubs, and otool would fail the run on the first of them.
  case "$(file -b "$bin" 2>/dev/null || true)" in
    *Mach-O*) ;;
    *) continue ;;
  esac
  checked=$((checked + 1))
  label="${bin#"$APP"/}"
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
          echo "ok:   $label -> $dep"
        else
          echo "FAIL: $label -> $dep cannot be resolved" >&2
          echo "      LC_RPATH entries: ${rpaths:-<none>}" >&2
          status=1
        fi
        ;;
    esac
  done < <(dependency_paths "$bin")
done < <(bundle_files)

if [ "$checked" -eq 0 ]; then
  echo "error: no Mach-O binaries under $APP/Contents" >&2
  exit 1
fi

if [ "$status" -ne 0 ]; then
  echo "error: unresolved @rpath dependencies — this bundle would abort at launch" >&2
  exit 1
fi

echo "dylibs-ok: every @rpath dependency of $checked bundle binaries resolves"
