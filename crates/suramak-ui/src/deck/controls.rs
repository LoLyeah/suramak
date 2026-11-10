//! ONE control system (M9a): three tiers, fully painted, every state defined.
//!
//! Tiers: **primary** (the accent-filled call to action — Export inside the
//! export sheet, the destructive confirm), **secondary** (labeled buttons on
//! the card surface — Polish, Stop, Save, Export…, Download), **tertiary**
//! (quiet glyphs and text actions — Home/Back, trash, skip, chips).
//!
//! Everything is painted from the theme tokens: no egui stock buttons, no
//! emoji/unicode glyph controls (the M4.5 🗑 ✔ ✖ set is replaced by
//! [`Glyph`] paintings). Every state is defined — rest / hover / active /
//! focus / disabled — with token deltas, and hover/press tint crossfades use
//! the fast motion token (a snap under Reduce Motion).
//!
//! Hit areas are ≥ 24 pt; glyph controls carry tooltips; the cursor turns
//! into a pointing hand over an enabled control.

use egui::{
    pos2, vec2, Color32, CornerRadius, FontFamily, FontId, Rect, Response, Sense, Stroke,
    StrokeKind, Ui, WidgetInfo, WidgetType,
};

use crate::motion::{self, Motion};
use crate::theme::Tokens;

/// The control tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Accent-filled call to action.
    Primary,
    /// Labeled button on the card/control surface.
    Secondary,
    /// Quiet text or glyph action.
    Tertiary,
    /// A translucent capsule/rounded-rect body — the iOS-26-style "real
    /// button" treatment (owner-directed, 2026-10-14). Used by the editor's
    /// title-line cluster (trash / Polish / Undo); rests on the control fill,
    /// hovers to the surface wash, and reads in the floating pill's material
    /// family (translucent fill + hairline). The body is a capsule: its corner
    /// radius is half its height.
    Capsule,
}

/// Every control state, defined for every tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Rest,
    Hover,
    Active,
    Focus,
    Disabled,
}

/// The resolved colours of one control state (pure, unit-tested).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StateColors {
    pub fill: Color32,
    pub stroke: Color32,
    pub text: Color32,
}

/// The state a control is in, from its interaction facts.
pub fn state_for(enabled: bool, pressed: bool, hovered: bool, focused: bool) -> State {
    if !enabled {
        State::Disabled
    } else if pressed {
        State::Active
    } else if hovered {
        State::Hover
    } else if focused {
        State::Focus
    } else {
        State::Rest
    }
}

/// The token deltas for a tier × state.
pub fn tier_colors(tokens: &Tokens, tier: Tier, state: State) -> StateColors {
    let dim = |color: Color32| color.gamma_multiply(tokens.disabled_alpha);
    let (fill, stroke, text) = match (tier, state) {
        (Tier::Primary, State::Rest) => (tokens.accent, Color32::TRANSPARENT, tokens.on_accent),
        (Tier::Primary, State::Hover) => {
            (tokens.accent_hover, Color32::TRANSPARENT, tokens.on_accent)
        }
        (Tier::Primary, State::Active) => {
            (tokens.accent_active, Color32::TRANSPARENT, tokens.on_accent)
        }
        (Tier::Primary, State::Focus) => (tokens.accent, tokens.accent, tokens.on_accent),
        (Tier::Primary, State::Disabled) => (
            dim(tokens.accent),
            Color32::TRANSPARENT,
            dim(tokens.on_accent),
        ),
        (Tier::Secondary, State::Rest) => (tokens.control_fill, tokens.hairline, tokens.ink),
        (Tier::Secondary, State::Hover) => (
            tokens.control_fill_hover,
            tokens.hairline_strong,
            tokens.ink,
        ),
        (Tier::Secondary, State::Active) => (
            tokens.control_fill_active,
            tokens.hairline_strong,
            tokens.ink,
        ),
        (Tier::Secondary, State::Focus) => (tokens.control_fill, tokens.accent, tokens.ink),
        (Tier::Secondary, State::Disabled) => (
            dim(tokens.control_fill),
            dim(tokens.hairline),
            dim(tokens.ink),
        ),
        (Tier::Tertiary, State::Rest) => {
            (Color32::TRANSPARENT, Color32::TRANSPARENT, tokens.ink_soft)
        }
        (Tier::Tertiary, State::Hover) => (tokens.surface_hover, Color32::TRANSPARENT, tokens.ink),
        (Tier::Tertiary, State::Active) => {
            (tokens.surface_active, Color32::TRANSPARENT, tokens.ink)
        }
        (Tier::Tertiary, State::Focus) => (tokens.surface_hover, tokens.accent, tokens.ink),
        (Tier::Tertiary, State::Disabled) => (
            Color32::TRANSPARENT,
            Color32::TRANSPARENT,
            dim(tokens.ink_soft),
        ),
        // Capsule (the title-line cluster): a translucent body at rest — the
        // pill's material family — brightening on hover/press.
        (Tier::Capsule, State::Rest) => (tokens.control_fill, tokens.hairline, tokens.ink),
        (Tier::Capsule, State::Hover) => (
            tokens.control_fill_hover,
            tokens.hairline_strong,
            tokens.ink,
        ),
        (Tier::Capsule, State::Active) => (
            tokens.control_fill_active,
            tokens.hairline_strong,
            tokens.ink,
        ),
        (Tier::Capsule, State::Focus) => (tokens.control_fill, tokens.accent, tokens.ink),
        (Tier::Capsule, State::Disabled) => (
            dim(tokens.control_fill),
            dim(tokens.hairline),
            dim(tokens.ink),
        ),
    };
    StateColors { fill, stroke, text }
}

