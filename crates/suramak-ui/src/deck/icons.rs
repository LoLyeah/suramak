//! M20B — the painted icon kit.
//!
//! Eleven hand-drawn glyphs at one stroke (1.75 px, round caps and joins) and
//! two designed sizes (18 pt inline, 20 pt chrome). No SVG parser, no icon
//! font, no unicode stand-ins: every curve is an `egui` shape, so the kit looks
//! like the product's own hand on Android and macOS alike.
//!
//! Each icon's interactive form is a [`a11y::touch`] response: the control is
//! labelled for TalkBack (`"Settings"`, `"Search transcripts"`, …) before a
//! single pixel is painted. The caller owns the hit box (the plan's chrome
//! targets are 44 × 44 pt); the icon ink is sized by [`IconSize`].
//!
//! **Not adopted by the screens in this slice** — slices 3–5 consume it.

use egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Response, Shape, Stroke, Ui, WidgetType};

use crate::deck::compact::a11y;
use crate::deck::theme::Tokens;

/// The plan's stroke: 1.75 px, the same weight at both designed sizes.
pub const STROKE: f32 = 1.75;
/// Inline icons beside a row of text.
pub const INLINE: f32 = 18.0;
/// Chrome icons (the app bar's gear, the dock's mic).
pub const CHROME: f32 = 20.0;
/// Chrome hit box (the plan's 44 × 44 pt target).
pub const HIT: f32 = 44.0;

/// The two designed icon sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSize {
    /// 18 pt — inline with a row title or a field.
    Inline,
    /// 20 pt — the app bar, the floating dock.
    Chrome,
}

impl IconSize {
    pub const fn points(self) -> f32 {
        match self {
            Self::Inline => INLINE,
            Self::Chrome => CHROME,
        }
    }
}

/// The eleven icons the plan names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Settings (the app bar's trailing affordance).
    Gear,
    /// Search (inside the transcript search field).
    Search,
    /// Import audio (the dock's Import pill).
    ImportWave,
    /// Record (the dock's Record pill).
    Mic,
    /// Navigation chevron.
    Chevron,
    /// Installed / selected.
    Check,
    /// Download a model.
    Download,
    /// Share / export.
    Share,
    /// Speaker playback.
    Speaker,
    /// Close (sheet dismiss).
    Close,
    /// Delete a transcript.
    Trash,
}

impl Icon {
    /// The spoken label a button for this icon carries by default. Short,
    /// action-naming, no punctuation — TalkBack reads it as the control name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Gear => "Settings",
            Self::Search => "Search transcripts",
            Self::ImportWave => "Import audio",
            Self::Mic => "Record",
            Self::Chevron => "Back",
            Self::Check => "Installed",
            Self::Download => "Download",
            Self::Share => "Share",
            Self::Speaker => "Play",
            Self::Close => "Close",
            Self::Trash => "Delete",
        }
    }
}

/// Paints `icon` centred in `rect` at `size` points. Pure: no interaction, no
/// state. The hit-box variants below call this after [`a11y::touch`].
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, size: f32, color: Color32) {
    let pen = Pen {
        painter,
        center: rect.center(),
        scale: size / 20.0,
        stroke: Stroke::new(STROKE, color),
    };
    match icon {
        Icon::Gear => {
            pen.circle(0.0, 0.0, 6.2);
            for tooth in 0..8 {
                let angle = tooth as f32 * std::f32::consts::TAU / 8.0;
                let (sin, cos) = angle.sin_cos();
                pen.line(&[(5.6 * cos, 5.6 * sin), (8.3 * cos, 8.3 * sin)]);
            }
            pen.circle(0.0, 0.0, 2.3);
        }
        Icon::Search => {
            pen.circle(-1.4, -1.4, 4.4);
            pen.line(&[(2.1, 2.1), (6.8, 6.8)]);
        }
        Icon::ImportWave => {
            pen.line(&[(0.0, -7.0), (0.0, 1.2)]);
            pen.line(&[(-3.0, -1.6), (0.0, 1.2), (3.0, -1.6)]);
            pen.line(&[
                (-6.5, 6.0),
                (-3.25, 4.0),
                (0.0, 6.0),
                (3.25, 8.0),
                (6.5, 6.0),
            ]);
        }
        Icon::Mic => {
            pen.rounded_rect(-2.6, -7.0, 2.6, 1.4, 2.6);
            pen.arc(0.0, 0.2, 4.8, 0.0, 180.0);
            pen.line(&[(0.0, 5.0), (0.0, 7.4)]);
            pen.line(&[(-3.0, 7.4), (3.0, 7.4)]);
        }
        Icon::Chevron => {
            pen.line(&[(-3.2, -5.2), (1.8, 0.0), (-3.2, 5.2)]);
        }
        Icon::Check => {
            pen.line(&[(-5.4, -0.2), (-1.6, 3.6), (5.4, -4.6)]);
        }
        Icon::Download => {
            pen.line(&[(0.0, -7.2), (0.0, 1.6)]);
            pen.line(&[(-3.4, -1.8), (0.0, 1.6), (3.4, -1.8)]);
            pen.line(&[(-5.8, 4.6), (-5.8, 7.2), (5.8, 7.2), (5.8, 4.6)]);
        }
        Icon::Share => {
            pen.line(&[(0.0, 1.4), (0.0, -7.2)]);
            pen.line(&[(-3.2, -4.0), (0.0, -7.2), (3.2, -4.0)]);
            pen.line(&[(-5.8, 0.4), (-5.8, 7.4), (5.8, 7.4), (5.8, 0.4)]);
        }
        Icon::Speaker => {
            pen.line(&[
                (-6.4, -2.4),
                (-2.6, -2.4),
                (2.4, -6.2),
                (2.4, 6.2),
                (-2.6, 2.4),
                (-6.4, 2.4),
                (-6.4, -2.4),
            ]);
            pen.arc(2.4, 0.0, 3.4, -55.0, 55.0);
            pen.arc(2.4, 0.0, 5.6, -50.0, 50.0);
        }
        Icon::Close => {
            pen.line(&[(-5.3, -5.3), (5.3, 5.3)]);
            pen.line(&[(5.3, -5.3), (-5.3, 5.3)]);
        }
        Icon::Trash => {
            pen.line(&[(-6.6, -4.8), (6.6, -4.8)]);
            pen.line(&[(-2.4, -6.9), (2.4, -6.9)]);
            pen.line(&[(-5.2, -2.8), (-4.4, 7.4), (4.4, 7.4), (5.2, -2.8)]);
            pen.line(&[(-1.8, -0.4), (-1.8, 5.2)]);
            pen.line(&[(1.8, -0.4), (1.8, 5.2)]);
        }
    }
}

