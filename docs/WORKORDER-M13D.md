# WORKORDER — M13D: models on Android (the phone first-run requires it)

Plan source: `docs/PLAN-M13-ANDROID.md` §3 M13d (size M). Tree: `main` v0.4.0
post (`7037c9f`). **Context**: the owner approved an Android APP release
("I'll test it on release") — the spike phase is over; from here the REAL app
is built slice by slice. M13c's JVM-bridge scaffolding (MainActivity/
PlatformBridge/Insets/JobService skeleton) already exists from the spike
(`crates/suramak-android/` cdylib + `android/` Gradle project, debug-signed).
This slice does NOT assume the compact UI yet — the CLI's model flows must be
reusable as a library first.

## Deliverables

1. **Extract the model download/verify/resume logic** from
   `suramak-cli` into `suramak-core::models` (if it already lives there, this
   is a no-op — verify first and say so in the report; do not duplicate code).
   The CLI keeps its command surface; the app consumes the library API.
2. **HTTP stack on Android = rustls** (no cross-built OpenSSL): confirm what
   the models download path already uses; if reqwest/hyper fields a native-tls
   dependency anywhere in the Android build graph, switch the feature set to
   rustls (documented; source verified by the CI `cargo ndk check`).
3. **Model recommendations for phones** (the plan's guidance change):
   `tiny` for quick tests, `base`/`small` quantized as the on-device defaults
   with honest language — and **Indonesian users are steered AWAY from `tiny`**
   (quality): the landing UI text + a store-provided manifest field
   (additive schema where the store model registry lives; no behavior change on
   desktop unless the manifest gains a column — additive only).
4. **Device floor** (owner, 2026-10-16): arm64 + 4 GB RAM is the minimum;
   below the floor larger models are refused with a clear message; 4–6 GB
   devices default to the smaller pinned model; ≥8 GB may choose `small`.
   Memory probe = `ActivityManager.getMemoryInfo` over the existing bridge.
5. **Free-space check before download** (the models dir's FileSystem stats
   available through the bridge/paths provider) — refuses with a clear message,
   never half-starts.
6. **JobService-typed `dataSync`** hosting a download with a progress
   notification (deterministic progress from the existing callback), **cancel
   action**, and `onTimeout` → clean stop with a resume-on-next-open. The
   skeleton JobService from M13c becomes the real thing.
7. **Resume survives an app kill**: the existing partial-file + checksum
   resume must survive a hard process death (the service's own process is the
   same process — kill mid-download → next launch resumes at the bookmark).
   Host tests already cover resume math; the app-side tests run as unit tests
   on the host (no emulator in this workorder).

## Verification

- `cargo ndk -t arm64-v8a --platform 29 check` + a debug `build-apk.sh` run
  green (the artifact is NOT the release APK; just a build gate).
- CI stays green (the android-build + android-spike jobs — do not break the
  spike; the spike remains installed on the owner's phone until the real app
  replaces it).
- Host unit tests for: the recommendation mapping (4/6/8 GB tiers × language,
  EN + ID), free-space refusal, resume-after-kill semantics (simulated at the
  library layer), rustls-only graph on the android target
  (`cargo ndk check -- --locked` + a feature-tree assertion if practical).
- Kotlin compiles warning-free; manifest gains the `dataSync` type + the
  notification channel (existing handling may cover it — verify, don't dupe).
- fmt/clippy/workspace tests green (baseline 807→grow; ONE full workspace run
  at the end; crate-scoped tests meanwhile; disk: clean if < 10 GB).

## Do NOT

- No compact UI (M13g), no import/share-target (M13e), no mic (M13f);
  sign/release (M13h) is separate. No new dependencies except rustls-feature
  flips of existing ones; license review if ANY binary artifact changes;
  no destructive migrations; no desktop behavior change beyond the additive
  manifest fields. Tests keep every M15 suite green.
- No git commit/push; no sudo. Report: `docs/reports/M13d-models-android.md`.
