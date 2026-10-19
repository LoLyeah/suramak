# M13 — Suramak for Android: Milestone Plan

Status: draft for owner review. No workorder is cut until approved.
Drafted by **claude-opus-5.5** (via OpenRouter) at the owner's request, 2026-10-16; grounded and reviewed by Hermes against the live repo. Items marked ASSUMPTION are unverified at planning time.
Scope baseline: Suramak v0.3.0 (desktop), with M13 following the currently queued work.

---

## 1. Summary

M13 ships a sideloadable, signed **arm64 Android APK** of Suramak. It reuses the existing Rust engine (`suramak-core` and `suramak-asr`) unchanged in behaviour. It adds:

- a touch-first rendition of the Deck UI;
- the Android platform glue that a long-running, on-device transcription app needs:
  - mic capture;
  - SAF import and share-to-import;
  - a foreground service;
  - scoped storage;
  - resumable, checksummed model downloads.

Distribution is **GitHub Releases only for now** (owner, 2026-10-16); any other channel is a later decision.

Guiding decisions:

1. **UI stays egui/wgpu, in Rust.** One codebase and one design language. A deliberately thin Kotlin shim covers only what Android forbids native code from owning: the `Service` class, intents, permission dialogs, insets and back handling.
2. **whisper.cpp is the only engine in v1.** It runs CPU-only on arm64 with pinned ISA flags. Parakeet/sherpa-onnx is a No-Go for v1.
3. **The engine crates do not fork.** Android gets `cfg(target_os = "android")` backends for paths and secrets only. Routing, pipeline, worker and decode code are byte-identical across targets.
4. **There is a de-risking gate.** M13b is a hard Go/No-Go spike on egui text input and lifecycle on a real device. If it fails, the documented fallback is option C (§2.1).
5. **Motion is first-class and materials degrade honestly.** Android gets the Linux frosted *treatment*, not fake glass. Real Android window blur (API 31+) is a post-v1 investigation.

Non-goals for v1:

- Liquid Glass, SpeechTranscriber, any self-updater.
- Parakeet, GPU (Vulkan) whisper.
- 32-bit ABIs, tablets as a tuned form factor, Play Store listing.

---

## 2. Decisions

### 2.1 UI stack — **Decision: (A) egui/wgpu on Android via winit + android-activity (GameActivity backend), with a thin Kotlin platform shim**

This is technically "A with the JVM glue that every Android app needs." It is not option C, because no UI is drawn by Kotlin.

**Why not (B), a Kotlin/Compose shell over JNI:**

- It reimplements the entire Deck visual language and motion system in a second toolkit. Every future design milestone would then be done twice, and the "motion-first in every milestone" rule doubles in cost forever.
- It moves the product's centre of gravity off "fully native Rust." Compose is the better Android citizen, but Suramak's identity is the coherent Deck UI.
- The owner is a single non-specialist reviewer. Two divergent UIs mean two sets of bugs to find on real hardware.

**Why not (C), a hybrid (Compose for chrome, egui for editor/transcript):**

- It carries the cost of both stacks, adds a seam where egui and Compose motion must match frame-for-frame, and needs two surfaces composited.
- It remains the **fallback only** if M13b proves egui text editing on Android unshippable. In that case Compose would own text-entry surfaces: editor and settings fields.

**Why (A) is viable:**

- winit's Android backend runs on `android-activity`. GameActivity, unlike NativeActivity, brings GameTextInput, which is the realistic path to soft-keyboard editing.
- wgpu targets Vulkan, with a GLES fallback.
- egui already handles touch events.
- Desktop screenshot harnesses can render the *same* widget tree at phone viewports on Linux. This is the main reason the UI-verification story (§2.3) is tractable.

**The pinned winit fork:** the fork carries a macOS IME guard fix and is based on upstream. Android code paths in winit are upstream code the fork does not touch.

