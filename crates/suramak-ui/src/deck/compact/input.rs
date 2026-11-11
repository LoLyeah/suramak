//! Touch → pointer synthesis, the phone's touch-tuned click window, and the
//! input log the M19A emulator step reads (M19A).
//!
//! **Why this exists.** `android-activity` delivers `MotionEvent`s; winit's
//! Android backend turns them into `WindowEvent::Touch` only, and `egui-winit`
//! 0.33 translates the *primary* touch into `Event::PointerMoved` /
//! `Event::PointerButton` in its own `on_touch` (via `on_cursor_moved` +
//! `on_mouse_button_input`). That translation is what makes the phone tappable
//! today — the owner reached Settings, so *something* delivered a click — but
//! it is one crate's private choice: a different integration, a `winit` that
//! stops forwarding `WindowEvent::Touch`, or a future `egui-winit` leaves the
//! product with no pointer at all. It also never synthesises the *release* for
//! a cancelled touch, which is how a system back-swipe strands egui with the
//! primary button down and the last row highlighted.
//!
//! So the shell owns the contract, in front of egui, and stands down whenever
//! the integration already did the work:
//!
//! * the first finger becomes the pointer: `PointerMoved` then
//!   `PointerButton { Primary, pressed: true }`;
//! * every later finger is **left alone** as a raw [`egui::Event::Touch`] — no
//!   second pointer, no pinch-to-click;
//! * the end releases and then sends `PointerGone`, so egui does not keep a
//!   hover highlight on the last row the finger touched;
//! * a cancel parks the pointer far off-screen **before** releasing, so a
//!   back-swipe or a notification shade can never click whatever sat under the
//!   finger when the system took the gesture away;
//! * **idempotence:** if the frame's event list already contains a
//!   `PointerButton`, the integration translated this phase and the
//!   synthesiser appends nothing for it.
//!
//! [`apply_touch_options`] is the second half of M19A's thumb work: egui's
//! defaults (`max_click_dist` 6 pt, `max_click_duration` 0.25 s) are tuned for
//! a mouse that does not roll 10 pt between press and release on a 450 ppi
//! panel.

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, TouchId, TouchPhase};

/// Where the pointer is parked before a cancelled touch is released.
///
/// Far outside any viewport, so the release cannot land on a control. egui
/// keeps the position (a `PointerButton` sets `latest_pos`), which is exactly
/// why the release must not be sent at the finger's last position.
pub const OFF_SCREEN: Pos2 = Pos2::new(-10_000.0, -10_000.0);

/// How far a finger may roll between press and release and still count as a tap.
pub const TOUCH_MAX_CLICK_DIST: f32 = 12.0;

/// How long a touch may be held and still count as a tap.
///
/// Longer than this is a drag (or a press-and-hold), which is what the scroll
/// areas need to claim the gesture from the row underneath it.
pub const TOUCH_MAX_CLICK_DURATION: f64 = 0.5;

/// The frame's raw event kinds are logged when this is on.
///
/// `SURAMAK_INPUT_LOG=1`/`0` overrides; the default is on in debug builds —
/// the emulator smoke installs exactly that APK (`scripts/android/build-apk.sh`
/// builds the x86_64 artifact with `profile.dev`), so the harness gets the
/// evidence without a second switch, and a release build never pays for it.
pub fn input_log_enabled() -> bool {
    match std::env::var("SURAMAK_INPUT_LOG").ok().as_deref() {
        Some("1") | Some("true") => true,
        Some("0") | Some("false") => false,
        _ => cfg!(debug_assertions),
    }
}

/// The touch→pointer state machine. One per surface; it tracks the finger that
/// owns the pointer across frames.
#[derive(Debug, Default)]
pub struct TouchPointer {
    active: Option<TouchId>,
}

impl TouchPointer {
    pub fn new() -> Self {
        Self { active: None }
    }

    /// The finger that currently owns the pointer, if any.
    pub fn active(&self) -> Option<TouchId> {
        self.active
    }

    /// Forgets the active finger without emitting anything.
    ///
    /// Called when the Activity resumes: a touch that was live when the process
    /// was backgrounded has no `End`/`Cancel` left to deliver, and holding the
    /// old id would make every later `Start` look like a second finger.
    pub fn reset(&mut self) {
        self.active = None;
    }

    /// Folds this frame's touch events into pointer events. Returns how many
    /// events were appended (0 when the integration already did the work).
    pub fn prepare(&mut self, raw: &mut RawInput) -> usize {
        self.prepare_events(&mut raw.events)
    }

