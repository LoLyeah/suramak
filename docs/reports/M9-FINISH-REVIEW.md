# M9-FINISH-REVIEW — fresh-eyes finish pass over the M9 UI remake

**Tree:** frozen at `55177a0` (M9a `8e472db` + M9b `d24a3b2` + M9c `55177a0`; working tree clean). Reviewer did not build any of the three slices.
**Method.** Read, in order: `M9-CRITIQUE-A.md` (the scoring convention: severity load, 0–4 per heuristic, lower is better, max 40 — reused verbatim below), `M9-BRIEF.md` (principles + protect list), `.impeccable/motion-brief.md` (§5 checklist; owner-confirmed), the three slice reports and workorders. Then: re-ran the gates on the frozen tree (**335 passed / 0 failed / 6 ignored** — matches the M9c report), re-ran the M9c pixel suite (**18 PASS / 0 FAIL** — reproduced), read every `build/m9c-e2e/` capture with zoomed crops, re-measured the geometry claims directly in pixels (ImageMagick), and read the final source for every code claim. Frame numbers re-read from `build/m9c/smoke-*.log`. macOS-only paths (glass, `NSAppearance`, `NSWorkspace` Reduce Motion, accesskit-macOS) are **judged from code + the honesty of the reports, not pixels** — there is no macOS host here; that boundary is respected throughout.

---

## 1. Nielsen re-score (same convention as the critique)

| # | Heuristic | Before | After | Key residual (one per row) |
|---|---|---|---|---|
| H1 | Visibility of system status | 2 | **1** | `recording` is now a legible red word; the meter is a real bar + track + rolling waveform; record timer and pill both `hh:mm:ss`; the "meter samples" telemetry string is gone. Residual: the meter carries no scale/peak marker. |
| H2 | Match with the real world | 2 | **1** | Copy sweep landed (human model names, `engine: Core ML`, pluralization, no `16 kHz` / `· tiny` / `planned`); the two sanctioned clock formats are the only clocks. Residual: recording titles are still timestamp filenames; deck `mm:ss.mmm` and gutter `hh:mm:ss` coexist on one screen by design (brief principle 7). |
| H3 | User control and freedom | 2 | **1** | Escape is bound (sheet → editor); ⌘⌫ enters the confirm flow. Residual: no undo for delete, no cancel for in-flight jobs, Escape unbound outside the editor. |
| H4 | Consistency and standards | 3 | **1** | ONE painted control system (primary/secondary/tertiary/Capsule), dead titlebar glyphs removed, emoji set gone, one `⌃ Home` navigation, Stop vs pill differentiated by weight. Residual: record view + settings still use themed stock checkbox/slider/combobox (scope declared in M9a/M9b). |
| H5 | Error prevention | 2 | **2** | Unchanged: quitting mid-recording still stops capture and **discards the buffers silently** (`runtime.rs::shutdown` → `session.stop()`, result dropped; no UI warning); import/export path fields still accept anything until the action fails. Delete confirm + settings validation still good. |
| H6 | Recognition rather than recall | 2 | **1** | Dead titlebar glyphs removed; glyph/action controls carry tooltips + a11y labels. Residual: the waveform strip and gutter stamps have a11y labels but no hover tooltip; the truncated source row has none. |
| H7 | Flexibility and efficiency | 3 | **2** | Full keyboard map now exists (Space, ⌘R, ⌘E, Esc, ↑/↓, ⌘⌫ — guarded, tested). Residual: no scrub-drag or fine seek keys on the strip, recents still silently capped at 6, no bulk ops / search / menu commands beyond system Quit. |
| H8 | Aesthetic and minimalist design | 3 | **1** | The ladder is real: transcript rows sit on the pane (38,253 px of exact `gray(10)` → 0), cards use one recipe (fill + hairline + soft shadow), the deck is one band, the pill floats over the transcript. Residual: the export sheet occludes rows while open (popover), the long polish setup line competes for attention, the light ladder is soft. |
| H9 | Error recovery | 1 | **1** | Unchanged quality (specific, in-place, actionable); status lines now fade in/out through one helper. Residual: still 12 px captions with no icon, position varies per view. |
| H10 | Help and documentation | 2 | **1** | Tooltips now exist on painted controls (label and tooltip from one source). Residual: no Help menu / shortcut sheet / onboarding; shortcuts remain undiscoverable except by hovering. |

