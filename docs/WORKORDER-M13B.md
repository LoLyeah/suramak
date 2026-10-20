# WORKORDER — M13B: Android shell spike + Go/No-Go harness

Tree: current `main`. **Read first:** `docs/PLAN-M13-ANDROID.md` §M13b (the four checks are the contract), §2.1 (option A = GameActivity + egui; option C = Compose overlays is the No-Go fallback), `docs/android.md` (toolchain pins), `docs/reports/M13a.md`, and `crates/suramak-android/src/lib.rs`.

## Goal

An **installable debug APK** that runs egui/wgpu inside a GameActivity on an arm64 Android phone and exercises the four things that can kill option A. The owner installs it and answers the checklist; nothing here needs to be pretty, it needs to be honest.

Environment already on this box: JDK 17 (`/usr/bin/java`), NDK r27d at `~/android-ndk/android-ndk-r27d`, cargo-ndk 4.1.2, rust targets `aarch64-linux-android` + `x86_64-linux-android`, ~13 GB free. **No Android SDK yet, no sudo** — install `commandlinetools` under `~/android-sdk` and accept licences with `yes | sdkmanager --licenses`. Keep the whole install + build under ~6 GB (use `sdkmanager --list_installed` to keep only what you need: platform-tools, the platform you target, build-tools). Do not delete anything outside the new SDK dir.

## Scope

1. **`android/` Gradle project** (new top-level dir):
   - Gradle wrapper (pin a version compatible with the AGP you pick — commit `gradle/wrapper/gradle-wrapper.properties` + jar), `settings.gradle.kts`, root `build.gradle.kts`, `app/build.gradle.kts`.
   - `app/src/main/AndroidManifest.xml`: one `GameActivity`-derived activity, `android:configChanges` that does NOT swallow everything (we need to see recreate behaviour), `minSdk 29`, `targetSdk` = the platform you installed, `android:screenOrientation="unspecified"`, no action bar, app label `Suramak Spike`, package `app.suramak.spike`.
   - `app/src/main/java/app/suramak/spike/MainActivity.kt`: extends `com.google.androidgamesdk.GameActivity` (androidx `games-activity`), loads the Rust cdylib, and implements the JNI surface for (a) insets, (b) the back gesture/button, (c) a "recreate" trigger. Keep Kotlin minimal — the UI must be egui.
   - **The AAR version must match what the `android-activity` crate expects** — read the crate's docs/source for the games-activity version it targets; state the pair in the report.
2. **Rust spike binary** — a NEW crate `crates/suramak-android-spike` (cdylib) so nothing in `suramak-ui` is touched: `android-activity` + `winit` (the same fork pin as the workspace) + `egui`/`egui-winit`/`egui-wgpu` + `wgpu`, drawing one screen with four tabs/sections:
   - **(1) Text input**: a multiline `TextEdit` — type Indonesian with autocorrect on, backspace across words, and a counter of characters received (proves IME events arrive).
   - **(2) Lifecycle**: a counter that increments per resume + a text field whose content must survive background→foreground (surface destroy/recreate). Show the resume count on screen.
   - **(3) Motion**: one 60 fps animation (a moving element with a frame-time readout; report the measured average fps on screen).
   - **(4) Insets + back**: draw the current inset values (top/bottom/left/right) from the Kotlin shim and a back-button counter that must NOT exit the app while on screen.
   A big "M13B GO/NO-GO" header, and each check's result visible on screen so the owner can screenshot it.
3. **Build script** `scripts/android/build-apk.sh`: `cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build --release -p suramak-android-spike` then `./gradlew assembleDebug`, and **assert** the APK exists + contains `lib/arm64-v8a/libsuramak_android_spike.so` (unzip -l) + print its size and sha256.
4. **CI**: add one job `android-spike` to `.github/workflows/ci.yml` (this workorder DOES allow editing ci.yml — but only to append this job; do not touch the existing four jobs) that installs the SDK + JDK 17 + NDK r27d (reuse the existing NDK cache key), runs the script, and uploads `android/app/build/outputs/apk/debug/app-debug.apk` as artifact `suramak-android-spike-debug`. Timeout 45 min.
5. **Doc** `docs/m13-gate.md`: the checklist the owner runs on the phone (what to type, what to expect, what counts as Go vs No-Go per check, and the No-Go fallback = option C for text fields only), plus the install command.
6. **Report** `docs/reports/M13b.md`: versions (AGP, Gradle, games-activity AAR, android-activity crate), what built, APK size + sha256, and the honest boundary (built here; **run only on the owner's device**).

## Do NOT

- No changes to existing crates' logic (new crate only; if a shared crate genuinely needs a cfg, document it).
- No signing keys, no release APK, no Play/F-Droid work, no share-target, no foreground service, no model management — those are M13c+.
- No `git commit`/`push`; no sudo; nothing outside `/home/ubuntu/suramak` (the SDK may live in `~/android-sdk`).

## Verification

- `bash scripts/android/build-apk.sh` exits 0 and prints the APK path, size and sha256.
- `unzip -l app-debug.apk` shows the `.so` under `lib/arm64-v8a/` and `AndroidManifest.xml`.
- `aapt dump badging` (from build-tools) shows the package, minSdk 29, and the launchable activity.
- Host gates unchanged: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (468 baseline + your new crate's tests if any).
- `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"` and the five job names present.

The single most likely failure is the games-activity/android-activity version pair — resolve it by reading the crate source, not by guessing.
