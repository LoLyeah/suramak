# WORKORDER — M13A: the engine on the NDK (Android toolchain bring-up, CI-only)

Tree: current `main` (post-`819e754`; no M13 work has run yet). **Read first, in order:** `docs/PLAN-M13-ANDROID.md` (the whole file — especially §3 M13a, §4 build layout, §6 CI, and the 2026-10-19 addendum), `docs/PLAN-M15-M18.md` §2 ("Import parity and shared decode reuse" row), and the existing workspace layout (`crates/suramak-core`, `crates/suramak-asr`, `.github/workflows/ci.yml`, `scripts/`).

## Goal

Prove the Rust engine stack cross-compiles for Android (`aarch64-linux-android` + `x86_64-linux-android`) — `suramak-core`, `suramak-asr` (whisper.cpp, bundled libopus, symphonia), and a minimal new `suramak-android` cdylib — and wire a CI job (`android-build`) so every push keeps proving it. **No UI, no JNI surface beyond one smoke entry point; no runtime execution is expected on this VPS** (the VPS cannot run Android binaries — runtime verification happens later: CI emulator + the owner's device at M13b).

## Environment facts (this VPS, verified 2026-10-19)

- No Android toolchain installed yet: install it yourself, entirely in your user home, **no sudo** — the NDK (current stable r27.x zip from Google's official dl.google.com URL into `~/android-ndk/`; record the exact version + sha256), `cargo install cargo-ndk`, `rustup target add aarch64-linux-android x86_64-linux-android`.
- Disk: ~18 GB free — the NDK (~2–3 GB) + artifacts fit, but **stop and report if free space drops below 5 GB**.
- 2 CPU cores: first cross-builds of whisper.cpp/libopus take tens of minutes per ABI. Batch your compile calls; generous timeouts; do not fight a slow build with retries.
- Leave existing targets (`aarch64-apple-darwin`, `x86_64-pc-windows-msvc`) untouched.
- CI: pin the SAME exact NDK version you used locally (e.g. `nttld/setup-ndk` or direct download).

## Scope

1. **Toolchain pin + docs**: exact NDK version (+sha256) and `cargo-ndk` version recorded in a new, brief `docs/android.md`, referenced from the workflow.
2. **Cross-compile the engine, both ABIs**: `cargo ndk -t arm64-v8a -t x86_64 build -p suramak-core -p suramak-asr` must succeed (release profile for the arm64 gate; debug acceptable for x86_64 if time forces — say which in the report). Fix ONLY what stands in the way: cfg gating, build-script env (`CC_*`/`AR_*`, CMake toolchain for whisper-rs-sys, `c++_static`), target-specific deps. **No behavioral changes.**
3. **`crates/suramak-android`** (new, minimal): a cdylib linking `suramak-core` + `suramak-asr`, exposing exactly one smoke symbol — e.g. `#[no_mangle] pub extern "C" fn suramak_android_version() -> *const c_char` returning the crate version — plus a host-runnable unit test of its logic. This crate is the future home of the app shell (M13b+); keep it nearly empty, but make it BUILD for Android.
4. **`scripts/android/build.sh`**: one wrapper (installs nothing; discovers the NDK path; runs the cargo-ndk build for both ABIs). CI and humans call the same script.
5. **CI job** `android-build` in `.github/workflows/ci.yml`: ubuntu runner, NDK setup, runs `scripts/android/build.sh`, caches cargo registry + NDK (keyed on the pinned version), timeout ~45 min. Must not alter any existing job.
6. **`cfg(target_os = "android")` stubs in `suramak-core`** for path/logging seams ONLY — additive, behind cfg; engine crates stay behaviorally identical.
7. **Report** `docs/reports/M13a.md`: item-by-item — what was installed (versions + hashes), exact commands, timings, deviations with whys, and the honest boundary ("runtime not verified here — next gates: CI emulator on push, owner device at M13b").

## Do NOT

- No JNI/Kotlin/Gradle work (M13b+). No UI. No changes to whisper behavior, routing invariants, capture, playback, exports, or any existing test expectation. No new dependencies outside what the cross-build requires (document each). No `git commit`/`push`; no sudo; touch nothing outside `/home/ubuntu/suramak` except toolchain installs in your user home.

## Verification (all gates; evidence in the report)

- `cargo fmt --check`, `cargo clippy --workspace --all-targets` (default features), `cargo test --workspace` — all green (host).
- `scripts/android/build.sh` succeeds for both ABIs; paste the tail with produced `.so` paths + `file`/`readelf` evidence they are AArch64/x86-64 Android objects; record `.so` sizes.
- `.github/workflows/ci.yml` still parses (`python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"`); the real proof is the first push — say so honestly.
- Desktop unchanged: `cargo build --workspace` still works; `cargo tree -p suramak-asr` shows no android-only deps in default features.

## After this

M13b (shell spike + Go/No-Go) once the owner has an Android device day. M14a is queued separately and must NOT run concurrently with this workorder (shared files: `ci.yml`, `suramak-core`).
