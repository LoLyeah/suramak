# M9-CRITIQUE-B — Suramak current UI: the second full fresh-eyes critique (tree `e299d00`)

**Method: native target — Assessment A by an isolated reviewer; the bundled detector reads HTML/CSS only and has no verdict on native code, so the detector slot is n/a by design (the documented native path, not a failure).**

**Provenance.** Second full critique of the remade interface, written by an isolated reviewer who built none of it, on the frozen tree **`e299d00`** (M9a `8e472db` + M9b `d24a3b2` + M9c `55177a0` + M9d `e299d00`; working tree clean at review time). The scoring convention is reused **verbatim** from `docs/reports/M9-CRITIQUE-A.md`: severity load, 0–4 per heuristic, lower is better, max 40; bands 0–4 Excellent · 5–9 Good · 10–16 Fair · 17–24 Poor · 25–40 Critical. Anchors: **22/40** (pre-remake, M9-CRITIQUE-A) → **12/40** (M9-FINISH-REVIEW, tree `55177a0`) → this run.

**Evidence read, in order:** `M9-CRITIQUE-A.md` (structure + convention), `M9-FINISH-REVIEW.md` (§6 findings checked one by one), `M9d.md`, `M9-BRIEF.md`, `.impeccable/motion-brief.md`, `M9a/M9b/M9c.md`, PRODUCT.md's M9 status block (line 67). **Re-run on this tree, not trusted from the reports:** `cargo fmt --check` clean · `cargo clippy --workspace --all-targets -D warnings` clean · `RUST_TEST_THREADS=1 cargo test --workspace` **346 passed / 0 failed / 6 ignored** (19 binaries — matches M9d) · `pixelcheck.py` m9d suite **25 PASS / 0 FAIL** · m9c suite (corrected probe) **18 PASS / 0 FAIL** · `cargo check -p suramak-glass --target aarch64-apple-darwin` clean (forced re-check). **New captures** (this run, `build/m9d-e2e/extra/`, harnesses `shoot-extra.sh` / `shoot-minsize*.sh`): Settings (light), empty first-run Home, the transcribe/job screen, and the app at its own **900×600 minimum** (Settings dead-end + Models collision) — surfaces no M9c/M9d capture covers — plus zoomed crops (`build/m9d-e2e/crops/`) read with pixel probes wherever a visual read was load-bearing. Every vision read below was pixel-checked before it entered a finding; two plausible-looking reads (a "light sidebar" in dark mode, a "truncated time readout") were **false** and were discarded (pixels: sidebar `srgb(14,21,38)`; readout `00:06.231 / 00:09.000`).

**Boundaries, stated honestly.** macOS-only paths (real glass, `NSAppearance` re-resolve, `NSWorkspace` Reduce Motion, the accesskit macOS adapter) are judged from source + the honesty of the reports — there is no macOS host here. `suramak-glass` compiles clean for `aarch64-apple-darwin`; the **UI crate cannot be cross-checked here** because `ring`'s C build needs a macOS SDK/toolchain (attempted, failed in the C compiler — an environment limit, not a code finding). The Linux build is capture-free, so a few Linux-only strings (`feature \`capture\``) are dev artifacts that never render on the Mac; they are treated as such throughout.

**What M9d closed (verified, one by one).** The finish review's §6 items: the Home "Re" fragment — **fixed** (pixel probe: no ink above the drop zone; the note lives in the message slot at y 384–394) · pill spring / living dot / seek easing / follow-scroll — **implemented** (pixel: press shrinks the pill content 115→112 px, ratio 0.974; 28 dot px beside the word; the settled playhead sits at the clicked x=800; unit tests cover the rest) · first frame static — **fixed** (route epoch registers startup without an arrival; test) · contrast artifact — **fixed** (`record_red_text` + WCAG test; worst text pair 4.66:1, the dot 4.46:1 ≥ 3:1) · pixel-probe fix — **fixed** (status band y 380–400; both suites re-run green) · the truncated source row tooltip — **added** (`.on_hover_text(&source.label)`). Still open from that list: quit-mid-recording, recents cap, settings/models scrolling, duplicate empty states, `format_mmss`, `ink_faint`. The M9c deviation "~5 px" was already corrected to ~48 px by the finish review; nothing re-opens it here.

