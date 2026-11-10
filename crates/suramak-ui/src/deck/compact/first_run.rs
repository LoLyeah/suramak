//! The first-run model card (M13g3): the release path's zero-empty launch.
//!
//! A phone with no installed model does not land on an empty library. It
//! lands here, on M13D's one recommendation for *this* device and language:
//!
//! * the tier's headline and the recommendation's detail (the same strings
//!   `suramak_core::device::recommend` produces, so the guidance cannot drift);
//! * Indonesian's `tiny` warning verbatim, when it applies;
//! * the recommended model first, then the other offered models, each with its
//!   size — and **no** model this phone refuses (that statement lives on
//!   Settings' model table, where it can be read without a download card);
//! * the free-space preflight, from the number the shell reports (`None` says
//!   so instead of pretending there is room);
//! * a running download's **job percent** and the resume offer for a `.part`
//!   left by an interrupted run.
//!
//! The card is a view: every tap becomes a [`CompactAction`] the host runs.

use egui::{pos2, vec2, Align2, CornerRadius, FontId, Rect, Stroke, StrokeKind, Ui, WidgetType};

use crate::deck::motion::Motion;
use crate::deck::theme::Tokens;

use super::a11y;
use super::chrome;
use super::model::{bytes_label, CompactAction, CompactApp, DownloadState, ModelRow};

/// The card's maximum height (the Library places the list below it).
pub const CARD_HEIGHT: f32 = 356.0;
/// One offered-model row's height.
const ROW: f32 = 58.0;

/// The card's measured height for the current state: the shared parts plus the
/// language note and the download block when they are present. A card that is
/// always `CARD_HEIGHT` tall leaves a visible hole on the release-path shot;
/// a card that grows must never clip the progress line.
pub fn height(state: &CompactApp) -> f32 {
    let mut height = 272.0;
    if state.language_note().is_some() {
        height += 48.0;
    }
    let alternatives = state
        .offered_models()
        .into_iter()
        .filter(|row| !row.recommended && !row.installed && row.id != "tiny")
        .count();
    if alternatives > 1 {
        height += ROW;
    }
    if state.download.is_some() {
        height += 34.0;
    }
    height.clamp(220.0, CARD_HEIGHT)
}

/// The card's title, shared by the screenshot suite's assertions.
pub const TITLE: &str = "Get a model for this phone";

