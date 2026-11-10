//! Compact-surface motion primitives: the sheet, list insert/delete and the
//! progress fill.
//!
//! These live beside the screens rather than in `deck::motion` because they
//! own **state machines with clocks**, not just easing curves: a sheet can be
//! dismissed mid-present and must retarget, a row list must remember what it
//! painted last frame, and a progress fill must settle exactly at its target.
//! The curves are the Deck's ([`motion::ease_out`], [`motion::spring_ease`]).
//!
//! **Reduce Motion rule (M13g2): the springs are what goes.** The sheet's rise,
//! the list's slide and any overshoot are zero in that mode; opacity
//! crossfades keep their token. A progress fill is a width tween, not a
//! vestibular motion, so it is unchanged.

use crate::deck::motion::{self, ease_out, spring_ease};

/// How far a sheet rises while presenting (points).
pub const SHEET_RISE: f32 = 14.0;
/// How far a new list row slides up while it appears (points).
pub const ROW_RISE: f32 = 6.0;

/// The curve a sheet's present animation rides. G2's export/quit sheets use
/// the ease-out; G3's one-time intro uses the Deck press spring (the
/// workorder's "the sheet rides the Deck spring").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetCurve {
    Ease,
    Spring,
}

/// A sheet's present/dismiss animation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SheetAnim {
    value: f32,
    from: f32,
    target: f32,
    started_at: f64,
    duration: f32,
    done: bool,
    present: bool,
    curve: SheetCurve,
}

impl SheetAnim {
    /// A closed sheet (nothing drawn, nothing animating).
    pub const fn closed() -> Self {
        Self {
            value: 0.0,
            from: 0.0,
            target: 0.0,
            started_at: 0.0,
            duration: 0.0,
            done: true,
            present: false,
            curve: SheetCurve::Ease,
        }
    }

    /// Starts (or retargets) the animation. A flip mid-flight starts from the
    /// **current** value — it never snaps back to 0 and never queues.
    pub fn begin(&mut self, present: bool, now: f64) {
        self.begin_with(present, now, SheetCurve::Ease);
    }

    /// The intro's presentation: the same fade and rise, but the value rides
    /// the Deck's damped spring ([`motion::spring_ease`]) instead of the
    /// ease-out. Dismissal stays the fast ease-out (exits do not bounce).
    pub fn begin_spring(&mut self, present: bool, now: f64) {
        self.begin_with(
            present,
            now,
            if present {
                SheetCurve::Spring
            } else {
                SheetCurve::Ease
            },
        );
    }

    fn begin_with(&mut self, present: bool, now: f64, curve: SheetCurve) {
        self.from = self.value;
        self.target = if present { 1.0 } else { 0.0 };
        self.duration = if !present {
            motion::FAST
        } else if curve == SheetCurve::Spring {
            motion::SPRING_DURATION
        } else {
            motion::BASE
        };
        self.started_at = now;
        self.present = present;
        self.done = false;
        self.curve = curve;
    }

    /// Advances the animation. Returns whether another frame is needed.
    ///
    /// Under Reduce Motion a spring-sheet keeps its **fade** (the base token,
    /// the same one the export sheet uses) and loses the spring's travel
    /// shape — the state is unchanged, only the arrival stops springing.
    pub fn advance(&mut self, now: f64, reduce_motion: bool) -> bool {
        if self.done {
            return false;
        }
        motion::reanchor(&mut self.started_at, now);
        let elapsed = (now - self.started_at).max(0.0) as f32;
        let reduced_spring = reduce_motion && self.curve == SheetCurve::Spring;
        let duration = if reduced_spring {
            motion::BASE
        } else {
            self.duration
        };
        let t = (elapsed / duration.max(1e-6)).clamp(0.0, 1.0);
        let eased = if reduced_spring {
            ease_out(t)
        } else {
            match self.curve {
                SheetCurve::Ease => ease_out(t),
                SheetCurve::Spring => spring_ease(t),
            }
        };
        self.value = self.from + (self.target - self.from) * eased;
        if t >= 1.0 {
            self.value = self.target;
            self.done = true;
        }
        !self.done
    }

    /// The fade factor (0..=1). A spring may overshoot past 1 internally; the
    /// opacity is clamped (an opacity cannot exceed opaque), while [`rise`]
    /// uses the unclamped value so the travel carries the spring.
    pub fn value(&self) -> f32 {
        self.value.clamp(0.0, 1.0)
    }

    /// The unclamped 0..=1+overshoot value (the spring's geometry).
    pub fn raw_value(&self) -> f32 {
        self.value
    }

