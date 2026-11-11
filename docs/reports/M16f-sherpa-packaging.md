# M16F — sherpa release packaging gate (macOS)

**Status: implemented and locally verified; the macOS execution gates are written
but unrun on this box.** The repository lives on a Linux VPS with no Apple
toolchain and the work order forbids a push, so `otool`, `install_name_tool`,
`codesign`, `ditto` and dyld itself never executed here. Everything that can be
verified without Apple tools was verified (pins against the real universal2
libraries, gate logic against those real Mach-O files with an `otool`/`file`
emulator, shell/YAML syntax, fmt/clippy, the full workspace test run). The
CI macOS jobs and the owner's Mac are the execution gates — §8 names exactly
what each one still has to prove.

## 1. What ships

With `parakeet` in `SURAMAK_FEATURES`, `scripts/make-app.sh` now:

1. runs `scripts/bundle-sherpa-dylibs.sh`, which verifies
   `packaging/sherpa-dylibs.tsv` against the pristine copies sherpa-rs-sys left
   in `target/release`, copies the two linked libraries into
   `Contents/Frameworks`, adds `@executable_path/../Frameworks` as an `LC_RPATH`
   on each copy, and refuses to continue if the C API's `@rpath` chain does not
   resolve inside the bundle;
2. copies `packaging/THIRD-PARTY-NOTICES.txt` into `Contents/Resources`;
3. then runs the existing inside-out ad-hoc signing block, which already signs
   every `*.dylib` under `Contents/Frameworks` before sealing the app.

The app and CLI binaries get the `@executable_path/../Frameworks` rpath at link
time (`crates/suramak-ui/build.rs` now emits it for `updates` **or** `parakeet`;
`crates/suramak-cli/build.rs` for `parakeet`) — no post-hoc edit of an
executable, which is the project's existing contract. The dylibs themselves are
third-party artifacts whose load commands are rewritten only on the bundle copy,
before signing.

Runtime behaviour is unchanged: the feature is compiled in, but
`settings.asr.fast_engine` and the engine routing still decide whether Parakeet
is ever selected (the M10B default-OFF decision). Builds without the feature
link, bundle and ship exactly as before.

## 2. Per-dylib inventory

Source: sherpa-rs-sys 0.6.8's pinned download for both apple-darwin targets,
`sherpa-onnx-v1.12.9-osx-universal2-shared.tar.bz2`
(sha256 `e3f3596b84c65eef8939a8a45ced3d7c4ed857455ba5ae1148ce7a1c93ff9a3c`;
that hash is also the cache directory name). All values below were computed from
the cached universal2 files on this box.

| file | bytes | sha256 | slices (x86_64 / arm64) | install name | non-system deps |
|---|---:|---|---|---|---|
| `libsherpa-onnx-c-api.dylib` | 8 907 832 | `dbee23f44d7c66d858242127b9a7fffc99ee989726fc89386b64afb8214e78cb` | 4 607 504 / 4 287 544 | `@rpath/libsherpa-onnx-c-api.dylib` | `@rpath/libonnxruntime.1.17.1.dylib` |
| `libonnxruntime.1.17.1.dylib` | 50 396 248 | `4979386d1e8b42f4177fa110dbc9600b10846662fe555eac2f338ffef3ab6bfa` | 26 451 816 / 23 919 704 | `@rpath/libonnxruntime.1.17.1.dylib` | none (CoreML/CoreFoundation/Foundation/libc++/libobjc/libSystem only) |

Total added to the bundle: **59 304 080 bytes (56.56 MiB)** before zip/dmg
compression. Both are universal2 (the release target is arm64; the x86_64 slice
is carried by upstream and kept intact).

**Not bundled:** `libsherpa-onnx-cxx-api.dylib` (161 328 B). sherpa-rs-sys skips
every library whose name contains `cxx` when linking, and the C API does not
depend on it. Shipping it would be dead weight and a second thing to sign.

