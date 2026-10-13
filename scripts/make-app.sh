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
cp "packaging/Info.plist" "$APP/Contents/Info.plist"

# App icon: build AppIcon.icns from the generated iconset (iconutil, macOS) and bundle it.
ICONSET="assets/logo/icons/macos/AppIcon.iconset"
if [[ -d "$ICONSET" ]] && command -v iconutil >/dev/null; then
  iconutil -c icns "$ICONSET" -o assets/logo/icons/macos/AppIcon.icns
fi
if [[ -f assets/logo/icons/macos/AppIcon.icns ]]; then
  cp assets/logo/icons/macos/AppIcon.icns "$APP/Contents/Resources/"
else
  echo "warning: AppIcon.icns missing (run scripts/build_brand.py; iconutil needed on macOS)" >&2
fi

# Include the CLI when it has been built (scripts/mac-smoke.sh runs it from the bundle).
if [[ -f target/release/suramak-cli ]]; then
  cp target/release/suramak-cli "$APP/Contents/MacOS/suramak-cli"
fi

# Ad-hoc signature: stable per binary; the user may re-see the mic prompt after a rebuild.
codesign --force --deep --sign - "$APP"
codesign --verify --verbose=2 "$APP"

echo "built $APP"
