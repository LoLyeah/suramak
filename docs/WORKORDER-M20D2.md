# WORKORDER — M20D2: Settings polish from the design review

Follow-up to M20D (`fa22513`). A design-director review of the new grouped
Models card found six issues; four are real, two are wording. Small slice —
one file plus goldens. Read the craft floor before editing:
`/home/ubuntu/.hermes/skills/creative/impeccable/reference/craft-floor.md`.

## Fixes

1. **Raw model ids leave the primary card.** `tiny` / `base` / `small` /
   `turbo-q5-0` on the mono second line read as jargon (the owner's complaint
   verbatim). Show them only in Diagnostics (a "model files" line per
   installed model). The smoke's `SURAMAK-MODEL` log lines keep the raw ids —
   that contract is untouched.
2. **The selected row must not read as a nested pill.** Today the in-use row
   is an inset rounded rect with a border inside the card — it breaks the
   group. Use a full-bleed `accent_tint` row fill + a 3 px accent bar on the
   leading edge (or equivalent full-width treatment), no inner border.
3. **"Recommended" gets a real badge** (a small accent-tinted capsule with the
   word), not small grey text appended to the id.
4. **State-aware header copy.** Today: "8 GB or more · Suramak picks Balanced
   here" while the in-use model is Compact. Say the truth in one line:
   `12 GB RAM · Balanced recommended` when the tier default is not installed,
   and `12 GB RAM · using Compact — Balanced recommended` when a different
   model is in use. "RAM" is named explicitly (the review flagged the
   ambiguity).
5. **The action word**: the recommended-but-not-installed row's action is
   `Download` (primary accent), other non-installed rows keep `Get`, the
   installed-not-selected row keeps `Use`.

## Verification

- Settings goldens re-blessed (dark, light, tiers, model-truth, grouped).
- A test pinning the header copy across the three states (tier default in
  use / other model in use / nothing installed), and one asserting no raw id
  string appears in the primary card's painted text (the row-struct path the
  tests can read) while Diagnostics still carries them.
- fmt/clippy -D warnings; NDK cross-check on both Android crates; workspace
  tests green (one full run at the end).
- Report: append a §7 to `docs/reports/M20d-settings.md`.

## Do NOT

- No logic changes to model state; no Library/editor edits; no new crates;
  no CI edits; no git commit/push; no sudo.
