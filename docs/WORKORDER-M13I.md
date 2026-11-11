# WORKORDER — M13I: the Android emulator CI smoke — the release APK's launch proven without a device

Context: the owner reported the v0.4.1/0.4.2 release APK "utterly broken" with
no symptom detail, and the compact Library launch surface (M13G1–G3) has never
run on real Android — only in Xvfb screenshots. GitHub's ubuntu runners
provide **KVM** (`/dev/kvm` exists), so an x86_64 emulator CAN boot there. This
workorder builds an **x86_64** variant of the app, boots an emulator in CI,
installs the APK, launches the app, and proves the compact Library renders.

## Deliverables

1. **The x86_64 variant**: `scripts/android/build-apk.sh` (or the CI job
   directly) builds the app for `x86_64` (the build.sh lineage already builds
   x86_64 debug for the emulator ABI) — a debug-signed APK, NOT shipped to
   releases (CI-only artifact, `if-no-files-found: ignore`).
2. **The emulator smoke job** in `ci.yml` (runs on the android-build lineage
   or standalone):
   - enable KVM, `sdkmanager emulator` + `system-images;android-35;google_apis;x86_64`
     (cache the image), create+launch the AVD headless
   - `adb wait-for-device` → install the x86_64 APK → `am start` the launcher
     activity → wait for the first frame
   - assert the launch: logcat carries the app's startup marker AND
     `uiautomator dump` (or a screencap inspected as a job artifact) shows the
     compact Library surface — a black/blank screen or a crash dialog FAILS
   - a 10-second monkey/stability pass (`adb shell monkey` bounded) as a
     crash canary
   - the emulator job is `continue-on-error: false` — a launch failure is a
     red build, since that is exactly what the owner hit on hardware
3. **The report** names what this proves (the release APK's launch surface on
   a real Android runtime — the x86_64 emulator, not the owner's arm64) and
   what stays owner-only (arm64 Metal/Vulkan specifics, the Vivo OEM layer).

## Verification

- The job green on a push to main (the emulator boots, the APK installs, the
  Library renders, the screencap artifact uploaded and inspected in the
  report).
- The arm64 release artifacts unaffected; the emulator job does not slow the
  release path (CI-only; runs on main pushes + tags, cached image).
- fmt/clippy -D warnings; workspace tests green.
- Report: `docs/reports/M13i-emulator-smoke.md` (the screencap inspected, the
  launch assertions, the x86_64/arm64 boundary).

## Do NOT

- No production code changes unless the smoke exposes a real launch bug (then
  fix it as part of this workorder — that is the point: the owner's
  "utterly broken" gets diagnosed without a device); no release-path changes;
  no signing changes; no new dependencies beyond the SDK/emulator tooling.
- No git commit/push; no sudo beyond the CI's own.
