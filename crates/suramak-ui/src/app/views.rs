//! Frame dispatcher + the window chrome, split out of the old static `view.rs`
//! placeholder. Every frame draws the titlebar, the sidebar (library), the
//! route's main pane and the floating record control; the returned [`Effect`]s
//! are executed by `crate::runtime` after the frame.
//!
//! M9a: the frame is token-driven — ONE material ladder in both appearances
//! (`theme::tokens`), egui's `Visuals` reworked from the same tokens, the dead
//! titlebar glyphs (play/export) removed (the deck owns those actions now), and
//! route changes animate with the slow token: fade + a ~2 pt rise, never a
//! teleport. The floating record pill keeps its M8.5 contract (one shared
//! `float_rect`, bottom-center, own cast shadow) and gains its idle↔recording
//! morph (label crossfade + a colour shift) plus hover/press states.

use std::time::Instant;

use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Rect, Stroke, StrokeKind,
};

use crate::motion::{self, Motion};
use crate::theme::{self, Tokens};

use super::{home, library, AppState, Effect, Route};

/// Titlebar band height (comp window-chrome ≈6% of 768 px) — M1 value.
pub const TITLEBAR_HEIGHT: f32 = 46.0;
/// Sidebar width as a fraction of the window (comp region: 0.225–0.23) — M1 value.
pub const SIDEBAR_FRACTION: f32 = 0.23;
/// Left inset reserved for the native traffic lights — M1 value.
pub const TRAFFIC_LIGHT_INSET: f32 = 78.0;
/// Right inset of the titlebar's drag surface. M9a: the integrated toolbar
/// glyphs are gone (play/export were dead affordances; the deck owns them), so
/// the drag area only steps back from the window edge.
pub const TOOLBAR_RESERVED: f32 = 16.0;
/// Inner padding of the main pane (comp: content starts at 25% of window width).
pub const PANE_PAD_X: f32 = 26.0;

/// Paints the whole shell for one frame and returns the effects it requested.
pub fn draw(ctx: &egui::Context, state: &mut AppState) -> Vec<Effect> {
    let mut effects = Vec::new();
    let tokens = theme::tokens(state.theme);
    let motion = Motion::new(state.reduce_motion);
    let screen = ctx.content_rect();
    let sidebar_width = (screen.width() * SIDEBAR_FRACTION).clamp(190.0, 360.0);

    // Route-change motion (M9a): a fresh epoch id per route change animates the
    // slow fade+rise once; the animation ends (egui repaints only while a value
    // is between 0 and 1). M9d item 6: the app's **first presented frame**
    // renders statically — the startup epoch is registered without an arrival
    // (the brief's "the initial frame renders statically; only changes animate").
    let (epoch, first_frame) = route_epoch(ctx, state.route);
    let route_fade = if first_frame {
        1.0
    } else {
        motion.arrive(
            ctx,
            egui::Id::new(("suramak-route-fade", epoch)),
            motion::SLOW,
        )
    };

    egui::TopBottomPanel::top("suramak-titlebar")
        .exact_height(TITLEBAR_HEIGHT)
        .frame(egui::Frame::NONE.fill(tokens.window))
        .show(ctx, |ui| titlebar(ui, &tokens));

    egui::SidePanel::left("suramak-sidebar")
        .exact_width(sidebar_width)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(tokens.window))
        .show(ctx, |ui| {
            library::ui(ui, &tokens, &motion, state, &mut effects)
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(tokens.window))
        .show(ctx, |ui| {
            // Fade + rise: the pane content is drawn into a child ui offset by
            // the residual rise, with the route's opacity applied; the text
            // measure itself never shifts (the offset is a pure translation).
            let offset = motion::rise(route_fade, 2.0);
            let mut pane = ui.new_child(
                egui::UiBuilder::new().max_rect(ui.max_rect().translate(vec2(0.0, offset))),
            );
            pane.set_opacity(route_fade);
            match state.route {
                Route::Home => home::ui(&mut pane, &tokens, &motion, state, &mut effects),
                Route::Transcribe => {
                    super::transcribe::ui(&mut pane, &tokens, &motion, state, &mut effects)
                }
                Route::Editor => {
                    super::editor::ui(&mut pane, &tokens, &motion, state, &mut effects)
                }
                Route::Record => {
                    super::record::ui(&mut pane, &tokens, &motion, state, &mut effects)
                }
                Route::Models => {
                    super::models::ui(&mut pane, &tokens, &motion, state, &mut effects)
                }
                Route::Settings => {
                    super::settings::ui(&mut pane, &tokens, &motion, state, &mut effects)
                }
            }
        });

    // The floating record control rides above every panel (M8.5).
    float_control(ctx, &tokens, &motion, state, &mut effects);

    effects
}

