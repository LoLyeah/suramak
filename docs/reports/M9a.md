# M9a — the remake, slice 1: material · deck · controls · motion · appearance

Workorder: `docs/WORKORDER-M9A.md` (owner decision 2026-10-14: composition **C — Deck**, appearance
**System-following with Light/Dark override**). Brief + protect list: `docs/M9-BRIEF.md`. Defects being
fixed: `docs/reports/M9-CRITIQUE-A.md` (2 P0s, 3 P1s).

**Method / environment.** Linux VPS (2 cores, headless). All runtime evidence is Linux/Xvfb (layout truth
only — no glass on Linux); the macOS-only paths (`suramak-glass`, the macOS token ladder) are
**compile-checked** (`cargo check -p suramak-glass --target aarch64-apple-darwin`) and reviewed, never
executed — there is no macOS host on this machine. Where the workorder asked for macOS behaviour
(NSAppearance, Reduce Motion) the code is written and type-checked; the report says so explicitly per item.

---

## 1. What shipped

### 1) Theme tokens — ONE material ladder, TWO appearances (`crates/suramak-ui/src/theme.rs`)

The ad-hoc `Palette` is gone; `Tokens` is the single struct, authored for **both** appearances and
**both** platform families (macOS = translucent washes over the real material; Linux/Windows = opaque
tones). `install()` reworks egui's `Visuals` **fully** — for BOTH theme slots (`all_styles_mut`), so no
matter how egui resolves its preference, stock fills cannot appear.

The P0-1/P0-2 root causes are closed at the source:

| stock fill | before | after |
|---|---|---|
| `extreme_bg_color` | `gray(10)` (dark) / white (light) — the transcript bars | `TRANSPARENT` (both appearances) |
| `text_edit_bg_color` | (unset → extreme) | `Some(TRANSPARENT)` |
| `panel_fill` | `gray(27)` / `gray(248)` | the `panel` token |
| `window_fill` | stock | the `card` token (+ hairline + shadow) |
| `faint_bg_color` | stock | `panel_faint` |
| widget bg/weak/hover/active/open, selection, strokes, shadows | stock grey/teal | the token deltas (§4) |

Measured on the transcript region of the editor screenshot: **38,253 px of `gray(10)`-exact bars before →
0 px after** (both appearances; see §7).

Token table (authored values; macOS column = translucent washes over glass, Linux column = opaque):

| token | Light (macOS) | Light (Linux/Win) | Dark (macOS) | Dark (Linux/Win) |
|---|---|---|---|---|
| `ink` | `#1c1f26` | `#1c1f26` | `#ecedf2` | `#ecedf2` |
| `ink_soft` | `#596171` | `#596171` | `#a9b1c2` | `#a9b1c2` |
| `ink_faint` | `#8a92a2` | `#8a92a2` | `#7e879a` | `#7e879a` |
| `hairline` | black α36 | black α30 | white α40 | white α34 |
| `hairline_strong` | black α72 | black α64 | white α80 | white α72 |
| `accent` / hover / active | `#2f6fed` / `#4a84f2` / `#245ccf` | same | `#6ea8ff` / `#8abaff` / `#5890e8` | same |
| `accent_tint` | accent α44 | accent α40 | accent α52 | accent α48 |
| `record_red` | `#e0352b` | `#e0352b` | `#ff453a` | `#ff453a` |
| `on_accent` | white | white | `#0b1226` | `#0b1226` |
| `window` (base) | **transparent** | `#e9ebf0` | **transparent** | `#0e1526` |
| `panel` (the one translucent fill) | white α96 | `#f4f6fa` | white α20 | `#17203a` |
| `panel_faint` | white α56 | `#edeff4` | white α10 | `#121a30` |
| `card` (+ hairline + shadow) | white α150 | `#ffffff` | white α32 | `#1a2440` |
| `card_shadow` | black α40, blur 20, +8y | black α38 | black α120, blur 24, +10y | black α140 |
| `control_fill` / hover / active | white α140/184/96 | `#ffffff`/`#f1f4f9`/`#e4e8f0` | white α24/40/52 | `#1c2746`/`#253359`/`#2f406b` |
| `surface_hover` / `surface_active` | black α14/26 | black α12/24 | white α18/30 | white α16/28 |
| `disabled_alpha` | 0.45 | 0.45 | 0.45 | 0.45 |
| `wave` / `wave_dim` | `#545c6b` / `#8b93a3` | same | `#aeb7c5` / `#83868e` | same |
| `float_fill` (pill fallback) | white α200 | white α232 | white α22 | `#1e2a54` α217 |

