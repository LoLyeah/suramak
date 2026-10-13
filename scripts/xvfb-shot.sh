#!/usr/bin/env bash
# Runs the M1 shell under Xvfb and captures the window to build/smoke.png
# (ImageMagick `import -window root`). Linux only; needs xvfb + imagemagick.
set -euo pipefail
cd "$(dirname "$0")/.."

SECS="${SURAMAK_SMOKE_SECS:-6}"
mkdir -p build

cargo build -p suramak-ui
export SURAMAK_BIN="$PWD/target/debug/suramak"
export SURAMAK_SMOKE_SECS="$SECS"

xvfb-run -a --server-args="-screen 0 1400x900x24" bash -c '
  set -euo pipefail
  "$SURAMAK_BIN" &
  app_pid=$!
  sleep 3
  import -window root build/smoke.png
  wait "$app_pid"
'

echo "screenshot: build/smoke.png"