- **Decision:** Android uses the **same fork revision** as desktop. One `[patch]` entry and one `winit` in `Cargo.lock`. The macOS fix is inert on Android because it is cfg-gated by nature of living in the macOS backend.
- **ASSUMPTION:** the fork's base version exposes the `android-game-activity` feature (winit 0.29+ does). M13a verifies this.
- If it does not, we rebase the fork forward before M13b. We do not maintain a second winit for Android.

**What is reused vs reimplemented:**

| Layer | Fate |
|---|---|
| `suramak-core` (model, settings, sqlite library) | Reused as-is. Android path provider and secret-store backend added behind `cfg`. |
| `suramak-asr` (engines, pipeline, worker, capture) | Reused as-is. cpal capture runs on Android's AAudio backend. |
| `suramak-playback` (cpal + symphonia) | Reused as-is. |
| `suramak-ui` Deck components, theme tokens, motion system | Reused. Refactored so components are a library consumed by two shells (desktop binary, Android cdylib). |
| Deck *layout* | Reimplemented as a **compact/touch profile**: single-pane navigation stack, ≥48 dp hit targets, bottom action bar, sheet-based modals, drag/fling scroll. |
| Desktop shell (windowing, menus, single-instance `flock`, Sparkle) | Not used on Android. |
| `suramak-glass`, `suramak-taps` | Not compiled for Android. |
| Kotlin shim | New: ~6 small classes, no UI. |

**ASSUMPTION:** `suramak-ui` currently mixes Deck components and desktop shell concerns in one crate. Slice M13g's first task is a split. Whether it lands as modules or as a `suramak-deck` sub-crate is decided by what the code looks like.

### 2.2 ASR engines for v1

- **whisper.cpp via whisper-rs on the NDK: Go.**
  - CPU only.
  - ISA flags are pinned explicitly for arm64 (NEON + dotprod; i8mm opt-in after device testing). There is never `-march=native` or host-detected flags.
  - Thread count defaults to performance-core count, capped at 4, and is user-adjustable.
  - Whisper's Vulkan backend is excluded: driver variance across Android GPUs is too wide for v1.
- **Model guidance changes on phones.** The model manager shows device-appropriate recommendations: `base` and `small` quantized as defaults, with `large-v3-turbo` quantized listed as "slow, high quality."
  - Indonesian users are steered away from `tiny`.
  - **Supported floor: arm64 with 4 GB RAM** (owner, 2026-10-16). Below the floor the larger models are refused with a clear message; 4–6 GB devices default to the smaller pinned model.
  - Pins and sha256 entries reuse the existing manifest.
- **sherpa-onnx / Parakeet: No-Go for v1.**
  - Dylib bundling is not yet solved even on desktop.
  - It adds ONNX Runtime `.so` weight to the APK.
  - By the routing invariant it can only ever serve covered languages under Auto, never Indonesian. So the user-visible gain on a phone is "faster English," at a large packaging and size cost.
  - The cargo feature stays default-off, and the router code path is compiled identically, so the invariant tests still run on the Android target. Revisit as a post-v1 slice, once desktop dylib bundling exists.
- **Android's on-device `SpeechRecognizer`: rejected.** Vendor-dependent behaviour and opaque privacy semantics conflict with "everything on-device, verifiably."

### 2.3 UI-verification strategy

The desktop Xvfb suites (25 + 18 checks) cannot run against an APK. The strategy has three tiers:

1. **Phone-viewport goldens on Linux (gating).** A new screenshot suite drives the Deck *compact profile* through the existing headless harness at fixed phone viewports. Proposed viewports are 360×800 dp and 412×915 dp at scale 2.625, plus a soft-keyboard-open variant with a simulated bottom inset. This tier:
   - catches layout, hit-target and inset regressions on every PR on `ubuntu`;
   - is possible only because of decision 2.1.
2. **Emulator smoke (gating once stable, informative before then).**
   - An x86_64 API 34 emulator on the ubuntu runner (KVM) installs a debug APK.
   - It imports a fixture via an instrumented intent, transcribes it with the `tiny` model (cached), and asserts the sqlite row and segment count via `adb`.
   - It captures `screencap` PNGs as artifacts.
   - It does not compare pixels: emulator GPU paths (SwiftShader/ANGLE) are not representative of devices.
