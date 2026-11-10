//! The compact (phone) navigation model: a per-screen push/pop stack with the
//! Deck motion system driving one spring per direction.
//!
//! **Why a stack and not swipe pages.** The plan (§2.1) calls for "single-pane
//! navigation stack"; Android's system back gesture is already intercepted and
//! funnelled by the v2.4 shell (`MainActivity.installBackHandler` — three
//! dispatch paths, one `handleBack`), so the app only has to answer one
//! question: *pop, dismiss, or leave?* That answer is [`NavStack::back`].
//!
//! **Motion (M13g2).** A navigation move paints **two layers**: the screen
//! leaving and the screen arriving. A push enters from the right while the
//! previous screen drifts left by 18 % of the travel and dims; a pop sends the
//! outgoing screen right while the revealed one drifts in from 18 % left. Both
//! directions use ONE spring — [`motion::spring_ease`] over the base token — so
//! the two cannot drift apart. Overshoot is bounded by
//! [`motion::SPRING_OVERSHOOT`] (2 %) and the value lands exactly at 1.0.
//!
//! **Reduce Motion removes only the springs.** The travel above is settled on
//! the first frame and no overshoot is ever drawn, but the opacity crossfade
//! still runs over [`REDUCE_FADE`] (120 ms). Opacity is not vestibular motion;
//! cutting straight to the next screen is what makes a reduced-motion user
//! lose their place. There is no translation, scale or bounce in that path.
//!
//! A finished transition requests no repaint (`advance()` returns false once
//! the token elapsed) — an idle phone UI draws only on input.
//!
//! The stack is pure: it owns no egui state, so every rule here is unit-tested
//! on the host (the device-only boundary is touch/fling delivery, not this).

use crate::deck::motion::{self, Motion};

/// The crossfade Reduce Motion keeps (120 ms — the fast token). See the module
/// docs: only a spring's *travel* is removed.
pub const REDUCE_FADE: f32 = motion::FAST;

/// The outgoing screen's parallax as a fraction of the viewport width. Small
/// on purpose: the arriving screen is the subject; the old one only has to
/// move enough to read as "behind".
pub const PARALLAX: f32 = 0.18;

/// Which way a navigation move goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// A new screen enters from the right.
    Push,
    /// The current screen leaves to the right, revealing the previous one.
    Pop,
}

/// What the platform back gesture should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackOutcome {
    /// A screen was popped; the UI stays in the app.
    Popped,
    /// A modal/sheet was dismissed; the screen stays.
    Dismissed,
    /// The stack is at its root: the host finishes the Activity.
    Finish,
}

/// One painted layer's transform for the current frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layer {
    /// Horizontal offset in points (positive moves right).
    pub dx: f32,
    /// Opacity, 0..=1.
    pub alpha: f32,
}

/// The two layers a transition paints: the screen leaving (if any) and the one
/// arriving.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavLayers {
    pub outgoing: Option<Layer>,
    pub incoming: Layer,
}

/// The in-flight navigation animation.
///
/// `travel` is the **spring's** normalized position (0 = fully outside, 1 =
/// settled); `fade` is the opacity crossfade. They share a clock in the normal
/// path and diverge only under Reduce Motion (travel snaps, fade still runs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavMotion {
    pub direction: Direction,
    started_at: f64,
    travel: f32,
    fade: f32,
    done: bool,
}

impl NavMotion {
    /// Starts a transition at `now` (egui's input time, seconds).
    pub fn begin(direction: Direction, now: f64) -> Self {
        Self {
            direction,
            started_at: now,
            travel: 0.0,
            fade: 0.0,
            done: false,
        }
    }

