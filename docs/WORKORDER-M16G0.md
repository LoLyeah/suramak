# WORKORDER — M16.5-S0L: the PipeWire-desktop gate, closed headlessly

Plan source: `docs/reports/M16g-loopback-feasibility.md` — Linux GO with the
PipeWire-desktop gate as slice 0. The owner said to finish the plan without
waiting on their hardware — so this gate closes on THIS VPS: install a real
PipeWire desktop stack headlessly and run the same monitor-capture probe the
M16G feasibility ran against PulseAudio.

## Deliverables

1. **A headless PipeWire instance** (sudo, documented commands): pipewire +
   pipewire-pulse + wireplumber with a virtual/null sink; a tone playing
   through it (ffmpeg/parec source). All steps recorded in the report.
2. **The probe re-run** (the M16G probe's harness, gitignored under build/):
   the `pulse` PCM + `@DEFAULT_MONITOR@` path against PipeWire: 30 s capture,
   real-time factor, tone RMS sanity, the dual-stream coexistence test
   (mic-shape stream + monitor in one process — the M6 mixer requirement).
3. **The device-name strategy** on PipeWire: what cpal lists, what the
   Settings picker would show, whether `@DEFAULT_MONITOR@` still resolves or
   the picker needs the PipeWire name.
4. **The slice-0 verdict**: Gate PASSED (the production capture workorder's
   slice list stands, with the PipeWire findings folded in) or DEFER (what a
   real desktop adds that headless cannot).

## Verification

- Numbers machine-named (the probe table + the PulseAudio baseline side by
  side); the PulseAudio daemon stopped after (the M16G hygiene); the
  installed packages listed.
- fmt/clippy -D warnings; workspace tests untouched-green; no production
  capture-path changes; no committed new dependencies; no CI edits; no
  git commit/push.
- Report: `docs/reports/M16g0-pipewire-gate.md` — the slice-0 verdict first.
