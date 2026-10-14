# M9b — slice 2: waveform truth · repaint budget · accessibility floor

Workorder: `docs/WORKORDER-M9B.md`. Brief + protect list: `docs/M9-BRIEF.md`. Motion contract:
`.impeccable/motion-brief.md` §3/§5. Defects being fixed: `docs/reports/M9-CRITIQUE-A.md` — P1-5 (repaint heat,
the owner's complaint) and the Sam/Alex keyboard/help red flags. The M9a look is **frozen**: nothing here
restyles it (the one visible strip change is the data source itself).

**Method / environment.** Linux VPS (2 cores, headless), rustc/cargo 1.99, all runtime evidence on
Xvfb/llvmpipe (layout + pixel truth; no glass on Linux). The macOS-only paths are compile-checked
(`cargo check --target aarch64-apple-darwin`) — there is no macOS host on this machine. Every frame number
below was measured on this machine with the `SURAMAK_FRAME_STATS` seam (procedure in §2.3).

---

## 1. What shipped

### 1) Real waveform peaks (`crates/suramak-playback/src/peaks.rs`, new)

The strip stopped being a silhouette. `Peaks` is a min/max envelope computed **once per source load**, in a
single pass over the decoded samples, clamped to `[-1, 1]` (float sources may exceed full scale; the strip
must never draw past its band):

```rust
Peaks::from_samples(&[f32], bucket_count) -> Peaks   // the one-pass envelope (default 1024 buckets)
Peaks::buckets()   -> &[(f32, f32)]                  // raw, clamped
Peaks::peak()      -> f32                            // the file's max |sample| (0.0 = silence)
Peaks::normalized() -> Vec<(f32, f32)>               // scaled to the file peak
Peaks::range(from_t, to_t) -> (f32, f32)             // normalized min/max over the buckets a span touches
Peaks::from_file(path, bucket_count) -> Result<Peaks, String>   // decode once (symphonia via core::decode)
```

* **Bucket mapping:** bucket `i` covers `[i·len/n, (i+1)·len/n)` of the buffer; a bucket that receives no
  samples (fewer samples than buckets) stays `(0.0, 0.0)` — no invented data.
* **Normalization (documented choice):** the envelope is scaled by `1 / peak` — the file's own maximum — so a
  quiet recording and a loud one both read at usable height, with the shape (relative dynamics) preserved.
  Silence (`peak ≤ 1e-6`) is returned untouched: all-zero buckets, the honest flat baseline. A percentile
  divisor (robust against one loud click) was the considered alternative; the max was chosen for this slice
  because it is exact and unit-testable. **Noted for a later refinement** (see §2.7).
* The strip draws `range(t0, t1)` per bar — each bar shows the true min/max of the audio span it covers
  (~9 source buckets per bar at 1024 buckets over a ~110-column strip), in the frozen M9a geometry (2 px bars,
  5 px pitch, the same hairline axis, playhead and click-to-seek untouched).
* **No audio → flat baseline.** A source with no envelope (recording with `keep_audio` off, a missing file, or
  the decode still in flight) draws a quiet 1.5 px line at zero amplitude plus the M8 reason hint — never the
  old deterministic `sin`+hash silhouette (deleted) and never fake stubs.
* **Delivery:** opening a transcript emits `Effect::LoadPeaks { transcript, path }`; the runtime decodes it on
  a detached `suramak-peaks` thread and requests a repaint when it lands (the event-driven idle loop then
  wakes to draw it). Results are tagged by transcript id: a late decode for a switched-away transcript is
  discarded, never drawn over the wrong strip. A decode failure leaves the baseline and logs the reason.
* **Library-row thumbs: deferred** (workorder allows it). Peaks are per-source-load, not per-row: a sidebar
  row would need either a per-file decode at list time or a persisted thumbnail cache format. Not worth a
  cache format today; noted in §2.7.

### 2) Repaint budget (`crates/suramak-ui/src/cadence.rs`, new + the frame loop)

The M9a build repainted at the present rate in **every** state: `about_to_wait` kept `ControlFlow::WaitUntil(50 ms)`
+ `request_redraw()` whenever `AppState::busy()` held, **and** `window_event` re-requested a redraw for
`RedrawRequested` itself (egui-winit reports `repaint: true` for it), a self-perpetuating full-window loop.
Both are gone. The policy is a pure function, `cadence::repaint_interval(&AppState, now) -> Cadence`:

| state | cadence | why |
|---|---|---|
| idle (nothing in flight) | `EventDriven` — no periodic repaint | input, worker-queue arrivals, animation frames and the peaks job each request their own |
| recording | 100 ms (~10 fps) | the meter/timer; live transcript lines repaint on arrival (`live_tick` reports a published revision) |
| playing | 33 ms (≤30 fps) | the playhead moves continuously while it plays; stops on pause/end |
| model download | 150 ms | progress is polled (no thread wake-up exists for it) |
| unsaved edits | at the autosave due time | the debounce tick must run; the save caption paints then |
| running job / polish run | `EventDriven` | worker events arrive through the bridge (each requests a repaint); the polish thread wakes the loop at completion |

The loop (`main.rs::about_to_wait`) repaints only when the policy's deadline is due or when `pump()` saw a
**discrete** change (a drained event, a background result, a terminal transport transition, a published live
line, a fired tick) — continuously moving values (the playback position, the live meter) deliberately do not
set that flag, so a busy state can never turn it into a busy loop. Animations need no entry: egui's animation
manager requests repaints while a value is in flight and every animation ends (M9a contract).

### 3) Frame instrumentation (`crates/suramak-ui/src/framestats.rs`, new)

Every **presented** frame is attributed to the visible state it was drawn in (recording > playing > job >
polish > download > idle) and counted, with the per-state span accumulated between frames. With
`SURAMAK_FRAME_STATS=1` the summary prints at exit (stdout + the `suramak::frames` log target):

```text
suramak frame stats
elapsed=30.0s frames=9 fps=0.3
idle       frames=9      seconds=0.3  fps=32.6
recording  frames=0      seconds=0.0  fps=0.0
...
```

The plain smoke's `build/smoke_ok.txt` and the job smoke's `build/smoke_job_ok.txt` also carry the raw
`frames=` count. The egui repaint callback logs each egui-side request at `debug` (with its delay) — the
companion when tracing why a frame came.

### 4) Accessibility floor

* **accesskit enabled** (`Cargo.toml`: `egui`/`egui-winit` features only — no new crates). `shell.rs` creates
  the platform adapter with `egui_winit::State::init_accesskit::<AccessKitEnabler>` **before the window is
  first shown** (accesskit_winit panics otherwise; the window is created invisible and shown right after) and
  the action requests arrive through the winit user-event channel (`AccessKitEnabler` in `main.rs`, handled in
  `App::user_event` → `on_accesskit_action_request`). No macOS-only cfg: the same three files carry it on every
  platform; the platform adapter inside accesskit_winit differs (`accesskit_macos` / `accesskit_unix`). On this
  Linux VPS the adapter initialises and stays silent (no AT-SPI bus) — the smokes are green.
* **Semantic labels on every painted control** — the full sweep (label = the string a screen reader
  announces; glyph controls take their tooltip as the label, so both come from one source):

  | control (call site) | label |
  |---|---|
  | `controls::button` / `button_sized` (every labeled tier-1/2/3 action: `Export…`, `Polish`, `Undo`, `Accept`, `Discard`, `Dismiss`, `Save`, `Reset to defaults`, `Re-check`, `Download`, `Delete`, `Set default`, `Stop`, `Import`, `Choose file…`, `Models`, `Settings`, `Save as…`, `Export`) | the visible label |
  | `controls::glyph_button` (trash / confirm / cancel glyphs) | the tooltip (`Delete this transcript`, `Keep this transcript`, `Delete this transcript`) |
  | `controls::glyph_label_button` / `nav_home` | `Home` |
  | `controls::segmented` (engine, language, appearance, export format chips) | the segment label, as a selected/radio node (`Markdown`, `Text`, `System`, `Light`, `Dark`, …) |
  | `controls::transport` capsule | `Skip to start`; `Play` / `Pause` (follows state) |
  | floating pill (`views::float_control`) | `Record` / `Stop recording` |
  | waveform strip (`editor.rs`) | `Waveform — click to seek` (enabled only when seeking is possible) |
  | transcript gutter timestamp (`editor.rs`) | `Seek to hh:mm:ss` |
  | sidebar + home rows (`library.rs`, `home.rs`) | `Open <title>` (+ selected state) |
  | stock widgets (out of scope, workorder) | settings checkboxes/sliders/combobox, record gain/mute, every `TextEdit` — egui's own nodes |

  Two new tests run a real frame with accesskit enabled and assert the labels are in the tree (controls + full
  editor).
* **Focus rings** — every painted control was already ringed by `painted_body` (M9a); this slice adds the ring
  to the one-off painted controls that lacked it: the floating pill, the waveform strip, the gutter timestamps,
  the sidebar rows and the home rows. Tab order is egui's registration order (sidebar → deck → title line →
  rows → pill) and is screenshot-verified (§2.4).
