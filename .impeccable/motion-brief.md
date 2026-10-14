# Motion brief — Suramak

Shaped per impeccable `shape` (v4.4.0), 2026-10-14. Status: **confirmed by owner 2026-10-14** (decisions in §7). This is the standing motion contract: each future milestone implements its part; each surface gets an `impeccable animate` review at polish time.

## 1. Job and audience

Suramak is used in **Operate** mode: people record, read, edit, export. Motion's job is not spectacle — it is the legibility of state and the felt quality of a native Tahoe-era app. Every change is *perceived*; nothing teleports; the signature objects move at Apple's restraint. The audience is the owner (design-taste evaluator, on an M4 MacBook Air) and, by extension, macOS users — the yardstick is a side-by-side with Apple's own apps.

## 2. Outcome and proof

- No state change is a teleport: route, pane, mode, selection, list mutation, status.
- Three signature moments read instantly: (a) the pill's idle↔recording morph; (b) the playback strip — playhead motion + active-segment follow; (c) live transcript arrival cadence.
- Motion stays inside the repaint budget (M9b): animations schedule frames while active; the app is never held at full tilt.
- Reduce Motion honored everywhere: state changes snap; opacity only where legibility demands.
- Proof: frame-count instrumentation (M9b before/after), screenshot poses at animation midpoints for the signature moments, owner's eyes on the Mac.

## 3. Selected direction

**Motion as grammar, not decoration.** The world (C — "OS citizen") stays; motion inherits its restraint.

- Tokens (foundation from M9a): fast 120 / base 180 / slow 240 ms, ease-out family; springs only where physicality is the point (the pill).
- Three levels:
  - **L1 Transitions** — routes / panes / dialogs: fade with a small rise (≤4 px); content fades, surfaces crossfade; navigation never slides half a screen.
  - **L2 Signature objects** — *pill*: state morph (dot/label crossfade, gentle scale; press = soft spring; recording = living dot, not a blink). *Waveform*: playhead moves continuously (not stepped); click-to-seek eases; follow-scroll animates. *Transcript*: live lines fade+rise in; active-segment tint crossfades; caret/selection stay native.
  - **L3 Micro** — controls (hover/press/focus per tier); list mutations (insert/remove ease + height animate); toasts/status (fade; dismiss timer pauses on hover); progress (model download, jobs — smoothed, no jitter); settings apply (subtle confirm pulse).
- Interruption discipline: animations retarget from current value (never queue; never snap-back); rapid state flips land correctly; the initial frame renders statically — only *changes* animate.
- Reduce Motion: durations → 0; opacity-only survivors where legibility demands (per the M9a rule).

## 4. Scope and boundaries

- Every surface: Home, Editor, Record, Settings, Model manager, Export/Polish flows, dialogs, empty/loading/error states.
- **Anti-goals**: no idle decoration (nothing loops without a state reason); no parallax/3D/gimmicks; no motion that shifts text measure; nothing that fights the keyboard; every animation ends.
- Costs: motion respects the repaint budget; frames requested only while animating; idle stays event-driven.
- Constraints: egui immediate-mode; helpers live in `crates/suramak-ui/src/motion.rs` (extend the M9a foundation); no new crates without an argument.

## 5. States needing motion (the checklist milestones consume)

- Route set: home ↔ editor ↔ record ↔ settings ↔ models ↔ transcribe/polish/export.
- Record lifecycle: idle → arming → recording → stopping → finalizing → saved.
- Live: line arrivals; provisional → final swap.
- Playback: play/pause, seek, follow-scroll, end-of-audio.
- Lists: create / delete / rename; search filter results.
- Status: toasts, inline errors, destructive confirms (trash), permission prompts.
- Progress: model download, transcription jobs, polish jobs, export.
- Settings: appearance switch (material swaps instantly — no animation on it), toggles, language picker.
- First-run: empty library, sample hints.

## 6. Interaction and layout intents

- What moves vs what fades: surfaces crossfade/rise; text content fades in place (never slides under the eye mid-read); chrome (deck, sidebar, titlebar) is static — its *contents* animate.
- Durations by class as in §3; rises ≤4 px; scales ≤2% except the pill press.
- Enter/exit: exits faster than enters (exit fast 120, enter base 180).
- The pill floats above everything: it persists across routes; only its state morphs.

## 7. Constraints and decided details

- Implementation: extend `motion.rs`; every workorder from M9b on carries a **Motion** section mapping its states to L1/L2/L3.
- Gates: `impeccable animate` review per surface at polish time; the finish review walks §5.
- **Owner decisions (confirmed 2026-10-14)**:
  - **Record view delight — subtle.** A living dot + the real level meter only. No breathing/pulsing waveform, no ambient pulses.
  - **Pill press — gentle spring, no bounce.**
