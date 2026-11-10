# M13g2 — the compact phone UI, part 2: exports, motion, battery, CI, a11y

Workorder: `docs/WORKORDER-M13G2.md` (plan source `docs/PLAN-M13-ANDROID.md` §3
M13g, split into G1/G2; G1 is `docs/reports/M13g1-compact-library.md`). Base
tree: `main` post-`08db85e` (1026 tests, five CI platforms green).

**Scope delivered:** the compact editor's export sheet writes through
`ACTION_CREATE_DOCUMENT` with the M15 serializers (six formats, byte-identical
to the desktop) and offers the disclosed one-file `ACTION_SEND` alternative;
the compact motion pass (two-layer push/pop, sheet, list, progress, the
predictive-back mid-transition contract, Reduce Motion = springs only); the
job-time frame budget (no idle redraws); the `tests/ui-compact` suite as a
named linux CI step; and the compact a11y floor (labels, roles, focus rings).

**No emulator and no device.** Every runtime sentence below is a host test, a
compile/artifact claim, or a screenshot inspected by a human. §8 is the honest
boundary and §9 is the owner-device checklist.

---

## 1. Exports: `ACTION_CREATE_DOCUMENT` through the M15 serializers

### 1.1 The path, once

```text
export sheet (six chips + Share)                  deck/compact/editor.rs
  → CompactAction::Export { transcript, format, bilingual, suggested_name }
  → MainActivity.exportFromCompact(format, bilingual)          Kotlin
  → UiBridge.export → compact_host::prepare_export               (JNI, one JSON op)
  → export_job::render        ← suramak_core::export::to_*    (the M15 serializer)
  → ExportJobs::stage         ← .part → write 16 KiB chunks → fsync → rename
                                {job, path, name, bytes, checksum, mime}
  → ACTION_CREATE_DOCUMENT (mime + EXTRA_TITLE) → the frame the user picks
  → ExportWriter.write        ← 16 KiB copy, byte count + FNV-1a 64 re-checked
  → UiBridge.exportDone(job, bytes) → ExportJobs::finish
                                verify count → delete the staging dir
  → sheet state: Written { name, bytes } → "Saved Morning notes.srt · 12.5 KB"
```

The share alternative is the same head and tail with one different middle:
`export_share` stages the same bytes, Kotlin hands the staged file's
`FileProvider` URI to `Intent.ACTION_SEND` (one file), and `export_shared`
releases the job while leaving the file for the receiving app. The next process
start prunes it. Both routes are named in the sheet, with the picker and
`ACTION_SEND` spelled out — the disclosure the workorder asks for.

### 1.2 The serializers are the desktop's, called directly

| chip | extension | MIME (`ACTION_CREATE_DOCUMENT`) | core function |
|---|---|---|---|
| Markdown | `md` | `text/markdown` | `export::to_markdown` |
| Text | `txt` | `text/plain` | `export::to_txt` |
| JSON | `json` | `application/json` | `export::to_json` |
| SRT | `srt` | `application/x-subrip` | `export::to_srt` / `to_srt_bilingual` |
| VTT | `vtt` | `text/vtt` | `export::to_vtt` / `to_vtt_bilingual` |
| TSV | `tsv` | `text/tab-separated-values` | `export::to_tsv` (new, shared) |

`export_job::render` is the phone's **only** formatting function; the picker
and the share route both call it. `to_tsv` is new in `suramak-core` and is now
in the desktop `ExportFormat` list and the CLI as well, so the "keep parity"
instruction holds: the phone's six chips are exactly the desktop's six
non-ASS formats, in the same order.

**Subtitle diagnostics and the bilingual stacked paths are included.** The
sheet's translation switch is offered only for SRT/VTT (the formats with a
second-line slot) and only when the transcript actually has stored
translations; `render` refuses the other combinations with a sentence instead
of silently writing one language. The bilingual path calls
`to_srt_bilingual` / `to_vtt_bilingual` verbatim. The desktop's subtitle
diagnostics (`suramak_core::diagnostics::diagnose`, the row-level line-length
and duration badges) are advisory UI only — they never rewrite an export — so
"the phone writes the desktop's bytes" is true both with and without them; the
phone simply has no row badges in this cut.

### 1.3 Byte parity is asserted, not claimed

Three layers:

1. `export_job`'s `every_format_stages_exactly_the_core_serializer_bytes` walks
   all six formats, reads the staged file back and compares it byte-for-byte
   with the core function's output, then verifies the reported length and
   checksum.
2. `a_bilingual_staging_matches_the_stacked_serializer` does the same for
   `to_srt_bilingual`, including the translated lines.
3. `compact_host`'s `an_export_stages_exactly_the_desktop_serializer_bytes`
   drives the real bridge op (`{"op":"export","format":"srt"}`), reads the
   staged file and compares it to `suramak_core::export::to_srt` of the row read
   back from sqlite. This is the same function the desktop's
   `ExportFormat::render` calls; there is no second serializer to drift.

### 1.4 The write is bounded and atomic

`ExportJobs::stage` renders once, writes `<name>.part` in 16 KiB chunks,
`sync_all`s it, then renames it into `<files>/exports/<job>/<name>`. A reader
(and the platform) never sees a partial file. `finish` verifies the byte count
the platform reports; a short or long write fails with the numbers and keeps
the staged bytes for a retry. The job is single-flight and bound to its
transcript id; a second export while one is live is refused rather than queued.
Kotlin recomputes the FNV-1a 64 checksum while copying, so a provider that
rewrites the stream is caught even when the byte count matches.

### 1.5 The Kotlin half

`ExportWriter.kt` owns the `ActivityResultContract` that takes the staged export
(so the MIME type is per-format, not fixed at registration), the bounded copy
with checksum/byte verification, and the `FileProvider` share intent.
`MainActivity` wires it to `exportFromCompact` / `shareFromCompact`; failures
call `export_failed` with the host's reason (the sheet shows it and offers Try
again), a cancelled picker calls `export_cancel` (back to the chips), and a
success calls `export_done` (the sheet shows Saved). The manifest's existing
`FileProvider` already covers the whole `files/` tree, so the share URI needs no
new provider path.

---

## 2. Motion polish

### 2.1 Navigation push/pop — two layers, one spring

`deck::nav` now paints **two** panes during a transition:

| | outgoing | incoming |
|---|---|---|
| push | −18 % parallax over the spring, dimmed up to 30 % by the fade | enters from +100 % width, fades 0→1 |
| pop | slides to +100 % width, fades 1→0 | starts at −18 %, fades 0.55→1 |

Both directions use the one `motion::spring_ease` over the 180 ms base token;
the overshoot stays inside `SPRING_OVERSHOOT` (2 %) and lands exactly at 1.0.
The pop keeps its outgoing screen in `NavStack::outgoing` until the transition
settles, so the editor is painted while it slides away; `advance()` then drops
it and `CompactApp` releases the editor data. Tests cover the layer geometry at
the endpoints, the outgoing token's lifetime, and the exact landing.

### 2.2 Sheet present/dismiss