Scale: `space` 4/8/12/16/24 · `radius` deck 10, card 12, control 8, row 6 · `stroke` hairline 1.0,
strong 1.5, focus 2.0.

System font loading (`SANS_FACE`/`SERIF_FACE`/`MONO_FACE`, `/System/Library/Fonts/*.ttf`) is untouched —
no fonts added, no crates added.

### 2) Appearance: System / Light / Dark

- `crates/suramak-core/src/settings.rs`: `Appearance { System | Light | Dark }` (serde `snake_case`,
  `Default = System`), `ui.appearance` with `#[serde(default)]` so pre-M9a files keep loading.
- `crates/suramak-glass/src/macos.rs`: the M1 **DarkAqua pin is removed**; `Handle::set_appearance`
  sets/clears `NSAppearance` (`NSAppearanceNameAqua` / `NSAppearanceNameDarkAqua` / nil → system) on the
  **window, the base material view and the glass content container**, so the real blur re-resolves with
  the forced appearance. Everything else in the glass crate is byte-identical apart from the new API
  (`ThemeMode`, `GlassHandle::set_appearance`, `reduce_motion()`), plus the `NSWorkspace` feature flag on
  the existing objc2-app-kit dependency (no new crates).
- Shell (`shell.rs::sync_appearance`): resolves the effective theme (`System` → winit `Window::theme()`;
  `WindowEvent::ThemeChanged` re-resolves live; **Linux System → Light is pinned in
  `theme::effective_theme`**, documented in code), installs the egui visuals, pins/clears the glass
  appearance and refreshes Reduce Motion. Called at startup, on `ThemeChanged` and after every effect
  batch (a settings save lands through the same path).
- Settings UI: an **Appearance** section with a segmented System / Light / Dark control (secondary
  tier). The screen's existing Save model is the apply point — applied live, no restart (§7 shows the
  E2E).

### 3) Editor rebuilt to comp C — the Deck (`editor.rs`)

- **Deck band** (panel level, 96 pt, hairline below, full pane width) directly under the titlebar:
  - LEFT: compact transport capsule (76×34): skip-to-start + play/pause, **painted glyphs** via the one
    control system; no duplicated play controls anywhere (the dead titlebar play/export glyphs are gone).
  - MIDDLE: waveform strip spanning capsule→readout (geometry final; the M1 silhouette is still the data
    source — real peaks are slice 2), slim accent playhead, click-to-seek.
  - RIGHT: `mm:ss.mmm / mm:ss.mmm` readout (mono 12, one time-format system) + **one tier-2 `Export…`**
    button. Nothing else floats in the header.
- **Title line**: serif 20 px title; far right a single quiet `Polish` (tertiary) — with `Undo` when a
  polish undo is pending, and the M4.5 delete affordance as painted trash + check/cross glyphs.
- **Transcript rows**: mono `hh:mm:ss` gutter (muted, hover crossfade, click-to-seek), serif 16 px body
  at a **68 % measure**, subtle hover wash, active segment = **3 pt accent edge bar + faint accent tint**
  (fast-token crossfade), inline `TextEdit` editing unchanged (frame off, transparent bg, focus ring in
  the accent language).
