# M14 — Suramak for Windows: Milestone Plan

Status: draft for owner review. No workorder is cut until this plan is approved.
Drafted by **claude-opus-5.5** (via OpenRouter) at the owner's request, 2026-10-16; grounded and reviewed by Hermes against the live repo. Items marked ASSUMPTION are unverified at planning time.
Scope: a first-class `windows-x86_64` desktop build of the existing app, with the same Deck UI, pipeline and invariants, verified on the owner's Windows PC.

---

## Summary

M14 makes Windows a real target, not a compile-checked stub. The shared crates `suramak-core`, `suramak-asr`, `suramak-cli`, `suramak-ui` and `suramak-playback` build and run natively on Windows with MSVC. The platform seams that are macOS- or Unix-only today get Windows implementations:

- **Material:** DWM system backdrop, replacing the stub in `suramak-glass`.
- **Secrets:** Credential Manager, replacing Keychain.
- **Single instance:** a named mutex, replacing `flock`.
- **Notifications:** native toasts.

CI produces a signed per-user installer plus a portable zip on every tag.

Guiding decisions:

1. **Documented Win32/DWM APIs only, via the `windows` crate.** We do not use undocumented attributes or `SetWindowCompositionAttribute`. We do not paint anything that imitates Acrylic or Mica. Where the OS cannot provide the material, the window uses the non-glass Deck surface.
2. **Build natively on a Windows runner with MSVC.** No cross-compiling for release artifacts.
3. **Static CRT, portable CPU baseline.** The zip contains our executables and nothing else. It has no VC++ runtime DLLs and no ONNX Runtime DLL. The build never uses `-march=native` or GGML native tuning.
4. **v1 is the default engine set: whisper.cpp plus Opus.** Parakeet is compile-checked on Windows but not packaged, which matches its status on macOS and Linux. SpeechTranscriber, Process Taps and Sparkle do not apply.
5. **The owner's PC is the source of truth for anything visual or OS-integrated.** CI covers build, tests, a headless CLI transcription, and a best-effort GUI launch smoke on a software adapter.
6. **Motion stays first-class.** Window activation, backdrop transitions, progress and toast-triggering completion states all get the same motion treatment as on macOS. Windows-specific frame-pacing hazards (modal resize loop, DWM vsync) are tracked as explicit work, not afterthoughts.

Non-goals for v1:
- In-app updater
- MSI/MSIX/winget packaging (v1 ships our own per-user installer + portable zip)
- ARM64 Windows
- System-audio capture
- GPU-accelerated whisper on Windows (Vulkan/CUDA is its own follow-up milestone — owner, 2026-10-16)
- File-type associations

---

## Decisions

### D1 — Frosted-glass material: direct `windows` crate calls into DWM, in `suramak-glass`

**Decision.** Add a `windows` module to `suramak-glass` that uses the `windows` crate (features `Win32_Graphics_Dwm`, `Win32_Foundation`, `Win32_UI_Controls`). It applies the material to the winit `HWND`, obtained via `raw-window-handle`.

**Material ladder** (the build number is detected once at startup with `RtlGetVersion`, not `GetVersionEx`, which lies without a manifest):

| Windows build | Material | Calls |
|---|---|---|
| ≥ 22621 (Win11 22H2+) | **Acrylic** backdrop for the main window | `DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW`; `DwmExtendFrameIntoClientArea(-1 margins)`; `DWMWA_USE_IMMERSIVE_DARK_MODE`; `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND` |
| 22000–22620 (Win11 21H2) | **No backdrop**; opaque Deck surface | dark mode + rounded corners only |
| Windows 10 | **Not supported** — the milestone is Windows 11-only (owner, 2026-10-16); no material work | — |
| "Transparency effects" off in Settings, or high contrast | Opaque Deck surface, live-switched | listen for `WM_SETTINGCHANGE` / `WM_DWMCOMPOSITIONCHANGED` |

