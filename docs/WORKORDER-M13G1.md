# WORKORDER — M13G1: the au split + compact phone UI, part 1 (split + navigation + library)

Plan source: `docs/PLAN-M13-ANDROID.md` §3 M13g — **split into G1/G2 for
executability** (the slice is bigger than one child run; G2 is cut when G1
lands). Tree: `main` post-`ceb79f0` (M13F: mic recording; 987 tests; CI green
on all five platforms). The phone already has: models (M13D), import/share +
background transcription (M13E), recording (M13F). G1 gives it a REAL UI.

## Deliverables

1. **The `suramak-ui` split** (the plan's prerequisite): Deck components
   (tokens, painted controls, material ladder, motion primitives, the views'
   reusable parts) extracted so the Android UI reuses EXACTLY them; the desktop
   shell (window chrome, menubar, export sheets' desktop behaviors, etc.)
   stays desktop-only. Decide the shape by what the code looks like: modules
   inside `suramak-ui` first choice — a new crate ONLY if module topology
   fails (state the argument in the report). Every desktop golden and M15
   suite must keep proving the SAME behavior after the split (the split must
   not regress desktop — the plan's #1 risk, test-gated by the existing
   suites plus the new module tests).
2. **Compact layout profile** (De ASSUMPTION from §8): a phone-viewport layout
   for Library + Transcript editor (view/edit/search/exports entry) + Settings
   (models incl. the M13D tiers, the HUD settings' absence is fine, appearance).
   Same data and semantics as desktop; the phone profile only changes the
   navigation and the layout.SKTOP vs swipe: per-screennavigationBar-ish
   push/pop model with the Android back gesture ALREADY proven (v2.4's back
   interception).
3. **Navigation motion**: navigation push/pop transitions with the Deck
   motion system (one spring per direction), reduced-motion = hard transition,
   no continuous idle redraw; fling/scroll: verify egui's touch momentum on
   the fixed screens — implement in the Deck motion layer ONLY if it is
   insufficient (defer with a documented probe otherwise).
4. **Where the app's screen C-todays live**: main.rs's route table gains the
   android-compact layout set behind `cfg(target_os = "android")` — the
   desktop shell keeps its existing identical routes (byte-for-byte behavior).
5. **Journal completion tap**: the M13E completion notification now opens the
   Library (replacing the toast), deep-linking to the finished transcript's
   editor (the route's arguments flow over the existing platform event).
6. **Quit action** (owner requirement, notification channel too): the FGS's
   persistent notification gains a Quit action that stops any capture/job,
   stops services, and exits — matching the M13F ordering (capture first,
   service second, exit third).

## Verification

- Campus compilation gates: `cargo clippy -p suramak-ui --target
  aarch64-linux-android --all-targets -- -D warnings` (the new ui code must
  compile for Android); `cargo test --workspace` green (987 baseline → grow;
  the desktop suites untouched, PLUS the new split-module tests).
- The compact UI is deterministic on the host: the phone-viewport (portrait
  1216×2640 logical scale × 2.6) screenshot suite in `tests/ui-compact/` runs
  on Xvfb like the existing CI smoke — snapshot the Library, the editor, and
  Settings; inspect each in the report (CI-gating of the suite: add to the ci
  linux job if it is Xvfb-reusable).
- The android-Gradle build compiles and packages the new UI (debug
  build-apk.sh with the shared feature set; aapt/apksigner assertions).
- fmt/clippy -D warnings; one full workspace run at the end.
- **Honest boundary**: the COMPACT UI's real device behavior (touch, fling,
  back-stack) is owner-device verifiable next pass; Xvfb shots + unit tests
  are the CI-side evidence.
- Report: `docs/reports/M13g1-compact-library.md`.

## Do NOT

- No engine, model, or service changes (M13D/E/F behavior stays); no CI
  workflow edits; no new dependencies; no destructive migrations; no
  desktop-only regressions (the split keeps every existing test green).
- No git commit/push; no sudo.