    /// Advances the transition and returns the travel progress (kept for the
    /// callers that only need the spring). Under Reduce Motion the travel is
    /// settled on the first call; [`NavMotion::is_done`] stays false until the
    /// crossfade has run.
    pub fn advance(&mut self, now: f64, reduce_motion: bool) -> f32 {
        if self.done {
            return self.travel;
        }
        motion::reanchor(&mut self.started_at, now);
        let elapsed = (now - self.started_at).max(0.0) as f32;
        if reduce_motion {
            self.travel = 1.0;
            self.fade = motion::ease_out((elapsed / REDUCE_FADE).clamp(0.0, 1.0));
            if elapsed >= REDUCE_FADE {
                self.fade = 1.0;
                self.done = true;
            }
            return self.travel;
        }
        let t = (elapsed / motion::BASE).clamp(0.0, 1.0);
        self.travel = motion::spring_ease(t);
        self.fade = motion::ease_out(t);
        if t >= 1.0 {
            self.travel = 1.0;
            self.fade = 1.0;
            self.done = true;
        }
        self.travel
    }

    /// The spring's progress (1 under Reduce Motion from the first frame).
    pub fn travel(&self) -> f32 {
        self.travel
    }

    /// The opacity crossfade (0..=1).
    pub fn fade(&self) -> f32 {
        self.fade
    }

    /// The entering screen's horizontal offset as a fraction of the viewport
    /// width (always positive: it comes from the right on push; on pop the
    /// outgoing screen is the one drawn at this offset).
    pub fn offset(&self, viewport_width: f32) -> f32 {
        (1.0 - self.travel).max(0.0) * viewport_width
    }

    /// Opacity of the entering screen (a short fade over the same spring).
    pub fn opacity(&self) -> f32 {
        self.fade.clamp(0.0, 1.0)
    }

    /// The two layers for this frame. Callers paint `outgoing` first.
    pub fn layers(&self, viewport_width: f32) -> NavLayers {
        let travel = self.travel.clamp(0.0, 1.0);
        let fade = self.fade.clamp(0.0, 1.0);
        let behind = PARALLAX * (1.0 - travel) * viewport_width;
        match self.direction {
            Direction::Push => NavLayers {
                outgoing: Some(Layer {
                    dx: -behind,
                    alpha: 1.0 - 0.30 * fade,
                }),
                incoming: Layer {
                    dx: (1.0 - travel) * viewport_width,
                    alpha: fade,
                },
            },
            Direction::Pop => NavLayers {
                outgoing: Some(Layer {
                    dx: travel * viewport_width,
                    alpha: 1.0 - fade,
                }),
                incoming: Layer {
                    dx: -behind,
                    alpha: 0.55 + 0.45 * fade,
                },
            },
        }
    }

    /// Whether the animation has ended (and therefore must not request a
    /// repaint).
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Force-settle (a second back press during a transition, a lifecycle
    /// pause): the next frame is static.
    pub fn settle(&mut self) {
        self.travel = 1.0;
        self.fade = 1.0;
        self.done = true;
    }
}

/// A single-pane navigation stack.
///
/// `S` is the screen token (`Copy` so a popped screen can be returned). The
/// root screen is pushed at construction and can never be popped: back at the
/// root is [`BackOutcome::Finish`], which the Android shell maps to
/// `finishAndRemoveTask()` (the v2.4 contract).
///
/// A pop keeps its **outgoing** screen until the transition settles so the
/// renderer can slide it away; [`NavStack::advance`] drops it at the end.
#[derive(Debug, Clone)]
pub struct NavStack<S> {
    screens: Vec<S>,
    motion: Option<NavMotion>,
    outgoing: Option<S>,
    /// Direction of the most recent move, so the renderer can tell which side
    /// is entering without re-deriving it from the animation.
    pub last_direction: Direction,
}

impl<S: Copy + PartialEq> NavStack<S> {
    /// A stack with one root screen, rendered statically on the first frame
    /// (no arrival animation — the brief's "the initial frame is static").
    pub fn new(root: S) -> Self {
        Self {
            screens: vec![root],
            motion: None,
            outgoing: None,
            last_direction: Direction::Push,
        }
    }

    /// The screen currently on top.
    pub fn current(&self) -> S {
        *self.screens.last().expect("a nav stack always has a root")
    }

    /// The screen below the top, when there is one.
    pub fn previous(&self) -> Option<S> {
        let len = self.screens.len();
        (len >= 2).then(|| self.screens[len - 2])
    }

    /// The screen leaving a pop, while its transition runs.
    pub fn outgoing(&self) -> Option<S> {
        self.outgoing
    }

