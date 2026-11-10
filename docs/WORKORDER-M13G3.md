# WORKORDER — M13G3: the compact Library becomes the LAUNCH surface (+ release-site polish)

Plan source: `docs/PLAN-M13-ANDROID.md` §3 M13g (completion) + M13h's
requirements; the G2 report's residual facts. Tree: `main` post-`2b9971b`
(M13G2 landed: 1062 tests). This is the LAST slice before the M13H tag.

## Deliverables

1. **Launch surface**: `MainActivity` opens the **compact Library**, not the
   spike diagnostics. The spike's four-check diagnostics remain reachable
   from Settings → "Diagnostics" (they retain the release rotation criterion
   the owner still has to eyeball at review time), styled as a single
   diagnostics card per check group rather than a tab farm (the compact look
   reads the same state; do not reduce the checks' coverage — every counter
   and banner continues to work).
2. **The release path had zero empty screens**: the first-run flow (no model
   yet) lands on a **model download card** (M13D's tiers: recommendation per
   language with `tiny`-avoidance for Indonesian; free-space + the
   dataSync-progress notification already coded); the in-progress download
   shows the same percent as the notification, resume on relaunch; the
   Library's empty state offers the import/share-target hint (M13E, working
   today) so a first-time user is never blocked.
3. **Playback**: the library row's play affordance opens the transcript with
   playback-synced highlighting per the M15.6 word chips compact behavior (the
   segments seek; the compact chips ride the same state machine — the M15.6
   machinery is available; wire it, reduced-motion honored).
4. **Onboarding**: the one-time intro overlay explaining the three paths
   (share a voice note / open a file / record in-app) with a skip; state kept
   additively in the settings (a first_run_seen bool — additive schema step
   if needed).
5. **a11y + motion care**: all compact controls keep the M9b labels/rings; the
   onboarding's sheet uses the Deck spring; Reduce Motion removes springs
   only.
6. **The quit/release bar**: Quit-on-notification + the in-app Quit work as
   today; the diagnostics card surfaces the LAST crash (v2.4's banner) so the
   release rotation test rests on the same evidence the spike gives.

## Verification

- The phone-viewport suite: new deterministic shots for the launch surface
  (Library), first-run download card, onboarding sheet, Diagnostics card;
  each inspected in the report; existing shots refreshed.
- The launch surface on Android-target compile; Kotlin warning-free;
  build-apk.sh (versionCode 11) with aapt assertions incl. the missing-
  model gate; fmt/clippy -D warnings; workspace tests green (1062 baseline →
  grow; ONE full run at the end; crate-scoped tests meanwhile; disk: clean
  if < 10 GB).
- **Honest boundary**: the real thumb path (share → transcribe → open export
  → predictive back) is the owner-device test at the M13H release; the
  release-tag dry-run in M13H is the CI vehicle.
- Report: `docs/reports/M13g3-launch-surface.md`.

## Do NOT

- No engine change; no new dependencies; no destructive migrations; desktop
  untouched (all suites stay green and byte-identical); no CI workflow edits;
  no git commit/push; no sudo.
