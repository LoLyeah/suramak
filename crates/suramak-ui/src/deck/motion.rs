//! Motion system (M9a): duration tokens, ease-out only, Reduce Motion → snap.
//!
//! The three tokens are the whole vocabulary — fast 120 ms (hover/press),
//! base 180 ms (state swaps: the pill morph, live-line arrivals), slow 240 ms
//! (route changes). Everything eases out; nothing bounces. Every animation
//! ENDS: the helpers are built on egui's animation manager, which requests
//! repaints only while a value is between 0 and 1, so an idle UI goes back to
//! on-demand repaints when the motion is over.
//!
//! Reduce Motion (macOS `NSWorkspace.accessibilityDisplayShouldReduceMotion`,
//! read through `suramak-glass`) sets every duration to 0 — a snap. Opacity is
//! kept where legibility demands it (a snapped-to-1 fade is simply opaque).
//!
//! ## M20A: the screen-facing vocabulary
//!
//! The M9a helpers animate an egui `Id` through the context ([`Motion::factor`],
//! [`Motion::press_spring`]); they are what the desktop's widgets use. The
//! phone's transitions are state machines owned by the caller, so M20A adds a
//! second, pure family that takes `now` and returns plain values:
//!
//! | helper | token | what moves | Reduce Motion |
//! |---|---|---|---|
//! | [`ScreenMotion`] | [`SLOW`] | incoming ±24 pt X + alpha; outgoing ∓8 pt + alpha | [`FAST`] crossfade, no travel |
//! | [`SheetMotion`] | [`SLOW`] | slide 100 %→0 + scrim + alpha | [`FAST`] crossfade, no slide |
//! | [`PressMotion`] | [`FAST`] in / [`SPRING_DURATION`] out | scale 1→0.97 + fill 0→1 | fill swap, no scale |
//! | [`SuccessBadge`] | [`BASE`] scale, [`SLOW`] flash | scale 0.88→1 + `ok` flash 0.22→0 | instant |
//! | [`RubberBand`] | [`BASE`] release | ≤4 pt offset past a scroll bound | hard clamp |
//!
//! Each ends exactly at its endpoint and reports `running` — and
//! [`AnimationClock`] turns "any helper running" into at most one
//! `request_repaint` per frame. That is the whole repaint discipline: no idle
//! loop exists to forget.

use egui::{Color32, Id};

/// Fast token: hover / press / active-segment tint (120 ms).
pub const FAST: f32 = 0.120;
/// Base token: state swaps — the pill morph, live-line arrivals (180 ms).
pub const BASE: f32 = 0.180;
/// Slow token: route changes — fade + rise (240 ms).
pub const SLOW: f32 = 0.240;

/// The pill's scale while pressed (M9d item 1: "≈0.97").
pub const SPRING_PRESS_SCALE: f32 = 0.97;
/// The press spring's duration — the brief's "ends within ~240 ms".
pub const SPRING_DURATION: f32 = 0.240;
/// The spring's overshoot ceiling as a fraction of the travel (2 %). The
/// authored spring peaks at ~1.5 % — the owner's "gentle spring, no bounce":
/// the scale never deviates more than 0.05 % from its target.
pub const SPRING_OVERSHOOT: f32 = 0.02;
/// A position change larger than this (ms) is a **seek** — the drawn playhead
/// eases to it. Smaller changes are the playback data itself and are followed
/// exactly (the playhead stays continuous, never lagged).
pub const SEEK_JUMP_MS: f32 = 120.0;

/// The one easing: cubic ease-out (`emath::easing::cubic_out`).
pub fn ease_out(t: f32) -> f32 {
    egui::emath::easing::cubic_out(t.clamp(0.0, 1.0))
}

/// The pill's press spring (M9d item 1, motion brief §3/§7 — owner-confirmed:
/// "gentle spring, no bounce"): the damped-spring step response as an easing
/// function over [`SPRING_DURATION`]. `0 → 0`, `1 → 1` (the motion ENDS), and
/// the first overshoot peaks at ~1.5 % of the travel — bounded by
/// [`SPRING_OVERSHOOT`], and sub-0.1 % of the pill's scale, so it reads as
/// spring, never as bounce.
pub fn spring_ease(t: f32) -> f32 {
    /// Damping ratio — "critically-damped-ish" (ζ = 0.80 → ~1.5 % overshoot).
    const ZETA: f32 = 0.80;
    /// Undamped natural frequency over the normalised token: the overshoot
    /// peaks at ~26 % of the duration (≈60 ms of the 240 ms) and the tail is
    /// fully settled (1e-7) by t = 1.
    const OMEGA0: f32 = 20.0;
    let t = t.clamp(0.0, 1.0);
    if t >= 1.0 {
        return 1.0;
    }
    let lambda = ZETA * OMEGA0;
    let omega = OMEGA0 * (1.0 - ZETA * ZETA).sqrt();
    let decay = (-lambda * t).exp();
    let value = 1.0 - decay * ((omega * t).cos() + (lambda / omega) * (omega * t).sin());
    // The analytic response is inside [0, 1 + 2 %]; the clamp only guards the
    // float edges (the easing must never push a caller past the bound).
    value.clamp(0.0, 1.0 + SPRING_OVERSHOOT)
}

/// How far apart two clocks may be before a transition treats them as
/// different bases. The UI advances with egui's input time; a host command
/// (a notification deep link, a back from Kotlin, a worker event) carries the
/// process uptime instead. Both start at process launch, so they differ by at
/// most the time since the last context rebuild — but a rebuild can reset
/// egui's clock. A gap larger than this is a base mismatch, not jitter.
pub const CLOCK_SKEW_LIMIT: f64 = 0.5;

/// Re-anchors a transition whose stored start is in a **different clock base**
/// than the frame that is advancing it. Called at the top of every `advance`:
/// without it, a host-started transition sees a negative elapsed time forever,
/// stays at progress 0 and keeps asking for frames (an invisible sheet and a
/// battery leak). Small skew is left alone so a normal UI-started animation is
/// never restarted.
pub fn reanchor(started_at: &mut f64, now: f64) {
    if now + CLOCK_SKEW_LIMIT < *started_at {
        *started_at = now;
    }
}

/// The spring's value at normalised progress `t` (0..=1) between `from` and
/// `to` — [`spring_ease`] scaled onto the travel. Exactly `from` at 0, exactly
/// `to` at 1, bounded overshoot in between.
pub fn spring_response(t: f32, from: f32, to: f32) -> f32 {
    from + (to - from) * spring_ease(t)
}

/// The motion policy for this run: Reduce Motion or the full token set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Motion {
    pub reduce: bool,
}

impl Motion {
    pub const fn new(reduce: bool) -> Self {
        Self { reduce }
    }

    /// The effective duration for a token: the token, or 0 when Reduce Motion
    /// is on (a snap — egui's animation manager returns the target value
    /// immediately for a zero duration).
    pub fn duration(&self, token: f32) -> f32 {
        if self.reduce {
            0.0
        } else {
            token
        }
    }

    /// An animated 0..=1 factor for a boolean state (hover, playing, active…).
    ///
    /// On the very first call for `id` this returns the target immediately
    /// (egui's rule); transitions between states animate. For one-shot
    /// appearances use [`Motion::arrive`].
    pub fn factor(&self, ctx: &egui::Context, id: Id, on: bool, token: f32) -> f32 {
        ctx.animate_bool_with_time_and_easing(id, on, self.duration(token), ease_out)
    }

    /// A directional 0..=1 factor with **different enter and exit durations**
    /// (the brief's "exits faster than enters": enter base 180 / exit fast 120
    /// for the export sheet and status lines).
    ///
    /// Built on the same animation manager as [`Motion::factor`]: on a mid-flight
    /// flip the value retargets from its *current* position at the new
    /// direction's rate — it never queues and never snaps back. Under Reduce
    /// Motion both durations are 0, so the flip is a snap either way.
    pub fn fade(&self, ctx: &egui::Context, id: Id, on: bool, enter: f32, exit: f32) -> f32 {
        let duration = self.duration(if on { enter } else { exit });
        ctx.animate_bool_with_time_and_easing(id, on, duration, ease_out)
    }

