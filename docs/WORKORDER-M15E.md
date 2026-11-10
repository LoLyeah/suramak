# WORKORDER — M15-E: opt-in bilingual export (M15.8)

Plan source: `docs/PLAN-M15-M18.md` §3 M15.8 (size M). Tree: `main` at
`5d026a9` (M15-C merged: 723 tests, schema v4 FTS search, snippets + pending
seek). This the LAST M15 slice before v0.4.0. M15.6 alignment stays out of this
run (its feasibility workorder will follow independently — see PLAN-NOTES;
do not start it here).

## Deliverables

1. **Explicitly requested translation only**: no endpoint call happens merely
   by opening any panel; the bilingual export action norm: user clicks
   "Translate for export" on a transcript → the Polish endpoint (already wired
   for the M10 polish flow) is called per segment (bounded batch sizing,
   documented), editable results populate the derived-text column with
   provenance (model + timestamps + the source segment id set).
2. **Stale tracking**: editing (or splitting/merging via M15.3) marks the
   translation stale — a visible badge column; a re-translate is per-user
   action; the stale marker survives restore.
3. **Bilingual output formats**: stacked SRT (EN line / blank / ID line with
   the documented per-line budget), VTT, and the M15-A ASS serializer's new
   style pair (top/bottom position per the spec's docs); the honest line max
   per subtitle row respected (CPL/CPS advisory badges show on BOTH lines).
4. **Failure + cancellation**: endpoint failure names itself per segment
   (partial results inline, errored segments show a retry affordance);
   mid-flight cancellation lands everywhere (the existing job cancellation
   seam); zero endpoint traffic while the endpoint toggle is off (asserted in
   tests, not promised).
5. **No-endpoint users**: the bilingual export surface is visible but disabled
   with the resolved reason; the ordinary export paths unchanged.

Verify (the gate): EN→ID and ID→EN now fixture-tested through a MOCK endpoint
(the real endpoint is opt-in and unavailable in CI), cue alignment, escaping
per format, stale-after-edit, partial-failure + retry, cancellation, the
zero-requests-when-disabled assertion, restored provenance; fmt/clippy/
workspace tests (baseline 723→grow; ONE full workspace run at the end).

## Do NOT

- No automatic translation, no engine changes, no new deps, no CI edits, no
  Android work, no git commit/push, no sudo; report:
  `docs/reports/M15e-bilingual-export.md` (batch sizing, stale semantics per
  mutation type, format notes, honest boundaries).
