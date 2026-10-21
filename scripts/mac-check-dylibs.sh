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

# The app binary and its rpaths (the consuming image for embedded frameworks).
app_bin="$(find "$BIN_DIR" -maxdepth 1 -type f | sort | head -1)"
app_dir="$(cd "$(dirname "$app_bin")" && pwd)"
app_rpaths="$(otool -l "$app_bin" | awk '
  /cmd LC_RPATH/ { getline; getline; sub(/^[ \t]*path /, ""); sub(/ \(offset .*/, ""); print }
')"

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
        # First pass: the checked binary's own rpaths, expanded against its own
        # directory (correct for executables in Contents/MacOS and for
        # @loader_path inside embedded frameworks).
        while IFS= read -r rp; do
          [ -n "$rp" ] || continue
          expanded="${rp//@executable_path/$abs_dir}"
          expanded="${expanded//@loader_path/$abs_dir}"
          case "$expanded" in
            # The dyld shared-cache carve-out (comment preserved verbatim):
            # /usr/lib/swift has no on-disk files, dyld still resolves it.
            /usr/lib/swift | /usr/lib/swift/*)
              resolved="$expanded/$suffix (dyld shared cache)"
              ;;
          esac
          if [ -z "$resolved" ] && [ -e "$expanded/$suffix" ]; then
            resolved="$expanded/$suffix"
          fi
          [ -n "$resolved" ] && break
        done <<< "$rpaths"
        # Second pass (SUR2-NEW-06 macOS false-fail): an embedded framework's
        # `@rpath/…` dependency resolves against the CONSUMING image's rpaths —
        # @executable_path then means the APP binary's directory, and the
        # framework's own rpath list is legitimately empty (Sparkle ships no
        # LC_RPATH; its location comes from the app's rpath entry).
        if [ -z "$resolved" ]; then
          while IFS= read -r rp; do
            [ -n "$rp" ] || continue
            expanded="${rp//@executable_path/$app_dir}"
            expanded="${expanded//@loader_path/$abs_dir}"
            case "$expanded" in
              /usr/lib/swift | /usr/lib/swift/*)
                resolved="$expanded/$suffix (dyld shared cache)"
                ;;
            esac
            if [ -z "$resolved" ] && [ -e "$expanded/$suffix" ]; then
              resolved="$expanded/$suffix (via the app's rpath)"
            fi
            [ -n "$resolved" ] && break
          done <<< "$app_rpaths"
        fi
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
