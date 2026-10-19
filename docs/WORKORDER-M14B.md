# WORKORDER — M14B: Windows GUI shell basics

Tree: current `main` (post-`ac1f5e8`, M14a landed green). **Read first, in order:** `docs/PLAN-M14-WINDOWS.md` §3 M14b, D2 (single-instance design), §4 platform layers, §5 packaging; `docs/reports/M14a.md` (what the windows job already proves); `crates/suramak-ui/src/{main.rs,shell.rs,instance.rs}`, `packaging/Info.plist` (the macOS metadata analogue), `scripts/mac-smoke.sh` (the smoke pattern to mirror).

## Goal

The Deck UI **launches on Windows with an opaque surface, correct data paths, and a single instance** — the M14b slice. No DWM material yet (that is M14c), no notifications (M14f), no installer (M14g). This VPS cannot run Windows GUI code: the acceptance split is **CI compiles + runs the guarded unit tests** (the windows job already does both) and **the owner lane runs the GUI**.

## Scope

1. **Subsystem + metadata**: `#![cfg_attr(windows, windows_subsystem = "windows")]` on the GUI binary only (the CLI stays a console binary). A `build.rs` in the UI binary crate using `embed-resource` (or an equivalent hand-rolled `winres`-style writer) adding: the app icon, VERSIONINFO (product/file version from `CARGO_PKG_VERSION`), and an **application manifest** with PerMonitorV2 DPI awareness, `longPathAware`, `<activeCodePage>UTF-8</activeCodePage>`, Common Controls v6, and the supportedOS GUIDs (Win10/11). Keep the resource files under `packaging/windows/` (new dir) and committed.
2. **Single-instance guard (D2)**, in `instance.rs` behind `#[cfg(windows)]`: `CreateMutexW` on `Local\Suramak-SingleInstance-<user SID>`; if `ERROR_ALREADY_EXISTS`, signal a named auto-reset event `Local\Suramak-Activate-<SID>` and exit 0; the first instance waits on that event on a background thread and posts a winit user event that restores/raises/focuses the window (reuse the existing user-event channel pattern — `UserEvent` already carries `Repaint`; add an `Activate` variant). The exiting second process calls `AllowSetForegroundWindow(ASFW_ANY)` before signalling. **The unix `flock` path and its `GuardError` must stay byte-identical** — the guard error type is already `#[cfg(unix)]`; mirror the shape for Windows with its own error type if needed.
3. **Data paths**: on Windows resolve the data dir to `%LOCALAPPDATA%\Suramak\` (models under `%LOCALAPPDATA%\Suramak\models`), via the existing `suramak-core` path seam (the `cfg(windows)` branch there). Verify no path on Windows falls back to a unix-style home path.
4. **Shell behaviour**: Ctrl maps where macOS uses Cmd (audit the existing shortcuts; do not redesign bindings); "reveal in folder" via `explorer /select,` (new small helper, cfg(windows), used by whatever reveal action exists on macOS today — if none exists, add the helper + a menu entry behind cfg(windows) only, minimal).
5. **Tests that the CI can run**: `#[cfg(windows)]` unit tests for the guard's *pure* logic (mutex-name construction incl. SID formatting via a pure function; the exists→signal→exit path factored so the decision is testable without a second process). Do NOT attempt to spawn a real second instance in CI.
6. **Report** `docs/reports/M14b.md`: item-by-item, exact commands, local-vs-runner-vs-owner boundary, deviations.

## Do NOT

- No DWM/acrylic (M14c), no toasts (M14f), no installer/signing (M14g), no loopback/hotkeys (M16). No changes to engine, routing, decode, exports, library, polish. Do not touch `.github/workflows/ci.yml` — the existing `windows` job already covers compile+test; if a genuinely new CI step is unavoidable, justify it in the report and keep the diff minimal. No `git commit`/`push`; no sudo; nothing outside `/home/ubuntu/suramak`.

## Verification (all gates; evidence in the report)

- Host (Linux): `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` — all green, and **no change to Linux/macOS behaviour** (`git diff` shows windows-cfg additions only outside `build.rs`/`packaging/windows/`).
- Windows C-free checks locally: `cargo check -p suramak-glass -p suramak-taps --target x86_64-pc-windows-msvc` still green; `cargo tree` default-feature dep trees unchanged for Linux.
- `build.rs` compiles for the host but no-ops off-Windows (state how you proved that — e.g. `cargo build -p suramak-ui` on Linux shows no resource step).
- The windows CI job's `tests` step will run your new `#[cfg(windows)]` tests on the next push — say so honestly; do not claim you ran them here.

## After this

M14c (real DWM material + frame pacing) once the owner has launched the M14b shell on their PC and reported the first-launch checklist (PLAN §3 M14b's "Verified by"). M13b remains blocked on the owner's Android device.
