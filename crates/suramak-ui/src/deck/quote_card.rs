//! M22B — the quote card the voice-note notification shares.
//!
//! This is the M18.2 shape, in the Deck's own type: the transcript title, the
//! speaker attribution (only when the share intent provided it), the page's
//! text in Newsreader, and a quiet app footer. It is a **pure function**:
//! text + metadata + theme in, a pixel buffer out — no window, no wgpu, no
//! Android view. The off-screen pass is the same deterministic rasteriser the
//! phone screenshot suite already uses ([`snapshot`]), so a card is
//! reproducible on the host and on the phone.
//!
//! The card is drawn at 540 × auto points and rasterised at 2× (1080 px wide),
//! which is a shareable size for a chat without pretending to be print.

use egui::{pos2, vec2, Color32, CornerRadius, FontId, Rect, Stroke, StrokeKind, Ui, UiBuilder};

use crate::deck::material::{self, Plane};
use crate::deck::snapshot::{self, Shot};
use crate::deck::theme::{self, AppTheme};
use crate::deck::type_scale;

/// The logical card width (1080 physical at the 2× raster).
pub const CARD_WIDTH: f32 = 540.0;
/// The raster scale (physical pixels per point).
pub const CARD_SCALE: f32 = 2.0;
/// The narrowest card the layout allows (a very short quote).
pub const CARD_MIN_HEIGHT: f32 = 320.0;
/// The tallest card before the body is clipped; callers pass one notification
/// page (≤ 90 words), which stays well inside this.
pub const CARD_MAX_HEIGHT: f32 = 1_100.0;

/// What the card paints.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuoteCard<'a> {
    /// The transcript's title — the card's headline.
    pub title: &'a str,
    /// The sender's name, only when the sharing intent provided one.
    pub attribution: Option<&'a str>,
    /// The page's text, with speaker prefixes when diarization ran.
    pub body: &'a str,
    pub theme: AppTheme,
}

/// Paints one card and returns its pixels.
pub fn render(card: &QuoteCard<'_>, width: f32) -> Shot {
    let width = width.max(280.0);
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    theme::install(&ctx, card.theme, true);
    // egui builds the font atlas on the first frame; measuring before that
    // panics ("No fonts available until first call to Context::run()").
    let warm = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 10.0))),
        ..Default::default()
    };
    let _ = ctx.run(warm, |_| {});
    let tokens = theme::tokens(card.theme);
    let margin = 30.0;
    let wrap = width - margin * 2.0 - 48.0;

    let title_font = FontId::new(30.0, theme::sans_semibold());
    let body_font = type_scale::editorial_display().font;
    let body_font = FontId::new(25.0, body_font.family.clone());

    // Measure first: the card's height follows the words, never a fixed frame
    // that clips a long Indonesian sentence.
    let title_galley = ctx.fonts_mut(|fonts| {
        fonts.layout(card.title.to_owned(), title_font.clone(), tokens.ink, wrap)
    });
    let body_galley = ctx
        .fonts_mut(|fonts| fonts.layout(card.body.to_owned(), body_font.clone(), tokens.ink, wrap));
    let attribution_h = if card.attribution.is_some() {
        30.0
    } else {
        0.0
    };
    let height = (margin * 2.0
        + title_galley.size().y
        + 18.0
        + attribution_h
        + 22.0
        + body_galley.size().y
        + 56.0)
        .clamp(CARD_MIN_HEIGHT, CARD_MAX_HEIGHT);
    let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(width, height));

    snapshot::render(&ctx, screen, CARD_SCALE, |ctx| {
        // The card sits on the theme's window fill; the rasteriser's clear
        // colour is black, which would otherwise read as a painted border.
        ctx.layer_painter(egui::LayerId::background()).rect_filled(
            screen,
            CornerRadius::ZERO,
            tokens.window,
        );
        let ui = Ui::new(
            ctx.clone(),
            egui::Id::new("suramak-quote-card"),
            UiBuilder::new().max_rect(screen),
        );
        let card_rect = screen.shrink(margin);
        material::frosted_panel(&ui, &tokens, card_rect, tokens.radius.card, Plane::Card);
        let painter = ui.painter();
        let left = card_rect.left() + 24.0;
        let mut top = card_rect.top() + 24.0;

        painter.galley(pos2(left, top), title_galley.clone(), tokens.ink);
        top += title_galley.size().y + 18.0;

        if let Some(name) = card.attribution {
            painter.text(
                pos2(left, top),
                egui::Align2::LEFT_TOP,
                name,
                FontId::new(15.0, theme::sans_medium()),
                tokens.ink_soft,
            );
            top += attribution_h;
        }

        // The one accent rule: a 40 pt mark, not a box around the quote.
        painter.rect_filled(
            Rect::from_min_size(pos2(left, top + 2.0), vec2(40.0, 3.0)),
            CornerRadius::same(2),
            tokens.accent,
        );
        top += 22.0;

        painter.galley(pos2(left, top), body_galley.clone(), tokens.ink);

        // The footer sits on the card's bottom edge, above the last line.
        let footer = type_scale::mono();
        painter.text(
            pos2(left, card_rect.bottom() - 30.0),
            egui::Align2::LEFT_TOP,
            "SURAMAK · ON-DEVICE",
            FontId::new(11.0, footer.font.family.clone()),
            tokens.ink_faint,
        );
        // A hairline separates the quote from the mark.
        painter.line_segment(
            [
                pos2(left, card_rect.bottom() - 44.0),
                pos2(card_rect.right() - 24.0, card_rect.bottom() - 44.0),
            ],
            Stroke::new(1.0_f32, tokens.hairline),
        );
        painter.rect_stroke(
            card_rect,
            tokens.radius.card,
            Stroke::new(1.0_f32, tokens.hairline),
            StrokeKind::Inside,
        );
    })
}

