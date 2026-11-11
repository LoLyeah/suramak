# M19 — Android audit plan (Carbon planning pass)

Source: `gemini-4-carbon` (Google AI Studio), two calls, 2026-11-11, with the
owner's Settings screenshot. Prompt files:
`carbon-android-audit-prompt.md` / `carbon-android-audit-followup.md` (scratch).

Carbon's output is a hypothesis with good manners — the parent verifies each
named mechanism against the repo before implementing (one claim per line,
`VERIFIED` / `OPEN` below).

## The two confirmed defects

1. **Settings does not scroll.** `settings.rs` paints rows at absolute
   `y = bands.content.top() + …` *inside* `ScrollArea::show`, using the OUTER
   band's static top rather than the scrolled cursor. `library.rs` (which
   scrolls) uses `ui.cursor().top()` in the same position. The scroll range
   exists (one `allocate_space` at the end), the content simply never moves.
   **VERIFIED in code.** Carbon's inferred mechanism matches.
2. **A model can read "In use" without a file.** "In use" is
   `selected = row.id == settings.model_id`; `installed` only picks
   "Use"/"Get" for other rows. `Settings::default()` = `turbo-q5-0` (the macOS
   default), and the Android host never applies the phone-tier default it
   already knows (base 4–6 GB, small 8 GB+). **VERIFIED in code.**

## The open question: how do taps work at all?

Carbon [INFERRED] that with winit emitting only `WindowEvent::Touch` and
egui-winit forwarding only `egui::Event::Touch`, no `Sense::click()` widget
could fire. Verified: winit 0.30.13 (and the fork) emit Touch only;
egui-winit 0.33 forwards Touch only; egui's `PointerState` consumes only
`PointerMoved`/`PointerButton`; nothing in the app synthesises pointer events.
Yet the owner navigated to Settings, so SOMETHING delivers clicks. **OPEN —
Step 0 of M19A is an instrumented tap on the emulator that logs the raw event
kinds.** Fixes must not depend on the assumed mechanism: M19A synthesises
primary-touch pointer events explicitly (idempotent if some path already
does it: a `PointerButton` already seen in the frame's events suppresses
synthesis for that phase).

## Fix order (Carbon's slices, adopted with names)

| Slice | Scope | Acceptance |
|---|---|---|
| **M19A** (WO-1) | Touch→pointer synthesis + flow-relative Settings layout + bottom inset clearance; Step 0 instrumentation | Settings flicks top→bottom; DEVICE card clears the gesture pill; taps still work (emulator smoke + screenshots) |
| **M19B** (WO-2) | Truthful model registry: tier default at first run AND migration for existing installs; "In use" only with a verified file; honest header/copy; per-row state machine (not downloaded / downloading / stale / installed / in use) | Fresh + upgraded install on a 8 GB+ device both target `small`; no "In use" before the file lands; flip only after verification |
| **M19C** (WO-3) | Download resilience: StatFs preflight, `.part` cleanup on drop, actionable retry | Airplane-mode mid-download aborts cleanly, no partial left behind |
| **M19D** (WO-4) | FGS lifecycle, surface suspend mid-transcribe, notification deep links | Denied/granted notification permission paths; background + rotate mid-job; tap notification → exact transcript |
| **M19E** (WO-5) | Live config sync (theme/font scale/locale) + 120 Hz repaint discipline | OS dark-mode toggle updates live; idle Settings = no repaints |

## Audit checks adopted (C5–C12), each with a pass/fail line

5. IME: opening the keyboard insets the content; focused field visible; back
   dismisses with clean focus.
6. Rotation/process death mid-transcribe: job survives or resumes honestly.
7. Live theme/font-scale/locale changes apply without restart.
8. TalkBack/Switch Access: the surface exposes labeled nodes (a11y audit
   already records every control; the AccessKit→Android bridge is [OPEN] —
   verify before claiming).
9. Notification denial + Android 16 FGS start restrictions (background start
   must use the allowed path).
10. Low storage / offline: preflight + cleanup + honest banner.
11. Notification deep links route to the exact transcript.
12. 120 Hz discipline: no idle repaints, no jank during kinetic scroll.

Carbon's tone on 8/11 ("FAIL" for unverified items) is noted as speculation,
not evidence; each slice's report must state what was actually measured.
