# Versioning & releases

One source of truth: **`Cargo.toml` → `[workspace.package] version`** (semver). Everything else derives from it.

- Crates use `version.workspace = true` (already wired).
- `packaging/Info.plist` marketing keys are synced by `scripts/bump_version.py` and MUST match the Cargo version on `main` at release time.
- The macOS **build number** (`CFBundleVersion`) is stamped at bundle time by `scripts/make-app.sh` as the git commit count — monotonic, zero manual bookkeeping.
- The CLI reports its version (`suramak-cli --version`).

## Scheme (pre-1.0)

- `0.Y.0` — one minor per shipped milestone that changes user-facing capability: MVP → **0.1.0**, video files → 0.2.0, capture 2.0 (multi-source + per-app) → 0.3.0, …
- `0.Y.Z` — fixes on top of a milestone (no new capability).
- `1.0.0` — when macOS + Windows are both released and smoke-stable (or by explicit decision).
- Pre-release tags (`v0.2.0-beta.1`) are allowed when a build needs wider testing.

## Cutting a release

1. Milestone acceptance verified in CI; for app releases the owner's Mac smoke has passed.
2. `python3 scripts/bump_version.py X.Y.Z` — updates `Cargo.toml`, `packaging/Info.plist`, and rotates the CHANGELOG (`[Unreleased]` → `[X.Y.Z] — date`).
3. Commit `release: X.Y.Z`; then annotated tag `vX.Y.Z` and push both.
4. The **Release workflow** (`.github/workflows/release.yml`, tag-triggered) builds the macOS app bundle **and the Linux app tarball** (app + CLI + `.desktop` + icon + LICENSE), and attaches both to the GitHub Release. The release body comes from the CHANGELOG section; the unsigned-build note is appended automatically.
5. Artifacts: `Suramak-vX.Y.Z-macos-arm64.zip` **+ `Suramak-vX.Y.Z-macos-arm64.dmg`** (plain layout with an `/Applications` drag target), `suramak-vX.Y.Z-linux-x86_64.tar.gz`. Windows joins when the port lands.

## Distribution caveats

- Builds are **ad-hoc signed** (no Developer ID yet): the owner's machine runs them directly; other people's Macs need right-click → Open (Gatekeeper quirk of unsigned apps). Proper signing + notarization is a later decision (requires an Apple Developer account, $99/yr).
- Models are never bundled — first run downloads them on demand (sha256-pinned), so release artifacts stay small.