/// The one interactive constructor: hit box + semantics + painted ink.
///
/// `rect` is the **hit box** (44 × 44 pt chrome, 48 pt rows); `size` only
/// changes the ink. `label` is spoken by TalkBack; pass [`Icon::label`] unless
/// the surrounding context demands a longer name.
#[allow(clippy::too_many_arguments)]
pub fn button(
    ui: &mut Ui,
    tokens: &Tokens,
    rect: Rect,
    id: impl std::hash::Hash,
    icon: Icon,
    label: &str,
    size: IconSize,
    enabled: bool,
) -> Response {
    let response = a11y::touch(ui, tokens, rect, id, WidgetType::Button, label, enabled);
    let color = if !enabled {
        tokens.ink_faint.gamma_multiply(tokens.disabled_alpha)
    } else if response.is_pointer_button_down_on() {
        tokens.accent_active
    } else if response.hovered() {
        tokens.accent
    } else {
        tokens.ink
    };
    paint(ui.painter(), rect, icon, size.points(), color);
    response
}

/// The 44 × 44 chrome hit box around `center`.
pub fn hit_rect(center: Pos2) -> Rect {
    Rect::from_center_size(center, vec2(HIT, HIT))
}

macro_rules! icon_button {
    ($name:ident, $icon:ident) => {
        /// Paints the icon and returns its labelled touch response.
        pub fn $name(
            ui: &mut Ui,
            tokens: &Tokens,
            rect: Rect,
            id: impl std::hash::Hash,
            size: IconSize,
        ) -> Response {
            button(
                ui,
                tokens,
                rect,
                id,
                Icon::$icon,
                Icon::$icon.label(),
                size,
                true,
            )
        }
    };
}

icon_button!(gear, Gear);
icon_button!(search, Search);
icon_button!(import_wave, ImportWave);
icon_button!(mic, Mic);
icon_button!(chevron, Chevron);
icon_button!(check, Check);
icon_button!(download, Download);
icon_button!(share, Share);
icon_button!(speaker, Speaker);
icon_button!(close, Close);
icon_button!(trash, Trash);

