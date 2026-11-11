# WORKORDER — M20A: the type + motion foundation (Android-first)

Plan: `docs/reports/M20-ui-refresh-plan.md` (Carbon high) — slice 1 of 5.
Before any UI edit, read the impeccable craft floor:
`/home/ubuntu/.hermes/skills/creative/impeccable/reference/craft-floor.md`
(it is the quality bar for this slice; the skill itself is at
`/home/ubuntu/.hermes/skills/creative/impeccable/SKILL.md`).

## The problem this closes (verified)

`theme.rs::load_system_fonts` is `#[cfg(target_os = "macos")]` only — on
Android nothing loads, so every phone string renders in egui's baked-in
Ubuntu-Light/Hack. That is the owner's "especially the font". epaint 0.33
uses `ab_glyph`: no variable axes, no OpenType feature toggles — static
instances are required.

## Deliverables

1. **Bundle OFL faces** under `assets/fonts/` with their licenses
   (`assets/fonts/OFL.txt` + a provenance note with source URL, version,
   sha256 of the upstream file, and the generated subset's sha256):
   - **Inter** statics: Regular 400, Medium 500, SemiBold 600
     (Google Fonts `ofl/inter`, SIL OFL 1.1), instanced from the variable
     font with `fonttools varLib.instancer`, then subsetted to Latin +
     Indonesian coverage: `U+0020-024F` + `U+2000-206F` + `U+20AC` + `U+2122`
     + arrows `U+2190-2193`. Target ≤ 120 KB per face.
   - **Newsreader** Regular 400 (OFL) — the serif voice (empty-state titles,
     onboarding hero, editor document title) — same subset.
   - **Geist Mono** Regular 400 (OFL) — mono, earned only (diagnostics,
     hashes, paths, fixed-width numerals if the tnum bake fails).
   - Tooling: `/usr/bin/python3 -m fontTools` (4.68 installed); the generation
     script committed at `scripts/fonts/build_fonts.py` so the assets are
     reproducible, and the script must verify the produced files load
     (fontTools) and print each sha256.
2. **Register the faces on every platform** (`theme.rs`): a platform table —
   macOS keeps SF Pro / New York / SF Mono (unchanged behaviour); Android
   (and Windows/Linux, which today also fall back to egui's defaults) load
   the bundled statics. Register the weight families as Carbon's plan names:
   `SansMedium`, `SansSemiBold`, and keep `Proportional` = Regular.
   `serif_family()` must resolve to the bundled Newsreader on non-mac.
3. **The type scale as tokens** (new `deck/type_scale.rs` or a `Type` block
   in `theme.rs` — follow the codebase's token discipline): Screen title 22
   SemiBold/-0.4 tracking/28 line; section label 11 SemiBold/+0.8 CAPS/16;
   row title 15 Medium/-0.1/20; row value 14 Regular/20; body 15.5 Regular/24;
   caption 12.5 Regular/+0.1/17; numerals 12.5 Medium/16 (fixed-width cells
   helper until tnum proves out); editorial display 21 Newsreader/-0.2/27.
   Sizes in egui points; expose helpers (`type_scale::title()`, `.row_title()`,
   …) returning `FontId` + `extra_letter_spacing` so screens stop hard-coding
   sizes. Do NOT rework the screens in this slice — just the tokens/helpers.
4. **Motion foundation** (`deck/motion.rs`): add the plan's helpers — screen
   push/pop (SLOW, ±24 pt X + opacity), sheet present/dismiss (SLOW, Y slide +
   scrim), press (FAST in / spring out), success badge (BASE), scroll
   rubber-band (BASE). Each helper: takes `now`, returns values that END at
   t=1; Reduce Motion → the plan's per-interaction substitution (crossfade or
   instant). A shared "animation clock" that requests repaints ONLY while
   running (no idle loops) — with a unit test that at t≥1 no repaint is
   requested.
5. **Tests**: font registration per platform (macOS table unchanged; Android
   loads all five faces); a wrap test with Indonesian long words at 360 dp
   width (no panic, line count sane); the type-scale helper returns the
   plan's values; motion helpers end + reduce-motion substitutions; the
   repaint test.
6. **Goldens**: phone-viewport goldens will shift (intentional — the fonts
   change). Re-bless them in this slice and state the new baseline in the
   report; desktop goldens must shift only in font metrics if at all
   (macOS keeps SF; Linux goldens will now render Inter — re-bless and note).

## Verification

- fmt/clippy -D warnings; workspace tests green with ONE full run at the end;
  the emulator smoke unchanged in this slice (it must stay green).
- Report: `docs/reports/M20a-type-motion.md` — the font provenance table
  (source URL, upstream sha256, subset sha256, byte sizes), the type-scale
  table, the motion table, the re-blessed golden list, and the honest
  boundary (phone screens still use the old layout until M20C/D/E).

## Do NOT

- Do not restyle screens yet (slice 3+); no new crates; no variable-font
  runtime loading; no fake glass; no engine changes; no CI edits; no
  git commit/push; no sudo.
