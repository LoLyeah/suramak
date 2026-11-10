# M16B — macOS global hotkey + floating HUD (M16.2 + M16.3-macOS)

Executed per `docs/WORKORDER-M16B.md` (PLAN-M15-M18 §4 M16.2 + M16.3-macOS,
macOS-first per the owner's 2026-10-19 order). **Orchestrator-authored
report**: the child hit its turn budget before the final full workspace run and
this file; the code below was verified by the orchestrator.

## Feasibility facts (verified; cited)

- **Carbon `RegisterEventHotKey`** is the one macOS global-hotkey API that needs
  NO TCC permission and intercepts the chord (other apps never see it). It
  requires the OS main thread with a running event loop — the winit loop is
  exactly that. Media-key chords (CGEventTap + Input Monitoring) are out of
  scope by design.
- **Insertion** is the only permission-bearer: synthetic keyboard input
  (CGEvent) requires Accessibility; the adapter degrades, never fails —
  Copy is primary until Accessibility is granted, and the grant is surfaced
  with a deep-link button (System Settings › Privacy › Accessibility). We post
  a paste keystroke of the clipboard content WE OWN; the UI copy discloses the
  mechanism (the plan forbids privileged uinput and universal clipboard
  inspection).

## Landed

1. **`crates/suramak-taps/src/hotkey.rs`** — hand-written Carbon FFI (no new
   dependency): register/unregister/set-chord, callback marshalled onto the
   event loop via the existing repaint wake (the M12 `EventLoopProxy` path —
   never a synchronous dispatch while the egui lock is held). Compiles clean
   under `cargo clippy -p suramak-taps --target aarch64-apple-darwin
   --all-targets -D warnings`.
2. **The floating HUD** (`crates/suramak-ui`): borderless, always-on-top,
   non-activating panel semantics with the six-state machine —
   **idle → recording → processing → result → cancelled → permission-failure**
   — driven through the M16.1 queue conventions. Recording starts from the HUD
   (the existing M6 capture path + `suramak-taps` per-app options); closing it
   mid-recording stops the capture cleanly (no leaked capture); the
   always-visible indicator discipline (M9) holds; idle renders no repaint
   churn. Result rows offer Insert / Copy / Keep / Dismiss.
3. **Insertion adapter**: CGEvent paste when Accessibility is granted, else the
   honest Copy-first path with the disclosure text; deep-link button opens
   System Settings.
4. **Settings**: the Compact HUD section — enable/disable, chord choice
   (system-collision collisions detected at registration and named),
   insertion preference.
5. **Host tests**: the state machine + serde + settings plumbing run on Linux —
   all six HUD states were additionally run under **Xvfb with screenshots
   inspected** after fixing card transparency/height, shadow bounds, and
   disclosure clipping.

## Verification (orchestrator)

- `cargo fmt --all --check` clean; `cargo clippy --workspace --all-targets
  -- -D warnings` clean.
- **Full workspace: 938 passed / 0 failed / 10 ignored** (single orchestrator
  run covering both this and the parallel M13D child).
- Crates as landed: suramak-taps + suramak-ui test suites green pre-merge; the
  macOS-target clippy pass proves the Carbon/AppKit FFI compiles for a real
  Apple build (no linking on this host).

## Honest boundaries (owner checklist for the next Mac pass)

1. Press the configured chord while another app is focused → the HUD opens;
   record → Stop → the result appears in the HUD.
2. Tap **Insert** with Accessibility granted vs revoked: revoked shows Copy as
   primary + the deep-link hint; granted pastes.
3. Change the chord in Settings → the old chord stops working, the new one
   works, no duplicate registrations (the state machine's unregister path).
4. Chord conflict: registering the same chord twice names the conflict.
5. Switch apps mid-recording: the HUD stays on top; closing it stops the mic
   (never hidden recording).
6. Confirm the HUD is operable by keyboard alone (M9b floor).

**Not verified here and marked as such**: true NSPanel runtime behavior
(non-activating, focus layering) cannot be exercised on this Linux host — the
child compiled the panel bootstrap via a throwaway `build/probe-hud` crate
targeting aarch64-apple-darwin (clean), and the Xvfb states verify layout
logic; the real macOS runtime is this slice's one owner-only verdict.
No appearance changes were made to desktop windows (suramak-ui's other
surfaces); `hud.toml` was intentionally not added (the parallel child owns the
core/cli file space) — settings already persist the HUD section, so the file
is only needed if the owner asks for a human-editable config.
