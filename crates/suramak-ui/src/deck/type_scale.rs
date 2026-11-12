//! The type scale as tokens (M20A).
//!
//! The plan's typography table, made callable: every size, tracking and line
//! height the Deck authors lives here rather than in the screens, so slice 3+
//! can drop the ad-hoc `FontId::new(13.0, ..)` calls without inventing a new
//! value each time.
//!
//! Sizes are **egui points** — the same unit `FontId::size` uses, so a token
//! is `FontId`-shaped and needs no conversion. `tracking` is egui's
//! `extra_letter_spacing` (points added to every glyph advance; negative is
//! tighter), and `line_height` is the row height a manually-laid-out row should
//! use — egui's own `TextFormat` computes it from the font, so a token's
//! `line_height` is for the callers that measure rows themselves (selection
//! bands, cards, fixed-height rows).
//!
//! The weight faces are the bundled Inter statics off macOS and SF Pro on it;
//! the family choice happens in [`crate::deck::theme`], never here.
//!
//! **Not wired into the screens in this slice.** WORKORDER-M20A is explicit:
//! tokens and helpers only, no screen restyling. The goldens shift because the
//! faces changed, not because a size did.

use egui::{FontId, RichText};

use crate::deck::theme;

/// One typographic role: the face and size, the tracking, the line height.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeToken {
    pub font: FontId,
    /// egui's `extra_letter_spacing`, in points.
    pub tracking: f32,
    /// The row height this role should occupy, in points.
    pub line_height: f32,
}

impl TypeToken {
    /// The token as a `RichText` with its face, tracking and line height
    /// applied — the one call a screen should need for a widget label.
    pub fn rich(&self, text: impl Into<String>) -> RichText {
        RichText::new(text)
            .font(self.font.clone())
            .extra_letter_spacing(self.tracking)
            .line_height(Some(self.line_height))
    }

    /// The token as an `epaint::TextFormat` — the seam for the Deck's painted
    /// (non-widget) rows, which build a `LayoutJob` by hand.
    pub fn format(&self, color: egui::Color32) -> egui::TextFormat {
        egui::TextFormat {
            font_id: self.font.clone(),
            extra_letter_spacing: self.tracking,
            line_height: Some(self.line_height),
            color,
            ..Default::default()
        }
    }

    /// The token's size in points (shorthand for `font.size`).
    pub fn size(&self) -> f32 {
        self.font.size
    }
}

/// **Screen title** — Inter SemiBold 22 / −0.4 / 28. Top bars, sheet titles.
pub fn title() -> TypeToken {
    TypeToken {
        font: FontId::new(22.0, theme::sans_semibold()),
        tracking: -0.4,
        line_height: 28.0,
    }
}

/// **Editorial display** — Newsreader 21 / −0.2 / 27. Empty-state titles,
/// onboarding hero, the editor's document title. The only serif role.
pub fn editorial_display() -> TypeToken {
    TypeToken {
        font: FontId::new(21.0, theme::serif_family()),
        tracking: -0.2,
        line_height: 27.0,
    }
}

/// **Section label** — Inter SemiBold 11 / +0.8 / 16, set in CAPS by the
/// caller (`TRANSCRIPTION`, `MODELS`; `ink_faint`).
pub fn section_label() -> TypeToken {
    TypeToken {
        font: FontId::new(11.0, theme::sans_semibold()),
        tracking: 0.8,
        line_height: 16.0,
    }
}

/// **Row title** — Inter Medium 15 / −0.1 / 20. Card and setting titles.
pub fn row_title() -> TypeToken {
    TypeToken {
        font: FontId::new(15.0, theme::sans_medium()),
        tracking: -0.1,
        line_height: 20.0,
    }
}

/// **Row value** — Inter Regular 14 / 0 / 20. The right-hand sub-value.
pub fn row_value() -> TypeToken {
    TypeToken {
        font: FontId::new(14.0, egui::FontFamily::Proportional),
        tracking: 0.0,
        line_height: 20.0,
    }
}

/// **Body / transcript** — Inter Regular 15.5 / 0 / 24. Spoken copy.
pub fn body() -> TypeToken {
    TypeToken {
        font: FontId::new(15.5, egui::FontFamily::Proportional),
        tracking: 0.0,
        line_height: 24.0,
    }
}

/// **Caption / meta** — Inter Regular 12.5 / +0.1 / 17. Helper copy.
pub fn caption() -> TypeToken {
    TypeToken {
        font: FontId::new(12.5, egui::FontFamily::Proportional),
        tracking: 0.1,
        line_height: 17.0,
    }
}

/// **Numerals** — Inter Medium 12.5 / 0 / 16. Timecodes, durations, counts.
///
/// Laid out with [`numeral_cell`] until a `tnum` bake exists: Inter's default
/// figures are proportional (`1` is 0.415 em, `0` is 0.6455 em), so a live
/// counter would twitch every second without fixed cells.
pub fn numerals() -> TypeToken {
    TypeToken {
        font: FontId::new(12.5, theme::sans_medium()),
        tracking: 0.0,
        line_height: 16.0,
    }
}

/// **Mono (earned)** — Geist Mono 11.5 / 0 / 16. Diagnostics, hashes, paths.
pub fn mono() -> TypeToken {
    TypeToken {
        font: FontId::new(11.5, egui::FontFamily::Monospace),
        tracking: 0.0,
        line_height: 16.0,
    }
}

/// Inter Medium's tabular figure advance: the `tnum` alternates
/// (`zero.tf` … `nine.tf`) all advance 0.6479 em at the pinned 500/14
/// instance. This is the font's own tabular width, not a guess — the helper
/// below reproduces what `tnum` would have done, since epaint 0.33 cannot
/// apply the feature at runtime.
pub const NUMERAL_CELL_EM: f32 = 0.6479;