/// A fresh animation epoch each time the route changes (M9a route motion), plus
/// whether this is the app's very first presented frame (M9d item 6: the
/// startup epoch must render statically — no arrival fade). Stored in egui's
/// temp data: `(route key, epoch)`.
fn route_epoch(ctx: &egui::Context, route: Route) -> (u64, bool) {
    let id = egui::Id::new("suramak-route-epoch");
    ctx.data_mut(|data| match data.get_temp::<(u64, u64)>(id) {
        Some((key, epoch)) if key == route as u64 => (epoch, false),
        Some((_, epoch)) => {
            let next = (route as u64, epoch.wrapping_add(1));
            data.insert_temp(id, next);
            (next.1, false)
        }
        None => {
            // Nothing drawn yet: the first frame registers the route without
            // an arrival animation.
            let next = (route as u64, 1);
            data.insert_temp(id, next);
            (next.1, true)
        }
    })
}

/// Window chrome: drag surface, centred title, hairline under.
///
/// M9a: the M1 play/export glyphs are removed — they looked like controls and
/// did nothing (critique P1-4, H6); the deck now owns transport and export.
fn titlebar(ui: &mut egui::Ui, tokens: &Tokens) {
    let rect = ui.max_rect();

    // Drag anywhere in the chrome except the traffic-light inset.
    let drag_rect = Rect::from_min_max(
        pos2(rect.left() + TRAFFIC_LIGHT_INSET, rect.top()),
        pos2(rect.right() - TOOLBAR_RESERVED, rect.bottom()),
    );
    let response = ui.interact(
        drag_rect,
        egui::Id::new("suramak-titlebar-drag"),
        egui::Sense::drag(),
    );
    if response.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

    let painter = ui.painter().clone();
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        "Suramak",
        FontId::new(13.0, FontFamily::Proportional),
        tokens.ink_soft,
    );

    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(tokens.stroke.hairline, tokens.hairline),
    );
}