    /// Whether the sheet must still be painted (the closing fade ends first).
    pub fn visible(&self) -> bool {
        self.value > 0.002 || (!self.done && self.present)
    }

    pub fn is_present(&self) -> bool {
        self.present
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn curve(&self) -> SheetCurve {
        self.curve
    }

    /// The body's rise in points. Zero under Reduce Motion: translation is the
    /// part of a sheet presentation that can unsettle, the fade is not.
    pub fn rise(&self, reduce_motion: bool) -> f32 {
        if reduce_motion {
            0.0
        } else {
            SHEET_RISE * (1.0 - self.value.clamp(0.0, 1.0 + motion::SPRING_OVERSHOOT))
        }
    }

    /// Force-settle (lifecycle pause).
    pub fn settle(&mut self) {
        self.value = self.target;
        self.done = true;
    }
}

/// One row's last-frame state.
#[derive(Debug, Clone, Copy, PartialEq)]
struct RowState {
    started_at: f64,
    appear: f32,
    done: bool,
}

/// A row that is leaving: its slot for the fast token, then gone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ghost {
    pub key: u64,
    /// The index the row occupied in the previous list.
    pub at: usize,
    /// 1 → 0 while the slot collapses.
    pub value: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct GhostState {
    key: u64,
    at: usize,
    started_at: f64,
    value: f32,
    done: bool,
}

/// The list's painted state for one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ListSnapshot {
    /// Live rows, in caller order, with their appear factor.
    pub rows: Vec<(u64, f32)>,
    /// Leaving rows, collapsing at their old index.
    pub ghosts: Vec<Ghost>,
    /// Whether another frame is needed.
    pub animating: bool,
}

/// Insert/delete animation for a keyed list.
///
/// The first `sync` is the **initial frame**: every row is at rest (the brief's
/// "the initial frame is static"). Later syncs animate new keys in over the
/// base token and keep removed keys as ghosts over the fast token.
#[derive(Debug, Clone, Default)]
pub struct ListAnim {
    /// In caller order — the ghost's slot index must be the index the screen
    /// painted last frame, not a key-sorted position.
    rows: Vec<(u64, RowState)>,
    ghosts: Vec<GhostState>,
    initialized: bool,
}

impl ListAnim {
    pub fn new() -> Self {
        Self::default()
    }

    /// Diffs the list against the last frame and returns what to paint.
    /// `now` is egui's input time in seconds; Reduce Motion makes the insert
    /// immediate and drops a removed row at once.
    pub fn sync(&mut self, keys: &[u64], now: f64, reduce_motion: bool) -> ListSnapshot {
        let first = !self.initialized;
        let mut next: Vec<(u64, RowState)> = Vec::with_capacity(keys.len());
        for key in keys {
            let state = match self.rows.iter().find(|(seen, _)| seen == key) {
                Some((_, state)) => *state,
                None => RowState {
                    started_at: now,
                    appear: if first || reduce_motion { 1.0 } else { 0.0 },
                    done: first || reduce_motion,
                },
            };
            next.push((*key, state));
        }
        // Removed keys become ghosts at the index they held last frame.
        let mut removed: Vec<GhostState> = Vec::new();
        for (old_index, (key, _)) in self.rows.iter().enumerate() {
            if !next.iter().any(|(seen, _)| seen == key) && !reduce_motion {
                removed.push(GhostState {
                    key: *key,
                    at: old_index,
                    started_at: now,
                    value: 1.0,
                    done: false,
                });
            }
        }
        // A ghost from the previous frame that is still collapsing stays.
        for ghost in self.ghosts.iter().filter(|ghost| !ghost.done) {
            if !removed.iter().any(|item| item.key == ghost.key) {
                removed.push(*ghost);
            }
        }
        self.rows = next;
        self.ghosts = removed;
        self.initialized = true;
        self.advance(now, reduce_motion)
    }

    /// Advances every live appear factor and ghost; returns the current frame.
    pub fn advance(&mut self, now: f64, reduce_motion: bool) -> ListSnapshot {
        let mut animating = false;
        for (_, state) in self.rows.iter_mut() {
            if state.done {
                continue;
            }
            let elapsed = (now - state.started_at).max(0.0) as f32;
            let t = (elapsed / motion::BASE).clamp(0.0, 1.0);
            state.appear = ease_out(t);
            if t >= 1.0 || reduce_motion {
                state.appear = 1.0;
                state.done = true;
            } else {
                animating = true;
            }
        }
        for ghost in &mut self.ghosts {
            if ghost.done {
                continue;
            }
            let elapsed = (now - ghost.started_at).max(0.0) as f32;
            let t = (elapsed / motion::FAST).clamp(0.0, 1.0);
            ghost.value = 1.0 - ease_out(t);
            if t >= 1.0 || reduce_motion {
                ghost.value = 0.0;
                ghost.done = true;
            } else {
                animating = true;
            }
        }
        self.ghosts.retain(|ghost| !ghost.done);
        ListSnapshot {
            rows: self
                .rows
                .iter()
                .map(|(key, state)| (*key, state.appear))
                .collect(),
            ghosts: self
                .ghosts
                .iter()
                .map(|ghost| Ghost {
                    key: ghost.key,
                    at: ghost.at,
                    value: ghost.value,
                })
                .collect(),
            animating,
        }
    }

