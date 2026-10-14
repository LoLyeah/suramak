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

#[cfg(test)]
mod tests {
    use super::*;

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
}