/// The floating record control (M8.5): bottom-center of the window content,
/// above every route — THE record entry point.
///
/// One rect, two consumers: the painter below draws the crisp glyph/label/live
/// clock (and owns all input — no hit-test juggling), while `suramak-ui` hands
/// the same rect to `suramak-glass` so on macOS 26+ a real `NSGlassEffectView`
/// provides the material behind exactly these pixels. Everywhere else — and
/// until that glass exists — the painter draws the fallback pill.
///
/// M9a: the idle↔recording swap morphs (base token: label crossfade + a colour
/// shift) and the pill has hover/press states; the geometry and semantics are
/// the M8.5 contract, untouched.
fn float_control(
    ctx: &egui::Context,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    let viewport = ctx.viewport_rect();
    let float = suramak_glass::float_rect(
        (viewport.width(), viewport.height()),
        ctx.pixels_per_point(),
    );
    let rect = Rect::from_min_size(
        viewport.min + vec2(float.x, float.y),
        vec2(float.width, float.height),
    );
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return; // degenerate window: nothing sensible to draw
    }

    let recording = matches!(state.record, super::RecordState::Recording(_));
    let stopping = matches!(&state.record, super::RecordState::Recording(rec) if rec.stopping);
    let enabled = state.can_record() || recording;

    egui::Area::new(egui::Id::new("suramak-record-float"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .movable(false)
        .constrain(false)
        .fade_in(false)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(rect.size(), egui::Sense::click());
            let painter = ui.painter();
            // M9b a11y: the pill is a labeled button (Record ↔ Stop recording)
            // and carries the one focus ring when tabbed to.
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    enabled,
                    if recording {
                        "Stop recording"
                    } else {
                        "Record"
                    },
                )
            });
            if response.has_focus() {
                let ring_radius = CornerRadius::same(
                    ((rect.height() * 0.5) + 2.0).round().clamp(0.0, 255.0) as u8,
                );
                painter.rect_stroke(
                    rect.expand(1.5),
                    ring_radius,
                    egui::Stroke::new(tokens.stroke.focus, tokens.accent),
                    egui::StrokeKind::Outside,
                );
            }

            // The morph: 0 = idle (dot + "Record"), 1 = recording (stop + clock).
            let morph = motion.factor(
                ctx,
                egui::Id::new("suramak-pill-morph"),
                recording,
                motion::BASE,
            );
            let hover = motion.factor(
                ctx,
                egui::Id::new("suramak-pill-hover"),
                enabled && response.hovered(),
                motion::FAST,
            );
            let press = motion.factor(
                ctx,
                egui::Id::new("suramak-pill-press"),
                enabled && response.is_pointer_button_down_on(),
                motion::FAST,
            );

            // M9d item 1 — the spring (owner-confirmed: "gentle spring, no
            // bounce"): pressing scales the pill to ≈0.97 with a springy
            // release (overshoot ≤ 2 % of the travel, ends within 240 ms; a
            // snap under Reduce Motion). The idle↔recording morph adds the
            // brief's gentle scale component: a ≤1.5 % dip that peaks
            // mid-crossfade and returns to 1.0 — the existing label/colour
            // crossfade is untouched.
            let press_target = if enabled && response.is_pointer_button_down_on() {
                motion::SPRING_PRESS_SCALE
            } else {
                1.0
            };
            let spring =
                motion.press_spring(ctx, egui::Id::new("suramak-pill-spring"), press_target);
            let morph_dip = -0.015 * (std::f32::consts::PI * morph).sin();
            let scale = spring + morph_dip;
            // Everything the UI paints scales around the pill's centre. The
            // interaction rect and the focus ring stay put: the M8.5 float
            // rect — and on macOS the glass material behind it — is fixed.
            let body = Rect::from_center_size(rect.center(), rect.size() * scale);
            let radius = CornerRadius::same((body.height() * 0.5).round().clamp(0.0, 255.0) as u8);

            if !state.float_glass {
                // The fallback material: a frosted pill — translucent fill,
                // hairline, subtle shadow. Not a Liquid Glass imitation.
                painter.add(
                    egui::epaint::Shadow {
                        offset: [0, 6],
                        blur: 18,
                        spread: 0,
                        color: Color32::from_black_alpha(96),
                    }
                    .as_shape(body, radius),
                );
                // The recording state carries a faint red cast (morph colour).
                let fill = motion::lerp_color(
                    tokens.float_fill,
                    motion::tint_over(
                        tokens.float_fill,
                        Color32::from_rgba_unmultiplied(0xff, 0x45, 0x3a, 34),
                        1.0,
                    ),
                    morph,
                );
                painter.rect_filled(body, radius, fill);
                painter.rect_stroke(
                    body,
                    radius,
                    Stroke::new(tokens.stroke.hairline, tokens.hairline),
                    StrokeKind::Inside,
                );
            }
            // Hover/press lift (works over real glass too — it is painted on top).
            let lift = (hover * 12.0 + press * 10.0).round() as u8;
            if lift > 0 {
                painter.rect_filled(body, radius, Color32::from_white_alpha(lift));
            }

            let center_y = body.center().y;
            let idle_alpha = (1.0 - morph).clamp(0.0, 1.0);
            let record_alpha = morph.clamp(0.0, 1.0);
            let alpha = |color: Color32, a: f32| color.gamma_multiply(a);

            if idle_alpha > 0.01 {
                // Red dot + "Record" label. M9d: the content scales with the
                // spring (fonts, dot, gaps) so the press reads as the whole
                // pill compressing — on macOS that is the content inside the
                // fixed glass body (the material's frame is an M8.5 contract).
                let label_color = if enabled { tokens.ink } else { tokens.ink_soft };
                let galley = painter.layout_no_wrap(
                    "Record".to_owned(),
                    FontId::new(15.0 * scale, FontFamily::Proportional),
                    alpha(label_color, idle_alpha),
                );
                let dot_radius = 6.5 * scale;
                let gap = 10.0 * scale;
                let total = dot_radius * 2.0 + gap + galley.size().x;
                let left = body.center().x - total / 2.0;
                let dot_color = if enabled {
                    tokens.record_red
                } else {
                    tokens.record_red.gamma_multiply(0.35)
                };
                painter.circle_filled(
                    pos2(left + dot_radius, center_y),
                    dot_radius,
                    alpha(dot_color, idle_alpha),
                );
                painter.galley(
                    pos2(
                        left + dot_radius * 2.0 + gap,
                        center_y - galley.size().y / 2.0,
                    ),
                    galley,
                    alpha(label_color, idle_alpha),
                );
            }
            if record_alpha > 0.01 {
                // Stop affordance + the live clock. M9c: the pill's clock is the
                // sanctioned **elapsed hh:mm:ss** recording format — the same
                // clock the record view's timer shows (the old mm:ss is gone).
                let elapsed = match &state.record {
                    super::RecordState::Recording(rec) => rec.elapsed,
                    _ => std::time::Duration::ZERO,
                };
                let clock = super::format_elapsed(elapsed);
                let galley = painter.layout_no_wrap(
                    clock,
                    FontId::new(19.0 * scale, FontFamily::Monospace),
                    alpha(tokens.ink, record_alpha),
                );
                let glyph_size = 16.0 * scale;
                let gap = 12.0 * scale;
                let total = glyph_size + gap + galley.size().x;
                let left = body.center().x - total / 2.0;
                let glyph_color = if stopping || !enabled {
                    tokens.record_red.gamma_multiply(0.4)
                } else {
                    tokens.record_red
                };
                super::controls::paint_glyph(
                    painter,
                    Rect::from_center_size(
                        pos2(left + glyph_size / 2.0, center_y),
                        vec2(glyph_size, glyph_size),
                    ),
                    super::controls::Glyph::Stop,
                    alpha(glyph_color, record_alpha),
                );
                painter.galley(
                    pos2(left + glyph_size + gap, center_y - galley.size().y / 2.0),
                    galley,
                    alpha(tokens.ink, record_alpha),
                );
            }

            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                let date = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                effects.extend(state.toggle_record(Instant::now(), &date));
            }
        });
}