---

## 1. Nielsen's 10 heuristics — scored 0–4 (convention of M9-CRITIQUE-A, reused verbatim)

Scale: 0 = no issue · 1 = cosmetic · 2 = minor usability issue · 3 = major usability issue · 4 = catastrophic. Total is a severity load (max 40; lower is better). "A" = pre-remake score, "F" = finish-review score.

| # | Heuristic | A | F | **B** | Key issue (one per row) — Δ note |
|---|---|---|---|---|---|
| H1 | Visibility of system status | 2 | 1 | **1** | The record state is legible (breathing dot + text-safe red word + `hh:mm:ss` + real meter/waveform), but the level meter still has **no scale or peak marker**, and the transcribe card read `0%` for most of a short job (percent arrives per segment; the tiny fixture finishes in ~7 s). Δ vs A −1; same as F. |
| H2 | Match between system and real world | 2 | 1 | **1** | Copy sweep holds (human model names, `engine: CPU`, two sanctioned clocks, no telemetry strings), but recording titles remain **timestamp filenames** (`Recording 2026-10-13 14-05-33`) and the same row repeats the date in a second format (`13 Oct 2026 14:05`); settings engine chips still show raw ids (`coreml` / `metal` / `cpu`) while the transcribe card says `Core ML`. Δ vs A −1; same as F. |
| H3 | User control and freedom | 2 | 1 | **2** | Escape resolves in the editor/sheet and ⌘⌫ arms the delete confirm, but an in-flight job has **no cancel affordance at all** (the transcribe screen offers no control), a confirmed delete has no undo, and Escape is unbound outside the editor. Δ vs A same; **differs from F (1→2)**: I read the no-cancel + no-undo pair as a minor usability issue, not cosmetic — see the delta note at the table's foot. |
| H4 | Consistency and standards | 3 | 1 | **1** | One painted control system now holds (tiers, capsule buttons, painted glyphs, one `⌃ Home`), but the declared stock-widget pocket remains — and its themed corner radius (8) clamps the stock checkbox box (~14 px) into a **circle**, so `mute`, `keep audio after transcription` and the Inputs list read as radio buttons while the Inputs copy says "everything ticked". Δ vs A −2; same as F. |
| H5 | Error prevention | 2 | 2 | **2** | Unchanged and still the top residual: quitting mid-recording stops capture and **discards the buffers silently** (`main.rs` `CloseRequested → exit`; `runtime.rs::shutdown` → `let _ = session.stop()`, result dropped; no UI warning). Import/export path fields still accept anything until the action fails; delete confirm + settings validation remain good. Δ vs A and F: same. |
| H6 | Recognition rather than recall | 2 | 1 | **1** | Dead titlebar glyphs are gone, painted controls carry tooltips + a11y labels from one source, and the truncated source row gained its tooltip in M9d; the waveform strip and gutter stamps still have a11y labels but **no hover tooltip** — the last recall gap. Δ vs A −1; same as F. |
| H7 | Flexibility and efficiency | 3 | 2 | **2** | The keyboard map is real (Space, ⌘R, ⌘E, Esc, ↑/↓, ⌘⌫, guarded and tested), but there is still **no scrub-drag or fine seek on the strip** (click-to-seek only, verified in `editor.rs`), no search/filter/re-run, Home's recents silently cap at 6, no app-menu commands beyond the system Quit — and **Settings has no scroll, so at the app's own 900×600 minimum its Save button is unreachable** (verified in pixels this run; P2-3). Δ vs A −1; same as F. |
| H8 | Aesthetic and minimalist design | 3 | 1 | **1** | The ladder is real — **0 exact `gray(10)` px in the transcript region, re-measured this run in both appearances** (the A critique counted 38,253), one card recipe, the deck as one band, the pill floating free — residual: the export sheet **covers the second transcript row while open**, the polish setup note is a long always-on sentence, and the sources card packs its rows into half its width. Δ vs A −2; same as F. |
| H9 | Help users recognize, diagnose, recover from errors | 1 | 1 | **1** | Unchanged strength: specific, actionable, in-place messages (audio-tap permission guidance, the WebM/`ffmpeg` message, per-app no-match, "recording is disabled while a job is running") — still 12 px red captions with no icon, and their position varies per view. Δ vs A and F: same. |
| H10 | Help and documentation | 2 | 1 | **1** | Tooltips are the whole help system (they are real and consistent); still **no Help menu, no shortcut sheet, no onboarding, no About** — the six shortcuts are only discoverable by hovering. Δ vs A −1; same as F. |