/// Writes the card as a PNG (the one file the platform shares).
pub fn write_png(path: &std::path::Path, shot: &Shot) -> std::io::Result<()> {
    snapshot::write_png(path, shot)
}

/// The card's body colour for a theme — exposed so a test can assert the two
/// appearances really differ rather than both painting the dark ladder.
pub fn background(theme: AppTheme) -> Color32 {
    theme::tokens(theme).window
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card<'a>(body: &'a str, theme: AppTheme) -> QuoteCard<'a> {
        QuoteCard {
            title: "Voice note",
            attribution: Some("Rina"),
            body,
            theme,
        }
    }

    #[test]
    fn a_card_renders_real_pixels_in_both_themes() {
        for theme in [AppTheme::Dark, AppTheme::Light] {
            let shot = render(&card("Halo, apa kabar?", theme), CARD_WIDTH);
            assert_eq!(shot.width, (CARD_WIDTH * CARD_SCALE) as usize);
            assert!(shot.height > 0);
            let painted = shot.painted_fraction(Color32::BLACK);
            assert!(painted > 0.9, "{theme:?} card painted {painted}");
            assert_ne!(background(AppTheme::Dark), background(AppTheme::Light));
        }
    }

    #[test]
    fn a_long_indonesian_quote_makes_a_taller_card_and_never_panics() {
        let short = render(&card("Halo.", AppTheme::Dark), CARD_WIDTH);
        let long = render(
            &card(
                "Selamat pagi, ini catatan suara yang cukup panjang tentang \
                 rapat kemarin: anggaran kuartal depan, jadwal peluncuran, dan \
                 nama-nama yang perlu dihubungi sebelum hari Jumat. \
                 Tolong follow-up deck-nya sebelum rapat berikutnya, ya.",
                AppTheme::Dark,
            ),
            CARD_WIDTH,
        );
        assert!(
            long.height > short.height,
            "the card grows with the words ({} vs {})",
            long.height,
            short.height
        );
        // Quotes, diacritics, emoji and very long words are ordinary text.
        let awkward = render(
            &card(
                "“Kata” — café · 日本語 · 🎹 · supercalifragilisticexpialidocious",
                AppTheme::Light,
            ),
            CARD_WIDTH,
        );
        assert!(awkward.painted_fraction(Color32::BLACK) > 0.9);
    }

    #[test]
    fn two_renders_of_the_same_card_are_pixel_identical() {
        let first = render(&card("Halo, dunia.", AppTheme::Dark), CARD_WIDTH);
        let second = render(&card("Halo, dunia.", AppTheme::Dark), CARD_WIDTH);
        assert_eq!(first.width, second.width);
        assert_eq!(first.height, second.height);
        assert_eq!(first.pixels, second.pixels);
    }
}
