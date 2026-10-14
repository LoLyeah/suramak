# WORKORDER — M9d: the owner-confirmed motion details + finish-review polish

Tree: `55177a0`. Read `docs/reports/M9-FINISH-REVIEW.md` §3, §6, §7, §8 first — this slice implements exactly the named items. The look is frozen; change nothing else.

## Items

1. **Pill — the spring (motion brief §3 + §7, owner-confirmed: "gentle spring, no bounce").** Press = a gentle spring: scale to ≈0.97 on press with a springy release (critically-damped-ish, overshoot ≤ 2%, ends within ~240 ms). The idle↔recording morph gains the brief's gentle scale component (the existing label/colour crossfade stays). Reduce Motion → snap (no spring).
2. **Record view — the living dot (brief §7 Q1: "living dot + level meter only", owner-confirmed).** A small red dot beside the `recording` status word with a subtle opacity breath (~0.7↔1.0, period ≥ 1 s, one-shot-friendly; nothing else pulses — no waveform breathing). Reduce Motion → static dot.
3. **Waveform — seek eases, follow animates (brief §3).** Click-to-seek: the drawn playhead eases to the new position (fast/base token) while the underlying data/seek math stays an instant jump — do not change `waveform_ms_at` semantics or the protected click-to-seek model. Follow-scroll: the transcript's active-row follow while playing animates (eased scroll offset between rows, base token, ends) instead of an instant `scroll_to_rect` snap. Reduce Motion → instant both.
4. **Home "Re" fragment (review §6.1, P3).** The capture-not-compiled caption is drawn in the flow while the drop zone is absolutely positioned — the zone covers all but "Re". Move that caption above the zone (or into the message slot); add a pixel check that no ink peeks from under the zone's top edge on the capture-free build.
5. **Source-row tooltip (review §7.1).** `record.rs`: `.on_hover_text(&source.label)` on the truncated source row (the a11y label already exists).
6. **First frame static (review §6.3).** Suppress the route fade on the very first presented frame (fresh epoch at startup).
7. **Contrast artifact (review §6.6 / brief principle 8).** A test computing WCAG contrast for the key pairs (ink & ink_soft on panel and card, light + dark; `record_red` as 12 px text on the light card must clear ≥ 4.5:1 — adjust the token or introduce a text-safe red variant if it doesn't). Report the measured numbers.
8. **Harness fix (review §6.5).** The status-line pixel probe in the evidence harness mis-scopes to y 355–380 (it hits the Models/Settings buttons); move it to the actual status band (≈ y 380–400).

## Do NOT
- Restyle anything beyond the named items; touch the asr/capture/live pipeline, playback internals (item 3 is draw/scroll-side only), glass runtime (appearance code unchanged), taps, instance, shutdown; add crates or fonts; `git commit`/`push`; modify files outside `/home/ubuntu/suramak`.

## Verification
- fmt clean; clippy `--workspace --all-targets -- -D warnings` clean; `RUST_TEST_THREADS=1 cargo test --workspace` green with new tests: pill spring math (overshoot bounded, ends, snaps under Reduce Motion); seek-ease and follow-ease end + snap under Reduce Motion; contrast math for the token pairs; first-frame-static; the Home fragment guard.
- xvfb smokes green; screenshots: record view with the living dot, the editor strip settled, Home (no fragment); the full pixel-check suite still passes with the corrected status probe; `cargo check -p suramak-glass --target aarch64-apple-darwin` clean.
- Report `docs/reports/M9d.md`: item-by-item (what changed, evidence path, measured numbers); deviations with whys; a closing line on what remains for M10/finish (nothing new expected).

## After this slice
Owner's macOS acceptance test (their pull closes M9); then the next release cut (v0.1.1) and M10 (fast engines).