- **Floating pill**: semantics/position UNCHANGED (M8.5 `float_rect`, 24 pt inset, 184×52); idle↔recording
  now morphs (base token: label crossfade + colour cast) with hover/press lift. **Collision audit:** the
  old fixed 148 px export band (the pill's named overlap) is replaced by an anchored export sheet under
  the deck's `Export…` button — in every editor state the pill now floats over the transcript, not over a
  control (verified in `editor.png` + `editor-export.png`, both appearances).
- **Record/Home**: composition unchanged (protect list) — tokens, controls and motion only.

### 4) ONE control system (`app/controls.rs`, new)

Three tiers — **primary** (accent fill), **secondary** (labeled on the control surface), **tertiary**
(quiet text/glyph) — every state defined (rest / hover / active / focus / disabled) with token deltas and
fast-token crossfades. Painted glyph vocabulary replaces the emoji set (🗑 ✔ ✖ → `Trash` / `Check` /
`Cross`, plus `Play` / `Pause` / `SkipStart` / `Stop` / `ChevronLeft`). Focus is always the accent ring;
disabled is 45 % alpha; hit areas ≥ 24 pt; glyph controls carry tooltips; hover sets the pointing hand.

Routed through it: home (Choose file…/Import/Models/Settings), editor (Polish/Undo/Accept/Discard/
Dismiss/Export…/trash+confirm+cancel), record (Stop, nav), settings (Save/Reset/Refresh), models
(Re-check/Download/Delete/Set default), export sheet (Save as…/Export/format chips), polish bar, and the
**unified navigation pattern** (`⌃ Home`, same quiet control in the same top-right position on editor /
settings / models / transcribe / record-failure; the old `Home` text link vs `Back` buttons split is
gone). Library/sidebar and home rows are painted rows with distinct hover vs selected states.

### 5) Motion system (`motion.rs`, new)

Tokens **fast 120 ms / base 180 ms / slow 240 ms**, ease-out only (`cubic_out`); helpers: `factor`
(bool crossfade), `arrive` (one-shot fade-in, ends), `arrival_progress` (time-based, ends),
`lerp_color` / `tint_over` (tint crossfade), `rise` / `scale` (slide/scale). Built on egui's animation
manager, which requests repaints only while a value is between 0 and 1 — **every animation ends**; a
`Motion::new(true)` (Reduce Motion) zeroes every duration and snaps, and egui's own widget
`animation_time` is zeroed with it so stock widget fades follow the same rule.

Applied (minimum set, all verified in the screenshots/code): route changes (slow fade + 2 pt rise via a
child-UI offset — no teleport, no layout thrash), pill idle↔recording morph (base), live transcript line
arrivals (base, fade+rise from each line's arrival instant), active-segment tint (fast crossfade both
ways), every control hover/press (fast), plus the drop-zone ring (fast fade + scale) and the record
view's `finishing…` status crossfade. **Text measure never shifts** — motion is colour and paint-time
translation only.

**Reduce Motion** is read from macOS `NSWorkspace.accessibilityDisplayShouldReduceMotion` via
`suramak_glass::reduce_motion()` (objc2, same pattern as the existing platform calls) at startup and
whenever settings apply/open; Linux/Windows return `false` (documented: no equivalent wired up yet).

### 6) Per-file change summary

| file | change |
|---|---|
| `crates/suramak-core/src/settings.rs` | `Appearance` enum + `ui.appearance` (serde default) + 2 tests |
| `crates/suramak-ui/src/theme.rs` | rewritten: `Tokens`, light/dark ladders, `effective_theme`, `install()` (both theme slots) + 5 tests |
| `crates/suramak-ui/src/motion.rs` | **new**: tokens, ease-out, `Motion`, helpers + 7 tests |
| `crates/suramak-ui/src/app/controls.rs` | **new**: tiers × states, glyphs, buttons/segmented/transport/nav + 3 tests |
| `crates/suramak-ui/src/app/editor.rs` | rebuilt to the Deck (band, capsule, waveform, readout, Export…, title line, rows, export hook) + 3 geometry tests |
| `crates/suramak-ui/src/app/export.rs` | bottom band → anchored export sheet (chips, destination, Save as…, primary Export) |
| `crates/suramak-ui/src/app/views.rs` | frame: window-level panels, route fade+rise, pill morph/hover, dead titlebar glyphs removed, `paint_card` |
| `crates/suramak-ui/src/app/home.rs` | tokens/controls/motion; ring fade+scale; rows crossfade |
| `crates/suramak-ui/src/app/library.rs` | selected vs hover distinct (accent tint + edge bar), animated |
| `crates/suramak-ui/src/app/record.rs` | tokens/controls/motion; Stop = secondary; live-line fade+rise; cards |
| `crates/suramak-ui/src/app/settings.rs` | Appearance section (segmented), tokens/controls, `⌃ Home` |
| `crates/suramak-ui/src/app/models.rs` | tokens/controls, `⌃ Home`, painted row actions |
| `crates/suramak-ui/src/app/transcribe.rs` | tokens/card, `⌃ Home` |
| `crates/suramak-ui/src/app/settings_form.rs` | `appearance` field plumbing + 1 test |
| `crates/suramak-ui/src/app/mod.rs` | `theme`/`appearance`/`reduce_motion` state, export sheet state, provisional arrivals, `format_transport`, `seed_demo_recording` + 4 tests |
| `crates/suramak-ui/src/shell.rs` | `sync_appearance` (visuals + glass + reduce), initial install |
| `crates/suramak-ui/src/main.rs` | `ThemeChanged` handling, appearance sync after effects, `SURAMAK_SMOKE_RECORD` harness |
| `crates/suramak-ui/src/runtime.rs` | `appearance()` getter |
| `crates/suramak-glass/src/{lib,macos,stub,windows}.rs` | `ThemeMode`, `set_appearance`, `reduce_motion`; DarkAqua pin removed; no-ops elsewhere |
| `crates/suramak-glass/Cargo.toml` | `NSWorkspace` feature on the existing objc2-app-kit dependency |

---

## 2. Verification (all commands run on the final tree)

```
cargo fmt --all --check                          → clean
cargo clippy --workspace --all-targets -- -D warnings → clean (log: build/m9a-clippy.log)
RUST_TEST_THREADS=1 cargo test --workspace       → 276 passed / 0 failed (log: build/m9a-tests.log)
cargo check -p suramak-glass --target aarch64-apple-darwin → clean
```

**Tests: 276 total (baseline 251 → +25 new).** New tests:

- appearance: `appearance_roundtrips_through_toml`, `appearance_defaults_to_system_when_absent` (core);
  `appearance_roundtrips_form_settings_file_and_back` (form → settings.toml → form);
  `effective_theme_follows_the_system_and_forced_modes_override` (live apply resolution).
- tokens: `both_appearances_author_the_full_ladder`, `installing_visuals_never_leaves_stock_fills`
  (panel/window/faint/extreme + widget deltas + selection), `switching_the_appearance_reinstalls_different_visuals`,
  `reduce_motion_zeroes_the_stock_widget_animation_time`.
- motion: `ease_out_is_the_endpoint_exact_cubic`, `reduce_motion_zeroes_every_duration`,
  `factors_snap_under_reduce_motion`, `factors_animate_and_end`, `arrivals_start_hidden_and_ease_in`,
  `arrival_progress_snaps_under_reduce_motion_and_ends`, `colour_helpers_mix_endpoints_and_rise_reverses`.
- controls: `state_selection_orders_disabled_press_hover_focus_rest`,
  `every_tier_defines_every_state_with_distinct_deltas`, `disabled_states_are_dimmer_than_rest`.
- deck: `deck_geometry_anchors_transport_readout_and_export`, `deck_geometry_survives_the_minimum_window`,
  `waveform_axis_round_trips_through_click_to_seek`.
- state: `the_transport_readout_is_mm_ss_mmm`, `the_export_sheet_toggles_and_closes_when_the_editor_is_left`,
  `provisional_arrivals_track_first_seen_per_segment`, `the_demo_recording_seed_shapes_a_real_session_state`.

**xvfb smokes** (final binary):

- plain: `SURAMAK_SMOKE_SECS=8 xvfb-run …` → exit 0, `build/smoke_ok.txt` (frames=237), log `build/m9a-smoke-plain.log`
  (contains `appearance applied setting=System effective=Light reduce_motion=false` — the Linux rule).
- job: `SURAMAK_SMOKE_JOB=fixtures/audio/en.wav SURAMAK_SMOKE_SECS=300 xvfb-run …` → exit 0,
  `build/smoke_job_ok.txt` (2 segments, model tiny, engine cpu), log `build/m9a-smoke-job.log`.

**Screenshots** (1280×800 app on a 1400×900 Xvfb root; harness `build/m9a-e2e/`):

| view | light | dark |
|---|---|---|
| Home | `build/m9a-e2e/light/home.png` | `build/m9a-e2e/dark/home.png` |
| Editor | `build/m9a-e2e/light/editor.png` | `build/m9a-e2e/dark/editor.png` |
| Editor + export sheet | `build/m9a-e2e/light/editor-export.png` | `build/m9a-e2e/dark/editor-export.png` |
| Record | `build/m9a-e2e/light/record.png` | `build/m9a-e2e/dark/record.png` |
| Settings | `build/m9a-e2e/light/settings.png` | `build/m9a-e2e/dark/settings.png` |
| before (pre-M9a) | — | `build/m9a-e2e/before/home.png`, `before/editor.png` |

The forced appearances come from `settings.toml` (`[ui] appearance = "light" | "dark"`) in the seeded
data dirs — the screenshots themselves are the proof the setting resolves and paints, and the Appearance
row shows the persisted value selected (Light in the light run, Dark in the dark run; verified on zoomed
crops of both settings screenshots).

**Pixel checks** (`build/m9a-e2e/pixelcheck.py`; full log `build/m9a-e2e/pixel-checks-final.log`,
24 PASS / 0 FAIL):

- *transcript-region background not near-black, either appearance* — light: modal `(233,235,240)`
  lum 235 (≥ 200 ✓); dark: modal `(14,21,38)`, channel spread 24, b−r 24 — a designed tinted surface,
  not the stock flat near-black (the pre-M9a editor measured **38,253 px of exact `(10,10,10)` bars** in
  the same region; after: **0** in both appearances).
- *active-row tint present* — accent edge bar found in both: tallest column x=312 with a 22 px vertical
  run (light and dark), plus the faint accent tint on the row.
- *deck band present* — 6,544 wave-toned pixels inside the band (x 414..974, y 64..124) and the band's
  bottom hairline found at row 141 in both appearances; the capsule and `Export…` anchor are visible in
  both.
- home/record/settings backgrounds also pass the near-black/flat-white rules in both appearances.

**Appearance applied live (no restart)** — `build/m9a-e2e/live-apply.sh`: start with `appearance = "light"`,
open Settings, click **Dark**, press **Save** in the same session:

```
INFO suramak::shell: appearance applied setting=Light effective=Light reduce_motion=false
INFO suramak::shell: appearance applied setting=Dark  effective=Dark  reduce_motion=false
settings.toml → appearance = "dark"
```

and the same window's base pixels went `(233,235,240)` → `(14,21,38)` (`live-1-settings-light.png` /
`live-2-after-save.png`).

