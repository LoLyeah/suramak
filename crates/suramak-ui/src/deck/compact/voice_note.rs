//! M22B — the 1.2 s frosted voice-note micro-sheet.
//!
//! A shared audio file is a glance, not a form: the sheet names the file, shows
//! the sender only when the intent carried one, offers the ID/EN pill, and
//! leaves. When the language is unambiguous and a verified model exists the
//! host starts the job and the sheet auto-dismisses in
//! [`AUTO_DISMISS_SECONDS`]; otherwise it waits for the one tap it needs.
//!
//! Everything is the shared Deck: [`chrome::micro_sheet`]'s Sheet plane, the
//! M20A press-spring on every body, and the type scale's own tokens. There is
//! no Android view here — the same code draws on the phone and in the
//! phone-viewport test suite.

use egui::{pos2, vec2, Color32, CornerRadius, Rect, Ui};

use crate::deck::motion::Motion;
use crate::deck::theme::Tokens;
use crate::deck::type_scale;

use super::a11y;
use super::chrome;
use super::model::VoiceNoteView;

/// The micro-sheet's designed height. Small enough to read as a confirmation,
/// tall enough for a 48 dp action and its state line.
pub const MICRO_SHEET_HEIGHT: f32 = 236.0;

/// The sheet's auto-dismiss window (the plan's `SLOW` token).
pub const AUTO_DISMISS_SECONDS: f64 = 1.2;

/// One of the two languages the pill offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pill {
    Id,
    En,
}

impl Pill {
    /// The language code the queue wants (`id` / `en`).
    pub fn tag(self) -> &'static str {
        match self {
            Pill::Id => "id",
            Pill::En => "en",
        }
    }

    /// The pill's visible label.
    pub fn label(self) -> &'static str {
        match self {
            Pill::Id => "ID",
            Pill::En => "EN",
        }
    }

    /// Parses an intent's language tag. `id-ID`, `id`, `in` (the legacy
    /// ISO-639 spelling whisper also accepts) and `en-US` all map; anything
    /// else is `None` — never a guess.
    pub fn from_tag(raw: &str) -> Option<Self> {
        let lower = raw.trim().to_ascii_lowercase();
        let base = lower.split(['-', '_']).next().unwrap_or("");
        match base {
            "id" | "in" => Some(Pill::Id),
            "en" => Some(Pill::En),
            _ => None,
        }
    }
}

/// What the sheet's one interaction produced this frame. The chosen pill (or
/// its absence, meaning Auto) lives on [`VoiceNoteView::pill`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceNoteChoice {
    Accept,
    Dismiss,
}

