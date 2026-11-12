//! The first-run model banner (M13g3, redrawn in M20C): the release path's
//! zero-empty launch.
//!
//! A phone with no installed model does not land on an empty library. It
//! lands here, on M13D's one recommendation for *this* device and language:
//!
//! * the banner's headline and the recommendation's one-line reason (the
//!   strings `suramak_core::device::recommend` produces, so the guidance
//!   cannot drift);
//! * Indonesian's `tiny` warning verbatim, when it applies (two lines at most);
//! * one inline model pill (`base · 148 MB`) and the primary Download action —
//!   **one** banner, no nested card and no dead space;
//! * the free-space preflight as the footer caption, from the number the shell
//!   reports (`None` says so instead of pretending there is room);
//! * a running download's **job percent** and the resume offer for a `.part`
//!   left by an interrupted run.
//!
//! The full model table is Settings' job (slice 4); this is a recommendation,
//! not a manifest. The banner disappears entirely once a model is verified on
//! the phone (`CompactApp::model_card_needed`).
//!
//! The card is a view: every tap becomes a [`CompactAction`] the host runs.

use egui::{pos2, vec2, Align2, CornerRadius, Rect, Stroke, StrokeKind, Ui, WidgetType};

use crate::deck::material::{self, Plane};
use crate::deck::motion::Motion;
use crate::deck::theme::Tokens;
use crate::deck::type_scale;

use super::a11y;
use super::model::{bytes_label, CompactAction, CompactApp, DownloadState};

/// The banner's internal padding (the 8 pt rhythm: 16 in, 16 out).
pub const PAD: f32 = 16.0;
/// The action row's designed height (the pill and the button share it).
const ACTION_H: f32 = 36.0;
/// The headline's line height, from the editorial display token.
const HEADLINE_H: f32 = 27.0;
/// The reason / footer caption line height.
const CAPTION_H: f32 = 17.0;
/// The gap between the headline and the reason line.
const GAP_REASON: f32 = 4.0;
/// The gap above the action row.
const GAP_ACTION: f32 = 14.0;
/// The gap above the footer.
const GAP_FOOTER: f32 = 12.0;
/// The Download/resume button's width.
const BUTTON_W: f32 = 104.0;

/// The banner's title, shared by the screenshot suite's assertions.
pub const TITLE: &str = "Get a model for this phone";

/// One measured banner: every rect the painter uses, plus the height the parts
/// occupy. [`height`] and [`card`] both come through here, so a golden can
/// never disagree with the space the Library reserved.
struct BannerLayout {
    headline: Rect,
    reason: Rect,
    note: Option<Rect>,
    pill: Rect,
    action: Rect,
    footer: Rect,
    job: Option<Rect>,
    progress: Option<Rect>,
    message: Option<Rect>,
    used: f32,
}

fn layout(ctx: &egui::Context, state: &CompactApp, rect: Rect) -> BannerLayout {
    let content = rect.shrink(PAD);
    let mut y = rect.top() + PAD;

    let headline = Rect::from_min_size(pos2(content.left(), y), vec2(content.width(), HEADLINE_H));
    y = headline.bottom() + GAP_REASON;
    let reason = Rect::from_min_size(pos2(content.left(), y), vec2(content.width(), CAPTION_H));
    y = reason.bottom();

    let note = state.language_note().map(|note| {
        let height = note_height(ctx, &note, content.width());
        let rect =
            Rect::from_min_size(pos2(content.left(), y + 4.0), vec2(content.width(), height));
        y = rect.bottom();
        rect
    });

    let action = Rect::from_min_size(
        pos2(content.left(), y + GAP_ACTION),
        vec2(content.width(), ACTION_H),
    );
    y = action.bottom() + GAP_FOOTER;

    // The pill is measured at paint time (its text width depends on the model
    // and the face); this rect carries its left/top anchor and row height.
    let pill = Rect::from_min_size(pos2(content.left(), action.top()), vec2(0.0, ACTION_H));
    let button = Rect::from_min_size(
        pos2(content.right() - BUTTON_W, action.top()),
        vec2(BUTTON_W, ACTION_H),
    );

    let footer = Rect::from_min_size(pos2(content.left(), y), vec2(content.width(), CAPTION_H));
    y = footer.bottom();

    let job = state.download.as_ref().map(|_| {
        let rect = Rect::from_min_size(
            pos2(content.left(), y + 6.0),
            vec2(content.width(), CAPTION_H),
        );
        y = rect.bottom();
        rect
    });
    let progress = job.map(|job| {
        let rect = Rect::from_min_size(
            pos2(content.left(), job.bottom() + 5.0),
            vec2(content.width(), 4.0),
        );
        y = rect.bottom();
        rect
    });
    let message = state
        .download
        .as_ref()
        .filter(|download| download.state == DownloadState::Failed && !download.message.is_empty())
        .map(|_| {
            let rect = Rect::from_min_size(
                pos2(content.left(), y + 5.0),
                vec2(content.width(), CAPTION_H),
            );
            y = rect.bottom();
            rect
        });

    BannerLayout {
        headline,
        reason,
        note,
        pill,
        action: button,
        footer,
        job,
        progress,
        message,
        used: y + PAD - rect.top(),
    }
}

