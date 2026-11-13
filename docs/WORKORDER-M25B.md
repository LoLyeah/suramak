# WORKORDER — M25B: long sessions, honestly capped, and split at pauses

From the gate: `docs/reports/M25a-long-recording-gate.md`. Single-pass tiny is
safe to **45 min on a 4 GB phone** and **60 min here**; 90 minutes needs
1,135 MB and dies on a phone. Chunked decoding is NOT quality-safe as measured.
So the product does the honest thing: **cap the session, and for anything
longer, split at real pauses instead of mid-word.**

## Deliverables

1. **The cap, named and enforced**: a constant per platform
   (`SESSION_CAP_MS`: 45 min Android, 60 min desktop; one function, both hosts
   read it) checked **before** a job starts, on recorded sessions AND imports.
   Over the cap the queue refuses with a named reason (`session_too_long`) that
   carries the measured budget, not a generic error.
2. **Recording rotates at silence, never mid-phrase**: when the current part
   reaches the cap, the recorder keeps writing until the next silence of
   ≥ 600 ms (grace ≤ 120 s, then it splits anyway and says so). It closes the
   WAV, opens `Part N+1`, and the split is **sample-exact**: concatenating the
   parts equals the uninterrupted capture (a test asserts this on a synthetic
   signal). Parts are queued as normal jobs and the library shows
   `Part 2 of 3` beside the title.
3. **Imports over the cap split the same way**: the file is cut at silences
   (same ≥ 600 ms rule) into parts under the cap, each queued; the row says
   how many parts and why. If no silence is found in a window, the cut happens
   at the nearest low-energy point and the row says `split at 45:00 (no pause
   found)` — honest, never silent.
4. **Diarization's own limit is surfaced**: it refuses above 30 min. A part
   longer than that shows the existing named refusal on the Speakers action,
   with "split into shorter parts to label speakers" — no new behaviour, just
   the honest route.
5. **The recording UI**: elapsed time plus a quiet line `Part 1 · 44:12 of 45:00`
   once past 80% of the cap; at the rotation the counter resets with a 1-line
   note. Nothing modal, nothing that interrupts.

## Verification

- Tests: the cap function per platform; the refusal reason; rotation at a
  synthetic silence with sample-exact concatenation; grace-split when no
  silence arrives; import splitting (2 and 3 parts) with part metadata;
  a file exactly at the cap is accepted; 1 ms over is refused.
- A host test that runs the splitter over a 50-minute fixture (synthesised,
  deleted after) and asserts every part is under the cap and the total sample
  count equals the input.
- fmt/clippy -D warnings; NDK check + clippy on both Android crates; one full
  workspace run (baseline 1434).
- Report: `docs/reports/M25b-long-sessions.md` with the real part lengths and
  the measured peak RSS of a 50-minute split run (should stay under the cap's
  722 MB expectation).

## Do NOT

- No chunked decoding of a single file (the gate's STOP stands); no streaming
  engine; no new crates; no changes to the queue journal/resume; no diarization
  work beyond surfacing its existing refusal; no CI edits; no git commit/push;
  no sudo.