/// Draws the card into `rect`. Returns the actions its controls requested.
pub fn card(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut CompactApp,
    rect: Rect,
) -> Vec<CompactAction> {
    let mut actions = Vec::new();
    chrome::card(ui, tokens, rect);
    let x = rect.left() + 14.0;
    let width = rect.width() - 28.0;
    ui.painter().text(
        pos2(x, rect.top() + 26.0),
        Align2::LEFT_CENTER,
        TITLE,
        FontId::new(
            20.0,
            egui::FontFamily::Name(crate::deck::theme::SERIF_FAMILY.into()),
        ),
        tokens.ink,
    );
    a11y::describe(
        ui,
        Rect::from_min_size(pos2(x, rect.top() + 10.0), vec2(width, 32.0)),
        TITLE,
    );
    ui.painter().text(
        pos2(x, rect.top() + 50.0),
        Align2::LEFT_CENTER,
        state.model_headline(),
        FontId::proportional(12.5),
        tokens.ink_soft,
    );

    let mut y = rect.top() + 66.0;
    if let Some(note) = state.language_note() {
        let galley =
            ui.painter()
                .layout(note.clone(), FontId::proportional(11.5), tokens.warn, width);
        ui.painter().galley(pos2(x, y), galley.clone(), tokens.warn);
        a11y::describe(
            ui,
            Rect::from_min_size(pos2(x, y), vec2(width, galley.size().y.max(14.0))),
            &note,
        );
        y += galley.size().y + 8.0;
    }

    // The recommended model, always first.
    if let Some(row) = state.recommended_model().cloned() {
        let row_rect = Rect::from_min_size(pos2(x, y), vec2(width, 78.0));
        paint_model_row(ui, tokens, &row, row_rect, true);
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
        let action = Rect::from_min_size(
            pos2(row_rect.right() - 104.0, row_rect.top() + 12.0),
            vec2(92.0, 36.0),
        );
        if !running {
            let response = a11y::touch(
                ui,
                tokens,
                action,
                "download-recommended",
                WidgetType::Button,
                &format!("{primary_label} {}", row.id),
                !blocked,
            );
            ui.painter().rect(
                action,
                CornerRadius::same(tokens.radius.control),
                if blocked {
                    tokens.control_fill
                } else {
                    tokens.accent
                },
                Stroke::new(
                    1.0_f32,
                    if blocked {
                        tokens.hairline
                    } else {
                        tokens.accent
                    },
                ),
                StrokeKind::Inside,
            );
            ui.painter().text(
                action.center(),
                Align2::CENTER_CENTER,
                primary_label,
                FontId::proportional(12.5),
                if blocked {
                    tokens.ink_faint
                } else {
                    tokens.on_accent
                },
            );
            if response.clicked() && !blocked {
                actions.push(CompactAction::DownloadModel(row.id.clone()));
            }
        } else {
            // The job OWNS the download; the card's only control is Pause.
            let pause = Rect::from_min_size(
                pos2(row_rect.right() - 104.0, row_rect.top() + 12.0),
                vec2(92.0, 36.0),
            );
            let response = a11y::touch(
                ui,
                tokens,
                pause,
                "download-pause",
                WidgetType::Button,
                "Pause the download",
                true,
            );
            ui.painter().rect(
                pause,
                CornerRadius::same(tokens.radius.control),
                tokens.control_fill,
                Stroke::new(1.0_f32, tokens.hairline),
                StrokeKind::Inside,
            );
            ui.painter().text(
                pause.center(),
                Align2::CENTER_CENTER,
                "Pause",
                FontId::proportional(12.5),
                tokens.ink,
            );
            if response.clicked() {
                actions.push(CompactAction::CancelDownload);
            }
        }
        a11y::describe(
            ui,
            row_rect,
            &format!("{} recommended, {}", row.id, row.size_role()),
        );
        y = row_rect.bottom() + 8.0;
    }

    // The other offered models, cheapest first, two at most: the card is a
    // recommendation, not the full manifest (that is Settings' table).
    let alternatives: Vec<ModelRow> = state
        .offered_models()
        .into_iter()
        .filter(|row| !row.recommended)
        .filter(|row| !row.installed)
        .filter(|row| row.id != "tiny")
        .take(2)
        .cloned()
        .collect();
    for row in alternatives {
        let row_rect = Rect::from_min_size(pos2(x, y), vec2(width, ROW));
        paint_model_row(ui, tokens, &row, row_rect, false);
        let blocked = state.has_space_for(row.size_bytes) == Some(false);
        let action = Rect::from_min_size(
            pos2(row_rect.right() - 72.0, row_rect.top() + 11.0),
            vec2(60.0, 34.0),
        );
        let response = a11y::touch(
            ui,
            tokens,
            action,
            ("download-alternative", row.id.clone()),
            WidgetType::Button,
            &format!("Download {}", row.id),
            !blocked,
        );
        ui.painter().rect(
            action,
            CornerRadius::same(tokens.radius.control),
            tokens.control_fill,
            Stroke::new(1.0_f32, tokens.hairline),
            StrokeKind::Inside,
        );
        ui.painter().text(
            action.center(),
            Align2::CENTER_CENTER,
            "Get",
            FontId::proportional(12.5),
            if blocked {
                tokens.ink_faint
            } else {
                tokens.ink
            },
        );
        if response.clicked() && !blocked {
            actions.push(CompactAction::DownloadModel(row.id.clone()));
        }
        y = row_rect.bottom() + 2.0;
    }
    let _ = motion;
    ui.painter().line_segment(
        [pos2(x, y + 3.0), pos2(x + width, y + 3.0)],
        Stroke::new(1.0_f32, tokens.hairline),
    );
    y += 16.0;

    // The M13D free-space preflight, with the number the shell reported.
    let recommended_size = state
        .recommended_model()
        .map(|row| row.size_bytes)
        .unwrap_or(0);
    let (space_line, space_error) = match state.free_bytes {
        Some(free) if free >= recommended_size => (
            format!(
                "Needs {} · {} free on this phone",
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
    ui.painter().text(
        pos2(x, y),
        Align2::LEFT_CENTER,
        &space_line,
        FontId::proportional(11.5),
        if space_error {
            tokens.record_red_text
        } else {
            tokens.ink_faint
        },
    );
    a11y::describe(
        ui,
        Rect::from_min_size(pos2(x, y - 10.0), vec2(width, 20.0)),
        &space_line,
    );
    y += 18.0;

    // The download's own block: the job's percent, never an estimate.
    if let Some(download) = state.download.clone() {
        let line = download.line();
        ui.painter().text(
            pos2(x, y),
            Align2::LEFT_CENTER,
            &line,
            FontId::proportional(12.0),
            match download.state {
                DownloadState::Failed => tokens.record_red_text,
                _ => tokens.ink_soft,
            },
        );
        a11y::describe(
            ui,
            Rect::from_min_size(pos2(x, y - 10.0), vec2(width, 20.0)),
            &line,
        );
        y += 10.0;
        let track = Rect::from_min_size(pos2(x, y), vec2(width, 4.0));
        ui.painter()
            .rect_filled(track, CornerRadius::same(2), tokens.control_fill);
        let fraction = if download.state == DownloadState::Done {
            1.0
        } else {
            download.fraction()
        };
        if fraction > 0.0 {
            ui.painter().rect_filled(
                Rect::from_min_size(track.min, vec2(track.width() * fraction, 4.0)),
                CornerRadius::same(2),
                if download.state == DownloadState::Failed {
                    tokens.record_red
                } else {
                    tokens.accent
                },
            );
        }
        if download.state == DownloadState::Failed && !download.message.is_empty() {
            ui.painter().text(
                pos2(x, y + 18.0),
                Align2::LEFT_CENTER,
                truncate(&download.message, 76),
                FontId::proportional(11.0),
                tokens.record_red_text,
            );
        }
    }
    actions
}

fn paint_model_row(ui: &mut Ui, tokens: &Tokens, row: &ModelRow, rect: Rect, recommended: bool) {
    ui.painter().rect(
        rect,
        CornerRadius::same(tokens.radius.card),
        if recommended {
            tokens.accent_tint
        } else {
            tokens.panel_faint
        },
        Stroke::new(
            1.0_f32,
            if recommended {
                tokens.accent
            } else {
                tokens.hairline
            },
        ),
        StrokeKind::Inside,
    );
    ui.painter().text(
        pos2(rect.left() + 12.0, rect.top() + 21.0),
        Align2::LEFT_CENTER,
        format!(
            "{}{}",
            row.id,
            if recommended { " · recommended" } else { "" }
        ),
        FontId::proportional(16.0),
        tokens.ink,
    );
    ui.painter().text(
        pos2(rect.left() + 12.0, rect.top() + 42.0),
        Align2::LEFT_CENTER,
        truncate(&format!("{} · {}", row.size_role(), row.note), 46),
        FontId::proportional(11.5),
        tokens.ink_soft,
    );
}

fn truncate(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(chars.saturating_sub(1)).collect();
    out.push('…');
    out
}
