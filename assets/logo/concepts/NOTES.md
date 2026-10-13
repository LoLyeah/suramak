# Suramak Logo Concepts — Design Notes (Round 2)

Round: Round 2 Fixes (greyscale, single fill `#111111`, `viewBox="0 0 256 256"`).
Category: Local-first native transcription (macOS Tahoe system citizen, Apple-level precision).

---

## Concept A — "Speak → Text" (`a-speak-to-text.svg`)

- **One-line idea:** A solid macOS squircle tile featuring vertical audio waveform amplitudes on the left collapsing into horizontal transcript text lines on the right — the literal, unmistakable sound-to-text transformation.
- **Round 2 change:** The three right-side pills were reoriented from vertical to horizontal stacked text rules with a ragged line end, eliminating the "ellipsis / menu dots" reading and creating an instant audio-to-text semantic read.
- **Construction summary:**
  - Outer container: 208×208 px squircle tile (`x: 24..232`, `y: 24..232`), corner radius $r = 46\text{ px}$.
  - Negative-space cutouts: 6 cutouts via `fill-rule="evenodd"` (3 vertical pills + 3 horizontal rules).
  - LEFT zone (Waveform, `x: 44..120`): 3 vertical pills, width $= 16\text{ px}$, gap $= 14\text{ px}$, centered vertically on $Y = 128$:
    - Pill 1: `cx = 52` (`x: 44..60`), height $= 72\text{ px}$ (`y: 92..164`), $r = 8\text{ px}$.
    - Pill 2: `cx = 82` (`x: 74..90`), height $= 116\text{ px}$ (`y: 70..186`), $r = 8\text{ px}$ (crest amplitude).
    - Pill 3: `cx = 112` (`x: 104..120`), height $= 88\text{ px}$ (`y: 84..172`), $r = 8\text{ px}$.
  - RIGHT zone (Text rules, `x: 160..214`): 3 horizontal rules stacked, height $= 16\text{ px}$, vertical gap $= 14\text{ px}$, left-aligned at $X = 160$, vertically centered ($Y: 90..166$):
    - Rule 1: `x: 160..214` (width $= 54\text{ px}$), `y: 90..106`, $r = 8\text{ px}$.
    - Rule 2: `x: 160..214` (width $= 54\text{ px}$), `y: 120..136`, $r = 8\text{ px}$.
    - Rule 3: `x: 160..198` (width $= 38\text{ px}$), `y: 150..166`, $r = 8\text{ px}$ (ragged paragraph end).
  - Single path, 0 strokes, 45 clean geometric anchors.
- **Small-size drop note:**
  - At $\ge 32\text{ px}$: Both zones remain distinct and legible.
  - Below $32\text{ px}$ (e.g. $16\text{ px}$): The vertical waveform bars retain their height contrast while the 3 horizontal text rules optically solidify into a neat paragraph block.

---

## Concept B — "The Trace S" (`b-trace-s.svg`)

- **One-line idea:** A clean, timeless monoline "S" whose middle diagonal spine carries a subtle, precise seismograph waveform — letterform first, recorder trace second.
- **Round 2 change:** Tamed the previous excessive amplitude across the entire letter into a pure system-font "S" skeleton, isolating the waveform strictly to the central diagonal segment with an amplitude of $\pm 13\text{ px}$ and exactly 1.5 periods (crest–trough–crest). All curves built with smooth cubic Beziers (`C` commands), anchor count reduced from 194 to 40.
- **Construction summary:**
  - Geometry: Single continuous expanded monoline stroke (pure `<path>`, no stroke attributes).
  - Stroke weight: Uniform $20\text{ px}$ thickness with outward semicircular end caps ($r = 10\text{ px}$).
  - S skeleton:
    - Top hook: Terminal at $(172, 72)$ sweeping over top crest $(126, 42)$ to left inflection $(80, 98)$.
    - Diagonal segment: Connects $(80, 98) \to (176, 158)$, carrying exactly 1.5 periods (crest–trough–crest $\to$ end) with amplitude $\pm 13\text{ px}$ perpendicular to the diagonal ($T \approx (0.848, 0.530)$, $N \approx (-0.530, 0.848)$).
    - Bottom hook: Sweeps from right inflection $(176, 158)$ through bottom trough $(130, 214)$ to terminal at $(84, 184)$.
  - Boundaries: Margins $L = 69.2$, $R = 69.3$, $T = 32.0$, $B = 32.0$.
  - 40 anchor points, 0 corners sharper than 120°.
- **Small-size drop note:**
  - At $100\text{ px}$: Reads instantly as an authoritative "S", with the active voice-trace modulation visible upon closer inspection.
  - At $16\text{ px}$: The $20\text{ px}$ uniform stroke weight preserves the bold "S" silhouette without clutter or fill-in.

---

## Concept C — "Playhead & Caption" (`c-playhead-caption.svg`)

- **One-line idea:** Two balanced visual masses mirroring the native macOS app UI: an audio scrubber strip ending in a prominent playhead dot above full-bleed transcript lines.
- **Round 2 change:** Removed all fragmented lead and continuation wireframe lines, reorganizing the mark into two cohesive masses (TOP scrubber mass + BOTTOM transcript mass) with exactly 10 solid shapes and optical centering ($L=32, R=32$).
- **Construction summary:**
  - TOP mass (Scrubber & playhead, baseline $Y = 88$):
    - 6 vertical amplitude pills ($W = 14\text{ px}$, gaps $= 12\text{ px}$, $X: 32..176$):
      - Pill 1: `cx = 39`, height $= 48\text{ px}$ (`y: 64..112`)
      - Pill 2: `cx = 65`, height $= 80\text{ px}$ (`y: 48..128`)
      - Pill 3: `cx = 91`, height $= 120\text{ px}$ (`y: 28..148`) (peak amplitude)
      - Pill 4: `cx = 117`, height $= 96\text{ px}$ (`y: 40..136`)
      - Pill 5: `cx = 143`, height $= 64\text{ px}$ (`y: 56..120`)
      - Pill 6: `cx = 169`, height $= 44\text{ px}$ (`y: 66..110`)
    - Playhead dot: Filled circle $r = 20\text{ px}$ centered at $(204, 88)$ ($X: 184..224$, gap $= 8\text{ px}$ after Pill 6).
  - BOTTOM mass (Transcript reading column):
    - 3 full-bleed horizontal rules starting at $X = 32$, height $= 16\text{ px}$, vertical gaps $= 16\text{ px}$, corner radius $r = 8\text{ px}$:
      - Rule 1: `x: 32..208` (width $= 176\text{ px}$), `y: 142..158`
      - Rule 2: `x: 32..180` (width $= 148\text{ px}$), `y: 174..190`
      - Rule 3: `x: 32..144` (width $= 112\text{ px}$), `y: 206..222`
  - Margins: $L = 32.0$, $R = 32.0$, $T = 28.0$, $B = 34.0$ (all $\ge 24\text{ px}$).
  - Exactly 10 shapes (6 pills + 1 circle dot + 3 rules) in 1 path, 54 anchors.
- **Small-size drop note:**
  - At $\ge 32\text{ px}$: All 6 audio pills, the playhead dot, and all 3 transcript rules remain clear and distinct.
  - At $16\text{ px}$: The top mass reads as a solid scrubber gesture with a terminating dot; the bottom mass reads as an aligned transcript block. No feature is thinner than $14\text{ px}$ at 256 scale.
