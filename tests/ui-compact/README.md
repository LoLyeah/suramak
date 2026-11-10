# tests/ui-compact — the phone-viewport suite (M13g1, gated in M13g2)

The compact profile's CI-side evidence. `run.sh` renders the **real egui shape
stream** of the shared Deck compact screens at the plan's portrait viewport and
writes the shots below. Since M13g2 it runs as a named step in the linux CI job
(`UI compact (phone viewport, Xvfb)`), so a layout regression is a red build.

```sh
tests/ui-compact/run.sh
```

| shot | screen | what it proves |
|---|---|---|
| `shots/library.png` | Library | app bar, search field, three rows with metadata + chevrons, Import/Record bar |
| `shots/library-light.png` | Library | the same layout on the light token ladder |
| `shots/editor.png` | Editor | back, title, meta line, selected segment, play marks, transport + scrubber + Export |
| `shots/settings.png` | Settings | transcription card, M13D model tiers (offered only), appearance chips, device card, Quit |
| `shots/export-sheet.png` | Editor + sheet | six format chips (md/txt/json/srt/vtt/tsv), the bilingual switch, both export routes disclosed |
| `shots/export-written.png` | Editor + sheet | the picker result path: **Saved** with the name, size and MIME |
| `shots/settings-tiers.png` | Settings | the M13D low-tier statement: only `tiny` offered below the 4 GB floor, 5 models hidden |

Viewport: **1216 × 2640 physical at scale 2.6** (467.7 × 1015.4 points), the
M13 plan's portrait phone. The second test renders 360 × 800 dp for the
hit-target assertions.

## What is asserted (machine)

* every action target is ≥ 48 dp and inside the viewport;
* the bottom bar clears a 20 pt gesture inset and never overlaps its buttons;
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
* the light and dark ladders both render.

## What is NOT asserted here (the honest boundary)

Touch delivery, fling feel, IME resizing and the system back gesture are
**owner-device** checks. This suite proves layout and state on the host; it
cannot prove how Android routes a finger. The fling probe records the gesture
so the owner run has numbers; the deferral is stated in
`docs/reports/M13g2-compact-completion.md` §7.

The PNGs are review artifacts, not pixel goldens: the rasteriser is stable, but
CI gates on the geometry assertions so a font-hinting difference on a runner
cannot turn the suite red for the wrong reason.