**Total B: 13 / 40 → Fair (10–16 band).** Deltas: **vs A 22 → −9 (Poor → Fair)**; **vs F 12 → +1**. The single differing row is H3, and it is a deliberate disagreement, not an oversight: the finish review's residual list for H3 ("no cancel for in-flight jobs") is the same as mine, but it scored that residue cosmetic. A user who starts the wrong job on a long file cannot stop it and cannot leave the job surface; that is a minor usability issue by this scale's own definitions, so H3 stays 2. Everything else in the table is a fresh-eyes confirmation, not a re-statement: each row's key issue was re-observed in the pixels or the source on this tree.

---

## 2. Design-specificity verdict

**Verdict: authored at the shell level now — with three named pockets that are still category-interchangeable.**

At the finish review the verdict was "authored at the concept level, interchangeable in execution". That is no longer true of the execution on the primary surfaces:

1. **The deck is a composition, not a toolbar.** Transport capsule → waveform strip → `mm:ss.mmm` readout → one `Export…` — the C-comp pick, built in one band with one radius system. Nothing in the strip is fake anymore: the peaks are the file's own min/max envelope (re-measured this run: the fixture's documented silent gap reads as a flat baseline at ≈2.8–3.6 s of the 9 s — bar-quantized against the fixture's 2.9–3.8 s — with real variable-height speech either side), and the playhead eases only on true jumps (>120 ms) so playback stays exact.
2. **The reading surface is the product's surface.** Serif body on the pane at a 68 % measure, a calm mono `hh:mm:ss` gutter, a hover wash, an active segment that is a 3 pt accent edge bar + faint tint — and **zero stock `gray(10)`** where the old critique measured 38,253 px of near-black bars. This was P0-2; it is closed in both appearances.
3. **The signature objects carry a state language.** The pill morphs idle↔recording with a ≤1.5 % scale dip and a press spring whose overshoot is bounded at 2 % of a 3 % travel (measured 115→112 px, centre held 0.5 px); the record view's dot breathes 0.7↔1.0 at 1.6 s on the shared clock; the title-line cluster wears iOS-26 capsule bodies with a real 9 px gap (pixel-asserted). The copy system ("transcribed on this machine, never uploaded"; "live transcript — provisional · the final pass runs on Stop"; "the meter shows the last 10 seconds") is untouched and still the app's best writing.

The remaining interchangeable pockets, named:

- **The record view's sources card** — stock checkbox + slider + `mute` + a fixed 220 px label column, packing its content into the card's left half while `System audio (all processes ex…` truncates with the space available; the circular-checkbox semantics wobble lives here too.
- **The settings form and the model manager table** — conventional label/control rows and a plain table. For an Operate-mode settings screen that is *acceptable* genericity (Apple's own settings are generic), but nothing in them is Suramak's; the engine chips still wear raw ids.
- **The transcribe/job card** — title, "transcribing on this machine…", a progress bar, engine/model lines. It is honest and plain; it is also the one screen with no authored state language beyond the status fade.

None of these three is a defect by itself; together they are the honest answer to "could this be any app?" — on those screens, still yes; everywhere else, no.

---

## 3. Cognitive load checklist (8 items)

| # | Check | Result | Evidence |
|---|---|---|---|
| 1 | Primary action is obvious on every screen | **Pass** | Home = the drop zone; Record = Stop (secondary, tooltipped) + the pill; Editor = the deck's `Export…` and the floating pill; Settings = Save; Models = per-row actions; Transcribe = nothing to do but wait (that absence is H3's issue, not a hierarchy failure). The A-critique failure (pill parked on the export band) is gone. |
| 2 | Choices are limited and grouped | Pass | Screens are sparse; settings is sectioned; the export sheet offers five chips inline; the sources card groups per-source gain/mute. |
| 3 | Control placement is consistent across screens | Pass | One `⌃ Home` in the same corner (editor, settings, models, record failure/idle, transcribe); Save/Reset always at the form's end; the pill bottom-centre everywhere; one navigation word. |
| 4 | Labels and affordances need no guessing | **Pass (weakest pass)** | Tooltips on painted controls; no emoji; the truncated source row gained its tooltip. Residuals: the strip and gutter have a11y labels but no hover tooltips, and the circular checkboxes imply radio semantics against the "everything ticked" copy. |
| 5 | Every action produces visible feedback | Pass | Status captions fade through one helper (verified live: "deleted en.wav" on the settled Home); autosave `saved`; progress eased for downloads; ticking clock; job percent (coarse but present). |
| 6 | Errors are actionable and sit near the error | Pass | Best-in-app system, unchanged; the protect-list strings render verbatim in code and pixels. |
| 7 | Current state is visible (mode, selection, focus) | Pass | Legible `recording` + dot; sidebar selected (accent bar + tint) vs hover (quiet wash) distinct and animated (pixel-verified in both appearances); focus rings on painted controls; active segment marked; two clocks, both `hh:mm:ss`. |
| 8 | No unexplained jargon or debug artifacts | Pass | The copy sweep holds (grep + pixels); residual: engine chip ids (`coreml`/`metal`/`cpu`) in settings, and the capture-free build's `feature \`capture\`` notes — the latter never render on macOS. |

**Result: 8 pass / 0 fail** (A: 3/5; every A-failure is closed or reduced to a named residual). Item 4 is the one I would watch: the circle-checkbox semantics is the kind of small wrongness that re-accumulates if the stock pocket is left unowned.

---

## 4. Emotional journey (peak–end)

- **First 10 seconds:** the first frame is static now (no fade-in), Home is calm, and the glass window / frosted fallback is the first object — the material still buys hope before anything is read.
- **Early:** the drop zone, the formats line, "transcribed on this machine, never uploaded"; the pill invites (on the Mac; the Linux dev build dims it with the reason in the message slot).
- **Peak (real, working):** press Record → the pill morphs to stop + `hh:mm:ss` under the spring, the dot breathes beside a legible red `recording`, the meter and the rolling waveform are real, and words arrive under the provisional label. This is still the product's magic moment — and it now moves.
- **Descent — largely repaired:** Stop → the editor: the deck with real peaks, the eased seek, serif rows on the pane, the sheet easing open, the delete confirm in-place, the row sliding away on delete. The old "the final screen is the most defective screen" is no longer true.
- **End:** export → sheet → written file; "deleted en.wav" / save status in the message slot. The end is designed now.
- **Background anxiety that remains:** quitting mid-recording still discards silently (the one true data-loss path); first-run emptiness has no sample or guidance; the export sheet covers a row while open; the destination is a prefilled path you didn't choose. Peak–end memory now lands on *calm and coherent* rather than *buggy* — with the quit case as the one thing that can still feel careless.

---

## 5. What's working (protect these)