    /// A one-shot 0..=1 arrival factor: call with a FRESH `id` per arrival
    /// (e.g. one per new transcript line). The first frame renders at 0, then
    /// the factor eases to 1 over the token — and stops. Under Reduce Motion
    /// the arrival is a snap: visible on the very first frame.
    pub fn arrive(&self, ctx: &egui::Context, id: Id, token: f32) -> f32 {
        if self.reduce {
            // Snap: no registration dance, visible immediately.
            return ctx.animate_bool_with_time_and_easing(id, true, 0.0, ease_out);
        }
        let armed = ctx.data_mut(|data| {
            data.get_temp::<bool>(id.with("suramak-arrive"))
                .unwrap_or(false)
        });
        if !armed {
            // Register the id at 0 first (a zero-duration write), hide this
            // frame, then animate from the next one.
            let _ = ctx.animate_bool_with_time_and_easing(id, false, 0.0, ease_out);
            ctx.data_mut(|data| data.insert_temp(id.with("suramak-arrive"), true));
            ctx.request_repaint();
            return 0.0;
        }
        ctx.animate_bool_with_time_and_easing(id, true, token, ease_out)
    }

    /// The pill's press spring (M9d item 1): the pill's **scale**, springing
    /// between `target` (1.0 at rest, [`SPRING_PRESS_SCALE`] pressed) over
    /// [`SPRING_DURATION`]. A flip mid-flight restarts the spring from the
    /// *current* value (never snap-back, never queued); the value is exactly
    /// the target once the duration elapses; Reduce Motion returns the target
    /// immediately (no spring). State lives in egui's temp data under `id`.
    pub fn press_spring(&self, ctx: &egui::Context, id: Id, target: f32) -> f32 {
        if self.reduce {
            return target;
        }
        let now = ctx.input(|input| input.time);
        let mut state = ctx.data_mut(|data| {
            data.get_temp::<PressSpring>(id).unwrap_or(PressSpring {
                value: target,
                from: target,
                to: target,
                start: now,
            })
        });
        if state.to != target {
            // Retarget from the current value — a mid-flight release continues
            // the scale from where the press left it.
            state.from = state.value;
            state.to = target;
            state.start = now;
        }
        let elapsed = (now - state.start) as f32;
        if elapsed >= SPRING_DURATION || state.from == state.to {
            // Ended (or settled at the target): the value IS the target and no
            // frame is requested — an idle pill asks for nothing (brief §4:
            // frames only while animating).
            state.value = state.to;
        } else {
            state.value = spring_response(elapsed / SPRING_DURATION, state.from, state.to);
            ctx.request_repaint();
        }
        ctx.data_mut(|data| data.insert_temp(id, state));
        state.value
    }

    /// The current word's highlight spring (M15.6 karaoke): the same damped
    /// spring as the press, ramped 0→1 when a word **becomes** the current one
    /// and 1→0 when it stops being it. Reduce Motion returns the state's own
    /// value (1.0 or 0.0) immediately — the highlight is kept, only its arrival
    /// stops springing. There is deliberately no new motion vocabulary here:
    /// the constants are [`SPRING_DURATION`] / [`SPRING_OVERSHOOT`].
    pub fn highlight_spring(&self, ctx: &egui::Context, id: Id, current: bool) -> f32 {
        self.press_spring(ctx, id, if current { 1.0 } else { 0.0 })
    }

    /// The drawn playhead's seek ease (M9d item 3): what the strip *draws*
    /// eases toward `target` (ms) when the position **jumps** — a click-to-seek
    /// or a segment-stamp seek — while frame-to-frame playback advance is
    /// followed exactly, so the playing playhead is never lagged and the
    /// underlying data/seek math is untouched. The ease ends exactly at the
    /// target within `token`; Reduce Motion returns the target immediately.
    /// State lives in egui's temp data under `id`.
    pub fn seek_ease(&self, ctx: &egui::Context, id: Id, target: f32, token: f32) -> f32 {
        if self.reduce {
            return target;
        }
        let now = ctx.input(|input| input.time);
        let mut state = ctx.data_mut(|data| {
            data.get_temp::<SeekEase>(id).unwrap_or(SeekEase {
                value: target,
                target,
                from: target,
                start: now,
                easing: false,
            })
        });
        if target != state.target {
            let jump = (target - state.target).abs();
            if jump > SEEK_JUMP_MS {
                // A seek: ease from wherever the drawn playhead is now.
                state.from = state.value;
                state.start = now;
                state.easing = true;
            } else if !state.easing {
                // The data itself (playback, or a hair of drift): follow it.
                state.value = target;
            }
            state.target = target;
        }
        if state.easing {
            let elapsed = (now - state.start) as f32;
            if elapsed >= token {
                state.value = state.target;
                state.easing = false;
            } else {
                state.value = state.from + (state.target - state.from) * ease_out(elapsed / token);
                ctx.request_repaint();
            }
        } else {
            state.value = state.target;
        }
        ctx.data_mut(|data| data.insert_temp(id, state));
        state.value
    }
}

/// Linear colour mix (used for tint crossfades: hover, active-segment, press).
pub fn lerp_color(from: Color32, to: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        mix(from.r(), to.r()),
        mix(from.g(), to.g()),
        mix(from.b(), to.b()),
        mix(from.a(), to.a()),
    )
}

/// Overlays a translucent tint on `base` at strength `t` (0..=1) — the row /
/// control hover and active washes.
pub fn tint_over(base: Color32, tint: Color32, t: f32) -> Color32 {
    lerp_color(base, tint, t.clamp(0.0, 1.0))
}

/// The residual rise offset for a fade+rise animation: full `distance` at 0,
/// 0 at 1 (the "no teleport" route entrance).
pub fn rise(t: f32, distance: f32) -> f32 {
    (1.0 - t.clamp(0.0, 1.0)) * distance
}

/// Interpolates a scalar between `from` (at t=0) and `to` (at t=1) — the
/// geometric helper for scale/slide motion (ring insets, popover scale, rises).
pub fn scale(t: f32, from: f32, to: f32) -> f32 {
    from + (to - from) * t.clamp(0.0, 1.0)
}

/// Progress (0..=1) of a fade+rise that started `age` ago and lasts `token`.
/// A zero (or negative) token is Reduce Motion's snap: immediately 1.
pub fn arrival_progress(age: std::time::Duration, token: f32) -> f32 {
    if token <= 0.0 {
        return 1.0;
    }
    ease_out(age.as_secs_f32() / token)
}

/// A token (seconds) as a [`std::time::Duration`] — the seam for state that
/// timestamps its animations (list arrivals, the settings confirm).
pub fn token_duration(token: f32) -> std::time::Duration {
    std::time::Duration::from_secs_f32(token.max(0.0))
}

/// The pill press spring's state (M9d item 1) — see [`Motion::press_spring`].
#[derive(Debug, Clone, Copy)]
struct PressSpring {
    /// The current scale.
    value: f32,
    /// Where the running spring started.
    from: f32,
    /// The target scale (1.0 rest, [`SPRING_PRESS_SCALE`] pressed).
    to: f32,
    /// Start time (`egui::InputState::time`).
    start: f64,
}

/// The drawn playhead's seek-ease state (M9d item 3) — see
/// [`Motion::seek_ease`].
#[derive(Debug, Clone, Copy)]
struct SeekEase {
    /// The drawn value (ms).
    value: f32,
    /// The last data target (ms).
    target: f32,
    /// Where the running ease started.
    from: f32,
    /// Start time (`egui::InputState::time`).
    start: f64,
    /// Whether a timed ease is in flight.
    easing: bool,
}

// ---------------------------------------------------------------------------
// M20A: the screen-facing motion vocabulary (pure, caller-owned state)
// ---------------------------------------------------------------------------

