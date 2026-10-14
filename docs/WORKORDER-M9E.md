# WORKORDER — M9e: the pre-release slice (quit confirm · job cancel · min-size layout)

Tree: `1af3ef3` (clean). This slice answers exactly three findings from `docs/reports/M9-CRITIQUE-B.md` — §6 P1-1, P2-2, P2-3 — nothing else. Read first: that report's §6 (the exact findings), `docs/reports/HOTFIX-QUIT.md` (**protect-list invariants for quit** — every one must survive), `docs/reports/M9c.md` §1.5/§4 (the control + bounds patterns), `.impeccable/motion-brief.md` §3 (confirm motion), and `docs/USER_SMOKE.md` items 19/25/26 (behaviors the owner tests).

## Items

### 1) Quit mid-recording — the confirm (P1-1)
With a **live capture session**, BOTH quit paths must present a small in-app confirm before anything is discarded — title "Still recording", three actions: **Stop & Save** (recommended) · **Discard Recording** · **Keep Recording** (cancel = stay).
- **Window close**: `CloseRequested` with a live session = ignore the event, show the confirm.
- **Cmd-Q / menu Quit**: terminates via AppKit (HOTFIX-QUIT: `exiting` → bounded teardown → `libc::_exit`). Intercept **before** teardown: the AppKit veto (`applicationShouldTerminate` → `NSTerminateCancel` + show the confirm; reply/terminate when answered) via objc2 — the objc goo may live in `suramak-glass` behind a small API. **Keep every HOTFIX-QUIT invariant**: no ggml teardown abort, bounded final exit after the answer, single-instance guard untouched.
- **Stop & Save semantics (bounded!)**: stop the capture session, **preserve the recording's WAV** (it streams to a temp file today — keep it instead of deleting), persist a small **pending-transcription entry** in the data dir, then exit cleanly. **On next launch the pending entry auto-runs its transcription** (status visible). If auto-resume exceeds the budget, the sanctioned fallback: keep the WAV + a Home status "recording saved — open Suramak to transcribe it" + an action to start it. Document which you built.
- **Discard** = today's behavior exactly. **Keep Recording** = abort the quit completely (still recording).
- UI: painted confirm card (capsule controls per the system), fade per the motion brief (snap under Reduce Motion); no OS dialog. A11y labels; **Esc = Keep Recording** (cancel the quit).

### 2) In-flight transcription — Cancel (P2-2)
- A quiet tertiary **Cancel** on the transcribe card, wired to a **cooperative cancel token** in the runtime (checked between decode stages/chunks; the worker exits cleanly between stages; partial results discarded; the engine is never corrupted).
- Cancel → Home with a status `transcription cancelled` (the shared status fade). Esc on the transcribe screen = the same cancel path (documented). Paint the control per the system; a11y label.
- Unchanged: the pill stays disabled during jobs; quitting during a job keeps today's documented clean-exit behavior (not this slice).

### 3) Min-size layout — 900×600 (P2-3)
- **Settings**: add a `ScrollArea` (the pattern exists in four other views) — Save/Reset reachable at 900×600.
- **Models**: `ScrollArea` + columns **flow** below ~1020 px (wrap the action cluster to the row's second line, or shrink/hide ENCODER first — your call) — no fixed-offset collisions; the named defect is the `ACTIONS`/`INSTALLED` header mash + per-row label overlap, both must be gone.
- Captions wrap rather than clip mid-word (within reason).
- **Acceptance shots at 900×600** for both screens (adapt the reviewer's min-size scripts under `build/m9d-e2e/extra/`): Save visible and clickable (click → applied pulse/status), header labels never overlap, no mid-word clipping. Re-verify 1280×800 is unchanged (full pixel suites green).

## Do NOT
- Restyle anything beyond these items; touch asr/capture/live pipeline internals (item 2's cancel is runtime/worker-level and cooperative — the audio path is untouched), playback internals, glass runtime beyond the quit-veto hook, taps, instance guard; add crates or fonts; `git commit`/`push`; modify files outside `/home/ubuntu/suramak`.

## Verification (all gates; evidence in the report)
- fmt/clippy `-D warnings`/workspace tests green, with new tests: quit-confirm state machine (both paths; stop-and-save / discard / keep outcomes); pending-transcription persistence + auto-resume (or the documented fallback); job-cancel token + clean worker exit; min-size geometry (settings scrollable; models columns non-overlapping below 1020).
- xvfb: existing smokes green; a scripted **quit-while-recording** smoke (start a record smoke, send close, answer Stop & Save, assert the pending entry + bounded exit wall time); the existing bounded mid-job quit smoke must stay green; 900×600 screenshots for Settings/Models; both pixel suites still green at 1280×800; `cargo check -p suramak-glass --target aarch64-apple-darwin` clean.
- Report `docs/reports/M9e.md`: item-by-item (what changed, evidence paths, measured numbers); which stop-and-save flavor you built; deviations with whys; smoke items to tell the owner (add the new checks to `docs/USER_SMOKE.md` as item 27).

## After this slice
Everything is release-ready; the cut (`v0.1.1`) waits only on the owner's macOS acceptance test (smoke items 19–27).
