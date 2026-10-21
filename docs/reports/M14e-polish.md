# Report — M14E: Settings & shell polish (fast-engine UX + the five findings)

Executed by a delegated worker (deepseek-v4.1-flash, `deleg_0a13fc66`); the
worker hit its tool-iteration cap after the final verification edit, so this
report is written by the orchestrator from the worker's verified record, with
the workspace gates re-run on the final tree by the orchestrator (see §5).
No source code was written by the orchestrator.

## 1. Part 1 — the fast-engine setup flow

The problem being fixed (owner, verbatim): the experimental toggle's copy told
GUI users to run `suramak-cli models download parakeet` — a CLI instruction
inside a GUI app — and SpeechTranscriber's only setup knowledge was "needs
macOS 26+" with no path to fix a denial.

- **Row 1 unchanged**: "Fast engine (experimental)" + toggle, OFF default.
- **Copy seam test added**: scans every painted Settings label for CLI-command
  shapes — the `suramak-cli …` instruction can no longer reappear without
  failing a test.
- **Apple SpeechTranscriber row** (status chip + inline action):
  - `Available` — the `speech` crate's availability report says usable;
  - `Needs permission` → inline **"Allow speech recognition…"** button opening
    `x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition`;
  - `Needs macOS 26` / `Not in this build` — grey, honest;
  - caption: "managed by macOS — nothing to download; the OS keeps its models
    updated" (carries the AssetInventory rationale from `docs/ENGINES.md`).
- **Parakeet row**: `Not downloaded · 670.5 MB · [Download]` /
  `Installed · 670.5 MB · [Delete]` / `Partial download` states;
  the download runs **suramak-core's own installer** (its `download_files`, with
  new `*_with_progress` + checked variants — no re-implementation, no new deps)
  on the existing download thread; per-file progress ("file 2 of 4 ·
  encoder.int8.onnx"); a real Cancel (`SuramakError::Cancelled`, partial bytes
  kept for resume); Delete wired through the existing delete-plan machinery
  (now Parakeet-aware).
- **Coverage caption**: the 25-language list, collapsible, read from the
  manifest — with the standing "Indonesian is always Whisper" note.
- **The `auto` caption**: the transcribing card gains the one-liner "auto stays
  Whisper — fast engines need a forced covered language" when the fast engine
  is on — the exact confusion the owner hit (`· auto` rows never tightening).
- Routing untouched: the pinned guardrail (Indonesian never routes fast) still
  has its tests; this slice is UI over the existing ladder.

## 2. Part 2 — the five shell findings (each with its test)

| Finding | Change | Test |
|---|---|---|
| Dropdown sheet backdrop (owner-reported "cluttered") | custom scrim + **opaque sheet** (new sheet/scrim tokens) + card shadow + hover/painted selection marker — the ✓ glyph was absent from the bundled fonts and is now a painted marker (caught in the Xvfb visual pass) | frame test: scrim paints full-window, sheet opaque, no content bleed-through, selection marker present |
| Theme copy seam ("macOS light/dark" on Windows) | one resolver over a `Platform` enum — macOS/Windows/Linux sentences, all three pinned from Linux | copy test ×3 platforms |
| Pill overlaps Settings content (Windows) | when the pill is the **painted (non-glass) fallback**, the scroll viewport now ends above the pill's actual `float_rect`; the macOS 26+ glass path keeps its 76 pt tail reserve | pill-clearance frame test at painted-pill settings |
| "Appearance" heading clipped at scroll top | scroll top now derives from the **measured header cursor** instead of the mac-tuned 96 pt | frame test asserting the heading is fully visible |
| Progress bar dead-flat on short clips | interpolate from clock × input duration (new `probe_duration_ms` in core, header-only) clamped at 97% until a real Progress event wins; "~NN%" / "estimating…" labels; repaint only while visibly moving; plus the §1 `auto` caption | interpolation rule tests + duration probe test |
| (bonus) "← Home" discoverability (#4) | the editor gained a **real quiet `< Home` control** (Tier::Tertiary, hover/press/focus, ≥24 px target, reserved constant slot before the title) — the editor previously had no Home control at all | frame test: hover wash changes only that region; title slot verified |

## 3. Changed layout constants (all deliberate, all explained)

- Settings `scroll_top`: now **measured** (+10 pt) from the header cursor — no
  longer a hard-coded 96 pt.
- Settings `scroll_bottom`: `pill_top − 10` when the pill is the painted
  fallback; the tail reserve ITSELF stays 76 pt only under real macOS 26+ glass
  (14 pt otherwise) — that is the last-edit split the workspace gates re-ran
  against.
- Editor `title_left`: shifted right by the new `NAV_HOME_SLOT = 88 pt`; the
  title's max width shrinks by the same.
- Dropdown sheet geometry: `ITEM_HEIGHT 26 · SHEET_PAD 5 · SHEET_GAP 4`; new
  tokens `ok / warn / sheet / scrim` with measured contrast (§ in frame tests).

## 4. Honest boundary (runtime vs compile)

- Compiled + clippy-checked + unit-tested **here** (Linux, Xvfb headless):
  everything in §1–§2.
- **Owner-lane (real hardware)**: the Parakeet install/delete against real
  670 MB network I/O, the SpeechTranscriber deep link opening the real System
  Settings pane, the macOS 26 glass-tail layout, and the Windows 900×600
  layout with the new clearance. CI's macos/windows jobs prove the compiled
  shell; Xvfb screenshots (checked by vision in `build/m14e/`) proved the
  no-bleed/no-overlap/no-clip invariants in headless rendering.
- On-macOS-only behavior of `speech` remains macOS-CI/owner-only (Linux here
  exercises the off-macOS refusal stub, by design).

## 5. Orchestrator re-verification (final tree, after the last edit)

`cargo fmt --all --check` clean · `cargo clippy --workspace --all-targets`
`-D warnings` clean · `cargo test --workspace` **561 passed / 0 failed / 8
env ignored** (532 baseline + 29 new). Feature gates (run by the worker, and
re-run on the final tree where applicable): `suramak-asr --features parakeet`,
`suramak-ui --features suramak-ui/parakeet`, `suramak-ui --features
suramak-ui/speech-transcriber` — all clean.