    /// [`Self::prepare`] on a bare event list (the unit tests drive this).
    pub fn prepare_events(&mut self, events: &mut Vec<Event>) -> usize {
        let touches: Vec<(TouchId, TouchPhase, Pos2)> = events
            .iter()
            .filter_map(|event| match event {
                Event::Touch { id, phase, pos, .. } => Some((*id, *phase, *pos)),
                _ => None,
            })
            .collect();
        if touches.is_empty() {
            return 0;
        }

        // The idempotence guard: an integration that already translated this
        // phase leaves a `PointerButton` in the frame. Stand down for the whole
        // phase rather than delivering a second press/release.
        let integration_translated = events
            .iter()
            .any(|event| matches!(event, Event::PointerButton { .. }));

        let mut appended = Vec::new();
        for (id, phase, pos) in touches {
            match phase {
                TouchPhase::Start => {
                    // Only the first finger owns the pointer; a second one keeps
                    // its raw `Event::Touch` and nothing else.
                    if self.active.is_some() {
                        continue;
                    }
                    self.active = Some(id);
                    if integration_translated {
                        continue;
                    }
                    appended.push(Event::PointerMoved(pos));
                    appended.push(Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::NONE,
                    });
                }
                TouchPhase::Move => {
                    if self.active != Some(id) {
                        continue;
                    }
                    // A duplicate move at the same position would only repeat
                    // the integration's event; skip it instead of double-
                    // counting motion in egui's velocity tracker.
                    let already_moved = events.iter().any(
                        |event| matches!(event, Event::PointerMoved(at) if at.distance(pos) < 0.5),
                    );
                    if already_moved {
                        continue;
                    }
                    appended.push(Event::PointerMoved(pos));
                }
                TouchPhase::End => {
                    if self.active != Some(id) {
                        continue;
                    }
                    self.active = None;
                    if integration_translated {
                        continue;
                    }
                    appended.push(Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: false,
                        modifiers: Modifiers::NONE,
                    });
                    // Without this egui keeps the last row hovered: a touch
                    // screen has no cursor to move away.
                    appended.push(Event::PointerGone);
                }
                TouchPhase::Cancel => {
                    if self.active != Some(id) {
                        continue;
                    }
                    self.active = None;
                    // Never release on top of a button: park the pointer first.
                    // (egui-winit's own emulation releases nothing on a cancel,
                    // so this path is also what clears a stranded button.)
                    appended.push(Event::PointerMoved(OFF_SCREEN));
                    appended.push(Event::PointerButton {
                        pos: OFF_SCREEN,
                        button: PointerButton::Primary,
                        pressed: false,
                        modifiers: Modifiers::NONE,
                    });
                    appended.push(Event::PointerGone);
                }
            }
        }

        let count = appended.len();
        events.extend(appended);
        count
    }
}

/// Gives the phone a thumb-sized click window.
///
/// Applied by the Android shell only: the desktop keeps egui's mouse defaults,
/// and the phone-viewport suite applies these explicitly when it asserts the
/// wobble/drag behaviour.
pub fn apply_touch_options(ctx: &egui::Context) {
    ctx.options_mut(|options| {
        options.input_options.max_click_dist = TOUCH_MAX_CLICK_DIST;
        options.input_options.max_click_duration = TOUCH_MAX_CLICK_DURATION;
    });
}

/// Whether `apply_touch_options` has already landed on this context.
pub fn touch_options_applied(ctx: &egui::Context) -> bool {
    ctx.options(|options| {
        options.input_options.max_click_dist == TOUCH_MAX_CLICK_DIST
            && options.input_options.max_click_duration == TOUCH_MAX_CLICK_DURATION
    })
}

