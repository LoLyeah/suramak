//! Compact chrome: the phone's bands, rows, toggles and sheets — painted from
//! the same Deck tokens and motion as the desktop controls.
//!
//! Every tappable body is at least [`MIN_TOUCH`] (48 dp, Android's minimum touch
//! target) tall; the bands reserve room for the window insets the Kotlin shim
//! reports, so the system bars and the gesture handle never cover a control.

use egui::{
    pos2, vec2, Align, Align2, Color32, Context, CornerRadius, FontId, Layout, Rect, Response,
    Stroke, StrokeKind, Ui, WidgetType,
};

use crate::deck::controls::{self, Glyph};
use crate::deck::icons::{self, Icon, IconSize};
use crate::deck::material::{self, Plane};
use crate::deck::motion::{self, Motion, PressFrame, PressMotion};
use crate::deck::theme::{self, Tokens};
use crate::deck::type_scale::{self, TypeToken};

use super::a11y;

/// The minimum touch target (dp).
pub const MIN_TOUCH: f32 = 48.0;
/// The app bar's band height, insets excluded.
pub const APP_BAR: f32 = 56.0;
/// The bottom action bar's band height, insets excluded (editor and Settings
/// still dock a bar; the Library floats a [`DOCK_HEIGHT`] plane instead).
pub const BOTTOM_BAR: f32 = 68.0;
/// A list row's designed height.
pub const ROW: f32 = 64.0;
/// The content gutter.
pub const GUTTER: f32 = 16.0;

/// The floating Library dock's height (8 pt padding + 48 pt buttons + 8 pt).
pub const DOCK_HEIGHT: f32 = 64.0;
/// The dock's horizontal inset from the screen edge.
pub const DOCK_INSET: f32 = 16.0;
/// The dock's gap above the system gesture inset.
pub const DOCK_GAP: f32 = 12.0;
/// The dock's internal padding around its two pills.
pub const DOCK_PADDING: f32 = 8.0;

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

/// The app bar's serif title, at the type scale's **title** token: the house
/// voice keeps its face, but its size, tracking and line height come from the
/// scale (22 / −0.4 / 28) rather than a per-screen number.
pub fn title_token() -> TypeToken {
    let mut token = type_scale::title();
    token.font.family = theme::serif_family();
    token
}

/// What the app bar's trailing slot shows. The Library uses [`Trailing::Icon`]
/// with the gear; the other phone screens still pass a text action.
#[derive(Debug, Clone, Copy)]
pub enum Trailing<'a> {
    /// Nothing on the right.
    Nothing,
    /// A text action (Settings' and the editor's current form).
    Text { label: &'a str, enabled: bool },
    /// A chrome icon with its spoken label in a 44 × 44 pt hit box.
    Icon {
        icon: Icon,
        label: &'a str,
        enabled: bool,
    },
}

/// The app bar: optional back chevron, a serif title at the title token, and
/// one trailing action. Returns whether back / the trailing action was tapped.
pub fn app_bar(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    title: &str,
    back: bool,
    trailing: Option<(&str, bool)>,
) -> (bool, bool) {
    let trailing = match trailing {
        Some((label, enabled)) => Trailing::Text { label, enabled },
        None => Trailing::Nothing,
    };
    let (back_hit, trailing_hit, _) = app_bar_with(ui, tokens, motion, rect, title, back, trailing);
    (back_hit, trailing_hit)
}

