//! Compact chrome: the phone's bands, rows, toggles and sheets — painted from
//! the same Deck tokens and motion as the desktop controls.
//!
//! Every tappable body is at least [`MIN_TOUCH`] (48 dp, Android's minimum touch
//! target) tall; the bands reserve room for the window insets the Kotlin shim
//! reports, so the system bars and the gesture handle never cover a control.

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Layout, Rect, Response,
    Stroke, StrokeKind, Ui, WidgetType,
};

use crate::deck::controls::{self, Glyph};
use crate::deck::motion::{self, Motion};
use crate::deck::theme::{Tokens, SERIF_FAMILY};

use super::a11y;

/// The minimum touch target (dp).
pub const MIN_TOUCH: f32 = 48.0;
/// The app bar's band height, insets excluded.
pub const APP_BAR: f32 = 56.0;
/// The bottom action bar's band height, insets excluded.
pub const BOTTOM_BAR: f32 = 68.0;
/// A list row's designed height.
pub const ROW: f32 = 64.0;
/// The content gutter.
pub const GUTTER: f32 = 16.0;

/// The three bands of a phone screen, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bands {
    /// System-bar inset above the app bar.
    pub top_inset: f32,
    /// The app bar itself.
    pub app_bar: Rect,
    /// Scrolling content between the bands.
    pub content: Rect,
    /// The bottom bar (zero-height when the screen has none).
    pub bottom_bar: Rect,
    /// System gesture inset below the bottom bar.
    pub bottom_inset: f32,
    /// The full screen.
    pub screen: Rect,
}

/// Splits a screen into chrome bands. `bottom` is `Some(height)` when the
/// screen has a bottom bar.
pub fn bands(screen: Rect, top_inset: f32, bottom_inset: f32, bottom: Option<f32>) -> Bands {
    let app_top = screen.top() + top_inset;
    let app_bar = Rect::from_min_size(pos2(screen.left(), app_top), vec2(screen.width(), APP_BAR));
    let bottom_height = bottom.unwrap_or(0.0);
    let bar_top = screen.bottom() - bottom_inset - bottom_height;
    let content = Rect::from_min_max(
        pos2(screen.left(), app_bar.bottom()),
        pos2(screen.right(), bar_top),
    );
    let bottom_bar = Rect::from_min_size(
        pos2(screen.left(), bar_top),
        vec2(screen.width(), bottom_height),
    );
    Bands {
        top_inset,
        app_bar,
        content,
        bottom_bar,
        bottom_inset,
        screen,
    }
}

/// The app bar: optional back chevron, a serif title, and one trailing text
/// action. Returns whether back was tapped.
pub fn app_bar(
    ui: &mut Ui,
    tokens: &Tokens,
    _motion: &Motion,
    rect: Rect,
    title: &str,
    back: bool,
    trailing: Option<(&str, bool)>,
) -> (bool, bool) {
    let mut back_hit = false;
    let mut trailing_hit = false;
    let painter = ui.painter().clone();
    painter.rect_filled(rect, CornerRadius::ZERO, tokens.window);
    let center_y = rect.center().y;
    if back {
        let hit = Rect::from_center_size(
            pos2(rect.left() + GUTTER + 12.0, center_y),
            vec2(48.0, 48.0),
        );
        let response = a11y::touch(
            ui,
            tokens,
            hit,
            ("compact-back", title),
            WidgetType::Button,
            "Back",
            true,
        );
        let color = if response.is_pointer_button_down_on() {
            tokens.accent_active
        } else if response.hovered() {
            tokens.accent_hover
        } else {
            tokens.ink
        };
        controls::paint_glyph(&painter, hit, Glyph::ChevronLeft, color);
        if response.clicked() {
            back_hit = true;
        }
    }
    let title_left = if back {
        rect.left() + 64.0
    } else {
        rect.left() + GUTTER
    };
    painter.text(
        pos2(title_left, center_y),
        Align2::LEFT_CENTER,
        title,
        FontId::new(20.0, FontFamily::Name(SERIF_FAMILY.into())),
        tokens.ink,
    );
    if let Some((label, enabled)) = trailing {
        let width = (ui.painter().layout_no_wrap(
            label.to_owned(),
            FontId::proportional(15.0),
            Color32::PLACEHOLDER,
        ))
        .size()
        .x
        .max(48.0)
            + 12.0;
        let hit = Rect::from_min_size(
            pos2(rect.right() - width - GUTTER + 12.0, center_y - 24.0),
            vec2(width, 48.0),
        );
        let response = a11y::touch(
            ui,
            tokens,
            hit,
            ("compact-trailing", title),
            WidgetType::Button,
            label,
            enabled,
        );
        let color = if !enabled {
            tokens.ink_faint.gamma_multiply(tokens.disabled_alpha)
        } else if response.hovered() {
            tokens.accent
        } else {
            tokens.ink
        };
        painter.text(
            hit.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(15.0),
            color,
        );
        if response.clicked() && enabled {
            trailing_hit = true;
        }
    }
    // The one hairline under the bar.
    painter.line_segment(
        [
            pos2(rect.left(), rect.bottom() - 0.5),
            pos2(rect.right(), rect.bottom() - 0.5),
        ],
        Stroke::new(1.0_f32, tokens.hairline),
    );
    (back_hit, trailing_hit)
}