After the bundle step each copy carries `LC_RPATH @executable_path/../Frameworks`
— the upstream dylibs carry none — and ad-hoc signatures from the existing
signing pass. `install_name_tool` invalidates the upstream ad-hoc signature, so
the signature on the shipped copy is this project's, not upstream's.

## 3. Pins

`packaging/sherpa-dylibs.tsv` is the dylib pin manifest (name, bytes, sha256),
verified by `scripts/bundle-sherpa-dylibs.sh` **before** anything is copied —
a mismatch aborts with expected/actual, and no partial bundle is left behind.
`scripts/pin-model-hashes.sh` gained a fourth block that reproduces the manifest
from the sherpa-rs cache (or `SHERPA_LIB_DIR`), so a sherpa-onnx bump re-pins in
the same place every other artifact does. Its output matches the checked-in
manifest byte for byte today.

The Parakeet model pins are **unchanged and not duplicated** — the M10A
`PARAKEET.files` block in `crates/suramak-core/src/models.rs` remains the single
source of truth, enforced by the existing Model Manager download path
(`download::download_files` verifies size + sha256 per file, including on
resume, and re-verifies after rename):

| file | bytes | sha256 |
|---|---:|---|
| `encoder.int8.onnx` | 652 184 281 | `acfc2b4456377e15d04f0243af540b7fe7c992f8d898d751cf134c3a55fd2247` |
| `decoder.int8.onnx` | 11 845 275 | `179e50c43d1a9de79c8a24149a2f9bac6eb5981823f2a2ed88d655b24248db4e` |
| `joiner.int8.onnx` | 6 355 277 | `3164c13fc2821009440d20fcb5fdc78bff28b4db2f8d0f0b329101719c0948b3` |
| `tokens.txt` | 93 939 | `d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d` |

Total **670 478 772 bytes**. (This corrects the 670 609 772 figure in
`docs/ENGINES.md`, `docs/MODEL-PINS.md` and the historical `docs/reports/M10a.md`;
the four pinned sizes sum to 670 478 772, which is also what the installed
directory reports on this box. The living docs are corrected; M10a is left as
the historical record.)

**Why the in-bundle hash is not pinned:** the shipped copy differs from the
pinned upstream artifact by one added `LC_RPATH` load command. The byte result
of `install_name_tool` is not stable across Xcode/cctools versions, so a
post-patch hash would be a false pin. The report is explicit about the split:
pins cover the upstream artifact (supply-chain identity), and the gate covers
the post-patch properties (presence, rpath, resolution, signature).

**Where the model pins are enforced:** the Model Manager's install path
(`models download parakeet` → `download::download_files`) verifies size + sha256
for every installed file when it short-circuits an existing install and after
it downloads one; the recognizer's `load` then does the layout check
(`dir_ready`) exactly as in M10A. This slice deliberately did **not** add a
second hashing pass to `ParakeetRecognizer::load`: it would read 670 MB on every
first load (the M10A measured load is 5.9–6.4 s) and would fork the pin
enforcement instead of extending it.

## 4. Gate behaviour — `scripts/mac-check-dylibs.sh`

Kept from before: every `@rpath` dependency of **every Mach-O file** in
`Contents/MacOS` and `Contents/Frameworks` must resolve against the file's own
`LC_RPATH`s or the app's (the SUR2-NEW-06 rules; paths with spaces stay whole;
`/usr/lib/swift` is the dyld-shared-cache carve-out).

Added for M16F:

- **Sherpa chain, not just a generic dependency.** When any binary links
  `libsherpa-onnx-c-api.dylib`, the gate requires that file **and** the exact
  `libonnxruntime.<version>.dylib` named in its load commands to exist in
  `Contents/Frameworks`, each with `LC_RPATH @executable_path/../Frameworks`.
  A missing `libonnxruntime` therefore fails even though the app itself never
  names it.
- **`SURAMAK_EXPECT_PARAKEET=1`** makes the chain mandatory. Both macOS jobs set
  it, so a feature that silently falls out of the build cannot pass by having
  nothing to check.