/// [`app_bar`] with an explicit trailing slot — the M20C library passes a gear.
pub fn app_bar_with(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    title: &str,
    back: bool,
    trailing: Trailing<'_>,
) -> (bool, bool, Option<egui::Pos2>) {
    let mut back_hit = false;
    let mut trailing_hit = false;
    // Where the trailing control was drawn (the Android smoke taps it).
    let mut anchor: Option<egui::Pos2> = None;
    let _ = motion;
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

    // The trailing control first, so the title knows how much room is left.
    let mut title_right = rect.right() - GUTTER;
    match trailing {
        Trailing::Nothing => {}
        Trailing::Text { label, enabled } => {
            let width = (ui
                .painter()
                .layout_no_wrap(
                    label.to_owned(),
                    FontId::proportional(15.0),
                    Color32::PLACEHOLDER,
                )
                .size()
                .x)
                .max(48.0)
                + 12.0;
            let hit = Rect::from_min_size(
                pos2(rect.right() - width - GUTTER + 12.0, center_y - 24.0),
                vec2(width, 48.0),
            );
            title_right = hit.left() - 8.0;
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
        Trailing::Icon {
            icon,
            label,
            enabled,
        } => {
            let hit = icons::hit_rect(pos2(rect.right() - GUTTER - icons::HIT / 2.0, center_y));
            anchor = Some(hit.center());
            title_right = hit.left() - 8.0;
            let response = icons::button(
                ui,
                tokens,
                hit,
                ("compact-gear", title),
                icon,
                label,
                IconSize::Chrome,
                enabled,
            );
            if response.clicked() && enabled {
                trailing_hit = true;
            }
        }
    }

    paint_title(
        ui,
        tokens,
        title,
        Rect::from_min_max(
            pos2(title_left, rect.top() + 4.0),
            pos2(title_right.max(title_left + 24.0), rect.bottom() - 4.0),
        ),
    );

    // The one hairline under the bar.
    painter.line_segment(
        [
            pos2(rect.left(), rect.bottom() - 0.5),
            pos2(rect.right(), rect.bottom() - 0.5),
        ],
        Stroke::new(1.0_f32, tokens.hairline),
    );
    (back_hit, trailing_hit, anchor)
}

/// Paints the app-bar title inside `band`, one line, elided with `…` when the
/// transcript title runs long. Painted through a clip so a title can never
/// spill over the trailing control.
fn paint_title(ui: &Ui, tokens: &Tokens, title: &str, band: Rect) {
    let token = title_token();
    let mut job = egui::text::LayoutJob::default();
    job.append(title, 0.0, token.format(tokens.ink));
    job.wrap.max_width = band.width().max(24.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let galley = ui.painter().layout_job(job);
    let y = band.center().y - galley.size().y / 2.0;
    ui.painter()
        .with_clip_rect(band)
        .galley(pos2(band.left(), y), galley, tokens.ink);
}

/// M22C: the extra anatomy a library row carries. All three are display data:
/// no decode happens here, and an empty waveform is drawn as an honest flat
/// baseline rather than invented height.
#[derive(Debug, Clone, Copy)]
pub struct RowExtras<'a> {
    /// Down-sampled RMS bars in `0..=1` (the store's stored peaks).
    pub waveform: &'a [f32],
    /// Speaker labels the transcript carries (`0` = no pips).
    pub speakers: usize,
    /// The transcript's duration, painted on the tabular grid.
    pub duration_ms: i64,
}

impl RowExtras<'_> {
    /// A row with no facets (kept for callers that do not have them).
    pub const NONE: RowExtras<'static> = RowExtras {
        waveform: &[],
        speakers: 0,
        duration_ms: 0,
    };
}

