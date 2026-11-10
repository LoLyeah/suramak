# WORKORDER — M16B: macOS global hotkey + floating HUD (M16.2 + M16.3-macOS)

Plan source: `docs/PLAN-M15-M18.md` §4 M16.2 + M16.3 (both size M; the owner's
approved order puts macOS FIRST). Tree: `main` post-`199468d` (M16.1 queue
landed, 846 tests). The two slices cohere — the hotkey drives the HUD — so one
workorder ships them together on macOS only.

## Feasibility facts (verified 2026-11-10, cite in the report)

- **Carbon `RegisterEventHotKey` is the one macOS global-hotkey API needing NO
  TCC permission**, and it intercepts the chord (other apps never see it).
  Requires the OS main thread with a running event loop — the winit loop is
  exactly that. Media-key chords would need CGEventTap + Input Monitoring — OUT
  OF SCOPE here (regular keys only).
- **Insertion territory**: synthetic input (CGEvent keyboard events) needs the
  Accessibility permission; listen-only observing needs Input Monitoring.
  Permission failure must degrade, never fail: the HUD offers "Copy" +
  honest "Insert" when Accessibility is granted (check + a deep-link button to
  System Settings › Privacy › Accessibility). Do NOT add a universal
  clipboard-spoofing path ("no privileged uinput, no clipboard inspection as a
  universal solution" is the plan's rule — insertion is Affinity-style
  keystroke paste of the clipboard content we OWN, disclosed in the UI cop).

## Deliverables

1. **`suramak-taps` gains Carbon hotkey FFI** (hand-written bindings — no new
   dependency): register/unregister a configurable chord (default ⌘⇧R-style
   configurable in Settings), the Carbon callback marshals onto the event loop
   via the existing repaint wake (the M12 `EventLoopProxy` path — never a
   sync dispatch while the egui lock is held), and RESETS cleanly when the
   hotkey changes or the HUD closes (unregister is tested).
2. **The floating HUD** (`crates/suramak-ui`): a borderless, always-on-top,
   non-activating panel window (NSPanel-level design rendered by the existing
   egui stack — follow how make-app/Sparkle already treat the bundle; the
   window type selected must render while another app is focused). States per
   the plan: **idle → recording → processing → result(your transcript) →
   cancelled → permission-failure**, each with named behavior and motion per
   the Deck tokens (idle = no repainting; recording = the living dot +
   elapsed time; processing = in-progress; result shows the text with
   Insert / Copy / Keep / Dismiss).
3. **Recording inside the HUD** = the M6 capture path (mic + per-app options
   via `suramak-taps`) started FROM the HUD; hotkey-to-start and the HUD's
   Stop button or chord re-press stops it. The HUD lives in a single-window
   state machine; closing it mid-recording stops the recording cleanly (never
   leaks a capture).
4. **Insertion adapters**: on result, Insert (when Accessibility granted)
   synthesizes a paste keystroke of the clipboard content the app just wrote
   (disclosed), else Copy is the primary. Keyboard access: the HUD must be
   reachable and operable by keyboard alone (accessibility floor from M9b).
5. **Settings integration**: a Compact HUD section — enable/disable the HUD,
   the chord (defaults ⌘⇧R or ⌘⇧Space — avoid system collisions; document the
   avoided ones), insertion preference, and the app's own hotkey-to-menu
   bindings the plan calls out. Chord conflicts with the user's own shortcuts
   are detected at registration and say so.
6. **No hidden recording**: at every moment there is a visible indicator (the
   HUD or the pill) whenever the mic is open (the M9 discipline continues).

## Tests

- Host (Linux-cross-compiled where the AppKit FFI compiles: gate the FFI behind
  cfg(macos) so the host tests only cover the state machine + serde + the
  settings plumbing; the sendEvent/carbon parts verify at compile-time only via
  the `cargo clippy -p suramak-taps --target aarch64-apple-darwin` (works for
  pure-Rust; if the crate needs linking, use `cargo ndk check`-style clippy with
  the darwin check path the repo already supports).
- State machine tests: every transition incl. failure/cancel/permission-denied;
  unregister on chord change; no idle repaint assertions (the M9 budget path).
- Bounds: the HUD window never spawns a second single-instance race (the
  existing named-mutex single-instance contract holds).
- fmt/clippy -D warnings; workspace tests (baseline 846; grow; ONE full run at
  the end; crate-scoped tests meanwhile; disk: clean if < 10 GB).
- **Honest boundary**: the hotkey + HUD + Accessibility flow is Mac-only and
  owner-verified next Mac pass (can't be exercised here); the report's §"owner
  checklist" names the exact steps (trigger chord, HUD result, insert attempt
  with/without the permission, quit behavior, chord conflict).

## Do NOT

- No new dependencies; no CI edits; no Android files; no Windows/Linux hotkeys
  (M16.3's other platforms follow separately per the approved order); no
  engine changes; M15 suites untouched. No git commit/push; no sudo.
- Report: `docs/reports/M16b-hotkey-hud-macos.md`.
