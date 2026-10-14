# M9e — the pre-release slice: quit confirm · job cancel · min-size layout

Answers exactly §6 P1-1 / P2-2 / P2-3 of `docs/reports/M9-CRITIQUE-B.md`. Nothing else
moved: no restyle, no audio-path change, no new crates.

Executor: delegated agent (deepseek-v4.1-flash via opencode-go, 250 api calls,
4,675 s) — it landed the substance and most fixes, then hit its iteration cap with four
items open (one failing test, a leftover probe module, this report, USER_SMOKE item 27).
The orchestrator closed the tail itself (the caption wrap fix, the full gate re-run,
the report). Everything below was re-verified on the final tree unless marked
"owner-verify".

## 1. Quit mid-recording — the confirm (P1-1)

Both quit paths now funnel through one quit handler:

- **Window close** (`CloseRequested`): with a live session the event is vetoed and the
  confirm appears; without a session: exit as before. Verified end-to-end under Xvfb
  with a real `WM_DELETE_WINDOW` sender (`build/m9e-e2e/send-close.c` — `xdotool
  windowclose` destroys the X drawable instead of delivering the protocol event, which
  is why the first probe failed; the replacement sends the real message).
- **Cmd-Q / menu Quit (macOS)**: an `applicationShouldTerminate` veto on the app
  delegate — the class method is installed via `class_addMethod` behind a small
  `GlassHandle::set_terminate_veto` API (`crates/suramak-glass`); on answer the quit
  proceeds through the existing bounded teardown. Compile-checked for
  `aarch64-apple-darwin`; **no macOS runtime on this VPS — owner-verify (smoke 27)**.

The card: “Still recording” + three answers — **Stop & Save** / **Discard Recording** /
**Keep Recording**; `Esc` = Keep. Painted card per the control system; modal guard (a
second close or a background click cannot get behind it).

**Stop & Save is bounded.** In order: stop the session; **preserve the recording**
(`<data>/suramak/recordings/<title>.wav`); write `pending-transcription.txt` (title +
wav path, escaped, malformed-rejected); exit through the existing hard-exit. **Next
launch reads the entry, clears it, and resumes** — with real audio it queues the
transcription; on a capture-free build it reports “the saved recording’s audio is
missing” honestly. Measured (orchestrator re-run, `quit-smoke.sh`): close → confirm
latency 531 ms; answering Stop & Save → process exit **225 ms**. Every HOTFIX-QUIT
invariant kept: no ggml teardown abort, bounded exit, `libc::_exit` after the answer,
single-instance guard untouched.

## 2. In-flight transcription — Cancel (P2-2)

`suramak-asr`: a cooperative `CancelToken` threaded through the worker — checked at
stage boundaries and via whisper.cpp’s abort callback (no mid-decode memory corruption;
the worker exits between stages; partial results discarded) → a `Cancelled` event. UI: a
quiet tertiary **Cancel** on the transcribe card; `Esc` runs the same path; cancel
returns Home with `transcription cancelled` (the shared status fade). The pill stays
disabled during jobs; quitting during a job keeps the documented clean-exit behavior
(unchanged — that half-done job is re-runnable).

## 3. Min-size 900×600 (P2-3)

- **Settings**: the form scrolls (`ScrollArea`, `id_salt("settings-form")`); the
  Save/Reset action row is the form’s tail; a bottom reserve clears the floating pill.
  `find-save.py` (pixel search) locates Save at (261,473) at 900×600 and clicks it; the
  fresh capture shows Save + Reset fully visible, clear of the pill.
- **Models**: `ScrollArea` + **flowing columns below ~1020 px** — the action cluster
  wraps to the row’s own line, so the fixed-offset collision is structurally gone. Fresh
  capture: `MODEL / SIZE / INSTALLED / ENCODER` well-spaced, no `ACTIONSINSTALLED` mash,
  no label stacking.
- **Captions wrap instead of clipping.** The leftover failing test: the appearance
  caption wrapped at 567.8 pt — the scroll-content max-rect is looser than the pane —
  and ran to x=948, past the pane edge (874). Fix: `row_caption` wraps against the
  **visible clip** (`clip_rect`, floored) — now 491.8 pt wide, right edge 872.0 ≤ 875 at
  the minimum; central for every caption row. (Orchestrator’s fix; the test is green.)
- **Scroll-offset isolation**: Settings and Models are both children of the same central
  panel; without distinct ids egui handed them the same id and the offset leaked across
  views — explicit salts + a regression test.

## Verification (re-run by the orchestrator on the final tree)

- `cargo fmt --check` clean; `clippy --workspace --all-targets -- -D warnings` clean;
  `cargo check -p suramak-glass --target aarch64-apple-darwin` clean.
- **Tests: 373 passed / 0 failed / 7 ignored** (M9e added 27 since `e299d00`’s 346 —
  the quit funnel and its answer state machine, the pending-transcription entry, the
  cancel token, the scroll isolation, the caption/geometry checks — the new state
  machines each carry their own tests).
- **Pixel suites, fresh seeds + fresh captures on this tree: m9d 25/25 PASS, m9c 18/18
  PASS** at 1280×800.
- **quit-smoke: PASS** (`build/m9e-e2e/quit-smoke.sh`): close intercepted; Stop & Save
  exit=0, wall 225 ms; the entry written → read → cleared on relaunch; Keep + Esc-keep
  verified; Discard clean, no entry; `SURAMAK_SMOKE_QUIT_ANSWER=stop-save` answers the
  confirm deterministically (headless test hook).
- Min-size acceptance (fresh, fixed build): `min-settings-top/bottom/saved.png`,
  `min-models.png`, `settings-wide.png`, `editor-wide.png` — caption wraps inside the
  pane; Save reachable and clickable; all headers clean.

## Deviations & boundaries

- The first quit probe failed for two harness reasons (`xdotool windowclose` destroys
  the X drawable — no event; and a stale `target/debug/suramak` was in play). Fixed with
  the real `WM_DELETE_WINDOW` sender + rebuild; noted here because it changed the
  harness, not the app.
- The macOS AppKit veto is compile-checked only (no macOS runtime here); smoke 27
  covers it on the owner’s Mac.
- Stop & Save on this capture-free Linux build cannot exercise the audio-present resume
  end-to-end (there is no real capture); the audio-present branch is unit-tested, the
  real branch is owner-verify.
- The floating pill still paints over mid-scroll content at 900×600 — the tail reserves
  ensure the actions and the form/table ends clear it, and any occluded line is revealed
  by scrolling; inherent to a floating control, same as on the record screens.
- `suramak_playback::format_mmss` remains (uncalled) — same rationale as M9c
  (protecting the playback crate from an unrelated touch).
- The executor’s cap: no functional work was left partially applied — the failing test
  is green and the leftover probe module is gone (clippy’s dead-code gate would have
  caught it; it is clean).

## After this slice

Everything is release-ready; the **v0.1.1** cut (zip + dmg + Linux tarball) waits only
on the owner’s macOS acceptance pass (smoke items 19–27).
