# WORKORDER — M13B2: Android spike v2 — self-diagnostics rig

Tree: current `main` (post-`c24cb90`). **Read first:** `docs/m13-gate.md` §8 (the owner's NO-GO verdict + context), `docs/reports/M13b.md` §10, `crates/suramak-android-spike/src/*`, `android/app/src/main/java/app/suramak/spike/MainActivity.kt`.

## Owner verdict (the input this workorder exists to answer)

Vivo X300, Android 16: check 3 (motion/60 fps) **GO**; checks 1/2/4 broken hard; relaunches **"open then immediately close"** until a force-stop. Interpretation: rendering works; the Kotlin↔Rust bridge and/or lifecycle path crashes the process. The spike is blind (no logcat access on the owner's side) — v2 must make the failure **visible on the phone screen**.

## Goal

Turn the spike into a self-diagnostics rig: it survives its own crashes, shows the last error on screen on the next launch, and logs every platform callback visibly. Nothing here changes the four checks' intent — it instruments them.

## Scope

1. **Rust panic hook**: a global "last crash" buffer (message + location + thread + timestamp). On panic: write it into the buffer, append to a crash log file in the app's files dir (obtain the files-dir path once from Kotlin over JNI at startup — one small addition to the bridge), then let the existing unwinding behavior proceed. No new dependencies.
2. **Crash-proof JNI exports**: every `Java_app_suramak_spike_*` entry (`nativeImeText`, `nativeSetInsets`, `nativeBackPressed`) wrapped in `catch_unwind` (AssertUnwindSafe is acceptable and must be justified in the report); a panic inside must NOT unwind across JNI into Kotlin (UB) — it records into the crash buffer and returns normally. Bad/empty strings and out-of-range insets are defensive-checked, never a panic.
3. **Persistence + display**: on startup, if a crash file exists, its content is shown (a) in a Kotlin-side overlay TextView (so it survives even if the Rust surface dies again) and (b) in egui as a red banner. The file is cleared only by the new **"Clear crash"** button. This directly answers "opens then immediately closes": the next launch will show WHY on screen.
4. **On-screen log ring**: an always-visible (collapsible) egui panel with the last ~40 timestamped lines: every JNI callback received (IME text + length, inset values, back presses), surface create/destroy/resume events, wgpu adapter/surface errors. The owner screenshots this — no logcat needed.
5. **Isolation + recovery**: tab state stays independent so a wedge in one check doesn't poison the others; add a **"Restart surface"** button (drop wgpu surface/objects, rebuild — the lifecycle path made user-triggerable so check 2 becomes repeatable on demand with visible pass/fail instead of requiring app switching).
6. **Lifecycle correctness**: verify v1 actually drops GPU objects on suspend and rebuilds on resume (the plan's rule). If v1 leaks the surface, fix it — this is a prime suspect for the wedge.
7. **Version marker**: `versionName 0.3.0-m13b.2`, the header reads **M13B v2 DIAGNOSTICS**.
8. **Build**: `bash scripts/android/build-apk.sh` must pass with its assertions; report the APK path, size, sha256.

## Do NOT

- No new dependencies (the spike's current set is sufficient). No changes to `suramak-asr`/`suramak-core`/other crates; no `.github/workflows/ci.yml` edits (the existing `android-spike` job builds whatever is in-tree); no Gradle/AGP version churn. No `git commit`/`push`; no sudo; only files inside `/home/ubuntu/suramak`.
- Do NOT attempt to "fix" the IME properly in this workorder (that is M13c's design decision); v2 is instrumentation.

## Verification

- Host gates: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (baseline 490 + the audio child's tests may be landing concurrently — do not treat a higher count as a failure; only FAILED matters; if Cargo.lock conflicts with the concurrent audio work, leave the lock consistent with a single `cargo` resolution run and note it).
- `bash scripts/android/build-apk.sh` green; `aapt dump badging` shows versionName `0.3.0-m13b.2`.
- Report `docs/reports/M13b2-spike-v2.md`: the instrumentation list, the lifecycle finding (was v1 leaking?), what each owner-visible element will show, the honest boundary (built + statically verified here; the phone is the only place it can run).

The single most valuable property: **whatever killed the process last time must be visible as text on the screen the next time it is opened.**