/// The painted glyph vocabulary — vector shapes only, never emoji/unicode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    /// Play triangle.
    Play,
    /// Two pause bars.
    Pause,
    /// Skip-to-start: a bar plus a left-pointing triangle.
    SkipStart,
    /// A wastebasket (lid + tapered body).
    Trash,
    /// A check mark.
    Check,
    /// A cross (cancel).
    Cross,
    /// A rounded stop square (the floating pill's live state).
    Stop,
    /// A back chevron (navigation).
    ChevronLeft,
}

/// The painted ink bounds of `glyph` centred in `rect`, **without painting** —
/// the pure mirror of [`paint_glyph`]'s shape math (stroke widths included:
/// line strokes are centred on their segment, filled rects expand by nothing).
/// The audit (`app::bounds`) and the `debug_assert` inside [`paint_glyph`] use
/// it to guarantee every glyph's ink fits the rect its control allocated.
pub fn glyph_bounds(glyph: Glyph, rect: Rect) -> Rect {
    let center = rect.center();
    let half = 0.8; // half of the 1.6 pt glyph stroke
    let stroke_pad = |min: egui::Pos2, max: egui::Pos2| {
        Rect::from_min_max(
            pos2(min.x - half, min.y - half),
            pos2(max.x + half, max.y + half),
        )
    };
    match glyph {
        Glyph::Play => Rect::from_min_max(
            pos2(center.x - 5.0, center.y - 7.5),
            pos2(center.x + 7.5, center.y + 7.5),
        ),
        Glyph::Pause => Rect::from_min_max(
            pos2(center.x - 6.5, center.y - 7.5),
            pos2(center.x + 6.5, center.y + 7.5),
        ),
        Glyph::SkipStart => Rect::from_min_max(
            pos2(center.x - 7.75, center.y - 7.0),
            pos2(center.x + 6.0, center.y + 7.0),
        ),
        Glyph::Trash => stroke_pad(
            pos2(center.x - 7.0, center.y - 7.0),
            pos2(center.x + 7.0, center.y + 7.0),
        ),
        Glyph::Check => stroke_pad(
            pos2(center.x - 6.0, center.y - 5.0),
            pos2(center.x + 6.5, center.y + 4.5),
        ),
        Glyph::Cross => stroke_pad(
            pos2(center.x - 5.0, center.y - 5.0),
            pos2(center.x + 5.0, center.y + 5.0),
        ),
        Glyph::Stop => Rect::from_center_size(center, vec2(13.0, 13.0)),
        Glyph::ChevronLeft => stroke_pad(
            pos2(center.x - 3.0, center.y - 5.0),
            pos2(center.x + 2.5, center.y + 5.0),
        ),
    }
}