/// A full-width tappable row: title + optional meta, optional selected state.
/// The body is `ROW` tall; the caller positions it.
#[allow(clippy::too_many_arguments)]
pub fn row(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    id: impl std::hash::Hash,
    title: &str,
    meta: &str,
    selected: bool,
) -> Response {
    let response = a11y::touch(ui, tokens, rect, id, WidgetType::Button, title, true);
    let fill = if selected {
        tokens.accent_tint
    } else if response.is_pointer_button_down_on() {
        tokens.surface_active
    } else if response.hovered() {
        tokens.surface_hover
    } else {
        Color32::TRANSPARENT
    };
    let wash = motion.factor(
        ui.ctx(),
        ui.id().with(("compact-row-wash", title)),
        selected || response.hovered(),
        motion::FAST,
    );
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(tokens.radius.row),
        fill.gamma_multiply(wash.max(if selected { 1.0 } else { 0.0 })),
    );
    let title_galley = ui.painter().layout(
        title.to_owned(),
        FontId::proportional(17.0),
        tokens.ink,
        (rect.width() - GUTTER * 2.0).max(40.0),
    );
    ui.painter().galley(
        pos2(rect.left() + GUTTER, rect.top() + 14.0),
        title_galley,
        tokens.ink,
    );
    if !meta.is_empty() {
        ui.painter().text(
            pos2(rect.left() + GUTTER, rect.bottom() - 16.0),
            Align2::LEFT_CENTER,
            meta,
            FontId::proportional(12.5),
            tokens.ink_soft,
        );
    }
    ui.painter().line_segment(
        [
            pos2(rect.left() + GUTTER, rect.bottom() - 0.5),
            pos2(rect.right(), rect.bottom() - 0.5),
        ],
        Stroke::new(1.0_f32, tokens.hairline),
    );
    response
}

/// A section label above a card.
pub fn section_label(ui: &mut Ui, tokens: &Tokens, pos: egui::Pos2, label: &str) {
    ui.painter().text(
        pos,
        Align2::LEFT_CENTER,
        label.to_uppercase(),
        FontId::proportional(11.5),
        tokens.ink_faint,
    );
}

/// A settings card body: one rounded card fill, caller draws the rows inside.
pub fn card(ui: &mut Ui, tokens: &Tokens, rect: Rect) {
    ui.painter().rect(
        rect,
        CornerRadius::same(tokens.radius.card),
        tokens.card,
        Stroke::new(tokens.stroke.hairline, tokens.hairline),
        StrokeKind::Inside,
    );
}

