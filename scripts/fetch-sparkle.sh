#!/usr/bin/env bash
# M12: fetches + verifies the pinned Sparkle 2.9.6 distribution — the
# framework (Sparkle.framework) and the tools (bin/generate_appcast,
# bin/generate_keys, bin/sign_update) the release pipeline uses.
#
# The pin is the official release archive:
#   https://github.com/sparkle-project/Sparkle/releases/download/2.9.6/Sparkle-2.9.6.tar.xz
# sha256 measured on the release VPS (2026-10-16; 15,554,568 bytes, matching
# the GitHub release asset's size):
#   52bf9e88cdd972fc0c81501377a880e90d47031bd8ca5462488f843e2609e192
# Never change the version, URL or hash without re-measuring all three against
# the real release.
#
# Prints one line on stdout — the directory containing Sparkle.framework, i.e.
# SPARKLE_FRAMEWORK_PATH for the build — so callers capture it:
#   SPARKLE_DIR="$(scripts/fetch-sparkle.sh)"
# All diagnostics go to stderr. Idempotent and quiet on repeat runs: a marker
# file keyed to the verified sha256 short-circuits the whole fetch.
# Runs on macOS and Linux (the archive is extracted, never executed).
set -euo pipefail
cd "$(dirname "$0")/.."

SPARKLE_VERSION="2.9.6"
SPARKLE_URL="https://github.com/sparkle-project/Sparkle/releases/download/${SPARKLE_VERSION}/Sparkle-${SPARKLE_VERSION}.tar.xz"
SPARKLE_SHA256="52bf9e88cdd972fc0c81501377a880e90d47031bd8ca5462488f843e2609e192"

DEST="build/sparkle/${SPARKLE_VERSION}"
ARCHIVE="${DEST}/Sparkle-${SPARKLE_VERSION}.tar.xz"
MARKER="${DEST}/.verified-${SPARKLE_SHA256}"

if [[ -f "$MARKER" && -d "${DEST}/Sparkle.framework" ]]; then
  echo "$DEST"
  exit 0
fi

mkdir -p "$DEST"

if [[ -f "$ARCHIVE" ]]; then
  echo "sparkle: using the cached ${ARCHIVE}" >&2
else
  echo "sparkle: downloading ${SPARKLE_URL}" >&2
  curl -fL --retry 3 --retry-delay 2 -o "$ARCHIVE" "$SPARKLE_URL" >&2
fi

# Verify BEFORE extracting: the archive is the only thing we trust.
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL="$(sha256sum "$ARCHIVE" | cut -d' ' -f1)"
else
  ACTUAL="$(shasum -a 256 "$ARCHIVE" | cut -d' ' -f1)"
fi
if [[ "$ACTUAL" != "$SPARKLE_SHA256" ]]; then
  echo "sparkle: SHA256 MISMATCH for ${ARCHIVE}" >&2
  echo "sparkle:   expected ${SPARKLE_SHA256}" >&2
  echo "sparkle:   actual   ${ACTUAL}" >&2
  echo "sparkle: refusing to extract an unverified archive — delete it and re-run" >&2
  exit 1
fi
echo "sparkle: sha256 verified (${SPARKLE_SHA256})" >&2

tar -xJf "$ARCHIVE" -C "$DEST"

if [[ ! -f "${DEST}/Sparkle.framework/Versions/B/Sparkle" || ! -x "${DEST}/bin/generate_appcast" ]]; then
  echo "sparkle: the archive did not contain the expected framework/tools — aborting" >&2
  exit 1
fi

touch "$MARKER"
echo "$DEST"
