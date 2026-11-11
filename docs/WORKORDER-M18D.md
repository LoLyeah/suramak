# WORKORDER — M18.4: the Android Quick Settings tile

Plan source: `docs/PLAN-M15-M18.md` §M18.4 (S-size). Reuse the shipped
foreground recording state and notification controls — **no second capture
path, no second recorder lifecycle.**

## Deliverables

1. **A QS tile (`TileService`)** that starts and stops the existing foreground
   recording (the same `ACTION_*` intents the notification's controls use).
   State mirrors the FGS: unavailable when recording is impossible (no mic
   permission), active when recording, inactive with "tap to record" otherwise.
2. **Contract edges (the plan's verify list):**
   - locked device: `startActivityAndCollapse` rules honored; a tap on the
     locked device either records (if the app already holds the permission and
     no unlock is needed) or shows the honest "unlock to record" — never a
     silent no-op;
   - permission denial: the tile sends the user to the app's permission
     surface, never crashes;
   - duplicate starts: two taps in one second must not start two takes (the
     existing state machine's guard is the source of truth — assert it);
   - Android 14+ background-start restrictions: the tile uses the FGS start
     path the notification already proves.
3. **Tests:** unit tests for the tile's state mapping (idle/recording/denied),
   plus a host test that the tile's intents are exactly the notification's
   (one constant table — no divergent strings). The emulator smoke gains a
   step: `adb shell cmd statusbar` / `cmd qs` tile toggle if the API allows
   headless toggling; if not, assert the tile service's `onStartListening`
   state mapping via logcat and document the boundary in the report.
4. **Manifest:** the tile service entry + its icon asset; no new permissions.

## Verification

- fmt/clippy -D warnings; workspace tests green (one full run at the end);
  the Android emulator smoke still green (it is the gate for the whole
  Android chain).
- Report: `docs/reports/M18d-qs-tile.md` — the state map, the locked-device
  behavior (what the emulator could/couldn't prove), the duplicate-start
  guard, and the boundary (OEM tile quirks stay the owner-device gate).

## Do NOT

- No new recording implementation, no new permissions, no notification
  redesign; no engine changes; no CI edits beyond the smoke's tile step;
  no git commit/push; no sudo.
