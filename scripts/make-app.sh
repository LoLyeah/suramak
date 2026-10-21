#!/usr/bin/env bash
# Builds dist/Suramak.app from the release binaries and ad-hoc codesigns it (PLAN §7).
# macOS only. Proper Developer ID signing is post-MVP (PLAN §8.6).
#
# M12: SURAMAK_FEATURES (comma list, default empty) selects the cargo features
# for the suramak-ui build. When the list contains `updates`, the pinned
# Sparkle distribution is fetched through scripts/fetch-sparkle.sh (never a
# bare download), the build links Sparkle.framework (SPARKLE_FRAMEWORK_PATH,
# which the crate's build script requires) and the framework is embedded into
# Contents/Frameworks before signing — nested code is signed inside-out (see
# the signing block below; `--deep` is no longer used). The
# @executable_path/../Frameworks rpath is a link-time argument
# (crates/suramak-ui/build.rs), so nothing here patches the binary.
set -euo pipefail
cd "$(dirname "$0")/.."

APP="dist/Suramak.app"
BIN="target/release/suramak"
FEATURES="${SURAMAK_FEATURES:-}"

# The `updates` feature needs the framework on disk at build time.
SPARKLE_DIR=""
if [[ ",${FEATURES}," == *",updates,"* ]]; then
  # Absolutize even if the fetch script's output ever becomes relative again:
  # the value feeds a build script whose CWD is the crate's source dir.
  SPARKLE_DIR="$(cd "$(scripts/fetch-sparkle.sh)" && pwd)"
  export SPARKLE_FRAMEWORK_PATH="$SPARKLE_DIR"
fi

if [[ -n "$FEATURES" ]]; then
  cargo build --release -p suramak-ui --features "$FEATURES"
else
  cargo build --release -p suramak-ui
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/suramak"
cp "packaging/Info.plist" "$APP/Contents/Info.plist"

# Stamp the monotonic build number (git commit count) onto the bundle. The
# marketing version comes from the repo plist, kept in sync by scripts/bump_version.py.
BUILD="$(git rev-list --count HEAD 2>/dev/null || echo 0)"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD" "$APP/Contents/Info.plist"

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

# M12: embed the pinned Sparkle framework (the `updates` feature). `ditto`
# preserves the framework's symlinks — following them would break its code
# signature. Contents/Frameworks + the build-time @executable_path/../Frameworks
# rpath are the bundling contract from the crate's README / Sparkle's guide.
if [[ -n "$SPARKLE_DIR" ]]; then
  mkdir -p "$APP/Contents/Frameworks"
  ditto "$SPARKLE_DIR/Sparkle.framework" "$APP/Contents/Frameworks/Sparkle.framework"
fi

# Ad-hoc signature: stable per binary; the user may re-see the mic prompt after a
# rebuild.
#
# Inside-out, **without** `--deep` (SUR-LOW-09): `--deep` is deprecated and does
# not reliably sign nested code, so every nested bundle is signed here first —
# deepest first (`find -depth`), because a helper's signature is sealed into its
# enclosing bundle. The two XPC services and the Autoupdate/Updater.app helpers
# live inside Sparkle.framework, so they must be signed before the framework
# itself, which must be signed before the app that seals it.
if [[ -d "$APP/Contents/Frameworks" ]]; then
  while IFS= read -r -d '' nested; do
    codesign --force --sign - "$nested"
  done < <(find "$APP/Contents/Frameworks" -depth \
    \( -name '*.xpc' -o -name '*.app' -o -name '*.dylib' -o -name '*.framework' \) -print0)
fi
codesign --force --sign - "$APP"
codesign --verify --verbose=2 "$APP"

echo "built $APP"
