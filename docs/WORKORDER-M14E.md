# WORKORDER — M14E: Settings & shell polish (fast-engine UX + the five findings)

Tree: current `main`. **Read first:** `docs/USER_SMOKE-WINDOWS.md` §6 (findings 1–5), `docs/USER_SMOKE.md` "Report (2026-10-21)" finding 1 (dropdown backdrop), `crates/suramak-ui/src/app/settings.rs` (fast engine row + copy seam), `crates/suramak-ui/src/app/settings_form.rs`, the pill in `crates/suramak-ui/src/shell.rs`, and `crates/suramak-core/src/models.rs` (MODELS catalog + `parakeet_dir` + the CLI's parakeet install path — reuse it, never duplicate it), `crates/suramak-cli/src/cmd/models.rs`.

## Goal

The owner's two UX reports — (a) "the drop down not blurring the bg so it cluttered", (b) *choosing "experimental" needs better UX: the toggle's copy literally tells a GUI user to run a CLI command* — plus the five logged shell findings, in one polish slice. This is UX discipline, not new plumbing: everything needed already exists in `suramak-core` (download + progress + verify) and the worker job cards.

## Part 1 — the fast-engine setup flow (the main dish)

Replace the one checkbox + CLI hint with a **readiness list** the user can't misread:

1. **Row 1 stays**: "Fast engine (experimental)" + the ON/OFF toggle (default OFF), behavior unchanged.
2. **Under it, one line per fast engine**, each with a live status and the action **inline in Settings**:
   - **Apple SpeechTranscriber (macOS 26+)**
     - `Available` (green dot) when the `speech` crate reports the OS-managed assets are usable;
     - `Needs permission` → an inline **"Allow speech recognition…"** button that opens `x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition` (the exact System Settings pane) — never a dead end;
     - `Not in this build` (grey) when the cargo feature is off, `Needs macOS 26` (grey) below 26.
     - One quiet caption: "managed by macOS — nothing to download; the OS keeps its models updated."
   - **Parakeet (NVIDIA Parakeet TDT 0.6B v3)**
     - `Not downloaded … [Download]` — the button runs the **same install path the CLI uses** (`suramak-core`'s parakeet installer; sha256 pins, resumable) **through the existing job-card UI**: progress events + cancel already exist for model downloads — reuse that machinery, show ~670 MB + per-file progress, cancellable;
     - `Installed · 670 MB · [Delete]` when present;
     - `Not in this build` (grey) without the feature.
     - A coverage caption: "25 languages — English, Spanish, French… (hover for the list). Indonesian is always Whisper." An expandable line listing coverage is enough; keep it collapsible.
3. **The settings copy loses the CLI instruction** — `suramak-cli models download parakeet` disappears from GUI copy (the CLI keeps working for scriptable installs; the copy seam check keeps the sentence out of the Settings build).
4. **Coverage guardrails stay untouched**: Indonesian never routes fast — this is UI polish over the *existing* routing, not a routing change. One new routing-related item though: the **deck/job-card caption** when the language setting is `auto` should say why the fast engine is idle: *"auto stays Whisper — fast engines need a forced covered language"* (a quiet one-liner in the transcribing card; the degrade warning already covers toggle-on-missing-model).

## Part 2 — the five shell findings (from docs/USER_SMOKE-WINDOWS.md §6 + USER_SMOKE.md finding 1)

1. **Dropdown backdrop**: the model dropdown sheet needs a real backdrop — a solid panel (#fff/#1c1c1e family tint) + a drop shadow at minimum; blur is better on macOS (the same `suramak-glass` material family). The list must read cleanly above its content.
2. **Theme copy seam**: "System follows the macOS light/dark setting live" must say *Windows* on Windows (and Linux's honest "follows the desktop theme" where applicable) — a small copy-resolution function, not string duplication everywhere.
3. **Pill overlap on Settings (Windows)**: the pill covered the "Automatic updates" checkbox. Fix = the scroll region reserves bottom clearance for the pill's actual rect (the same bottom-inset the Deck already reserves on Home) — verify at 900×600 and large windows on the Windows layout, keep macOS untouched (the Mac scrolls correctly).
4. **"← Home" affordance**: promote the editor's top-left link from text to a real (still quiet) control — hover/press states, FG/click target ≥ 24 px; all platforms.
5. **Progress bar interpolation**: between Progress events, animate the bar from (a) elapsed wall time vs audio duration (RTF≈ from settings/model), clamped below 97% until a real event confirms more; keep honest labels ("~estimating") when no size is known. Long files keep the true ladder; short jobs get *motion* instead of a flat 0. Respect the heat budget (only repaint while the bar visibly moves).

## Do NOT

- No routing changes, no new dependencies except none needed (parakeet download uses existing core paths), no CI/workflow edits, no Gradle changes, no settings-schema breaks (settings.toml keeps its keys; new UI state is derived, not persisted — unless a new key is clearly needed, then versioned per docs).
- No git commit/push; no sudo; only files inside `/home/ubuntu/suramak`.

## Verification

- `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` (532 baseline + your new tests).
- **Feature gates**: run `cargo clippy -p suramak-asr --features parakeet --all-targets` and the ui build features on the relevant combo (`speech-transcriber`) — a pass-2 lesson: feature-gated breaks are invisible to default builds. If these features need the network locally, check the pins and cache carefully and graduate to the CI check instead, saying so honestly.
- Windows-target check for the ui probe pattern (`docs/reports/M14b.md` §2.2) if touching windows-only cfg blocks.
- UI tests: e2e pixel tests must still pass (they screenshot Settings/the deck at fixed sizes — pixel diffs WILL show from your changes; update the constants per the existing §20/§25 pattern, explaining each value in the report).
- Report `docs/reports/M14e-polish.md`: per finding (the five + the two UX parts), the change, the test, the honest boundary (runtime-verified on device only in the owner's next pass; screenshots welcome).

## Order of landing (commit-shaped)

1. Dropdown backdrop + theme copy (small, contained).
2. The fast-engine readiness list + Parakeet-in-Models download (the meat).
3. Pill clearance + Home control + progress interpolation (layout-sensitive).
