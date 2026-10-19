# WORKORDER — M14A: Windows bring-up and the CI windows job

Tree: current `main` (post-`cf5f7e6`, M13a landed — do not disturb anything it added). **Read first, in order:** `docs/PLAN-M14-WINDOWS.md` (the whole file — especially §3 M14a, D4's CI decisions, §4 platform layers checklist, and the 2026-10-19 addendum), then the live workspace (`Cargo.toml`, `.github/workflows/ci.yml`, `crates/suramak-core`, `crates/suramak-asr`, `crates/suramak-cli`).

## Goal

The workspace **compiles and tests for Windows (x86_64-pc-windows-msvc)** and gains a `windows` CI job that proves it on every push — the first slice of M14. Scope is build-system + cfg gating only: the GUI on Windows is later slices (M14b+). The real gate is the GitHub **windows runner** (a native MSVC build); this VPS verifies only what it can (pure-Rust target checks, host gates, workflow YAML).

## Environment facts (this VPS, verified 2026-10-19)

- `x86_64-pc-windows-msvc` is installed as a rustup target, **but this Linux box cannot compile MSVC C dependencies** (no MSVC toolchain; whisper-rs-sys/ring/coreaudio-sys will fail under `--target x86_64-pc-windows-msvc`). That is EXPECTED — do NOT fight it. Local windows-target work is limited to `cargo check --target x86_64-pc-windows-msvc` for crates/feature-combinations that are C-free (e.g. `-p suramak-core`), plus `cargo tree` feature audits. Everything C-backed (whisper, opus, upcoming sherpa) is proven on the CI runner instead.
- 2 CPU cores; do not attempt full cross builds of C deps via mingw/xwin — out of scope.
- The CI runner hosts LLVM + VS Build Tools + CMake (assume standard `windows-2022` image capabilities; screenshot the plan's note that the pinned image choice is validated by the first push).

## Scope

1. **cfg gating for Windows** (compile-correct formations, no behavior work): make `suramak-taps` (macOS-only), the `updates` feature (Sparkle, macOS-only), the macOS glass path, and any Unix-only code (e.g. the single-instance `flock`) cfg-correct so the workspace compiles on Windows with default features — follow the plan's seam rules: no `#[cfg(windows)]` inside the engine/routing/model manager/exports; platform facts travel through `suramak-core` (paths, capabilities).
2. **Target-specific deps** in the workspace `Cargo.toml` where needed (e.g. windows-only crates behind `[target.'cfg(windows)'.dependencies]`), keeping default-feature dep trees for macOS/Linux byte-identical (`cargo tree` diff evidence).
3. **`.cargo/config.toml`** (new): `RUSTFLAGS=-C target-feature=+crt-static` guidance for the Windows target only, per D4. Keep it target-scoped; do not touch Linux/macOS flags.
4. **whisper build flags for a portable baseline**: ensure whisper.cpp's cmake is invoked with native tuning OFF and an explicit x86-64 feature level (AVX2 baseline), matching D4. If this needs build-script env, wire it minimally and document.
5. **CI job `windows`** in `.github/workflows/ci.yml` (append; existing jobs untouched): runs-on the pinned image (`windows-2022` unless you document a reason for 2025), timeout ~60 min; steps: checkout, Rust cache, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p suramak-core -p suramak-asr -p suramak-cli -p suramak-playback` (cfg-excluded crates skip; ensure that is by cfg, not test filters), `cargo check -p suramak-asr --features parakeet` (compile-check only), a `suramak-cli doctor` run, a `suramak-cli transcribe` run on the repo's committed fixture with the tiny model (cache the tiny model keyed on its sha256 pin), the **dumpbin /dependents VCRUNTIME/MSVCP check** (fail if the VC++ runtime DLLs appear), and an artifact upload of `suramak-cli.exe`. Keep every step's failure mode honest (no `continue-on-error`).
6. **Report** `docs/reports/M14a.md`: item-by-item, exact commands, what was verified locally vs what is delegated to the runner push, deviations with whys, the honest boundary ("real proof = the first push's windows job").

## Do NOT

- No GUI work (M14b+), no installer, no signing, no updater, no system-audio — all later slices. No behavioral changes to shared crates; no dependency upgrade sweep (only what Windows compilation requires, each documented); no `git commit`/`push`; no sudo; nothing outside `/home/ubuntu/suramak`.

## Verification (all gates; evidence in the report)

- Host (Linux): `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` — all green, unchanged from `cf5f7e6` behavior.
- `cargo check --target x86_64-pc-windows-msvc -p suramak-core` succeeds, and any other C-free windows-target checks you can run — paste exact commands + results.
- `cargo tree` diffs show default-feature dep trees for Linux unchanged; `.github/workflows/ci.yml` parses (PyYAML) and existing jobs are byte-identical (diff shows additions only).
- Report `docs/reports/M14a.md` with the deferred-to-CI boundary stated plainly.

## After this

M14b (GUI shell basics) once M14a's windows CI job is proven by a push. The M13 line continues separately (M13b awaits the owner's device day). Do NOT start M13b work here.