    /// Number of screens on the stack (1 = root).
    pub fn depth(&self) -> usize {
        self.screens.len()
    }

    /// The whole stack, bottom → top (the Android host mirrors it for
    /// diagnostics and process-death restore).
    pub fn screens(&self) -> &[S] {
        &self.screens
    }

    /// Pushes a screen. A push onto the same screen is a no-op and returns
    /// `false` (so a double tap cannot deepen the stack).
    pub fn push(&mut self, screen: S, now: f64) -> bool {
        if self.current() == screen {
            return false;
        }
        self.screens.push(screen);
        self.outgoing = None;
        self.last_direction = Direction::Push;
        self.motion = Some(NavMotion::begin(Direction::Push, now));
        true
    }

    /// Replaces the whole stack with `root` + `screen` — the deep-link shape
    /// (notification tap → Library, then the finished transcript's editor).
    /// Returns `false` when `screen` is already current on that root.
    pub fn open_deep(&mut self, root: S, screen: S, now: f64) -> bool {
        if self.current() == screen && self.screens.first() == Some(&root) {
            return false;
        }
        self.screens.clear();
        self.screens.push(root);
        self.screens.push(screen);
        self.outgoing = None;
        self.last_direction = Direction::Push;
        self.motion = Some(NavMotion::begin(Direction::Push, now));
        true
    }

    /// Pops the top screen. The root is never popped: `None` means "nothing
    /// below", and the caller must treat it as a finish request.
    pub fn pop(&mut self, now: f64) -> Option<S> {
        if self.screens.len() <= 1 {
            return None;
        }
        let popped = self.screens.pop();
        self.outgoing = popped;
        self.last_direction = Direction::Pop;
        self.motion = Some(NavMotion::begin(Direction::Pop, now));
        popped
    }

    /// The back gesture's one decision. `modal_open` is the sheet/modal layer
    /// above the stack (export sheet, quit confirm): back dismisses it before
    /// it touches navigation, matching Android's predictive-back convention.
    pub fn back(&mut self, modal_open: bool, now: f64) -> BackOutcome {
        if modal_open {
            return BackOutcome::Dismissed;
        }
        if self.pop(now).is_some() {
            BackOutcome::Popped
        } else {
            BackOutcome::Finish
        }
    }

    /// Advances the in-flight transition (if any). Returns whether a repaint
    /// must be requested — exactly while the spring/crossfade is between its
    /// endpoints. Drops the outgoing screen when the transition ends.
    pub fn advance(&mut self, now: f64, reduce_motion: bool) -> bool {
        let Some(anim) = self.motion.as_mut() else {
            return false;
        };
        if anim.is_done() {
            return false;
        }
        anim.advance(now, reduce_motion);
        if anim.is_done() {
            self.outgoing = None;
            false
        } else {
            true
        }
    }

    /// The in-flight transition, if any.
    pub fn motion(&self) -> Option<&NavMotion> {
        self.motion.as_ref()
    }

    /// Settles any in-flight transition (a lifecycle pause, a second back).
    pub fn settle(&mut self) {
        if let Some(anim) = self.motion.as_mut() {
            anim.settle();
        }
        self.outgoing = None;
    }

    /// The Deck motion policy, for callers that need `reduce_motion` here.
    pub fn policy(&self, reduce_motion: bool) -> Motion {
        Motion::new(reduce_motion)
    }