    /// Whether a row or ghost is mid-flight.
    pub fn is_animating(&self) -> bool {
        self.rows.iter().any(|(_, state)| !state.done) || !self.ghosts.is_empty()
    }
}

/// A width/percentage fill that eases to its target and settles exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressAnim {
    shown: f32,
    from: f32,
    target: f32,
    started_at: f64,
    duration: f32,
    done: bool,
}

impl ProgressAnim {
    pub const fn new() -> Self {
        Self {
            shown: 0.0,
            from: 0.0,
            target: 0.0,
            started_at: 0.0,
            duration: 0.0,
            done: true,
        }
    }

    /// Sets a new target; a repeated identical target is a no-op (this is what
    /// keeps a job's status line from asking for a frame per poll).
    pub fn set(&mut self, target: f32, now: f64) -> bool {
        let target = target.clamp(0.0, 1.0);
        if (target - self.target).abs() < f32::EPSILON {
            return false;
        }
        self.from = self.shown;
        self.target = target;
        self.started_at = now;
        self.duration = motion::BASE;
        self.done = false;
        true
    }

    pub fn advance(&mut self, now: f64) -> bool {
        if self.done {
            return false;
        }
        motion::reanchor(&mut self.started_at, now);
        let elapsed = (now - self.started_at).max(0.0) as f32;
        let t = (elapsed / self.duration.max(1e-6)).clamp(0.0, 1.0);
        self.shown = self.from + (self.target - self.from) * ease_out(t);
        if t >= 1.0 {
            self.shown = self.target;
            self.done = true;
        }
        !self.done
    }

    pub fn value(&self) -> f32 {
        self.shown.clamp(0.0, 1.0)
    }

    pub fn is_done(&self) -> bool {
        self.done
    }
}

impl Default for ProgressAnim {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sheet_presents_over_base_and_dismisses_over_fast() {
        let mut sheet = SheetAnim::closed();
        assert!(!sheet.visible());
        sheet.begin(true, 0.0);
        assert!(sheet.advance(0.0, false), "the first frame animates");
        let mid = {
            sheet.advance(0.06, false);
            sheet.value()
        };
        assert!(mid > 0.0 && mid < 1.0);
        assert!(!sheet.is_done());
        sheet.advance(0.2, false);
        assert_eq!(sheet.value(), 1.0);
        assert!(sheet.is_done());
        // Dismissal is faster and keeps painting until the fade ends.
        sheet.begin(false, 1.0);
        sheet.advance(1.05, false);
        assert!(sheet.visible(), "a closing sheet still draws");
        sheet.advance(1.2, false);
        assert_eq!(sheet.value(), 0.0);
        assert!(!sheet.visible());
    }

    #[test]
    fn a_sheet_retargets_mid_flight_without_snapping() {
        let mut sheet = SheetAnim::closed();
        sheet.begin(true, 0.0);
        sheet.advance(0.06, false);
        let climbing = sheet.value();
        assert!(climbing > 0.0 && climbing < 1.0);
        sheet.begin(false, 0.07);
        sheet.advance(0.07, false);
        assert!(
            (sheet.value() - climbing).abs() < 1e-6,
            "the dismiss starts from the current value, not 0"
        );
        sheet.advance(0.4, false);
        assert_eq!(sheet.value(), 0.0);
    }