/// How far the arriving screen travels on a push/pop, in points (the plan's
/// ±24). The outgoing screen drifts the other way by [`SCREEN_EXIT_TRAVEL`].
pub const SCREEN_TRAVEL: f32 = 24.0;
/// The departing screen's drift, in points (the plan's ∓8).
pub const SCREEN_EXIT_TRAVEL: f32 = 8.0;
/// The press scale (the plan's 0.97 — the same value as the M9d pill spring).
pub const PRESS_SCALE: f32 = SPRING_PRESS_SCALE;
/// The success badge's starting scale (the plan's 0.88).
pub const SUCCESS_SCALE_FROM: f32 = 0.88;
/// The success flash's starting alpha (the `ok` tint at 0.22).
pub const SUCCESS_FLASH_ALPHA: f32 = 0.22;
/// The scroll rubber-band's ceiling, in points (the plan's 2–4).
pub const RUBBER_MAX: f32 = 4.0;
/// The distance (points) at which the rubber-band reaches 1 − 1/e of its
/// ceiling — the resistance constant.
pub const RUBBER_RESISTANCE: f32 = 24.0;

/// The easing a [`Tween`] rides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Curve {
    /// The one ease-out ([`ease_out`]).
    Ease,
    /// The Deck's damped spring ([`spring_ease`]) — press release only.
    Spring,
}

/// A one-shot scalar animation: from `from` at `start`, exactly `to` once
/// `duration` has elapsed, eased in between.
///
/// `sample` returns the value **and whether the tween is still running**, which
/// is the only thing allowed to request a repaint (see [`AnimationClock`]). A
/// zero duration is the Reduce Motion snap: the first sample is already the
/// endpoint and nothing runs.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tween {
    from: f32,
    to: f32,
    value: f32,
    started_at: f64,
    duration: f32,
    curve: Curve,
    running: bool,
}

impl Tween {
    /// A tween that has already settled at `value` — the first frame of a
    /// helper must not animate (no startup teleport).
    fn settled(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            value,
            started_at: 0.0,
            duration: 0.0,
            curve: Curve::Ease,
            running: false,
        }
    }

    /// Starts a tween at `now`. `duration == 0` settles immediately.
    fn begin(from: f32, to: f32, now: f64, duration: f32, curve: Curve) -> Self {
        let mut tween = Self {
            from,
            to,
            value: from,
            started_at: now,
            duration,
            curve,
            running: true,
        };
        // A zero token is Reduce Motion's snap: no frame is ever needed.
        if duration <= 0.0 {
            tween.value = to;
            tween.running = false;
        }
        tween
    }

    /// Retargets toward a new endpoint, continuing from the current value (a
    /// mid-flight flip never snaps back and never queues).
    fn retarget(&mut self, to: f32, now: f64, duration: f32, curve: Curve) {
        if self.running && to == self.to {
            return; // already heading there — do not restart the clock
        }
        if to == self.value && to == self.to {
            return; // settled at the requested value
        }
        self.from = self.value;
        self.to = to;
        self.started_at = now;
        self.duration = duration;
        self.curve = curve;
        self.running = true;
        if duration <= 0.0 {
            self.value = to;
            self.running = false;
        }
    }

    /// Jumps to `value` with no animation (a drag's frame-by-frame offset, or
    /// Reduce Motion's instant swap).
    fn snap(&mut self, value: f32) {
        self.from = value;
        self.to = value;
        self.value = value;
        self.running = false;
    }

    fn running(&self) -> bool {
        self.running
    }

    /// Advances to `now`. Returns `(value, running)`; once `running` is false
    /// the value is exactly `to` and stays there.
    fn sample(&mut self, now: f64) -> (f32, bool) {
        if !self.running {
            return (self.value, false);
        }
        // A host-started tween can carry a clock base older than this frame's
        // (a context rebuild). Re-anchor rather than run backwards forever.
        reanchor(&mut self.started_at, now);
        let elapsed = (now - self.started_at).max(0.0) as f32;
        if self.duration <= 0.0 || elapsed >= self.duration {
            self.value = self.to;
            self.running = false;
            return (self.value, false);
        }
        let t = elapsed / self.duration;
        let eased = match self.curve {
            Curve::Ease => ease_out(t),
            Curve::Spring => spring_ease(t),
        };
        self.value = self.from + (self.to - self.from) * eased;
        (self.value, true)
    }
}

/// Which way a screen transition goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenDirection {
    /// A new screen enters from the right (`+X`).
    Push,
    /// The current screen leaves to the right (`−X` arriving from the left).
    Pop,
}

/// One frame of a screen transition, in points / alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenFrame {
    /// Incoming screen's horizontal offset; 0 at the end.
    pub incoming_dx: f32,
    /// Incoming screen's opacity; 1 at the end.
    pub incoming_alpha: f32,
    /// Outgoing screen's horizontal offset; ±[`SCREEN_EXIT_TRAVEL`] at the
    /// end, 0 at the start.
    pub outgoing_dx: f32,
    /// Outgoing screen's opacity; 0 at the end.
    pub outgoing_alpha: f32,
    /// True while another frame is needed.
    pub running: bool,
}

/// A push/pop transition: [`SLOW`], incoming ±24 pt → 0 with an opacity
/// crossfade, outgoing ∓8 pt drift. Under Reduce Motion the travel is removed
/// and only a [`FAST`] crossfade remains (opacity is not vestibular motion —
/// cutting straight to the next screen is what loses a reduced-motion user
/// their place).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenMotion {
    direction: ScreenDirection,
    tween: Tween,
    reduce: bool,
}

impl ScreenMotion {
    /// A push starting at `now`.
    pub fn push(now: f64, reduce: bool) -> Self {
        Self::new(ScreenDirection::Push, now, reduce)
    }

    /// A pop starting at `now`.
    pub fn pop(now: f64, reduce: bool) -> Self {
        Self::new(ScreenDirection::Pop, now, reduce)
    }

    fn new(direction: ScreenDirection, now: f64, reduce: bool) -> Self {
        let token = if reduce { FAST } else { SLOW };
        Self {
            direction,
            tween: Tween::begin(0.0, 1.0, now, token, Curve::Ease),
            reduce,
        }
    }

    /// The transition's value this frame.
    pub fn sample(&mut self, now: f64) -> ScreenFrame {
        let (progress, running) = self.tween.sample(now);
        if self.reduce {
            return ScreenFrame {
                incoming_dx: 0.0,
                incoming_alpha: progress,
                outgoing_dx: 0.0,
                outgoing_alpha: 1.0 - progress,
                running,
            };
        }
        let sign = match self.direction {
            ScreenDirection::Push => 1.0,
            ScreenDirection::Pop => -1.0,
        };
        ScreenFrame {
            incoming_dx: sign * SCREEN_TRAVEL * (1.0 - progress),
            incoming_alpha: progress,
            outgoing_dx: -sign * SCREEN_EXIT_TRAVEL * progress,
            outgoing_alpha: 1.0 - progress,
            running,
        }
    }

    /// Whether the transition is still between endpoints.
    pub fn is_running(&self) -> bool {
        self.tween.running()
    }
}

/// One frame of a sheet transition. `slide` is the fraction of the sheet's own
/// height still below its resting place (1 = fully off, 0 = presented); the
/// caller multiplies by the sheet rect's height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SheetFrame {
    pub slide: f32,
    pub scrim: f32,
    pub alpha: f32,
    pub running: bool,
}

/// A sheet present/dismiss: [`SLOW`], slide 100 %→0 with the scrim and the
/// sheet's own alpha rising together. Reduce Motion keeps the [`FAST`]
/// crossfade and removes the slide.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SheetMotion {
    presenting: bool,
    tween: Tween,
    reduce: bool,
}

impl SheetMotion {
    /// A sheet presenting at `now`.
    pub fn present(now: f64, reduce: bool) -> Self {
        Self::new(true, now, reduce)
    }

    /// A sheet dismissing at `now`.
    pub fn dismiss(now: f64, reduce: bool) -> Self {
        Self::new(false, now, reduce)
    }

    fn new(presenting: bool, now: f64, reduce: bool) -> Self {
        let token = if reduce { FAST } else { SLOW };
        Self {
            presenting,
            tween: Tween::begin(0.0, 1.0, now, token, Curve::Ease),
            reduce,
        }
    }

    /// The sheet's value this frame.
    pub fn sample(&mut self, now: f64) -> SheetFrame {
        let (progress, running) = self.tween.sample(now);
        // `shown` is the presentation level: 0 hidden, 1 presented. A dismiss
        // runs the same 0→1 tween against the presentation it is leaving.
        let shown = if self.presenting {
            progress
        } else {
            1.0 - progress
        };
        SheetFrame {
            slide: if self.reduce { 0.0 } else { 1.0 - shown },
            scrim: shown,
            alpha: shown,
            running,
        }
    }

