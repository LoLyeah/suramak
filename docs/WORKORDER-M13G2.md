# WORKORDER — M13G2: compact phone UI, part 2 (exports, motion polish, CI gating)

Plan source: `docs/PLAN-M13-ANDROID.md` §3 M13g (G2 = the remainder after G1).
Tree: `main` post-`08db85e` (M13G1 landed; 1026 tests; five-platform CI green).
G1 shipped the split + Library/editor/Settings + navigation + notification
deep-link + Quit. G2 completes the phone experience for the release.

## Deliverables

1. **Exports via `ACTION_CREATE_DOCUMENT`** (the WP's "Export via
   ACTION_CREATE_DOCUMENT"): the editor's export sheet on compact writes md,
   txt, json, srt, vtt (+ tsv via the desktop list — keep parity), each to a
   storage frame the user picks once; the sheet reuses the M15 serializers
   verbatim (subtitle diagnostics + the bilingual stacked paths included —
   the phone gets the same bytes the desktop writes). Export source: a job
   bound by transcript id, running through the same worker conventions
   (bounded atomic write; a share intent is ALSO offered as a faster alternative
   — `ACTION_SEND` for one file — disclosed and deterministic).
2. **Motion polish** on compact: navigation push/pop, sheet present/dismiss,
   list insert/delete, progress; predictive-back-compatible pop (the v2.3-era
   back contract holds; document exactly what happens on the
   predictive-back gesture mid-transition); Reduce Motion removes only the
   springs; fling: implement in the Deck motion layer if egui's touch
   momentum is insufficient on the phone (defer with a probe if not — the
   report states WHICH).
3. **Latency + battery states**: the pipeline runs bounded by the M16.1 slot;
   during a job the compact screens keep the same frame-budget rules (no
   continuous idle redraw).
4. **CI gating**: the `tests/ui-compact/` suite joins the linux job
   (Xvfb) as a named step so a layout regression is a red build.
5. **a11y floor**: labels + focus rings on the compact controls (M9b rules),
   keyboard access on the phone irrelevant but the semantics stay.

## Verification

- Deterministic phone-viewport shots for the export sheet open + the picker
  result path + the settings/models tiers (the M13D tier statements), each
  inspected in the report; the CI step green.
- The M15 serializer byte-parity tests run UNCHANGED (the phone writes the
  exact bytes the desktop writes — asserted by calling the same functions).
- fmt/clippy -D warnings; workspace tests green (1026 → grow; ONE full run at
  the end; crate-scoped tests meanwhile; disk: clean if < 10 GB). Desktop
  keeps every suite green + the existing Xvfb smoke unchanged.
- The Android_target gates stay green (`cargo ndk clippy -p suramak-ui`),
  Kotlin warning-free, build-apk.sh with the aapt assertions (versionCode 10).
- **Honest boundary**: fling feel and the predictive-back gesture are
  owner-phone facts; the report's §owner checklist covers them.
- Report: `docs/reports/M13g2-compact-completion.md`.

## Do NOT

- No engine/model/service change; no desktop regression (all desktop suites
  stay green); no new dependencies; no CI workflow edits beyond ADDING the
  ui-compact step; no git commit/push; no sudo.
