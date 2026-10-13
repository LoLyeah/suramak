# WORKORDER M1 — Liquid Glass shell (`suramak-glass` + `suramak-ui`)

You are a senior Rust engineer working **inside this repository**. Read first, in order:

1. `docs/PLAN.md` — §1 (glass integration call sequence — follow it closely) and §6 (M1 milestone), §9–10 for layout/deps.
2. `docs/PLAN-NOTES.md` — verified corrections (**NOTES beat PLAN**; NOTE §2: objc2-app-kit 0.3.2 has typed `NSGlassEffectView` wrappers behind features `NSGlassEffectView` / `NSGlassEffectContainerView`).
3. `docs/reports/M0b.md` — current repo state.
4. `PRODUCT.md` + `.impeccable/surface-brief-body.md` — product truth + the **direction contract** (system-zenith OS citizen; real materials; integrated chrome; system typography).
5. The approved north-star comp: `.impeccable/mocks/decision/C-system.png`; measured region boxes/palette in `.impeccable/build/spec.json` + `.impeccable/build/regions.json` (sidebar ≈23% width; main pane: serif title, wide waveform-scrubber strip, transcript column; toolbar lives in the window chrome, NOT a floating pill).

## Task (M1 ONLY)

1. **`crates/suramak-glass`** — per PLAN §1/§9: `lib.rs` (`apply(window, config)` + cfg dispatch + `GlassMode {Auto, Always, Never}`), `macos.rs` (the exact AppKit sequence of PLAN §1 steps 1–8: non-opaque NSWindow, transparent titlebar + full-size content, runtime `AnyClass::get("NSGlassEffectView")` presence check → typed wrapper if available, else `NSVisualEffectView`; contentView swap so the glass sits BEHIND the winit view), `windows.rs` (DWM `DWMWA_SYSTEMBACKDROP_TYPE` stub, compile-checked only), `stub.rs` (Linux no-op). Pure Rust; must pass cross-checks from Linux for both `aarch64-apple-darwin` and `x86_64-pc-windows-msvc`.
2. **`crates/suramak-ui`** — winit 0.30 `ApplicationHandler` + **manual** `egui-winit` + `egui-wgpu` wiring (NOT eframe — see PLAN §1 why); transparent surface (pick `PostMultiplied` if offered, else `PreMultiplied`; clear color `{0,0,0,0}`); call `suramak_glass::apply` behind `#[cfg(target_os="macos")]`; egui custom titlebar strip with drag support (`window.drag_window()`) and a ~78px left inset for the traffic lights; a **static comp-faithful placeholder view**: sidebar silhouette with 4–5 text-only recording rows, main pane with a system-face serif title, a placeholder waveform scrubber strip (static bars + time ticks), and a transcript column of 3 static lines. No functionality yet (that's M2) — this is the shell. `theme.rs`: on macOS try loading SF Pro / New York / SF Mono from `/System/Library/Fonts` if present (paths: `SFNS.ttf`, `NewYork.ttf`, `SFNSMono.ttf` — verify at runtime, fall back silently), otherwise egui defaults; do NOT add new font files. `SURAMAK_SMOKE_SECS=N` mode: run N seconds, write `build/smoke_ok.txt`, exit 0.
3. **`scripts/make-app.sh`** + **`packaging/Info.plist`** per PLAN §7 (`NSMicrophoneUsageDescription`, `LSMinimumSystemVersion 13.0`, `CFBundleIdentifier dev.suramak.app`, ad-hoc codesign).
4. **CI additions** in `.github/workflows/ci.yml` — linux job: `rustup target add aarch64-apple-darwin x86_64-pc-windows-msvc`; `cargo check -p suramak-glass --target aarch64-apple-darwin`; `cargo check -p suramak-glass --target x86_64-pc-windows-msvc`; apt `xvfb libgl1-mesa-dri mesa-vulkan-drivers`; `SURAMAK_SMOKE_SECS=3 xvfb-run -a cargo run -p suramak-ui`. macos job: `cargo build --release -p suramak-ui`; `./scripts/make-app.sh`; `SURAMAK_SMOKE_SECS=3 ./dist/Suramak.app/Contents/MacOS/suramak`; upload `dist/Suramak.app` artifact.

## Constraints

- Only files inside this repo; no `sudo`; no `git commit`/`git push`.
- Dependencies: exactly PLAN §10 for `suramak-ui`/`suramak-glass` (egui/egui-winit/egui-wgpu/pollster/winit/raw-window-handle/objc2 trio; rfd is M2). Consume wgpu via `egui_wgpu::wgpu` re-export — never a direct wgpu dep.
- Do not modify `suramak-core`/`suramak-asr`/`suramak-cli` except the workspace `members` line.
- The Linux preview renders an opaque dark window (glass is macOS-only); Linux is for layout development, macOS is the material truth.
- Be patient: egui+wgpu compile is heavy on 2 cores (batch cargo calls; use ≤600s foreground / background+notify for longer).

## Acceptance (run on THIS Linux box; paste exact outputs)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p suramak-glass --target aarch64-apple-darwin          # exit 0 from Linux
cargo check -p suramak-glass --target x86_64-pc-windows-msvc        # exit 0 from Linux
SURAMAK_SMOKE_SECS=3 xvfb-run -a cargo run -p suramak-ui; echo "smoke=$?"   # expect 0
test -f build/smoke_ok.txt && echo SMOKE-OK
```

If wgpu finds no adapter under Xvfb: try explicit backend chains (Vulkan, then GL) and print why; if it still fails, STOP and report the blocker honestly — never fake the smoke file.
Optional bonus: `scripts/xvfb-shot.sh` that runs the app under Xvfb and captures `build/smoke.png` via ImageMagick `import -window root` when available.

## Report

`docs/reports/M1.md`: what was implemented, deviations with reasons, full acceptance outputs, blockers. Then stop.
