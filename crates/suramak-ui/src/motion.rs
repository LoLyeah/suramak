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

/// The one easing: cubic ease-out (`emath::easing::cubic_out`).
pub fn ease_out(t: f32) -> f32 {
    egui::emath::easing::cubic_out(t.clamp(0.0, 1.0))
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
}