**Before → after (dark editor, same seed data):** near-black transcript bars (38k px of `gray(10)`) →
0; no accent edge bar → 22 px edge bar; fake-but-present strip → deck band with capsule/readout/Export;
export band under the pill → anchored sheet (pill never overlaps a control).

---

## 3. Deviations (each with its why)

1. **Export panel → anchored export sheet.** The workorder names the pill/export overlap as a defect and
   pins the pill's position (M8.5 contract), so the export UI could not stay a bottom band. The deck's
   `Export…` button now opens a card anchored under itself (chips + destination + Save as… + primary
   Export), and it closes on outside click / leaving the editor. This is the one composition change
   beyond the workorder's letter; it is required by the collision audit.
2. **Strip ruler ticks dropped.** The old `HH:MM:SS` tick row was a named copy-hygiene defect (five time
   formats). The deck keeps the waveform + playhead + click-to-seek; the transport readout is the only
   clock in the header (brief principle 7).
3. **Delete affordance kept, restyled.** Workorder §4 requires the 🗑 ✔ ✖ set to be replaced by painted
   glyphs, so the trash + in-place confirm lives on the title line (quiet tertiary glyphs, tooltips) —
   the comp's title line shows only Polish, but removing the affordance would be a regression.
4. **`SURAMAK_SMOKE_RECORD` harness (main.rs, smoke modes only).** Linux has no capture, so the protected
   Record composition could not otherwise be captured headlessly. With the env var set *and* a smoke mode
   active, `AppState::seed_demo_recording()` seeds a frozen-but-ticking demo session (shaped exactly like
   a real one). It starts no capture session, touches no pipeline, and is inert in normal runs.
