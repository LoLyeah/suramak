# WORKORDER — M9b: waveform truth · repaint budget · accessibility floor

Slices: M9a landed (`8e472db`; report `docs/reports/M9a.md` — read its §4 "En route" notes and §3 deviations; the M9a look is **frozen**: do not restyle, only the surfaces named here).
Contracts: `docs/M9-BRIEF.md` (principles + protect list) · `.impeccable/motion-brief.md` (the Motion section below) · `docs/reports/M9-CRITIQUE-A.md` (P1 heat item + Sam/Alex red flags).

## Read first
1. `docs/reports/M9a.md` (what exists now; geometry is final and unit-tested — peaks drop into it).
2. This workorder. 3. `.impeccable/motion-brief.md` §3 (levels) and §5 (states). 4. The critique's P1-5 (repaint heat) + H7/H10 (keyboard/help) + Sam persona.

## Deliverables

### 1) Real waveform peaks (the strip stops being a silhouette)
- `crates/suramak-playback`: compute a min/max envelope from the decoded source **once, at source load** (streaming over samples; store e.g. 1024 buckets; trivial memory). Expose it (e.g. `Source::peaks(bucket_count) -> Vec<(f32, f32)>` or an upsampled accessor). No new crates.
- `crates/suramak-ui/src/app/editor.rs`: the deck strip draws the real peaks in the existing geometry (band, playhead, click-to-seek all stay). A source with **no audio** (recording, `keep_audio` off) draws a quiet flat baseline — never fake data.
- Scale sanity: normalize peaks to bucket max (or a documented percentile) so quiet and loud recordings both read well; document the choice.
- Library-row thumbs: **skip** unless peaks are trivially available at row time without decoding each file — thumbnails are not worth a cache format today; note it for later.

### 2) Repaint budget (the owner's heat complaint, measured)
The current build requests full-window redraws continuously in busy states (critique measurement: ~32 fps → 7,733 frames / 240 s). Make cadence a policy:
- Idle: repaint **only on events** (input, queue arrivals, animation completion) — no periodic redraws.
- Recording: meter/timer cadence at ~10 fps (`repaint_after`); live-transcript arrivals repaint on arrival.
- Playback: ≤30 fps while playing; stop on pause/end.
- Animations (motion.rs) keep their natural in-span repaints; they already end.
- Implement at the winit/egui seam in `shell.rs`/`main.rs` (about_to_wait / ControlFlow / `ctx.request_repaint_after`).
- **Instrumentation + before/after evidence:** frame counter with a per-state summary, printed at exit when `SURAMAK_FRAME_STATS=1` (or an equivalent seam). Procedure: measure the CURRENT binary first (it is the M9a baseline), then measure the new one; report a before/after table (idle plain smoke; a job/recording-length run). Note honestly in the report that VPS frames proxy **cadence**, not the macOS glass compositing cost — the throttle's purpose is fewer full-window composites.
- Transcript rows: if uniform-height rows host egui's `ScrollArea::show_rows` (virtualization) cleanly, do it for long transcripts; otherwise document precisely why it was deferred (row heights / editing state).

### 3) Accessibility floor
- Enable **accesskit** (egui `accesskit` feature + the egui-winit wiring incl. `AccessKitEnabler` on macOS; features only — no new crates). Ensure it activates on macOS builds; Linux activation for the VPS is a bonus if free.
- Every painted control (controls.rs + one-off glyph interactions) must carry a **semantic label** (`WidgetInfo::labeled`-style) — screen reader + tooltip both come from this. Sweep every call site; the painted glyph set (Trash/Check/Cross/Play/Pause/SkipStart/Stop/Chevron) got tooltips in M9a — now ensure labels.
- **Focus**: every interactive control reachable and visibly ringed (the M9a focus state); Tab order sane on each screen. If xvfb scripting can drive Tab + screenshot the ring (xdotool), include one screenshot as evidence.
- **Keyboard map** (`crates/suramak-ui/src/app/keymap.rs`, new): Space = play/pause (editor, NOT while a TextEdit is focused), ⌘R = start/stop recording, ⌘E = toggle the export sheet, Esc = close sheet / leave editor, ↑/↓ = previous/next segment (editor), ⌘⌫ = delete the transcript (existing confirm flow). Guard: shortcuts never fire while a text edit has focus (except Esc). Central dispatch, unit-tested.
- NOT in scope: settings checkboxes/sliders keep themed stock widgets (their stock a11y nodes are fine); the record view's copy stays for M9c.

### 4) Motion (per motion-brief; small, this slice)
- Peaks appear with a **base fade on first paint** only (no growth animation); with Reduce Motion: instant.
- Keyboard-triggered actions reuse existing transitions (play/pause, export sheet open, segment nav = the existing active-row crossfade).
- Nothing else new; do not touch the M9a motion applications.

## Do NOT
- Restyle anything decided in M9a (look frozen until the owner's next feedback); touch asr/capture/live pipeline, glass runtime, taps, instance/shutdown; add crates or fonts; `git commit`/`push`; modify files outside `/home/ubuntu/suramak`.

## Verification (all gates, evidence in the report)
- fmt clean; clippy `--workspace --all-targets -- -D warnings` clean; `RUST_TEST_THREADS=1 cargo test --workspace` green with new tests: peaks math (silence → zeros; DC → flat; clipping; bucket counts; normalization), cadence policy (state → repaint interval), keymap dispatch (each shortcut + the text-edit focus guard), no-audio → baseline (editor state test).
- xvfb: plain + job smokes green; `SURAMAK_FRAME_STATS` before/after table (same scenarios); screenshot of the editor strip on `fixtures/audio/en.wav` proving real peaks (pixel check: variable-height columns present; a silent fixture or the lead-in region reads flat) in one appearance at least; Tab-focus ring screenshot if the harness allows.
- `cargo check -p suramak-glass --target aarch64-apple-darwin` still clean; note which modules the macOS-only accesskit path touches.
- Report `docs/reports/M9b.md`: per-file summary; peaks API + normalization rationale; the frame table with raw numbers; a11y coverage list (every control labeled, every shortcut listed with its guard); deviations with whys; what remains for M9c (motion grammar + copy sweep + finish review).

## En route outputs to leave behind
- The frame-stats seam documented (how to reproduce the table); keymap table in the report (for docs/USER_SMOKE + README later).
