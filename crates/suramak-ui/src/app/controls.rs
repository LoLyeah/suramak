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
    StrokeKind, Ui,
};

use crate::motion::{self, Motion};
use crate::theme::Tokens;

/// The three control tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Accent-filled call to action.
    Primary,
    /// Labeled button on the card/control surface.
    Secondary,
    /// Quiet text or glyph action.
    Tertiary,
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

/// Paints `glyph` centred in `rect` in `color`.
pub fn paint_glyph(painter: &egui::Painter, rect: Rect, glyph: Glyph, color: Color32) {
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
                    pos2(center.x - 2.5, center.y - 7.5),
                    pos2(center.x + 2.5, center.y - 7.5),
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
}

/// The resolved interaction of a painted control (state + animated factors).
struct Painted {
    response: Response,
    colors: StateColors,
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

    let radius = CornerRadius::same(tokens.radius.control);
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
            rect.expand(1.5),
            CornerRadius::same(tokens.radius.control.saturating_add(2)),
            Stroke::new(tokens.stroke.focus, tokens.accent),
            StrokeKind::Outside,
        );
    }
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    Painted { response, colors }
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
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), label_font(tier), painted.colors.text);
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        painted.colors.text,
    );
    painted.response
}

/// A painted glyph control (tertiary by default; tooltip attached).
#[allow(clippy::too_many_arguments)]
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
    painted.response.on_hover_text(tooltip)
}

/// The label font for a tier (secondary/primary 13 pt; tertiary 12 pt).
fn label_font(tier: Tier) -> FontId {
    match tier {
        Tier::Primary | Tier::Secondary => FontId::new(13.0, FontFamily::Proportional),
        Tier::Tertiary => FontId::new(12.0, FontFamily::Proportional),
    }
}

/// The measured size of a labeled button.
fn label_size(ui: &Ui, tokens: &Tokens, label: &str, tier: Tier) -> egui::Vec2 {
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), label_font(tier), Color32::PLACEHOLDER);
    let (pad_x, pad_y) = match tier {
        Tier::Primary => (tokens.space.l, tokens.space.s),
        Tier::Secondary => (tokens.space.m, tokens.space.xs + 2.0),
        Tier::Tertiary => (tokens.space.s, tokens.space.xs),
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
    let glyph_rect = Rect::from_center_size(
        pos2(
            rect.left() + tokens.space.m + glyph_w / 2.0,
            rect.center().y,
        ),
        vec2(glyph_w, glyph_w),
    );
    paint_glyph(ui.painter(), glyph_rect, glyph, painted.colors.text);
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::new(12.0, FontFamily::Proportional),
        painted.colors.text,
    );
    ui.painter().galley(
        pos2(
            glyph_rect.right() + tokens.space.xs,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        painted.colors.text,
    );
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
            let hover = motion.factor(ui.ctx(), id.with("hover"), response.hovered(), motion::FAST);
            // The selected segment carries the accent tint; the rest are quiet.
            let fill = if selected {
                motion::lerp_color(
                    tokens.accent_tint,
                    tokens.accent.gamma_multiply(0.55),
                    hover * 0.35,
                )
            } else {
                motion::lerp_color(Color32::TRANSPARENT, tokens.surface_hover, hover)
            };
            let text = if selected {
                tokens.ink
            } else {
                tokens.ink_soft
            };
            ui.painter()
                .rect_filled(rect, CornerRadius::same(tokens.radius.control), fill);
            if response.has_focus() {
                ui.painter().rect_stroke(
                    rect.expand(1.5),
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
            ui.painter()
                .galley(rect.center() - galley.size() / 2.0, galley, text);
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
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
            for tier in [Tier::Primary, Tier::Secondary, Tier::Tertiary] {
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
}