/// Paints `glyph` centred in `rect` in `color`, returning its ink bounds.
///
/// The `debug_assert` is the painted-bounds invariant enforced at the source:
/// a glyph may never paint outside the rect its control allocated (every call
/// site allocates at least a 26 pt body around it).
pub fn paint_glyph(painter: &egui::Painter, rect: Rect, glyph: Glyph, color: Color32) -> Rect {
    let bounds = glyph_bounds(glyph, rect);
    debug_assert!(
        rect.expand(0.01).contains_rect(bounds),
        "glyph {glyph:?} paints outside its allocated rect {rect:?} (ink {bounds:?})"
    );
    let center = rect.center();
    let stroke = Stroke::new(1.6_f32, color);
    match glyph {
        Glyph::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(center.x - 5.0, center.y - 7.5),
                    pos2(center.x - 5.0, center.y + 7.5),
                    pos2(center.x + 7.5, center.y),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Glyph::Pause => {
            for offset in [-4.5_f32, 4.5] {
                painter.rect_filled(
                    Rect::from_center_size(pos2(center.x + offset, center.y), vec2(4.0, 15.0)),
                    CornerRadius::same(1),
                    color,
                );
            }
        }
        Glyph::SkipStart => {
            painter.rect_filled(
                Rect::from_center_size(pos2(center.x - 6.5, center.y), vec2(2.5, 14.0)),
                CornerRadius::same(1),
                color,
            );
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(center.x + 6.0, center.y - 7.0),
                    pos2(center.x + 6.0, center.y + 7.0),
                    pos2(center.x - 4.0, center.y),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Glyph::Trash => {
            // Lid + tapered body outline.
            painter.line_segment(
                [
                    pos2(center.x - 7.0, center.y - 5.0),
                    pos2(center.x + 7.0, center.y - 5.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(center.x - 2.5, center.y - 7.0),
                    pos2(center.x + 2.5, center.y - 7.0),
                ],
                stroke,
            );
            painter.rect_stroke(
                Rect::from_min_max(
                    pos2(center.x - 5.5, center.y - 3.5),
                    pos2(center.x + 5.5, center.y + 7.0),
                ),
                CornerRadius::same(2),
                stroke,
                StrokeKind::Inside,
            );
            for offset in [-2.0_f32, 2.0] {
                painter.line_segment(
                    [
                        pos2(center.x + offset, center.y - 1.5),
                        pos2(center.x + offset, center.y + 4.5),
                    ],
                    Stroke::new(1.2_f32, color),
                );
            }
        }
        Glyph::Check => {
            painter.line_segment(
                [
                    pos2(center.x - 6.0, center.y),
                    pos2(center.x - 1.5, center.y + 4.5),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(center.x - 1.5, center.y + 4.5),
                    pos2(center.x + 6.5, center.y - 5.0),
                ],
                stroke,
            );
        }
        Glyph::Cross => {
            painter.line_segment(
                [
                    pos2(center.x - 5.0, center.y - 5.0),
                    pos2(center.x + 5.0, center.y + 5.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(center.x + 5.0, center.y - 5.0),
                    pos2(center.x - 5.0, center.y + 5.0),
                ],
                stroke,
            );
        }
        Glyph::Stop => {
            painter.rect_filled(
                Rect::from_center_size(center, vec2(13.0, 13.0)),
                CornerRadius::same(3),
                color,
            );
        }
        Glyph::ChevronLeft => {
            painter.line_segment(
                [
                    pos2(center.x + 2.5, center.y - 5.0),
                    pos2(center.x - 3.0, center.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(center.x - 3.0, center.y),
                    pos2(center.x + 2.5, center.y + 5.0),
                ],
                stroke,
            );
        }
    }
    bounds
}

/// The resolved interaction of a painted control (state + animated factors).
struct Painted {
    response: Response,
    colors: StateColors,
}

/// The one sanctioned excess beyond a control's allocated rect: the focus
/// ring's path sits at `rect.expand(FOCUS_RING_EXPAND)` (its 2 pt outside
/// stroke rides on that path; `app::bounds` audits the total).
pub const FOCUS_RING_EXPAND: f32 = 1.5;

/// The body radius for a tier: a capsule (height/2) for [`Tier::Capsule`], the
/// control radius for the rest.
fn body_radius(tokens: &Tokens, tier: Tier, rect: Rect) -> CornerRadius {
    match tier {
        Tier::Capsule => CornerRadius::same((rect.height() / 2.0).round().clamp(0.0, 255.0) as u8),
        _ => CornerRadius::same(tokens.radius.control),
    }
}

/// Paints one control body from its tier × state and returns the response.
fn painted_body(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    id: egui::Id,
    rect: Rect,
    tier: Tier,
    enabled: bool,
) -> Painted {
    let response = ui.interact(rect, id, Sense::click());
    let state = state_for(
        enabled,
        enabled && response.is_pointer_button_down_on(),
        enabled && response.hovered(),
        response.has_focus(),
    );
    // Fast crossfades between rest/hover/press; a snap under Reduce Motion.
    let hover = motion.factor(
        ui.ctx(),
        id.with("hover"),
        enabled && (response.hovered() || response.has_focus()),
        motion::FAST,
    );
    let press = motion.factor(
        ui.ctx(),
        id.with("press"),
        enabled && response.is_pointer_button_down_on(),
        motion::FAST,
    );
    let rest = tier_colors(tokens, tier, State::Rest);
    let hovered = tier_colors(tokens, tier, State::Hover);
    let active = tier_colors(tokens, tier, State::Active);
    let base = tier_colors(tokens, tier, state);
    let colors = if enabled {
        StateColors {
            fill: motion::lerp_color(
                motion::lerp_color(rest.fill, hovered.fill, hover),
                active.fill,
                press,
            ),
            stroke: motion::lerp_color(
                motion::lerp_color(rest.stroke, hovered.stroke, hover),
                active.stroke,
                press,
            ),
            text: motion::lerp_color(
                motion::lerp_color(rest.text, hovered.text, hover),
                active.text,
                press,
            ),
        }
    } else {
        base
    };

    let radius = body_radius(tokens, tier, rect);
    if colors.fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, radius, colors.fill);
    }
    if colors.stroke != Color32::TRANSPARENT {
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(tokens.stroke.hairline, colors.stroke),
            StrokeKind::Inside,
        );
    }
    if response.has_focus() && enabled {
        ui.painter().rect_stroke(
            rect.expand(FOCUS_RING_EXPAND),
            CornerRadius::same(tokens.radius.control.saturating_add(2)),
            Stroke::new(tokens.stroke.focus, tokens.accent),
            StrokeKind::Outside,
        );
    }
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // M9c painted-bounds audit: every control body is on the record (no-op
    // unless `SURAMAK_BOUNDS_AUDIT`/the test seam enabled it).
    super::bounds::record(ui.ctx(), id, rect, tier_kind(tier));
    Painted { response, colors }
}

/// The audit's kind label for a tier.
fn tier_kind(tier: Tier) -> &'static str {
    match tier {
        Tier::Primary => "primary",
        Tier::Secondary => "secondary",
        Tier::Tertiary => "tertiary",
        Tier::Capsule => "capsule",
    }
}

