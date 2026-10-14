# M9 — UI remake brief

Craft overhaul *inside* the settled world (C-system, "OS citizen"). The glass material, the floating record pill, and the on-device honesty stay; the layout system, motion, controls, and surfaces get rebuilt to Apple-level craft.

- Evidence: `docs/reports/M9-CRITIQUE-A.md` (Assessment A, isolated reviewer; native path — detector n/a by design). Score 22/40 severity load, 2 P0s, 3 P1s, measured repaint heat.
- Owner's words (the brief's why): "the layout, no animation and transition, and some weird button styling choice"; "the software makes my Macbook Air M4 runs hot."

## Principles (adopted from the critique)

1. **The transcript is the product.** Design from the reading surface outward: a typeset reading column (or structured list) as the first-class citizen; the sidebar is a quiet rail; nothing competes with the text.
2. **ONE material ladder, ≤3 levels.** Glass window → one translucent panel → one card recipe (single hairline + soft shadow). Themed egui `Visuals` (panel/window/widget/bg fills, strokes, rounding, shadows) so no stock near-black or flat-white fills ever appear. Kill the three-surface patchwork.
3. **ONE control system, three tiers** (primary pill/transport · secondary labeled · tertiary icon). Every state defined: rest/hover/active/focus/disabled. Painted glyphs only — no emoji/unicode glyph controls. Dead affordances are wired or removed.
4. **Motion token set.** Fast 120 ms (hover/press) · base 180 ms (state swaps) · slow 240 ms (route changes), ease-out only; honor Reduce Motion. Route changes, pane changes, pill state morphs, live-line arrivals, active-segment changes all animate — nothing teleports.
5. **Repaint budget.** Animation cadence decoupled from "busy": recording = throttled cadence for meter/timer only; playback ≤30 fps; idle = on-demand. Instrument frames/CPU before/after (owner heat complaint has a measured UI component — 7,733 full-window frames in 240 s while busy).
6. **The waveform is real.** Peaks computed from decoded samples. Where there is no audio yet, a quiet baseline — never a fake waveform.
7. **Copy hygiene.** One time-format system (elapsed hh:mm:ss for recordings · mm:ss.mmm for transport). Human model names. Plain words, no internal jargon, no quant-suffix strings on screen.
8. **Accessibility floor.** accesskit enabled (screen reader), visible focus rings, contrast checked on the ladder in both appearances, full keyboard path (⌘R record, space play/pause, ⌘E export, ↑/↓ segment nav, ⌘⌫ delete with undo, Esc back).

## Protect list (no regressions)

- Real `NSGlassEffectView` window material + the floating record pill (bottom-center, own cast shadow, honest fallback).
- Error/status copy quality (permission guidance, codec messages, per-app no-match).
- Record view information order + "live transcript — provisional · the final pass runs on Stop" honesty.
- Playback model: click-to-seek, timestamp seek, follow-only-while-playing.
- Single-instance guard, clean quit, model manager flows.

## Comps (this round)

`.impeccable/mocks/m9/{A-reading-room,B-instrument,C-deck}.png` — three compositions of the remade editor in the same world. The pick (or a stated mix) becomes the build target for the editor shell.

- **A — Reading Room**: transcript as a typeset document; slim strip; quiet top-right actions.
- **B — Instrument**: waveform as an elevated hero card with a transport capsule; transcript as a structured list.
- **C — Deck**: one integrated control deck under the titlebar (transport + waveform + readout + export in one band); transcript as clean rows.

Common to all three: light appearance (appearance decision pending), one surface system, the pill bottom-center, real content.

## Open decisions (owner)

1. **Composition**: A, B, C, or a stated mix ("C's deck + A's reading column" etc.).
2. **Appearance**: follow the system (author the ladder in light + dark) — recommended, it is what a Tahoe OS-citizen does — vs pinned light vs pinned dark.
3. Anything the comps dropped that must stay.

## Build plan (post-pick)

- **M9a** layout system + material ladder + themed egui Visuals (kills the patchwork; tokens in `theme.rs`).
- **M9b** control system + reading surface (gutter, active segment, hover; text-edit backgrounds transparent; no stock fills).
- **M9c** motion token set + route/state transitions + Reduce Motion.
- **M9d** real waveform peaks + repaint/heat budget (instrument before/after).
- **M9e** accessibility floor (accesskit, focus, keyboard map, tooltips) + copy sweep.

Each step: workorder → delegated implementation → independent verification (tests + xvfb/pixel + mac smoke). Release coupling: with the owner (folds into the next cut, likely 0.2.0 if it lands after v0.1.1).