1. **The record view's live state is the app's best screen.** Breathing dot + text-safe red word (`#c8281e`, 5.56:1 on the light card) + `hh:mm:ss` everywhere + a real meter and a rolling waveform + the pill's morph and bounded spring — and the protected copy ("live transcript — provisional · the final pass runs on Stop", the on-device line) renders verbatim. The three owner-confirmed delight details (spring, living dot, seek/follow easing) are implemented, bounded, Reduce-Motion-aware, and pixel-verified.
2. **The reading surface and the deck.** Zero stock fills under the transcript; one card recipe; real peaks from the decoded samples (flat baseline when there is no audio — never the old silhouette); click-to-seek with a drawn ease that never lags playback; an active-segment treatment you can find without hunting. The product's core is now the best-dressed surface, which is the correct priority order.
3. **The craft now has teeth.** One token ladder in both appearances, a motion grammar with retarget-from-current discipline and Reduce Motion zeroing every duration, a centralised keymap, one-source tooltips/a11y labels, a WCAG contrast table, a painted-bounds audit, and two pixel suites that **reproduce green on this tree** (25/25 + 18/18) beside 346/0/6 tests. This is the difference between "polished once" and "cannot drift silently".

---

## 6. Priority issues

### P1-1 — Quitting mid-recording still discards the recording, silently

- **What:** `WindowEvent::CloseRequested` exits immediately (`main.rs`); the runtime's shutdown stops the capture session and drops the result (`runtime.rs::shutdown` → `let _ = session.stop();`); nothing in the UI warns, confirms, or offers to keep the audio. Verified in source on this tree; carried over from A and the finish review, still unassigned.
- **Why:** it is the only path where the app destroys user work with no warning — and a recording is precisely the data a local-first recorder exists to keep. Every other safety surface (delete confirm, settings validation, error copy) is careful; this one is not.
- **Fix:** intercept CloseRequested while a session is live: a small confirm ("Still recording — Stop & save / Discard / Cancel"), or the minimal version — finalize-and-save on quit with a status line on next launch. Keep the HOTFIX-QUIT single-instance/clean-quit behaviour intact (protect list).
- **Command:** `harden`.

### P2-2 — An in-flight job cannot be cancelled, and the job screen offers no control

- **What:** the transcribe card (captured this run) shows title, "transcribing on this machine…", a progress bar, `engine: CPU`, `model: Tiny (fastest)` — and nothing else; the runtime has no cancel token for the job pipeline (no cancel path in `runtime.rs`/`app/mod.rs`); the floating pill is disabled while a job runs. A's H3 "no cancel affordance for a running transcription" survives the remake untouched.
- **Why:** on a long recording the user is locked out of the decision they just made — wrong file, wrong language, wrong moment — until the job ends. It is the last ordinary control the product is missing.
- **Fix:** a quiet Cancel (tertiary tier) in the transcribe card, wired to a runtime cancel token (the worker bridge owns the thread; whisper context can be checked between chunks); cancel → Home with a "transcription cancelled" status. Also worth: Esc resolves to "leave the job screen" only when cancelling is safe — today Esc does nothing outside the editor.
- **Command:** `shape` (plan the flow before code).

### P2-3 — At the declared 900×600 minimum the settings form dead-ends and the Models table collides

