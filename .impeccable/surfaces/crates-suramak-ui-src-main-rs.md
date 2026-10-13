---
version: 1
slug: "crates-suramak-ui-src-main-rs"
primary_target: "crates/suramak-ui/src/main.rs"
related_targets: []
---

# Surface brief — Suramak main window

Primary target: `crates/suramak-ui/src/main.rs` (the app's single main window: recording library + transcription view). Visitor mode: **Operate**.

**Audience & job**: the owner (later, Indonesian users too) transcribing English/Indonesian audio fully on-device — drop a file or hit record, watch the transcript assemble, edit it, export md/txt/json or polish with their own LLM. Proof content: real en+id transcripts; the local-first promise is the product's truth.

**Chosen direction** (approved comp: `.impeccable/mocks/decision/C-system.png`): **the system-zenith OS citizen** — a macOS Tahoe app at Apple's own level of material and restraint. The world enters through the OS's real materials (AppKit Liquid Glass), system typography (SF Pro / New York / SF Mono), and one dominant move: a luminous glass window whose waveform scrubber is the signature object, with the transcript as the reading surface. Memorable moment: watching the waveform scrub while text materializes under the playhead.

**Constraints**: real AppKit materials only — never a painted fake of glass; true macOS controls; unified window chrome; the comp's iOS-style floating pill becomes an integrated native toolbar. Comp is the north star; exact text/behavior are implementation responsibilities.

## Direction contract

THESIS: the transcription app that looks and behaves like an Apple system app; refuse the third-party dashboard (sidebar + gray pane + waveform header) and every painted-glass skin.
OWN-WORLD: real AppKit materials (NSGlassEffectView, NSVisualEffectView fallback), the system palette, SF Pro / New York / SF Mono; recognizable native controls; translucency only where the OS provides it.
STORY: the user opens the app, drops audio or records, watches the waveform scrub while the transcript assembles; edits lines; exports or polishes. Local by default; network only on explicit action.
FIRST VIEWPORT: the app window — translucent sidebar of recordings at left; main pane with a serif title, a wide waveform scrubber strip with fine time ticks and a slim playhead, the transcript reading column beneath; record / play / export discoverable at the top chrome, all macOS-native.
FORM: grounded candidate #1 of 7 (Control-Center-grade OS citizen); seed key **b7306df5**.
FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance.

**Unresolved decisions**: transcript reading face (New York vs SF) settled at M1 by rendering both; sidebar material intensity (regular vs clear glass) chosen on the real Mac; toolbar integration point (top chrome vs accessory) per Tahoe conventions.