/// A full-width tappable row: title + optional meta, optional selected state.
/// The body is `ROW` tall; the caller positions it.
///
/// The row wears the type scale ([`type_scale::row_title`] /
/// [`type_scale::caption`]), separates with an inset 1 px hairline, and presses
/// with the M20A motion (`FAST` in, spring out): the whole painted body scales
/// to [`motion::PRESS_SCALE`] while the finger is down and the active wash
/// ramps with it. The hit rect never scales — touch stays 64 pt.
///
/// M22C adds the row's anatomy: a 28 px mini-waveform in the leading gutter,
/// up to four speaker pips (then `+N`) on the meta line, and the duration on
/// the tabular numeral grid at the trailing edge. Every extra maps through the
/// press scale with the body, so the M20C motion is unchanged.
///
/// Returns the response and the painted scale (1.0 at rest) so the caller can
/// keep its own affordances (the play triangle, the chevron) on the same body.
#[allow(clippy::too_many_arguments)]
pub fn row(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    id: impl std::hash::Hash,
    title: &str,
    meta: &str,
    extras: RowExtras<'_>,
    selected: bool,
    divider: bool,
) -> (Response, f32) {
    let response = a11y::touch(ui, tokens, rect, &id, WidgetType::Button, title, true);
    let base = ui.id().with(&id);
    let press = press_frame(
        ui.ctx(),
        base.with("press"),
        response.is_pointer_button_down_on(),
        motion.reduce,
    );
    let scale = press.scale;
    let visual = Rect::from_center_size(rect.center(), rect.size() * scale);
    let radius = CornerRadius::same((f32::from(tokens.radius.row) * scale).round() as u8);

    let wash = motion.factor(
        ui.ctx(),
        base.with("wash"),
        selected || response.hovered(),
        motion::FAST,
    );
    if selected {
        ui.painter().rect_filled(visual, radius, tokens.accent_tint);
    } else if wash > 0.0 {
        ui.painter().rect_filled(
            visual,
            radius,
            tokens.surface_hover.gamma_multiply(wash.clamp(0.0, 1.0)),
        );
    }
    if press.fill > 0.0 {
        ui.painter().rect_filled(
            visual,
            radius,
            tokens.surface_active.gamma_multiply(press.fill),
        );
    }

    // --- M22C: the 28 px mini-waveform -------------------------------------
    let mark = Rect::from_min_size(
        pos2(
            visual.left() + GUTTER * scale,
            visual.center().y - 14.0 * scale,
        ),
        vec2(28.0 * scale, 28.0 * scale),
    );
    paint_waveform(ui.painter(), mark, extras.waveform, tokens.ink_faint);

    // The title and meta start after the mark; the trailing 96 pt stays the
    // play/chevron lane (`chrome::row`'s pre-M22C reserve).
    let text_left = GUTTER * scale + 40.0 * scale;
    let trailing = 96.0 * scale;
    let title_token = scaled(&type_scale::row_title(), scale);
    let mut job = egui::text::LayoutJob::default();
    job.append(title, 0.0, title_token.format(tokens.ink));
    job.wrap.max_width = (visual.width() - text_left - trailing).max(40.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    ui.painter().galley(
        pos2(visual.left() + text_left, visual.top() + 12.0 * scale),
        ui.painter().layout_job(job),
        tokens.ink,
    );

    // Duration: right-aligned on the title line, in the tabular face so a
    // column of rows never twitches (M22C).
    if extras.duration_ms > 0 {
        let numerals = scaled(&type_scale::numerals(), scale);
        type_scale::paint_tabular(
            ui.painter(),
            pos2(
                visual.right() - trailing,
                visual.top() + 12.0 * scale + numerals.line_height / 2.0,
            ),
            Align2::RIGHT_CENTER,
            &crate::deck::compact::model::clock(extras.duration_ms),
            &numerals.font,
            tokens.ink_soft,
        );
    }

    // Pips: up to four speaker hues, then `+N`. They sit at the start of the
    // meta line, before the date.
    let mut meta_left = visual.left() + text_left;
    let pip_diameter = 6.0 * scale;
    let pip_gap = 9.0 * scale;
    let shown = extras.speakers.min(4);
    for index in 0..shown {
        let center = pos2(
            meta_left + pip_diameter / 2.0 + index as f32 * pip_gap,
            visual.top() + 36.0 * scale + 8.0 * scale,
        );
        ui.painter().circle_filled(
            center,
            pip_diameter / 2.0,
            tokens.speaker[index % tokens.speaker.len()],
        );
    }
    if extras.speakers > 4 {
        let more = format!("+{}", extras.speakers - 4);
        let caption = scaled(&type_scale::caption(), scale);
        ui.painter().text(
            pos2(
                meta_left + shown as f32 * pip_gap + 1.0 * scale,
                visual.top() + 36.0 * scale + 8.0 * scale,
            ),
            Align2::LEFT_CENTER,
            more,
            caption.font.clone(),
            tokens.ink_soft,
        );
        meta_left += shown as f32 * pip_gap + caption.size() + 8.0 * scale;
    } else if shown > 0 {
        meta_left += shown as f32 * pip_gap + 4.0 * scale;
    }

    if !meta.is_empty() {
        // The pips are a separate fact from the date; a middle dot keeps the
        // overflow (`+1`) from reading as part of `12 Nov` (M22C).
        let meta_text = if extras.speakers > 0 {
            format!("· {meta}")
        } else {
            meta.to_string()
        };
        let meta_token = scaled(&type_scale::caption(), scale);
        let mut job = egui::text::LayoutJob::default();
        job.append(&meta_text, 0.0, meta_token.format(tokens.ink_soft));
        job.wrap.max_width = (visual.right() - trailing - meta_left).max(40.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        ui.painter().galley(
            pos2(meta_left, visual.top() + 36.0 * scale),
            ui.painter().layout_job(job),
            tokens.ink_soft,
        );
    }
    if divider {
        ui.painter().line_segment(
            [
                pos2(rect.left() + GUTTER, rect.bottom() - 0.5),
                pos2(rect.right() - GUTTER, rect.bottom() - 0.5),
            ],
            Stroke::new(1.0_f32, tokens.hairline),
        );
    }
    (response, scale)
}

/// Paints `bars` as a quiet 9-column mark inside `rect`; an empty slice draws
/// the one-line baseline (no stored peaks, never invented height).
fn paint_waveform(painter: &egui::Painter, rect: Rect, bars: &[f32], color: Color32) {
    let columns = 9usize;
    let gap = (rect.width() * 0.035).max(1.0);
    let bar_w = ((rect.width() - gap * (columns as f32 - 1.0)) / columns as f32).max(1.0);
    let baseline_y = rect.center().y + rect.height() * 0.5 - 1.0;
    if bars.is_empty() {
        painter.line_segment(
            [
                pos2(rect.left(), baseline_y),
                pos2(rect.right(), baseline_y),
            ],
            Stroke::new(1.0_f32, color.gamma_multiply(0.5)),
        );
        return;
    }
    for index in 0..columns {
        // Sample the stored bars evenly; the last column takes the tail.
        let start = index * bars.len() / columns;
        let end = ((index + 1) * bars.len() / columns)
            .max(start + 1)
            .min(bars.len());
        let value = bars[start..end]
            .iter()
            .copied()
            .fold(0.0f32, f32::max)
            .clamp(0.0, 1.0);
        let height = (rect.height() - 2.0) * value;
        let x = rect.left() + index as f32 * (bar_w + gap);
        let top = baseline_y - height.max(1.0);
        painter.rect_filled(
            Rect::from_min_max(pos2(x, top), pos2(x + bar_w, baseline_y)),
            CornerRadius::same(1),
            color,
        );
    }
}

/// The row's press state, kept across frames in `ctx.data`: `FAST` down,
/// [`motion::SPRING_DURATION`] spring back. Reduce Motion keeps the wash and
/// drops the scale (the plan's "fill swap only").
#[derive(Clone, Copy)]
struct RowPress {
    pressed: bool,
    motion: PressMotion,
}

/// The row/button press frame for `id`: `FAST` down, [`motion::SPRING_DURATION`]
/// spring back, Reduce Motion keeping only the wash. Shared by the Library
/// rows, the dock and the M20D Settings rows (which paint their own multi-line
/// bodies).
pub(crate) fn press_frame(ctx: &Context, id: egui::Id, pressed: bool, reduce: bool) -> PressFrame {
    let now = ctx.input(|input| input.time);
    let mut state = ctx.data_mut(|data| {
        data.get_temp::<RowPress>(id).unwrap_or(RowPress {
            pressed: false,
            motion: PressMotion::new(reduce),
        })
    });
    if state.pressed != pressed {
        state.pressed = pressed;
        state.motion.set_pressed(pressed, now);
    }
    let frame = state.motion.sample(now);
    ctx.data_mut(|data| data.insert_temp(id, state));
    if frame.running {
        ctx.request_repaint();
    }
    frame
}

/// A type token scaled about its size and line height (the press's 0.97 body).
pub(crate) fn scaled(token: &TypeToken, scale: f32) -> TypeToken {
    if (scale - 1.0).abs() < f32::EPSILON {
        return token.clone();
    }
    let mut token = token.clone();
    token.font.size *= scale;
    token.line_height *= scale;
    token.tracking *= scale;
    token
}

/// A section label above a card: the type scale's **section label** role
/// (Inter SemiBold 11 / +0.8 / 16), set in CAPS by this call, in `ink_soft`.
///
/// `pos` is the label's **top-left**, not its centre: the M20D rhythm lays
/// every section out as label (16) → 8 → card, and a centre-aligned call would
/// force every caller to re-derive the half-height.
pub fn section_label(ui: &mut Ui, tokens: &Tokens, pos: egui::Pos2, label: &str) {
    let token = type_scale::section_label();
    // `ink_soft`, not `ink_faint`: 11 pt text needs 4.5:1, and the light
    // ladder's faint (#8a92a2 on #e9ebf0) is 2.6:1 — a label nobody can read
    // is not "quiet". Size and CAPS carry the quiet, not a failing alpha.
    ui.painter().text(
        pos,
        Align2::LEFT_TOP,
        label.to_uppercase(),
        token.font,
        tokens.ink_soft,
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
    setting_row_with_ink(ui, tokens, motion, rect, id, label, value, enabled, None)
}

/// [`setting_row`] whose value line takes an explicit ink (M19B: the model row
/// tints `small · not downloaded` with the warning colour instead of painting
/// a reassuring grey).
#[allow(clippy::too_many_arguments)]
pub fn setting_row_with_ink(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    id: impl std::hash::Hash,
    label: &str,
    value: &str,
    enabled: bool,
    value_ink: Option<Color32>,
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
        value_ink.unwrap_or(tokens.ink_soft),
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
///
/// Pressed with the control kit's spring: the body scales to
/// [`motion::PRESS_SCALE`] on `FAST` and returns over `SPRING_DURATION`, while
/// the tier's fill crossfades to its active tone. The hit rect never scales.
pub fn action_button(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    id: impl std::hash::Hash,
    label: &str,
    primary: bool,
) -> Response {
    let response = a11y::touch(ui, tokens, rect, &id, WidgetType::Button, label, true);
    let base = ui.id().with(&id);
    let press = press_frame(
        ui.ctx(),
        base.with("press"),
        response.is_pointer_button_down_on(),
        motion.reduce,
    );
    let hover = motion.factor(
        ui.ctx(),
        base.with("hover"),
        response.hovered(),
        motion::FAST,
    );
    let scale = press.scale;
    let visual = Rect::from_center_size(rect.center(), rect.size() * scale);
    let radius = CornerRadius::same((f32::from(tokens.radius.control) * scale).round() as u8);

    let (rest, hovered, active, ink) = if primary {
        (
            tokens.accent,
            tokens.accent_hover,
            tokens.accent_active,
            tokens.on_accent,
        )
    } else {
        (
            tokens.control_fill,
            tokens.control_fill_hover,
            tokens.surface_active,
            tokens.ink,
        )
    };
    let fill = motion::lerp_color(motion::lerp_color(rest, hovered, hover), active, press.fill);
    ui.painter().rect_filled(visual, radius, fill);
    if !primary {
        ui.painter().rect_stroke(
            visual,
            radius,
            Stroke::new(1.0_f32, tokens.hairline),
            StrokeKind::Inside,
        );
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            visual.expand(2.0),
            CornerRadius::same(tokens.radius.control.saturating_add(2)),
            Stroke::new(tokens.stroke.focus, tokens.accent),
            StrokeKind::Outside,
        );
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let token = scaled(&type_scale::row_title(), scale);
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), token.font.clone(), Color32::PLACEHOLDER);
    ui.painter().galley(
        pos2(
            visual.center().x - galley.size().x / 2.0,
            visual.center().y - galley.size().y / 2.0,
        ),
        galley,
        ink,
    );
    response
}

/// The search field's caption token (the placeholder's face and size).
pub fn search_hint_token() -> TypeToken {
    type_scale::caption()
}

/// A text field sized for touch: 48 tall, a **frosted Card plane** (veil + rim,
/// no shadow — it sits in the scroll plane), the 18 pt magnifier inline at the
/// leading edge, and the caption token's placeholder. Entered text uses the
/// body token so a query reads at speaking size.
pub fn search_field(
    ui: &mut Ui,
    tokens: &Tokens,
    rect: Rect,
    text: &mut String,
    hint: &str,
) -> bool {
    material::frosted_panel(ui, tokens, rect, tokens.radius.control, Plane::Card);
    let icon = Rect::from_center_size(
        pos2(rect.left() + 14.0 + icons::INLINE / 2.0, rect.center().y),
        vec2(icons::INLINE, icons::INLINE),
    );
    icons::paint(
        ui.painter(),
        icon,
        Icon::Search,
        IconSize::Inline.points(),
        tokens.ink_faint,
    );
    let field = Rect::from_min_max(
        pos2(icon.right() + 10.0, rect.top() + 6.0),
        pos2(rect.right() - 12.0, rect.bottom() - 6.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(field)
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.visuals_mut().text_cursor.stroke = Stroke::new(1.5_f32, tokens.accent);
    let response = child.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .hint_text_font(search_hint_token().font)
            .font(type_scale::body().font)
            .frame(false)
            .desired_width(f32::INFINITY)
            .margin(vec2(0.0, 0.0)),
    );
    response.changed()
}

/// The modal sheet scrim + body. Returns the body rect.
///
/// `slide` is the present progress, 0..=1: 0 is the body fully below the
/// screen edge, 1 is resting. The plan's sheet motion is `translateY(100% → 0)`
/// with the scrim already covering the page, so the scrim is painted at the
/// full screen and only the **body** travels. Reduce Motion passes 1.0 (the
/// fade is the whole transition).
///
/// The body is the [`Plane::Sheet`] material: opaque sheet fill, veil, grain,
/// the one rim and the one float shadow. Its top corners round; the bottom is
/// the screen edge.
pub fn sheet(ui: &mut Ui, tokens: &Tokens, rect: Rect, slide: f32) -> Rect {
    let height = (rect.height() * 0.46).clamp(240.0, 420.0);
    sheet_sized(ui, tokens, rect, slide, height)
}

/// The same Sheet plane at an explicit height (M23A: the speaker surfaces and
/// the taller export sheet). Never taller than the screen.
pub fn sheet_sized(ui: &mut Ui, tokens: &Tokens, rect: Rect, slide: f32, height: f32) -> Rect {
    material::scrim(ui, rect, tokens);
    let height = height.clamp(160.0, rect.height());
    let progress = slide.clamp(0.0, 1.0);
    let body = Rect::from_min_max(
        pos2(rect.left(), rect.bottom() - height * progress),
        pos2(rect.right(), rect.bottom() + height * (1.0 - progress)),
    );
    material::frosted_panel(
        ui,
        tokens,
        body,
        CornerRadius {
            nw: tokens.radius.card,
            ne: tokens.radius.card,
            sw: 0,
            se: 0,
        },
        Plane::Sheet,
    );
    // The grabber.
    ui.painter().rect_filled(
        Rect::from_center_size(pos2(body.center().x, body.top() + 10.0), vec2(36.0, 4.0)),
        CornerRadius::same(2),
        tokens.hairline_strong,
    );
    body
}

/// The M22B micro-sheet: the same Sheet material and scrim as [`sheet`], with
/// the voice-note sheet's fixed height. A glance, not a page — the body is
/// [`super::voice_note::MICRO_SHEET_HEIGHT`] tall and the drag handle stays.
pub fn micro_sheet(ui: &mut Ui, tokens: &Tokens, rect: Rect, slide: f32) -> Rect {
    material::scrim(ui, rect, tokens);
    let height = super::voice_note::MICRO_SHEET_HEIGHT.min(rect.height());
    let progress = slide.clamp(0.0, 1.0);
    let body = Rect::from_min_max(
        pos2(rect.left(), rect.bottom() - height * progress),
        pos2(rect.right(), rect.bottom() + height * (1.0 - progress)),
    );
    material::frosted_panel(
        ui,
        tokens,
        body,
        CornerRadius {
            nw: tokens.radius.card,
            ne: tokens.radius.card,
            sw: 0,
            se: 0,
        },
        Plane::Sheet,
    );
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

// ---- M20C: the floating dock ---------------------------------------------

/// The floating dock's rect: [`DOCK_INSET`] in from both edges,
/// [`DOCK_HEIGHT`] tall, [`DOCK_GAP`] above the gesture inset. It floats OVER
/// the list; the list's bottom padding has to clear it (see
/// `library::list_bottom_padding`).
pub fn dock_rect(screen: Rect, bottom_inset: f32) -> Rect {
    let width = (screen.width() - DOCK_INSET * 2.0).max(0.0);
    Rect::from_min_size(
        pos2(
            screen.left() + DOCK_INSET,
            screen.bottom() - bottom_inset - DOCK_GAP - DOCK_HEIGHT,
        ),
        vec2(width, DOCK_HEIGHT),
    )
}

/// The dock's two pills, left to right: Import at 38 % of the inner width, the
/// Record primary taking the rest (the plan's 38 / 62 split).
pub fn dock_action_rects(dock: Rect) -> [Rect; 2] {
    let inner = dock.shrink(DOCK_PADDING);
    let gap = 10.0;
    let usable = (inner.width() - gap).max(0.0);
    let import = Rect::from_min_size(inner.min, vec2(usable * 0.38, inner.height()));
    let record = Rect::from_min_max(pos2(import.right() + gap, inner.top()), inner.max);
    [import, record]
}

/// The Record pill's fill: the record red, deepened to the text-safe red only
/// where the ladder's own ink would fall under the 4.5:1 small-text floor on it
/// (the light ladder's `#E0352B` is 4.46:1 with white; `#C8281E` is 5.56:1).
/// The dot/fill stays unmistakably the record red either way.
pub fn record_fill(tokens: &Tokens) -> Color32 {
    if theme::contrast_ratio(tokens.on_accent, tokens.record_red) >= 4.5 {
        tokens.record_red
    } else {
        tokens.record_red_text
    }
}

/// One dock pill: 48 pt tall inside the frosted dock, an optional 20 pt icon,
/// the type scale's row title, and the M20A press (FAST in, spring out).
///
/// `primary` paints the Record fill ([`record_fill`]) with `on_accent` ink;
/// otherwise the secondary `control_fill` + hairline with `ink`. `spoken` is
/// the TalkBack name (the visible label stays short).
// The dock pill is one call site with a fixed geometry; a parameter struct
// would only move the same ten fields, exactly like the other chrome builders.
#[allow(clippy::too_many_arguments)]
pub fn dock_button(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    id: impl std::hash::Hash,
    label: &str,
    spoken: &str,
    icon: Option<Icon>,
    primary: bool,
    enabled: bool,
) -> Response {
    filled_button(
        ui, tokens, motion, rect, id, label, spoken, icon, primary, enabled, false,
    )
}

/// M22B — a primary action in the app's accent, not the Record red. The
/// voice-note sheet's one action is "Transcribe"; the record red belongs to the
/// mic and would read as a second Record button.
#[allow(clippy::too_many_arguments)]
pub fn accent_button(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    id: impl std::hash::Hash,
    label: &str,
    spoken: &str,
    icon: Option<Icon>,
    enabled: bool,
) -> Response {
    filled_button(
        ui, tokens, motion, rect, id, label, spoken, icon, true, enabled, true,
    )
}

#[allow(clippy::too_many_arguments)]
fn filled_button(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    rect: Rect,
    id: impl std::hash::Hash,
    label: &str,
    spoken: &str,
    icon: Option<Icon>,
    primary: bool,
    enabled: bool,
    accent: bool,
) -> Response {
    let response = a11y::touch(ui, tokens, rect, &id, WidgetType::Button, spoken, enabled);
    let base = ui.id().with(&id);
    let press = press_frame(
        ui.ctx(),
        base.with("press"),
        enabled && response.is_pointer_button_down_on(),
        motion.reduce,
    );
    let hover = motion.factor(
        ui.ctx(),
        base.with("hover"),
        enabled && response.hovered(),
        motion::FAST,
    );
    let scale = press.scale;
    let visual = Rect::from_center_size(rect.center(), rect.size() * scale);
    let radius = CornerRadius::same((f32::from(tokens.radius.control) * scale).round() as u8);

    let (fill, ink) = if !enabled {
        (tokens.control_fill, tokens.ink_faint)
    } else if accent {
        (tokens.accent, tokens.on_accent)
    } else if primary {
        (record_fill(tokens), tokens.on_accent)
    } else {
        (
            motion::lerp_color(tokens.control_fill, tokens.control_fill_hover, hover),
            tokens.ink,
        )
    };
    ui.painter().rect_filled(visual, radius, fill);
    if !enabled || !(primary || accent) {
        ui.painter().rect_stroke(
            visual,
            radius,
            Stroke::new(1.0_f32, tokens.hairline),
            StrokeKind::Inside,
        );
    }
    if press.fill > 0.0 && enabled {
        // The press wash rides the pill's own ink so it reads on both tiers.
        ui.painter()
            .rect_filled(visual, radius, ink.gamma_multiply(0.10 * press.fill));
    }

    let token = scaled(&type_scale::row_title(), scale);
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), token.font.clone(), Color32::PLACEHOLDER);
    let icon_size = icons::CHROME * scale;
    let gap = 6.0 * scale;
    let total = if icon.is_some() {
        icon_size + gap + galley.size().x
    } else {
        galley.size().x
    };
    let start = visual.center().x - total / 2.0;
    if let Some(icon) = icon {
        let icon_rect = Rect::from_center_size(
            pos2(start + icon_size / 2.0, visual.center().y),
            vec2(icon_size, icon_size),
        );
        icons::paint(ui.painter(), icon_rect, icon, icon_size, ink);
    }
    let text_left = if icon.is_some() {
        start + icon_size + gap
    } else {
        start
    };
    ui.painter().galley(
        pos2(text_left, visual.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::theme::{self, AppTheme};

    /// The row/button press is the M20A grammar: `FAST` down to the 0.97 body,
    /// `SPRING_DURATION` back out. The helper is driven directly so the test
    /// sees the same values the painter uses.
    #[test]
    fn the_press_is_fast_in_and_springs_out() {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        theme::install(&ctx, AppTheme::Dark, false);
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 200.0));
        let id = egui::Id::new("press-test");
        let at = |ctx: &egui::Context, time: f64, pressed: bool| {
            let mut frame = None;
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..Default::default()
            };
            let _ = ctx.run(raw, |_| {
                frame = Some(press_frame(ctx, id, pressed, false));
            });
            frame.expect("one frame")
        };

        // Frame one arms the state at rest.
        assert_eq!(at(&ctx, 0.0, true).scale, 1.0);
        let mid = at(&ctx, f64::from(motion::FAST) / 2.0, true).scale;
        assert!(
            mid < 0.995 && mid > motion::PRESS_SCALE,
            "halfway through FAST the body is between rest and pressed: {mid}"
        );
        let down = at(&ctx, f64::from(motion::FAST), true).scale;
        assert!(
            (down - motion::PRESS_SCALE).abs() < 0.001,
            "at the FAST token the body is pressed: {down}"
        );

        // Release: the spring starts and lands exactly on 1.0.
        let _ = at(&ctx, f64::from(motion::FAST) + 0.001, false);
        let rising = at(&ctx, f64::from(motion::FAST) + 0.08, false).scale;
        assert!(
            rising > motion::PRESS_SCALE,
            "the body starts to rise: {rising}"
        );
        let settled = at(
            &ctx,
            f64::from(motion::FAST + motion::SPRING_DURATION) + 0.01,
            false,
        )
        .scale;
        assert!(
            (settled - 1.0).abs() < 1e-4,
            "the spring settles exactly at rest: {settled}"
        );
    }

    /// Reduce Motion keeps the wash but removes the scale (the plan's rule).
    #[test]
    fn reduce_motion_drops_the_press_scale() {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        theme::install(&ctx, AppTheme::Dark, true);
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 200.0));
        let id = egui::Id::new("press-reduce");
        let mut frames = Vec::new();
        for time in [0.0, 0.03, 0.06] {
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..Default::default()
            };
            let _ = ctx.run(raw, |_| {
                frames.push(press_frame(&ctx, id, time > 0.0, true));
            });
        }
        for frame in &frames {
            assert_eq!(frame.scale, 1.0, "Reduce Motion never scales the body");
        }
        assert!(
            frames.last().unwrap().fill > 0.9,
            "the wash still swaps under Reduce Motion"
        );
    }
}
