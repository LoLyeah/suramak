# WORKORDER — M13B3: spike v2.1 — the texture-rebuild crash fix + crash-log reachability

Tree: current `main`. **Input — the v2 field report is a SUCCESS for diagnostics:** the owner's Vivo X300 crash named itself on screen:

```
crash #4: thread <unnamed>, egui-wgpu-0.33.3/src/renderer.rs:657:18
panic: Tried to update a texture that has not been allocated yet.
[15.79s] surface configured: Rgba8UnormSrgb 1216x2640
[15.79s] gpu ready (resume, attempt 3, 77 ms): Mali-G1-Ultra MC12 (Vulkan)
[15.79s] ERR PANIC (first frame after a successful resume-rebuild)
```

**Also open (owner question): "where's the crash log?"** — it is written to the app's *internal* files dir (`/data/data/app.suramak.spike/files/`), which scoped storage hides from every file manager; the on-screen banner mirrors the file byte-for-byte (so screenshots carry the data — proven). Reachability is what's missing.

## Goal

1. Kill the resume panic. 2. Make the crash logs getable from the phone without a PC. 3. Keep the four checks intact.

## Part 1 — root cause + fix (the texture panic)

Read `crates/suramak-android-spike/src/app.rs` first. The mechanism:

- `egui_ctx` lives on the `Handler` (line ~169, built once at `run()`), so it **outlives** suspend.
- On suspend: `self.gpu = None` drops surface/device/queue/**renderer**/`egui_winit::State`.
- On resume: `Gpu::new(window, self.egui_ctx.clone())` builds a **brand-new `egui_wgpu::Renderer` with an empty texture registry**, while the long-lived `egui::Context` still owns its texture-registry state — glyph-atlas and any painted textures have ids the context believes exist. egui then emits a texture **update** delta (not a create) for an id the new renderer has never allocated → egui_wgpu panics at `renderer.rs:657`. The `attempt 3` in the log is the GPU-retry loop succeeding on the third try, and the first frame after any successful rebuild panics the same way. (Sibling of the batch-3 texture-free-on-error fix, SUR2-NEW-07.)

**Fix (structural, no API chase):** on suspend, drop the `egui::Context` **together with** the renderer (it belongs to the GPU lifecycle). On resume, build a fresh context + fresh `egui_winit::State` + fresh renderer: every texture id re-registers through real creates. The spike's state (`SpikeState`) must stay app-owned (verify nothing holds a `Context` outside `app.rs`/`ui.rs` params; `ui::draw` takes `&Context` — fine). Update the module docs where "the context is kept" is claimed (workorder M13B2's summary said keeping the Window is safe — the CONTEXT is not). If a partial rebuild is cheaper (e.g. dropping only the renderer requires teaching egui to re-send full texture state — do NOT chase that API), prefer the full context rebuild; report the choice.

Also re-run the ~`draw`-path unit tests (the 19 spike tests include headless full-screen draws — they keep proving the UI after the lifecycle change).

## Part 2 — crash-log reachability

In `CrashFiles.kt` (it already centralizes the paths):

1. **Redirect the crash files to the app's external files dir** (`context.getExternalFilesDir(null)`) — browseable at `/sdcard/Android/data/app.suramak.spike/files/` by the phone's own Files app (Vivo's system one can open its own app's Android/data). Migrate: on startup, if the internal copies exist, move them.
2. **Add a "Share log" button** on the crash banner itself (next to Close/HIDE): `FileProvider` + `ACTION_SEND` (the share sheet) so the owner can hand the log straight to a chat without any PC. The button must work even when the Rust surface is dead (it is Kotlin-side, and the file lives next to the overlay's reader).
3. Keep the on-screen banner rendering (it already worked — no behaviour change beyond the extra button).

## Do NOT

- No new dependencies (FileProvider needs no artifact — it ships with androidx core; add it to the manifest as a provider with the existing package only if genuinely needed, else grantUriPermission route), no changes outside `crates/suramak-android-spike/` + `android/`, no CI edits, no Gradle/AGP churn. The four checks stay as they are (test screens unchanged except the Share button).
- No `git commit`/`push`; no sudo.

## Verification

- `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` (561 baseline); `cargo clippy -p suramak-android-spike --all-targets -- -D warnings` (the gate that sees the spike's cfg(android) code).
- `bash scripts/android/build-apk.sh` green with assertions; versionName **0.3.0-m13b.2.1**, versionCode 3.
- Kotlin: `./gradlew` assembleDebug compiles the new Kotlin (lint pass must not block: check `app/build.gradle.kts` warnings setup and match the repo's current treatment).
- **Honest boundary (report it):** the resume-panic fix is structural reasoning + unit tests ONLY — this exact panic can only happen on a real device's suspend/resume. The report must say so plainly in `docs/reports/M13b3-spike-v2.1.md`, with the exact reproduction steps for the owner (launch → background → foreground → expect the first frame to draw without the texture panic; run the four checks after).