3. **Owner-device lane (release gating).** A scripted checklist (`docs/android-device-checklist.md`) on the owner's phone. It includes a screen recording of the motion sequences, because motion cannot be judged from stills.

---

## 3. Slice breakdown

Each slice is one workorder. The ordering front-loads the two existential risks: the engine on the NDK, and egui text and lifecycle on a device.

### M13a — Engine on the NDK (CI-only, no UI)

- **Goal:** prove the Rust core, whisper.cpp and libopus cross-compile for `aarch64-linux-android` and `x86_64-linux-android`, and transcribe correctly.
- **Deliverables:**
  - `crates/suramak-android/` as a cdylib (`lib suramak_android`) with a headless entry point that transcribes a bundled 10 s en + 10 s id fixture with `tiny` and logs segments.
  - `scripts/android/build.sh` (cargo-ndk wrapper).
  - `cfg(target_os = "android")` stubs in `suramak-core` for the path provider.
  - A CI job `android-build` running `cargo ndk check`, then `cargo ndk build` for both ABIs.
  - A pinned NDK version in `rust-toolchain`-adjacent config.
- **Verified by:**
  - CI green.
  - An emulator run on CI (x86_64) printing expected segments.
  - Owner side-loads a debug build (via `adb`, documented step by step) and reports real-time factor from logcat.
- **Risks:**
  - whisper-rs build-script assumptions about cmake and the host.
  - libopus cross-compile.
  - TLS stack choice (see M13d).
  - **ASSUMPTION:** the cpal and symphonia versions in the workspace build for Android without patching.

### M13b — Shell spike and Go/No-Go gate