/// A settings row: label (and optional value/caption) with a trailing chevron,
/// toggle or action. Returns the row's response.
#[allow(clippy::too_many_arguments)]
pub fn setting_row(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    id: impl std::hash::Hash,
    label: &str,
    value: &str,
    enabled: bool,
) -> Response {
    let response = a11y::touch(ui, tokens, rect, id, WidgetType::Button, label, enabled);
    let wash = if response.is_pointer_button_down_on() {
        tokens.surface_active
    } else if response.hovered() && enabled {
        tokens.surface_hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::ZERO, wash);
    let ink = if enabled {
        tokens.ink
    } else {
        tokens.ink_faint
    };
    ui.painter().text(
        pos2(rect.left() + GUTTER, rect.center().y - 8.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(16.0),
        ink,
    );
    ui.painter().text(
        pos2(rect.left() + GUTTER, rect.center().y + 12.0),
        Align2::LEFT_CENTER,
        value,
        FontId::proportional(12.5),
        tokens.ink_soft,
    );
    if enabled {
        // A small right chevron: the row opens something.
        let c = pos2(rect.right() - GUTTER - 8.0, rect.center().y);
        ui.painter().add(egui::Shape::line(
            vec![
                pos2(c.x - 2.5, c.y - 5.0),
                pos2(c.x + 2.5, c.y),
                pos2(c.x - 2.5, c.y + 5.0),
            ],
            Stroke::new(1.6_f32, tokens.ink_faint),
        ));
    }
    let _ = motion;
    ui.painter().line_segment(
        [
            pos2(rect.left() + GUTTER, rect.bottom() - 0.5),
            pos2(rect.right(), rect.bottom() - 0.5),
        ],
        Stroke::new(1.0_f32, tokens.hairline),
    );
    response
}

/// A painted toggle (48 × 28 body inside a 48-tall hit area). Returns the hit
/// response; the knob crossfades with the fast token.
#[allow(clippy::too_many_arguments)]
pub fn toggle(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    center: egui::Pos2,
    id: impl std::hash::Hash,
    label: &str,
    on: bool,
) -> Response {
    let hit = Rect::from_center_size(center, vec2(56.0, MIN_TOUCH));
    let response = a11y::touch(ui, tokens, hit, &id, WidgetType::Checkbox, label, true);
    let track = Rect::from_center_size(center, vec2(46.0, 28.0));
    let t = motion.factor(
        ui.ctx(),
        ui.id().with(("compact-toggle", &id)),
        on,
        motion::FAST,
    );
    let fill = motion::lerp_color(tokens.control_fill, tokens.accent, t);
    ui.painter()
        .rect_filled(track, CornerRadius::same(14), fill);
    let knob_x = track.left() + 14.0 + (track.width() - 28.0) * t;
    ui.painter()
        .circle_filled(pos2(knob_x, track.center().y), 11.0, Color32::WHITE);
    response
}

/// A primary action button across a bar. `width` is the designed body width.
pub fn action_button(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    id: impl std::hash::Hash,
    label: &str,
    primary: bool,
) -> Response {
    let tier = if primary {
        controls::Tier::Primary
    } else {
        controls::Tier::Secondary
    };
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Center)),
    );
    controls::button_sized(
        &mut child,
        tokens,
        motion,
        id,
        label,
        tier,
        true,
        rect.size(),
    )
}

/// A text field sized for touch: 48 tall, panel fill, hairline.
pub fn search_field(
    ui: &mut Ui,
    tokens: &Tokens,
    rect: Rect,
    text: &mut String,
    hint: &str,
) -> bool {
    ui.painter().rect(
        rect,
        CornerRadius::same(tokens.radius.control),
        tokens.control_fill,
        Stroke::new(1.0_f32, tokens.hairline),
        StrokeKind::Inside,
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(12.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let response = child.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .frame(false)
            .desired_width(f32::INFINITY)
            .margin(vec2(0.0, 12.0)),
    );
    response.changed()
}

/// The modal sheet scrim + body. Returns the body rect.
///
/// `rise` moves the **body** only: at the start of the present animation the
/// body sits `rise` points below its resting place (partly past the screen
/// edge, clipped), so it slides up while the scrim already covers the whole
/// screen. Translating the scrim too would leave an unscrimmed strip at the
/// bottom for the length of the animation.
pub fn sheet(ui: &mut Ui, tokens: &Tokens, rect: Rect, rise: f32) -> Rect {
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, tokens.scrim);
    let height = (rect.height() * 0.46).clamp(240.0, 420.0);
    let body = Rect::from_min_max(
        pos2(rect.left(), rect.bottom() - height + rise),
        pos2(rect.right(), rect.bottom() + rise),
    );
    ui.painter().rect_filled(
        body,
        CornerRadius {
            nw: tokens.radius.card,
            ne: tokens.radius.card,
            sw: 0,
            se: 0,
        },
        tokens.sheet,
    );
    // The grabber.
    ui.painter().rect_filled(
        Rect::from_center_size(pos2(body.center().x, body.top() + 10.0), vec2(36.0, 4.0)),
        CornerRadius::same(2),
        tokens.hairline_strong,
    );
    body
}

/// A screen's static background.
pub fn background(ui: &mut Ui, tokens: &Tokens, rect: Rect) {
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, tokens.window);
    // The frosted treatment on Linux is the panel wash, not fake blur; the app
    // bar and cards sit on it (there is no real blur in egui).
    ui.painter().rect_filled(
        Rect::from_min_size(rect.min, vec2(rect.width(), APP_BAR)),
        CornerRadius::ZERO,
        tokens.panel_faint,
    );
}