/// One 20-unit design grid, centred on the hit box, scaled to the ink size.
struct Pen<'a> {
    painter: &'a Painter,
    center: Pos2,
    scale: f32,
    stroke: Stroke,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> Pos2 {
        pos2(
            self.center.x + x * self.scale,
            self.center.y + y * self.scale,
        )
    }

    /// A round-capped, round-joined polyline. epaint's open paths have butt
    /// ends (`TODO(emilk): add line caps`), so the kit draws the caps itself:
    /// a half-stroke disc at every vertex.
    fn line(&self, points: &[(f32, f32)]) {
        let points: Vec<Pos2> = points.iter().map(|(x, y)| self.at(*x, *y)).collect();
        self.painter.add(Shape::line(points.clone(), self.stroke));
        for point in points {
            self.painter
                .circle_filled(point, self.stroke.width / 2.0, self.stroke.color);
        }
    }

    fn circle(&self, x: f32, y: f32, radius: f32) {
        self.painter.circle_stroke(
            self.at(x, y),
            radius * self.scale,
            Stroke::new(self.stroke.width, self.stroke.color),
        );
    }

    fn rounded_rect(&self, left: f32, top: f32, right: f32, bottom: f32, radius: f32) {
        let rect = Rect::from_min_max(self.at(left, top), self.at(right, bottom));
        self.painter.rect_stroke(
            rect,
            egui::CornerRadius::same((radius * self.scale).round() as u8),
            Stroke::new(self.stroke.width, self.stroke.color),
            egui::StrokeKind::Inside,
        );
    }

    /// An arc in the 20-unit grid, degrees clockwise from +x (screen y grows
    /// down, so 0°→90° sweeps the lower-right quadrant).
    fn arc(&self, cx: f32, cy: f32, radius: f32, start_deg: f32, end_deg: f32) {
        let steps = ((radius * 2.0).ceil() as usize).clamp(8, 24);
        let points: Vec<(f32, f32)> = (0..=steps)
            .map(|step| {
                let t = step as f32 / steps as f32;
                let angle = (start_deg + (end_deg - start_deg) * t).to_radians();
                (cx + radius * angle.cos(), cy + radius * angle.sin())
            })
            .collect();
        self.line(&points);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::theme::{self, AppTheme};

    /// Every icon's ink stays inside its hit box at both designed sizes, and
    /// something is actually painted.
    #[test]
    fn every_icon_fits_its_hit_box_at_both_sizes() {
        let icons = [
            Icon::Gear,
            Icon::Search,
            Icon::ImportWave,
            Icon::Mic,
            Icon::Chevron,
            Icon::Check,
            Icon::Download,
            Icon::Share,
            Icon::Speaker,
            Icon::Close,
            Icon::Trash,
        ];
        for icon in icons {
            for size in [IconSize::Inline, IconSize::Chrome] {
                let ctx = egui::Context::default();
                let hit = Rect::from_center_size(pos2(40.0, 40.0), vec2(HIT, HIT));
                let output = ctx.run(Default::default(), |ctx| {
                    let ui = Ui::new(
                        ctx.clone(),
                        egui::Id::new("icons-test"),
                        egui::UiBuilder::new()
                            .max_rect(Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0))),
                    );
                    paint(ui.painter(), hit, icon, size.points(), Color32::WHITE);
                });
                let bounds: Rect = output
                    .shapes
                    .iter()
                    .map(|clipped| clipped.shape.visual_bounding_rect())
                    .fold(Rect::NOTHING, |acc, rect| acc.union(rect));
                assert!(bounds.is_positive(), "{icon:?} {size:?}: nothing painted");
                assert!(
                    hit.expand(0.51).contains_rect(bounds),
                    "{icon:?} {size:?}: ink {bounds:?} escapes the {} pt hit box {hit:?}",
                    size.points()
                );
            }
        }
    }

    /// The a11y contract: each icon is a labelled Button before any pixel.
    #[test]
    fn every_icon_is_a_labelled_button() {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        theme::install(&ctx, AppTheme::Dark, false);
        let tokens = theme::tokens_for(
            AppTheme::Dark,
            crate::deck::material::MaterialBackend::PaintedFrost,
        );
        a11y::begin_frame(&ctx);
        let mut seen = Vec::new();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let icons = [
                    Icon::Gear,
                    Icon::Search,
                    Icon::ImportWave,
                    Icon::Mic,
                    Icon::Chevron,
                    Icon::Check,
                    Icon::Download,
                    Icon::Share,
                    Icon::Speaker,
                    Icon::Close,
                    Icon::Trash,
                ];
                for (index, icon) in icons.into_iter().enumerate() {
                    let rect =
                        Rect::from_min_size(pos2(10.0, 10.0 + index as f32 * 48.0), vec2(HIT, HIT));
                    button(
                        ui,
                        &tokens,
                        rect,
                        index,
                        icon,
                        icon.label(),
                        IconSize::Chrome,
                        true,
                    );
                }
            });
        });
        seen.extend(
            a11y::controls(&ctx)
                .into_iter()
                .map(|control| (control.label, control.role)),
        );
        assert_eq!(seen.len(), 11);
        for (label, role) in &seen {
            assert!(!label.is_empty());
            assert_eq!(*role, WidgetType::Button);
        }
        assert!(seen.iter().any(|(label, _)| label == "Settings"));
        assert!(seen.iter().any(|(label, _)| label == "Search transcripts"));
    }

    /// The stroke is the one weight; every path is round-capped by construction
    /// (each vertex carries its disc), so the kit cannot drift to a mixed set.
    #[test]
    fn the_stroke_is_one_weight() {
        assert_eq!(STROKE, 1.75);
        assert_eq!(IconSize::Inline.points(), 18.0);
        assert_eq!(IconSize::Chrome.points(), 20.0);
        assert_eq!(HIT, 44.0);
    }
}