`compact::motion::SheetAnim` animates the export and quit sheets: present =
180 ms fade + 14 pt rise, dismiss = 120 ms fade (exits faster than it enters,
the Deck's rule). A mid-flight flip retargets from the **current** value — it
never snaps back and never queues. While closing, the sheet is still painted
but non-interactive. The scrim, body and controls share one opacity.

### 2.3 List insert/delete

`ListAnim` diffs the library's keys each frame: a new row fades and rises 6 pt
over the base token; a removed row becomes a ghost at its old index and
collapses over the fast token before it is dropped. The first frame is static
(the brief's "the initial frame is static"). The editor's segment rows are not
animated as inserts: search filters must not read as deletions.

### 2.4 Progress

Two progress surfaces, both ending exactly on their value:

* the editor playhead is drawn through the Deck's `seek_ease`: a jump > 120 ms
  eases over the base token while playback advance is followed exactly (the
  M9d rule, unchanged);
* the Library's job strip eases its fill to each reported percentage over the
  base token, and an identical percentage is a no-op — which is what keeps a
  job poll from becoming an animation.

### 2.5 Reduce Motion removes only the springs

The rule this milestone implements: **travel is the vestibular part; opacity is
not.** Under Reduce Motion:

* navigation travel is settled on the first frame (`offset == 0`, no
  overshoot), but a **120 ms opacity crossfade still runs** — a hard cut is what would
  make a reduced-motion user lose their place;
* the sheet keeps its fade and loses its 14 pt rise;
* list inserts appear immediately and deletes drop immediately (no slide);
* the playhead jumps (the shared `seek_ease` reduce path, unchanged).

This deliberately supersedes G1's "Reduce Motion is a hard cut" sentence for
navigation; the new contract is asserted by
`reduce_motion_removes_the_travel_and_keeps_one_crossfade` (nav),
`reduce_motion_keeps_the_sheet_fade_but_removes_the_rise` (sheet),
`reduce_motion_makes_a_list_insert_and_delete_immediate` (list), and the
phone-viewport test `reduce_motion_removes_the_travel_and_keeps_one_crossfade_token`.

### 2.6 Predictive back, mid-transition — the exact contract

The gesture is still the v2.4 funnel: `MainActivity.handleBack` is reached from
`OnBackPressedDispatcher`, `OnBackInvokedCallback` and `KEYCODE_BACK`, deduped
at 250 ms, and asks `UiBridge.back()` first. The one state-machine answer is
`CompactApp::back`:

1. **A sheet is open** → the sheet is dismissed (its exit animation runs) and
   the gesture is consumed. A second back while it is already exiting is
   ignored — the exit is not restarted. Backing out of the export sheet also
   cancels its staged job (there is no longer anything that could finish it).
2. **No sheet, stack depth > 1** → the top screen is popped **at once**. If a
   push or pop spring is in flight, that transition is replaced by the pop from
   the stack as it stands: the screen the stack calls current is the one that
   leaves, its pane becomes the outgoing layer, and the screen below is the
   incoming one. Nothing is queued and nothing waits for the old spring to
   finish; the animation always ends and requests no frames after its token.
3. **At the root** → `finish:true`. The Android shell falls through to the
   existing v2.4 `nativeBackPressed` funnel; mapping that to
   `finishAndRemoveTask()` is the shell swap (§8), not a Library-screen rule.

This is the whole contract: the gesture never has to know what is animating,
because the answer is derived from the stack, not from the transition.

### 2.7 Fling — deferred, with a probe (which)

**Decision: deferred with a probe, not implemented.** egui's `ScrollArea` owns
the drag offset and its kinetic continuation; adding a second momentum model in
the Deck layer would fight the offset egui is already writing, and nothing in
the host evidence says the built-in momentum is insufficient — that is a feel
question only the owner's thumb can answer. So the Deck layer carries a
**probe** instead:

* `FlingTracker` samples the pointer velocity on drag frames and starts on
  release (egui zeroes the velocity as the finger lifts, so the release frame
  alone measures nothing);
* it accumulates how far the scroll offset travels afterwards, for how many
  frames, and stops when the offset stops (or at 180 frames);
* `FlingProbe::expected_distance` is `release_velocity × 0.12 s` — the Deck
  layer's expectation for comparison, not a claim about egui;
* the result is on `CompactApp::fling_probe` and in the host snapshot; the
  owner run prints `fling <v> pt/s → <d> pt over <n> frames (egui expectation
  <e> pt)`.

If the numbers (or the feel) disagree, the fix belongs in `deck/motion.rs` and
`ScrollArea` is replaced by an offset the Deck owns — that is the follow-up
this probe exists to justify. §9 keeps the feel on the owner checklist.

---

## 3. Battery: no idle redraws, job or not

The compact surface has exactly these frame sources:

| source | requests frames | stops |
|---|---|---|
| navigation spring/crossfade | only between endpoints | at the token |
| sheet present/dismiss | only while the fade moves | at the token |
| list insert/delete | only while a row/ghost is mid-flight | at the token |
| playhead seek ease | only while a seek eases | on landing |
| job progress fill | only while the fill moves to a new value | on landing |
| job **events** | one wake per changed value | immediately |

`CompactApp::on_job(label, done, total, now)` returns false for a repeated
value, so a worker polling the same percent does not produce an event.
`take_frame_request()` is a one-shot take; `needs_frame()` is the host's idle
question. The test `a_settled_screen_with_a_live_job_requests_no_frames` runs
three frames on a warm context with a live job and asserts the last frame's
`ctx.has_requested_repaint()` is **false**; `a_job_event_asks_the_host_to_wake_exactly_once`
asserts one wake per change.

The M16.1 pipeline is untouched: one inference slot, bounded decodes. The
export job is not an inference job; it is a separate single-flight slot so it
cannot queue behind (or in front of) transcription.

---

## 4. CI gating

`.github/workflows/ci.yml`, linux job, immediately after `Tests (debug
profile)`:

```yaml
- name: UI compact (phone viewport, Xvfb)
  run: bash tests/ui-compact/run.sh
```

`tests/ui-compact/run.sh` runs the seven phone-viewport tests under `xvfb-run`,
compresses the PNGs when ImageMagick is present, and fails on a missing shot or
a wrong size. A layout/a11y/motion regression in the compact profile is now a
red linux build, not a review note.

---

## 5. a11y floor

`deck/compact/a11y.rs` is the one constructor every compact control uses:

* `touch(...)` creates the interaction, attaches `WidgetInfo::labeled(role,
  enabled, label)` (what TalkBack reads and Switch Access focuses), paints the
  Deck focus ring when the control has focus, and records the control in the
  frame's audit;
* `record_button` covers the painted buttons whose body comes from the shared
  control kit (that kit already sets `WidgetInfo` and its own ring);
* `describe` records read-outs that are not controls (meta lines, progress,
  the job strip).

Labels are the spoken names: "Back", "Open transcript" rows by title, "Segment
N · <text>", "Play"/"Pause", "Find", "Export", "Export format: SRT", "Include
the translation", "Share instead", "Quit Suramak". Focus rings are the shared
geometry (`FOCUS_RING_EXPAND` 1.5 pt outside, `tokens.stroke.focus` wide,
accent).

The phone-viewport test `every_compact_control_carries_a_label_and_a_role`
renders the Library, the editor and the export sheet and asserts the registry
is non-empty, every label is non-empty, and the expected controls are present.
`a_focused_control_paints_the_deck_ring` asserts the ring is drawn for a focused
response. 48 dp targets are already asserted by the hit-target test.

---

## 6. Verification (run in this workspace)

| gate | command | result |
|---|---|---|
| fmt | `cargo fmt --all --check` | clean |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| Android compile (Deck) | `ANDROID_NDK_HOME=… cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-ui --all-targets -- -D warnings` | clean |
| workspace tests | `RUST_TEST_THREADS=1 cargo test --workspace` | **1062 passed, 0 failed, 10 ignored**, exit 0 (1026 baseline → +36) |
| phone-viewport suite | `bash tests/ui-compact/run.sh` (Xvfb) | 7 tests pass; 7 PNGs at 1216×2640 |
| CLI batch loop | `cargo test -p suramak-cli` | green after a gate-found fix (below) |
| Kotlin | `:app:compileDebugKotlin` with `allWarningsAsErrors = true` | `BUILD SUCCESSFUL` |
| APK | `bash scripts/android/build-apk.sh` | green: **36 056 369 bytes (34 MiB)**, sha256 `258bb5f0335b2a15571ecc91122f8c751ef61b698aec0f599931593d7902ae85`, versionCode **10** / `0.4.0-m13g2.1`; aapt/dexdump assertions (version, `ExportWriter`, manifest) passed |
| packaged manifest/dex | aapt + dexdump (inside the build script) | versionCode **10**, versionName `0.4.0-m13g2.1`; `ExportWriter` in the dex; FileProvider/`ACTION_SEND`/`ACTION_OPEN_DOCUMENT` present |

Growth: `suramak-core` 18 → 20 export tests (TSV golden + whitespace folding),
`suramak-ui` 409 → 434 in the package (nav layer tests, motion primitives,
export flow/frame-budget/fling model tests, a11y tests, three more
phone-viewport tests), `suramak-android` 35 → 44 (the `export_job` staging/
finish/cancel/bilingual/failure suite plus the host parity, cancel and
failed-write tests).

### 6.1 The shots, inspected

All seven are in `tests/ui-compact/shots/` and were reviewed at full size while
building this cut:

* **`export-sheet.png`** — six chips in a 3×2 grid (Markdown/Text/JSON,
  SRT/VTT/TSV), SRT selected with the accent outline, the translation checkbox
  checked, the two disclosure lines ("Picker: the system file chooser writes to
  the frame you pick." / "Share: hands this one file to another app
  (ACTION_SEND)."), then "Share instead" and "Export SRT". Inspected: no
  overlap, no clipped chip labels, the disclosure lines are separate and clear
  of the buttons.
* **`export-written.png`** — the picker-result state: a card reading **Saved**,
  "Morning notes.srt · 12.5 KB", "SRT · application/x-subrip", and a Done
  button. Inspected: the first pass printed "0 MB" for a 12 KB file, which read
  as an empty export; the sheet now has a file-size formatter (B/KB/MB) and the
  shot was regenerated. No overlap.
* **`settings-tiers.png`** — the M13D low-tier statement: "below the 4 GB floor
  · only tiny runs here", the `tiny · recommended` row with "78 MB · CI + tests
  · installed" and "In use", then "5 larger models hidden — not offered on this
  phone". Inspected: the tier line, the offered row and the hidden note are all
  legible; nothing runs under the action column.
* **`library.png` / `library-light.png` / `editor.png` / `settings.png`** —
  re-rendered after the two-layer change; the library, the selected segment,
  the transport and both ladders are unchanged. Inspected for overlap and
  clipping: none.

The shots are review artifacts, not pixel goldens (the rasteriser is
deterministic, but CI gates on geometry/state assertions; see the README).

---

## 7. The completion notification, Quit, and what did not change

G1's deep link, Quit receiver and the M13E/F services are untouched. The export
ops are additive JSON arms beside them. The desktop's export sheet gains the
seventh chip (`TSV`) through the same shared serializer; all 349 desktop tests
still pass, and the desktop suite's expectations were not edited.

**One gate-found fix outside the compact surface.** The full workspace run
caught a pre-existing race in `suramak-cli`'s batch loop: `while !driver.is_idle
{ run_once(); drain_events() }` skips the drain when the last `run_once` both
produces the terminal events and makes the driver idle, so `--cancel` batches
intermittently reported zero outcomes (the `cancel_by_position…` test failed
about half the time). The loop now drains after every pump and breaks on idle
*after* the drain; the test passed 10/10 consecutive runs. No engine, model or
service code changed.

---

## 8. The honest boundary

1. **No emulator, no device.** The picker, the SAF stream, the share chooser
   and the checksum-verified copy were never executed on Android. What is
   proven: the host ops, the staged bytes, the Kotlin compile, the packaged
   dex/APK, and the state machine the sheet renders.
2. **The visible phone surface is still the M13b spike shell.** The compact
   routes are drawn in the host suite and screenshot-gated; the APK's window
   still belongs to `suramak-android-spike`. `MainActivity.exportFromCompact` /
   `shareFromCompact` are the platform seam those routes call; on this APK no
   thumb can reach the export sheet yet. That is the same G1 boundary, not a
   new one — and it is why the owner checklist's export item is an acceptance,
   not a regression test.
3. **The root back gesture still falls through to the v2.4 spike funnel**
   (`nativeBackPressed`). The compact stack returns `finish:true` and the shell
   decides; `finishAndRemoveTask()` belongs to the shell swap.
4. **Fling is deferred with a probe** (§2.7). The probe exists and is wired to
   the library's ScrollArea; the decision is the owner's thumb.
5. **Playback-synced segment highlighting and onboarding copy** were named in
   the plan's G2 sentence but are not in the G2 workorder; they are not
   delivered here and remain open.
6. **Predictive back's system preview** is the platform's (the app registers
   `OnBackInvokedCallback` and answers at commit). The mid-transition rule in
   §2.6 is the app's side of that contract; the gesture's visuals are the OS's.

---

## 9. Owner-device checklist (the runtime boundary)

Install the debug APK from `scripts/android/build-apk.sh`
(`0.4.0-m13g2.1`, versionCode 10), then:

1. **Export a WhatsApp voice-note transcript to SRT.** Share a WhatsApp voice
   note into Suramak, let it transcribe, open the transcript, Export → SRT →
   pick a folder (e.g. Documents). Expect: the system file chooser appears with
   "…​.srt" pre-filled; after saving the sheet says Saved with the file name and
   size; the file opens in a subtitle player and matches the desktop's SRT for
   the same transcript byte-for-byte. Then repeat with **Share instead** and
   send the same file to another app.
2. **Predictive back mid-transition.** Open a transcript and immediately swipe
   back **while the push is still sliding**; repeat on Settings. Expect: the
   gesture pops the current screen at once (no queued second animation, no
   stuck half-screen), the sheet dismisses before navigation when open, and a
   back at the Library root leaves the app. Note whether the system's
   predictive-back preview matches the screen that actually leaves.
3. **Fling feel.** Scroll a long transcript and a long library list; release
   with a flick. Expect a native-feeling decay; read the logged line
   `fling <v> pt/s → <d> pt over <n> frames (egui expectation <e> pt)` (or the
   equivalent snapshot field) and note whether the distance tracks the
   expectation. If it does not, this is the Deck-motion-layer follow-up.
4. **Battery.** Start a long transcription, leave the Library (or the editor)
   on screen without touching it. Expect no visible redraw work and no warmth
   attributable to the UI; the job strip only changes when a new percentage
   arrives.
5. **Rotation / process death during the export sheet.** Rotate while the
   picker is open; the Activity recreates and the staged job is pruned at the
   next start (no orphan file in `files/exports`).

---

## 10. Files touched

**Rust — `suramak-core`**

* `src/export.rs` — `to_tsv` (the shared spreadsheet serializer).
* `tests/export.rs` — TSV golden + whitespace folding.

**Rust — `suramak-ui`**

* `src/deck/nav.rs` — two-layer transitions, `Layer`/`NavLayers`, the Reduce
  Motion crossfade, the outgoing-screen lifetime, layer-routes API.
* `src/deck/compact/mod.rs` — dispatcher paints two layers and the animated
  sheet; `needs_frame`/`wants_repaint`.
* `src/deck/compact/motion.rs` — new: `SheetAnim`, `ListAnim`, `Ghost`,
  `ProgressAnim`.
* `src/deck/compact/a11y.rs` — new: `touch`, focus ring, per-frame audit.
* `src/deck/compact/model.rs` — six formats + MIME/bilingual, export session
  and phases, job view/coalescing, frame requests, fling tracker/probe,
  settings scroll, editor translation flag.
* `src/deck/compact/editor.rs` — the export sheet (chips, bilingual switch,
  both routes, result card), a11y wiring, seek-eased playhead.
* `src/deck/compact/library.rs` — list animation, job strip, fling observation,
  a11y.
* `src/deck/compact/settings.rs` — a11y, the toggle label, one-shot scroll.
* `src/deck/compact/chrome.rs` — labels/rings on back, trailing action, rows,
  setting rows, toggles.
* `src/desktop/app/mod.rs` — `ExportFormat::Tsv` (seventh chip).
* `tests/ui_compact.rs` — export-sheet/picker-result/tier shots, a11y floor,
  Reduce Motion and idle-frame tests.

**Rust — `suramak-android`**

* `src/export_job.rs` — new: `ExportJobs`, `StagedExport`, `render`,
  bounded atomic writer, FNV-1a 64.
* `src/compact_host.rs` — export/export_share/export_done/export_shared/
  export_cancel/export_failed ops, translation probe, back cancels a staged job.
* `src/lib.rs` — module.
* `tests` (in-module) — staging, bilingual, mismatch, single-flight, host parity.

**Kotlin / build / CI / tests**

* `java/app/suramak/ui/ExportWriter.kt` — new: picker contract, bounded copy +
  checksum, share intent.
* `java/app/suramak/ui/UiBridge.kt` — export ops.
* `java/app/suramak/spike/MainActivity.kt` — export launcher, write/verify,
  `exportFromCompact`, `shareFromCompact`.
* `crates/suramak-cli/src/cmd/transcribe.rs` — the batch loop drains before the
  idle check (the gate-found race above); `cmd/mod.rs` gains `Tsv`.
* `app/build.gradle.kts` — versionCode 10 / `0.4.0-m13g2.1`.
* `scripts/android/build-apk.sh` — version assertions, `ExportWriter` dex
  assertion, export-intent source assertions.
* `tests/ui-compact/{run.sh,README.md,shots/*.png}` — seven shots; the step is
  now a CI gate.
* `.github/workflows/ci.yml` — the named `UI compact (phone viewport, Xvfb)`
  step (the only workflow edit).
* `docs/PLAN-M13-ANDROID.md` — G2 execution note.
* `Cargo.lock` — **unchanged**: no new package, no new dependency anywhere.