/// Draws the micro-sheet's body. `body` is the Sheet plane's rect returned by
/// [`chrome::micro_sheet`].
pub fn sheet(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    view: &mut VoiceNoteView,
    now: f64,
    body: Rect,
) -> Option<VoiceNoteChoice> {
    let mut choice = None;
    let gutter = chrome::GUTTER;
    let left = body.left() + gutter;
    let right = body.right() - gutter;
    let width = (right - left).max(0.0);

    // The countdown is the sheet's whole life made visible: an accent hairline
    // along the bottom edge that empties over the auto-dismiss window. It sits
    // on the sheet's own edge, clear of the grabber and the action.
    if view.auto_start {
        let remaining =
            ((view.opened_at + AUTO_DISMISS_SECONDS - now) / AUTO_DISMISS_SECONDS).clamp(0.0, 1.0);
        let track = Rect::from_min_size(
            pos2(body.left() + gutter, body.bottom() - 2.0),
            vec2((body.width() - gutter * 2.0).max(0.0), 2.0),
        );
        ui.painter()
            .rect_filled(track, CornerRadius::same(1), tokens.hairline);
        let fill = Rect::from_min_size(track.min, vec2(track.width() * remaining as f32, 2.0));
        ui.painter()
            .rect_filled(fill, CornerRadius::same(1), tokens.accent);
    }

    // Title, then the sender only when the intent named one.
    let title = Rect::from_min_size(pos2(left, body.top() + 24.0), vec2(width - 64.0, 26.0));
    let title_token = type_scale::row_title();
    let title_galley = ui.painter().layout(
        view.title.clone(),
        title_token.font.clone(),
        tokens.ink,
        title.width(),
    );
    let title_height = title_galley.size().y;
    ui.painter()
        .galley(pos2(title.left(), title.top()), title_galley, tokens.ink);

    let mut cursor = body.top() + 24.0 + title_height + 6.0;
    if let Some(sender) = &view.sender {
        ui.painter().text(
            pos2(left, cursor),
            egui::Align2::LEFT_TOP,
            sender,
            type_scale::caption().font,
            tokens.ink_soft,
        );
        cursor += 20.0;
    }

    // Cancel is a text action in the top-right corner (48 dp hit area, no
    // second filled button competing with Transcribe).
    let cancel = Rect::from_min_size(pos2(right - 56.0, body.top() + 16.0), vec2(56.0, 48.0));
    let cancel_hit = a11y::touch(
        ui,
        tokens,
        cancel,
        "voice-note-cancel",
        egui::WidgetType::Button,
        "Dismiss the voice note",
        true,
    );
    ui.painter().text(
        pos2(cancel.center().x, cancel.center().y),
        egui::Align2::CENTER_CENTER,
        "Cancel",
        type_scale::caption().font,
        tokens.ink_soft,
    );
    if cancel_hit.clicked() {
        choice = Some(VoiceNoteChoice::Dismiss);
    }

    // The language pill: two segments, a 48 dp hit each, the M20A spring.
    let row_top = cursor + 8.0;
    ui.painter().text(
        pos2(left, row_top + 12.0),
        egui::Align2::LEFT_TOP,
        "Language",
        type_scale::caption().font,
        tokens.ink_faint,
    );
    let pill = Rect::from_min_size(pos2(right - 132.0, row_top), vec2(132.0, 40.0));
    ui.painter().rect_filled(
        pill,
        CornerRadius::same(tokens.radius.control),
        tokens.control_fill,
    );
    for (slot, language) in [Pill::Id, Pill::En].into_iter().enumerate() {
        let segment = Rect::from_min_size(
            pos2(pill.left() + 4.0 + slot as f32 * 62.0, pill.top() + 4.0),
            vec2(60.0, 32.0),
        );
        let hit = a11y::touch(
            ui,
            tokens,
            Rect::from_center_size(segment.center(), vec2(60.0, 48.0)),
            ("voice-note-language", language.tag()),
            egui::WidgetType::RadioButton,
            &format!(
                "Transcribe as {}",
                if language == Pill::Id {
                    "Indonesian"
                } else {
                    "English"
                }
            ),
            true,
        );
        let press = chrome::press_frame(
            ui.ctx(),
            ui.id().with(("voice-note-pill", language.tag())),
            hit.is_pointer_button_down_on(),
            motion.reduce,
        );
        let visual = Rect::from_center_size(segment.center(), segment.size() * press.scale);
        let selected = view.pill == Some(language);
        let fill = if selected {
            tokens.accent
        } else {
            Color32::TRANSPARENT
        };
        ui.painter()
            .rect_filled(visual, CornerRadius::same(tokens.radius.control), fill);
        ui.painter().text(
            visual.center(),
            egui::Align2::CENTER_CENTER,
            language.label(),
            type_scale::row_value().font,
            if selected {
                tokens.on_accent
            } else {
                tokens.ink_soft
            },
        );
        if hit.clicked() {
            // Tapping the selected pill clears it back to Auto — the third
            // state the two segments need to be able to express.
            view.pill = if selected { None } else { Some(language) };
            view.auto_start = false;
        }
    }

    // The honest state line, then the one primary action.
    let hint = if view.reason.is_empty() {
        "On this phone · nothing is uploaded."
    } else {
        view.reason.as_str()
    };
    let action_top = body.bottom() - 16.0 - 52.0;
    let hint_rect = Rect::from_min_size(pos2(left, action_top - 22.0), vec2(width, 16.0));
    let hint_galley = ui.painter().layout(
        hint.to_owned(),
        type_scale::caption().font,
        tokens.ink_faint,
        hint_rect.width(),
    );
    ui.painter().galley(
        pos2(hint_rect.left(), hint_rect.top()),
        hint_galley,
        tokens.ink_faint,
    );

    let action = Rect::from_min_size(pos2(left, action_top), vec2(width, 52.0));
    let enabled = view.model_ok;
    let response = chrome::accent_button(
        ui,
        tokens,
        motion,
        action,
        "voice-note-transcribe",
        "Transcribe",
        "Transcribe this voice note",
        None,
        enabled,
    );
    if enabled && response.clicked() {
        // An unpicked pill is Auto: the caller maps the view's pill (or its
        // absence) to the job's language. A disabled button (no model) never
        // accepts, even though the painted control still senses the tap.
        choice = Some(VoiceNoteChoice::Accept);
    }
    choice
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::compact::{CompactAction, CompactApp, CompactSettings, Sheet};
    use crate::deck::theme::AppTheme;

    fn app_with(view: VoiceNoteView) -> CompactApp {
        let mut app = CompactApp::new(CompactSettings::default());
        app.theme = AppTheme::Dark;
        app.offer_voice_note(view);
        app
    }

    fn run(ctx: &egui::Context, app: &mut CompactApp, time: f64) -> Vec<CompactAction> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(467.7, 1015.4),
            )),
            time: Some(time),
            ..Default::default()
        };
        let mut actions = Vec::new();
        let _ = ctx.run(raw, |ctx| {
            actions = crate::deck::compact::ui(ctx, app);
        });
        actions
    }

    #[test]
    fn the_sheet_auto_dismisses_at_one_point_two_seconds_with_the_pill() {
        let mut app = app_with(VoiceNoteView {
            title: "Ptt.opus".to_string(),
            sender: Some("Rina".to_string()),
            pill: Some(Pill::En),
            model_ok: true,
            auto_start: true,
            reason: String::new(),
            opened_at: 0.0,
        });
        let ctx = egui::Context::default();
        crate::deck::theme::install_fonts(&ctx);
        crate::deck::theme::install(&ctx, AppTheme::Dark, false);
        assert!(run(&ctx, &mut app, 10.0).is_empty());
        assert_eq!(app.sheet, Some(Sheet::VoiceNote));
        let inside = run(&ctx, &mut app, 11.0);
        assert!(!inside
            .iter()
            .any(|action| matches!(action, CompactAction::VoiceNoteAccept { .. })));
        let due = run(&ctx, &mut app, 11.25);
        assert!(
            due.iter().any(|action| matches!(
                action,
                CompactAction::VoiceNoteAccept { language } if language == "en"
            )),
            "auto-dismiss must start the job: {due:?}"
        );
    }

    #[test]
    fn no_pill_means_auto_and_no_timer() {
        let mut app = app_with(VoiceNoteView {
            title: "Voice note".to_string(),
            sender: None,
            pill: None,
            model_ok: true,
            auto_start: false,
            reason: "Pick a language, or transcribe with auto-detect.".to_string(),
            opened_at: 0.0,
        });
        let ctx = egui::Context::default();
        crate::deck::theme::install_fonts(&ctx);
        crate::deck::theme::install(&ctx, AppTheme::Dark, false);
        assert!(run(&ctx, &mut app, 5.0).is_empty());
        assert_eq!(app.sheet, Some(Sheet::VoiceNote));
        assert!(app.voice_note_deadline().is_none());
        assert!(run(&ctx, &mut app, 30.0).is_empty());
        assert_eq!(app.sheet, Some(Sheet::VoiceNote));
    }
}