**Total after: 12 / 40 → Fair (10–16 band).** **Delta vs 22 / 40: −10 (Poor → Fair).** The two P0s and the measured heat P1 are closed; what remains is one genuine H5 risk (quit-mid-recording), flexibility gaps, and polish.

---

## 2. Critique priority items — per-item verdicts

### 2.1 P0 / P1

| Item | Verdict | One-line evidence |
|---|---|---|
| **P0-1** three-surface patchwork | **Fixed** | One ladder (window → panel → card), `theme::install()` rewrites egui `Visuals` in **both** theme slots so no stock fill can appear; transcript region measured 38,253 px of `gray(10)` before → **0 px** after, both appearances (M9a pixel log; theme tests `installing_visuals_never_leaves_stock_fills`). Cards carry hairline + shadow; one radius scale. [code][pixel][test] |
| **P0-2** near-black transcript bars | **Fixed** | `extreme_bg_color`/`text_edit_bg_color` → transparent; rows are clean text on the pane surface with a hover wash and a 3 pt accent edge bar + faint tint for the active segment (found in pixels, both appearances); gutter `hh:mm:ss` in a calm mono gutter; `TextEdit` frame off. [shot][pixel][code] |
| **P1-3** zero motion | **Fixed** | `motion.rs` tokens 120/180/240 ms, ease-out only; applied to routes, pill morph, live-line arrivals, active segment, every control hover/press, drop-zone ring, save/status captions, and the M9c additions (sheet, lists, progress, settings, errors); every animation ends; Reduce Motion zeroes every duration (egui `animation_time` too); retarget-on-interrupt tested. [code][test][shot] |
| **P1-4** five control languages + dead affordances | **Partially** | One painted system with defined states + tooltips, dead titlebar glyphs removed, emoji replaced, unified `⌃ Home` — but the record view's gain/mute and settings' checkbox/slider/combobox remain themed stock egui widgets (declared scope in M9a/M9b). The "one action, two visuals" stop duplication is resolved by weight (pill primary, Stop secondary), both painted by the same system. [code][shot] |
| **P1-5** repaint heat | **Fixed** | `cadence.rs` policy replaces the 50 ms busy loop and the `RedrawRequested` self-repaint; measured on this machine: idle 873 → **9** frames / 30 s, 240 s record analogue 4,487 → **2,328**, job 353 → **11**; `SURAMAK_FRAME_STATS` instrumentation shipped. Honest caveat (repeated from M9b): these proxy cadence, not the macOS glass composite cost — the Mac number is the owner's to read. [code][log] |

### 2.2 P2/P3-class findings (cognitive load, persona flags, minor observations)

