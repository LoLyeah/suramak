#!/usr/bin/env bash
# M12: builds the Sparkle appcast for one release — EdDSA-signed, release
# notes taken from the CHANGELOG section, over a staging dir that also collects
# previous releases' archives so `generate_appcast` can emit delta updates.
# macOS only (Sparkle's generate_appcast is a macOS tool; the release workflow
# runs it in the macOS job).
#
# Usage: scripts/make-appcast.sh <tag> <archive>
#   <tag>      the release tag as pushed (v0.2.1)
#   <archive>  the packaged app zip (Suramak-<tag>-macos-arm64.zip)
#
# Environment:
#   SPARKLE_ED_KEY    the base64 EdDSA private key (the repo secret). Absent →
#                     a loud workflow warning and exit 0: the release still
#                     ships, without a signed feed (the shipped placeholder
#                     SUPublicEDKey matches nothing). See docs/reports/M12.md.
#   SPARKLE_DIST_DIR  the fetched Sparkle distribution (default: the pinned
#                     output of scripts/fetch-sparkle.sh).
#
# Outputs (into build/appcast/): appcast.xml and any *.delta files — the
# release job attaches them. Idempotent: the staging dir is rebuilt from
# scratch on every run, and the EdDSA signature is deterministic.
set -euo pipefail
cd "$(dirname "$0")/.."

TAG="${1:?usage: scripts/make-appcast.sh <tag> <archive>}"
ARCHIVE="${2:?usage: scripts/make-appcast.sh <tag> <archive>}"
REPO_URL="https://github.com/LoLyeah/suramak"
PLACEHOLDER_KEY="U1VSQU1BSy1QTEFDRUhPTERFUi1FRDI1NTE5LUtFWSE="

if [[ -z "${SPARKLE_ED_KEY:-}" ]]; then
  echo "::warning::SPARKLE_ED_KEY is not set — skipping the appcast. The release ships without a signed feed and the updater stays dormant (the shipped SUPublicEDKey placeholder matches nothing). One-time setup: docs/reports/M12.md, \"Owner key setup\"."
  exit 0
fi

if [[ ! -f "$ARCHIVE" ]]; then
  echo "make-appcast: archive not found: $ARCHIVE" >&2
  exit 1
fi

# Guard the secret's shape before handing it to generate_appcast: the value is
# the base64 key string `generate_keys -x` writes — 44 chars (the current
# 32-byte seed) or 128 chars (the older 96-byte format). The common mistake is
# pasting base64 *of the exported file* (double encoding); catch it here with
# a message that says so instead of failing deep inside the tool.
if [[ ! "$SPARKLE_ED_KEY" =~ ^[A-Za-z0-9+/]+=*$ ]]; then
  echo "make-appcast: SPARKLE_ED_KEY is not a base64 string." >&2
  echo "make-appcast: the secret must be the *content* of the file written by 'generate_keys -x'." >&2
  exit 1
fi
case "${#SPARKLE_ED_KEY}" in
  44 | 128) ;;
  *)
    echo "make-appcast: SPARKLE_ED_KEY is ${#SPARKLE_ED_KEY} characters — expected 44 (the current 32-byte seed format) or 128 (the older 96-byte format)." >&2
    echo "make-appcast: the secret must be the *content* of the file written by 'generate_keys -x' (a base64 string) — if you pasted base64 of that file, paste the file's content instead (docs/reports/M12.md, \"Owner key setup\")." >&2
    exit 1
    ;;
esac

DIST="${SPARKLE_DIST_DIR:-$(scripts/fetch-sparkle.sh)}"
GENERATE="${DIST}/bin/generate_appcast"
if [[ ! -x "$GENERATE" ]]; then
  echo "make-appcast: generate_appcast not found at $GENERATE (run scripts/fetch-sparkle.sh)" >&2
  exit 1
fi

# The owner hasn't replaced the placeholder public key yet: the appcast is
# still built and signed, but shipped apps cannot verify it — say so loudly.
if grep -q "$PLACEHOLDER_KEY" packaging/Info.plist; then
  echo "::warning::packaging/Info.plist still carries the placeholder SUPublicEDKey — the signed appcast will not verify in shipped apps until the owner runs the one-time key setup (docs/reports/M12.md)."
fi

STAGE="build/appcast"
rm -rf "$STAGE"
mkdir -p "$STAGE"

# The archive, under its release asset name.
cp "$ARCHIVE" "$STAGE/"

# Release notes: a .md with the same base name as the archive (generate_appcast
# picks up same-named .html/.md/.txt files — the documented behavior); the text
# is the CHANGELOG section for this version, the same extraction the release
# job uses for the release body.
BASE="$(basename "$ARCHIVE")"
BASE="${BASE%.*}"
awk -v ver="${TAG#v}" \
  'BEGIN{f=0} $0 ~ "^## \\[" ver "\\]" {f=1; next} /^## \[/ {if (f) exit} f {print}' \
  CHANGELOG.md > "${STAGE}/${BASE}.md"
if [[ ! -s "${STAGE}/${BASE}.md" ]]; then
  echo "See CHANGELOG.md." > "${STAGE}/${BASE}.md"
fi
echo "make-appcast: release notes → ${STAGE}/${BASE}.md ($(wc -l < "${STAGE}/${BASE}.md") lines)"

# Previous releases' archives give generate_appcast the material for delta
# updates (it diffs each older app bundle against the new one). Best effort —
# the first release has none, and a failure must never fail the release. On by
# default in CI (GITHUB_ACTIONS=true); opt in locally with
# SURAMAK_APPCAST_HISTORY=1 so a local run never reaches for the network.
if [[ "${GITHUB_ACTIONS:-}" == "true" || "${SURAMAK_APPCAST_HISTORY:-}" == "1" ]] \
  && command -v gh >/dev/null 2>&1; then
  while read -r previous; do
    [[ -z "$previous" || "$previous" == "$TAG" ]] && continue
    gh release download "$previous" --pattern 'Suramak-*-macos-arm64.zip' \
      --dir "$STAGE" --skip-existing >/dev/null 2>&1 || true
  done < <(gh release list --limit 30 --json tagName --jq '.[].tagName' 2>/dev/null || true)
fi
ARCHIVES=$(find "$STAGE" -maxdepth 1 -name 'Suramak-*.zip' | wc -l | tr -d ' ')
echo "make-appcast: staging ${ARCHIVES} archive(s) (older ones enable delta updates)"

# The documented CI signing path: the private key arrives base64 on stdin
# (`--ed-key-file -`); never on the command line, never in the repo. The
# download URL prefix is this tag's GitHub release assets.
printf '%s\n' "$SPARKLE_ED_KEY" | "$GENERATE" \
  --ed-key-file - \
  --download-url-prefix "${REPO_URL}/releases/download/${TAG}/" \
  --embed-release-notes \
  "$STAGE"

if [[ ! -f "${STAGE}/appcast.xml" ]]; then
  echo "make-appcast: generate_appcast did not write ${STAGE}/appcast.xml" >&2
  exit 1
fi

DELTAS=$(find "$STAGE" -maxdepth 1 -name '*.delta' | wc -l | tr -d ' ')
echo "make-appcast: wrote ${STAGE}/appcast.xml and ${DELTAS} delta file(s)"
ls -la "$STAGE"
