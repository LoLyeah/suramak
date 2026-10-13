# LOGO ROUND 2 — fixes (same three files, overwrite in place)

Round 1 was rendered and critiqued. Findings + exact fixes below. Keep: monochrome `#111111`, `viewBox="0 0 256 256"`, pure expanded-fill geometry (no stroke attrs), no `<text>`, valid XML. All curves as **smooth cubics (C commands)** — no L-only zigzags on curved forms.

## A — overwrite `a-speak-to-text.svg` (round 1: polished, but "text lines" read as an ellipsis)

Problem: three short VERTICAL pills read as "…" / menu dots, not text.
Fix — make the right half read as TEXT by changing orientation:
- Keep the squircle tile (24..232, r=46).
- LEFT zone (x 44..146): 3 **vertical** waveform pills, width 16, gap 14, centered on y=128, heights: 72, 116, 88 (middle tallest).
- RIGHT zone (x 160..214): 3 **horizontal** text rules stacked, height 16, gap 14, left-aligned at x=160, widths: 54, 54, 38 (last one short = ragged line end). Vertically centered: rules' total height = 16*3+14*2 = 76 → y 90..166.
- Everything still one evenodd path (tile outline + 6 holes). No other changes.

## B — overwrite `b-trace-s.svg` (round 1: oscillation kills the letterform; reads as scribble)

Problem: ±34px amplitude over the whole spine = sharp zigzags, no letter.
Fix — letterform FIRST, trace second:
- Draw a CLEAN geometric monoline "S": top hook sweeping from upper-right (terminal ≈(172, 72)) over the top to the left inflections (≈(80, 98)), diagonal down to lower-right inflection (≈(176, 158)), bottom hook to terminal ≈(84, 184). Timeless S proportions, like a system-font S skeleton.
- Stroke: uniform 20px, round caps, expanded to fill geometry.
- ONLY the middle diagonal segment (between the two inflections, x≈92..164) carries ONE smooth waveform: amplitude ±13px perpendicular to the diagonal, exactly 1.5 periods (crest–trough–crest→end). Everything else stays perfectly clean.
- Acceptance: at 100px, a stranger reads an "S" instantly; on second look the spine has a visible trace character. No corner sharper than ~120°. Anchor count ≤ 120.

## C — overwrite `c-playhead-caption.svg` (round 1: three separate icons, fails 16px)

Problem: lead line + continuation line fragmented the composition; too many thin elements.
Fix — TWO masses, mirroring the app UI (scrubber over transcript):
- TOP mass (waveform + playhead, baseline y=88): 6 vertical pills, width 14, gap 12, x from 40 (40..184), heights centered on y=88: 48, 80, 120, 96, 64, 44. Then the **playhead dot**: filled circle r=20 centered (212, 88) (spans 192..232, gap 8 after the last pill). Dot sits on the same baseline — it is the end of the scrubber run.
- BOTTOM mass (transcript): 3 **full-bleed two-column-wide** horizontal rules starting at x=40, height 16, vertical gaps 16, widths 176, 148, 112, top rule starts y=142.
- Remove: all lead/continuation lines from round 1. Exactly 10 shapes (6 pills + dot + 3 rules) in one path.
- Keep margins ≥24 on all sides (shift the whole composition if needed; optical centering).
- Small-size story: at 16px the top mass reads as one bold dash; the three rules read as a text block. No element thinner than 12px at 256 scale.

## After writing

Update `NOTES.md`: per concept, replace the construction section with the round-2 numbers + one line on what changed and why. Run `ls -la` on the folder, print it, then STOP. Do not touch other files.
