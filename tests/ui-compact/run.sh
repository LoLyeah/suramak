#!/usr/bin/env bash
# M13g1/M13g2 — the phone-viewport screenshot suite (a CI gate since M13g2).
#
# Renders the compact profile at the plan's portrait viewport (1216 × 2640
# physical, scale 2.6) and writes the release-surface shots under
# tests/ui-compact/shots/: launch-library, library, library-light, library-empty,
# first-run-download, download-progress, download-resume, onboarding,
# diagnostics, editor, settings, settings-tiers, export-sheet, export-written.
# The renderer is
# `suramak_ui::deck::snapshot` (egui's own tessellation + a CPU rasteriser), so
# the output is deterministic and needs no GPU; the command still runs under
# Xvfb because CI's UI harness family does.
#
#   tests/ui-compact/run.sh
#
# The test also asserts the layout rules: 48 dp targets, the bottom bar clear of
# the gesture inset, no screen outside the viewport, Reduce Motion's
# crossfade-only transition, the a11y label/role floor on every control, and
# that a settled screen with a live job requests no frames. The shots are the
# review artifact, not a pixel golden: CI compares the asserted geometry and a
# human reviews the PNGs.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

mkdir -p tests/ui-compact/shots
xvfb-run -a --server-args="-screen 0 1400x2800x24" \
  cargo test -p suramak-ui --test ui_compact -- --nocapture

# The Rust writer emits uncompressed PNGs (no image crate in the workspace).
# Compress the committed artifacts when ImageMagick is present; the assertions
# below are unaffected.
SHOTS="launch-library library library-light library-empty first-run-download download-progress download-resume onboarding diagnostics editor settings settings-tiers settings-bottom-inset export-sheet export-written"
if command -v convert >/dev/null 2>&1; then
  for shot in $SHOTS; do
    path="tests/ui-compact/shots/$shot.png"
    convert "$path" -strip -define png:compression-level=9 "$path.tmp" && mv "$path.tmp" "$path"
  done
fi

for shot in $SHOTS; do
  path="tests/ui-compact/shots/$shot.png"
  [ -f "$path" ] || { echo "missing shot: $path" >&2; exit 1; }
  file "$path" | grep -q "1216 x 2640" \
    || { echo "wrong size: $path ($(file -b "$path"))" >&2; exit 1; }
  echo "shot ok: $path"
done