| Finding | Verdict | Evidence |
|---|---|---|
| Cognitive load: primary action obvious | **Fixed** | Editor's loudest object is the pill (record) with Export… a labeled secondary in the deck; record view's Stop demoted to secondary. [shot][code] |
| Control placement consistent | **Fixed** | `⌃ Home` same quiet control, same corner, editor/settings/models/transcribe/record-failure; `Back` split gone. [shot][code] |
| Labels need no guessing | **Fixed** | Dead glyphs removed; painted glyphs + tooltips; no emoji. Residual: no tooltip on the truncated source row. [code][shot] |
| Current state visible | **Fixed** | `recording` legible; sidebar selected (accent tint + edge bar) vs hover (quiet wash) distinct and animated; focus rings on painted controls. [shot][code][pixel] |
| No unexplained jargon | **Fixed** | Copy sweep table in M9c §3; protected strings kept verbatim (verified in code: `live transcript — provisional · the final pass runs on Stop`, `recording from … — transcribed on this machine, never uploaded`, the `ffmpeg` codec message, per-app no-match, `recording is disabled while a job is running`). [code][shot] |
| Alex: no shortcuts / dead glyphs / fake waveform | **Fixed** | Keymap (11 tests) + titlebar glyphs removed + real peaks (flat baseline when no audio — never the old silhouette). [code][test][shot] |
| Alex: no bulk ops / recents cap / no search | **Not** | Recents still silently `min(6)`; no multi-select/search/re-run. Carried over, unassigned. [code] |
| Alex: settings/models don't scroll at 900×600 | **Not** | No `ScrollArea` in `settings.rs`/`models.rs`; carried over, unassigned. [code] |
| Sam: no screen-reader support | **Fixed** | accesskit enabled (features only), adapter initialised before first show, semantic labels across painted controls (two a11y-tree tests run real frames). [code][test] |
| Sam: mouse-only world, no focus | **Fixed** | Focus rings everywhere painted; Tab-reachable (M9b Tab screenshots: rows, transport, strip). [test][shot] |
| Sam: contrast unmanaged | **Partially / unverified** | No contrast artifact exists (see §6, item 6); my spot measurements: light ink 15.2:1, ink_soft 5.75:1; dark ink 13.8:1, ink_soft 7.5:1; `record_red` on the light card ≈4.46:1 (a hair under AA for 12 px text); `ink_faint` 2.89:1 but **unused**. |
| Sam: meter color-only | **Fixed** | Meter is length-encoded (bar over track) plus a rolling waveform; status conveyed by the word + colour. [shot][pixel] |
| Sam: emoji announced by name | **Fixed** | Painted `Glyph` set; label = tooltip (one source). [code][test] |
| Sam: Reduce Motion forward requirement | **Fixed (macOS path compile-checked)** | `NSWorkspace.accessibilityDisplayShouldReduceMotion` read in `suramak-glass`; every helper snaps; Linux returns `false` (documented). Runtime proof is the owner's Mac. [code] |
| Jordan: no onboarding / empty-state | **Not** | No onboarding added; empty states fade in. Carried over, unassigned. [code] |
| Jordan: quit mid-recording discards | **Not** | Same as H5 — silent discard on quit. Carried over, unassigned. [code] |
| Jordan: clocks disagree / jargon / dead glyphs | **Fixed** | One `hh:mm:ss` for timer+pill; copy sweep; glyphs removed. [shot][code] |
| Jordan: prefilled export destination, faint `saved` | **Partially** | Destination still prefilled (standard default); the save/status caption now sits in a constant caption row with the shared fade — still a 12 px caption. [code] |
| Minor: five time formats → two | **Fixed** | Only `format_elapsed` (hh:mm:ss) and `format_transport` (mm:ss.mmm) remain in the UI; a test rejects the old shapes. [code][test] |
| Minor: sidebar selection ambiguous | **Fixed** | Distinct, animated states + edge bar. [code][shot] |
| Minor: clipped source label | **Partially** | Still `add_sized([220, 20], …truncate())` with **no tooltip** — but the full label is visible in the same view's caption line (see §7.1). [shot][code] |
| Minor: pill parked on the export band | **Fixed** | Bottom band → anchored sheet under `Export…`; pill floats over transcript in every editor state. [shot][pixel] |
| Minor: duplicate empty state, tofu policy, date-format split | **Not** | Unchanged; all cosmetic. [code] |
| Minor: raw model ids | **Fixed** | `display_name` map everywhere; raw id only as the manager's detail line. [shot][code] |

---

## 3. Motion §5 spot-walk (state → treatment → Reduce Motion; evidence per line)