/// A painted labeled button (secondary/primary: label + padding; tertiary: a
/// quiet label-only action).
pub fn button(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    id_salt: impl std::hash::Hash,
    label: &str,
    tier: Tier,
    enabled: bool,
) -> Response {
    let size = label_size(ui, tokens, label, tier);
    button_sized(ui, tokens, motion, id_salt, label, tier, enabled, size)
}

/// A painted labeled button at an exact designed size (the deck's anchors and
/// other fixed-geometry controls).
#[allow(clippy::too_many_arguments)]
pub fn button_sized(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    id_salt: impl std::hash::Hash,
    label: &str,
    tier: Tier,
    enabled: bool,
    size: egui::Vec2,
) -> Response {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let id = ui.id().with(id_salt);
    let painted = painted_body(ui, tokens, motion, id, rect, tier, enabled);
    // M9b a11y: the painted button carries its semantic label (screen reader +
    // tooltip from the same string).
    painted
        .response
        .widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), label_font(tier), painted.colors.text);
    // M9c painted-bounds invariant: a fixed-size control's label must fit it.
    debug_assert!(
        rect.expand(0.01)
            .contains_rect(Rect::from_center_size(rect.center(), galley.size())),
        "label {label:?} overflows its {size:?} body"
    );
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        painted.colors.text,
    );
    painted.response
}

/// The capsule body's designed height (owner direction: ~28–30 pt).
pub const CAPSULE_HEIGHT: f32 = 28.0;

/// The designed size of a capsule labeled button: the label plus ≥
/// `tokens.space.s` padding on each side, at the capsule height.
pub fn capsule_button_size(ui: &Ui, tokens: &Tokens, label: &str) -> egui::Vec2 {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        label_font(Tier::Capsule),
        Color32::PLACEHOLDER,
    );
    vec2(
        (galley.size().x + tokens.space.s * 2.0).max(CAPSULE_HEIGHT),
        CAPSULE_HEIGHT,
    )
}

/// The designed size of a capsule glyph button: the 16 pt glyph box plus ≥
/// `tokens.space.s` padding on each side, at the capsule height.
pub fn capsule_glyph_size(tokens: &Tokens) -> egui::Vec2 {
    vec2(16.0 + tokens.space.s * 2.0, CAPSULE_HEIGHT)
}

/// A capsule labeled button — the title-line cluster's "real button" treatment
/// (owner-directed): translucent body, hairline, capsule radius, hover/press at
/// the fast token.
pub fn capsule_button(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    id_salt: impl std::hash::Hash,
    label: &str,
    enabled: bool,
) -> Response {
    let size = capsule_button_size(ui, tokens, label);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let id = ui.id().with(id_salt);
    let painted = painted_body(ui, tokens, motion, id, rect, Tier::Capsule, enabled);
    painted
        .response
        .widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        label_font(Tier::Capsule),
        painted.colors.text,
    );
    // Padding invariant: the label sits inside the body with ≥ space.s to spare.
    debug_assert!(
        (rect.width() - galley.size().x) / 2.0 >= tokens.space.s - 0.01
            && rect
                .expand(0.01)
                .contains_rect(Rect::from_center_size(rect.center(), galley.size())),
        "capsule label {label:?} does not fit its {size:?} body with padding"
    );
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        painted.colors.text,
    );
    painted.response
}

