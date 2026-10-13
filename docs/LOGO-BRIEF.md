# LOGO BRIEF — Suramak app mark (concepts round)

Product: **Suramak** — local-first transcription app (English + Indonesian), macOS-first, Rust/native (repo README for context). The approved visual world: **"system-zenith OS citizen"** — a macOS Tahoe app at Apple's own level: real Liquid Glass, system typography, one signature object (the waveform scrubber with its playhead dot), calm and precise. Pixels: `.impeccable/mocks/decision/C-system.png`.

## The mark's job (binding)

- Read as **TRANSCRIPTION** at a glance: audio/speech **becoming text**. The category read must be literal — an abstract mark that doesn't say the category is rejected.
- Belong to the system world: precise geometry, SF-ish calm, monochrome-friendly, flat, no illustration.
- Survive **16px**: chunky simple silhouette, ≤7 shapes, no detail thinner than 8/256.
- **Not**: a microphone clip-art; a bars-only equalizer (reads music/analytics); blue corporate abstract; a lone speech bubble (reads chat); a memo-recorder (must include the text half).

## Category intelligence (from the design library, 1400+ logos)

- productivity-collaboration: abstract 26% / combination 20% / letterform 18.5%; colour mostly multi 36% + blue 26% → the category is abstract and blue. Stand apart with precision + monochrome (+ ONE signal accent in the later colour round).
- Sound marks that work: Spotify (three arcs in a circle — "minimal, instantly readable"); SoundCloud (sound bars); Graylog (waveform inside a ring).
- Territory to claim: **the "sound → text" transition** — rare in the category.

## Build these three concepts (all greyscale, one fill colour, transparent background, `viewBox="0 0 256 256"`)

### A — "Speak → Text" (negative-space tile)
A solid rounded-square (squircle) ~208px across, with REAL negative-space holes (`fill-rule="evenodd"` subpaths) forming 6–7 **vertical** bars: left bars **tall** (waveform amplitudes), right bars **short and uniform** (text lines) — amplitude collapsing into lines = speech becoming text. Bar width ≥16px, gaps ≥18px, edge inset ≥28px, tile corner radius ≈46px. One path (outline + holes). Drop rule idea: below 32px, the shortest bars may merge — state which survive.

### B — "The Trace S" (monoline letterform)
ONE continuous monoline trace drawing an "S" whose amplitude oscillates like a seismograph/waveform — the recorder that draws the brand's letter. Uniform stroke ≈20px (round caps), deviation ≈±34px around the S spine, 3–5 oscillations, calm and precise. Deliver as **expanded filled geometry** (do NOT rely on stroke attributes).

### C — "Playhead & Caption" (geometric composition)
Horizontal, one colour: 5 vertical amplitude bars (left) → short lead line → a filled **playhead dot** (r≈22) → straight line continuing right; BENEATH the right half, 3 short horizontal **text rules** (unequal lengths, height ≈12, gap ≈16) like justified text. Left half = audio, right half = text, joined by the playhead. Bars/rules ≥12px thick. State which elements drop below 24px.

## Output contract — write EXACTLY these files

- `assets/logo/concepts/a-speak-to-text.svg`
- `assets/logo/concepts/b-trace-s.svg`
- `assets/logo/concepts/c-playhead-caption.svg`
- `assets/logo/concepts/NOTES.md` — per concept: one-line idea, construction summary (coordinates), small-size drop note.

Rules: pure SVG; `viewBox="0 0 256 256"`; single colour `#111111` (or `currentColor`); **no** `<text>`, filters, masks, gradients, stroke-only geometry (expand strokes to fills); valid XML (escape stray `&`); each file renders standalone. Then STOP — do not touch any other files.