- **Absolute non-system dependencies are a hard failure.** Previously a leaked
  `/Users/…/.cargo/…` or `~/.cache/sherpa-rs/…` load command passed silently
  (the gate only looked at `@rpath`). Any absolute dependency outside
  `/usr/lib`, `/System`, `/Library/Apple` now fails with
  “a dev path leaked into the bundle”. This is the static half of the
  clean-machine requirement.

`scripts/mac-check-dylibs-test.sh` is the negative test: it starts from a passing
bundle, hides each dylib listed in the manifest in turn, requires the gate to
fail, restores it (trap-protected), and re-requires a pass. If a hidden dylib
still passes, the script fails loudly. It prints
`dylib-gate-negative-ok: each bundled sherpa dylib is load-bearing for the gate`.
It hides the file rather than rebuilding without the feature: the gate's input
is the same (a bundle missing that dylib), and hiding each file in turn isolates
which link of the chain the gate actually depends on — a rebuild would only
prove the whole feature was absent.

Local logic verification (Linux, real universal2 dylibs, `otool`/`file`
emulated — the emulator reads the real load commands and takes only `LC_RPATH`
from sidecar specs, because the pre-bundle files have none):

| case | expected | result |
|---|---|---|
| full bundle | pass | pass |
| `libsherpa-onnx-c-api.dylib` removed | fail | fail |
| `libonnxruntime.1.17.1.dylib` removed | fail | fail |
| restored | pass | pass |
| C API without the bundle `LC_RPATH` | fail | fail |
| app with an absolute `/Users/runner/.cargo/...` dependency | fail | fail |
| `SURAMAK_EXPECT_PARAKEET=1` on a non-sherpa bundle | fail | fail |
| `mac-check-dylibs-test.sh` end-to-end | pass | pass (both dylibs load-bearing) |

This proves the script's logic; it does **not** prove Apple's `otool` output
format on a runner — that is the CI step, and the negative test there uses the
real tool.

## 5. Clean-machine dynamic-load smoke — `scripts/mac-smoke-parakeet.sh`

The packaged bundle is copied out of the repo with `ditto`, then
`suramak-cli doctor` runs under `env -i` with only `HOME`, `TMPDIR`, a system
`PATH` and `DYLD_PRINT_LIBRARIES=1`. The script asserts:

- the app binary links `@rpath/libsherpa-onnx-c-api.dylib` (the artifact that
  ships has the engine, not just a helper);
- `doctor` reports `compiled in (sherpa-onnx linked; parakeet feature on)`;
- dyld's trace names both libraries **under the copied bundle**, and no loaded
  image comes from `.cargo/`, `sherpa-rs`, `target/release` or `target/debug`.

It does not load the 670 MB model or run a decode. That is deliberate: model
loading and Core ML/ANE engagement on real Apple Silicon are the owner-Mac pass
(§8). The CLI is used because it is a non-GUI surface in the shipping bundle;
the same libraries are linked into the app binary and the gate checks both.

Local logic verification (Linux; fake CLI emitting doctor text and a dyld
trace, `ditto` stubbed): a bundle-resolved trace passes; a `.cargo/` trace
fails with “loaded from outside the copied bundle”. Harness only — the real
dyld trace is CI/owner-Mac.

## 6. CI and release wiring

`release.yml` (`macos-app`, tag builds):

- `Build release binaries` now compiles `suramak-ui` and `suramak-cli` with
  `speech-transcriber` **and** `parakeet`;
- `SURAMAK_FEATURES: speech-transcriber,updates,parakeet` for `make-app.sh`;
- gate with `SURAMAK_EXPECT_PARAKEET=1`, then the negative test, then the
  clean-machine smoke, then the existing zip/dmg packaging.

`ci.yml` (`macos`, every push/PR):

- new step `cargo build --release -p suramak-cli --features parakeet` before
  packaging (the CLI inside the bundle must carry the engine for the smoke);
- `SURAMAK_FEATURES: speech-transcriber,updates,parakeet`;
- same gate (with `SURAMAK_EXPECT_PARAKEET=1`), negative test and smoke steps.

No other job or workflow was touched.

