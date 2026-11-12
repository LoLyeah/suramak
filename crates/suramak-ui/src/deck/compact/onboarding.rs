//! The one-time intro sheet (M13g3, M20C material pass): the three ways a
//! recording enters Suramak, in one skippable sheet.
//!
//! The copy is the product's own path, not a tour: **share a voice note**,
//! **open a file**, **record in-app**. Each row is a painted line pair inside
//! the shared Sheet plane; the sheet slides up over the scrim on present (the
//! plan's `translateY(100% → 0)`, the Deck's spring on the intro) and leaves on
//! the fast ease-out. Reduce Motion removes the travel only — the fade and the
//! sheet's state stay.
//!
//! M20C: the body is the [`chrome::sheet`] Sheet plane (frost, rim, grain, the
//! one float shadow) and the rows wear the type scale (Newsreader display hero,
//! row titles, caption bodies). The copy is unchanged.
//!
//! The sheet emits one of two choices; the dispatcher turns either into
//! [`CompactAction::OnboardingSeen`] so the host persists the additive
//! `first_run_seen` flag. There is deliberately no "next" pager: three rows
//! fit a phone and a pager would be a second state machine for one screen.

use egui::{pos2, vec2, Align2, Rect, Stroke, Ui, WidgetType};

use crate::deck::motion::Motion;
use crate::deck::theme::Tokens;
use crate::deck::type_scale;

use super::a11y;
use super::chrome::{self, GUTTER};

/// The sheet's choice, if the frame produced one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingChoice {
    None,
    Start,
    Skip,
}

/// One row of the intro: the number, the spoken title, the body line.
struct Path {
    number: &'static str,
    title: &'static str,
    body: &'static str,
    a11y: &'static str,
}

const PATHS: [Path; 3] = [
    Path {
        number: "1",
        title: "Share a voice note",
        body: "From WhatsApp or a recorder, choose Share to Suramak. It transcribes here, on the phone.",
        a11y: "Share a voice note into Suramak from another app",
    },
    Path {
        number: "2",
        title: "Open a file",
        body: "Tap Import to pick an audio or video file. Suramak copies it, so the transcript survives the original moving.",
        a11y: "Open an audio or video file with Import",
    },
    Path {
        number: "3",
        title: "Record in-app",
        body: "Tap Record. It keeps recording with the screen off, then lands in this library.",
        a11y: "Record from the microphone in the app",
    },
];

/// Draws the intro body into the shared sheet rect. Returns the tapped choice.
pub fn sheet(ui: &mut Ui, tokens: &Tokens, motion: &Motion, body: Rect) -> OnboardingChoice {
    let mut choice = OnboardingChoice::None;
    let display = type_scale::editorial_display();
    let caption = type_scale::caption();
    ui.painter().text(
        pos2(body.center().x, body.top() + 34.0),
        Align2::CENTER_CENTER,
        "Welcome to Suramak",
        display.font,
        tokens.ink,
    );
    ui.painter().text(
        pos2(body.center().x, body.top() + 58.0),
        Align2::CENTER_CENTER,
        "Three ways in. Everything stays on this phone.",
        caption.font.clone(),
        tokens.ink_soft,
    );
    a11y::describe(
        ui,
        Rect::from_min_size(
            pos2(body.left() + GUTTER, body.top() + 18.0),
            vec2(body.width() - GUTTER * 2.0, 50.0),
        ),
        "Welcome to Suramak. Three ways in. Everything stays on this phone.",
    );

    let width = body.width() - GUTTER * 2.0;
    let mut y = body.top() + 84.0;
    for path in PATHS {
        let row = Rect::from_min_size(pos2(body.left() + GUTTER, y), vec2(width, 76.0));
        let number = Rect::from_min_size(pos2(row.left() + 4.0, row.top() + 4.0), vec2(30.0, 30.0));
        ui.painter().circle(
            number.center(),
            15.0,
            tokens.accent_tint,
            Stroke::new(1.0_f32, tokens.accent.gamma_multiply(0.55)),
        );
        ui.painter().text(
            number.center(),
            Align2::CENTER_CENTER,
            path.number,
            type_scale::row_value().font,
            tokens.accent,
        );
        ui.painter().text(
            pos2(row.left() + 44.0, row.top() + 12.0),
            Align2::LEFT_CENTER,
            path.title,
            type_scale::row_title().font,
            tokens.ink,
        );
        let mut job = egui::text::LayoutJob::default();
        job.append(path.body, 0.0, caption.format(tokens.ink_soft));
        job.wrap.max_width = width - 44.0;
        job.wrap.max_rows = 2;
        job.wrap.break_anywhere = false;
        job.wrap.overflow_character = Some('…');
        ui.painter().galley(
            pos2(row.left() + 44.0, row.top() + 30.0),
            ui.painter().layout_job(job),
            tokens.ink_soft,
        );
        a11y::describe(ui, row, path.a11y);
        y += 78.0;
    }

    // The two ways out. Skip is a quiet text button on the right of the intro's
    // heading band; Start is the primary action at the sheet's foot.
    let skip = Rect::from_min_size(
        pos2(body.right() - GUTTER - 72.0, body.top() + 14.0),
        vec2(72.0, 44.0),
    );
    let skip_response = a11y::touch(
        ui,
        tokens,
        skip,
        "onboarding-skip",
        WidgetType::Button,
        "Skip the introduction",
        true,
    );
    ui.painter().text(
        skip.center(),
        Align2::CENTER_CENTER,
        "Skip",
        type_scale::row_value().font,
        tokens.ink_soft,
    );
    if skip_response.clicked() {
        choice = OnboardingChoice::Skip;
    }

    let start = Rect::from_min_size(
        pos2(body.left() + GUTTER, body.bottom() - 72.0),
        vec2(width, 52.0),
    );
    if chrome::action_button(ui, tokens, motion, start, "onboarding-start", "Start", true).clicked()
    {
        choice = OnboardingChoice::Start;
    }
    a11y::record_button(ui, start, "Start using Suramak", true);

    // The thin rule above the action separates the paths from the way out.
    ui.painter().line_segment(
        [
            pos2(body.left() + GUTTER, start.top() - 14.0),
            pos2(body.right() - GUTTER, start.top() - 14.0),
        ],
        Stroke::new(1.0_f32, tokens.hairline),
    );
    choice
}
