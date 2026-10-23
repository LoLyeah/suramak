# WORKORDER — M13B4: spike v2.2 — surface lifecycle (the resume validation panic)

Tree: current `main` at `e6aebf5`. **v2.1 field evidence is COMPLETE SUCCESS on the
egui fix**: the owner's Vivo X300 / Android 16 pass on `0.3.0-m13b.2.1` shows ZERO
"texture not allocated" panics — the egui-Context-with-GPU drop + fresh registry +
first-frame full repaint works (v2 crash #2/#3's mechanism is gone).

**The one remaining crash is a NEW bug the egui fix exposed** — crash #1 of the
v2.1 session (2026-10-23 05:37:22Z, uptime 11.95s):

```
wgpu-27.0.1/backend/wgpu_core.rs:3793: Surface::configure:
  Surface does not support the adapter's queue family
```

Repro trace (their spike-crash.txt, verbatim):
- 0.18s gpu ready attempt 1 + fresh egui registry + first frame presented ✓ (the v2.1 path, working)
- 3.29s onPause → 3.32s surfaceDestroyed → 3.40s suspended — surface/device/queue/renderer/egui context dropped ✓ (all in one generation)
- **3.40s `gpu: retrying surface init (attempt 2 of 3)` — a raced frame started a NEW gpu init DURING the suspend path, right after the full drop** → 3.58s "gpu ready (retry, attempt 2, 183 ms)" + egui ctx rebuilt → 3.59s onStop — that retry built a surface against the window that was ALREADY destroyed (surfacedestroyed at 3.32s)
- 11.91s real resume (onStart/onResume/insets ✓) → 11.95s `surface outdated/lost — reconfiguring` → **configure panics** on the stale Surface (created from the pre-destroy window) — Mali VK WSI queue-family check rejects it. Contained; footer showed "rebuilds 4 T4" on the surviving session.
- The footer's "rebuilds 4" in 111s = this same stale-surface rebuild loop re-firing on subsequent resumes.

## Root cause (deterministic)

`Gpu::new(window …)` is handed the window handle **captured when the gpu was last
created**, but `wgpu::create_surface(window.clone())` (app.rs:592) binds the
`ANativeWindow` identity from that moment. After `surfaceDestroyed`, the same
handle is dead; on the next surfaceCreated the native window is a NEW object, and
configuring the OLD `wgpu::Surface` against it fails Mali's queue-family probe.
Two contract violations make it fire:
1. **reconfigure-vs-rebuild**: the `surface outdated/lost` path (app.rs:728-729)
   calls `self.surface.configure(...)` on the existing Surface instead of dropping
   and recreating it from the CURRENT window's handle.
2. **frames during suspend**: the gpu init/retry loop can start while suspended
   (the 3.40s raced init after the drop) — out of the lifecycle contract, and it
   creates the stale-generation surface that the next resume's reconfigure rejects.

## Fix

In `crates/suramak-android-spike/src/app.rs` (`Gpu`/union flow already app-owned):

1. **The wgpu Surface joins the gpu generation**: drop it on `onPause` together
   with device/queue/renderer/egui ctx (it already drops on the suspend path's
   "dropped" event — verify, and if the drop leaves the `Surface` behind in the
   `Gpu` struct while the flow tracks only gpu-scope objects, include it), and on
   every resume create a **fresh surface** from the window handle that is current
   at THAT moment (`Gpu::new` already takes the window — make sure the window the
   caller passes is the CURRENT `ANativeWindow` from the resumed surface, not a
   captured one; pass it through the same path that feeds `restart_surface`).
2. **No gpu work while suspended**: the frame loop must not run (and must not kick
   init/retry) when the surface is gone/paused — gate the entry points so the
   3.40s-style raced init is structurally impossible; if a frame arrives during
   `surfaceDestroyed..resumed`, it is a resume-in-waiting, not a new gpu. Add a
   startup-log line naming the gate when it trips.
3. **outdated/lost → rebuild, not reconfigure**: on
   `SurfaceError::Outdated/Lost/Timeout` (or the v2.1 reconfiguring path), drop
   the gpu generation entirely and rebuild exactly like a resume does (fresh
   surface from the current window + fresh egui ctx — reusing the SAME "resume"
   entry as the crash fix already exercises). Cap the automatic rebuilds per
   lifecycle event at 2 (the v2.1 bound) and log `rebuild (outdated) — fresh
   surface from the current window` when it fires.

## Do NOT

- No new dependencies; no changes outside `crates/suramak-android-spike/`
  (+ version string in `scripts/android/build-apk.sh` if it carries the version,
  and `android/app/build.gradle.kts` version bumps: versionCode 4,
  versionName `0.3.0-m13b.2.2`); no CI edits; no Gradle/AGP churn; the four
  checks stay as-is.
- No `git commit`/`push`; no sudo.

## Verification

- `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D
  warnings`; `cargo test --workspace` (565 baseline, +whatever the new tests
  add); `cargo clippy -p suramak-android-spike --all-targets -- -D warnings`.
- Unit tests that pin the new contract: (a) the suspend path drops ALL gpu-scope
  objects including the Surface (no leak across generations); (b) a frame arriving
  while suspended does NOT start a gpu init (the gate counts it instead); (c) the
  outdated→rebuild path produces a fresh surface + fresh egui ctx, and the 2-auto
  cap holds.
- `bash scripts/android/build-apk.sh` green with assertions; versionCode 4,
  versionName 0.3.0-m13b.2.2. Kotlin/Gradle compile warning-free.
- **Honest boundary (report it)**: the queue-family check is Mali/WSI behavior
  only verifiable on the device — the structure (drop + fresh-create + gate) is
  unit-test-proven, and the owner's next pass judges it. `docs/reports/
  M13b4-spike-v2.2.md`, with the exact repro steps: launch → Home → reopen →
  expect zero panics and the first frame presented; state "restarts 0 · rebuilds
  (surfaced) N" stays small during repeat Home round-trips.