    /// Which route each painted layer is: `(outgoing, incoming)`. A push's
    /// outgoing is the screen below; a pop's is the popped screen.
    pub fn layer_routes(&self) -> (Option<S>, S) {
        match self.last_direction {
            Direction::Push => (self.previous(), self.current()),
            Direction::Pop => (self.outgoing, self.current()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Screen {
        Library,
        Editor,
        Settings,
    }

    #[test]
    fn the_root_is_static_and_cannot_be_popped() {
        let mut nav = NavStack::new(Screen::Library);
        assert_eq!(nav.depth(), 1);
        assert_eq!(nav.current(), Screen::Library);
        assert!(nav.motion().is_none());
        assert_eq!(nav.back(false, 0.0), BackOutcome::Finish);
        assert_eq!(nav.depth(), 1, "back at the root never empties the stack");
    }

    #[test]
    fn push_then_pop_moves_one_screen_at_a_time() {
        let mut nav = NavStack::new(Screen::Library);
        assert!(nav.push(Screen::Editor, 1.0));
        assert_eq!(nav.depth(), 2);
        assert_eq!(nav.current(), Screen::Editor);
        assert_eq!(nav.previous(), Some(Screen::Library));
        assert_eq!(nav.pop(2.0), Some(Screen::Editor));
        assert_eq!(nav.current(), Screen::Library);
        assert_eq!(nav.depth(), 1);
        assert_eq!(
            nav.outgoing(),
            Some(Screen::Editor),
            "the leaving screen survives for the pop transition"
        );
    }

    #[test]
    fn a_double_push_of_the_same_screen_is_a_no_op() {
        let mut nav = NavStack::new(Screen::Library);
        assert!(nav.push(Screen::Settings, 0.0));
        assert!(!nav.push(Screen::Settings, 0.1));
        assert_eq!(nav.depth(), 2);
    }

    #[test]
    fn back_dismisses_a_modal_before_navigation() {
        let mut nav = NavStack::new(Screen::Library);
        nav.push(Screen::Editor, 0.0);
        assert_eq!(nav.back(true, 0.1), BackOutcome::Dismissed);
        assert_eq!(nav.current(), Screen::Editor, "the sheet, not the screen");
        assert_eq!(nav.back(false, 0.2), BackOutcome::Popped);
    }

    #[test]
    fn a_deep_link_lands_on_library_then_the_transcript() {
        let mut nav = NavStack::new(Screen::Settings);
        assert!(nav.open_deep(Screen::Library, Screen::Editor, 0.0));
        assert_eq!(nav.screens(), &[Screen::Library, Screen::Editor]);
        assert_eq!(nav.depth(), 2);
        // The same tap twice (a second notification for the open transcript)
        // must not grow the stack.
        assert!(!nav.open_deep(Screen::Library, Screen::Editor, 1.0));
        assert_eq!(nav.depth(), 2);
    }

    #[test]
    fn the_push_spring_enters_from_the_right_and_settles_exactly() {
        let mut anim = NavMotion::begin(Direction::Push, 10.0);
        assert_eq!(anim.advance(10.0, false), 0.0);
        let mid = anim.advance(10.0 + motion::BASE as f64 * 0.1, false);
        assert!(mid > 0.0 && mid < 1.0);
        assert!(anim.offset(400.0) > 0.0);
        let end = anim.advance(10.0 + motion::BASE as f64 + 0.001, false);
        assert_eq!(end, 1.0);
        assert_eq!(anim.offset(400.0), 0.0);
        assert!(anim.is_done());
    }

    #[test]
    fn the_spring_overshoot_stays_inside_the_deck_bound() {
        // Sample the whole token at 1 ms: the value never exceeds the authored
        // ceiling, and it ends exactly at 1.
        let mut anim = NavMotion::begin(Direction::Pop, 0.0);
        let mut max = 0.0_f32;
        for ms in 0..=motion::BASE as u32 {
            let value = anim.advance(ms as f64 / 1000.0, false);
            max = max.max(value);
        }
        assert!(max <= 1.0 + motion::SPRING_OVERSHOOT + 1e-6);
        assert_eq!(anim.advance(10.0, false), 1.0);
    }

    #[test]
    fn the_two_layer_offsets_partition_the_viewport_at_the_ends() {
        let mut anim = NavMotion::begin(Direction::Push, 0.0);
        anim.advance(0.0, false);
        let start = anim.layers(400.0);
        assert_eq!(start.incoming.dx, 400.0, "the new screen starts off-screen");
        assert_eq!(start.incoming.alpha, 0.0);
        let old = start.outgoing.expect("the library is painted beneath");
        assert_eq!(old.dx, -PARALLAX * 400.0);

        anim.advance(10.0, false);
        let end = anim.layers(400.0);
        assert_eq!(end.incoming.dx, 0.0);
        assert_eq!(end.incoming.alpha, 1.0);
        assert_eq!(end.outgoing.expect("beneath").dx, 0.0);

        // A pop sends the outgoing screen fully right and leaves the revealed
        // one exactly home.
        let mut pop = NavMotion::begin(Direction::Pop, 0.0);
        pop.advance(10.0, false);
        let settled = pop.layers(400.0);
        assert_eq!(settled.incoming.dx, 0.0);
        assert_eq!(settled.incoming.alpha, 1.0);
        assert_eq!(settled.outgoing.expect("leaving").alpha, 0.0);
    }

    #[test]
    fn reduce_motion_removes_the_travel_but_keeps_the_crossfade() {
        let mut anim = NavMotion::begin(Direction::Push, 5.0);
        // Travel settles on the first frame: no translation, ever.
        assert_eq!(anim.advance(5.0, true), 1.0);
        assert_eq!(anim.offset(400.0), 0.0);
        assert_eq!(anim.layers(400.0).incoming.dx, 0.0);
        // The fade is still climbing, so the frame is not static yet…
        assert!(anim.fade() < 1.0);
        assert!(!anim.is_done());
        let mid = anim.advance(5.0 + REDUCE_FADE as f64 * 0.5, true);
        assert_eq!(mid, 1.0);
        assert!(anim.fade() > 0.0 && anim.fade() < 1.0);
        // …and it ends exactly on the fast token, requesting no more frames.
        anim.advance(5.0 + REDUCE_FADE as f64 + 0.001, true);
        assert_eq!(anim.fade(), 1.0);
        assert!(anim.is_done());
        assert_eq!(anim.layers(400.0).incoming.alpha, 1.0);
    }

    #[test]
    fn a_finished_transition_requests_no_repaint() {
        let mut nav = NavStack::new(Screen::Library);
        nav.push(Screen::Editor, 0.0);
        assert!(nav.advance(0.0, false), "the first frame still animates");
        assert!(
            !nav.advance(1.0, false),
            "after the token the stack asks for no frames"
        );
        assert_eq!(nav.outgoing(), None);
    }

    #[test]
    fn a_finished_pop_drops_the_outgoing_screen() {
        let mut nav = NavStack::new(Screen::Library);
        nav.push(Screen::Editor, 0.0);
        nav.advance(1.0, false);
        nav.pop(2.0);
        assert_eq!(nav.layer_routes(), (Some(Screen::Editor), Screen::Library));
        assert!(nav.advance(2.05, false));
        assert!(!nav.advance(2.5, false));
        assert_eq!(nav.outgoing(), None);
        assert_eq!(nav.layer_routes(), (None, Screen::Library));
    }

    #[test]
    fn a_second_back_during_the_transition_settles_it() {
        let mut nav = NavStack::new(Screen::Library);
        nav.push(Screen::Editor, 0.0);
        nav.settle();
        assert!(nav.motion().expect("push motion").is_done());
        assert!(!nav.advance(0.1, false));
    }

    #[test]
    fn a_host_clock_start_reanchors_on_the_first_ui_frame() {
        // A host command (back/deep link) starts at process uptime; the UI
        // advances with egui time. The mismatch must start the spring, not
        // freeze it at progress 0 forever.
        let mut anim = NavMotion::begin(Direction::Push, 100.0);
        assert_eq!(
            anim.advance(2.0, false),
            0.0,
            "the first UI frame starts it"
        );
        assert!(!anim.is_done());
        let end = anim.advance(2.0 + motion::BASE as f64 + 0.01, false);
        assert_eq!(end, 1.0, "and it completes on the UI clock");
        assert!(anim.is_done());
    }

    #[test]
    fn reduce_motion_still_costs_exactly_one_token_of_frames() {
        let mut nav = NavStack::new(Screen::Library);
        nav.push(Screen::Editor, 0.0);
        assert!(nav.advance(0.0, true), "the crossfade starts");
        assert!(
            !nav.advance(REDUCE_FADE as f64 + 0.01, true),
            "and stops after the fast token — never an endless idle repaint"
        );
    }
}