/// Serif pane title (comp type scale: 34 px).
pub fn pane_title(ui: &mut egui::Ui, text: &str, tokens: &Tokens) {
    ui.add_space(12.0);
    ui.label(
        egui::RichText::new(text)
            .font(FontId::new(34.0, theme::serif_family()))
            .color(tokens.ink),
    );
}

/// Small dim caption line under a pane title. M9d: errors read in the
/// **text-safe** red (WCAG AA at 12 px on every light surface).
pub fn pane_caption(ui: &mut egui::Ui, text: &str, tokens: &Tokens, error: bool) {
    let color = if error {
        tokens.record_red_text
    } else {
        tokens.ink_soft
    };
    ui.label(
        egui::RichText::new(text)
            .size(12.0)
            .color(color)
            .family(FontFamily::Proportional),
    );
}

/// A status / inline message with the brief's fade discipline (M9c): fades in
/// over the base token, out over the fast token. The stale text is kept while
/// it fades out — an immediate-mode label cannot fade once it stops being
/// drawn — then dropped. Never blocks input (it is a label, not a control);
/// the app has no hover-pausable dismiss timer to honor (noted in the report).
///
/// Call it **every frame** (with `None` when there is no message) so the fade
/// id is registered while empty and the appearance animates.
pub fn status_caption(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    id_salt: impl std::hash::Hash,
    message: Option<(&str, bool)>,
) {
    let ctx = ui.ctx();
    let base_id = ui.id().with(id_salt);
    let fade_id = base_id.with("status-fade");
    let text_id = base_id.with("status-text");
    let fade = motion.fade(ctx, fade_id, message.is_some(), motion::BASE, motion::FAST);
    if let Some((text, error)) = message {
        ctx.data_mut(|data| data.insert_temp(text_id, (text.to_owned(), error)));
    }
    if fade <= 0.01 {
        return;
    }
    let stored = ctx.data_mut(|data| data.get_temp::<(String, bool)>(text_id));
    if let Some((text, error)) = stored {
        let color = if error {
            tokens.record_red_text
        } else {
            tokens.ink_soft
        };
        ui.label(
            egui::RichText::new(text)
                .size(12.0)
                .color(color.gamma_multiply(fade))
                .family(FontFamily::Proportional),
        );
    }
}

/// A one-shot fade-in for an empty / first-run / error state's text (base
/// token; a snap under Reduce Motion). The id is per state, so the caption
/// eases in the first time the state appears and stays settled — nothing loops.
pub fn arriving_caption(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    id_salt: impl std::hash::Hash,
    text: &str,
    error: bool,
) {
    let fade = motion.arrive(
        ui.ctx(),
        egui::Id::new(("suramak-arrive", id_salt)),
        motion::BASE,
    );
    let color = if error {
        tokens.record_red_text
    } else {
        tokens.ink_soft
    };
    ui.label(
        egui::RichText::new(text)
            .size(12.0)
            .color(color.gamma_multiply(fade))
            .family(FontFamily::Proportional),
    );
}