/// A capsule glyph button (the title-line trash / confirm / cancel): the glyph
/// centred in a capsule body with ≥ `tokens.space.s` padding on each side.
pub fn capsule_glyph_button(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    id_salt: impl std::hash::Hash,
    glyph: Glyph,
    enabled: bool,
    tooltip: &str,
) -> Response {
    let size = capsule_glyph_size(tokens);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let id = ui.id().with(id_salt);
    let painted = painted_body(ui, tokens, motion, id, rect, Tier::Capsule, enabled);
    let glyph_rect = Rect::from_center_size(rect.center(), vec2(16.0, 16.0));
    // `paint_glyph` asserts the ink fits `glyph_rect`; the padding invariant is
    // asserted here (the glyph box inside the body with space.s to spare).
    let ink = paint_glyph(ui.painter(), glyph_rect, glyph, painted.colors.text);
    debug_assert!(
        (rect.width() - ink.width()) / 2.0 >= tokens.space.s - 0.1,
        "capsule glyph {glyph:?} has less than space.s padding in its {size:?} body"
    );
    painted
        .response
        .widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, tooltip));
    painted.response.on_hover_text(tooltip)
}

/// A painted glyph control (tertiary by default; tooltip attached).
///
/// Part of the control vocabulary; the M9c capsule treatment took its last
/// call site (the editor's title-line cluster), so it is kept for the
/// surfaces that still want a bare tertiary glyph.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub fn glyph_button(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    id_salt: impl std::hash::Hash,
    glyph: Glyph,
    tier: Tier,
    enabled: bool,
    tooltip: &str,
) -> Response {
    let side = 26.0_f32.max(tokens.space.xl + tokens.space.xs);
    let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
    let id = ui.id().with(id_salt);
    let painted = painted_body(ui, tokens, motion, id, rect, tier, enabled);
    paint_glyph(ui.painter(), rect, glyph, painted.colors.text);
    // M9b a11y: the glyph's meaning is the tooltip string — label and tooltip
    // come from the same text.
    painted
        .response
        .widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, tooltip));
    painted.response.on_hover_text(tooltip)
}

/// The label font for a tier (secondary/primary 13 pt; tertiary/capsule 12 pt).
fn label_font(tier: Tier) -> FontId {
    match tier {
        Tier::Primary | Tier::Secondary => FontId::new(13.0, FontFamily::Proportional),
        Tier::Tertiary | Tier::Capsule => FontId::new(12.0, FontFamily::Proportional),
    }
}

/// The measured size of a labeled button.
fn label_size(ui: &Ui, tokens: &Tokens, label: &str, tier: Tier) -> egui::Vec2 {
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), label_font(tier), Color32::PLACEHOLDER);
    if tier == Tier::Capsule {
        return capsule_button_size(ui, tokens, label);
    }
    let (pad_x, pad_y) = match tier {
        Tier::Primary => (tokens.space.l, tokens.space.s),
        Tier::Secondary => (tokens.space.m, tokens.space.xs + 2.0),
        Tier::Tertiary => (tokens.space.s, tokens.space.xs),
        Tier::Capsule => unreachable!("handled above"),
    };
    vec2(
        (galley.size().x + pad_x * 2.0).max(24.0),
        (galley.size().y + pad_y * 2.0).max(24.0),
    )
}

/// A glyph control plus label (the unified navigation pattern: chevron + word).
pub fn glyph_label_button(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    id_salt: impl std::hash::Hash,
    glyph: Glyph,
    label: &str,
    enabled: bool,
) -> Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::new(12.0, FontFamily::Proportional),
        Color32::PLACEHOLDER,
    );
    let glyph_w = 18.0;
    let size = vec2(
        (glyph_w + tokens.space.xs + galley.size().x + tokens.space.m * 2.0).max(24.0),
        (galley.size().y + tokens.space.s).max(24.0),
    );
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let id = ui.id().with(id_salt);
    let painted = painted_body(ui, tokens, motion, id, rect, Tier::Tertiary, enabled);
    painted
        .response
        .widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    let glyph_rect = Rect::from_center_size(
        pos2(
            rect.left() + tokens.space.m + glyph_w / 2.0,
            rect.center().y,
        ),
        vec2(glyph_w, glyph_w),
    );
    let ink = paint_glyph(ui.painter(), glyph_rect, glyph, painted.colors.text);
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::new(12.0, FontFamily::Proportional),
        painted.colors.text,
    );
    let galley_pos = pos2(
        glyph_rect.right() + tokens.space.xs,
        rect.center().y - galley.size().y / 2.0,
    );
    // M9c painted-bounds invariant: glyph ink + label stay inside the body.
    debug_assert!(
        rect.contains_rect(ink)
            && rect.contains_rect(Rect::from_min_size(galley_pos, galley.size())),
        "glyph+label control overflows its {size:?} body"
    );
    ui.painter().galley(galley_pos, galley, painted.colors.text);
    painted.response
}