- **What (all verified in pixels this run — the first captures ever taken at the app's own minimum size, `build/m9d-e2e/extra/min-*.png`):** at 900×600 under Xvfb — (a) **Settings dead-ends**: no `ScrollArea` exists (`settings.rs`), the form cuts mid-"Polish" section with the Model field clipped at the bottom edge, and **neither Save nor Reset to defaults is reachable** (no scrollbar anywhere) — the form's only apply point is off-screen; (b) **the Models table collides**: its action column is right-anchored (`models.rs`: actions at `right − 320.0`, 300 wide) while INSTALLED sits at a fixed offset, so as the pane narrows the two land on each other — the first rows render `missing` split across the Download button ("mis" … button … "issing"), the Delete label overlaps the same text, and the header row prints ACTION/INSTALLED on top of each other; (c) long captions clip at the right edge instead of wrapping ("…after the transcript te", "…this app's appearance. App"). At 1280×800 all three are clean (captured this run). The finish review knew "settings/models don't scroll" from code; this run shows what it actually costs.
- **Why:** 900×600 is the app's own declared minimum (`shell.rs` `MIN_SIZE`). At a supported size the settings form's only apply point is unreachable and the model table is illegible — that is the definition of "does not work at the minimum", not a polish item.
- **Fix:** a `ScrollArea` for settings (the pattern already exists in four other views) and for models once the manifest grows; make the models columns flow below ~1020 px window width (let the actions wrap to the row's second line, or shrink/hide ENCODER first); verify at 900×600 under Xvfb as an acceptance shot.
- **Command:** `adapt` (native variant).

### P3-4 — The stock-widget pocket: circles where checkboxes belong, a self-inflicted truncation, raw engine ids

- **What:** the record view's sources card and the settings form still use themed stock egui checkbox/slider/combobox (declared scope in M9a/M9b). Consequences visible on this tree: (a) the themed widget corner radius (8) clamps the ~14 px checkbox box into a **circle**, so `mute` / `keep audio after transcription` / the Inputs list read as radio buttons — against copy that says "everything ticked"; (b) `System audio (all processes ex…` truncates at a fixed 220 px while roughly half the sources card is empty; (c) the settings engine chips show raw ids (`auto / coreml / metal / cpu`) while the transcribe card humanizes the same engine as `Core ML`.
- **Why:** this is the remaining "could be any app" pocket and the one visible crack in the control language (circles beside capsules and segments); (b) is self-inflicted — the space exists — and (c) is a copy-sweep straggler.
- **Fix:** give checkbox/slider/combobox the painted treatment when the pocket is next touched (or, minimally: clamp the checkbox radius to a squircle and widen the label allocation / right-align the gain column); humanize the engine chips.
- **Command:** `polish`.

### P3-5 — First run and help: no onboarding, no help surface, shortcuts undiscoverable

- **What:** first-run Home (captured this run) is an empty drop zone with the same message twice (sidebar "No recordings yet. / Drop a file to transcribe it." + main "Nothing yet — your transcriptions will appear here."); there is no sample transcript, no Help menu, no shortcut sheet, no About. The keymap (Space, ⌘R, ⌘E, Esc, ↑/↓, ⌘⌫) exists and is tested, but is only discoverable by hovering tooltips.
- **Why:** Jordan's first minute is still the app's weakest minute; and the shortcuts are good enough that their invisibility is a waste. The keymap is already centralised, so a shortcut sheet is cheap.
- **Fix:** a one-screen first-run card or a seeded sample transcript; a `?` shortcut sheet sourced from the keymap; dedupe the two empty states; tooltips on the strip and gutter while there.
- **Command:** `onboard`.

---

## 7. Persona red flags

**Alex (power user)**
- Cannot cancel a running job — no control exists on the transcribe screen, and the pill is disabled while a job runs (P2-2).
- Still no scrub-drag or fine seek on the strip (click-to-seek only, verified in `editor.rs`); ↑/↓ step segments but there is no frame-level seek.
- Home's recents still silently cap at 6 (`list_slots(&state.library, Some(6))`); no search, no filter, no re-run, no multi-select; no menu commands beyond the system Quit.
- The strip and gutter stamps carry a11y labels but no hover tooltips — he hovers, learns nothing.
- At the 900×600 minimum he hits two walls (both captured this run): Settings has no scroll, so Save is unreachable; and the Models table's columns collide — `missing` splits across the Download button and the header prints ACTION/INSTALLED on top of each other (P2-3).
- Carried over honestly: the fake waveform he distrusted is gone (real peaks verified); the dead titlebar glyphs he clicked are gone.

**Sam (accessibility, keyboard-first, contrast)**
- accesskit is enabled with labels across painted controls (two a11y-tree tests run real frames); on macOS the adapter path is source-proven only — the UI crate cannot be cross-compiled here (ring's C build), and that boundary is stated, not hidden.
- Focus rings exist on every painted control (M9b Tab captures: rows, transport, strip); stock widgets show egui's hover-style focus only — no dedicated ring — a small residual in the same pocket as P3-4.
- Contrast now has a verification artifact: the WCAG table (worst text pair 4.66:1; `record_red_text` clears all three light surfaces; the dot 4.46:1 ≥ 3:1 non-text). `ink_faint` (2.89:1 light) remains unused dead weight.
- Reduce Motion: real on macOS (`NSWorkspace.accessibilityDisplayShouldReduceMotion` read on the main thread, refreshed at startup/theme-change/effect batches; Linux returns false, documented) — the felt behaviour is the owner's Mac.
- The circular checkboxes read as radio buttons (semantics, not contrast) — P3-4.
- Carried over: the meter has no scale/peak (length-encoded only — acceptable, but the "level" label explains less than it could).

**Jordan (first-timer)**
- First run is still empty with no sample, no onboarding, no help surface (P3-5); the empty state says the same thing twice.
- Quitting mid-recording silently discards his recording; nothing in the UI warned him (P1-1).
- The editor's polish setup note is a long always-on sentence when polish is unconfigured; it starts lowercase ("polish is not configured — …") beside a capitalized `Polish` button.
- The export sheet covers the second transcript row while open, and the destination is a prefilled path he didn't choose (`/home/ubuntu/en.md`).
- Recording titles are still timestamp filenames, with the date repeated in a second format on the same row.
- On the capture-free Linux build the pill is dimmed with its explanation in the message slot; on macOS this never renders — noted so Linux screenshots are not over-read.
- Carried over honestly: the clocks agree now (`hh:mm:ss` everywhere), the jargon strings are gone, and the dead glyphs he clicked are gone.

---

## 8. Minor observations

- **Recording titles are timestamp filenames** (`Recording 2026-10-13 14-05-33`) and the row repeats the date in a second format (`13 Oct 2026 14:05`) — carried over.
- **Two lists, one truncated:** the sidebar shows all recordings; Home's recents silently cap at 6, with no "and more" — carried over.
- **Duplicate empty state:** sidebar and Home say the same thing twice — carried over.
- **Settings/Models don't scroll** — at 900×600 the settings form dead-ends (no Save reachable) and the Models table's columns collide; long captions clip at the right edge instead of wrapping — carried over (now pixel-verified; P2-3).
- **Export sheet occludes the second transcript row while open** (popover by design; transient) and the destination is prefilled — carried over as observation.
- **Stock checkboxes render as circles** (radius 8 clamped on a ~14 px box) — radio semantics; the Inputs copy says "everything ticked" — new, folded into P3-4.
- **The sources card packs its rows into half its width** while the second label truncates at 220 px with space available — new, folded into P3-4.
- **"the meter shows the last 10 seconds" sits beside the Stop button**, two cards below the meter it describes — new (cosmetic placement).
- **Settings engine chips show raw ids** (`auto / coreml / metal / cpu`) while the transcribe card says `Core ML` — new.
- **The polish setup note** starts lowercase and is a sentence-long always-on line in the editor — carried over (finish-review residual).
- **`suramak_playback::format_mmss`** remains unused by the UI — carried over (M9c flagged it itself).
- **`ink_faint`** remains unused (2.89:1 light) — carried over.
- **The pill is window-centred (x≈640) while the reading column centres at ≈787** — the signature object sits ~147 px left of the content column's centre in the editor and record views. By design (M8.5's one shared `float_rect`), and defensible as a FAB; worth one deliberate look on the Mac.
- **The transcribe progress bar read 0% for most of a short job** (percent arrives per segment; the tiny fixture finishes in ~7 s) — observed this run; fine for long files, coarse for short ones.

---

## 9. Questions to consider

1. Quitting mid-recording: warn, keep, or discard? The UI still says nothing before the user decides — this is the one finding I would argue deserves to break the "carried over, unassigned" status (P1-1).
2. Should the job pipeline get a cancel token before M10? Faster engines shorten jobs but long files still run; the no-cancel gap is the last ordinary control missing (P2-2).
3. Now that the sidebar exists and the reading surface is first-class, what is Home for — a drop zone with six recents, or the search/filter surface Alex keeps expecting? (P3-4/P3-5 both touch this.)
4. Does the stock-widget pocket (record sources, settings form) get the painted treatment, or is themed-stock the accepted floor? If it stays, clamp the checkbox radius so a checkbox is a checkbox.
5. On the Mac: does the pill's spring/dot/seek feel right at 120 Hz, and does the content-only press scale read correctly against the fixed glass frame (M9d deviation 1 — the material body itself does not move)?
6. Is an onboarding/help surface wanted before this ships beyond the owner — or is the owner-only phase the right time to defer it?
7. Should the export sheet gain a scrim or move so it stops covering rows, now that rows are the product?

---

## 10. The next UI work, if any

**Verdict: the interface is done — the remake's work is finished on this tree; what remains is the owner's macOS acceptance plus a small safety/adaptation tail (quit warning, job cancel, minimum-size fixes) that belongs in the next slice, not in another remake pass.**

Everything this machine can verify is green and *reproduced*, not trusted: 346/0/6 tests, both pixel suites (25/25 + 18/18), fmt/clippy clean, the glass crate's darwin check clean, and every M9d item verified in pixels (spring 115→112, dot 28 px, seek settled at the click, "Re" fragment gone, WCAG table real). No P0 exists; the P2s (job cancel, the 900×600 dead-ends) and the P3s are ordinary product work, not craft debt; the one P1 is a flow, not a surface.

The single next step that would move the needle most, in order: **(1) the owner's macOS acceptance pass** — real glass + `NSAppearance` re-resolve, `NSWorkspace` Reduce Motion, the system-font header, and the felt quality of the spring/dot/seek (all four are compile-checked only, by construction); then **(2) the quit-mid-recording warning** as a small, contained flow before the next release cut, because it is the only remaining path where a user loses work silently. Everything else on this list can ride M10.

*Reviewer note: the second pass confirms the first remake's promise without sentimentality — the patchwork is gone, the transcript is the product, the controls are one system, motion is a grammar, and the evidence culture (pixel suites, WCAG table, bounds audit) is the strongest part of the codebase. The residual defects are the honest kind: a missing cancel, an unscrolled settings screen, a pocket of stock widgets, and a quit dialog that should exist. None of them is a reason for another remake; all of them are reasons for the next small slice.*

*Evidence trail: re-runs — `build/m9d-e2e/extra/tests-rerun.log` (346/0/6), `pixelcheck-rerun.log` (25/25), `pixelcheck-m9c-rerun.log` (18/18), `darwin-ui-check.log` (environment-blocked: ring needs a macOS C toolchain — stated in the method); new captures — `build/m9d-e2e/extra/{settings-light,home-empty,job-A,min-after,min-models}.png` + `shoot-extra.sh` / `shoot-minsize*.sh`; crops + pixel probes — `build/m9d-e2e/crops/` (incl. the re-measured flat waveform gap ≈2.8–3.6 s and the 0-px `gray(10)` count); source reads of `main.rs`, `runtime.rs`, `shell.rs`, `theme.rs`, `motion.rs`, `views.rs`, `record.rs`, `editor.rs`, `home.rs`, `settings.rs`, `models.rs`, `keymap.rs`, `mod.rs`, `suramak-glass/{lib,macos}.rs`; `build/m9d/contrast-table.txt`.*
