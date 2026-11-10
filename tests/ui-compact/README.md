# tests/ui-compact — the phone-viewport suite (M13g1)

The compact profile's CI-side evidence. `run.sh` renders the **real egui shape
stream** of the shared Deck compact screens at the plan's portrait viewport and
writes the shots below.

```sh
tests/ui-compact/run.sh
```

| shot | screen | what it proves |
|---|---|---|
| `shots/library.png` | Library | app bar, search field, three rows with metadata + chevrons, Import/Record bar |
| `shots/library-light.png` | Library | the same layout on the light token ladder |
| `shots/editor.png` | Editor | back, title, meta line, selected segment, play marks, transport + scrubber + Export |
| `shots/settings.png` | Settings | transcription card, M13D model tiers (offered only), appearance chips, device card, Quit |

Viewport: **1216 × 2640 physical at scale 2.6** (467.7 × 1015.4 points), the
M13 plan's portrait phone. The second test renders 360 × 800 dp for the
hit-target assertions.

## What is asserted (machine)

* every action target is ≥ 48 dp and inside the viewport;
* the bottom bar clears a 20 pt gesture inset and never overlaps its buttons;
* the three routes are reachable by the push/pop stack and a notification deep
  link lands on Library → Editor;
* Reduce Motion settles the push on the first frame (no frame is requested);
* each shot is non-blank and exactly the requested size;
* the light and dark ladders both render.

## What is NOT asserted here (the honest boundary)

Touch delivery, fling momentum, IME resizing and the system back gesture are
**owner-device** checks. This suite proves layout on the host; it cannot prove
how Android routes a finger. The deferral is stated in
`docs/reports/M13g1-compact-library.md` §7.

The PNGs are review artifacts, not pixel goldens: the rasteriser is stable, but
CI gates on the geometry assertions so a font-hinting difference on a runner
cannot turn the suite red for the wrong reason.
