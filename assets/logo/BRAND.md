# Suramak brand

The mark: **"Speak to Text (A3 — Bars + Live Record Dot)"** (updated 2026-10-14) — a
system-style squircle tile whose cutouts run 3 vertical waveform bars → 3 horizontal
text rules + an inline signal-red record dot at the end of the third line.

## Palette (flat master)

| role | hex | rgb | approx CMYK |
|---|---|---|---|
| Ink — tile | `#111111` | 17, 17, 17 | 0/0/0/93 |
| Signal — waveform | `#FF453A` | 255, 69, 58 | 0/73/77/0 |
| Paper — text rules / reversed tile | `#FFFFFF` | 255, 255, 255 | 0/0/0/0 |

App neutrals referenced by the theme twins: pane `#0b1228`, paper bg `#F4F1EA`.
The signal red matches the app's record control (`#FF453A`) — one red, one meaning.

## Masters — `assets/logo/brand/` (hand-kept vector)

- `mark.svg` — primary: ink tile, signal bars, paper rules.
- `mark-reversed.svg` — for dark surfaces (white tile, ink rules).
- `mark-mono.svg` — knockout: tile + real holes, inherits any backdrop.
- `mark-bleed.svg` — tile edge-to-edge; the source for all app icons.
- `wordmark.svg` — “Suramak”, Inter wght 600 outlined (OFL — licenced for logo use;
  Apple system fonts are not).
- `lockup-h.svg` / `lockup-h-reversed.svg` — horizontal lockups.

## Generated — never hand-edit; run `scripts/build_brand.py` (PIL venv at `~/.venvs/brand`)

- `icons/macos/AppIcon.iconset/` → `.icns` via `scripts/make-app.sh` on macOS (iconutil).
- `icons/windows/suramak.ico` (+ PNGs), `icons/linux/*.png`, `icons/web/favicon.*`.
- `icons/readme-logo.png`, `build/theme-matrix.png`, `build/icon-strip.png`.
- The script verifies every output (IHDR dimensions + colour probes) and fails loudly.

## Rules

- Clear space ≥ one bar-width (≈16/256 of the tile) around the mark or lockup.
- Minimum sizes: mark 24 px (32 px preferred); lockup ≥ 96 px wide; below 24 px use `mark-mono`.
- On dark surfaces always use the reversed or mono variant — the ink tile sinks on the app's pane (1.1:1).
- Don't recolour the signal (it is the app's record red), don't add effects to the flat master,
  don't re-set the wordmark in another face.
- Wordmark specifics: Inter (SIL OFL 1.1), instanced `wght=600`, outlines only, no kerning applied.

## Follow-ups

- macOS 26 Liquid Glass icons: evaluate **Icon Composer** for default/dark/tinted layered variants;
  the flat `.icns` displays correctly today.
- Optional optical touch-ups to the wordmark if it should be “not typeable” as-is.