- **Goal:** egui/wgpu running in GameActivity on the owner's phone, exercising the four things most likely to kill option A.
- **Deliverables:**
  - The `android/` Gradle project (wrapper, AGP, GameActivity AAR pinned to match the `android-activity` crate's expected version).
  - A minimal Kotlin `MainActivity` extending GameActivity.
  - An egui test screen with:
    - (1) a multiline `TextEdit` driven by the soft keyboard, including Indonesian text, autocorrect on, and backspace across words;
    - (2) surface teardown and recreate on background/foreground with state preserved;
    - (3) a 60 fps Deck motion sample (one existing transition);
    - (4) system-bar and cut-out insets plus the back gesture, delivered from the shim over JNI.
- **Verified by:** owner-device checklist with video. Written Go/No-Go recorded in `docs/m13-gate.md`.
- **Risks:**
  - Soft-keyboard/IME quality is the top risk.
  - GameActivity ↔ `android-activity` version lockstep.
  - **No-Go path:** switch the editor and settings text fields to option C (Compose overlays). Everything else in this plan is unchanged.

### M13c — Platform shim and bridge

- **Goal:** a stable, small JNI contract between Rust and Kotlin.
- **Deliverables:**
  - `android/app/src/main/java/app/suramak/` containing:
    - `MainActivity`;
    - `PlatformBridge` (JNI entry points);
    - `JobService` (foreground service, skeleton);
    - `Permissions`;
    - `Insets`;
    - `SecretStore` (Android Keystore-wrapped AES-GCM).
  - `crates/suramak-android/src/bridge/` using the `jni` crate. A single `PlatformEvent` enum flows Kotlin→Rust and a `PlatformRequest` enum flows Rust→Kotlin, so the JNI surface stays a handful of functions rather than one per feature.
  - A `suramak-core` secret-store backend for Android.
    - **ASSUMPTION:** the keychain module is behind a trait or can be put behind one with a mechanical change.
  - App-private storage path provider (§5.5).
- **Verified by:**
  - Host unit tests of the event/request serde.
  - Emulator smoke asserting a round-trip ping and that paths resolve.
  - Owner checks the secret survives an app restart.
- **Risks:** JNI lifetime and threading bugs. Mitigation: all JVM calls are marshalled onto one attached thread owned by the bridge.

### M13d — Models on Android

- **Goal:** the existing checksum/resume model manager works on a phone, inside a foreground service.
- **Deliverables:**
  - Download, verify and resume logic extracted into a library module consumable by both `suramak-cli` and the app.
    - **ASSUMPTION:** this logic currently lives in `suramak-cli`. If it is already a library, this step is a no-op.
  - The HTTP stack is confirmed or switched to rustls on Android, so OpenSSL is never cross-built.
  - A model screen in the compact profile.
  - Downloads run under `JobService` (type `dataSync`) with a progress notification.
  - A free-space check runs before starting.
  - Resume survives app kill.
- **Verified by:**
  - Host tests of resume and checksum (existing, now exercised on the library crate).
  - Emulator smoke that downloads `tiny`, kills the app mid-download, and resumes to a matching sha256.
  - Owner downloads `small` over mobile data and Wi-Fi.
- **Risks:** Android 15 6-hour `dataSync` limit (irrelevant for model sizes, but `onTimeout` is handled anyway); captive portals and metered networks.

### M13e — Import and background transcription

- **Goal:** the core loop. Import a file (SAF or share), transcribe it with the screen off, get a notification, and see it in the library.
- **Deliverables:**
  - SAF `ACTION_OPEN_DOCUMENT` import.
  - `ACTION_SEND` / `SEND_MULTIPLE` intent filters.
  - fd-based copy into app storage with progress.
  - `JobService` hosting transcription jobs (type `mediaProcessing` on API 35+, `dataSync` on 34, untyped below), with a partial wake lock, progress notification, cancel action, and `onTimeout` handling.
  - Interrupted-job recovery on launch.
- **Verified by:**
  - Emulator smoke via share intent with a fixture.
  - Owner checklist: 60-minute podcast import, screen off, app swiped away, and a completion notification tap that opens the transcript.
- **Risks:** OEM battery killers (Xiaomi, Samsung and similar); thermal throttling on long jobs.

### M13f — Mic recording

- **Goal:** record 16 kHz mono PCM16 WAV identical to desktop output, including with the screen locked.
- **Deliverables:**
  - `RECORD_AUDIO` runtime flow.
  - `microphone`-typed foreground service started from the foreground.
  - cpal AAudio capture through the existing `suramak-asr` capture path.
  - Interruption handling: incoming call, audio focus loss, Bluetooth headset switch.
  - Record → transcribe hand-off.
- **Verified by:**
  - Host tests unchanged.
  - Owner checklist: 30-minute locked-screen recording and a call interruption mid-record. The WAV is checked with `suramak-cli doctor` (or an equivalent probe) on the desktop.
- **Risks:**
  - Device-native rate (48 kHz) resampling.
    - **ASSUMPTION:** the capture path already resamples, as desktop mics are rarely 16 kHz native.
  - The Android 14+ while-in-use restriction on microphone foreground services.

### M13g — Touch Deck

- **Goal:** the real product UI in compact profile. Library, transcript editor, playback-synced segments, exports, settings, and polish (§9 — retained).
- **Deliverables:**
  - The `suramak-ui` components/shell split.
  - Compact layout set.
  - Motion pass covering: navigation push/pop, sheet present/dismiss, list insert/delete, progress, and predictive-back-compatible pop.
  - Fling scrolling: verify egui's touch momentum, and implement it in the Deck motion layer if it is insufficient.
  - Frosted treatment reused from Linux.
  - Export via `ACTION_CREATE_DOCUMENT` for md, txt, json, srt and vtt.
  - **Quit action** (owner-requested, 2026-10-16): stops any running job with the standard cancel semantics, stops the foreground service, and exits the app — reachable from Settings.
  - The phone-viewport screenshot suite (`tests/ui-compact/`) added to CI as gating.
- **Verified by:** new compact goldens; existing desktop suites stay green (proves the split did not regress desktop); owner motion video review.
- **Risks:** desktop regressions from the split; accessibility (§8).

### M13h — Packaging, release and docs

- **Goal:** a tagged release produces a signed APK alongside the mac and linux artifacts.
- **Deliverables:**
  - Release signing config reading a keystore from repo secrets. This is a one-time owner step, documented like the Sparkle key.
  - A `release.yml` android job.
  - Artifact `suramak-<version>-android-arm64-v8a.apk` plus a `.sha256`.
  - The Gradle `versionCode` derived from semver.
  - `docs/android.md` (install, sideload, permissions explained).
  - F-Droid metadata draft (`fastlane/metadata/android/`).
  - CHANGELOG entry.
- **Verified by:** a tag dry-run on a pre-release tag, and the owner installing from the GitHub Release page with no `adb`.
- **Risks:** keystore loss is unrecoverable for update continuity, so the keystore needs a documented backup.

---

## 4. Crate & build layout

```
crates/
  suramak-core/        # + cfg(android): paths, secret backend
  suramak-asr/         # untouched in behaviour
  suramak-playback/    # untouched
  suramak-ui/          # split: deck components (shared) / desktop shell
  suramak-android/     # NEW cdylib: android_main, bridge/, compact shell
  suramak-glass/ suramak-taps/   # cfg(macos), excluded on android
android/               # NEW Gradle project
  gradlew, gradle/wrapper/
  settings.gradle.kts, build.gradle.kts
  app/
    build.gradle.kts   # minSdk/targetSdk, signing, jniLibs from cargo-ndk
    src/main/AndroidManifest.xml
    src/main/java/app/suramak/*.kt
    src/main/jniLibs/  # generated, gitignored
scripts/android/build.sh   # cargo ndk -t arm64-v8a [-t x86_64] -o android/app/src/main/jniLibs
```

**Build flow.**

- `scripts/android/build.sh` is the single source of truth.
- Gradle calls it via one `preBuild` dependency task, so `./gradlew assembleRelease` is a one-liner for CI and for future F-Droid recipes.
- Rust is built by cargo, never by a Gradle Rust plugin, to keep the Rust build reproducible outside Gradle.

**ABIs.**

- `arm64-v8a` is the only release ABI.
- `x86_64` is debug-only, for the CI emulator.
- `armeabi-v7a` is excluded: whisper on 32-bit phones is impractically slow, and it would double C build time.

**SDK levels.**

- **minSdk 29 (Android 10).** Rationale:
  - scoped storage is the baseline, so there is one storage model;
  - AAudio is mature;
  - Vulkan is broadly available with GLES fallback in wgpu;
  - pre-29 devices are unlikely to run `small` acceptably anyway.
- **targetSdk** is the latest stable at dispatch time, at least 35. This is needed to declare the `mediaProcessing` foreground-service type.
- **compileSdk** equals targetSdk.

**NDK.**

- The NDK version is pinned (exact revision recorded in `docs/android.md` and CI).
- `cargo-ndk` sets `CC`, `CXX`, `AR` and `CMAKE_TOOLCHAIN_FILE`.
- whisper.cpp's cmake is given explicit `GGML_NATIVE=OFF` plus fixed arm64 feature flags.
- libopus builds with the NDK clang through its existing build script.
- `c++_shared` vs `c++_static`: static. This avoids shipping `libc++_shared.so` and conflicts if sherpa-onnx is later added. It is revisited only then.
- 16 KB page-size alignment for `.so` files is enabled at link time, since newer Android devices require it.

**Keeping the engine crates untouched.**

- Allowed changes in `suramak-core` / `suramak-asr` are limited to `cfg(target_os = "android")` items for paths and secrets.
- Any behavioural change is out of scope and must go through a separate workorder.
- A CI step runs the existing host unit tests plus `cargo ndk check -p suramak-core -p suramak-asr`.
- The router invariant tests (Indonesian → whisper; fast engines only under Auto + covered languages; failure → whisper with a named reason) are run for the android target triple via `cargo test --no-run` cross-compiled, and executed on the emulator in M13a.

---

## 5. Platform glue detail

### 5.1 Mic capture

- cpal's Android backend (AAudio) is used through the existing `suramak-asr` capture code.
  - **ASSUMPTION:** the workspace cpal version's Android backend builds with the NDK as-is. Pinning or upgrading is decided in M13a.
- `RECORD_AUDIO` is requested at the moment of first record tap, with a rationale sheet before the system dialog.
- The microphone foreground service (`FOREGROUND_SERVICE_MICROPHONE` on API 34+) must be started while the activity is visible. The record button starts it synchronously before opening the stream.
- Audio focus loss and route changes stop or pause the stream, flush the WAV header, and surface a named reason in the UI.
- The output is the same 16 kHz mono PCM16 WAV as desktop.

### 5.2 SAF import

- `ACTION_OPEN_DOCUMENT` with MIME types `audio/*`, `video/*` (for m4a/mkv containers) and `application/ogg`.
- Kotlin opens the URI via `ContentResolver.openFileDescriptor("r")`, detaches the fd, and passes it to Rust. Rust copies the bytes into `files/media/<uuid>.<ext>` with progress, then imports that path through the normal pipeline.
- **Decision: copy, don't reference.** SAF grants can be revoked and source files moved, and the library must survive both.
  - **ASSUMPTION:** this matches or is compatible with how the desktop library holds source audio. If desktop references originals, Android's copy is a platform-specific deviation and is documented.
- A free-space check runs before the copy.

### 5.3 Share-to-import

- Intent filters on `MainActivity` for `ACTION_SEND` and `ACTION_SEND_MULTIPLE` with the same MIME set.
- `launchMode="singleTask"` replaces the desktop `flock` single-instance guard. Shares arriving while the app runs are delivered via `onNewIntent` → `PlatformEvent::Import(fds)`.
- The share flow also covers voice-recorder and messaging apps (WhatsApp voice notes are Opus/ogg, already handled by bundled libopus).

### 5.4 Foreground service and job lifecycle

- **One `JobService`** hosts all long work: downloads, transcription, recording.
- It switches `foregroundServiceType` per job:
  - `microphone` while recording;
  - `mediaProcessing` (API 35+) or `dataSync` (API 34) for transcription;
  - `dataSync` for downloads.
- Manifest declares the matching `FOREGROUND_SERVICE_*` permissions.
- **Process vs activity.** The Rust worker and app state live in a process-level singleton inside `suramak-android`, not in the window.
  - On `Suspended` the wgpu surface is dropped. On `Resumed` it is recreated and egui redraws from preserved state.
  - The service exists to keep the *process* at foreground priority. It does not run the work itself, so there is no IPC: it is the same process with a Rust worker thread.
- **Wake lock.** A `PARTIAL_WAKE_LOCK` is held for the job's duration with a safety timeout. A foreground service alone does not keep the CPU awake with the screen off.
- **Notifications.**
  - Channels: "Jobs" (low importance, progress, cancel action) and "Completed" (default importance).
  - `POST_NOTIFICATIONS` (API 33+) is requested on the first job, not at launch.
  - If denied, jobs still run and the in-app state remains the source of truth.
  - Progress updates are throttled to about 1 Hz.
- **Timeouts.** `Service.onTimeout` (Android 15) cancels the job cleanly and records the named reason "system time limit reached."
- **Process death.** Jobs are written to sqlite as `running`. On launch, any `running` job without a live worker is marked `interrupted` with a retry action.
  - **ASSUMPTION:** segments are persisted only at job end today. Mid-job checkpointing is not in v1.

### 5.5 Storage layout

| Data | Location | Backup |
|---|---|---|
| Models (multi-hundred MB) | `noBackupFilesDir/models/` | excluded |
| Imported media, recordings | `filesDir/media/`, `filesDir/recordings/` | excluded (size) |
| sqlite library | `filesDir/library/suramak.sqlite` | **included** (owner, 2026-10-19) |
| Settings | alongside the db, via `suramak-core` | **included** (owner, 2026-10-19) |
| Polish key | Keystore-wrapped blob in `noBackupFilesDir/secrets/` | never (Keystore keys don't migrate) |

- No external or shared storage permissions are requested.
- Exports go out through `ACTION_CREATE_DOCUMENT`; the user chooses the destination.
- Cache directory is used only for transient decode scratch.

### 5.6 Model downloads reuse

- The same manifest, pins and sha256 verification as the CLI.
- Partial files live in `models/.partial/`, using the same resume semantics.
- Downloads use Wi-Fi by default, with a "use mobile data" toggle that shows the size up front.
- Network use stays confined to model downloads and opt-in polish. No analytics and no update pings.

---

## 6. CI & packaging

**Buildable and testable on the ubuntu runner:**

- `cargo ndk check` and build for arm64 and x86_64 (NDK via `setup-android` / SDK manager, cached).
- Host unit tests as today.
- The Gradle `assembleDebug` and `assembleRelease` builds (release signed only on tag).
- Phone-viewport egui goldens under Xvfb, from M13g.
- x86_64 emulator smoke via an emulator-runner action on API 34 with KVM enabled. It installs the debug APK, launches the main activity, imports a bundled 10-second clip, transcribes it with the smallest pinned model, and asserts that a transcript row lands in the library.

**Not reliably testable on the runner (manual or device-lab only):**

- arm64 performance and thermals. The emulator is x86_64, and its timings say nothing about real devices.
- Microphone capture on real hardware, including Bluetooth headsets and OEM audio quirks.
- Foreground-service survival under OEM battery managers (Samsung, Xiaomi).
- Keystore behaviour across device lock and unlock, and after a biometric enrolment change.

**Packaging:**

- Release artifacts: an **arm64-v8a APK** attached to GitHub releases for sideloading. An AAB (arm64-v8a + x86_64) is produced only if/when a Play listing happens; x86_64 stays debug-only for CI.
- Native libs are stripped in release, and debug symbols are uploaded as a CI artifact (not published).
- Release signing uses a keystore held in repository secrets and is only unlocked on tag workflows. PR builds are debug-signed.
- `minSdk` 29 and `targetSdk` (≥ 35) follow §4's SDK decision. The values are recorded in `android/gradle.properties`, so CI and local builds agree.
- APK size budget: under 25 MB excluding models. CI fails the build if it is exceeded.

**Cache strategy:**

- Cargo registry and target dir cache are keyed on `Cargo.lock` plus the NDK version.
- The Gradle cache uses the standard setup action.
- The emulator system image and AVD snapshot are cached so the smoke job stays under ~10 minutes.

---

## 7. Testing

| Layer | What | Where | From |
|---|---|---|---|
| `suramak-core` unit | Library, settings, manifest parsing, sha256 resume, export formats | Host, as today | existing |
| JNI boundary | Round-trip of every exported fn with fixture inputs. Error mapping to Kotlin exceptions. No panics across FFI (`catch_unwind` audited) | Host via `jni` test harness, plus emulator instrumented tests | M13b |
| Audio decode | Fixture corpus (m4a, opus, mp3, wav, 3gp from stock recorder apps) decoded to identical PCM hashes on host and emulator | Host plus emulator | M13c |
| Transcription | Tiny-model golden transcripts on 3 short clips. A word-error threshold is used rather than exact match | Host plus emulator smoke | M13d |
| UI | Phone-viewport egui goldens (portrait, landscape, large font scale, dark mode) | Xvfb | M13g |
| Lifecycle | Rotate, background, process-death and restore during transcription. The job resumes or fails cleanly, with no orphaned partial rows | Emulator instrumented | M13e |
| Storage | Uninstall/reinstall leaves nothing behind. Models are never in backup. A Keystore-lost blob is detected and the user is re-prompted for the key | Emulator plus manual | M13f |
| Manual device pass | 1 low-end arm64 (4 GB RAM), 1 mid-range, 1 Samsung. Checklist covers mic, long recording, thermal throttle and a 2-hour file | Real devices | each release |

- Every fixture is checked into `tests/fixtures/android/` with a provenance note. No copyrighted audio.
- Golden updates require an explicit `--bless` and a reviewer ack in the PR.

---

## 8. Risks & mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Inference too slow or too hot on mid-range arm64 | High | High | Default to the smaller pinned model on 4–6 GB devices (4 GB arm64 is the supported floor — §9). Expose chunked progress. Measure early with the M13d bench on a real device before UI work commits to the UX |
| OEM battery managers kill the foreground service | Medium | High | Persist chunk progress so a killed job resumes from the last completed chunk. Surface a one-time "battery optimisation" hint linking to system settings. Do not request `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` in v1 |
| egui touch/IME ergonomics fall short of user expectations | Medium | Medium | Keep text editing minimal in v1 (rename, search). Phone goldens plus a manual IME pass on Gboard and Samsung Keyboard. Native Kotlin fallback is kept only for the file picker and share sheet |
| NDK / `cargo-ndk` / Gradle version drift breaks CI | Medium | Medium | Pin NDK, AGP and Gradle versions. Renovate PRs for them run the full emulator smoke before merge |
| Keystore key loss (biometric change, OEM bugs) silently breaks polish | Medium | Low | Detect unwrap failure, clear the blob and re-prompt for the key. Never fall back to plaintext |
| Model download over mobile data surprises users | Low | Medium | Wi-Fi default, size shown before any mobile-data download, and resume on reconnect |
| JNI panics crash the app | Low | High | `catch_unwind` at every exported fn, mapped to a typed Kotlin exception. Clippy lint plus a grep check in CI for un-wrapped exports |
| APK or AAB size creep from native deps | Medium | Low | Size budget gate in CI. `cargo bloat` report attached to release builds |

---

## 9. Decided (owner, 2026-10-16)

1. **Auto Backup: include the sqlite library.** Transcripts survive a phone migration; the privacy copy states that the library rides Android Auto Backup (Google servers). v1 keeps the default on; the documentation names how to exclude it. Categories (owner, 2026-10-19): **library + settings in; recordings/imported media, models, secrets out.**
2. **Distribution for v1: GitHub Releases only** (sideloaded APK). F-Droid/Play stay later decisions; the M13h work is scoped to the GitHub path.
3. **Minimum supported device: arm64 with 4 GB RAM.** That is the floor — larger models are refused below it with a clear message (4–6 GB devices default small).
4. **Background transcription: yes, via the foreground service + persistent notification — with a Quit action.** The app exposes an explicit quit/stop affordance (Settings) that cancels running work with the standard semantics, stops the service, and exits.
5. **Polish on mobile: yes, opt-in.** The toggle ships in v1; the recording-upload path stays behind its own explicit opt-in and the existing privacy copy.

---

## 10. Suggested first slice to dispatch

**M13a + M13b: `cargo ndk` build of `suramak-core` for arm64/x86_64, plus a minimal JNI bridge that loads the library and returns the core version string to an empty Kotlin activity, with the CI build and emulator launch smoke green.** This proves the toolchain, FFI boundary and CI pipeline end to end before any UI or inference work depends on them.

---

## Addendum — competitive-research refinements (2026-10-19, owner-directed)

Derived from the verified competitive research; full context in `docs/PLAN-M15-M18.md`. These refine the approved slices without adding a second Android UI stack.

- **JNI bridge:** handle shared content URIs through supported descriptor/stream access — temporary grants, unreadable content, cancellation; never assume an ordinary filesystem path.
- **Import + FGS jobs:** add Ogg/Opus share fixtures and reuse the desktop decode path (M10C native Opus via bundled libopus); jobs stay tied to the foreground-service lifecycle with visible notifications; sharing a file must never request microphone permission.
- **Mic:** verify recording across screen-off / app-switch / service-stop / device-interruption paths; foreground services reduce — not eliminate — termination risk.
- **Touch Deck (incl. Quit):** the share-result presentation is a compact Deck surface in egui (progress, cancel, transcript, copy, return); a bottom-sheet-like layout is optional, not a Kotlin UI rewrite. Quit semantics cover active jobs.
- **Packaging:** exercise share-to-import on a clean signed arm64 APK installed from GitHub.
- **Claim hygiene:** the research's "under three seconds" is removed as a universal promise — record latency by clip duration, device, model, and cold/warm state. Auto Backup stays ON as decided, with contents disclosed.