/// The banner's measured height for the current state. A shrink-wrapped banner
/// is the point (the M20C order): the old card carried ~80 pt of dead space.
pub fn height(ctx: &egui::Context, state: &CompactApp, width: f32) -> f32 {
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(width.max(1.0), 0.0));
    layout(ctx, state, rect).used
}

fn note_height(ctx: &egui::Context, note: &str, width: f32) -> f32 {
    let token = type_scale::caption();
    let mut job = egui::text::LayoutJob::default();
    job.append(note, 0.0, token.format(egui::Color32::PLACEHOLDER));
    job.wrap.max_width = width.max(1.0);
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    ctx.fonts_mut(|fonts| fonts.layout_job(job).size().y)
        .min(token.line_height * 2.0)
}

/// Draws the banner into `rect`. Returns the actions its controls requested.
pub fn card(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut CompactApp,
    rect: Rect,
) -> Vec<CompactAction> {
    let mut actions = Vec::new();
    material::frosted_panel(ui, tokens, rect, tokens.radius.card, Plane::Card);
    let measured = layout(ui.ctx(), state, rect);
    debug_assert!(
        (rect.height() - measured.used).abs() <= 0.5,
        "the Library reserved {} pt but the banner measures {} pt",
        rect.height(),
        measured.used
    );
    // Every glyph stays inside the banner, whatever the space preflight says.
    let painter = ui.painter().with_clip_rect(rect);

    let title = type_scale::editorial_display();
    let mut job = egui::text::LayoutJob::default();
    job.append(TITLE, 0.0, title.format(tokens.ink));
    job.wrap.max_width = measured.headline.width();
    job.wrap.max_rows = 1;
    job.wrap.overflow_character = Some('…');
    painter.galley(measured.headline.min, painter.layout_job(job), tokens.ink);
    a11y::describe(ui, measured.headline, TITLE);

    let caption = type_scale::caption();
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &state.model_headline(),
        0.0,
        caption.format(tokens.ink_soft),
    );
    job.wrap.max_width = measured.reason.width();
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    painter.galley(
        measured.reason.min,
        painter.layout_job(job),
        tokens.ink_soft,
    );

    if let (Some(note), Some(rect)) = (state.language_note(), measured.note) {
        let mut job = egui::text::LayoutJob::default();
        job.append(&note, 0.0, caption.format(tokens.warn));
        job.wrap.max_width = rect.width();
        job.wrap.max_rows = 2;
        job.wrap.break_anywhere = false;
        job.wrap.overflow_character = Some('…');
        painter.galley(rect.min, painter.layout_job(job), tokens.warn);
        a11y::describe(ui, rect, &note);
    }

    // The one model pill and the one primary action.
    if let Some(row) = state.recommended_model().cloned() {
        let pill_text = format!("{} · {}", row.id, bytes_label(row.size_bytes));
        let pill_font = type_scale::row_value().font;
        let pill_width = painter
            .layout_no_wrap(
                pill_text.clone(),
                pill_font.clone(),
                egui::Color32::PLACEHOLDER,
            )
            .size()
            .x
            + 28.0;
        let pill = Rect::from_min_size(
            measured.pill.min,
            vec2(
                pill_width
                    .min(measured.action.left() - 12.0 - measured.pill.left())
                    .max(0.0),
                ACTION_H,
            ),
        );
        painter.rect(
            pill,
            CornerRadius::same(tokens.radius.control),
            tokens.control_fill,
            Stroke::new(1.0_f32, tokens.hairline),
            StrokeKind::Inside,
        );
        painter.text(
            pill.center(),
            Align2::CENTER_CENTER,
            &pill_text,
            pill_font,
            tokens.ink,
        );
        a11y::describe(
            ui,
            pill,
            &format!("{} recommended, {}", row.id, row.size_role()),
        );

        let blocked = state.has_space_for(row.size_bytes) == Some(false);
        let download = state
            .download
            .clone()
            .filter(|download| download.model == row.id && download.state != DownloadState::Done);
        let running = download
            .as_ref()
            .is_some_and(|download| download.state == DownloadState::Running);
        let primary_label = match &download {
            Some(download) if download.state == DownloadState::Paused => "Resume",
            Some(download) if download.state == DownloadState::Failed => "Try again",
            _ => "Download",
        };
        if running {
            // The job OWNS the download; the banner's only control is Pause.
            let response = a11y::touch(
                ui,
                tokens,
                measured.action,
                "download-pause",
                WidgetType::Button,
                "Pause the download",
                true,
            );
            paint_button(
                &painter,
                tokens,
                response.is_pointer_button_down_on(),
                measured.action,
                "Pause",
                false,
                true,
            );
            if response.clicked() {
                actions.push(CompactAction::CancelDownload);
            }
        } else {
            let response = a11y::touch(
                ui,
                tokens,
                measured.action,
                "download-recommended",
                WidgetType::Button,
                &format!("{primary_label} {}", row.id),
                !blocked,
            );
            paint_button(
                &painter,
                tokens,
                response.is_pointer_button_down_on(),
                measured.action,
                primary_label,
                !blocked,
                !blocked,
            );
            if response.clicked() && !blocked {
                actions.push(CompactAction::DownloadModel(row.id.clone()));
            }
        }
    }

    // The footer: the M13D free-space preflight, with the number the shell
    // reported — never invented.
    let recommended_size = state
        .recommended_model()
        .map(|row| row.size_bytes)
        .unwrap_or(0);
    let (space_line, space_error) = match state.free_bytes {
        Some(free) if free >= recommended_size => (
            format!(
                "Needs {} · {} free",
                bytes_label(recommended_size),
                bytes_label(free)
            ),
            false,
        ),
        Some(free) => (
            format!(
                "Not enough space — {} free, {} needed",
                bytes_label(free),
                bytes_label(recommended_size)
            ),
            true,
        ),
        None => (
            "Free space not reported by the shell — the download will preflight it".to_string(),
            false,
        ),
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &space_line,
        0.0,
        caption.format(if space_error {
            tokens.record_red_text
        } else {
            tokens.ink_faint
        }),
    );
    job.wrap.max_width = measured.footer.width();
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    painter.galley(
        measured.footer.min,
        painter.layout_job(job),
        tokens.ink_faint,
    );
    a11y::describe(ui, measured.footer, &space_line);

    // The download's own block: the job's percent, never an estimate.
    if let (Some(download), Some(job_rect)) = (state.download.clone(), measured.job) {
        let line = download.line();
        let mut job = egui::text::LayoutJob::default();
        job.append(
            &line,
            0.0,
            caption.format(match download.state {
                DownloadState::Failed => tokens.record_red_text,
                _ => tokens.ink_soft,
            }),
        );
        job.wrap.max_width = job_rect.width();
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = false;
        job.wrap.overflow_character = Some('…');
        painter.galley(job_rect.min, painter.layout_job(job), tokens.ink_soft);
        a11y::describe(ui, job_rect, &line);

        if let Some(track) = measured.progress {
            painter.rect_filled(track, CornerRadius::same(2), tokens.control_fill);
            let fraction = if download.state == DownloadState::Done {
                1.0
            } else {
                download.fraction()
            };
            if fraction > 0.0 {
                painter.rect_filled(
                    Rect::from_min_size(track.min, vec2(track.width() * fraction, track.height())),
                    CornerRadius::same(2),
                    if download.state == DownloadState::Failed {
                        tokens.record_red
                    } else {
                        tokens.accent
                    },
                );
            }
        }
        if let (DownloadState::Failed, Some(message)) = (download.state, measured.message) {
            let mut job = egui::text::LayoutJob::default();
            job.append(
                &download.message,
                0.0,
                caption.format(tokens.record_red_text),
            );
            job.wrap.max_width = message.width();
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = false;
            job.wrap.overflow_character = Some('…');
            painter.galley(message.min, painter.layout_job(job), tokens.record_red_text);
        }
    }

    let _ = motion;
    actions
}

/// The banner's primary/secondary button body: 36 pt, control radius, the
/// accent fill when enabled (the Record red is the dock's alone).
fn paint_button(
    painter: &egui::Painter,
    tokens: &Tokens,
    pressed: bool,
    rect: Rect,
    label: &str,
    enabled: bool,
    primary: bool,
) {
    let fill = if !enabled {
        tokens.control_fill
    } else if primary {
        if pressed {
            tokens.accent_active
        } else {
            tokens.accent
        }
    } else {
        tokens.control_fill
    };
    painter.rect(
        rect,
        CornerRadius::same(tokens.radius.control),
        fill,
        Stroke::new(
            1.0_f32,
            if primary && enabled {
                fill
            } else {
                tokens.hairline
            },
        ),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        type_scale::row_title().font,
        if !enabled {
            tokens.ink_faint
        } else if primary {
            tokens.on_accent
        } else {
            tokens.ink
        },
    );
}
