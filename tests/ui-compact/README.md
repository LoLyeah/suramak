# tests/ui-compact — the phone-viewport suite (M13g1, gated in M13g2, extended in M13g3)

The compact profile's CI-side evidence. `run.sh` renders the **real egui shape
stream** of the shared Deck compact screens at the plan's portrait viewport and
writes the shots below. Since M13g2 it runs as a named step in the linux CI job
(`UI compact (phone viewport, Xvfb)`), so a layout regression is a red build.

```sh
tests/ui-compact/run.sh
```

| shot | screen | what it proves |
|---|---|---|
| `shots/launch-library.png` | Library | **the launch surface** (M13g3, redrawn in M20C): serif title at the title token, gear, frosted magnifier search, type-scaled rows, floating dock — **no model banner** (a verified model is in use) |
| `shots/library.png` | Library | the normal launch: the same chrome, three rows, the dock's Import (secondary) + record-red Record |
| `shots/library-light.png` | Library | the same layout on the light token ladder |
| `shots/library-empty.png` | Library | the empty state: 28 pt waveform, Newsreader 21 pt "No transcripts yet", the two-line prompt |
| `shots/first-run-download.png` | Library | **the first-run release path**: ONE shrink-wrapped banner — 159 pt, M13D's recommendation, one model pill + Download, free-space caption, `tiny` absent |
| `shots/download-progress.png` | Library | the job's own percent on the card (the notification's number) + Pause |
| `shots/download-resume.png` | Library | the `.part` resume offer at the same percent after a relaunch |
| `shots/onboarding.png` | Library + sheet | the one-time intro: three paths, Start, Skip, on the Sheet plane + full slide |
| `shots/library-bottom.png` | Library | **M20C dock clearance**: 14 rows scrolled to the end — the last row ends clear above the floating dock (list bottom padding = gap + dock + gesture inset + 16) |
| `shots/library-model-in-use.png` | Library | **M20C banner absence**: `base` verified on disk → no banner, the list starts at the search field |
| `shots/library-model-choice.png` | Library | a verified model that is **not** the configured one: Record is disabled and a reason chip sits above the dock |
| `shots/diagnostics.png` | Settings → Diagnostics | the four check groups + the v2.4 crash banner (Clear / Share log) |
| `shots/editor.png` | Editor | back, title, meta line, selected segment with **M15.6 word chips**, transport + Export |
| `shots/settings.png` | Settings | transcription card, M13D model tiers (offered only), appearance chips, device card, Diagnostics, Quit |
| `shots/settings-bottom-inset.png` | Settings | **M19A:** the same screen scrolled to its end on a 12 GB phone with a synthetic 54 px gesture inset — the DEVICE card and Quit are above the pill, with the inset + gutter as end padding |
| `shots/export-sheet.png` | Editor + sheet | six format chips (md/txt/json/srt/vtt/tsv), the bilingual switch, both export routes disclosed |
| `shots/export-written.png` | Editor + sheet | the picker result path: **Saved** with the name, size and MIME |
| `shots/settings-tiers.png` | Settings | the M13D low-tier statement: only `tiny` offered below the 4 GB floor, 5 models hidden |

Viewport: **1216 × 2640 physical at scale 2.6** (467.7 × 1015.4 points), the
M13 plan's portrait phone. The second test renders 360 × 800 dp for the
hit-target assertions.

## What is asserted (machine)

* every action target is ≥ 48 dp and inside the viewport;
* the floating dock clears a 20 pt gesture inset, never overlaps its buttons, and the list pads its bottom past the dock + inset (asserted for insets 0/20/54);
* the three routes are reachable by the push/pop stack and a notification deep
  link lands on Library → Editor;
* Reduce Motion removes the navigation **travel** and keeps exactly one 120 ms
  opacity crossfade; it never leaves a frame loop running;
* a settled screen with a live job requests no repaint (the battery rule);
* every compact control registers a non-empty semantic label and a role
  (Button / RadioButton / Checkbox / Label) — the a11y floor;
* the export sheet's open state and its picker-result state render at the phone
  viewport, and the sheet's six formats are the desktop serializers' set;
* each shot is non-blank and exactly the requested size;
* the light and dark ladders both render;
* **M20C:** the app-bar title is the type scale's title token in the serif, the gear is a 44 × 44 pt "Settings" control, and the Record fill clears 4.5:1 with its ink on both ladders;
* **M19A:** the Settings rows move with the scroll offset (offset 0 is
  pixel-identical to the pre-M19A render), a 10 pt wobble is still a tap, a
  100 pt drag on a row scrolls without toggling it, a cancelled touch never
  clicks nor leaves the pointer down, and the DEVICE note ends above a 54 px
  gesture inset with the inset + gutter as end padding.

## What is NOT asserted here (the honest boundary)

Touch delivery, fling feel, IME resizing and the system back gesture are
**owner-device** checks. This suite proves layout and state on the host; it
cannot prove how Android routes a finger. The fling probe records the gesture
so the owner run has numbers; the deferral is stated in
`docs/reports/M13g2-compact-completion.md` §7.

The PNGs are review artifacts, not pixel goldens: the rasteriser is stable, but
CI gates on the geometry assertions so a font-hinting difference on a runner
cannot turn the suite red for the wrong reason.