/// One line per frame, machine-readable: what the integration delivered.
///
/// The emulator smoke greps these to answer M19A's Step 0:
/// `ptrbutton=1` means pointer events already reach egui, and `screen=` (in
/// points) against `wm size` (in pixels) says whether the coordinates are
/// already divided by the device's pixels-per-point.
pub fn describe_frame(raw: &RawInput, pixels_per_point: f32) -> String {
    let screen = match raw.screen_rect {
        Some(rect) => format!("{:.1}x{:.1}", rect.width(), rect.height()),
        None => "none".to_string(),
    };
    let mut touches = String::new();
    let (mut moved, mut buttons, mut gone) = (0, 0, 0);
    for event in &raw.events {
        match event {
            Event::Touch { id, phase, pos, .. } => {
                if !touches.is_empty() {
                    touches.push(';');
                }
                touches.push_str(&format!(
                    "{}:{}@{:.1},{:.1}",
                    id.0,
                    match phase {
                        TouchPhase::Start => "start",
                        TouchPhase::Move => "move",
                        TouchPhase::End => "end",
                        TouchPhase::Cancel => "cancel",
                    },
                    pos.x,
                    pos.y
                ));
            }
            Event::PointerMoved(_) => moved += 1,
            Event::PointerButton { .. } => buttons += 1,
            Event::PointerGone => gone += 1,
            _ => {}
        }
    }
    format!(
        "ppp={pixels_per_point:.3} screen={screen} n={} touch=[{}] ptrmoved={moved} ptrbutton={buttons} ptrgone={gone}",
        raw.events.len(),
        touches
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Rect, Sense, Ui, Vec2};

    fn touch(id: u64, phase: TouchPhase, pos: Pos2) -> Event {
        Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: TouchId::from(id),
            phase,
            pos,
            force: None,
        }
    }

    fn pointer_events(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::PointerMoved(pos) => Some(format!("move@{:.0},{:.0}", pos.x, pos.y)),
                Event::PointerButton {
                    pos,
                    pressed,
                    button,
                    ..
                } => Some(format!(
                    "{}@{:.0},{:.0}",
                    if *pressed {
                        match button {
                            PointerButton::Primary => "press",
                            _ => "press-other",
                        }
                    } else {
                        "release"
                    },
                    pos.x,
                    pos.y
                )),
                Event::PointerGone => Some("gone".to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_tap_becomes_move_press_release_gone() {
        let mut pointer = TouchPointer::new();
        let mut start = vec![touch(7, TouchPhase::Start, Pos2::new(100.0, 200.0))];
        assert_eq!(pointer.prepare_events(&mut start), 2);
        assert_eq!(pointer.active(), Some(TouchId::from(7)));
        assert_eq!(
            pointer_events(&start),
            vec!["move@100,200", "press@100,200"],
            "a start is a move then the press"
        );

        let mut end = vec![touch(7, TouchPhase::End, Pos2::new(101.0, 200.5))];
        assert_eq!(pointer.prepare_events(&mut end), 2);
        assert_eq!(pointer.active(), None);
        assert_eq!(
            pointer_events(&end),
            vec!["release@101,200", "gone"],
            "the release carries the lift position, then the pointer vanishes"
        );
    }

    #[test]
    fn a_second_finger_is_never_synthesised() {
        let mut pointer = TouchPointer::new();
        let mut first = vec![touch(1, TouchPhase::Start, Pos2::new(40.0, 40.0))];
        pointer.prepare_events(&mut first);

        let mut second = vec![
            touch(1, TouchPhase::Move, Pos2::new(41.0, 40.0)),
            touch(2, TouchPhase::Start, Pos2::new(300.0, 500.0)),
            touch(2, TouchPhase::Move, Pos2::new(310.0, 510.0)),
        ];
        pointer.prepare_events(&mut second);
        assert_eq!(
            pointer_events(&second),
            vec!["move@41,40"],
            "only the first finger's move is translated: {:?}",
            pointer_events(&second)
        );
        assert_eq!(pointer.active(), Some(TouchId::from(1)));

        // The second finger lifting changes nothing about the pointer.
        let mut lift = vec![touch(2, TouchPhase::End, Pos2::new(310.0, 510.0))];
        assert_eq!(pointer.prepare_events(&mut lift), 0);
        assert_eq!(pointer.active(), Some(TouchId::from(1)));
    }

    #[test]
    fn an_integration_that_already_delivers_pointer_events_is_not_doubled() {
        let mut pointer = TouchPointer::new();
        // egui-winit 0.33's own translation of a touch start: PointerMoved +
        // PointerButton, plus the raw touch it also forwards.
        let mut frame = vec![
            touch(3, TouchPhase::Start, Pos2::new(10.0, 20.0)),
            Event::PointerMoved(Pos2::new(10.0, 20.0)),
            Event::PointerButton {
                pos: Pos2::new(10.0, 20.0),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ];
        assert_eq!(pointer.prepare_events(&mut frame), 0);
        assert_eq!(pointer_events(&frame).len(), 2, "no second press");
        assert_eq!(
            pointer.active(),
            Some(TouchId::from(3)),
            "the finger is still tracked, so its end/cancel stay ours"
        );

        let mut end = vec![
            touch(3, TouchPhase::End, Pos2::new(10.0, 20.0)),
            Event::PointerButton {
                pos: Pos2::new(10.0, 20.0),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
            Event::PointerGone,
        ];
        assert_eq!(pointer.prepare_events(&mut end), 0, "the release is theirs");
        assert_eq!(pointer.active(), None);
    }

    #[test]
    fn a_cancel_parks_the_pointer_before_releasing() {
        let mut pointer = TouchPointer::new();
        let mut start = vec![touch(9, TouchPhase::Start, Pos2::new(200.0, 700.0))];
        pointer.prepare_events(&mut start);

        let mut cancel = vec![touch(9, TouchPhase::Cancel, Pos2::new(200.0, 700.0))];
        assert_eq!(pointer.prepare_events(&mut cancel), 3);
        assert_eq!(
            pointer_events(&cancel),
            vec!["move@-10000,-10000", "release@-10000,-10000", "gone"],
            "the release must not land on the row the finger was over"
        );
        assert_eq!(pointer.active(), None);
    }

    #[test]
    fn a_duplicate_move_is_not_sent_twice() {
        let mut pointer = TouchPointer::new();
        let mut start = vec![touch(4, TouchPhase::Start, Pos2::new(5.0, 5.0))];
        pointer.prepare_events(&mut start);
        let mut frame = vec![
            touch(4, TouchPhase::Move, Pos2::new(5.0, 5.0)),
            Event::PointerMoved(Pos2::new(5.0, 5.0)),
        ];
        assert_eq!(pointer.prepare_events(&mut frame), 0);
        let mut moved = vec![touch(4, TouchPhase::Move, Pos2::new(6.0, 5.0))];
        assert_eq!(pointer.prepare_events(&mut moved), 1);
    }

    #[test]
    fn reset_drops_a_finger_that_died_with_the_activity() {
        let mut pointer = TouchPointer::new();
        let mut start = vec![touch(1, TouchPhase::Start, Pos2::new(1.0, 1.0))];
        pointer.prepare_events(&mut start);
        pointer.reset();
        assert_eq!(pointer.active(), None);
        let mut next = vec![touch(2, TouchPhase::Start, Pos2::new(2.0, 2.0))];
        assert_eq!(pointer.prepare_events(&mut next), 2, "a fresh finger works");
    }

    /// A real egui frame: does a synthesised tap actually click a widget, and
    /// does a cancelled one leave it alone?
    fn run_frame(ctx: &Context, events: Vec<Event>, time: f64, rect: Rect) -> bool {
        let mut clicked = false;
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 800.0))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui: &mut Ui| {
                let response = ui.interact(rect, ui.id().with("target"), Sense::click());
                clicked = response.clicked();
            });
        });
        clicked
    }

    #[test]
    fn a_synthesised_tap_clicks_and_a_cancel_does_not() {
        let rect = Rect::from_min_size(Pos2::new(50.0, 100.0), Vec2::new(300.0, 60.0));
        let ctx = Context::default();
        // egui hit-tests a frame's pointer against the PREVIOUS frame's widget
        // rects, so the widget must have been drawn once before the press —
        // which on a phone it always has (the row is on screen when the finger
        // lands on it).
        assert!(!run_frame(&ctx, Vec::new(), 0.9, rect));
        let mut pointer = TouchPointer::new();

        let mut start = vec![touch(1, TouchPhase::Start, rect.center())];
        pointer.prepare_events(&mut start);
        assert!(!run_frame(&ctx, start, 1.0, rect), "a press is not a click");
        let mut end = vec![touch(1, TouchPhase::End, rect.center())];
        pointer.prepare_events(&mut end);
        assert!(
            run_frame(&ctx, end, 1.1, rect),
            "press+release inside the widget is a click"
        );

        // Same gesture, but the system cancels it: no click, ever.
        let ctx = Context::default();
        assert!(!run_frame(&ctx, Vec::new(), 1.9, rect));
        let mut pointer = TouchPointer::new();
        let mut start = vec![touch(1, TouchPhase::Start, rect.center())];
        pointer.prepare_events(&mut start);
        run_frame(&ctx, start, 2.0, rect);
        let mut cancel = vec![touch(1, TouchPhase::Cancel, rect.center())];
        pointer.prepare_events(&mut cancel);
        assert!(
            !run_frame(&ctx, cancel, 2.1, rect),
            "a cancelled touch must never click the row under the finger"
        );
    }

    #[test]
    fn the_touch_options_are_thumb_sized() {
        let ctx = Context::default();
        assert!(!touch_options_applied(&ctx));
        apply_touch_options(&ctx);
        assert!(touch_options_applied(&ctx));
        ctx.options(|options| {
            assert_eq!(options.input_options.max_click_dist, 12.0);
            assert_eq!(options.input_options.max_click_duration, 0.5);
        });
    }

    #[test]
    fn the_input_log_line_names_the_event_kinds() {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(467.7, 1015.4))),
            events: vec![
                touch(0, TouchPhase::Start, Pos2::new(120.3, 400.1)),
                Event::PointerMoved(Pos2::new(120.3, 400.1)),
                Event::PointerButton {
                    pos: Pos2::new(120.3, 400.1),
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
            ..Default::default()
        };
        let line = describe_frame(&raw, 2.6);
        assert!(line.contains("ppp=2.600"), "{line}");
        assert!(line.contains("screen=467.7x1015.4"), "{line}");
        assert!(line.contains("touch=[0:start@120.3,400.1]"), "{line}");
        assert!(line.contains("ptrmoved=1 ptrbutton=1 ptrgone=0"), "{line}");
    }
}