    /// Whether the transition is still between endpoints.
    pub fn is_running(&self) -> bool {
        self.tween.running()
    }
}

/// One frame of a press: the scale and the `control_active` fill strength.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressFrame {
    pub scale: f32,
    pub fill: f32,
    pub running: bool,
}

/// Row/button press: scale 1.0 → [`PRESS_SCALE`] and the `control_active` fill
/// 0 → 1 over [`FAST`]; on release both spring back over
/// [`SPRING_DURATION`]. Reduce Motion keeps the fill swap and removes the
/// scale (the plan: "fill opacity swap only; no scale").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressMotion {
    tween: Tween,
    reduce: bool,
}

impl PressMotion {
    /// A press at rest.
    pub fn new(reduce: bool) -> Self {
        Self {
            tween: Tween::settled(0.0),
            reduce,
        }
    }

    /// Finger down (`pressed = true`) or up (`false`) at `now`.
    pub fn set_pressed(&mut self, pressed: bool, now: f64) {
        if self.reduce {
            self.tween.snap(if pressed { 1.0 } else { 0.0 });
            return;
        }
        if pressed {
            self.tween.retarget(1.0, now, FAST, Curve::Ease);
        } else {
            self.tween
                .retarget(0.0, now, SPRING_DURATION, Curve::Spring);
        }
    }

    /// The press's value this frame.
    pub fn sample(&mut self, now: f64) -> PressFrame {
        let (amount, running) = self.tween.sample(now);
        if self.reduce {
            return PressFrame {
                scale: 1.0,
                fill: amount,
                running,
            };
        }
        PressFrame {
            scale: 1.0 - (1.0 - PRESS_SCALE) * amount,
            // The fill is a wash: a spring's bounded overshoot must not push
            // the alpha outside 0..=1.
            fill: amount.clamp(0.0, 1.0),
            running,
        }
    }
}

/// One frame of a success badge: its scale and the fading `ok` tint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SuccessFrame {
    pub scale: f32,
    /// The `ok` tint's alpha (0.22 → 0), for the caller to composite.
    pub flash: f32,
    pub running: bool,
}

/// The success/completion moment: the badge scales [`SUCCESS_SCALE_FROM`] → 1
/// over [`BASE`] while the `ok` tint flashes [`SUCCESS_FLASH_ALPHA`] → 0 over
/// [`SLOW`]. One authored moment, not a loop. Reduce Motion swaps the
/// checkmark instantly (scale 1, no flash).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SuccessBadge {
    scale: Tween,
    flash: Tween,
}

impl SuccessBadge {
    /// Starts the badge at `now`.
    pub fn begin(now: f64, reduce: bool) -> Self {
        if reduce {
            return Self {
                scale: Tween::settled(1.0),
                flash: Tween::settled(0.0),
            };
        }
        Self {
            scale: Tween::begin(SUCCESS_SCALE_FROM, 1.0, now, BASE, Curve::Ease),
            flash: Tween::begin(SUCCESS_FLASH_ALPHA, 0.0, now, SLOW, Curve::Ease),
        }
    }

    /// The badge's value this frame (running while either half runs).
    pub fn sample(&mut self, now: f64) -> SuccessFrame {
        let (scale, scale_running) = self.scale.sample(now);
        let (flash, flash_running) = self.flash.sample(now);
        SuccessFrame {
            scale,
            flash,
            running: scale_running || flash_running,
        }
    }
}

/// The rubber-band offset for a finger `overscroll` points past a scroll
/// bound: signed, diminishing, and never wider than [`RUBBER_MAX`].
pub fn rubber_offset(overscroll: f32) -> f32 {
    if overscroll == 0.0 || !overscroll.is_finite() {
        return 0.0;
    }
    let sign = overscroll.signum();
    let distance = overscroll.abs();
    sign * RUBBER_MAX * (1.0 - (-distance / RUBBER_RESISTANCE).exp())
}

/// The scroll rubber-band: while the finger is past a bound the content
/// follows a resisted ≤4 pt offset; on release the offset eases back to 0 over
/// [`BASE`]. Reduce Motion hard-clamps (offset 0, no release animation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RubberBand {
    offset: Tween,
    reduce: bool,
}

impl RubberBand {
    /// A band at rest.
    pub fn new(reduce: bool) -> Self {
        Self {
            offset: Tween::settled(0.0),
            reduce,
        }
    }

    /// While dragging: sets the drawn offset from the finger's overscroll and
    /// returns it. Called every frame the finger moves; no clock runs.
    pub fn drag(&mut self, overscroll: f32) -> f32 {
        let offset = if self.reduce {
            0.0
        } else {
            rubber_offset(overscroll)
        };
        self.offset.snap(offset);
        offset
    }

    /// The finger lifted at `now`: the offset returns to 0 over [`BASE`].
    pub fn release(&mut self, now: f64) {
        if self.reduce {
            self.offset.snap(0.0);
            return;
        }
        self.offset.retarget(0.0, now, BASE, Curve::Ease);
    }

    /// The offset this frame, and whether it is still returning.
    pub fn sample(&mut self, now: f64) -> (f32, bool) {
        self.offset.sample(now)
    }

    /// Whether the release is still returning to 0.
    pub fn is_running(&self) -> bool {
        self.offset.running()
    }
}

/// The frame-level repaint gate for the authored motion.
///
/// A screen aggregates every helper's `running` into one clock and calls
/// [`AnimationClock::request`] at the end of its frame. While at least one
/// helper is between endpoints the clock asks for exactly one repaint; once
/// they have all ended it asks for none, so an idle screen returns to
/// on-demand rendering. There is deliberately no `request_repaint_after`: a
/// timer is how an animation silently becomes an idle loop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnimationClock {
    running: bool,
}

