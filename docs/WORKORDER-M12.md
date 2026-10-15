# Workorder M12 — in-app updates (Sparkle 2)

**Date**: 2026-10-16
**Author**: orchestrator (Hermes)
**Owner-requested** (2026-10-15): "the app had an update system … auto check for updates and can install itself (with user confirmation of course)."
**Refs**: PRODUCT.md (M12 bullet), sparkle-project.org documentation (Publishing / Security & Reliability / Programmatic setup), the Rust `sparkle-updater` crate (0.1.0, objc2 bindings for Sparkle 2.9.6).

## Goal

The macOS app **checks for updates in the background** and installs them **in place, with explicit user confirmation** (Sparkle's standard dialogs): auto-check default ON (toggle in Settings), a "Check for Updates…" menu item, release notes rendered from our CHANGELOG, EdDSA-signed appcast hosted on our own GitHub Releases. Windows/Linux updaters are later platform work (out of scope).

## Scope (what is buildable + verifiable from Linux + CI)

1. **Rust integration** (macOS-only, behind a new default-OFF cargo feature `updates` propagated to `suramak-ui`):
   - Spike first, then implement ONE of: the `sparkle-updater` crate vs a minimal own objc2 binding to `SPUStandardUpdaterController` (the crate's README/API is the reference for the subset needed: start updater, event callbacks, gentle reminders, relaunch hook). Prefer the crate unless it blocks; document the choice.
   - Start the updater on the main thread at boot (after the AppKit window is up); wire the event callbacks into our logging; **no new UI beyond**: one Settings row ("Automatic updates", default ON) and a "Check for Updates…" app-menu item per Sparkle's programmatic-setup docs.
   - `packaging/Info.plist`: `SUFeedURL` = `https://github.com/LoLyeah/suramak/releases/latest/download/appcast.xml`; `SUPublicEDKey` = placeholder until the owner generates the keypair (see 4). `CFBundleVersion` monotonic — the bump script already owns the plist.
2. **Packaging (`scripts/make-app.sh`)**: download the pinned Sparkle framework in CI (`scripts/fetch-sparkle.sh` with a REAL pinned URL + sha256 of the official release archive — verify the hash on this VPS); embed `Sparkle.framework` into `Contents/Frameworks`; add the `@executable_path/../Frameworks` rpath; ad-hoc sign **nested code** (framework + its helpers/XPC services) within the existing signing flow. Never bundle at build time without the pin script.
3. **Appcast pipeline (`.github/workflows/release.yml`, macOS job)**: after packaging → fetch the Sparkle tools (same pin) → `generate_appcast` over a staging dir with the archive + `RELEASE-NOTES.md` (same base name so Sparkle links the notes; markdown) → `--download-url-prefix https://github.com/LoLyeah/suramak/releases/download/<tag>/` → sign via `--ed-key-file -` reading the private key from the repo secret `SPARKLE_ED_KEY` (base64). Attach `appcast.xml` + deltas to the release. **If the secret is absent: skip signing/appcast with a loud log line** (release still ships; the updater stays dormant — the placeholder public key matches nothing).
4. **Key setup (owner step, guide it in the report + README)**: one-time on a Mac — run Sparkle's `generate_keys -x` to export the private key, add it as the repo secret `SPARKLE_ED_KEY`, put the printed public key into `packaging/Info.plist`. The child leaves a clearly-marked placeholder + exact instructions; no key material anywhere in the repo.
5. **Must not regress**: Linux/Windows builds untouched (feature off; the framework is macOS-only); whisper/engines/capture/M9 look untouched; the existing release steps stay; CI keeps working (optionally add a bounded macOS `--features updates` compile step — only if the framework fetch is cheap and pinned).
6. **Tests**: Linux-runnable only for pure bits (e.g., Info.plist key/pin checks via file parsing). The updater runtime = **owner-verify** (real cycle: install v0.2.0, publish v0.2.1+, watch check → prompt → install → relaunch).
7. **Docs**: README updates section; USER_SMOKE **§32**; report `docs/reports/M12.md` — exact commands, deviations + whys, the key-setup guide, honest boundaries.

## Do NOT

`git commit`/`push`; nothing outside `/home/ubuntu/suramak`; no `sudo`; never hardcode or invent keys; do not change the release flow's existing steps beyond adding the appcast step + framework fetch; do not guess framework archive hashes — download and hash.

## Verification

fmt / clippy `-D warnings` / tests green on Linux (default features unchanged — bit-identical behavior without the feature); the mac-cfg module written conservatively (it compiles in the Mac CI/release build — mirror how M10B gated the Swift bridge); report with exact commands + boundaries; USER_SMOKE §32; the appcast/packaging scripts made idempotent and quiet on repeat runs.

## After this

Owner: create the EdDSA keypair (one-time), then test the first real update cycle. This completes the runtime-updates milestone; notarization stays its own decision.
