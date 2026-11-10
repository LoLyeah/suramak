# M13G3 — the compact Library is the launch surface (release-ready phone)

Executed per `docs/WORKORDER-M13G3.md` (PLAN-M13-ANDROID §3 M13g completion +
M13h prerequisites). **Orchestrator-authored report**: the child hit its turn
budget before the final full-suite run, the final APK/Kotlin rebuild, the
ui-compact re-run, and this file; every gate below was re-run by the
orchestrator on the final tree.

## Landed (what the phone now does at launch)

1. **The launch surface is the compact Library** — `MainActivity` opens the
   one `CompactHost` app drawn by the `libsuramak_android` cdylib; the spike
   tab farm is gone from the launch path.
2. **Diagnostics stay available** behind Settings → Diagnostics: all four
   check groups (TEXT/LIFECYCLE/MOTION/INSETS+BACK) keep their counters and
   the v2.4 crash banner — the release rotation test rests on the same
   evidence the spike gave (no coverage reduction; the arrow-glyph placeholder
   and a clipped diagnostics footer were fixed on visual inspection).
3. **Zero empty screens**:
   - first run lands on the **model download card** (M13D tiers: language-aware
     recommendation, Indonesian steered off `tiny`, the free-space gate, the
     dataSync percent, resume on relaunch);
   - the Library empty state offers the import/share-target hint (M13E is
     live);
   - a one-time **onboarding sheet** (share a voice note / open a file /
     record in-app) with Skip, additive `first_run_seen` state, riding the
     Deck spring (Reduce Motion removes only the spring).
4. **Playback-synced highlighting** rides the M15.6 word chips with seek;
   reduced-motion aware.
5. a11y labels/rings kept on every compact control.

## Verification (orchestrator)

- `cargo fmt --all --check` clean; `cargo clippy --workspace --all-targets
  -- -D warnings` clean; Android-target clippy clean for `suramak-android`
  and `suramak-android-spike` (cargo ndk, arm64-v8a, platform 29).
- **Full workspace: 1079 passed / 0 failed / 10 ignored** (baseline 1062,
  +17).
- `tests/ui-compact`: **12 passed** — deterministic 1216×2640 shots for
  launch-library, first-run-download, download-progress, download-resume,
  onboarding, diagnostics, library-empty, library-light, editor,
  export-sheet, export-written, settings (inspected).
- `:app:compileDebugKotlin` warning-free (allWarningsAsErrors);
  `scripts/android/build-apk.sh` green — **versionCode 11 /
  `0.4.0-m13g3.1`, 38,508,857 bytes, sha256
  `4deb32e29a07cf568837cb47505ecd90f641b8ce1e1ad154c3cec48a39b7454c`** —
  built from the FINAL tree (the child's quoted APK/sha predated the last
  mic-status edit; this one does not), with the aapt/dexdump assertions +
  the missing-model-gate checks + the android_main/JNI-symbol checks.

## Honest boundaries (owner-device checklist — the M13H release vehicle)

- Share a WhatsApp/Telegram voice note → the progress notification with the
  screen off → completion → the Library row opens the transcript.
- First launch: the model download card runs on Wi-Fi (resume on relaunch).
- Record a clip in-app → hand-off → transcript.
- Export the transcript via the system picker → the file matches the desktop
  bytes.
- Rotate portrait↔landscape ×2 (the release rotation criterion) + the
  predictive back mid-transition; the Diagnostics card carries the v2.4
  counters/crash banner if anything reds.
- Residuals deliberately deferred: fling feel (deferred with a wired probe at
  G2), playback positioning nuances, onboarding copy review — owner pass
  items.