/// The unified navigation control: `⌃ Home` in the same place on every screen.
pub fn nav_home(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    id_salt: impl std::hash::Hash,
) -> Response {
    glyph_label_button(
        ui,
        tokens,
        motion,
        id_salt,
        Glyph::ChevronLeft,
        "Home",
        true,
    )
}

/// A segmented control (secondary tier): one capsule, N quiet segments.
/// Returns true when the selection changed.
pub fn segmented<T: PartialEq + Clone>(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    id_salt: impl std::hash::Hash,
    current: &mut T,
    options: &[(T, &str)],
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = tokens.space.xs;
        for (index, (value, label)) in options.iter().enumerate() {
            let selected = *current == *value;
            let galley = ui.painter().layout_no_wrap(
                (*label).to_owned(),
                FontId::new(12.0, FontFamily::Proportional),
                Color32::PLACEHOLDER,
            );
            let size = vec2(
                (galley.size().x + tokens.space.m * 2.0).max(28.0),
                (galley.size().y + tokens.space.s).max(26.0),
            );
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let id = ui.id().with(&id_salt).with(index);
            let response = ui.interact(rect, id, Sense::click());
            // M9b a11y: every segment is a labeled radio node with its state.
            response.widget_info(|| {
                WidgetInfo::selected(WidgetType::RadioButton, true, selected, label)
            });
            let hover = motion.factor(ui.ctx(), id.with("hover"), response.hovered(), motion::FAST);
            // The selected segment carries the accent tint; the rest are quiet.
            // The selection itself crossfades with the fast token (a snap under
            // Reduce Motion) — a deliberate state change, not a teleport.
            let selected_f = motion.factor(ui.ctx(), id.with("selected"), selected, motion::FAST);
            let fill = motion::lerp_color(
                motion::lerp_color(Color32::TRANSPARENT, tokens.surface_hover, hover),
                motion::lerp_color(
                    tokens.accent_tint,
                    tokens.accent.gamma_multiply(0.55),
                    hover * 0.35,
                ),
                selected_f,
            );
            let text = motion::lerp_color(tokens.ink_soft, tokens.ink, selected_f);
            ui.painter()
                .rect_filled(rect, CornerRadius::same(tokens.radius.control), fill);
            if response.has_focus() {
                ui.painter().rect_stroke(
                    rect.expand(FOCUS_RING_EXPAND),
                    CornerRadius::same(tokens.radius.control.saturating_add(2)),
                    Stroke::new(tokens.stroke.focus, tokens.accent),
                    StrokeKind::Outside,
                );
            }
            let galley = ui.painter().layout_no_wrap(
                (*label).to_owned(),
                FontId::new(12.0, FontFamily::Proportional),
                text,
            );
            // M9c painted-bounds invariant: the segment label fits its segment.
            debug_assert!(
                rect.expand(0.01)
                    .contains_rect(Rect::from_center_size(rect.center(), galley.size())),
                "segment {label:?} overflows its {size:?} body"
            );
            ui.painter()
                .galley(rect.center() - galley.size() / 2.0, galley, text);
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            super::bounds::record(ui.ctx(), id, rect, "segment");
            if response.clicked() && !selected {
                *current = value.clone();
                changed = true;
            }
        }
    });
    changed
}

/// The transport capsule: skip-to-start + play/pause painted glyphs in one
/// compact secondary-tier capsule. Returns the two responses.
pub struct Transport {
    pub skip_start: Response,
    pub toggle: Response,
}

/// The capsule's designed size (the deck's left anchor — geometry is final).
pub const TRANSPORT_WIDTH: f32 = 76.0;
/// The capsule's designed height.
pub const TRANSPORT_HEIGHT: f32 = 34.0;

