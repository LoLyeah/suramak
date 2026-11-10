# WORKORDER — M16F: sherpa release packaging gate (unblocks Parakeet-in-releases + diarization)

Plan source: `docs/PLAN-M15-M18.md` §4 M16.6's first piece + the owner's
decision (§6 item 3, 2026-10-19): sherpa release packaging is a standalone
workorder owned by Hermes. Gate: **per-OS shared libraries bundled, signed,
correct rpath; sha256 pinned; clean-machine dynamic-load test (no dev paths);
a feature-on release build.** Tree: dispatches after the VAD slice-1 child
lands (shared Cargo.lock discipline — one child at a time).

## Deliverables (macOS first — the owner's test platform)

1. **The sherpa-onnx shared libraries ship inside the macOS bundle** for
   release builds with the `parakeet` feature ON: `suramak-taps`' or the
   asr crate's sherpa dylibs land in `Contents/Frameworks` with the
   `@executable_path/../Frameworks` rpath (the Sparkle pattern already
   proven in make-app.sh), ad-hoc signed like the rest of the nested code,
   and `scripts/mac-check-dylibs.sh` extended to verify the sherpa dylibs'
   rpath resolution in the same static gate that guards
   `libswift_Concurrency`.
2. **sha256 pinning**: every bundled dylib + the Parakeet model files are
   pinned; Model Manager verifies on load (the existing M10A pins carry
   over — extend, don't fork).
3. **Clean-machine dynamic-load test**: the CI macOS job runs the packaged
   bundle with the feature ON and loads Parakeet (a smoke that proves no dev
   path leaks into the bundle — e.g. `@rpath` resolution from the app, not
   `~/.cargo/...`).
4. **Feature-on release build**: `release.yml`'s macOS job builds the bundle
   with `parakeet` ON so the release artifact ships the fast engine (the
   setting stays opt-in OFF by default — the M10B decision unchanged).
5. **License surface**: sherpa-onnx + ONNX Runtime's notices join the
   licenses surface (M16DF's license chain: sherpa-onnx Apache-2.0 / MIT +
   ORT MIT — record exact versions pinned).

## Verification

- The mac-check-dylibs gate catches a stripped/missing sherpa dylib (a
  negative test: build once without it → the gate fails → restore).
- The packaged app on CI: dynamic-load Parakeet from the bundle (no dev
  paths); `otool -l` shows the Frameworks rpath on the sherpa dylib.
- The release artifact (tag dry-run or the next release) carries the dylibs;
  the owner's Mac pass = flip the Fast-engine toggle → Parakeet loads (the
  M10B runtime matrix).
- fmt/clippy -D warnings; workspace tests green; `cargo build --features
  parakeet` on the mac CI job; no desktop behavior change without the
  feature.
- Report: `docs/reports/M16f-sherpa-packaging.md` (per-dylib inventory, pins,
  gate behavior, honest boundaries).

## Do NOT

- No diarization inference yet (M16.6's later slices), no model downloads in
  this slice, no Android packaging (the Android sherpa library is slice 4 of
  the VAD plan's lineage), no new dependencies beyond what sherpa-rs already
  carries, no desktop behavior change with the feature OFF, no CI workflow
  edits beyond the macOS job's feature/package steps.
- No git commit/push; no sudo.