    #[test]
    fn the_onboarding_sheet_rides_the_deck_spring_and_lands_exactly() {
        let mut sheet = SheetAnim::closed();
        sheet.begin_spring(true, 0.0);
        assert_eq!(sheet.curve(), SheetCurve::Spring);
        let mut peak = 0.0_f32;
        let mut time = 0.0;
        for _ in 0..60 {
            time += 0.006;
            sheet.advance(time, false);
            peak = peak.max(sheet.raw_value());
        }
        assert!(
            peak > 1.0,
            "the Deck spring overshoots by design: peak {peak}"
        );
        assert!(
            peak <= 1.0 + motion::SPRING_OVERSHOOT + 1e-6,
            "bounded by the shared overshoot: {peak}"
        );
        sheet.advance(1.0, false);
        assert_eq!(sheet.value(), 1.0);
        assert_eq!(sheet.raw_value(), 1.0);
        assert!(sheet.is_done());

        // Dismissal is the fast ease-out, never a bounce.
        sheet.begin_spring(false, 2.0);
        assert_eq!(sheet.curve(), SheetCurve::Ease);
        sheet.advance(2.2, false);
        assert_eq!(sheet.value(), 0.0);

        // Reduce Motion removes the spring and keeps the fade: the value is the
        // ease-out's, but the body has no rise at all.
        let mut reduced = SheetAnim::closed();
        reduced.begin_spring(true, 0.0);
        reduced.advance(0.06, true);
        assert!(reduced.value() > 0.0 && reduced.value() < 1.0);
        assert_eq!(reduced.rise(true), 0.0);
    }

    #[test]
    fn reduce_motion_keeps_the_sheet_fade_but_removes_the_rise() {
        let mut sheet = SheetAnim::closed();
        sheet.begin(true, 0.0);
        sheet.advance(0.03, true);
        assert!(sheet.value() > 0.0 && sheet.value() < 1.0, "the fade runs");
        assert_eq!(sheet.rise(true), 0.0, "no translation under Reduce Motion");
        let mut normal = SheetAnim::closed();
        normal.begin(true, 0.0);
        normal.advance(0.03, false);
        assert!(normal.rise(false) > 0.0);
    }

    #[test]
    fn a_list_animates_an_insert_and_holds_a_delete_for_the_fast_token() {
        let mut list = ListAnim::new();
        let first = list.sync(&[1, 2], 0.0, false);
        assert_eq!(first.rows, vec![(1, 1.0), (2, 1.0)]);
        assert!(!first.animating, "the initial frame is static");

        // Insert 3 at the end.
        let inserted = list.sync(&[1, 2, 3], 1.0, false);
        let row = inserted
            .rows
            .iter()
            .find(|(key, _)| *key == 3)
            .expect("new row");
        assert_eq!(row.1, 0.0, "the new row starts transparent");
        assert!(inserted.animating);
        let settled = list.advance(1.3, false);
        assert_eq!(
            settled.rows.iter().find(|(key, _)| *key == 3).unwrap().1,
            1.0
        );

        // Delete row 2: a ghost collapses, then the list is still.
        let deleted = list.sync(&[1, 3], 2.0, false);
        assert_eq!(deleted.ghosts.len(), 1);
        assert_eq!(deleted.ghosts[0].at, 1, "the ghost keeps the old slot");
        let gone = list.sync(&[1, 3], 2.2, false);
        assert!(gone.ghosts.is_empty(), "the ghost is gone after the token");
        assert!(!gone.animating);
    }

    #[test]
    fn reduce_motion_makes_a_list_insert_and_delete_immediate() {
        let mut list = ListAnim::new();
        list.sync(&[1], 0.0, true);
        let inserted = list.sync(&[1, 2], 1.0, true);
        assert_eq!(inserted.rows, vec![(1, 1.0), (2, 1.0)]);
        assert!(!inserted.animating);
        let deleted = list.sync(&[2], 2.0, true);
        assert!(deleted.ghosts.is_empty());
    }

    #[test]
    fn host_clock_starts_reanchor_on_the_first_ui_frame() {
        // A host command starts the sheet/progress at process uptime; the UI
        // advances with egui time. Both must run, not freeze at 0 and request
        // frames forever.
        let mut sheet = SheetAnim::closed();
        sheet.begin(true, 100.0);
        sheet.advance(1.0, false);
        assert!(!sheet.is_done(), "the first UI frame starts the present");
        sheet.advance(1.3, false);
        assert_eq!(sheet.value(), 1.0);
        assert!(sheet.is_done());

        let mut progress = ProgressAnim::new();
        progress.set(0.5, 100.0);
        progress.advance(1.0);
        assert!(progress.value() < 0.5);
        progress.advance(1.3);
        assert_eq!(progress.value(), 0.5);
        assert!(progress.is_done());
    }

    #[test]
    fn a_progress_fill_eases_and_settles_exactly() {
        let mut progress = ProgressAnim::new();
        assert!(!progress.set(0.0, 0.0), "the same target asks for no frame");
        assert!(progress.set(0.5, 0.0));
        progress.advance(0.05);
        assert!(progress.value() > 0.0 && progress.value() < 0.5);
        progress.advance(0.5);
        assert_eq!(progress.value(), 0.5);
        assert!(progress.is_done());
        // A poll that repeats the value must not restart the animation.
        assert!(!progress.set(0.5, 0.6));
    }
}
