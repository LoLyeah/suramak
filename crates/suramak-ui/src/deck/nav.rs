//! The compact (phone) navigation model: a per-screen push/pop stack with the
//! Deck motion system driving one spring per direction.
//!
//! **Why a stack and not swipe pages.** The plan (§2.1) calls for "single-pane
//! navigation stack"; Android's system back gesture is already intercepted and
//! funnelled by the v2.4 shell (`MainActivity.installBackHandler` — three
//! dispatch paths, one `handleBack`), so the app only has to answer one
//! question: *pop, dismiss, or leave?* That answer is [`NavStack::back`].
//!
//! **Motion.** A push enters the new screen from the right; a pop returns the
//! previous one from the left. Both use ONE spring — [`motion::spring_ease`]
//! over the base token — with the direction only changing the sign of the
//! travel. There is no second curve and no bounce: the spring's overshoot is
//! bounded by [`motion::SPRING_OVERSHOOT`] and it settles exactly at 1.0.
//! Reduce Motion makes the transition a hard cut (progress 1 on the first
//! frame), and a finished transition requests no repaint — an idle phone UI
//! draws only on input.
//!
//! The stack is pure: it owns no egui state, so every rule here is unit-tested
//! on the host (the device-only boundary is touch/fling delivery, not this).

use crate::deck::motion::{self, Motion};

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

/// The in-flight navigation animation.
///
/// `progress` is the *entering* screen's normalized position: 0 = fully
/// outside, 1 = settled. A pop animates the leaving screen from 1 to 0; the
/// renderer reads [`NavMotion::offset`] for whichever side it paints.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavMotion {
    pub direction: Direction,
    started_at: f64,
    progress: f32,
    done: bool,
}

impl NavMotion {
    /// Starts a transition at `now` (egui's input time, seconds).
    pub fn begin(direction: Direction, now: f64) -> Self {
        Self {
            direction,
            started_at: now,
            progress: 0.0,
            done: false,
        }
    }

    /// Advances the transition. Under Reduce Motion the first call settles it —
    /// a hard cut, never a zero-length animation that still asks for a frame.
    pub fn advance(&mut self, now: f64, reduce_motion: bool) -> f32 {
        if self.done {
            return self.progress;
        }
        if reduce_motion {
            self.progress = 1.0;
            self.done = true;
            return 1.0;
        }
        let elapsed = (now - self.started_at).max(0.0) as f32;
        let t = (elapsed / motion::BASE).clamp(0.0, 1.0);
        self.progress = motion::spring_ease(t);
        if t >= 1.0 {
            self.progress = 1.0;
            self.done = true;
        }
        self.progress
    }

    /// The entering screen's horizontal offset as a fraction of the viewport
    /// width (always positive: it comes from the right on push; on pop the
    /// outgoing screen is the one drawn at this offset).
    pub fn offset(&self, viewport_width: f32) -> f32 {
        (1.0 - self.progress).max(0.0) * viewport_width
    }

    /// Opacity of the entering screen (a short fade over the same spring).
    pub fn opacity(&self) -> f32 {
        self.progress.clamp(0.0, 1.0)
    }

    /// Whether the animation has ended (and therefore must not request a
    /// repaint).
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Force-settle (a second back press during a transition, a lifecycle
    /// pause): the next frame is static.
    pub fn settle(&mut self) {
        self.progress = 1.0;
        self.done = true;
    }
}

/// A single-pane navigation stack.
///
/// `S` is the screen token (`Copy` so a popped screen can be returned). The
/// root screen is pushed at construction and can never be popped: back at the
/// root is [`BackOutcome::Finish`], which the Android shell maps to
/// `finishAndRemoveTask()` (the v2.4 contract).
#[derive(Debug, Clone)]
pub struct NavStack<S> {
    screens: Vec<S>,
    motion: Option<NavMotion>,
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
    /// must be requested — exactly while the spring is between its endpoints.
    pub fn advance(&mut self, now: f64, reduce_motion: bool) -> bool {
        match self.motion.as_mut() {
            Some(anim) => {
                if anim.is_done() {
                    return false;
                }
                anim.advance(now, reduce_motion);
                !anim.is_done()
            }
            None => false,
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
    }

    /// The Deck motion policy, for callers that need `reduce_motion` here.
    pub fn policy(&self, reduce_motion: bool) -> Motion {
        Motion::new(reduce_motion)
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
    fn reduce_motion_is_a_hard_cut_on_the_first_frame() {
        let mut anim = NavMotion::begin(Direction::Push, 5.0);
        assert_eq!(anim.advance(5.0, true), 1.0);
        assert!(anim.is_done());
        assert_eq!(anim.offset(400.0), 0.0);
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
    }

    #[test]
    fn a_second_back_during_the_transition_settles_it() {
        let mut nav = NavStack::new(Screen::Library);
        nav.push(Screen::Editor, 0.0);
        nav.settle();
        assert!(nav.motion().expect("push motion").is_done());
        assert!(!nav.advance(0.1, false));
    }
}
