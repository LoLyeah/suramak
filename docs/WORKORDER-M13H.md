# WORKORDER — M13H: Android tagged release (the build the owner installs)

Plan source: `docs/PLAN-M13-ANDROID.md` §3 M13h. Tree by dispatch time:
post-M13G (compact UI). **Prerequisite gates before this workorder dispatches:
M13D ✓, M13E ✓, M13F ✓, M13G ✓ (owner-review not required — the release IS the
owner's test, per the owner's directive "I'll test it on release").**

## Deliverables

1. **Release signing**: generate an Android keystore ON THIS VPS (the owner
   does not create OTPs — document the command exactly as run), commit
   instructions into `docs/android.md`, and store the keystore + passwords
   where the release workflow can read them (repo secrets noted in the
   report; the keystore bytes go into secure storage on the owner's behalf —
   **never committed**; a sha256 of the keystore recorded in
   `docs/PLAN-NOTES.md` so continuity survives VPS loss only if the owner
   chooses; default: keep it here, note the risk).
2. **`release.yml` android job**: on tag push, build
   `suramak-<version>-android-arm64-v8a.apk` release-signed (minSdk 29,
   arm64-v8a only — the owner floor), with a `.sha256` file shipped beside it;
   the versionCode derives from semver (dot-rotation like the desktop's
   version sites; document).
3. **`docs/android.md`**: install (no adb), permissions explained (mic,
   notifications, background), the model download story (first-run Wi-Fi),
   and the honest OEM-killer note (Vivo/Xiaomi may background-kill; the FGS
   + persistent notification mitigates; restart-resume is built in).
4. **F-Droid metadata draft** (`fastlane/metadata/android/` — short
   description + full description per the upstream conventions; the
   chCommit chaining follows upstream).
5. **CHANGELOG entry** + a version bump `X.Y.Z` where the desktop release
   schema lands (versionCode consistency).
6. **Tag dry-run** on a pre-release tag: the workflow must produce the
   artifacts and the release page with the .apk; attached to a
   **pre-release GitHub release** the owner can install from the phone
   browser — that IS the owner test vehicle.

## Verification

- The tag's CI run: mac + linux + android-build + the NEW android release job
  green; the produced APK's aapt/apksigner assertions pass on the released
  bytes (not just CI logs); the install link in the notes.
- Owner test = the report's named pass: install, share a WhatsApp voice note,
  screen off, library row, notification tap; recording a clip; rotate ±2.
- fmt/clippy/bin-gates untouched; workspace tests green at merge time.
- Report: `docs/reports/M13h-android-release.md` (the install checklist +
  the signing story + the store/metadata status).

## Do NOT

- No new features in the release path; no model bundling (download-on-first
  use stays); no de-mode changes; CI workflow edits strictly in the name of
  M13h's job; no destructive change to the desktop release.
- No git commit/push except the final tag cut handled by the orchestrator if
  the pipeline's release cadence differs (the child stops at the tag-dry-run
  evidence).
