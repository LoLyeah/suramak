# WORKORDER — M14X: the Windows release artifact (completes the platform matrix on the release page)

Tree: dispatches after the M16F child lands (both touch `release.yml`).
Context: v0.4.1's release page ships mac dmg/zip + linux tar.gz + android APK
— **no Windows artifact**, despite M14b shipping a first-run PASS build. The
plan's Windows line (M14) ends at "installer restored to v1" with the free
signing decision: **no paid cert; a release-signed-style SmartScreen warning
is accepted** (the owner's call, 2026-10-16).

## Deliverables

1. **`release.yml` windows job** (mirrors the linux job's shape):
   - `runs-on windows-2022`, the M14-era env block (crt-static,
     `CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`, `GGML_NATIVE=OFF` … reuse
     the ci.yml windows job's env verbatim — one source of truth, no drift).
   - Build `suramak` + `suramak-cli` (release, `x86_64-pc-windows-msvc`).
   - Package `suramak-<tag>-windows-x86_64.zip`: the two EXEs + LICENSE +
     README; verify the PE checks the CI already asserts (subsystem GUI = the
     `(Windows GUI)` pattern, `dumpbin`/`strings` proof of NO VCRUNTIME/
     MSVCP dynamic deps — crt-static carries them).
   - Upload the artifact; the release job attaches it
     (`suramak-*-windows-x86_64.zip` in the files list).
2. **No installer yet**: the zip is the artifact (the owner's "add installer
   it's fine" note refers to the M14 install experience — the zip + a
   first-run SmartScreen note in the release notes IS the shipped form until
   the owner asks for an NSIS/MSI; document the decision in the report).
3. **Release notes template**: the SmartScreen warning sentence
   (unsigned-for-now: "Windows may show a SmartScreen note on first launch —
   More info → Run anyway; the binary is bit-for-bit the CI build with a
   published sha256").
4. **sha256 file beside the zip** (the android pattern).

## Verification

- The tag CI run: the windows release job green; the artifact attached to the
  release page; PE checks green on the RELEASED bytes (subsystem + no
  dynamic CRT).
- The zip's contents listed in the report (the EXE sizes + the sha256).
- fmt/clippy -D warnings; workspace tests untouched-green.
- Report: `docs/reports/M14x-windows-release.md`.

## Do NOT

- No code changes outside the release workflow + packaging scripts; no
  signing infrastructure; no CI edits beyond the windows job's addition; no
  Android/mac behavior changes; no git commit/push; no sudo.
