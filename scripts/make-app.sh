#!/usr/bin/env bash
# Builds dist/Suramak.app from the release binaries and ad-hoc codesigns it (PLAN §7).
# macOS only. Proper Developer ID signing is post-MVP (PLAN §8.6).
set -euo pipefail
cd "$(dirname "$0")/.."

APP="dist/Suramak.app"
BIN="target/release/suramak"

cargo build --release -p suramak-ui

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/suramak"
cp packaging/Info.plist "$APP/Contents/Info.plist"

# Include the CLI when it has been built (scripts/mac-smoke.sh runs it from the bundle).
if [[ -f target/release/suramak-cli ]]; then
  cp target/release/suramak-cli "$APP/Contents/MacOS/suramak-cli"
fi

# Ad-hoc signature: stable per binary; the user may re-see the mic prompt after a rebuild.
codesign --force --deep --sign - "$APP"
codesign --verify --verbose=2 "$APP"

echo "built $APP"
