# M9-CRITIQUE-A — Suramak current UI: the honest defects list that drives the M9 remake

**Method: native target — Assessment A by an isolated reviewer; detector n/a.** The bundled impeccable detector reads HTML/CSS only and has no verdict on native code (Rust/egui/AppKit here) — this is the documented native path, not a detector failure.

**Evidence.** The owner's macOS screenshots — S1 (recording view, 07:40: timer `00:20.8`, sources card, live transcript card, floating pill `00:20`), S2 (editor, 07:47: strip with pause glyph, four transcript rows, Export panel, pill over the export row) and a second editor capture (07:37, fixture transcript) — plus the full `crates/suramak-ui/` source and a Linux/Xvfb run of the debug build (layout truth only; no glass on Linux). Every defect from the brief was re-checked; each is marked **verified** (visible in the pixels), **partial**, or **source-proven** (provable in code, not observable in these captures). Nothing here is repeated unverified.

**Scope note.** This is Operate mode on a settled world ("system-zenith OS citizen"). Nothing below reopens the world; the findings are about how far the execution sits from it.

---

## 1. Nielsen's 10 heuristics — scored 0–4

Scale: 0 = no issue · 1 = cosmetic · 2 = minor usability issue · 3 = major usability issue · 4 = catastrophic. Total is a severity load (max 40; lower is better).

| # | Heuristic | Score | Key issue (one per row) |
|---|---|---|---|
| H1 | Visibility of system status | 2 | The status word `recording` on the metering card (12 px mono, `text_dim`) is nearly invisible **verified**; the "level" meter reads as a broken dashed line + a dot **verified**; the card clock `00:20.8` and the pill clock `00:20` disagree in precision; "current recording: 96 of the last 96 meter samples" is raw debug telemetry shown as UI copy. |
| H2 | Match between system and real world | 2 | Developer-speak leaks into the UI ("meter samples", "engine: coreml → planned metal", "16 kHz", sidebar `· tiny` tags); five different time formats on one screen; the recording title `Recording 2026-10-14 07:40:36` is a filename, not a name. |
| H3 | User control and freedom | 2 | No cancel affordance for a running transcription (`Back` exists only on failure); delete is permanent with no undo; Escape is bound to nothing anywhere in the app. |
| H4 | Consistency and standards | 3 | Five control languages coexist — stock egui buttons/chips/checkbox/slider, emoji glyphs (🗑 ✔ ✖), hand-painted glyphs (play/export/transport), a bare 12 px "Home" text link, and the custom pill; the same action is rendered two ways (record view's stock `Stop` button vs the pill's painted stop); navigation is "Home" in the editor, "Back" in Settings/Models, sidebar rows elsewhere. |
| H5 | Error prevention | 2 | Delete confirm and settings validation are good, but the import/export path fields accept anything until the action fails; quitting mid-recording silently discards the recording with nothing in the UI saying so. |
| H6 | Recognition rather than recall | 2 | The titlebar's play and export glyphs are painted decorations with no `Sense` — they look like buttons and do nothing **source-proven** (views.rs `play_glyph`/`export_glyph` are painter-only); zero tooltips in the whole app (`on_hover_text` never used); the trash is an emoji whose meaning depends on font rendering. |
| H7 | Flexibility and efficiency of use | 3 | Exactly one keyboard interaction exists (Enter in the home path field). No Cmd+R / Cmd+E / Cmd+S / Space / Esc, no app menu commands beyond the system Quit, no scrub-drag or arrow-key seek on the strip, no bulk operations, and Home's recent list is hard-capped at 6 rows (`min(6)`). |
| H8 | Aesthetic and minimalist design | 3 | The three-surface patchwork (real glass base + flat translucent washes + stock egui near-black widget fills) reads as a collage of hard-edged rectangles at uneven opacities **verified (S1, S2)**; the transcript sits on near-black bars **verified**; the loudest object on the editor screen is a Record pill parked on the Export panel. |
| H9 | Help users recognize, diagnose, recover from errors | 1 | The strongest system in the app: specific, actionable, in-place messages (audio-tap permission guidance; the WebM codec message with the `ffmpeg` conversion; "no running audio process matches…"; "recording is disabled while a job is running"). Minor: all render as 12 px red captions with no icon or hierarchy, and their position varies per view. |
| H10 | Help and documentation | 2 | No in-app help of any kind: no tooltips, no Help menu, no shortcut sheet, no onboarding, no About. The only guidance is one-line captions ("words appear here while you speak", the polish setup note); the real documentation lives in the repo, not the app. |