5. **`⌃ Home` replaces `Back` everywhere.** The critique's P1-4 asks for one navigation pattern; the
   unified control says `Home` (the destination) in the same position on every screen, including
   settings/models (which previously had `Back` in a bottom action row).
6. **Record view's `Stop` is now secondary tier.** The critique's "two stop affordances of equal weight"
   is resolved by weight (the pill is the primary record/stop affordance). Position and copy unchanged.
7. **macOS paths are compile-checked, not executed.** `suramak-glass` cross-checks for
   `aarch64-apple-darwin`; the macOS branch of `theme.rs` was cross-checked by temporarily flipping its
   `cfg` gates on Linux (check clean), then restored. NSAppearance pinning and NSWorkspace Reduce Motion
   need the owner's Mac for runtime verification — flagged here rather than claimed.
8. **Windows `set_appearance` is a no-op** (documented in `windows.rs`), consistent with that file's
   compile-checked-only status; the token ladder still carries light/dark there.
9. **`NSWorkspace` feature added to `objc2-app-kit`** in `suramak-glass` — an existing dependency, no new
   crates. (`serde`/`toml` etc. unchanged.)
10. **Appearance applies on Save**, not per keystroke — the settings screen's existing Save model; "live"
    means no restart (proven above). Refresh of Reduce Motion happens on the same events.