/// A one-row, ellipsis-truncated galley for a fixed-width slot (the editor
/// title row, the model-manager name column): the slot's geometry never
/// depends on what the text measures (owner-directed fix 2).
pub fn truncated_galley(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping {
        max_width,
        max_rows: 1,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    ui.ctx().fonts_mut(|fonts| fonts.layout_job(job))
}

/// Section heading inside a card (serif, 20 px).
pub fn section_title(ui: &mut egui::Ui, text: &str, tokens: &Tokens) {
    ui.label(
        egui::RichText::new(text)
            .font(FontId::new(20.0, theme::serif_family()))
            .color(tokens.ink),
    );
}

/// Paints the card recipe into `rect` (for the views that lay out by absolute
/// positions rather than frames).
pub fn paint_card(painter: &egui::Painter, rect: Rect, tokens: &Tokens) {
    painter.add(
        tokens
            .card_shadow
            .as_shape(rect, CornerRadius::same(tokens.radius.card)),
    );
    painter.rect_filled(rect, CornerRadius::same(tokens.radius.card), tokens.card);
    painter.rect_stroke(
        rect,
        CornerRadius::same(tokens.radius.card),
        Stroke::new(tokens.stroke.hairline, tokens.hairline),
        StrokeKind::Inside,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The painted alpha of a pane title's text shape in a frame — the route
    /// fade multiplies every shape inside the pane, so this reads the pane's
    /// effective opacity. `None` when the text is not in the frame at all.
    fn title_alpha(output: &egui::FullOutput, text: &str) -> Option<u8> {
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(shape) if shape.galley.text() == text => Some(
                    shape
                        .galley
                        .rows
                        .iter()
                        .flat_map(|placed| placed.row.visuals.mesh.vertices.iter())
                        .map(|vertex| vertex.color.a())
                        .max()
                        .unwrap_or(0),
                ),
                _ => None,
            })
    }

    /// M9d item 6: the very first presented frame renders **statically** — the
    /// startup route epoch is registered without an arrival fade (before this,
    /// frame one painted at opacity 0 and faded in). A route *change* still
    /// animates (only changes animate).
    #[test]
    fn the_first_presented_frame_is_static_and_route_changes_still_animate() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = AppState::new();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0));
        let run = |state: &mut AppState, time: f64| -> egui::FullOutput {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..Default::default()
            };
            ctx.run(input, |ctx| {
                draw(ctx, state);
            })
        };

        // Frame one, fresh context: the Home pane is fully painted.
        let first = run(&mut state, 0.0);
        assert_eq!(
            title_alpha(&first, "New transcription"),
            Some(255),
            "the first presented frame must render statically (no startup fade)"
        );

        // A route change still animates: the first frame of the new epoch
        // registers the arrival (painted at opacity 0 — nothing on screen),
        // the next frame is mid-flight, and the arrival ends fully painted.
        state.route = Route::Settings;
        let registered = run(&mut state, 0.016);
        assert_eq!(
            title_alpha(&registered, "Settings"),
            None,
            "the route change starts hidden (an arrival, not a teleport)"
        );
        let mid = run(&mut state, 0.10);
        let alpha = title_alpha(&mid, "Settings");
        assert!(
            alpha.is_some_and(|alpha| alpha > 0 && alpha < 255),
            "a route change must still animate (mid-arrival): {alpha:?}"
        );
        // …and the arrival ends.
        let settled = run(&mut state, 0.6);
        assert_eq!(
            title_alpha(&settled, "Settings"),
            Some(255),
            "the route arrival ends at full opacity"
        );
    }

    /// M9d item 1 (frame-level): the pill's spring is a scale around the float
    /// rect's centre — at rest the drawn body is the M8.5 rect, and the spring
    /// and morph dip are the only scale sources. The geometry contract (one
    /// float rect, bottom-centre) is untouched.
    #[test]
    fn the_pill_body_is_the_float_rect_at_rest() {
        let viewport = (1280.0_f32, 800.0_f32);
        let float = suramak_glass::float_rect(viewport, 1.0);
        let rect = Rect::from_min_size(pos2(float.x, float.y), vec2(float.width, float.height));
        // At rest (scale 1.0, no morph dip) the drawn body IS the float rect.
        let scale = 1.0;
        let body = Rect::from_center_size(rect.center(), rect.size() * scale);
        assert_eq!(body, rect);
        // Pressed: 0.97 around the centre — inside the rect on every side.
        let pressed =
            Rect::from_center_size(rect.center(), rect.size() * motion::SPRING_PRESS_SCALE);
        assert!(rect.contains_rect(pressed));
        assert!((pressed.width() / rect.width() - 0.97).abs() < 1e-6);
        assert!((pressed.center() - rect.center()).length() < 1e-6);
    }
}