Why Acrylic (`TRANSIENTWINDOW`) and not Mica (`MAINWINDOW`):
- The milestone brief and the Deck language ask for *frosted*. Acrylic is the OS's real blur-behind material.
- Mica is a wallpaper-tinted, low-cost material. It reads as "tinted solid", not glass.
- Microsoft's guidance prefers Mica for long-lived windows. That is a style guideline, not a capability limit.
- We expose `Mica` as an internal variant in the same enum, so switching is a one-line change if the owner prefers it on hardware (see Open Questions).

Why not `window-vibrancy`:
- It is a thin wrapper over the same calls.
- On Win10 and 21H2 it reaches for undocumented composition attributes, which have known resize lag and break across updates. That is exactly the "fragile fake-adjacent" path we refuse.
- It adds a dependency whose `windows-sys` version we would have to keep aligned with our pinned winit fork.

Owning ~200 lines in `suramak-glass` mirrors how we already own the NSGlassEffectView surface.

**Why no backdrop below 22621 rather than a painted blur.** "Never painted fakes" applies to Windows materials as much as Apple's. The opaque Deck surface is the honest degraded state. It is the same rule that gives Linux its own treatment rather than an imitation.

**The real hard part is the swapchain, not DWM.** The backdrop only shows if the client area is composited with alpha. This requires:
- winit `with_transparent(true)`
- a wgpu surface with a non-opaque `CompositeAlphaMode`