pub fn transport(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    id_salt: impl std::hash::Hash,
    playing: bool,
    enabled: bool,
) -> Transport {
    let size = vec2(TRANSPORT_WIDTH, TRANSPORT_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let id = ui.id().with(id_salt);
    // The capsule body: secondary rest surface + hairline.
    let colors = tier_colors(tokens, Tier::Secondary, State::Rest);
    ui.painter().rect_filled(
        rect,
        CornerRadius::same((rect.height() / 2.0) as u8),
        colors.fill,
    );
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same((rect.height() / 2.0) as u8),
        Stroke::new(tokens.stroke.hairline, colors.stroke),
        StrokeKind::Inside,
    );
    // The capsule body is a painted shape of this control — on the audit's
    // record alongside the two interactive halves.
    super::bounds::record(ui.ctx(), id.with("body"), rect, "transport");

    let half = rect.width() / 2.0;
    let skip_rect = Rect::from_min_size(rect.min, vec2(half, rect.height()));
    let toggle_rect = Rect::from_min_size(
        pos2(rect.left() + half, rect.top()),
        vec2(half, rect.height()),
    );
    let skip = painted_body(
        ui,
        tokens,
        motion,
        id.with("skip"),
        skip_rect.shrink(2.0),
        Tier::Tertiary,
        enabled,
    );
    paint_glyph(
        ui.painter(),
        skip_rect.shrink(2.0),
        Glyph::SkipStart,
        skip.colors.text,
    );
    let toggle = painted_body(
        ui,
        tokens,
        motion,
        id.with("toggle"),
        toggle_rect.shrink(2.0),
        Tier::Tertiary,
        enabled,
    );
    paint_glyph(
        ui.painter(),
        toggle_rect.shrink(2.0),
        if playing { Glyph::Pause } else { Glyph::Play },
        toggle.colors.text,
    );
    // M9b a11y: both capsule halves are labeled buttons; the toggle's label
    // follows its state (Play ↔ Pause).
    skip.response
        .widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, "Skip to start"));
    toggle.response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            enabled,
            if playing { "Pause" } else { "Play" },
        )
    });
    Transport {
        skip_start: skip.response.on_hover_text("Skip to start"),
        toggle: toggle
            .response
            .on_hover_text(if playing { "Pause" } else { "Play" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{tokens, AppTheme};

    #[test]
    fn state_selection_orders_disabled_press_hover_focus_rest() {
        assert_eq!(state_for(false, true, true, true), State::Disabled);
        assert_eq!(state_for(true, true, true, true), State::Active);
        assert_eq!(state_for(true, false, true, true), State::Hover);
        assert_eq!(state_for(true, false, false, true), State::Focus);
        assert_eq!(state_for(true, false, false, false), State::Rest);
    }

    #[test]
    fn every_tier_defines_every_state_with_distinct_deltas() {
        for theme in [AppTheme::Light, AppTheme::Dark] {
            let t = tokens(theme);
            for tier in [
                Tier::Primary,
                Tier::Secondary,
                Tier::Tertiary,
                Tier::Capsule,
            ] {
                let rest = tier_colors(&t, tier, State::Rest);
                let hover = tier_colors(&t, tier, State::Hover);
                let active = tier_colors(&t, tier, State::Active);
                let focus = tier_colors(&t, tier, State::Focus);
                let disabled = tier_colors(&t, tier, State::Disabled);
                assert_ne!(rest.fill, hover.fill, "{theme:?}/{tier:?} hover delta");
                assert_ne!(hover.fill, active.fill, "{theme:?}/{tier:?} active delta");
                // Focus is the accent ring — the one focus language, every tier.
                assert_eq!(
                    focus.stroke, t.accent,
                    "{theme:?}/{tier:?} focus ring is the accent"
                );
                assert_ne!(rest.stroke, focus.stroke, "{theme:?}/{tier:?} focus delta");
                assert!(
                    disabled.fill != rest.fill || disabled.text != rest.text,
                    "{theme:?}/{tier:?} disabled must be distinguishable from rest"
                );
                assert_ne!(rest.text, disabled.text, "{theme:?}/{tier:?} disabled text");
            }
            // The tier identities hold in both appearances.
            assert_eq!(
                tier_colors(&t, Tier::Primary, State::Rest).fill,
                t.accent,
                "{theme:?}: the primary tier IS the accent"
            );
            assert_eq!(
                tier_colors(&t, Tier::Tertiary, State::Rest).fill,
                Color32::TRANSPARENT,
                "{theme:?}: tertiary rests transparent"
            );
            assert_eq!(
                tier_colors(&t, Tier::Secondary, State::Rest).fill,
                t.control_fill
            );
            // The capsule rests on the translucent control fill with a hairline
            // — the pill's material family (owner direction).
            assert_eq!(
                tier_colors(&t, Tier::Capsule, State::Rest).fill,
                t.control_fill
            );
            assert_eq!(
                tier_colors(&t, Tier::Capsule, State::Rest).stroke,
                t.hairline
            );
        }
    }

    /// The capsule body (owner direction, 2026-10-14): ~28–30 pt tall, a real
    /// capsule radius, ≥ space.s padding around the glyph/label.
    #[test]
    fn capsule_bodies_are_padded_capsules_at_the_designed_height() {
        let ctx = egui::Context::default();
        let t = tokens(AppTheme::Light);
        let motion = Motion::new(false);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0))),
            ..Default::default()
        };
        let mut rects = Vec::new();
        let mut glyph_pads = Vec::new();
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let label = capsule_button(ui, &t, &motion, "cap-label", "Polish", true);
                    rects.push(label.rect);
                    let glyph = capsule_glyph_button(
                        ui,
                        &t,
                        &motion,
                        "cap-glyph",
                        Glyph::Trash,
                        true,
                        "Delete this transcript",
                    );
                    rects.push(glyph.rect);
                    glyph_pads.push((
                        glyph.rect.width(),
                        glyph_bounds(
                            Glyph::Trash,
                            Rect::from_center_size(glyph.rect.center(), vec2(16.0, 16.0)),
                        )
                        .width(),
                    ));
                });
        });
        for rect in &rects {
            assert!(
                (28.0..=30.0).contains(&rect.height()),
                "the capsule body is 28–30 pt tall: {rect:?}"
            );
        }
        // Label padding: width = label + 2 × space.s.
        let label_w = rects[0].width();
        assert!(
            label_w >= t.space.s * 2.0,
            "the labeled capsule carries its padding: {label_w}"
        );
        // Glyph padding: (body − ink) / 2 ≥ space.s − the stroke's half-pixel.
        let (body_w, ink_w) = glyph_pads[0];
        assert!(
            (body_w - ink_w) / 2.0 >= t.space.s - 0.1,
            "the glyph capsule carries ≥ space.s padding: body {body_w}, ink {ink_w}"
        );
        // The radius is the capsule's half-height.
        let radius = (rects[0].height() / 2.0).round() as u8;
        assert_eq!(radius, 14, "28 pt tall → 14 pt capsule radius");
    }

    /// The painted-bounds invariant at the source: every glyph's ink fits the
    /// rect a control allocates for it (26 pt tertiary, 16 pt capsule box).
    #[test]
    fn every_glyph_fits_its_control_rect() {
        for glyph in [
            Glyph::Play,
            Glyph::Pause,
            Glyph::SkipStart,
            Glyph::Trash,
            Glyph::Check,
            Glyph::Cross,
            Glyph::Stop,
            Glyph::ChevronLeft,
        ] {
            // The tertiary glyph button's body and the capsule glyph box.
            for side in [26.0_f32, 28.0, 16.0] {
                let rect = Rect::from_center_size(pos2(100.0, 100.0), vec2(side, side));
                let bounds = glyph_bounds(glyph, rect);
                assert!(
                    rect.expand(0.01).contains_rect(bounds),
                    "{glyph:?} ink {bounds:?} must fit a {side} pt box"
                );
            }
            // The pill's stop glyph draws in a 16 pt box.
            let pill = Rect::from_center_size(pos2(0.0, 0.0), vec2(16.0, 16.0));
            assert!(pill.contains_rect(glyph_bounds(Glyph::Stop, pill)));
        }
    }

    #[test]
    fn disabled_states_are_dimmer_than_rest() {
        let t = tokens(AppTheme::Dark);
        for tier in [Tier::Primary, Tier::Secondary, Tier::Tertiary] {
            let rest = tier_colors(&t, tier, State::Rest);
            let disabled = tier_colors(&t, tier, State::Disabled);
            let sum = |c: Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
            assert!(
                sum(disabled.text) < sum(rest.text),
                "{tier:?}: disabled text must be dimmer"
            );
        }
    }

    /// M9b a11y floor: every painted control type must land in the accesskit
    /// tree with its semantic label (and be focusable, i.e. Tab-reachable).
    #[test]
    fn every_painted_control_exposes_its_label_to_accesskit() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let t = tokens(AppTheme::Light);
        let motion = Motion::new(false);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0))),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = button(
                    ui,
                    &t,
                    &motion,
                    "a11y-secondary",
                    "Export…",
                    Tier::Secondary,
                    true,
                );
                let _ = glyph_button(
                    ui,
                    &t,
                    &motion,
                    "a11y-trash",
                    Glyph::Trash,
                    Tier::Tertiary,
                    true,
                    "Delete this transcript",
                );
                let mut selection = 0u8;
                let options = [(0u8, "Markdown"), (1u8, "Text")];
                let _ = segmented(ui, &t, &motion, "a11y-segments", &mut selection, &options);
                let _ = transport(ui, &t, &motion, "a11y-transport", false, true);
                let _ = nav_home(ui, &t, &motion, "a11y-home");
            });
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("an accesskit tree update");
        let labels: Vec<String> = update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label().map(str::to_string))
            .collect();
        for expected in [
            "Export…",
            "Delete this transcript",
            "Markdown",
            "Text",
            "Skip to start",
            "Play",
            "Home",
        ] {
            assert!(
                labels.iter().any(|label| label == expected),
                "missing a11y label {expected:?}; got {labels:?}"
            );
        }
    }
}