**Total: 22 / 40 → Poor (significant rework).** Bands: 0–4 Excellent · 5–9 Good · 10–16 Fair · 17–24 Poor · 25–40 Critical. This is not a polish backlog; it is a remake brief — the owner's "still pretty buggy" is an accurate reading of the pixels.

---

## 2. Design-specificity verdict

**Verdict: authored at the concept level, category-interchangeable in execution.** Swap the copy and this could be any transcription tool wearing a Tahoe skin. The world is Suramak's (glass window, New York serif titles, wide scrubber, live provisional transcript, local-first honesty); almost nothing between concept and pixel is.

The miss, stated sharply:

1. **The two surfaces that should be most "this product" are the least authored.**
   - The **transcript reading surface** — the thing users came for — is a stack of default `egui::TextEdit::multiline` widgets (editor.rs) wearing egui's default dark `extreme_bg_color` = `gray(10)`, i.e. near-black bars, with 15 px New York serif text on them. **Verified (S1/S2): "solid black rectangular backgrounds with white text"; "long opaque black bars that contrast sharply with the translucent window".**
   - The **signature waveform strip** carries no information at all: `bar_height()` (editor.rs) is a deterministic `sin`+hash silhouette, not the audio. The one object the brief calls "the signature" is decorative. (The record view's rolling waveform *is* real RMS; only the editor strip is fake.)
2. **No element has a designed state machine.** Painted controls (pill, transport, rows, glyphs, drop zone) change nothing on hover or press — `response.hovered()` only sets the cursor icon; widget controls change with stock egui styling (teal selection fills, gray button fills, gray-60 strokes). Neither set has a Suramak-specific state language.
3. **The comp's authored details were dropped without equivalents.** C-system.png had source-list rows with waveform thumbnails, a three-control capsule, a light material. The build has flat rows, a one-control pill, and a dark pin — each defensible individually, but the replacements were painted approximations, not designs.

What *is* genuinely authored and should survive: the floating glass pill (material + geometry), the copy system, and the record view's information order. That is the seed stock of the remake.

---

## 3. Cognitive load checklist (8 items)

| # | Check | Result | Evidence |
|---|---|---|---|
| 1 | Primary action is obvious on every screen | **Fail** | Editor: the loudest control is the glass "Record" pill; Export — the screen's purpose — is a stock gray button in a bottom band. Record view: two stop affordances of equal weight (view `Stop` + pill stop). |
| 2 | Choices are limited and grouped | Pass | Screens are sparse; cards group meter/sources/transcript; settings is sectioned; export offers five formats inline. |
| 3 | Control placement is consistent across screens | **Fail** | Record entry = pill; stop = pill + view button; navigation = "Home" text link (editor) vs `Back` buttons (Settings/Models) vs sidebar rows; saving = `Save` button (settings) vs silent autosave (editor). |
| 4 | Labels and affordances need no guessing | **Fail** | Dead titlebar glyphs (play/export do nothing); emoji trash; no tooltips anywhere; "1.00×" gain readout; a tofu glyph rendered raw in a live row. |
| 5 | Every action produces visible feedback | Pass | Status lines, autosave `saved` caption, progress %, engine line, ticking clock — all present (several too dim, but present). |
| 6 | Errors are actionable and sit near the error | Pass | Best-in-app system; messages name the cause and the next step, in place. |
| 7 | Current state is visible (mode, selection, focus) | **Fail** | `recording` status barely legible **verified**; sidebar selected and hovered share one fill (`row_selected`) so selection state is ambiguous; painted controls have no focus state; two clocks at different precision. |
| 8 | No unexplained jargon or debug artifacts | **Fail** | "96 of the last 96 meter samples", "engine: coreml → planned metal", "16 kHz", `· tiny`, raw tofu glyph. |

**Result: 3 pass / 5 fail.** The failures cluster exactly where the remake will work: state language, control system, copy hygiene.

---

## 4. Emotional journey (peak–end)

- **First 10 seconds:** the dark glass window over the wallpaper is genuinely convincing — the material buys hope before anything is read.
- **Early:** Home is calm and honest; the pill invites; "transcribed on this machine, never uploaded" earns trust.
- **Peak (real, working):** press Record → the pill becomes a stop glyph + ticking clock, and words start arriving in the live card under an honest `provisional` label. This is the product's magic moment and it functions today.
- **Descent:** Stop → "finishing…" → the editor. The surface you came for is black bars on glass; the signature strip is fake; the pill parks on your export destination field; nothing in the app has ever moved.
- **End:** the final screen is the most defective screen. Peak–end rules memory: Suramak will be remembered as the editor's roughness, not the live-transcript delight — which is precisely "still pretty buggy".
- **Background anxiety:** autosave is silent (a 12 px `saved` caption), quitting mid-recording silently discards, delete is permanent. The emotional low is not knowing what is safe.

---

## 5. What's working (protect these)

1. **The floating record pill is the one element at the promised level.** Real `NSGlassEffectView` behind exactly the painted pixels (one shared `float_rect` for painter and material, `suramak-glass`), correct bottom-center behavior across resizes, and an honest painted fallback where no material exists. It proves the world is achievable — and it is the app's best object.
2. **The error/status copy system.** Specific, actionable, in-place: the audio-tap permission path, the codec message with the exact `ffmpeg` command, the per-app "no running audio process matches…" case, "recording is disabled while a job is running". This is real product thinking; carry it into M9 verbatim.
3. **The record view's information order and the playback interaction model.** Title → clock/level/waveform → sources → provisional transcript is the right reading order, the "live transcript — provisional · the final pass runs on Stop" labeling is exactly the honesty the product needs, and the editor's playback behaviors are specified better than they are drawn (click-to-seek anywhere on the strip, timestamp-seek per row, active-segment follow **only while playing** so a paused playhead never yanks the reader's scroll).

---

## 6. Priority issues

### P0-1 — The three-surface patchwork (window / panel / compositing craft)

- **What:** Three incompatible surface systems stack on top of each other: (a) the real AppKit glass base; (b) flat translucent white washes per region — chrome 6 %, sidebar 12 %, pane 18 %, card 14 % (`Color32::from_white_alpha` in theme.rs), each a hard-edged rounded rect with a 1 px hairline and no shadow, no edge treatment, no depth; (c) egui's stock dark-theme widget fills on top — `extreme_bg_color = gray(10)` TextEdits, gray widget fills, teal selection chips. Result: every block reads as a hard-edged rectangle at an opacity unrelated to its neighbors — the collage the owner described. **Verified in both screenshots** ("distinct dark translucent panels… uneven opacities"; "hard-edged dark rectangles, with opacities differing from the translucent background"). The only authored shadow in the app is on the fallback pill (stock egui popups bring egui's own).
- **Why it matters:** the settled world's entire promise is "a macOS Tahoe app at Apple's own level of material and restraint". The material was paid for (real glass) and then contradicted by everything drawn on it. This is the owner's core complaint and the reason a remake exists at all.
- **Concrete fix:** define one material ladder with ≤ 3 levels (OS material = the window; one content surface; one card elevation with a single alpha / radius / stroke / shadow recipe). Then theme egui `Visuals` to that ladder — `text_edit_bg_color`, widget fills/strokes, `selection`, popup/window fills — so no stock gray or near-black fill can ever appear. Cards get either real depth (fill + top inner highlight + soft shadow) or get removed in favor of rhythm and spacing. One corner radius everywhere (today: 10 for cards, 6 for rows, 3/1 for glyphs, pill radius elsewhere), one hairline alpha.
- **Command:** `polish` — on the window surface, with the ladder written into DESIGN.md first.

### P0-2 — The transcript sits on near-black TextEdit bars (the reading surface is the least designed surface)

- **What:** Every segment row is an `egui::TextEdit::multiline` (editor.rs) with default visuals → `extreme_bg_color = gray(10)` near-black bars, 15 px serif text on them, rows 2 px apart; the "active segment" is a 12 % accent wash on an otherwise transparent row; the timestamp gutter is 11 px mono `HH:MM:SS.mmm`. **Verified in both editor captures** ("transcript text sits on long, opaque black bars"; "solid black rectangular backgrounds with white text").
- **Why it matters:** the transcript is the product. It is what users read, edit, and live in — and it is currently the surface that contradicts the glass world the hardest. Fixing the pill's material while the reading surface stays black is fixing the frame and not the painting.
- **Concrete fix:** design the reading surface first in M9. One row surface (transparent by default; a deliberate hover state; a clear active-segment treatment, e.g. accent edge bar + tinted row instead of a wash nobody sees). Set `text_edit_bg_color` to transparent (or to the row surface color) so the widget disappears into the row. Give rows a consistent rhythm (min height, spacing), shorten the timestamp to `mm:ss.t` or `mm:ss` in a calm gutter, and give focus/editing its own state in the same system.
- **Command:** `typeset` — the transcript is a typography problem before it is a color problem.

### P1-3 — Zero motion: every state change teleports

- **What:** There is no authored transition anywhere. Route switches hard-swap the pane; the pill swaps dot+label → square+clock in one frame; live transcript lines appear fully formed; the active-segment highlight jumps between rows; the drop zone's hover ring is an instant stroke swap; the save caption appears/disappears. Painted controls have **no hover or press visuals at all** — `response.hovered()` only sets the cursor icon (pill, transport glyph, rows, glyph links). The only movement in the app is egui's stock 100 ms fade on its own widgets (`animation_time` default 6/60 s), which the custom world doesn't share — so motion, where it exists, is itself inconsistent. **Source-proven** (no `ctx.animate_*` / animation usage in `crates/suramak-ui`).
- **Why it matters:** on macOS, motion is the OS's way of explaining state — its absence is why the app feels like a screenshot viewer. The owner named it directly. It is also the cheapest single lever that makes the *existing* world feel alive.
- **Concrete fix:** a small motion token set — durations ~120/180/240 ms, standard ease-out, nothing bouncy — applied to: route content (fade/slide), the pill's idle↔recording morph, live-line arrival, active-segment transitions, hover/press/focus on every painted control, the drop-zone hover, and the save caption. Honor Reduce Motion from day one.
- **Command:** `animate`.

### P1-4 — Five control languages, plus dead affordances

- **What:** Stock egui buttons (`Stop`, `Polish`, `Accept`/`Discard`, `Back`/`Save`/`Reset to defaults`, `Download`/`Delete`/`Set default`), stock selectable chips with egui's teal selection fill (engine, language, export formats), stock checkbox/slider/combobox (sources rows, settings), emoji glyphs (🗑 ✔ ✖), hand-painted glyphs (titlebar play/export, transport), a bare 12 px `Home` text link (editor) vs `Back` buttons (Settings/Models), and the custom pill. The titlebar's play and export glyphs look like buttons and have no interaction at all. **Verified (S2: "Home" link not distinguishable as a control; chips styled as egui selectables) + source-proven (no `Sense` on the titlebar glyphs; `on_hover_text` never used anywhere).**
- **Why it matters:** this is the owner's "weird button styling choice", literally. Every control teaches a different interaction rule, and the dead glyphs burn trust in the first minute.
- **Concrete fix:** one control system, three tiers (primary / secondary / quiet), all states drawn (hover, pressed, focused, disabled), hit areas ≥ 24 pt, tooltips on every glyph control, the titlebar glyphs wired to real actions or removed, emoji replaced with painted SF-like vector glyphs, and a single navigation pattern (Home/Back as the same quiet control in the same position).
- **Command:** `shape` — this is a control-system design task, not a repaint.

### P1-5 — Heat: full-window repaint cadence over glass, with no throttle (owner-reported: MacBook Air M4 runs hot)

- **What:** while any "busy" state holds (recording, job, download, pending autosave) the event loop sets `ControlFlow::WaitUntil(now + 50 ms)` and requests a redraw every wake (main.rs `about_to_wait`) — every repaint re-tessellates the whole UI and re-renders the whole transparent surface, and the compositor re-blurs the full-window glass. A 240 s smoke run of this build produced **7,733 frames (~32 fps of full-window rendering)**, measured on llvmpipe — on the Mac this is Metal plus WindowServer blur. The record view redraws its 96-bar waveform, meter, and timer at that same cadence; long transcripts re-layout every per-row TextEdit each frame. On top of that sit the on-device ASR runs (expected, separate cost).
- **Why it matters:** heat is fan noise, battery, and broken trust for a "quiet local app". It also masks the accounting — without a measurement, M9 cannot claim the UI got cheaper, only that it changed.
- **Concrete fix:** instrument the frame (timers around tessellate + render), decouple repaint cadence from the coarse "busy" flag (repaint when visible state changes; give the clock/meter their own cadence; stop repainting while the live card is unchanged), evaluate blur/composite cost at window size, and virtualize transcript rows. Record before/after frame budgets in the M9 report.
- **Command:** `optimize`.

---

## 7. Persona red flags

**Alex (power user)**
- No keyboard shortcuts exist at all — he will try Space to play, Cmd+E to export, Esc to dismiss, Cmd+S to save, and hit nothing. The only app menu command is the system Quit.
- The titlebar play/export glyphs are dead; he clicks them, nothing happens, and he starts distrusting every unlabeled control.
- The editor's waveform is a deterministic fake (`bar_height()` hash). He will compare it to the audio within one session and stop trusting the strip — the signature object becomes a liability.
- No bulk operations: delete is one-at-a-time, Home's recent list silently caps at 6 rows, no search/filter, no re-run, no multi-select in the model table.
- The live transcript is read-only and unselectable (no fixing a word mid-recording); polish is batch-only with a modal review; Settings and Models don't scroll, so at the 900×600 minimum window his Back/Save/Reset buttons fall below the pane.

**Sam (accessibility, keyboard-first, contrast)**
- **No screen-reader support at all**: egui's `accesskit` feature is not enabled anywhere in the workspace — VoiceOver sees an empty window.
- The custom world is mouse-only: pill, rows, glyphs, drop zone are not keyboard reachable and draw no focus state; there is no focus ring to see even where egui's Tab order works.
- Contrast is unmanaged: 11–12 px `text_dim` (#9fa6bb) captions sit at partial alpha over translucent washes over an arbitrary wallpaper — the faint `recording` word, the 10.5 px ruler ticks, and the 11 px source labels are the worst cases; nothing guarantees even 3:1, let alone 4.5:1.
- The level meter is color-only and reads as broken **verified**; the `saved` state is a 12 px dim caption — status conveyed by subtle color alone.
- Emoji glyphs (🗑 ✔ ✖) are announced by name, not function, and carry no accessible labels.
- (Forward requirement: M9's motion must ship with Reduce Motion honored.)

**Jordan (first-timer)**
- First run is an empty Home with no onboarding, no sample, and — on a build without capture — a dimmed pill whose explanation lives in a status line far from the pill.
- The titlebar glyphs suggest play/export even with nothing loaded; clicking them does nothing and nothing explains why.
- Jargon he has no way to decode: "Polish (LLM tidy-up)", "16 kHz", "engine: coreml → planned metal", "96 of the last 96 meter samples", sidebar `· tiny` tags.
- Quitting mid-recording silently discards his recording; nothing in the UI warned him.
- In the editor, the two clocks disagree (`00:20.8` vs `00:20`), the export destination is a prefilled path he didn't choose, and the `saved` confirmation is easy to miss — he cannot tell whether his edits are safe.

---

## 8. Minor observations

- **Five time formats on one screen**: ruler `HH:MM:SS` (10.5 px mono), strip readout `mm:ss / mm:ss` (12 px mono), rows `HH:MM:SS.mmm` (11 px mono), record timer `mm:ss.t` (30 px mono), pill `mm:ss` (19 px mono).
- **Date formats disagree**: title `Recording 2026-10-14 07:40:36` vs sidebar `14 Oct 2026 00:37` vs the wav stem `Recording 2026-10-14 07-40-36`.
- **Two lists, one truncated**: sidebar shows all recordings; Home's "Recent recordings" silently caps at `min(6)`.
- **Same navigation, two words**: `Home` (editor) vs `Back` (Settings/Models); neither is styled as a control.
- **Sidebar selection is ambiguous**: selected and hovered rows share the same `row_selected` fill; no focus state exists.
- **One action, two visuals**: record view's stock `Stop` button and the pill's painted stop.
- **Clipped source label** `System audio (all processes e…` **verified** — a 220 px `add_sized(…truncate())` in record.rs; the sources row (label + checkbox + slider) is cramped and the slider reads as broken **partial**.
- **Export panel is a fixed 148 px bottom band**; combined with the strip and polish bar, the minimum-height layout squeezes the transcript, and the pill (window-centered, not pane-centered — 147 px left of the reading column's center at 1280 wide) lands on the middle of the Destination field **verified**.
- **Tofu glyph** at the start of a live row **verified** — model output, not a UI bug, but the UI renders it raw with no missing-glyph treatment or row-level fallback.
- **Duplicate empty state**: sidebar "No recordings yet. / Drop a file to transcribe it." and Home "Nothing yet — your transcriptions will appear here." say the same thing twice.
- **Raw model ids** (`turbo-q5-0`, `· tiny`) with no human names.
- **No tooltips anywhere**; the only self-explaining controls are the labeled pill and the section captions.

---

## 9. Questions to consider

1. If the transcript is the product, should M9 be built **from the reading surface outward** — row rhythm, type scale, states, then everything else derived from it — rather than from the shell inward?
2. Should the editor's waveform become **real** (peaks from the audio) in M9, and what does the strip honestly show while a job runs or the audio is gone? The signature object currently carries zero information.
3. Do we reach Apple fidelity with hand-painted egui controls, or adopt native controls (NSButton / NSTextField overlays) where it matters most — and what does that cost the Linux/Windows story?
4. What material tint does the app commit to? The comp was light; the build is pinned DarkAqua. Pin the ladder (tint, wash levels, one radius, one hairline) in DESIGN.md so it cannot drift again.
5. What is the measured heat budget — frames vs ASR — and what repaint cadence can the record view afford on battery? (The 32 fps busy-loop measurement above is the starting number.)
6. What should quitting mid-recording do: warn, keep, or discard? Today it discards silently; the UI should say so before the user decides.
7. Is silent autosave with a 12 px caption enough, or does M9 need a visible edit state (saved/undo/version) for a product people will type into for an hour?

---

*Reviewer note: this critique is deliberately unsentimental. The world is right, the material is real, and the copy is better than most shipping apps; the failure is entirely in the craft between concept and pixel. M9 should treat sections P0-1/P0-2 as the acceptance bar and the "What's working" list as untouchable stock.*
