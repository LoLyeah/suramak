# WORKORDER-M8.5 — floating record control (Liquid Glass)

Base: master after M8 (playback) lands. Owner-agreed slot: right after playback,
before M5 (video). The owner asked for this directly: *“is it weird that the
record button is in the top right corner? Why not floating liquid glass like in
iOS and macOS?”* — the titlebar red dot stops being the record affordance; a
floating control takes over.

## Goal

Replace the record affordance with a **floating control anchored bottom-center**
of the window content:
- **idle**: red dot + “Record” label;
- **recording**: elapsed time ticking live (`mm:ss`) + stop affordance, same
  flow as today's stop (final pass → transcript);
- clicks toggle record/stop exactly like the current control (same
  `Effect`s / runtime path — no new recording machinery);
- remove the old titlebar red dot (and any other now-duplicate affordance) —
  the floating control is THE record entry point.

## Design + architecture constraints

1. **Real material on macOS, honest fallback elsewhere** (the M9 material
   ladder principle): on macOS the pill gets a real **`NSGlassEffectView`**
   placed as an additional subview in the existing `suramak-glass` arrangement;
   on Linux/Windows/older macOS it is an egui-painted pill (translucent fill,
   hairline, subtle shadow) — still the same rect, same behavior.
2. **Layering rule (learned the hard way, twice):** the existing container
   stays the `contentView`; never make the glass view the `contentView`. The
   float glass sits **between the base material and the winit view** (ordered
   subview), so the winit/egui layer stays on top: egui draws the crisp
   glyph/label/time and owns all input — **no hit-test juggling**, no extra
   event plumbing. The glass provides the material behind those pixels.
3. **One rect, two consumers:** a pure function
   `float_rect(content_size, pixels_per_point) -> Rect` (bottom-center, ~24 px
   inset, pill ~184×52 at 1x — tune) lives in shared code and is used by both
   the egui painter and the macOS `suramak-glass` API (new
   `set_float_rect(Option<Rect>)` on the glass handle; `None` = hidden). The
   rect is re-applied on resize.
4. Recording state keeps updating while recording (elapsed time): ensure the
   event-loop `busy()` predicate covers “recording” so frames keep pumping
   (same pattern as playback).
5. Non-mac platforms compile and behave identically minus the glass material;
   macOS-specific code stays inside `suramak-glass` (cfg-gated) — the UI crate
   talks to it through the existing cross-platform handle.

## Tests

- `float_rect` geometry (centering, insets, small-window clamping) — pure
  unit tests;
- record state machine: idle → recording → stop wiring produces the same
  effects as the old control (port/extend the existing record-action tests);
- the scheduled repaint-during-recording rule (busy predicate) is covered by
  a state test like the playback one.

## Also

- `docs/USER_SMOKE.md`: item 15 — floating control on macOS: real glass pill
  (blurs the desktop behind), Record starts with live timer, Stop finalizes
  exactly like before; window resize keeps it bottom-centered.
- `README.md` bullet; `CHANGELOG.md` [Unreleased] (Changed: floating record
  control replaces the titlebar dot); `docs/reports/M8.5.md`.
- `docs/PLAN-NOTES.md`: a note on the float-glass layering rule (between
  material and winit view; never contentView) under the existing AppKit
  section.

## Acceptance (from /home/ubuntu/suramak; report exact outputs)

1. `cargo fmt --all --check` — clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` — clean.
3. `cargo test --workspace` — all green + the new tests.
4. `cargo check -p suramak-glass --target aarch64-apple-darwin` — **must
   pass** (suramak-glass is pure Rust + objc2; this cross-check works from
   Linux and is part of CI already).
5. xvfb smoke: `cargo build -p suramak-ui && SURAMAK_SMOKE_SECS=3 xvfb-run -a
   -s "-screen 0 1024x768x24" ./target/debug/suramak` — ok, plus one
   screenshot pass (the repo's E2E habit, see `build/m8-e2e/README.md` for the
   harness pattern) showing the fallback pill rendering bottom-center in the
   Home view; pixel-check the pill’s position with the same ImageMagick
   technique if feasible.
6. `cargo tree -p suramak-ui --target x86_64-unknown-linux-gnu` — unchanged
   deps (no new crates expected; if you need one, justify it).

## Constraints

- Only modify files inside /home/ubuntu/suramak. No sudo. No git commit/push.
- Do not touch `suramak-taps`, the winit fork, `scripts/`, or
  `crates/suramak-ui/src/instance.rs`.
- The recording flow, effects and pipeline are NOT in scope — only where and
  how the control is presented. Existing tests must stay green.
- macOS glass rules: subview of the existing container (never `contentView`),
  apply the float rect on create and on resize, clean no-op when
  `set_float_rect(None)`.