impl AnimationClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one helper's running state.
    pub fn note(&mut self, running: bool) {
        self.running |= running;
    }

    /// Whether any helper is still running this frame.
    pub fn running(&self) -> bool {
        self.running
    }

    /// Requests a repaint **only** while something runs.
    pub fn request(&self, ctx: &egui::Context) {
        if self.running {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Token seconds as the `f64` clock the M20A helpers take.
    fn at(seconds: f32) -> f64 {
        f64::from(seconds)
    }

    fn run_frame(ctx: &egui::Context, time: f64, f: impl Fn(&egui::Context) -> f32) -> f32 {
        let input = egui::RawInput {
            time: Some(time),
            ..Default::default()
        };
        let mut value = f32::NAN;
        let _ = ctx.run(input, |ctx| {
            value = f(ctx);
        });
        value
    }

    #[test]
    fn ease_out_is_the_endpoint_exact_cubic() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        // Clamped outside the range.
        assert_eq!(ease_out(-1.0), 0.0);
        assert_eq!(ease_out(2.0), 1.0);
        // Monotonic, and past halfway at the midpoint (ease-out, not linear).
        assert!(ease_out(0.5) > 0.5);
        let mut last = 0.0;
        for step in 0..=10 {
            let value = ease_out(step as f32 / 10.0);
            assert!(value >= last, "monotonic at {step}");
            last = value;
        }
    }

    #[test]
    fn reduce_motion_zeroes_every_duration() {
        let full = Motion::new(false);
        assert_eq!(full.duration(FAST), FAST);
        assert_eq!(full.duration(BASE), BASE);
        assert_eq!(full.duration(SLOW), SLOW);
        let reduced = Motion::new(true);
        assert_eq!(reduced.duration(FAST), 0.0);
        assert_eq!(reduced.duration(BASE), 0.0);
        assert_eq!(reduced.duration(SLOW), 0.0);
    }

    #[test]
    fn factors_snap_under_reduce_motion() {
        let ctx = egui::Context::default();
        let motion = Motion::new(true);
        let id = Id::new("test-reduce");
        // Off first, so the id is registered…
        assert_eq!(
            run_frame(&ctx, 0.0, |ctx| motion.factor(ctx, id, false, FAST)),
            0.0
        );
        // …then the transition is a snap: one frame, already at the target.
        assert_eq!(
            run_frame(&ctx, 0.01, |ctx| motion.factor(ctx, id, true, FAST)),
            1.0
        );
    }

    #[test]
    fn factors_animate_and_end() {
        let ctx = egui::Context::default();
        let motion = Motion::new(false);
        let id = Id::new("test-animate");
        assert_eq!(
            run_frame(&ctx, 0.0, |ctx| motion.factor(ctx, id, false, FAST)),
            0.0
        );
        // The transition starts at 0 and climbs.
        let start = run_frame(&ctx, 0.001, |ctx| motion.factor(ctx, id, true, FAST));
        assert!(start < 0.5, "the transition starts near 0: {start}");
        let mut previous = start;
        let mut time = 0.001;
        for _ in 0..4 {
            time += 1.0 / 60.0;
            let value = run_frame(&ctx, time, |ctx| motion.factor(ctx, id, true, FAST));
            assert!(value >= previous, "monotonic");
            previous = value;
        }
        // Well past the token: at rest, exactly 1 (the animation ended).
        assert_eq!(
            run_frame(&ctx, 5.0, |ctx| motion.factor(ctx, id, true, FAST)),
            1.0
        );
    }

    #[test]
    fn arrivals_start_hidden_and_ease_in() {
        let ctx = egui::Context::default();
        let motion = Motion::new(false);
        let id = Id::new("test-arrive");
        // First frame: registered at 0 (invisible), no teleport.
        assert_eq!(
            run_frame(&ctx, 0.0, |ctx| motion.arrive(ctx, id, BASE)),
            0.0
        );
        let mut time = 0.0;
        let mut previous = 0.0;
        for _ in 0..3 {
            time += 1.0 / 60.0;
            let value = run_frame(&ctx, time, |ctx| motion.arrive(ctx, id, BASE));
            assert!(value >= previous && value < 1.0, "easing in: {value}");
            previous = value;
        }
        assert_eq!(
            run_frame(&ctx, 5.0, |ctx| motion.arrive(ctx, id, BASE)),
            1.0
        );
        // Reduce Motion: the arrival is a snap to fully visible on frame one.
        let reduced = Motion::new(true);
        let snap_id = Id::new("test-arrive-reduce");
        assert_eq!(
            run_frame(&ctx, 6.0, |ctx| reduced.arrive(ctx, snap_id, BASE)),
            1.0
        );
    }

    #[test]
    fn colour_helpers_mix_endpoints_and_rise_reverses() {
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(255, 128, 64);
        assert_eq!(lerp_color(a, b, 0.0), a);
        assert_eq!(lerp_color(a, b, 1.0), b);
        let mid = lerp_color(a, b, 0.5);
        assert_eq!(mid, Color32::from_rgb(128, 64, 32));
        // A tint overlay at 0 leaves the base alone.
        let base = Color32::from_rgb(20, 30, 40);
        let tint = Color32::from_rgba_unmultiplied(110, 168, 255, 52);
        assert_eq!(tint_over(base, tint, 0.0), base);
        assert_ne!(tint_over(base, tint, 1.0), base);
        // Rise: full distance at 0, zero at 1.
        assert_eq!(rise(0.0, 2.0), 2.0);
        assert_eq!(rise(1.0, 2.0), 0.0);
        assert_eq!(rise(0.5, 2.0), 1.0);
        // Scale/slide interpolation, clamped.
        assert_eq!(scale(0.0, 4.0, 0.0), 4.0);
        assert_eq!(scale(1.0, 4.0, 0.0), 0.0);
        assert_eq!(scale(0.5, 4.0, 0.0), 2.0);
        assert_eq!(scale(2.0, 0.0, 1.0), 1.0);
        assert_eq!(scale(-1.0, 0.0, 1.0), 0.0);
    }

    #[test]
    fn arrival_progress_snaps_under_reduce_motion_and_ends() {
        use std::time::Duration;
        // Reduce Motion (token 0): the line is fully in immediately.
        assert_eq!(arrival_progress(Duration::from_millis(0), 0.0), 1.0);
        assert_eq!(arrival_progress(Duration::from_millis(5), 0.0), 1.0);
        // Normal: 0 at the instant, easing up, exactly 1 at (and past) the token.
        assert_eq!(arrival_progress(Duration::ZERO, BASE), 0.0);
        let mid = arrival_progress(Duration::from_millis(90), BASE);
        assert!(mid > 0.5 && mid < 1.0, "mid-flight: {mid}");
        assert_eq!(arrival_progress(Duration::from_millis(180), BASE), 1.0);
        assert_eq!(arrival_progress(Duration::from_secs(10), BASE), 1.0);
    }

    #[test]
    fn directional_fades_honour_enter_and_exit_durations() {
        let ctx = egui::Context::default();
        let motion = Motion::new(false);
        let id = Id::new("test-fade-direction");
        // Register while off (the frame before the first open).
        assert_eq!(
            run_frame(&ctx, 0.0, |ctx| motion.fade(ctx, id, false, BASE, FAST)),
            0.0
        );
        // Opening animates over the ENTER token: still in flight at 130 ms.
        let opened = run_frame(&ctx, 0.130, |ctx| motion.fade(ctx, id, true, BASE, FAST));
        assert!(
            opened > 0.0 && opened < 1.0,
            "the enter still runs at 130 ms of the base token: {opened}"
        );
        // Settled open.
        assert_eq!(
            run_frame(&ctx, 1.0, |ctx| motion.fade(ctx, id, true, BASE, FAST)),
            1.0
        );
        // Closing runs over the EXIT token: fully gone once 120 ms elapsed.
        let closing = run_frame(&ctx, 1.050, |ctx| motion.fade(ctx, id, false, BASE, FAST));
        assert!(closing > 0.0 && closing < 1.0, "closing: {closing}");
        assert_eq!(
            run_frame(&ctx, 1.180, |ctx| motion.fade(ctx, id, false, BASE, FAST)),
            0.0,
            "the fast exit has ended 130 ms after the flip"
        );
        // Reduce Motion: both directions snap.
        let reduced = Motion::new(true);
        let snap = Id::new("test-fade-reduce");
        assert_eq!(
            run_frame(&ctx, 2.0, |ctx| reduced.fade(ctx, snap, false, BASE, FAST)),
            0.0
        );
        assert_eq!(
            run_frame(&ctx, 2.001, |ctx| reduced.fade(ctx, snap, true, BASE, FAST)),
            1.0
        );
        assert_eq!(
            run_frame(&ctx, 2.002, |ctx| reduced
                .fade(ctx, snap, false, BASE, FAST)),
            0.0
        );
    }

    #[test]
    fn interruptions_retarget_from_the_current_value() {
        // The brief's interruption discipline: a flip mid-flight continues from
        // where the value is — it never queues a second animation and never
        // snaps back to 0 (or 1) before the new direction runs.
        let ctx = egui::Context::default();
        let motion = Motion::new(false);
        let id = Id::new("test-interrupt");
        assert_eq!(
            run_frame(&ctx, 0.0, |ctx| motion.factor(ctx, id, false, SLOW)),
            0.0
        );
        // Climb part-way into the slow entrance…
        let mid = run_frame(&ctx, 0.150, |ctx| motion.factor(ctx, id, true, SLOW));
        assert!(mid > 0.0 && mid < 1.0, "in flight: {mid}");
        // …then flip back: the next frame moves down from the current value —
        // strictly below the mid-flight value, strictly above 0.
        let retargeted = run_frame(&ctx, 0.166, |ctx| motion.factor(ctx, id, false, SLOW));
        assert!(
            retargeted < mid && retargeted > 0.0,
            "retargets from the current value ({mid} -> {retargeted})"
        );
        // And the new direction ends exactly at its target.
        assert_eq!(
            run_frame(&ctx, 5.0, |ctx| motion.factor(ctx, id, false, SLOW)),
            0.0
        );
    }

    // ---- M9d item 1: the pill press spring ---------------------------------

    /// The spring's math: exact endpoints, a real (bounded) overshoot, and a
    /// scale deviation that stays inside the owner's "no bounce" reading
    /// (< 0.1 % of the pill's scale).
    #[test]
    fn spring_ease_has_bounded_overshoot_and_exact_endpoints() {
        assert_eq!(spring_ease(0.0), 0.0);
        assert_eq!(spring_ease(1.0), 1.0);
        assert_eq!(spring_ease(-1.0), 0.0);
        assert_eq!(spring_ease(2.0), 1.0);
        let mut peak: f32 = 0.0;
        let mut trough: f32 = 1.0;
        let mut peak_at = 0.0;
        for step in 0..=1000 {
            let t = step as f32 / 1000.0;
            let value = spring_ease(t);
            assert!(value >= 0.0, "the easing never goes negative (t = {t})");
            if value > peak {
                peak = value;
                peak_at = t;
            }
            trough = trough.min(value);
        }
        assert!(peak > 1.0, "a spring, not an ease: it overshoots ({peak})");
        assert!(
            peak <= 1.0 + SPRING_OVERSHOOT,
            "overshoot ≤ 2 % of the travel: {peak}"
        );
        assert!(
            (0.2..0.4).contains(&peak_at),
            "the overshoot peaks mid-flight, not at the end: {peak_at}"
        );
        assert!(trough >= 0.0);
        // The press travel is 3 % of the scale, so the overshoot past the rest
        // scale is ≤ 0.02 * 0.03 = 0.06 % — invisible as a bounce.
        let travel = 1.0 - SPRING_PRESS_SCALE;
        let scale_peak = 1.0 + (peak - 1.0) * travel;
        assert!(
            scale_peak <= 1.0 + 0.001,
            "the released scale overshoots by < 0.1 %: {scale_peak}"
        );
        // spring_response maps the easing onto the travel exactly.
        assert_eq!(spring_response(0.0, 1.0, SPRING_PRESS_SCALE), 1.0);
        assert_eq!(
            spring_response(1.0, 1.0, SPRING_PRESS_SCALE),
            SPRING_PRESS_SCALE
        );
        assert_eq!(
            spring_response(1.0, SPRING_PRESS_SCALE, 1.0),
            1.0,
            "the release ends exactly at rest"
        );
    }

    /// The stateful spring: press → ≈0.97 (never past the bound), release →
    /// back to 1.0 with the bounded overshoot, both ENDING exactly; a mid-flight
    /// flip continues from the current scale; Reduce Motion is a snap.
    #[test]
    fn the_press_spring_ends_retargets_and_snaps_under_reduce_motion() {
        let ctx = egui::Context::default();
        let motion = Motion::new(false);
        let id = Id::new("test-press-spring");
        let rest = 1.0;
        let pressed = SPRING_PRESS_SCALE;
        // First frame at rest: static (no teleport on the initial frame).
        assert_eq!(
            run_frame(&ctx, 0.0, |ctx| motion.press_spring(ctx, id, rest)),
            rest
        );
        // Press: the flip frame holds the current scale, then the spring moves —
        // in flight on the following frame, strictly between the two scales.
        assert_eq!(
            run_frame(&ctx, 0.01, |ctx| motion.press_spring(ctx, id, pressed)),
            rest
        );
        let first = run_frame(&ctx, 0.03, |ctx| motion.press_spring(ctx, id, pressed));
        assert!(first < rest && first > pressed, "in flight: {first}");
        // Sample the whole press: never past the pressed scale by more than the
        // bounded overshoot, and the deepest point is within a hair of 0.97.
        let mut deepest = rest;
        let mut time = 0.03;
        for _ in 0..30 {
            time += 0.008;
            deepest = deepest.min(run_frame(&ctx, time, |ctx| {
                motion.press_spring(ctx, id, pressed)
            }));
        }
        let bound = pressed - SPRING_OVERSHOOT * (rest - pressed);
        assert!(
            deepest >= bound,
            "the press overshoot is bounded: {deepest}"
        );
        assert!(
            (deepest - pressed).abs() < 0.002,
            "the press lands at ≈0.97: {deepest}"
        );
        // The press ENDS exactly at the pressed scale.
        assert_eq!(
            run_frame(&ctx, 5.0, |ctx| motion.press_spring(ctx, id, pressed)),
            pressed
        );
        // Settle back to rest, then press again and release **mid-flight**: the
        // flip frame holds the current scale — no snap back, no queue.
        assert_eq!(
            run_frame(&ctx, 5.30, |ctx| motion.press_spring(ctx, id, rest)),
            pressed,
            "the release starts from the pressed scale"
        );
        assert_eq!(
            run_frame(&ctx, 5.60, |ctx| motion.press_spring(ctx, id, rest)),
            rest,
            "the release ENDS exactly at rest"
        );
        assert_eq!(
            run_frame(&ctx, 5.70, |ctx| motion.press_spring(ctx, id, pressed)),
            rest,
            "the second press starts from rest"
        );
        let mid = run_frame(&ctx, 5.74, |ctx| motion.press_spring(ctx, id, pressed));
        assert!(mid < rest && mid > pressed, "in flight: {mid}");
        let released = run_frame(&ctx, 5.75, |ctx| motion.press_spring(ctx, id, rest));
        assert_eq!(
            released, mid,
            "the flip frame holds the current scale (no snap back)"
        );
        let moving = run_frame(&ctx, 5.77, |ctx| motion.press_spring(ctx, id, rest));
        assert!(
            moving > released && moving < rest,
            "the release continues from the current value ({released} -> {moving})"
        );
        // The release's overshoot past rest is the spring's, bounded, and it
        // ENDS exactly at rest.
        let mut peak = rest;
        let mut time = 5.77;
        for _ in 0..40 {
            time += 0.006;
            peak = peak.max(run_frame(&ctx, time, |ctx| {
                motion.press_spring(ctx, id, rest)
            }));
        }
        assert!(peak > rest, "the release springs past the rest: {peak}");
        assert!(
            peak <= rest + SPRING_OVERSHOOT * (rest - pressed) + 1e-6,
            "the release overshoot is ≤ 2 % of the travel: {peak}"
        );
        assert_eq!(
            run_frame(&ctx, 9.0, |ctx| motion.press_spring(ctx, id, rest)),
            rest
        );
        // Reduce Motion: no spring — the press and the release snap.
        let reduced = Motion::new(true);
        let snap = Id::new("test-press-spring-reduce");
        assert_eq!(
            run_frame(&ctx, 10.0, |ctx| reduced.press_spring(ctx, snap, rest)),
            rest
        );
        assert_eq!(
            run_frame(&ctx, 10.01, |ctx| reduced.press_spring(ctx, snap, pressed)),
            pressed
        );
        assert_eq!(
            run_frame(&ctx, 10.02, |ctx| reduced.press_spring(ctx, snap, rest)),
            rest
        );
    }

    // ---- M15.6: the karaoke highlight spring -------------------------------

    /// The current-word highlight: the state's value is always exactly 0 or 1,
    /// the arrival springs (bounded overshoot) and ENDS, and Reduce Motion
    /// keeps the state while removing only the spring.
    #[test]
    fn the_karaoke_highlight_springs_to_its_state_and_snaps_under_reduce_motion() {
        let ctx = egui::Context::default();
        let motion = Motion::new(false);
        let id = Id::new("test-highlight-spring");

        // At rest, a word that is not current has no highlight.
        assert_eq!(
            run_frame(&ctx, 0.0, |ctx| motion.highlight_spring(ctx, id, false)),
            0.0
        );
        // Becoming current: the flip frame holds, then the ramp springs past 1
        // by no more than the shared overshoot bound.
        assert_eq!(
            run_frame(&ctx, 1.0, |ctx| motion.highlight_spring(ctx, id, true)),
            0.0,
            "the flip frame starts from the value it had"
        );
        let mut peak = 0.0_f32;
        let mut time = 1.0;
        for _ in 0..40 {
            time += 0.006;
            peak = peak.max(run_frame(&ctx, time, |ctx| {
                motion.highlight_spring(ctx, id, true)
            }));
        }
        assert!(peak > 1.0, "the highlight springs in: {peak}");
        assert!(
            peak <= 1.0 + SPRING_OVERSHOOT + 1e-6,
            "bounded by the shared overshoot: {peak}"
        );
        // …and ends exactly on the state.
        assert_eq!(
            run_frame(&ctx, 2.0, |ctx| motion.highlight_spring(ctx, id, true)),
            1.0
        );
        // Leaving the word ramps back down to exactly 0.
        assert_eq!(
            run_frame(&ctx, 3.0, |ctx| motion.highlight_spring(ctx, id, false)),
            1.0
        );
        assert_eq!(
            run_frame(&ctx, 3.4, |ctx| motion.highlight_spring(ctx, id, false)),
            0.0
        );

        // Reduce Motion: the highlight is still there, immediately — the state
        // is kept, only the spring is removed.
        let reduced = Motion::new(true);
        let snap = Id::new("test-highlight-spring-reduce");
        assert_eq!(
            run_frame(&ctx, 4.0, |ctx| reduced.highlight_spring(ctx, snap, true)),
            1.0
        );
        assert_eq!(
            run_frame(&ctx, 4.01, |ctx| reduced.highlight_spring(ctx, snap, false)),
            0.0
        );
    }

    // ---- M9d item 3: the seek ease -----------------------------------------

    /// The drawn playhead: static on the first frame, eased on a jump, exact
    /// on playback advance, ending at the target; instant under Reduce Motion.
    #[test]
    fn seek_ease_ends_follows_playback_and_snaps_under_reduce_motion() {
        let ctx = egui::Context::default();
        let motion = Motion::new(false);
        let id = Id::new("test-seek-ease");
        // The first frame renders the data statically (no startup animation).
        assert_eq!(
            run_frame(&ctx, 0.0, |ctx| motion.seek_ease(ctx, id, 0.0, BASE)),
            0.0
        );
        // A jump (a click-to-seek): the flip frame holds the drawn value, then
        // the playhead eases — strictly between the old and the new position.
        assert_eq!(
            run_frame(&ctx, 0.01, |ctx| motion.seek_ease(ctx, id, 9_000.0, BASE)),
            0.0,
            "the seek frame holds the current drawn position"
        );
        let first = run_frame(&ctx, 0.03, |ctx| motion.seek_ease(ctx, id, 9_000.0, BASE));
        assert!(first > 0.0 && first < 9_000.0, "easing in: {first}");
        let mid = run_frame(&ctx, 0.09, |ctx| motion.seek_ease(ctx, id, 9_000.0, BASE));
        assert!(mid > first && mid < 9_000.0, "easing on: {mid}");
        // …and it ENDS exactly at the target within the token.
        assert_eq!(
            run_frame(&ctx, 0.20, |ctx| motion.seek_ease(ctx, id, 9_000.0, BASE)),
            9_000.0
        );
        // Playback advance (a frame's worth, below the jump threshold) is
        // followed exactly — the playing playhead never lags the data.
        assert_eq!(
            run_frame(&ctx, 0.24, |ctx| motion.seek_ease(ctx, id, 9_033.0, BASE)),
            9_033.0
        );
        assert_eq!(
            run_frame(&ctx, 0.28, |ctx| motion.seek_ease(ctx, id, 9_066.0, BASE)),
            9_066.0
        );
        // A new jump mid-flight retargets from the current drawn value (no
        // snap back to the previous position).
        assert_eq!(
            run_frame(&ctx, 0.30, |ctx| motion.seek_ease(ctx, id, 1_000.0, BASE)),
            9_066.0,
            "the second seek frame holds the current drawn position"
        );
        let in_flight = run_frame(&ctx, 0.34, |ctx| motion.seek_ease(ctx, id, 1_000.0, BASE));
        assert!(
            in_flight > 1_000.0 && in_flight < 9_066.0,
            "retargeted mid-flight: {in_flight}"
        );
        assert_eq!(
            run_frame(&ctx, 0.60, |ctx| motion.seek_ease(ctx, id, 1_000.0, BASE)),
            1_000.0
        );
        // Reduce Motion: every seek is an instant data jump.
        let reduced = Motion::new(true);
        let snap = Id::new("test-seek-ease-reduce");
        assert_eq!(
            run_frame(&ctx, 1.0, |ctx| reduced.seek_ease(ctx, snap, 0.0, BASE)),
            0.0
        );
        assert_eq!(
            run_frame(&ctx, 1.01, |ctx| reduced
                .seek_ease(ctx, snap, 9_000.0, BASE)),
            9_000.0
        );
        assert_eq!(
            run_frame(&ctx, 1.02, |ctx| reduced
                .seek_ease(ctx, snap, 1_000.0, BASE)),
            1_000.0
        );
    }

    // ---- M20A: the screen-facing helpers -----------------------------------

    /// A push travels in from +24 and ends exactly at rest; a pop mirrors it.
    #[test]
    fn a_screen_push_and_pop_travel_then_end() {
        let mut push = ScreenMotion::push(0.0, false);
        let start = push.sample(0.0);
        assert_eq!(start.incoming_dx, SCREEN_TRAVEL);
        assert_eq!(start.incoming_alpha, 0.0);
        assert_eq!(start.outgoing_dx, 0.0);
        assert_eq!(start.outgoing_alpha, 1.0);
        assert!(start.running);

        let mid = push.sample(at(SLOW / 2.0));
        assert!(mid.incoming_dx > 0.0 && mid.incoming_dx < SCREEN_TRAVEL);
        assert!(mid.incoming_alpha > 0.0 && mid.incoming_alpha < 1.0);
        assert!(mid.outgoing_dx < 0.0 && mid.outgoing_dx > -SCREEN_EXIT_TRAVEL);
        assert!(mid.running);

        let end = push.sample(at(SLOW));
        assert_eq!(end.incoming_dx, 0.0);
        assert_eq!(end.incoming_alpha, 1.0);
        assert_eq!(end.outgoing_dx, -SCREEN_EXIT_TRAVEL);
        assert_eq!(end.outgoing_alpha, 0.0);
        assert!(!end.running);
        assert!(!push.is_running());

        let mut pop = ScreenMotion::pop(0.0, false);
        assert_eq!(pop.sample(0.0).incoming_dx, -SCREEN_TRAVEL);
        let pop_end = pop.sample(at(SLOW));
        assert_eq!(pop_end.incoming_dx, 0.0);
        assert_eq!(pop_end.outgoing_dx, SCREEN_EXIT_TRAVEL);
        assert!(!pop_end.running);
    }

    /// A sheet slides from fully off to fully presented and ends exactly; a
    /// dismiss runs the other way from the presented state.
    #[test]
    fn a_sheet_slides_and_scrims_then_ends() {
        let mut present = SheetMotion::present(0.0, false);
        let start = present.sample(0.0);
        assert_eq!(start.slide, 1.0);
        assert_eq!(start.scrim, 0.0);
        assert_eq!(start.alpha, 0.0);
        assert!(start.running);

        let mid = present.sample(at(SLOW / 2.0));
        assert!(mid.slide > 0.0 && mid.slide < 1.0);
        assert!(mid.scrim > 0.0 && mid.scrim < 1.0);

        let end = present.sample(at(SLOW));
        assert_eq!(end.slide, 0.0);
        assert_eq!(end.scrim, 1.0);
        assert_eq!(end.alpha, 1.0);
        assert!(!end.running);

        let mut dismiss = SheetMotion::dismiss(1.0, false);
        let held = dismiss.sample(1.0);
        assert_eq!(held.slide, 0.0, "still presented on the dismiss frame");
        assert_eq!(held.scrim, 1.0);
        let gone = dismiss.sample(1.0 + at(SLOW));
        assert_eq!(gone.slide, 1.0);
        assert_eq!(gone.scrim, 0.0);
        assert_eq!(gone.alpha, 0.0);
        assert!(!gone.running);
    }

    /// The press: FAST in to 0.97 with the fill at 1, spring out to rest, both
    /// ending exactly; the flip frame holds its value (no snap back).
    #[test]
    fn the_press_scales_and_fills_then_ends() {
        let mut press = PressMotion::new(false);
        let rest = press.sample(0.0);
        assert_eq!(rest.scale, 1.0);
        assert_eq!(rest.fill, 0.0);
        assert!(!rest.running, "a press at rest asks for nothing");

        press.set_pressed(true, 0.0);
        assert_eq!(press.sample(0.0).scale, 1.0, "the press frame holds");
        let mid = press.sample(at(FAST / 2.0));
        assert!(mid.scale < 1.0 && mid.scale > PRESS_SCALE);
        assert!(mid.fill > 0.0 && mid.fill < 1.0);
        assert!(mid.running);
        let held = press.sample(at(FAST));
        assert_eq!(held.scale, PRESS_SCALE);
        assert_eq!(held.fill, 1.0);
        assert!(!held.running);

        // Release: the flip frame holds the pressed scale, then the spring
        // returns — bounded overshoot, exact rest at the end.
        press.set_pressed(false, at(FAST));
        assert_eq!(
            press.sample(at(FAST)).scale,
            PRESS_SCALE,
            "the release frame holds the current scale"
        );
        let mut peak: f32 = 1.0;
        let mut time = FAST;
        for _ in 0..40 {
            time += 0.006;
            peak = peak.max(press.sample(at(time)).scale);
        }
        assert!(peak > 1.0, "the release springs past rest: {peak}");
        assert!(
            peak <= 1.0 + SPRING_OVERSHOOT * (1.0 - PRESS_SCALE) + 1e-6,
            "the overshoot is bounded: {peak}"
        );
        let settled = press.sample(at(FAST + SPRING_DURATION));
        assert_eq!(settled.scale, 1.0);
        assert_eq!(settled.fill, 0.0);
        assert!(!settled.running);
    }

    /// The success badge: the scale lands on BASE, the tint flash outlives it
    /// on SLOW, and both end exactly.
    #[test]
    fn the_success_badge_flashes_then_ends() {
        let mut badge = SuccessBadge::begin(0.0, false);
        let start = badge.sample(0.0);
        assert_eq!(start.scale, SUCCESS_SCALE_FROM);
        assert_eq!(start.flash, SUCCESS_FLASH_ALPHA);
        assert!(start.running);

        let at_base = badge.sample(at(BASE));
        assert_eq!(at_base.scale, 1.0, "the badge lands on the base token");
        assert!(at_base.flash > 0.0 && at_base.flash < SUCCESS_FLASH_ALPHA);
        assert!(at_base.running, "the flash is still fading");

        let end = badge.sample(at(SLOW));
        assert_eq!(end.scale, 1.0);
        assert_eq!(end.flash, 0.0);
        assert!(!end.running);
    }

    /// The rubber-band: signed, diminishing, bounded by 4 pt, and back to 0
    /// within the base token after release.
    #[test]
    fn the_rubber_band_resists_and_returns() {
        assert_eq!(rubber_offset(0.0), 0.0);
        assert!(rubber_offset(8.0) > 0.0 && rubber_offset(8.0) < RUBBER_MAX);
        assert!(rubber_offset(40.0) > rubber_offset(8.0));
        assert!(rubber_offset(40.0) < RUBBER_MAX);
        assert!(rubber_offset(-40.0) < 0.0);
        assert!((rubber_offset(-40.0) + rubber_offset(40.0)).abs() < 1e-6);
        // Diminishing: doubling the pull less than doubles the offset.
        assert!(rubber_offset(16.0) < 2.0 * rubber_offset(8.0));
        assert_eq!(rubber_offset(f32::INFINITY), 0.0);

        let mut band = RubberBand::new(false);
        let drawn = band.drag(40.0);
        assert_eq!(drawn, rubber_offset(40.0));
        let (still, running) = band.sample(0.0);
        assert_eq!(still, drawn);
        assert!(!running, "a drag is frame-following, not a clock");

        band.release(1.0);
        let (start, running) = band.sample(1.0);
        assert_eq!(start, drawn, "the release starts where the finger left it");
        assert!(running);
        let (mid, _) = band.sample(1.0 + at(BASE / 2.0));
        assert!(mid > 0.0 && mid < drawn);
        let (end, running) = band.sample(1.0 + at(BASE));
        assert_eq!(end, 0.0);
        assert!(!running);
        assert!(!band.is_running());
    }

    /// Reduce Motion's per-interaction substitution, helper by helper.
    #[test]
    fn reduce_motion_substitutes_each_helper() {
        // Screen: FAST crossfade, no travel.
        let mut screen = ScreenMotion::push(0.0, true);
        let start = screen.sample(0.0);
        assert_eq!(start.incoming_dx, 0.0);
        assert_eq!(start.outgoing_dx, 0.0);
        assert_eq!(start.incoming_alpha, 0.0);
        let mid = screen.sample(at(FAST / 2.0));
        assert_eq!((mid.incoming_dx, mid.outgoing_dx), (0.0, 0.0));
        assert!(mid.incoming_alpha > 0.0 && mid.incoming_alpha < 1.0);
        let end = screen.sample(at(FAST));
        assert_eq!(end.incoming_alpha, 1.0);
        assert_eq!(end.outgoing_alpha, 0.0);
        assert!(!end.running, "the reduced crossfade ends at the fast token");

        // Sheet: opacity crossfade, no slide.
        let mut sheet = SheetMotion::present(0.0, true);
        for time in [0.0_f32, FAST / 2.0, FAST] {
            assert_eq!(sheet.sample(at(time)).slide, 0.0);
        }
        assert_eq!(sheet.sample(at(FAST)).alpha, 1.0);

        // Press: fill swaps, the scale never moves.
        let mut press = PressMotion::new(true);
        press.set_pressed(true, 0.0);
        let down = press.sample(0.0);
        assert_eq!(down.scale, 1.0);
        assert_eq!(down.fill, 1.0);
        assert!(!down.running, "the reduced press is a swap, not a clock");
        press.set_pressed(false, 0.01);
        let up = press.sample(0.01);
        assert_eq!(up.scale, 1.0);
        assert_eq!(up.fill, 0.0);

        // Success: instant checkmark, no flash.
        let mut badge = SuccessBadge::begin(0.0, true);
        let instant = badge.sample(0.0);
        assert_eq!(instant.scale, 1.0);
        assert_eq!(instant.flash, 0.0);
        assert!(!instant.running);

        // Rubber-band: hard clamp at the bound.
        let mut band = RubberBand::new(true);
        assert_eq!(band.drag(80.0), 0.0);
        band.release(0.0);
        assert_eq!(band.sample(0.0), (0.0, false));
        assert!(!band.is_running());
    }

    /// THE repaint rule: while a helper runs the clock asks for a frame; once
    /// every helper has ended (t ≥ 1) it asks for none — measured on a real
    /// egui context, not inferred from the state machine.
    #[test]
    fn no_repaint_is_requested_once_every_helper_has_ended() {
        let ctx = egui::Context::default();
        let mut screen = ScreenMotion::push(0.0, false);
        let mut sheet = SheetMotion::present(0.0, false);
        let mut press = PressMotion::new(false);
        press.set_pressed(true, 0.0);
        let mut badge = SuccessBadge::begin(0.0, false);
        let mut band = RubberBand::new(false);
        band.drag(40.0);
        band.release(0.0);

        let mut asked = Vec::new();
        for frame in 0..4 {
            let time = 0.05 + frame as f64 * 0.02;
            let mut clock = AnimationClock::new();
            let _ = ctx.run(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    clock.note(screen.sample(time).running);
                    clock.note(sheet.sample(time).running);
                    clock.note(press.sample(time).running);
                    clock.note(badge.sample(time).running);
                    clock.note(band.sample(time).1);
                    clock.request(ctx);
                    asked.push(ctx.has_requested_repaint());
                },
            );
        }
        assert!(
            asked.iter().all(|asked| *asked),
            "mid-flight frames must request a repaint: {asked:?}"
        );

        // The longest token is SLOW (240 ms); from t ≥ 1 everything is done.
        // The first pass still carries the mid-flight pass's outstanding
        // request (egui counts a request until the frame that consumes it), so
        // the rule is measured from the frame after it — three steady passes.
        let mut late = Vec::new();
        for frame in 0..4 {
            let time = 1.0 + frame as f64 * 0.1;
            let mut clock = AnimationClock::new();
            let _ = ctx.run(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    clock.note(screen.sample(time).running);
                    clock.note(sheet.sample(time).running);
                    clock.note(press.sample(time).running);
                    clock.note(badge.sample(time).running);
                    clock.note(band.sample(time).1);
                    assert!(!clock.running(), "every helper has ended at t = {time}");
                    clock.request(ctx);
                    late.push(ctx.has_requested_repaint());
                },
            );
        }
        assert_eq!(
            &late[1..],
            &[false, false, false],
            "an ended animation must not keep the UI awake: {late:?}"
        );
    }
}