11. **Copy sweep deferred** (M9e owns it): the record view still shows `current recording: N of the last
    96 meter samples`, the record clock `mm:ss.t` and the pill `mm:ss` remain alongside the two sanctioned
    formats, and `engine: … planned …` stays. Nothing in this slice invented new copy.
12. **Settings/Models still lay out with the M3 y-cursor helpers** (now token-driven) — their composition
    was out of scope; only the nav/control system moved.

## 4. En route (for slice 2)

- The strip's silhouette is still deterministic; real peaks need decoded-sample access in the UI (M9d).
  The geometry (band, capsule, readout, export anchors) is final and unit-tested, so peaks can drop in.
- Transcript rows remain per-row `TextEdit`s; virtualization for long transcripts is still open (heat
  budget / M9d). Row motion is paint-time only, so it will not fight virtualization.
- `controls.rs` is the place to grow if M9b/M9c want painted checkbox/slider/combobox equivalents — the
  record view and settings still use themed stock egui widgets (they now wear the ladder).
- accesskit is still not enabled anywhere (M9e); the painted controls define a focus state and ring, but
  Tab reachability/accessibility nodes remain M9e's floor.
- The route fade uses a child-UI offset rather than a layer transform: the pane shares
  `LayerId::background()` with the chrome, so `transform_layer_shapes` cannot address it alone. If slice 2
  wants transforms (or per-element scale), the pane should get its own layer first.
- Reduce Motion has no Linux/Windows source yet (returns false); if the VPS screenshots ever need it, the
  settings file is the natural seam.
- The export sheet is a plain `Area`; a keyboard path (Esc to close, ⌘E) is M9e's keyboard map work.