1. **Route set (home ↔ editor ↔ …)** — slow fade + 2 pt rise via a child-UI offset, fresh epoch per change (no layout thrash, measure never shifts); Reduce Motion snaps. [code][test] *Note:* the first app frame also plays the fade (fresh epoch at startup) — a mild reading of the brief's "the initial frame renders statically".
2. **Export sheet** — open = base fade + 2 pt rise, close = fast fade; fade id registered while closed so the first open animates; closing sheet keeps drawing but takes no input; rapid-toggle retarget test. Shots: open (white card at anchor), close (pixel-probed blend: `(230,232,238)` vs open `(255,255,255)` / settled `(233,235,240)`), closed (settled). [shot][pixel][test]
3. **Lists (library + home)** — insert ease (fade + rise, base) on post-first-load rows; remove ease = a leaving row collapses its height in place while rows below slide up; first load static; errored refresh animates nothing; delete-1 shot catches the leaving row mid-fade; tests cover the merge/keep-index logic. [shot][code][test]
4. **Status / inline messages** — one helper (`status_caption`) fades in base / out fast, stale text lingers while fading, never blocks input; wired to home (error priority), editor (constant caption row), record, transcribe, models, settings; "deleted en.wav" verified present on the settled Home. [shot][code]
5. **Record lifecycle** — idle→arming (route fade + pill morph + `starting capture…`), arming→recording (caption crossfades into the live-sources line in one preallocated row), recording→stopping (status-word crossfade + hint fade + Stop disables), finalizing→saved (route fade + status fade). `record.png` shows the live state; the transitions themselves are code + the shared helper tests (no mid-flight captures). [code][shot]
6. **Pill morph** — idle dot+`Record` ↔ recording stop glyph + `hh:mm:ss` clock crossfade (base) with a colour cast; hover/press lift at fast. Both states in shots. **Gap vs the brief:** §3's "gentle scale" and §7's "press = gentle spring" are **not implemented** (no spring anywhere in `suramak-ui`; press is a 120 ms alpha lift), and §7's record-view delight "a living dot" has **no implementation** (the live indicator is the red word `recording` + the meter). [code][shot]
7. **Progress** — model download bar smoothed (`ProgressEase`, τ 0.30 s, settles exactly, snaps under Reduce Motion), completion crossfade; `models-download.png` shows the seeded row at `31% — 180.0 MB / 574.0 MB` with accent bar in pixels; tests. [shot][pixel][test]
8. **Settings** — appearance switch **stays instant** (material/visuals swap in one call; proven live in M9a's `live-apply.sh` log); segmented selection crossfades (fast); a one-shot "applied" pulse (~700 ms, ends; skipped under Reduce Motion, status line confirms). [code][log][test]
9. **Empty / first-run / error states** — one-shot arrival fades (base) via `arriving_caption`; nothing loops. [code][test] (no m9c capture of the empty library — code-only evidence).
10. **Playback** — playhead continuous while playing (33 ms cadence), follow-only-while-playing intact (`if is_active && following`). **Gap vs the brief:** §3's "click-to-seek eases" and "follow-scroll animates" are not implemented — seek is an instant data jump and follow is egui's instant `scroll_to_rect`; the M9c table classifies both as "position motion is data, not animation". [code]

**Interruption + budget:** retarget-from-current-value tests for the shared helpers and the sheet; every animation ends; the idle smoke still presents **8 frames / 30 s** (all in the first 0.3 s) — the new motion does not hold the app at tilt. [test][log]

---

## 4. Protect-list verification (no regressions)

| Protected item | Verdict | Evidence |
|---|---|---|
| Real `NSGlassEffectView` material + floating pill | **Intact (macOS runtime = owner's)** | Glass crate diff is appearance-plumbing only (DarkAqua pin removed; `set_appearance`, `reduce_motion`, stored window/base handles) — the material creation path is unchanged; pill keeps one shared `float_rect` (M8.5, bottom-center, 24 pt inset), own cast shadow, honest painted fallback. Shots show the pill floating over the transcript, overlapping nothing. [code][shot][pixel] |
| Error/status copy quality | **Intact** | All named strings found verbatim in code (`ffmpeg` codec message, audio-tap permission guidance, per-app no-match, `recording is disabled while a job is running`). [code] |
| Record view information order + provisional honesty | **Intact** | `record.rs` order unchanged (title → clock/level/waveform → sources → Stop → live transcript); the provisional label and the on-device line render verbatim in `record.png`. [shot][code] |
| Playback model (click-to-seek, timestamp seek, follow-only-while-playing) | **Intact** | Strip click → `waveform_ms_at` (round-trip tested); gutter stamps click-to-seek; follow only while `is_playing`. [code][test] |
| Single-instance guard, clean quit, model manager flows | **Intact** | `instance.rs`, `shutdown.rs` untouched in the diff (HOTFIX-QUIT intact); model manager gains motion/display names only, flows unchanged. [code][diff] |

---

## 5. The owner's two fixes — verified

**Fix 1 — capsule cluster (trash / Polish / Undo).** Pixel log (reproduced): two painted bodies `[(1175, 1204), (1214, 1261)]`, **gap 9 px** (the 8.0 pt layout gap + antialiased edges), **0 stray px** in the gap interior; dark run matches `[(1174, 1205), (1214, 1261)]`. `titleline-crop.png` (light + dark) shows two separate bodies, nothing clipped or overlapping; the confirm step shows `Delete?` + check + cross capsules, all separate. Code: `Tier::Capsule` (translucent fill + hairline, radius = height/2, 28 pt, ≥ `space.s` padding), cluster spacing set explicitly to `space.s`, plus the frame-level test asserting pairwise non-overlap and the gap, and `debug_assert`s that every glyph/label fits its body. The trash reads as a circle (a glyph-only 28 pt capsule) — consistent, not a defect. [pixel][shot][code][test]

**Fix 2 — header line (exactly one rule).** Pixel log (reproduced): **exactly one full-width rule** in the header rule zone at y 141–145 (deck separator at y≈141.5), **no full-width rule** anywhere in the title/caption text zone (y≥146). Code: `header_layout(pane)` derives `separator_y = pane.top() + DECK_HEIGHT − 0.5` from the same constant `deck_band` uses for its single `hline`; the title is a constant 32 pt row with an ellipsis-truncated galley; captions are three explicit 18 pt rows; the frame-level test asserts exactly one rule at the constant y. The Linux render cannot prove the macOS font metrics, but the fix is font-metric-independent by construction — acceptance is the owner's screenshot. [pixel][code][test]

---

## 6. New issues introduced by the remake (honest list)

1. **Home "Re" fragment (Linux-only, real).** The capture-free build's second caption ("Recording is not compiled into this build (feature `capture`) — …") is drawn in the flow at y≈126–140 while the drop-zone card is absolutely positioned at y=130; the card covers all but the line's first ~8 px — **"Re"** is the readable remainder (measured: fragment ink x313–320; glyph band under the card edge across the width). Root cause: M9a's `install()` set `spacing.item_spacing = (8, 8)` (egui default y was 3), growing the flow block while the drop zone stayed absolute; the pre-remake shot shows the line fully visible. **Invisible on macOS** (`capture_available == true` there, so the line is never drawn) — but it is a genuine layout-fragility regression in the Linux build and would bite any future second caption. Severity: P3. Fix: move that caption above the zone (or into the message slot / make `zone_top` flow).
2. **Motion-contract details (owner-confirmed brief) never assigned by any workorder and not implemented:** pill press "gentle spring" (press = fast alpha lift), pill morph "gentle scale", record-view "living dot", waveform "click-to-seek eases" and "follow-scroll animates". The workorders' minimum motion set is fully met; these brief-level intents were dropped between brief and workorders. Polish-level, but the owner will test the pill by hand on the Mac.
3. **The first app frame animates in** (route epoch is fresh at startup) — a mild reading of "the initial frame renders statically". Harmless; noted for honesty.
4. **Report-accuracy: the M9c "~5 px Linux delta on the first transcript row" is understated.** Measured against M9a/M9b (same seed, same harness), the first transcript row sits **~48 px lower** in M9c (bands 226–240 → 274–288), from the reserved 3-row caption block; "~5 px" matches only an in-slice pre/post-fix comparison. It does not matter for the owner (the M9c header is the shipped header and is coherent; macOS never rendered the M9a header), but the sentence "the M9a look is preserved" should not be read as geometry-preserved. Also: the reserved rows cost ~36 px of reading space when save/status are empty — an acceptable price for the constant rhythm, or an argument to reserve fewer rows.
5. **Evidence-quality: one pixel check passes for the wrong reason.** "delete: the status line is present on the settled Home" probes y 355–380, which lands on the Models/Settings buttons (ink 333–451 × 359–369); the actual status line is at y 384–392 (ink 321–399) — present, verified directly, but outside the probe. Fix the probe region (y 380–400); the claim itself holds. [measured]
6. **Contrast has no verification artifact** (brief principle 8 asks for a check; none exists in code/tests). My spot numbers are in §2.2 — comfortable except `record_red` ≈4.46:1 on the light card (hairline under AA for 12 px text). `ink_faint` (2.89:1 light) is dead weight — unused. [measured][code]
7. **Export sheet occludes transcript rows while open** (popover by design; transient; the old band collided with the pill, so this is the accepted trade). Observation, not a defect.
8. **Carried over, not new:** quit-mid-recording discards silently; recents cap at 6; settings/models don't scroll at 900×600; duplicate empty-state copy; `suramak_playback::format_mmss` unused (M9c flagged it itself); truncated source row without tooltip.

---

## 7. The three known items — verdicts

1. **`System audio (all processes ex…` truncation.** Still `add_sized([220, 20], Label::truncate())` with **no tooltip** (`record.rs`). **Acceptable as shipped**: the complete label is visible in the same view's caption directly above the card (`recording from Default microphone + System audio (all processes except Suramak) — …`, verified in `record.rs:151` and the record shot). One-line fix if wanted: `.on_hover_text(&source.label)`. Nice-to-later.
2. **M9c deviation "Linux visual delta ~5 px on the first transcript row".** See §6.4 — measured ~48 px vs M9a/M9b; the claim's figure is in-slice. **Does not matter for the owner's macOS acceptance** (the header is font-metric-independent by construction and coherent in both appearances); it matters only as report accuracy and as a note that the caption block reserves ~36 px of reading space.
3. **Disabled "Polish" capsule's muted look.** **Deliberate disabled state**: `Tier::Capsule × State::Disabled` (fill/stroke/text at `disabled_alpha` 0.45), enabled = `can_polish()` which is false when polish is unconfigured; the view carries the setup note ("polish is not configured — set the base URL in Settings…") and the button's tooltip. Correct behaviour, correctly styled.

---

## 8. Must-fix before ship vs nice-to-later

**Must-fix (P0/P1): none found.** The frozen tree closes both P0s, all three P1s (P1-4 with the declared stock-widget residual), and no new P0/P1-class defect was found in shots, pixels, code or tests. The Linux "Re" fragment is P3 and macOS-invisible; the motion-brief gaps are polish; the carried-over items (quit-discard, recents cap, settings scroll, contrast artifact) are pre-existing and outside M9's assigned scope.

**Nice-to-later (ordered):**
1. Motion-brief details the owner may check by hand: pill press spring, living dot, seek easing / follow-scroll animation (each small, contained in `views.rs` / `editor.rs`).
2. Linux-only Home caption collision ("Re") — move the caption above the zone or into the message slot; prevents the whole fragility class.
3. Pixel-check probe fix (status region y 380–400) + a contrast test in `theme.rs` (a small WCAG-ratio helper over the token pairs; bump `record_red` a notch in light if the test demands).
4. Add the missing tooltip on the truncated source row; consider one on the waveform strip.
5. Carried-over: quit-mid-recording warning, recents cap/`and more`, settings/models scrolling at 900×600, duplicate empty-state copy, `format_mmss` removal/doc.

---

## 9. Ship verdict (for the owner's macOS acceptance test)

**Ship-ready, pending your Mac.** Everything that can be verified on this machine is verified on the frozen tree: the gates are green (335/0/6 re-run), the pixel suite is 18/18 re-run, the two owner fixes hold in both appearances with hard pixel evidence, and the protect list is intact in code and pixels. The remake does what it promised: the patchwork is gone, the transcript is the product, the controls are one system, motion is a grammar with Reduce Motion honored, the heat loop is throttled, and the copy is human. What remains is exactly what only your Mac can settle — the real glass + `NSAppearance` re-resolve, Reduce Motion from `NSWorkspace`, the system-font header metrics (fix 2 is built to be font-independent; verify with one screenshot), and the felt quality of the motion (note: the pill's press is a plain 120 ms crossfade, not the brief's gentle spring — if you miss the spring, it is a small follow-up). Run the acceptance pass expecting the M9c layout; ignore the Linux-only "Re" artifact if you look at Linux screenshots, and treat any faint hairline you see in the editor header as a bug report with a test already waiting for it.

---

*Evidence trail: `cargo test --workspace` re-run (335/0/6, log in scratch); `pixelcheck.py` re-run (18/18); `build/m9c/smoke-*.log` frame stats; `build/m9c-e2e/*` screenshots + zoomed crops; direct pixel measurements (Home caption collision, header rhythm, status-line position, M9a→M9c transcript-row delta, token contrast ratios); `git diff fbd00b8..55177a0` (39 files) for the change surface; source reads of `theme.rs`, `motion.rs`, `cadence.rs`, `framestats.rs`, `controls.rs`, `editor.rs`, `record.rs`, `home.rs`, `library.rs`, `export.rs`, `bounds.rs`, `views.rs`, `model_manager.rs`, `keymap.rs` (via tests), `runtime.rs`, `shutdown.rs`, `suramak-glass`.*