On Windows that generally means the DX12 backend presenting through DirectComposition (wgpu's DX12 `presentation_system` set to the visual-based path). Vulkan on Windows typically reports `Opaque` only.

ASSUMPTION: the wgpu version pinned in `suramak-ui` exposes the DX12 visual presentation option. If it does not, M14c includes the wgpu bump or a documented fallback to opaque. This is why M14c starts as a timeboxed spike.

**Clear colour contract.** On Windows-with-backdrop, the Deck root clears to fully transparent. The UI paints its own translucent tint layers over the OS material. This tinting is design, not a fake material: the same layering the macOS build places over real glass.

### D2 — Single-instance guard: named mutex plus activation event

**Decision.** In the platform layer that currently holds the `flock` guard, add `#[cfg(windows)]`:
- `CreateMutexW` on `Local\Suramak-SingleInstance-{user SID}`.
- If `GetLastError() == ERROR_ALREADY_EXISTS`, the new process signals a named auto-reset event `Local\Suramak-Activate-{SID}` and exits 0.
- The first instance waits on that event on a background thread. When signalled, it posts a winit user event that restores, raises and focuses the window, using the existing window-reveal motion.

ASSUMPTION: the current Unix guard lives in a small module behind a function like `acquire_single_instance()`. If the guard is inlined in `suramak-ui`'s startup, M14b extracts it into that shape first, with no behaviour change on Unix.

Design notes:
- Use the `Local\` namespace, not `Global\`. It is per logon session, works without privileges, and fast-user-switching gives each user their own instance.
- The SID in the name prevents RDP or other-user collisions.
- The OS releases the mutex on crash, so there is no stale-lockfile problem. This is strictly better than lockfiles.
- Raising a window from a background-signalled process is subject to foreground-lock rules. The exiting second process calls `AllowSetForegroundWindow(ASFW_ANY)` before signalling.
- No path or argument handoff in v1, since there are no file associations. The event is upgradable to a named pipe later if "Open with Suramak" lands.

### D3 — Polish API key: Windows Credential Manager via native `CredWriteW` / `CredReadW` / `CredDeleteW`

**Decision.** Implement the `suramak-core` keychain seam on Windows with direct `windows` crate calls (`Win32_Security_Credentials`):

| Field | Value |
|---|---|
| Type | `CRED_TYPE_GENERIC` |
| `TargetName` | `Suramak/polish-api-key` (mirrors the macOS service/account naming; exact string aligned with the existing constant) |
| `Persist` | `CRED_PERSIST_LOCAL_MACHINE`: per-user, not roaming |
| Blob | UTF-8 bytes of the key. The limit is `CRED_MAX_CREDENTIAL_BLOB_SIZE` = 2560 bytes; oversized input is rejected with a clear error, not silently truncated |
| Cleanup | memory returned by `CredReadW` is freed with `CredFree`, and our copy is zeroized |

Why not the `keyring` crate:
- We already take the `windows` dependency for D1 and D2.
- The surface is three calls.
- We want control over target naming, persistence scope and error mapping, since "key missing" vs "access denied" drives distinct UI states.

If `suramak-core` already uses `keyring` for macOS, the cost calculus flips. In that case enable its `windows-native` feature instead and stop. ASSUMPTION: it does not; the M14d workorder confirms this before writing code.

`CRED_PERSIST_ENTERPRISE` is rejected deliberately. Roaming a user's API key through domain profiles is not something a local-first app should do silently.

### D4 — CI: native Windows runner, MSVC, pinned image

**Decision.** Add a `windows` job to `ci.yml` on a **pinned** image (`windows-2022` or `windows-2025`, decided in M14a by whichever builds whisper.cpp clean). Do not float `windows-latest`. Use the `x86_64-pc-windows-msvc` target.

Why native rather than cross-compiling from Linux (`cargo-xwin` / mingw):
- whisper.cpp (via `whisper-rs-sys`), libopus (via the symphonia adapter's build) and optional sherpa-onnx are built by `cmake` and `cc`. Each has MSVC-specific paths and flags.
- `bindgen` needs `libclang`. The hosted Windows images ship LLVM, Visual Studio Build Tools and CMake.
- Under `cargo-xwin`, each C dependency is a separate failure surface, and prebuilt sherpa-onnx Windows libs are MSVC-ABI.
- mingw produces a different ABI than what sherpa-onnx's Windows builds expect, and is a second toolchain to support forever.
- The artifact must be built where it can at least be *launched*.

Required build settings, enforced in CI:

- **Static CRT.** Set `RUSTFLAGS=-C target-feature=+crt-static` for the Windows target in `.cargo/config.toml`. CMake deps must match (`CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`) or the link fails loudly. A CI step runs `dumpbin /dependents suramak.exe` and **fails** if `VCRUNTIME140.dll` or `MSVCP140.dll` appear. This is the enforcement of "nothing bundled except compiled-in C".
- **Portable CPU baseline.** Disable GGML native tuning and set an explicit x86-64 feature level. AVX2 is acceptable for the release, with an SSE-only fallback considered if the owner's PC lacks it, which is unlikely. ASSUMPTION: the existing Linux runner SIGILL may stem from build-host-tuned code. M14a checks whether the same flag fixes it, but does not block on it.
- `#![cfg_attr(windows, windows_subsystem = "windows")]` on the GUI binary so no console window appears. The CLI stays a console subsystem binary.

**Automated GUI smoke viability.** Hosted Windows runners have no GPU but do run an interactive desktop session. The plan:

- **Attempt:** wgpu DX12 with `force_fallback_adapter: true`, which selects **WARP**, Microsoft's software rasterizer, and is always present. Vulkan is not tried in CI: there is no ICD on the runner, and installing SwiftShader or lavapipe means fetching third-party binaries for a test we can do with WARP.
- **Smoke harness reuse:** the existing scripted smoke harness runs with a `SURAMAK_SMOKE=1`-style entrypoint (ASSUMPTION: the harness is mode-driven, not Xvfb-specific). It launches, renders N frames, imports a fixture WAV, transcribes with `ggml-tiny` (cached via `actions/cache` from the sha256-pinned manager), exports SRT, and exits 0.
- **Screenshots:** capture is attempted via the app's own frame readback, not OS screen capture. Pixel *assertions* are **not** ported in v1. WARP rasterization and DWM-on-runner composition differ enough that the Xvfb goldens would produce noise.
- **Honest fallback:** if the WARP smoke is flaky beyond two reruns in M14g, it is marked `continue-on-error`. The glass, notification and input lanes are owner-PC-only. The materials can never be CI-verified anyway: DWM backdrops are typically disabled in a GPU-less session.

### D5 — Notifications: native toast on completion; system-audio capture is out of v1

**Notifications (in v1).** Exactly one event fires a toast: *transcription finished* (or *failed*, with the named reason), and only when the window is not foreground.

- **Implementation:** the `windows` crate's `UI_Notifications` WinRT APIs, using a toast XML with title, file name and duration.
- **Identity:** an unpackaged app needs an AppUserModelID. On first toast, register `HKCU\Software\Classes\AppUserModelId\LoLyeah.Suramak` with `DisplayName` and `IconUri`, and call `SetCurrentProcessExplicitAppUserModelID`.
- **Removal:** the registration is per-user, reversible, and removed by a documented one-liner. No Start-Menu shortcut is created.
- **Fallback:** if registration or toast creation fails, the app calls `FlashWindowEx` (taskbar flash) and logs the reason. A failure is never fatal and never silent in logs.
- **Taskbar progress (in v1, motion-first):** `ITaskbarList3::SetProgressValue` mirrors pipeline progress, with error state on failure. It is cheap, very visible, and makes long transcriptions legible from the taskbar.
- **Rejected:** `winrt-notification` / `tauri-winrt-notification`, which default to borrowing PowerShell's AUMID, so the toast says "Windows PowerShell".

**System-audio capture (not in v1).** WASAPI offers two paths:
- Whole-endpoint loopback, which cpal can open.
- Per-process loopback (`AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`, build 20348+), which is the real analogue of macOS Process Taps.

Shipping whole-endpoint loopback would put a feature with *different semantics* under the same UI as Taps: it captures everything, including notification dings. That breaks cross-platform UX coherence. Doing per-process properly is a self-contained slice with its own owner-testing burden. Therefore:
- v1 hides the system-audio source on Windows. `suramak-taps` is `cfg(target_os = "macos")` and the UI capability flag reports `false`.
- Per-process loopback is logged as a follow-up candidate ("M14.1 or later").

---

## Slice breakdown

Each slice is one workorder, lands as Conventional Commits with a CHANGELOG `Unreleased` entry, and ends with a CI workflow artifact the owner can download.

### M14a — Build bring-up and Windows CI job

- **Goal:** the workspace compiles, tests pass, and the CLI transcribes on Windows.
- **Deliverables:**
  - `cfg` gating so `suramak-taps`, Sparkle (`updates` feature) and the macOS glass path are excluded on Windows.
  - Workspace `Cargo.toml` target-specific deps.
  - `.cargo/config.toml` with Windows `crt-static`.
  - whisper build flags with a portable CPU baseline.
  - A `ci.yml` `windows` job: fmt, clippy (`-D warnings`), `cargo test` for core/asr/cli/playback, `suramak-cli doctor`, `suramak-cli transcribe` on a fixture with cached `ggml-tiny`, and the `dumpbin` CRT check.
  - A workflow artifact with `suramak-cli.exe`.
  - Parakeet feature `cargo check` only.
- **Verified by:** CI green; the owner downloads the artifact and runs `doctor` and `transcribe` on an English and an Indonesian sample.
- **Risks:**
  - CMake/CRT mismatch in a C dependency.
  - `libclang` discovery.
  - Unix-only code in shared crates surfaces as compile errors. Expected and fixed here; it is the point of the slice.

### M14b — GUI launches: shell basics on Windows

- **Goal:** the Deck UI runs on Windows with an opaque surface, correct paths and a single instance.
- **Deliverables:**
  - `windows_subsystem` attribute.
  - A `build.rs` in the UI binary crate using `embed-resource` for the icon, version info and an **application manifest** with:
    - PerMonitorV2 DPI awareness
    - `longPathAware`
    - `<activeCodePage>UTF-8</activeCodePage>`
    - Common Controls v6
    - `supportedOS` GUIDs
  - The D2 single-instance guard.
  - Data and model directories resolved to `%LOCALAPPDATA%\Suramak\` (models are huge and must not roam).
  - The sqlite library path under the same root.
  - The Ctrl modifier mapped where macOS uses Cmd (ASSUMPTION: shortcuts are already modifier-abstracted for Linux; verify).
  - "Reveal in folder" via `explorer /select,`.
- **Verified by:** the owner lane runs a first-launch checklist (launch, second-launch activates the first, resize/move/maximize, snap layouts, 100%/150% scaling, drag a file in).
- **Risks:**
  - DPI scaling glitches when moving between monitors.
  - The winit fork's Windows path diverging from upstream. Low risk, since the fork only patches macOS IME, but confirm the fork's base includes current Windows fixes.

### M14c — Real DWM material, dark mode, motion pacing

- **Goal:** D1 shipped.
- **Deliverables:**
  1. A **timeboxed spike** (≤ 2 days) proving a transparent wgpu DX12 surface over a DWM backdrop on the owner PC. The spike answers: wgpu version and DX12 presentation option, `CompositeAlphaMode`, and resize behaviour.
  2. `suramak-glass/src/windows.rs` implementing the material ladder.
  3. Transparency-setting and high-contrast live switching, with a motion cross-fade between glass and opaque states.
  4. Deck clear-colour contract per platform.
  5. Frame-pacing work: continuous redraw during the Win32 modal size/move loop so animations do not freeze while dragging, and time-based animation verified under DWM vsync.
- **Verified by:** owner lane on Win11 22H2+ checks acrylic visible, dark title bar, rounded corners, toggling Settings → Personalization → Colors → Transparency, resize while an animation is running. If the owner has a Win10 machine, an opaque-fallback check. A CI unit test covers the build-number → material mapping.
- **Risks:**
  - wgpu cannot present with alpha on DX12 at our pinned version, triggering an upgrade.
  - Flicker or black frames on resize with DirectComposition.
  - **Gate:** if the spike fails, M14c ships the opaque path plus dark mode and corners, and the backdrop moves to a follow-up. The owner decides; we do not fake it.

### M14d — Credential Manager and polish end-to-end

- **Goal:** D3 shipped; the polish feature works against a user endpoint.
- **Deliverables:**
  - The `suramak-core` keychain seam's Windows impl.
  - Error mapping (`ERROR_NOT_FOUND` → "no key stored").
  - Unit tests behind `#[cfg(windows)]` that write, read and delete a test target name, running in the CI Windows job.
  - The polish text and audio paths exercised.
- **Verified by:** CI tests. Owner lane: store a key, restart, polish a transcript, confirm the entry appears under Credential Manager → Windows Credentials, delete it from the app and confirm it is gone.
- **Risks:** small. The main one is a target-name mismatch with existing settings migration; there is nothing to migrate on a fresh platform.

### M14e — Full pipeline on real hardware

- **Goal:** every pipeline step verified on Windows.
- **Deliverables:**
  - Mic capture through cpal/WASAPI (ASSUMPTION: capture is cpal-based) at 16 kHz mono PCM16, including resampling from the device's native 48 kHz shared-mode format.
  - Microphone privacy detection: when "Let desktop apps access your microphone" is off, the stream yields silence or errors. The app shows a named reason with a link to `ms-settings:privacy-microphone`, and `doctor` reports it.
  - Import via file dialog and drag-drop; symphonia decode of all supported containers plus Opus.
  - Playback via cpal.
  - Exports for md/txt/json/srt/vtt.
  - Model download with resume on NTFS (rename-over-existing while Defender holds the file, with retry and backoff).
  - The engine-routing invariant tests run on Windows.
- **Verified by:** owner checklist covering a 60-minute recording, an Indonesian file, a non-ASCII path (`C:\Users\<name>\Música\…`), a model download interrupted and resumed, and each export opened in a target app. CI covers the routing invariants and export goldens.
- **Risks:**
  - whisper.cpp `fopen` with non-ASCII paths. Mitigated by the UTF-8 active code page in M14b; verify explicitly.
  - Exclusive-mode or Bluetooth headset sample-rate quirks.

### M14f — Notifications and taskbar progress

- **Goal:** D5 shipped.
- **Deliverables:** toast on completion or failure when backgrounded; AUMID registration with removal docs; `FlashWindowEx` fallback; `ITaskbarList3` progress and error states; a `docs/` note on what registry key is written.
- **Verified by:** owner lane: background the app during transcription, receive the toast, click it to focus the app, watch taskbar progress. With Focus Assist / Do Not Disturb on, a suppressed toast with no crash is the expected result.
- **Risks:** toast attribution for unpackaged apps varies by build. The fallback is defined.

### M14g — CI GUI smoke and release artifact

- **Goal:** D4 smoke attempt; Windows in `release.yml`.
- **Deliverables:** a WARP smoke step in the `windows` job; `release.yml` Windows job producing the **installer + portable zip + SHA256SUMS** (see Packaging), including the installer tooling choice and the SignPath signing step once the certificate is available; README download section.
- **Verified by:** a dry-run tag on a fork or prerelease tag produces the zip; the owner downloads it from the Release page and runs it cold.
- **Risks:** WARP smoke flakiness, handled per D4's honest fallback.

### M14h — Owner verification pass and docs

- **Goal:** sign-off.
- **Deliverables:**
  - `docs/windows.md`: supported versions, SmartScreen first-run steps, data locations, how to uninstall (delete folder + `%LOCALAPPDATA%\Suramak` + Credential Manager entry + AUMID key).
  - The CHANGELOG `Added` entries.
  - The release-notes paragraph.
- **Verified by:** the owner completes the full checklist (Testing §) on the release artifact, not a dev build.

---

## Platform layers

Checklist of platform-gated seams needing Windows work. ASSUMPTION: exact file paths are confirmed by the M14a workorder; crate-level placement below is from the known crate responsibilities.

- [ ] **Window material** — `suramak-glass`: new `windows` module (D1); stub removed.
- [ ] **Surface alpha / clear contract** — `suramak-ui` renderer setup: DX12 presentation path and per-platform clear colour.
- [ ] **Single-instance guard** — current `flock` site → `cfg(unix)` / `cfg(windows)` split (D2).
- [ ] **Secret storage** — `suramak-core` keychain seam → Credential Manager (D3).
- [ ] **App data / model paths** — `suramak-core` settings and paths → `%LOCALAPPDATA%\Suramak`.
- [ ] **System-audio capture** — `suramak-taps`: remains macOS-only; UI capability flag `false` on Windows.
- [ ] **Updater** — `updates` feature (Sparkle): never enabled on Windows; no UI entry.
- [ ] **SpeechTranscriber** — feature excluded; routing falls to whisper with the existing named reason.
- [ ] **Parakeet / sherpa-onnx** — `cargo check` only; not packaged.
- [ ] **Notifications** — new Windows impl (D5) behind whatever notify seam exists. If none exists, introduce `notify_completion(…)` in `suramak-ui` with no-op default.
- [ ] **Taskbar progress** — new, Windows-only.
- [ ] **Mic permission diagnostics** — `suramak-asr` capture error mapping and `suramak-cli doctor`.
- [ ] **Reveal-in-folder / open-URL** — `explorer /select,` and `ShellExecuteW`.
- [ ] **Keyboard modifiers** — Cmd ↔ Ctrl mapping.
- [ ] **Executable metadata** — manifest, icon, version resource via `build.rs`.
- [ ] **Subsystem** — GUI binary `windows_subsystem`.

**Stays shared and unchanged:**
- The data model and sqlite library.
- The engine pipeline and worker.
- whisper.cpp integration.
- symphonia and libopus decode.
- The playback crate (cpal abstracts WASAPI).
- The routing invariants (Indonesian never routes to fast engines; any failure degrades to whisper with a named reason).
- Exports.
- The model manager with sha256 pins and resume.
- The polish client.
- The entire Deck layout and motion system.

The Windows changes are additive and fenced: every new code path sits behind `#[cfg(windows)]` or a capability flag that already defaults to `false`, so the macOS build compiles the same feature set it does today and no shared crate gains a Windows-only dependency in its default features.

**Rules for the platform seam:**
- No `#[cfg(windows)]` inside the engine, routing, model manager, or exports. If one of those needs a platform fact, it gets it through `suramak-core` (paths, capabilities) and never asks the OS directly.
- Capability flags are read once at startup and passed down. The UI hides unsupported features instead of showing them disabled with a tooltip, which matches how macOS already treats SpeechTranscriber on older OS versions.
- Each new Windows impl ships with a no-op or stub on macOS only where a shared trait demands one. We do not write speculative macOS ports of Windows-only features such as taskbar progress.
- `cargo check --target x86_64-pc-windows-msvc` runs on every PR from the first slice onward, even before anything is packaged.

## Packaging & release

- [ ] **Target**: `x86_64-pc-windows-msvc` only, **Windows 11 only** (owner, 2026-10-16). ARM64 Windows is out of scope; the x64 build under emulation is acceptable for now.
- [ ] **Toolchain**: MSVC on the **pinned** Windows runner chosen in D4 (never `windows-latest`). whisper.cpp builds through its existing CMake path; libopus builds statically through the same `cc`/CMake route macOS uses, and no system DLLs are expected.
- [ ] **CPU baseline**: build whisper.cpp with AVX2 enabled and rely on its runtime dispatch for anything higher. If AVX2 is absent, show a startup diagnostic. The app must not crash silently.
- [ ] **Installer (in v1 — owner, 2026-10-16)**: a per-user install into `%LOCALAPPDATA%\Programs\Suramak` needing no admin rights — Start Menu shortcut, HKCU uninstall entry. The uninstaller leaves the user library, settings and models in place, with an opt-in "also remove my data" checkbox. Tooling (Inno Setup vs NSIS) is chosen in M14g.
- [ ] **Portable zip (still produced)**: `suramak-<version>-windows-x86_64.zip` with the GUI exe + CLI exe + a short `README.txt`, plus `SHA256SUMS` covering both artifacts.
- [ ] **Code signing — the free route** (owner, 2026-10-16): apply to the **SignPath Foundation** free Authenticode program for open-source projects (Suramak is GPL-3.0 OSS). If accepted, sign the exe + installer in CI from the first public build. Fallback if ineligible: ship unsigned with the SmartScreen "More info → Run anyway" steps documented in the release notes and `docs/windows.md`. Expect reputation warnings for early releases either way; document, never work around.
- [ ] **Models**: not bundled. The installer stays small; the existing model manager downloads on first run with sha256 pins and resume. Models live under `%LOCALAPPDATA%\Suramak\models`.
- [ ] **Updates**: none in-app (Sparkle stays macOS-only). The release page links to the new installer; running a newer installer over an existing install upgrades in place and preserves `%LOCALAPPDATA%\Suramak` data.
- [ ] **Deferred (own future decisions)**: MSIX/winget/Microsoft Store, and GPU acceleration (its own follow-up milestone per the non-goals).

## Testing

- [ ] **CI matrix**: `cargo test --workspace` runs on `macos-latest` and the pinned Windows runner. Excluded crates (`suramak-taps`, the SpeechTranscriber feature, Sparkle) are skipped by cfg. They must not be skipped by test filters.
- [ ] **Routing invariants**: the existing tests run unchanged on Windows. Add one Windows-specific case: SpeechTranscriber is unavailable, the job routes to whisper, and the named reason matches the macOS-on-old-OS reason string exactly.
- [ ] **Paths**: unit tests for `suramak-core` path resolution must cover:
  - non-ASCII usernames, such as `C:\Users\Dzikrī`;
  - paths with spaces;
  - paths over 260 characters, with the manifest declaring `longPathAware`;
  - a redirected `%LOCALAPPDATA%`.
- [ ] **sqlite**: open, migrate, and write the library on Windows. Confirm that WAL and `-shm` files behave correctly when the folder is under OneDrive sync, or document the folder as unsupported.
- [ ] **Decode/playback**: the golden-file decode tests (symphonia and libopus) produce identical PCM hashes on both platforms. For playback, manually smoke-test on these devices:
  - built-in speakers;
  - a USB headset;
  - a Bluetooth headset that switches into its hands-free profile.
- [ ] **Mic diagnostics**: verify three cases, each producing the mapped message in the UI and in `suramak-cli doctor`:
  - privacy setting "Let desktop apps access your microphone" turned off;
  - no input device present;
  - device in use exclusively by another app.
- [ ] **Manual smoke matrix**: Windows 11 23H2/24H2 (21H2 best-effort), on one AVX2 machine and one low-end laptop. On each, run the full flow: fresh install, first-run model download, transcription, export, reveal-in-folder, completion notification, taskbar progress, then upgrade-install and uninstall.
- [ ] **Scaling**: confirm the Deck layout and motion at 100%, 150%, and 200% display scaling, and when a window is dragged across monitors with different scaling. The manifest declares per-monitor DPI awareness (v2).
- [ ] **Keyboard**: walk every documented shortcut with Ctrl in place of Cmd. Confirm none of the remapped shortcuts collides with a system binding (Alt+F4, Win-key combos, Ctrl+Esc).

## Risks & mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| whisper.cpp performance on Windows is noticeably worse than on Apple Silicon, so jobs feel slow | High | Default to a smaller model on Windows. Show the estimated duration before starting a job. Treat GPU backends as a follow-up, not a blocker. |
| MSVC build breaks for whisper.cpp or libopus on toolchain updates | Medium | Pin the VS Build Tools version in CI. Cache build outputs. Run the `cargo check` gate on every PR so breakage surfaces early. |
| SmartScreen and antivirus false positives on a new-reputation binary | High | Sign via the free SignPath Foundation OSS program from the first public build (fallback: documented unsigned path); submit the binary to Microsoft for analysis; ship the portable zip so testers have a fallback. |
| Antivirus quarantines model downloads or `.part` resume files | Medium | Keep resume files under `%LOCALAPPDATA%\Suramak\models`. Verify the sha256 after every resume. Surface a named "file was removed during download" error rather than a generic failure. |
| OneDrive or roaming profiles corrupt the sqlite database through sync | Medium | Keep the library in `%LOCALAPPDATA%`, which does not roam. Detect a library folder that sits inside an OneDrive-synced path and warn the user. |
| A Bluetooth hands-free profile drops capture to 8/16 kHz mono and quality degrades silently | Medium | The existing resampler already handles the rate change. Add a capture-quality diagnostic that names the device format. |
| `#[cfg(windows)]` creep into shared crates erodes the seam | Medium | Enforce the seam rules above in review. Add a CI grep that fails if `cfg(windows)` appears in the engine, routing, model manager, or exports crates. |
| Notification APIs require an AppUserModelID; the portable zip has no shortcut registration | High | Register the per-user AUMID (HKCU) on first toast; if registration or toast creation fails, degrade to `FlashWindowEx` and log the reason (never fatal, never silent). |

## Decided (owner, 2026-10-16)

1. **Minimum OS: Windows 11 only.** The Win10 row is dropped; testing exercises 23H2/24H2 (21H2 best-effort).
2. **Code signing: the free certificate route** — attempt SignPath Foundation (free Authenticode for open-source); if ineligible, ship unsigned with documented SmartScreen steps. No paid certificate this milestone.
3. **Distribution: GitHub Releases** — the installer + portable zip attached to each release. Store/winget stay deferred.
4. **GPU acceleration: yes — as its own follow-up milestone** (Vulkan first, CUDA later), not in v1.
5. **Default model: a smaller Windows default is accepted** — with the model selectable in Settings for higher quality; Indonesian users are steered to the quality tier with a clear note.

## Suggested first slice to dispatch

**`suramak-core` Windows paths + `cargo check --target x86_64-pc-windows-msvc` CI gate (with `suramak-taps`, the SpeechTranscriber feature, and Sparkle cfg-excluded).** Every later item depends on the workspace compiling on Windows. Landing this gate first turns each later item into a small, independently reviewable change instead of one large port.