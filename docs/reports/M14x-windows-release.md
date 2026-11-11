# M14X — the Windows release artifact

**Status: implemented in `release.yml`, structurally verified on this VPS; the
first real execution is the next `v*` tag's CI.** This machine is Linux with no
Windows SDK, no `dumpbin`, no `rc.exe` and no PE bytes to inspect, so the
Windows build/PE half is a CI gate by construction (the same boundary M14a/M14b
documented). Everything that can be verified without Windows was verified —
including executing the packaging step under PowerShell 7.4 and exercising the
PE gate against a stubbed `dumpbin` (§6).

**Workorder**: `docs/WORKORDER-M14X.md`. **Tree**: post-`55015b5` (M16F landed;
1079 tests, five CI platforms green).

---

## 1. What ships

`windows-app` is a new job in `.github/workflows/release.yml` (line 226), shaped
like `linux-app`, running on the pinned `windows-2022` image:

| Step | What it does |
|---|---|
| `actions/checkout@v7` | source |
| Rust cache | `Swatinem/rust-cache@v2`, key `windows-release` |
| Build release binaries | `cargo build --release -p suramak-ui -p suramak-cli` |
| Package | stages the two EXEs + `LICENSE` + `README.md`, writes `suramak-<tag>-windows-x86_64.zip`, then its `.sha256` |
| Verify the packaged EXEs | extracts the zip and runs the PE gate on **those** bytes |
| `actions/upload-artifact@v7` | artifact `windows-app`: the zip + the `.sha256` |

No installer and no signing (both decisions in §5). The job has no `needs`, so
it runs in parallel with the macOS/Linux/Android jobs; the `release` job now
waits on it (`needs: [macos-app, linux-app, android-app, windows-app]`), so a red
Windows job blocks publishing rather than silently shipping an incomplete
release page.

### Zip naming

```
suramak-${GITHUB_REF_NAME}-windows-x86_64.zip
```

i.e. the tag as-is, exactly like the Linux job (`suramak-v0.4.1-linux-x86_64.tar.gz`)
and the macOS job (`Suramak-v0.4.1-macos-arm64.zip`): a `v0.4.2` tag yields
`suramak-v0.4.2-windows-x86_64.zip`. The Android job strips the `v` for its own
`versionCode` lineage; Windows follows the two desktop platforms, and every glob
on the release page (`suramak-*-windows-x86_64.zip`) is version-agnostic.

### Zip contents (root, no wrapper directory)

