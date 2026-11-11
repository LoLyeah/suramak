# WORKORDER — M18.2: the quote card (selected text → PNG)

Plan source: `docs/PLAN-M15-M18.md` §M18.2 (S-size). Deck styling, all local:
**explicit output location, no hidden upload, cancellation, Unicode-safe.**
The audiogram half of M18.2 is a SEPARATE investigation — do not build any
video encoding here.

## Deliverables

1. **Selection → card:** from the editor, select text (one segment's text, or
   a generated excerpt = the segment the playhead is in) → "Share as image"
   produces a PNG quote card: Deck typography (the workbench's tokens), speaker
   label + name when present (the M16I/M18.1 display label), a subtle
   app-name footer, theme-aware (light/dark per the active appearance).
2. **Rendering:** draw with the existing egui/eframe stack and rasterize
   off-screen at a fixed scale (e.g. 1200 px wide, auto height), NOT a
   screenshot of the window — the card must be reproducible and testable on
   the host. Text wrapping must handle Indonesian (no CJK assumptions), long
   words, quotes, and emoji-free text without panicking.
3. **Output:** a save dialog (desktop) defaults to `~/Pictures/Suramak/`;
   nothing is written without the user's confirmation; cancellation leaves no
   file and a cancelled job leaves no spinner stuck. On Android the existing
   export/share intent path carries the PNG (`ACTION_CREATE_DOCUMENT` /
   `ACTION_SEND`), reusing M13G2's writer — no new Android UI.
4. **Tests:** the card renderer is a pure function (text + metadata + theme →
   pixel buffer); golden PNGs for: plain segment, speaker-labelled segment, a
   long-paragraph excerpt, Indonesian text with diacritics, light + dark.
   Byte-stable within a tolerance (allow ±1 per channel for AA differences —
   assert a perceptual bound instead of byte equality across GPUs/backends).
5. **No hidden I/O:** the renderer takes bytes in, pixels out; the caller owns
   the file write. The tests assert no network/temp writes.
6. **A11y:** the action is keyboard-reachable with a label; the share sheet
   result is announced.

## Verification

- fmt/clippy -D warnings; workspace tests green (one full run at the end);
  the new goldens committed under `tests/ui-desktop/shots/quote-*.png`.
- Report: `docs/reports/M18b-quote-card.md` — the renderer's contract, the
  golden list, the output-location policy, the cancellation path, and the
  audiogram gate restated (not started).

## Do NOT

- No video/audio encoding, no audiogram implementation (investigation only if
  the report's last section argues for it — no code); no engine changes; no new
  dependencies (egui's rasterizer is enough); no CI edits; no git commit/push;
  no sudo.
