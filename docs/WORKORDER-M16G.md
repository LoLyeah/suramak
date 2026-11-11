# WORKORDER — M16.5-F: Linux monitor-source + Windows WASAPI loopback — FEASIBILITY (no production code)

Plan source: `docs/PLAN-M15-M18.md` §4 M16.5: "Linux monitor-source selection
and Windows WASAPI loopback as separate workorders after their feasibility
checks." This is the feasibility check both platforms share (the macOS
`suramak-taps` path already ships with regression coverage). Tree: dispatches
after M16F/M14X land (workflow + packaging files settle first).

## Deliverables

1. **Linux (PulseAudio/PipeWire)**: can `cpal` (the pinned version in the
   workspace) open a monitor source (`*.monitor` on PulseAudio; the PipeWire
   equivalent) as a capture device on THIS VPS without added dependencies?
   The concrete answer on the machine: list what the API surface exposes
   (`host.devices()`), try a real open+read of a monitor source, and measure
   the capture's real-time factor for 30 s. Record the concrete device-name
   strategy (`pulse` host? `pipewire` host?) and what the Settings picker
   would list.
2. **Windows WASAPI loopback**: the decisive facts for the M14-era toolchain:
   - Does the workspace's `cpal` build expose WASAPI loopback (`wasapi` crate
     loopback mode, or cpal's built-in)? Inspect the pinned cpal source and
     answer from code, not docs (cite lines).
   - If cpal cannot, what is the smallest addition (a `windows` crate WASAPI
     wrapper behind the existing cfg) — measure the dependency weight and
     license.
   - The session-audio precedent already in the plan (M16 "system-audio
     loopback ... Windows WASAPI" is accepted-adapted) — the deliverable is
     the measured path, not the promise.
3. **The Suramak-side contract**: what the existing M6 multi-source mixer
   needs from a loopback source (16 kHz mono PCM16, per-source gain, drift
   correction) — confirm both platforms can meet it (rate conversion exists
   in the capture path).
4. **Decision block**: per platform GO (with the production workorder's slice
   list) or STOP/DEFER (with the re-visit condition). The desktop capture
   surface on Linux/Windows is exactly the owner's Windows PC — include what
   the Windows pass would look like for them.

## Verification

- The Linux probe runs on THIS VPS (real reads, real numbers).
- The Windows answer is source-inspected (the pinned cpal's windows backend;
  cite lines); a real Windows WASAPI probe happens on the owner's machine at
  their next Windows pass (named in the report — no emulator here).
- fmt/clippy -D warnings; workspace tests green (baseline untouched); the
  probe's throwaway crates stay gitignored under build/.
- Report: `docs/reports/M16g-loopback-feasibility.md` (decisions first,
  numbers machine-named).

## Do NOT

- No production capture-path changes; no new product dependencies; no CI
  edits; no Android work; no UI; no git commit/push; no sudo.