| Entry | Source |
|---|---|
| `suramak.exe` | `target/release/suramak.exe` — the GUI (`suramak-ui`'s `[[bin]]` name) |
| `suramak-cli.exe` | `target/release/suramak-cli.exe` — the console CLI |
| `LICENSE` | repository root |
| `README.md` | repository root |

`docs/PLAN-M14-WINDOWS.md` §Packaging asked for a `README.txt` and an in-zip
`SHA256SUMS`; the M14X workorder supersedes that with `LICENSE` + `README` and a
`.sha256` **beside** the zip (deliverable 4, the Android precedent). This report
records the supersession so the plan line is not mistaken for unfinished work.

The per-file sizes and the zip digest are produced by the tag run (they cannot
exist before a Windows build); §7 lists the exact log lines that fill it. The sandbox run in §6 proves the layout and the digest format with
stand-in files, not the product's bytes.

## 2. The build environment — one source of truth

The job's `env:` block is a byte-identical copy of the `ci.yml` `windows` job's
(`ci.yml` lines 419–426), comments included:

```yaml
    env:
      # whisper sessions are not concurrency-safe in-process (see the linux job)
      RUST_TEST_THREADS: "1"
      CMAKE_POLICY_DEFAULT_CMP0091: NEW
      CMAKE_MSVC_RUNTIME_LIBRARY: MultiThreaded
      CMAKE_TOOLCHAIN_FILE: ${{ github.workspace }}/scripts/windows/msvc-static-crt.cmake
      GGML_NATIVE: "OFF"
      GGML_AVX2: "ON"
```

Why each line is load-bearing (owned by the `ci.yml` comment and
`docs/reports/M14a.md`): `.cargo/config.toml` sets `+crt-static` for
`x86_64-pc-windows-msvc`, the three `CMAKE_*` variables make whisper.cpp/ggml
(`CMAKE_MSVC_RUNTIME_LIBRARY` + the CMP0091 policy) and libopus (the toolchain
file's `OPUS_STATIC_RUNTIME`) build against the static CRT too, and
`GGML_NATIVE=OFF` + `GGML_AVX2=ON` pin a portable AVX2 baseline instead of
`-march=native` (the SIGILL class the Linux job documents). A release build with
different settings would produce a different binary than the one CI proved.

`CMAKE_TOOLCHAIN_FILE` resolves through `${{ github.workspace }}`, so the block
is location-independent; no edit was needed when copying it.

No model cache: the release ships no models, so the CI-only
`suramak-windows-models-tiny-be07e048e1e5` cache (and the download/doctor/e2e
steps that use it) deliberately does not appear here. The release Windows app
downloads its model on first use, like every other platform.

## 3. Packaging

`Package suramak-<tag>-windows-x86_64.zip + .sha256` (`release.yml` line 252,
`shell: pwsh`):

- Fails if either EXE or either document is missing — a partially built tree
  cannot produce a green artifact.
- `Compress-Archive -CompressionLevel Optimal` over the staging directory's
  children, so the four entries sit at the archive root (extract → run
  `suramak.exe`; no nested folder).
- Deflate is lossless, so the PE gate in §4 inspects exactly the bytes a user
  downloads.
- SHA-256 via `Get-FileHash`, written as `<lowercase-hex><two spaces><filename>`
  with an **LF** ending (`[System.IO.File]::WriteAllText`, not `Out-File`, which
  would emit CRLF). That is `sha256sum(1)`'s format, so
  `sha256sum -c suramak-…zip.sha256` works on Linux/macOS — the same shape the
  Android release job writes (`release.yml` line 210). The step then reads the
  file back and compares it with a fresh hash before continuing, so a truncated
  write cannot pass.
- The step prints the zip size, the digest and a `Name/Length` table to the log —
  the source for the §7 record.

## 4. The PE gate — on the packaged bytes

`Verify the packaged EXEs — PE headers (GUI subsystem, no dynamic CRT)`
(`release.yml` line 293) extracts the zip into `build/windows-verify` and then:

1. **Byte identity** — each extracted EXE's SHA-256 must equal
   `target/release/<exe>`'s. This catches a packaging bug (wrong file, stale
   file, corrupt archive) before any PE claim is made.
2. **No dynamic CRT** — `dumpbin /dependents` on **both** EXEs must not match
   `(?im)^\s*(vcruntime|msvcp)`. This is the CI check's pattern and its D4
   enforcement, moved onto the released bytes. `dumpbin` is located the same way
   `ci.yml` does (the newest MSVC toolset under `Program Files`).
3. **GUI subsystem** — `dumpbin /headers suramak.exe` must contain a
   `(Windows GUI)` line and no `(Windows CUI)` line. This is the CI assertion
   verbatim, and it is what makes double-clicking the EXE open a window instead
   of a console.
4. **Console CLI** — `dumpbin /headers suramak-cli.exe` must contain
   `(Windows CUI)`. A bonus assertion: it proves the two binaries were not
   swapped in the zip.

Failure in any of these fails the job, which fails the `release` job, which
means no release page — fail-closed, as with the macOS dylib gate.

## 5. Decisions

**No installer.** The zip is the artifact (workorder deliverable 2). The owner's
"add installer, it's fine" note refers to the M14 install *experience*; until an
NSIS/MSI slice is commissioned, the zip + the first-run note is the shipped
form. An installer would add a second artifact, an uninstall path and another
unsigned binary for SmartScreen to flag — none of it asked for here.

**No signing infrastructure.** No paid certificate; the owner accepted the
first-launch SmartScreen warning (2026-10-16). The release notes say so plainly
and point at the published digest. There is no `signtool` anywhere in the
workflow, so a future signing slice has to add it deliberately.

**The SmartScreen sentence** (release notes, `release.yml` line 362), after a
Windows packaging line:

> Windows: extract the zip and run suramak.exe (suramak-cli.exe is the console
> CLI) — no installer; verify the download against the .sha256 file.
>
> Windows may show a SmartScreen note on first launch — More info → Run anyway;
> the binary is bit-for-bit the CI build with a published sha256.

**No `workflow_dispatch` dry-run.** It cannot be added without changing the
job's semantics: `release.yml` triggers only on `v*` tags, and on a dispatch
`GITHUB_REF_NAME` is a branch name, so the windows job would build
`suramak-main-…` while the `release` job would try to publish a GitHub release
for a non-tag ref (and the CHANGELOG extraction would find no section). A true
dry-run needs a separate throwaway workflow, which the workorder does not
authorise. The tag run is therefore the execution gate (§7).

**Inline PowerShell, no new script.** The packaging/PE logic only ever executes
on a Windows runner — a checked-in `.ps1` would be just as unexecutable on this
VPS, and the other platform jobs keep their packaging inline. One file owns the
release contract; no new dependency, no new entry point.

**Feature parity with the CI Windows build.** No `speech-transcriber`
(macOS-only) and no `parakeet` (D4: not packaged on Windows); the Windows app
ships the Whisper CPU engine, as M14b shipped it.

## 6. Verification performed here

| Check | Result |
|---|---|
| YAML parse (`yaml.safe_load`, PyYAML 6.0.3) | 5 jobs, order `macos-app, linux-app, android-app, windows-app, release` |
| Structural assertions (42, described below) | **42 PASS / 0 FAIL** |
| `env:` block vs `ci.yml` | dict-equal **and** byte-identical text (comments, order, values) |
| Job shape | `windows-2022`, `timeout-minutes: 60`, no `needs`, six steps in order |
| Release wiring | `needs` includes `windows-app`; files list has zip + `.sha256`; SmartScreen sentence present verbatim |
| Forbidden machinery | no `signtool`/`nsis`/`makensis`/`wix`/`msi` in the file |
| PowerShell parse | PowerShell 7.4.6 parser: both step bodies clean (226 and 276 tokens) |
| Packaging step executed | real run under pwsh with stand-in binaries: zip `suramak-v0.0.0-sandbox-windows-x86_64.zip`, 4 entries at root, `.sha256` LF-only, `sha256sum -c` → `OK`, extracted files `cmp`-identical |
| PE gate executed (logic) | harness with only the `dumpbin` discovery line replaced: clean → `PE-OK` exit 0; injected `VCRUNTIME140.dll` → throws; GUI exe flipped to CUI → throws |
| `cargo fmt --all --check` | clean |
| `git diff --stat` | `release.yml` only, +128/−1 (plus this report) |

The structural script asserted, among other things: the exact build command;
the four zip entries and their sources; the `Compress-Archive` root layout; the
`sha256sum(1)` format; `Expand-Archive` **before** the first `dumpbin` call (the
gate is on packaged bytes); both EXEs checked for `vcruntime|msvcp`; the GUI
pattern on `suramak.exe`; both artifact globs; `if-no-files-found: error`; and
the absence of any `${{ }}` inside the PowerShell bodies.

`cargo clippy --workspace --all-targets -- -D warnings` **passed** (cached
target, 1.71 s, exit 0), and `cargo test --workspace` was run against the cached
build: **1079 passed, 0 failed** (exit 0), matching the post-M16F baseline. No
Rust source is in the diff, so fmt/clippy/test outcomes are unchanged by
construction. (The UI-compact suite rewrites two tracked screenshot fixtures
under `tests/ui-compact/shots/`; they were restored with `git checkout --`, so
the diff is the workflow plus this report only.)

## 7. The honest boundary — what the tag run must prove

No Windows host exists here, so the following have **never executed**: `rc.exe`,
the MSVC link of the GUI binary, `dumpbin` on a real PE, `Compress-Archive` over
the real ~tens-of-MB EXEs, and the GitHub artifact/release plumbing. The first
tag after this lands is the real gate. It is green only if all of this is true:

- [ ] `windows-app` runs all six steps green within the 60-minute timeout.
- [ ] Log shows `packaged suramak-vX.Y.Z-windows-x86_64.zip (<bytes> bytes)` and
      `sha256 <64 hex>`; the `Name/Length` table lists the two EXEs, `LICENSE`,
      `README.md`.
- [ ] Log shows `subsystem evidence: GUI=[… (Windows GUI)] CUI=[]` and
      `PE-OK: suramak.exe (Windows GUI), suramak-cli.exe (Windows CUI), no
      VCRUNTIME/MSVCP imports` — from the extracted zip, not `target/release`.
- [ ] Artifact `windows-app` contains exactly the zip and its `.sha256`.
- [ ] The release page lists both, and `sha256sum -c` passes on a downloaded
      copy.
- [ ] On a real Windows machine: extract, `suramak.exe` launches (SmartScreen →
      More info → Run anyway), `suramak-cli.exe doctor` is clean, and one
      transcription works.

Then record the real numbers here:

| Entry | Size (bytes) | SHA-256 |
|---|---|---|
| `suramak.exe` | _from the tag log_ | — |
| `suramak-cli.exe` | _from the tag log_ | — |
| `suramak-<tag>-windows-x86_64.zip` | _from `packaged … (N bytes)`_ | _from `sha256 …`_ |

If the job fails, the release is blocked by design; removing `windows-app` from
`needs` would unblock the other platforms, but that is a deliberate rollback,
not a default.

## 8. Files changed

- `.github/workflows/release.yml` — new `windows-app` job (line 226); `release`
  job `needs` (line 344); release-notes Windows + SmartScreen lines (line 363);
  files list (lines 382–383).
- `docs/reports/M14x-windows-release.md` — this report.

Untouched: `ci.yml` (byte-identical), all Rust sources, `Cargo.toml`/lockfile,
`scripts/`, `packaging/`, `CHANGELOG.md`, Android/macOS jobs. No git
commit/push, no sudo, no new dependencies, no signing infrastructure.