* **Keyboard map** (`crates/suramak-ui/src/app/keymap.rs`, new) — one central dispatch, unit-tested, with the
  text-edit focus guard:

| key | action | scope | guard |
|---|---|---|---|
| Space | play / pause | editor, source playable | not while a text edit has focus |
| ⌘R | start / stop recording | anywhere (the pill's action) | not while a text edit has focus |
| ⌘E | open / close the export sheet | editor | not while a text edit has focus |
| Esc | close the sheet, else leave the editor | sheet open / editor | **always** (the documented exception) |
| ↑ / ↓ | previous / next segment (seek to its start) | editor, source playable | not while a text edit has focus; repeats count |
| ⌘⌫ | arm the delete confirm (the existing flow) | editor | not while a text edit has focus |

  The guard is `TextEditState::load` on `Memory::focused` — the same information any in-pass check would see.
  Dispatch runs in `Shell::render` on the **raw input before egui sees it**: a matched shortcut's key event is
  removed, so a focused widget, egui's own Tab/arrow focus movement and the keymap can never both act on one
  press (this is also what stops ↑/↓ from double-acting as focus movement). Segment navigation reuses the
  playhead → active-row crossfade; play/pause and the sheet go through the same `AppState` methods the pointer
  controls use (the Motion section's "reuse existing transitions", by construction).
* **Not in scope (workorder):** settings checkboxes/sliders and the record view's gain/mute controls keep the
  themed stock egui widgets (their stock a11y nodes are fine); the record view's copy stays for M9c.

### 5) Motion (per the motion brief; small)

* **Peaks appear with a base fade on first paint only** (`Motion::arrive`, one id per transcript; no growth
  animation; instant under Reduce Motion).
* Keyboard-triggered actions reuse the existing transitions by construction (see above). The export sheet has
  **no** open transition in M9a (the Area is `fade_in(false)`); ⌘E therefore reuses the identical path as the
  button — no new motion was added, per "nothing else new".
* Nothing else new; no M9a motion application was touched.

### 6) Per-file change summary

| file | change |
|---|---|
| `crates/suramak-playback/src/peaks.rs` | **new**: the envelope (`from_samples`/`buckets`/`peak`/`normalized`/`range`/`from_file`) + 7 tests |
| `crates/suramak-playback/src/lib.rs` | `pub mod peaks` + re-exports (`Peaks`, `PEAKS_BUCKETS`) |
| `crates/suramak-ui/src/cadence.rs` | **new**: the repaint-cadence policy (`Cadence`, `repaint_interval`) + 7 tests |
| `crates/suramak-ui/src/framestats.rs` | **new**: frame accounting (`FrameState`, `classify`, `FrameStats`, `enabled`) + 3 tests |
| `crates/suramak-ui/src/app/keymap.rs` | **new**: the central keyboard map (`Shortcut`/`KeyContext`/`Action`/`resolve`/`segment_target`/`dispatch`) + 11 tests |
| `crates/suramak-ui/src/app/editor.rs` | strip draws `Peaks::range` per bar + the flat baseline; the `sin`+hash silhouette deleted; peaks fade-in; strip label + focus ring; gutter stamps labeled + ringed; +2 tests (a11y tree, geometry unchanged) |
| `crates/suramak-ui/src/app/controls.rs` | `widget_info` labels on button/glyph/glyph+label/segmented/transport (label = tooltip for glyphs; segment state included); +1 a11y test |
| `crates/suramak-ui/src/app/views.rs` | pill: `widget_info` label (Record ↔ Stop recording) + focus ring |
| `crates/suramak-ui/src/app/home.rs`, `library.rs` | rows: `widget_info` label ("Open <title>", selected state) + focus ring |
| `crates/suramak-ui/src/app/mod.rs` | `EditorPeaks` state + `Effect::LoadPeaks` + `strip_peaks()`/`on_peaks_loaded()`; `open_editor` returns its effects (the peaks job); +4 tests |
| `crates/suramak-ui/src/runtime.rs` | the peaks job (`start_peaks`/`peaks_status`, thread `suramak-peaks`); `live_tick` returns whether it published a line; shutdown note |
| `crates/suramak-ui/src/shell.rs` | accesskit adapter init before show; window created invisible; the keymap dispatch seam in `render`; `FrameStats` per presented frame; repaint-callback debug log |
| `crates/suramak-ui/src/main.rs` | `AccessKitEnabler` + user-event loop (`EventLoop<AccessKitEnabler>`), `user_event`; `pump()` returns discrete-change; `about_to_wait` cadence policy + smoke 50 ms watchdog wakes; the `RedrawRequested` re-request removed; frame-stats print at exit; `frames=` in the job smoke file |
| `Cargo.toml` | `egui`/`egui-winit` gain the `accesskit` feature (features only) |

---

## 2. Verification

### 2.1 Gates

```text
cargo fmt --all --check                                  → clean
cargo clippy --workspace --all-targets -- -D warnings     → clean
RUST_TEST_THREADS=1 cargo test --workspace                → 310 passed / 0 failed / 6 ignored  (log: build/m9b/tests2.log)
cargo check -p suramak-glass --target aarch64-apple-darwin → clean
```

**Tests: 310 total (baseline 276 → +34 new).** New tests (all listed by name in `build/m9b/tests2.log`):

* peaks math (7): `silence_is_all_zeros`, `a_constant_signal_is_flat_in_every_bucket`,
  `out_of_range_samples_clamp_to_full_scale`, `bucket_counts_hold_at_the_edges`,
  `normalization_scales_quiet_files_and_keeps_loud_ones`, `range_aggregates_the_buckets_a_span_touches`,
  `a_real_wav_file_decodes_into_a_usable_envelope` (decodes `fixtures/audio/en.wav`).
* cadence policy (7): `an_idle_app_is_event_driven`, `recording_meters_at_ten_fps`,
  `playback_runs_at_thirty_fps_and_stops_on_pause`, `a_running_job_is_event_driven`,
  `a_download_wakes_for_its_progress`, `unsaved_edits_wake_at_the_autosave_due_time`,
  `recording_outranks_playback_and_edits`.
* frame stats (3): `frames_are_attributed_to_the_state_that_was_on_screen`,
  `classification_follows_the_cadence_priority`, `the_summary_lists_every_state_and_the_total`.
* keymap (11): `space_toggles_playback_only_in_the_editor_with_a_source`,
  `the_text_edit_focus_guard_blocks_every_shortcut_except_escape`,
  `command_r_toggles_recording_and_stops_a_live_session`, `command_e_toggles_the_sheet_only_in_the_editor`,
  `escape_closes_the_sheet_before_leaving_the_editor`, `arrow_keys_move_between_segment_starts`,
  `command_backspace_arms_the_existing_confirm_flow`, `dispatch_applies_the_action_and_consumes_the_key`,
  `dispatch_never_fires_while_a_text_edit_is_focused` (a real focused `TextEdit` frame),
  `dispatch_handles_the_command_shortcuts`, `dispatch_leaves_unhandled_and_guarded_keys_alone`.
* peaks state (4): `opening_a_transcript_with_audio_requests_its_envelope`,
  `a_source_without_audio_draws_the_baseline_and_never_starts_a_job`, `a_stale_peaks_answer_is_discarded`,
  `a_failed_decode_leaves_the_baseline_and_records_the_reason`.
* a11y tree (2): `every_painted_control_exposes_its_label_to_accesskit` (controls),
  `the_editor_exposes_labelled_accesskit_nodes` (a full editor frame with accesskit enabled: waveform,
  transport, Export…, Polish, trash, gutter stamp).

### 2.2 xvfb smokes (final binary)

* plain: `SURAMAK_SMOKE_SECS=30 xvfb-run …` → exit 0, `build/smoke_ok.txt` (frames=9), log `build/m9b/after-idle.log`.
* record view: `SURAMAK_SMOKE_RECORD=1 SURAMAK_SMOKE_SECS=30 …` → exit 0 (frames=296), log `build/m9b/after-record.log`.
* job: `SURAMAK_SMOKE_JOB=fixtures/audio/en.wav SURAMAK_SMOKE_SECS=300 …` → exit 0, `build/smoke_job_ok.txt`
  (2 segments, model tiny, engine cpu, frames=11), log `build/m9b/after-job.log`.

### 2.3 The frame table (raw numbers, same machine, same scenarios)

Measured with the **M9a binary** first (the tree at `8e472db` plus a reporting-only change — the `frames=` line
in the job smoke file; the pristine pre-patch binary is preserved at `build/m9b/suramak-m9a-baseline`), then
with the M9b build. Reproduce: `SURAMAK_FRAME_STATS=1 SURAMAK_SMOKE_SECS=<n> xvfb-run -a
-s "-screen 0 1280x800x24" ./target/debug/suramak` (+ `SURAMAK_SMOKE_RECORD=1` / `SURAMAK_SMOKE_JOB=…`), then
read the block from the log (`build/m9b/*.log`, kept; baseline logs `build/m9b/baseline-*.log`).

| scenario (llvmpipe, 1280×800) | before frames | before secs | before fps | after frames | after secs | after fps | cut |
|---|---|---|---|---|---|---|---|
| idle, plain smoke 30 s | 873 | 30 | 29.1 | **9** | 30 | **0.3** | −99 % |
| record view, demo recording 30 s | 563 | 30 | 18.8 | **296** | 30 | **9.8** | −47 % |
| record view, demo recording 240 s | 4,487 | 240 | 18.7 | **2,328** | 240 | **9.7** | −48 % |
| job run (`fixtures/audio/en.wav`, tiny, cpu) | 353 | 18.8 | 18.8 | **11** | 7.5 | **1.5** | −97 % (fps −92 %) |

The 240 s record row is the analogue of the critique's measurement (7,733 frames / 240 s on the owner's M4
Mac). Per-state attribution (after): the idle run's 9 frames all land in the first 0.3 s (startup animations),
then nothing for 29.7 s; the record runs are 100 % `recording` frames; the job run is 100 % `job` frames.

**Honest caveat (workorder):** these VPS frames proxy *cadence*, not the macOS glass compositing cost. On the
Mac the same loop change removes full-window composites (Metal render + WindowServer blur per frame) — the
throttle's purpose is fewer of those; the frame counts here measure the scheduling, and the ASR cost is
separate and untouched.

### 2.4 Screenshots + pixel evidence (harness `build/m9b-e2e/`)

`seed.py` builds a data dir whose transcript audio is a **copy of `fixtures/audio/en.wav`** (9.0 s, 16 kHz
speech: silent lead-in, speech, a ~2.9–3.8 s silent gap, speech, a silent tail after ~8.1 s) plus a second
transcript with **no audio** (`keep_audio` off). `shoot.sh` drives the app on Xvfb (:97), `pixelcheck.py`
asserts on the pixels (`build/m9b-e2e/pixel-checks.log`).

| shot | what it proves |
|---|---|
| `light/editor-peaks.png` | the strip on the real fixture: tallest bar 92 px, column-height stddev 27.0 px (variable heights = real data), the t 0.30–0.55 s speech region 88 px tall, the silent gap (t 2.95–3.75 s) ≤ 4 px, the silent tail (t 8.3–8.9 s) ≤ 4 px |
| `light/editor-noaudio.png` | the no-audio transcript: no waveform column above 12 px (the tallest is the 11 px hint-text ink), the flat baseline line present (1,600 px of baseline across the strip's centre), and never the old silhouette (11 px vs 92 px) |
| `light/editor-tab-1.png` | Tab 1 rings the first sidebar row (1,196 accent-ring pixels changed, x 8..285 y 58..97) |
| `light/editor-tab-3.png` | Tab 3 rings the deck transport's skip control (202 accent-ring pixels, x 311..351 y 76..112) |
| `light/editor-tab-5.png` | Tab 5 rings the waveform strip itself (2,483 accent-ring pixels, x 403..979 y 59..129) — every painted control is Tab-reachable and visibly ringed |

Tab order observed: sidebar row 1 → row 2 → transport skip → transport toggle → waveform strip → Export… → …
(`xdotool key Tab` under `xdotool windowfocus`; the first Tab after a mouse click is swallowed by egui's
focus bookkeeping, so the harness sends one warm-up Tab first — noted here, not a product bug).

### 2.5 Darwin cross-check

```text
cargo check -p suramak-glass --target aarch64-apple-darwin   → clean (35 s; log: build/m9b/darwin-check.log)
```

That is the workorder's gate and it is green on the final tree. The wider `cargo check -p suramak-ui --target
aarch64-apple-darwin` **cannot run on this VPS**: it aborts in a C dependency's build script (`ring` for
rustls: `cc … -arch arm64 -mmacosx-version-min=11.0`), because the machine has no macOS SDK/clang cross
toolchain — an environment limit, not a code error. What that check would have covered for this slice is
covered instead by the fact that **the M9b changes add no macOS-only code**: the accesskit wiring
(`Cargo.toml` features, `main.rs`'s event-loop type + user event, `shell.rs`'s adapter creation + window
show order) is plain cross-platform code (the platform adapter is selected *inside* accesskit_winit:
`accesskit_macos` on macOS, `accesskit_unix` here), and every other changed file is UI code that the Linux
check + clippy compile in full. Modules the macOS accesskit path touches: `Cargo.toml`, `main.rs`, `shell.rs`
(no cfg gates added).

### 2.6 Deviations

1. **The `RedrawRequested` self-repaint loop was removed** (`window_event`): egui-winit returns
   `repaint: true` for `RedrawRequested`, and re-requesting a redraw from the redraw handler made the M9a
   build paint at the present rate even when "idle". This is the largest single contributor to the table's
   idle row and is required by the workorder's "idle: no periodic redraws".
2. **⌘⌫ arms the confirm, it never deletes directly** — "the existing confirm flow" is trash → confirm; the
   shortcut enters it at the same step as the trash glyph.
3. **↑/↓ require a playable source** (they seek the playhead; the active-row crossfade follows it). For a
   transcript with no audio there is no playhead, so the keys stay inert (the gutter click-to-seek has the
   same rule). Documented rather than inventing a second selection concept.
4. **Esc leaves the editor only from the editor** (and closes the sheet first). Elsewhere (settings, models,
   record) it is not bound — the workorder scopes it to "close sheet / leave editor"; a general "back" key is
   M9c's call.
5. **Transcript virtualization deferred** (workorder allows it). `ScrollArea::show_rows` needs a uniform row
   height; transcript rows are wrapping `TextEdit::multiline` at the 68 % measure, so their height is
   content-dependent (1–3 lines in the fixtures). A fixed `row_height` would clip text or leave gaps; and the
   per-row editing state (focus, per-row buffers, `scroll_to_rect` active-row follow) assumes every row is
   laid out — `show_rows` skips widgets entirely, so a row being edited could vanish mid-edit and the follow
   scroll could target a skipped row. With the cadence policy in place the per-frame re-layout cost only
   occurs while actively scrolling/typing, which is not the heat driver (the full-window composite was).
   Precise deferral, revisit with a measured-height virtualizer in a later slice.
6. **Library-row waveform thumbnails deferred** (workorder: skip unless trivially free) — peaks are per-source
   decode; a row would need a per-file decode or a persisted thumbnail cache (§2.7).
7. **The peaks job decodes the file a second time** (once for the strip, once when playback opens its engine).
   Deliberate: the UI must draw the strip before the first Play, and the engine's decode is owned by its
   thread/session. The decode is off the UI thread in both cases; for the fixture-sized files this is
   milliseconds. A shared decode cache is possible later if a large-file case shows up.
8. **The export sheet has no open transition to reuse** (M9a ships it with `fade_in(false)`), so ⌘E plays the
   same instant appearance as the button. Adding a transition would be new motion, which this slice forbids.
9. **Smoke watchdogs keep a 50 ms wake** (not a repaint): the smoke modes check their deadlines in
   `about_to_wait`, so they need periodic wakes even in the idle state; the wakes do not repaint (the idle
   smoke's 9 frames prove it). Normal runs have no such wake.
10. **`frames=` was added to `build/smoke_job_ok.txt`** (additive) so the job scenario has a raw frame count
    in the same file as the job's other evidence; nothing parses that file programmatically.
11. **`Runtime::live_tick` returns a bool now** (did it publish a provisional revision?) — a return-value
    change in the UI-side glue so the frame loop can repaint on a line arrival without a periodic cadence.
    The live pipeline itself (suramak-asr's scheduler, decode thread, capture, mix) is untouched: the same
    calls happen in the same order; `live_step` only records that it published.

### 2.7 En route (for M9c / later)

* **Motion grammar (M9c)**: the sheet/menu/popover transitions and the rest of §5 of the motion brief (the
  sheet's instant appearance is the first candidate), plus the per-surface `impeccable animate` passes and the
  finish review that walks the brief's §5 state list.
* **Copy sweep (M9c/M9e per the brief)**: the record view's `meter samples` line, `engine: … planned …`, the
  remaining clock formats — untouched here.
* **Peaks refinements**: a percentile divisor (robustness against a single loud transient) if the owner's real
  recordings read badly; a persisted per-file thumbnail for library rows; peaks for the record view's live
  waveform (it already shows real RMS).
* **Virtualization** (see deviation 5) with a measured-height virtualizer once the editor's row layout is
  stable enough to cache heights.
* **Accessibility follow-ups**: a shortcut sheet / Help menu (H10), announcing live arrivals (polite
  live-region updates), and a Reduce Motion source on Linux/Windows (still `false`).
* **The `busy()` predicate** is now only the coarse fast path of the cadence policy (plus the tests); if a
  future state needs periodic work, add it to `cadence::repaint_interval` — not to the loop.
