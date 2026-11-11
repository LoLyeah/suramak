# WORKORDER — M19B: the model registry tells the truth (and picks the phone's default)

Plan: `docs/reports/M19-android-audit-plan.md` slice WO-2. Owner-reported: the
Settings screen shows `turbo-q5-0` as **"In use"** on a phone that never
downloaded it; the tier table says `small` is the default at 8 GB+, and the
row itself says "default (macOS)".

## VERIFIED starting facts

- `settings.rs` marks a row `selected = row.id == settings.model_id`
  (→ "In use"); `row.installed` only chooses "Use"/"Get" for other rows.
- `Settings::default()` sets `asr.model_id = "turbo-q5-0"` (macOS default);
  the Android host loads `Settings::load(..).unwrap_or_default()` and never
  applies the device tier it already knows (`suramak-core::models`: `base`
  4–6 GB, `small` 8 GB+).
- M13D built download/verify/resume in `suramak-core::models` (rustls,
  `.part` + dataSync resume). Reuse it; do not write a second downloader.

## Deliverables

1. **Tier default at first run (Android only):** when no settings file exists,
   the Android host selects the phone default for the device's RAM
   (`small` at 8 GB+; `base` at 4–6 GB; below the floor the existing
   "below the 4 GB floor" steer). Desktop defaults are unchanged.
2. **Migration for existing installs:** add `model_explicitly_chosen: bool`
   (serde default `false`; old files read as false). On Android load:
   if `!model_explicitly_chosen` AND `model_id == "turbo-q5-0"` AND that model
   is NOT installed/verified → select the highest-tier *installed* model if
   any, else the device's tier default, and persist. A user-chosen model is
   never overridden (set the flag on every explicit "Use" tap).
3. **The truth state machine per row:** evaluate against disk verification
   (file present, expected size, no stale `.part`) — not path existence:
   - `NotDownloaded` → action `Get`/`Download`; when the row IS the configured
     model the card must NOT read "In use": show `· Not downloaded` and a
     primary `Download` action;
   - `Downloading` → progress + `Cancel` (the M13D job state);
   - `Stale/Corrupt` → `Re-download` (and delete/unlink the bad file);
   - `Installed` (verified, not selected) → `Use`;
   - `InUse` (verified AND selected) → the only state allowed to show the
     accent border + "In use" pill.
4. **Honest header + copy:** the TRANSCRIPTION → Model row shows the truth:
   `small · not downloaded` (warning tint) when nothing usable is selected/
   present, `small` when in use. The catalog note `default (macOS)` becomes
   `desktop default` and is omitted on Android rows (or phrased
   `desktop default` — no platform-confusing copy on the phone).
5. **The Library's model card** (the first-run download card) uses the same
   tier default and the same honesty ("Needs 488 MB · 5.0 GB free"), and the
   Record button is disabled with an inline reason when no verified model
   exists (today's behaviour may allow a record with no model — verify and
   fix if so).

## Verification

- Tests: settings default per tier (4/6/8/12/16 GB), migration matrix
  (old file with turbo, with a real choice, with an installed model),
  the five row states, "In use" impossible without a verified file, the
  desktop defaults unchanged.
- The emulator smoke: after install (no model files), the Settings screencap
  must NOT contain "In use", and the header row must carry "not downloaded"
  (uiautomator can't read egui pixels; assert via the shell's own log marker
  for the row state — add one line per state change).
- fmt/clippy -D warnings; workspace tests green (one full run at the end).
- Report: `docs/reports/M19b-model-truth.md`.

## Do NOT

- No downloader rewrite (M13D's path stays); no changes to desktop defaults;
  no automatic model downloads; no engine changes; no new deps; no CI edits
  beyond the smoke's assertion line; no git commit/push; no sudo.
