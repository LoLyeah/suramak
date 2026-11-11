#!/usr/bin/env bash
# M16F — negative test for the static dylib gate.
#
# Proves the gate is not vacuous: starting from a bundle that passes, hide each
# bundled sherpa dylib in turn and require scripts/mac-check-dylibs.sh to fail.
# Then restore and require a pass again. If a hidden dylib still passes, the
# gate would ship a bundle that aborts at launch on a clean Mac — fail loudly.
#
# macOS only (otool/file). Run on the packaged bundle, after make-app.sh:
#   bash scripts/mac-check-dylibs-test.sh dist/Suramak.app
set -euo pipefail
cd "$(dirname "$0")/.."

APP="${1:-dist/Suramak.app}"
FW="$APP/Contents/Frameworks"
GATE="scripts/mac-check-dylibs.sh"
MANIFEST="packaging/sherpa-dylibs.tsv"

[ -d "$FW" ] || { echo "error: no $FW — package a parakeet bundle first" >&2; exit 1; }
[ -f "$MANIFEST" ] || { echo "error: no $MANIFEST" >&2; exit 1; }

# The positive baseline: with every dylib present, the gate must pass.
SURAMAK_EXPECT_PARAKEET=1 bash "$GATE" "$APP" >/dev/null

libs="$(awk -F'\t' '!/^#/ && NF >= 3 { print $1 }' "$MANIFEST")"
[ -n "$libs" ] || { echo "error: $MANIFEST lists no dylibs" >&2; exit 1; }

for lib in $libs; do
  [ -f "$FW/$lib" ] || {
    echo "error: $lib is not in the bundle — build with SURAMAK_FEATURES=…,parakeet" >&2
    exit 1
  }
  hidden="$FW/$lib.hidden-by-gate-test"
  moved=0
  restore() {
    if [ "$moved" -eq 1 ] && [ -f "$hidden" ]; then
      mv -f "$hidden" "$FW/$lib"
    fi
  }
  trap restore EXIT INT TERM

  mv "$FW/$lib" "$hidden"
  moved=1
  if SURAMAK_EXPECT_PARAKEET=1 bash "$GATE" "$APP" >/dev/null 2>&1; then
    restore
    moved=0
    trap - EXIT INT TERM
    echo "error: the gate PASSED without $lib — it does not cover the sherpa chain" >&2
    exit 1
  fi
  restore
  moved=0
  trap - EXIT INT TERM
  echo "gate-negative-ok: the gate fails when $lib is missing"
done

# And the restored bundle passes again (the test did not corrupt it).
SURAMAK_EXPECT_PARAKEET=1 bash "$GATE" "$APP" >/dev/null
echo "dylib-gate-negative-ok: each bundled sherpa dylib is load-bearing for the gate"
