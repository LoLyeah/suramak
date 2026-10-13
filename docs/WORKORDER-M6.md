# WORKORDER-M6 — Capture 2.0 (multi-source + per-app)

Base: master after M5. Order: … M5 → **M6** → M7 → M9 → M10.
Owner rule (PRODUCT.md): **the checkbox UI ships together with the mixing
engine — never a single-choice checkbox list.**

## Goal

1. **Multi-source selection** — the record source picker becomes a **checkbox
   list** so inputs combine: two mics; **mic + system audio** (the
   meeting-transcription combo) — N concurrent inputs mixed into the *single
   16 kHz mono pipeline* the app already has.
2. **Mixing engine** — sum N sources with per-source gain, with **clock-drift
   handling** (device clocks differ; naive mixing drifts into periodic
   artifacts over minutes), producing exactly the one 16 kHz mono i16 WAV the
   rest of the app consumes.
3. **Per-app capture** — record only one app's audio ("transcribe only this
   Zoom call"): the taps side is one parameter away
   (`ProcessSelection::Only(processes)`); add the picker of running audio
   processes + the plumbing.
4. **Per-source levels** — at least gain/mute per source at record time (live
   per-source meters only if cheap; the single combined meter is acceptable
   otherwise).

## Architecture requirements

- **The DSP is pure and Linux-testable.** Put the mixing/resampling/drift code
  in a module inside `suramak-asr` that compiles **everywhere** (not behind
  the `capture` feature) — only device I/O stays macOS-gated. This is what
  lets the VPS verify it.
- **Drift handling**: implement one documented approach (e.g. per-source
  async resampling via the existing `rubato`, or bounded periodic catch-up
  corrections) and explain it in the report; it must never click. Tests must
  simulate a source clocking slightly fast/slow over a long synthetic run and
  assert alignment stays bounded (show the numbers).
- **Regression guard**: with exactly one source selected and unity gain, the
  captured output must match today's single-source behavior (same format,
  length semantics, level policy). The existing solo-device path is the
  baseline.
- **Clipping policy**: document it (do not silently squash); per-source gain
  applies before the sum.
- **UI** (`app/record.rs` + settings): checkbox list of inputs (devices +
  "System audio" + a **"Only an app…"** entry that opens the process picker);
  ≥1 source required (validation); selection persists in settings (follow the
  existing settings patterns); capture-less builds: controls disabled with the
  existing copy; Linux: unchanged (no capture).
- **Per-app**: enumerate running processes with audio via `suramak-taps`
  (add a small listing function there if none exists — that crate is ours;
  keep changes additive and keep its existing tests green);
  `ProcessSelection::Only` wired through the existing capture start path.

## Tests

- Mixer: N-source sum + per-source gains; mute; drift-aligned long synthetic
  run; output format/length invariants; clipping policy behavior. All fast,
  non-ignored, runnable on Linux.
- UI state: multi-select → ordered source list → settings round-trip;
  never-zero-sources validation; per-app selection plumbing; disabled states.
- macOS-only: the real device path — covered by the macOS CI build + owner
  smoke, stated as such in the report.

## Docs

- `docs/USER_SMOKE.md` item 17: (a) record with **mic + system audio** while a
  call plays — both sides appear in the transcript; (b) "Only an app…" —
  pick one app, confirm only its audio lands.
- `README.md`, `CHANGELOG.md` [Unreleased]; `docs/PLAN-NOTES.md` — the drift
  approach + the per-app selection note; `docs/reports/M6.md`.

## Acceptance (from /home/ubuntu/suramak; report exact outputs)

1. `cargo fmt --all --check` — clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` — clean.
3. `cargo test --workspace` — all green + the new tests.
4. `cargo check -p suramak-taps --target aarch64-apple-darwin` — **must pass**.
5. xvfb smoke — ok.
6. The drift test's exact output (the numbers) shown in the report; the mixer
   demo (e.g. a small run mixing two synthetic sources to a WAV) if easy.
7. `cargo tree -p suramak-ui --target x86_64-unknown-linux-gnu` — no new
   crates (`rubato`/`cpal` already present).

## Constraints

- Only modify files inside /home/ubuntu/suramak. No sudo. No git commit/push.
- Do not touch `suramak-glass`, `suramak-playback`, the winit fork,
  `scripts/`, or `crates/suramak-ui/src/instance.rs`.
- `suramak-taps`: additive only; all existing tests stay green.
- No new dependencies. Linux capture stays OFF. Settings additions follow the
  existing patterns (no format breakage beyond an additive settings field).