## 7. Licenses

New surface: `packaging/THIRD-PARTY-NOTICES.txt`, copied into
`Contents/Resources` for a `parakeet` bundle and linked from the README. It
carries the exact pinned versions and the full upstream texts (Apache-2.0
requires the text to travel with the redistribution):

| component | version | license | upstream license sha256 |
|---|---|---|---|
| sherpa-onnx | v1.12.9 | Apache-2.0 | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |
| ONNX Runtime | 1.17.1 | MIT | `2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c` |

No other bundled component changed. `libsherpa-onnx-cxx-api.dylib` is not
redistributed, so it needs no notice.

## 8. Verification actually run (and what was not)

Run on this Linux VPS:

- `cargo fmt --all --check` — clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo test --workspace` (`RUST_TEST_THREADS=1`, the CI setting) — **1079
  passed, 0 failed, 10 ignored, 36 suites**; log at
  `build/m16f-workspace-tests.log`. Matches the 1079 baseline.
- `bash -n` on all six touched/new scripts — clean.
- YAML parse of both workflows — clean (5 jobs in ci, 4 in release).
- Pin reproduction: the new `pin-model-hashes.sh` block prints exactly the two
  manifest lines (bytes + sha256) from the real cache.
- Gate logic: the 8-case table in §4, against the real universal2 dylibs.
- Bundler logic: pin verification, copy, rpath addition and dependency check
  exercised with stubbed macOS tools; a one-byte corruption fails before any
  copy (no `Contents/Frameworks` created).
- Smoke logic: pass/fail paths in §5 with a stub CLI.

**Not run here (no Apple host, no push):** `otool` on the real bundle,
`install_name_tool`, `codesign`, `ditto`, dyld loading, and therefore the
authoritative negative test on a real bundle. The CI macOS jobs are the first
real execution; the owner's Mac is the second. A tag dry-run was not possible
without pushing.

**Owner-Mac pass (the honest boundary):** install the model
(`./target/release/suramak-cli models download parakeet`), build the bundle with
`SURAMAK_FEATURES=speech-transcriber,updates,parakeet ./scripts/make-app.sh`,
run the three gate commands, then flip Settings → Fast engine and transcribe an
English clip. Confirm the engine line says `parakeet (fast)`, the transcript is
sensible, and Core ML initialises (sherpa-onnx logs the provider; a failed
Core ML session is retried on CPU by design). That is the only way to validate
model loading, the Core ML execution provider and ANE behaviour on Apple
Silicon — none of which this VPS can execute.

## 9. Files changed

New:
- `packaging/sherpa-dylibs.tsv` — per-dylib pins.
- `packaging/THIRD-PARTY-NOTICES.txt` — license surface (sherpa-onnx + ORT).
- `scripts/bundle-sherpa-dylibs.sh` — verify + copy + rpath + resolve.
- `scripts/mac-check-dylibs-test.sh` — negative test for the gate.
- `scripts/mac-smoke-parakeet.sh` — clean-machine dynamic-load smoke.

Modified:
- `scripts/make-app.sh` — parakeet bundle step + notices.
- `scripts/mac-check-dylibs.sh` — sherpa chain, `SURAMAK_EXPECT_PARAKEET`,
  absolute dev-path check.
- `scripts/pin-model-hashes.sh` — fourth block for the dylib pins.
- `crates/suramak-ui/build.rs` — Frameworks rpath for `parakeet` as well as
  `updates`.
- `crates/suramak-cli/build.rs` — Frameworks rpath for `parakeet` (the bundled
  CLI links the engine).
- `.github/workflows/release.yml` — feature-on build/bundle + gate steps.
- `.github/workflows/ci.yml` — macOS job: CLI feature build, feature-on bundle,
  gate/negative/smoke steps.
- `docs/MODEL-PINS.md`, `docs/ENGINES.md` — dylib pins, implemented packaging
  boundary, corrected Parakeet total.
- `README.md` — notices pointer.

No git commit/push. No desktop behaviour change without the feature. No new
dependencies. No Android or Windows work.