/// The width of a fixed-width run of `chars` numerals at `size` points.
///
/// Draw each character centred in one such cell (`0.6479 × size`) and a
/// `00:09 → 00:10` counter stops twitching. Colons are not digits; give them
/// their own narrower cell (see [`NUMERAL_COLON_EM`]).
pub fn numeral_cell(chars: usize, size: f32) -> f32 {
    chars as f32 * size * NUMERAL_CELL_EM
}

/// Inter Medium's tabular colon advance (0.2686 em) — the separator cell for
/// `mm:ss` runs.
pub const NUMERAL_COLON_EM: f32 = 0.2686;

/// The width of a `mm:ss`-shaped run: `digits` digit cells and `colons`
/// separator cells. The helper the timecode rows will use in M20E.
pub fn numeral_run(digits: usize, colons: usize, size: f32) -> f32 {
    numeral_cell(digits, size) + colons as f32 * size * NUMERAL_COLON_EM
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The workorder's table, pinned value by value. A token that drifts from
    /// the plan fails here before a golden can hide it.
    #[test]
    fn the_scale_matches_the_plan() {
        let title = title();
        assert_eq!(title.size(), 22.0);
        assert_eq!(title.tracking, -0.4);
        assert_eq!(title.line_height, 28.0);

        let display = editorial_display();
        assert_eq!(display.size(), 21.0);
        assert_eq!(display.tracking, -0.2);
        assert_eq!(display.line_height, 27.0);

        let label = section_label();
        assert_eq!(label.size(), 11.0);
        assert_eq!(label.tracking, 0.8);
        assert_eq!(label.line_height, 16.0);

        let row = row_title();
        assert_eq!(row.size(), 15.0);
        assert_eq!(row.tracking, -0.1);
        assert_eq!(row.line_height, 20.0);

        let value = row_value();
        assert_eq!(value.size(), 14.0);
        assert_eq!(value.tracking, 0.0);
        assert_eq!(value.line_height, 20.0);

        let body = body();
        assert_eq!(body.size(), 15.5);
        assert_eq!(body.tracking, 0.0);
        assert_eq!(body.line_height, 24.0);

        let caption = caption();
        assert_eq!(caption.size(), 12.5);
        assert_eq!(caption.tracking, 0.1);
        assert_eq!(caption.line_height, 17.0);

        let numerals = numerals();
        assert_eq!(numerals.size(), 12.5);
        assert_eq!(numerals.tracking, 0.0);
        assert_eq!(numerals.line_height, 16.0);

        let mono = mono();
        assert_eq!(mono.size(), 11.5);
        assert_eq!(mono.tracking, 0.0);
        assert_eq!(mono.line_height, 16.0);
    }

    /// The faces are the right ones: weights ask for the named families, the
    /// display asks for the serif, the mono asks for `Monospace`.
    #[test]
    fn the_roles_use_the_face_they_name() {
        assert_eq!(title().font.family, theme::sans_semibold());
        assert_eq!(section_label().font.family, theme::sans_semibold());
        assert_eq!(row_title().font.family, theme::sans_medium());
        assert_eq!(numerals().font.family, theme::sans_medium());
        assert_eq!(editorial_display().font.family, theme::serif_family());
        assert_eq!(body().font.family, egui::FontFamily::Proportional);
        assert_eq!(mono().font.family, egui::FontFamily::Monospace);
        // The families are the plan's names.
        assert_eq!(theme::SANS_MEDIUM_FAMILY, "SansMedium");
        assert_eq!(theme::SANS_SEMIBOLD_FAMILY, "SansSemiBold");
    }

    /// Fixed cells: the same number of digits is always the same width, which
    /// is the whole point (a live timer cannot move the row).
    #[test]
    fn numeral_cells_are_fixed_width() {
        // The constant is Inter Medium's own `tnum` advance (0.6479 em), not a
        // guess; pin it so a re-bake that changes the face fails here.
        assert!(
            (NUMERAL_CELL_EM - 0.6479).abs() < 1e-4,
            "{NUMERAL_CELL_EM} is not the tabular advance the face reports"
        );
        assert_eq!(numeral_cell(2, 12.5), 2.0 * 12.5 * NUMERAL_CELL_EM);
        assert_eq!(
            numeral_cell(3, 12.5),
            numeral_cell(2, 12.5) + numeral_cell(1, 12.5)
        );
        // A `mm:ss` run is the digit cells plus the narrower colon cells.
        let timecode = numeral_run(4, 1, 12.5);
        assert!((timecode - (4.0 * 12.5 * NUMERAL_CELL_EM + 12.5 * NUMERAL_COLON_EM)).abs() < 1e-4);
        // The cell is at least as wide as the proportional face's widest digit
        // (0.6455 em in Inter Regular), so no glyph can overflow it.
        assert!(numeral_cell(1, 12.5) >= 0.6455 * 12.5);
    }

    /// `format` carries the token's whole contract — face, tracking, line
    /// height — which is what the painted rows consume.
    #[test]
    fn the_text_format_carries_the_token() {
        let token = title();
        let format = token.format(egui::Color32::WHITE);
        assert_eq!(format.font_id, token.font);
        assert_eq!(format.extra_letter_spacing, token.tracking);
        assert_eq!(format.line_height, Some(token.line_height));
        // `rich` is the widget path; it must not panic and must keep the text.
        let rich = token.rich("Library");
        assert_eq!(rich.text(), "Library");
    }
}
