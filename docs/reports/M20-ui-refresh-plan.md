# M20 — Android UI refresh plan (Carbon high + impeccable)

Source: `gemini-4-carbon` (thinking budget 12288), 2026-11-12, after
`impeccable context` (surface brief: desktop main window, Operate mode; no
Android surface brief existed — this plan creates the phone's direction).
Prompt kept at `carbon-ui-refresh-prompt.md` (scratch); raw answer below.

## Parent verification of the claims that drive implementation

| Claim | Verdict |
|---|---|
| Android renders egui's built-in Ubuntu-Light/Hack because `load_system_fonts` is `#[cfg(target_os = "macos")]` | **VERIFIED** (`theme.rs`: the fn and its helpers are macos-gated; nothing else loads faces) |
| epaint 0.33 cannot interpolate variable-font axes or toggle OpenType features | **VERIFIED** (Cargo.lock: `ab_glyph`; egui 0.33's `FontData` has no axis/feature API) |
| Google's OFL families are fetchable | **VERIFIED** (HTTP 200 for Inter, Newsreader, Geist Mono variable TTFs) |
| Android has no system backdrop blur for one GL surface | **VERIFIED** by architecture (single egui surface; `RenderEffect` blurs Views, not our GL output) |
| macOS uses real NSGlassEffectView / NSVisualEffectView | **VERIFIED** (`suramak-glass/src/macos.rs`) |
| Motion tokens exist and are barely wired on the phone | **VERIFIED** (tokens + helpers exist; compact screens use them in the list only) |

Adopted as-is: typography root cause, the static-face requirement, the
frost recipe (veil mesh + specular rim + 64×64 grain + 2-plane shadow budget),
the motion table, the de-jargoning list, the 8 pt rhythm and the slices.
Softened: "tabular numerals baked offline" — bake only if a scripted cmap
remap of Inter's `.tf` glyphs is clean; otherwise timecodes use a fixed-width
cell helper (egui cannot apply `tnum` at runtime).

---
**POV:** Suramak’s Android ugliness comes from three compounding defects: `egui`’s 2019 `Ubuntu-Light`/`Hack` fallbacks rendering every string `[KNOWN]`, flat navy rectangles using thick electric-blue strokes where depth and weight hierarchy should live `[KNOWN]`, and developer diagnostics leaking into user-facing cards `[KNOWN]`. We will ship static OFL font subsets, build a honest zero-blur frosted surface stack in `epaint`, wire existing motion tokens across every transition with zero idle repaints, and strip engineering jargon from primary flows `[INFERRED]`.

---

### A. TYPOGRAPHY

* **Root cause & selection:** `load_system_fonts` is `#[cfg(target_os = "macos")]` only (`SFNS.ttf`, `NewYork.ttf`, `SFNSMono.ttf`), leaving Android on `Ubuntu-Light` and `Hack` `[KNOWN]`. Do **not** read Android `/system/fonts/RobotoFlex-Regular.ttf`: OEM ROMs (like the owner’s Vivo X300 `[KNOWN]`) mutate system font metrics `[INFERRED]`, and `egui 0.33`’s `ab_glyph` rasterizer cannot interpolate variable `wght` axes or toggle OpenType features at runtime `[INFERRED]`.
* **Candidates evaluated (all SIL OFL 1.1) `[INFERRED]`:** *Geist Sans* (crisp, slightly technical), *Instrument Sans* (quirky neo-grotesque), *IBM Plex Sans* (editorial/industrial), **Inter 4.1** (neutral neo-grotesque engineered to match `SFNS` cap/x-height within <2%).
* **Verdict — Bundle 5 offline-subsetted static TTFs (~200 KB total, zero new crates) `[INFERRED]`:**
  1. **Sans (`Proportional`, `SansMedium`, `SansSemiBold`):** **Inter 4.1** Static Regular (400), Medium (500), SemiBold (600), subsetted to `U+0020–024F` + punctuation with `cv05` (disambiguated `l`/`1`) baked offline `[INFERRED]`. Preserves identical line wraps across shared macOS/Android "Deck" layouts `[INFERRED]`.
  2. **Numerals (`SansTabular`):** Inter Medium with `tnum` baked offline for jitter-free live timers and counters `[INFERRED]`.
  3. **Serif (`serif_family()` `[KNOWN]`):** **Newsreader** Static Regular (400, OFL). Survives on Android *strictly* for empty-state headlines, onboarding hero titles, and Editor document titles—never controls `[INFERRED]`.
  4. **Mono (`Monospace` `[KNOWN]`):** **Geist Mono** Regular (400, OFL). Earned *only* in Diagnostics, hashes, and paths `[INFERRED]`.
* **Indonesian fitness:** Bahasa Indonesia uses pure Latin script with long agglutinative words (*mentranskripsikan*, *ketidakberdayaan*) and zero CJK ideographs `[INFERRED]`. Inter’s tall x-height and fixed `epaint` `line_height` prevent word crowding without CJK line-box inflation `[INFERRED]`.

| Role | Face / Weight | Size | Tracking (`extra_letter_spacing`) | Line Height | Usage `[INFERRED]` |
|---|---|---|---|---|---|
| **Screen title** | Inter SemiBold (600) | 22 pt | `-0.4 pt` | 28 pt | Top bar (`Library`, `Settings`), sheet titles |
| **Editorial display** | Newsreader Regular (400) | 21 pt | `-0.2 pt` | 27 pt | Empty state title, onboarding hero |
| **Section label** | Inter SemiBold (600) | 11 pt | `+0.8 pt` (CAPS) | 16 pt | `TRANSCRIPTION`, `MODELS` (`ink_faint`) |
| **Row title** | Inter Medium (500) | 15 pt | `-0.1 pt` | 20 pt | Card/setting titles, transcript titles |
| **Row value** | Inter Regular (400) | 14 pt | `0.0 pt` | 20 pt | Right/sub-value (`Auto-detect`, tier summary) |
| **Body / transcript** | Inter Regular (400) | 15.5 pt | `0.0 pt` | 24 pt | Spoken segment copy, Indonesian/English prose |
| **Caption / meta** | Inter Regular (400) | 12.5 pt | `+0.1 pt` | 17 pt | Storage notes, helper copy (`ink_soft`) |
| **Numerals** | Inter Medium `tnum` (500) | 12.5 pt | `0.0 pt` | 16 pt | Segment timecodes (`01:14`), duration, line length |
| **Mono (earned)** | Geist Mono Regular (400) | 11.5 pt | `0.0 pt` | 16 pt | Diagnostics card, build target, SHA/paths only |

---

### B. MOTION GRAMMAR

Existing tokens (`FAST` 120 ms, `BASE` 180 ms, `SLOW` 240 ms, cubic-out, press spring `0.97` / 240 ms, Reduce Motion → snap) and helpers exist `[KNOWN]`, but are barely wired `[KNOWN]`.

| Interaction | Token & Easing | What Moves (Transform / Opacity) `[INFERRED]` | Reduce Motion `[KNOWN]`/`[INFERRED]` |
|---|---|---|---|
| **App launch** | `BASE` (180 ms), cubic-out | Content `rise` (`+6 pt → 0` Y) + opacity `0 → 1` | Instant (`t = 1.0`) |
| **Screen push / pop** | `SLOW` (240 ms), cubic-out | Incoming `±24 pt → 0` X + opacity `0 → 1`; outgoing `∓8 pt` X + opacity `1 → 0` | `FAST` (120 ms) crossfade only |
| **Sheet present / dismiss** | `SLOW` (240 ms), cubic-out | Sheet `translateY(100% → 0)`; scrim opacity `0 → 1` | Opacity crossfade (`FAST`), no slide |
| **Row / button press** | `FAST` in / `240 ms` spring out `[KNOWN]` | Scale `1.0 → 0.97` + `control_active` fill lerp (`0 → 1` opacity) | Fill opacity swap only; no scale |
| **List insert / remove** | `BASE` (180 ms), cubic-out | Height collapse/expand snapshot `[KNOWN]` + `rise` `4 pt` + opacity | Instant layout reflow |
| **Download progress** | `FAST` (120 ms), linear | Fill width lerp between byte-chunk ticks; percentage swaps via `tnum` | Width snaps per chunk |
| **Recording pulse** | Tied to audio RMS (~30 Hz) | Mic dot halo scales `1.0 → 1.28` from live input amplitude + `0.35` alpha | Static `record_red` dot + timer tick |
| **Success / completion** | `BASE` (180 ms), cubic-out | Badge scale `0.88 → 1.0` + `ok` tint flash (`0.22 → 0` over `SLOW`) | Instant checkmark swap |
| **Pull-to-refresh** | **Omitted (0 ms)** | **None** — local on-device store needs no network pull; preserves drag-to-scroll `[KNOWN]` | None |
| **Scroll edge glow** | `BASE` (180 ms) release | Subtle `2–4 pt` vertical rubber-band offset on content container; no Halo2 Android glow | Hard clamp at scroll bounds |

* **Repaint discipline:** Every animation reads monotonic elapsed time and calls `ctx.request_repaint()` **only** while `t < 1.0` `[INFERRED]`. At `t >= 1.0`, repaints drop strictly to **0 Hz** `[KNOWN]`. Active recording repaints only on incoming audio-level batches (~30 Hz), never an idle 120 Hz loop `[INFERRED]`. Transitions use single-pass vertex transforms to stay inside the 120 Hz (`8.33 ms`) frame budget `[INFERRED]`.

---

### C. MATERIAL & DEPTH (Android Frosted → macOS Real Glass)

Android lacks single-surface OS backdrop blur `[KNOWN]`, while macOS uses native `NSGlassEffectView` / `NSVisualEffectView` `[KNOWN]`. Introduce a capability enum (`MaterialBackend::SystemGlass` on macOS vs `MaterialBackend::PaintedFrost` on Android) `[INFERRED]`:

1. **Veil gradient:** Paint `card`, `sheet`, and floating bars as a 4-vertex `egui::Mesh` (`WHITE_UV`, zero texture binds) lerping vertically from `frost_veil_top` (white `@ 4.5%` dark / `62%` light) to `frost_veil_bottom` (`1.2%` dark / `38%` light) over base `card`/`sheet` tokens `[INFERRED]`.
2. **1 px inner specular + hairline rim:** Stroke the outer `1 px` perimeter with `rim_hairline` (white `@ 6%` dark / black `@ 7%` light), then overdraw the top straight edge + upper corner tangents with `1 px` `rim_specular` (white `@ 13%` dark / `75%` light) `[INFERRED]`.
3. **Micro-grain texture:** Generate a deterministic `64×64` tiled hash-noise `ColorImage` once at startup (4 KB VRAM, zero crates) and UV-tile across `sheet` and floating chrome at `frost_grain_alpha` (`1.8%` dark / `1.2%` light) to eliminate gradient banding `[INFERRED]`.
4. **Shadow & active elevation:** Restrict drop shadows (`card_shadow` `[KNOWN]`) to **two** floating planes only: bottom action dock and modal sheets (`12 pt` blur, `+4 pt` Y, `22%` black) `[INFERRED]`. Scrollable cards use `0 pt` shadow `[INFERRED]`. Replace thick neon-blue selection borders (`turbo-q5-0`, `System` `[KNOWN]`) with `accent_tint` fill (`10%`) + `1 px` `accent` stroke `@ 45%` `[INFERRED]`.
5. **macOS honesty:** Under `MaterialBackend::SystemGlass`, `frost_grain_alpha = 0` and window/panel bases stay transparent so AppKit’s real optical blur shows through untouched `[KNOWN]`/`[INFERRED]`.
* **New tokens `[INFERRED]`:** `frost_veil_top`, `frost_veil_bottom`, `rim_specular`, `rim_hairline`, `frost_grain_alpha`, `dock_scrim_top` (`0%`), `dock_scrim_bottom` (`92%` `window`).

---

### D. HIERARCHY & LAYOUT

* **Rhythm & Card Anatomy:** Enforce an `8 pt` spatial grid (`16 pt` screen margins, `24 pt` between sections, `12 pt` internal card padding) `[INFERRED]`. Fix the Library onboarding card bug where an outer card nests a bright-bordered inner card above ~80 pt of dead space `[KNOWN]`: flatten to **one** shrink-wrapped frosted banner (`Title` → 1-line reason → inline model pill + `Download` button → footer storage caption) `[INFERRED]`.
* **Iconography:** Paint crisp `1.75 px` round-cap vector strokes (`18 pt` inline, `20 pt` chrome) `[INFERRED]`. Replace top-right `"Settings"` text `[KNOWN]` with a `20 pt` gear icon (`44×44 pt` hit box); add a `16 pt` magnifier inside Search (`Search transcripts` `[KNOWN]`), checkmarks on active models, and an import-waveform + mic dot in the bottom dock `[INFERRED]`.
* **Bottom Action Bar:** Replace the full-width rule + flat 2-button footer `[KNOWN]` with a **floating frosted dock** (`16 pt` horizontal inset, `12 pt` above gesture inset, `dock_scrim` fade behind) `[INFERRED]`. Left (`38%`): frosted `Import` pill. Right (`62%`): primary `Record` pill using `record_red` `[KNOWN]` dot + crisp high-contrast fill (replacing the misleading generic blue `Record` button `[KNOWN]`) `[INFERRED]`.
* **Empty State:** Center vertically between banner and dock: `28 pt` subtle waveform glyph (`ink_faint`), `Newsreader` 21 pt `"No transcripts yet"`, and a tighter 2-line `ink_soft` prompt `[INFERRED]`.
* **Settings Model List:** Merge the 4 separate cards `[KNOWN]` into **one** grouped frosted card with hairline dividers `[INFERRED]`. Rename raw slugs (`tiny`, `base`, `small`, `turbo-q5-0` `[KNOWN]`) to human tiers: **Quick Test** (78 MB), **Compact** (148 MB), **Balanced** (488 MB · *Recommended* badge), **High Accuracy** (574 MB) `[INFERRED]`.
* **DELETE or quiet down `[INFERRED]`:**
  * **Delete on Android:** `Quit` row in Settings `[KNOWN]` (`#[cfg(not(target_os = "android"))]`), `"default (macOS)"` copy on phone `[KNOWN]`, and `"CI + tests"` / `"optional/legacy"` labels `[KNOWN]`.
  * **Relocate to `Diagnostics` sheet only:** Raw model IDs (`turbo-q5-0`), diarization strings (`pyannote-segmentation-3.0 + CAM++` `[KNOWN]`), and API-key env-var names `[KNOWN]`. Rename `"CPL"` `[KNOWN]` to `"Line length"`.

---

### E. IMPLEMENTATION SLICES (Workorders)

1. **Slice 1 — Type & Motion Foundation** `[INFERRED]`
   * *Touches:* `theme/fonts.rs` (remove `macos`-only gate, embed OFL subsets, register `SansMedium`/`SansSemiBold`/`SansTabular`/`serif_family`/`Monospace`), `theme/type_scale.rs`, `ui/motion.rs` (navigation/sheet/press helpers + finite repaint guard).
   * *Acceptance:* Phone-viewport goldens confirm Inter/Newsreader/Geist Mono metrics + Indonesian multi-syllable wrap test; unit test asserts `0` repaints requested at `t >= 1.0`.
   * *Risk:* Subtle baseline shift on shared desktop goldens (re-bless intentionally).
2. **Slice 2 — Frosted Material & Vector Primitives** `[INFERRED]`
   * *Touches:* `theme/tokens.rs` (new frost/rim tokens + `MaterialBackend`), `ui/surfaces.rs` (`Mesh` veil painter, `1 px` specular rim, `64×64` procedural grain texture), `ui/icons.rs`, segmented/toggle controls.
   * *Acceptance:* Light/dark contrast tests pass; Android goldens show specular rims + grain; macOS build confirms transparent passthrough to `NSGlassEffectView`.
   * *Risk:* Over-bright specular rims in light mode if alpha isn’t clamped.
3. **Slice 3 — Library, Empty State & Floating Dock** `[INFERRED]`
   * *Touches:* `ui/deck/library.rs`, `ui/chrome.rs` (gear icon header, magnifier search, shrink-wrapped single model banner, `Newsreader` empty state, floating `Import`/`Record` dock).
   * *Acceptance:* `390×844` & `412×915` goldens (`<4 GB` and `8 GB+` states, empty & populated) + Android emulator TalkBack/touch-slop smoke.
   * *Risk:* Scroll list bottom padding must clear floating dock + Android gesture inset.
4. **Slice 4 — Settings De-jargoning & Grouped Models** `[INFERRED]`
   * *Touches:* `ui/deck/settings.rs`, `models/catalog_copy.rs`, `ui/deck/diagnostics.rs` (grouped model card, human tier names, `Quit` cfg-gated off Android, raw engine/env strings moved to Diagnostics).
   * *Acceptance:* Settings phone goldens (Light/Dark) + grep check confirming zero `CI + tests`, `macOS`, `pyannote`, or env-var strings on primary Settings screen.
   * *Risk:* Keep TalkBack labels concise on multi-line model rows.
5. **Slice 5 — Editor, Onboarding & Sheets Polish** `[INFERRED]`
   * *Touches:* `ui/deck/editor.rs`, `ui/sheets.rs`, `ui/onboarding.rs` (`tnum` timecodes, `15.5/24 pt` Indonesian/English transcript body, sheet slide+scrim motion, `"CPL"` → `"Line length"`).
   * *Acceptance:* Editor & onboarding goldens + 120 Hz emulator profiling during playback/recording and Reduce-Motion toggle smoke.
   * *Risk:* Waveform + speaker lane hit targets during scroll drag.

---

### F. CRAFT-FLOOR SELF-CHECK

**Five ways this looks cheap if done wrong `[INFERRED]`:**
1. **Visible noise:** Grain above `2.5%` alpha looks like dirty sensor static instead of ground glass.
2. **Fake weight synthesis:** Skipping static Medium/SemiBold TTFs leaves headings thin and flat because `ab_glyph` ignores variable weights.
3. **Neon wireframe borders:** Keeping `1.5 px` saturated blue outlines around cards/segments destroys the frosted illusion.
4. **Jittering numerals:** Rendering `00:09 → 00:10` in proportional sans without `tnum` makes badges twitch every second.
5. **Battery-draining idle loops:** Leaving an unbounded `request_repaint()` in recording or progress widgets heats the phone.

**The Restraint Rule `[INFERRED]`:** *One elevated plane, one accent per viewport, zero decorative blursimulation*—if an element does not aid scanning audio or reading words, quiet its alpha or delete it.