//! Frame dispatcher + the M1 chrome strip (visuals unchanged), split out of the
//! old static `view.rs` placeholder. Every frame draws the titlebar, the sidebar
//! (library), the route's main pane and the floating record control; the
//! returned [`Effect`]s are executed by `crate::runtime` after the frame.
//!
//! M3: the record control is live — it starts a session when capture is compiled
//! in (and the palette/glyph stay the M1 ones; only the unavailable state dims).
//!
//! M8.5: the record affordance left the titlebar. The floating bottom-center
//! control below is THE record entry point — idle shows the red dot + "Record",
//! a live session shows the stop glyph + the ticking `mm:ss` clock. On macOS
//! 26+ a real `NSGlassEffectView` sits behind exactly these pixels (same rect,
//! via `suramak-glass`); everywhere else the painter draws the honest fallback
//! pill (translucent fill, hairline, subtle shadow — the M9 material ladder,
//! never a fake of Liquid Glass).

use std::time::Instant;

use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Painter, Pos2, Rect, Stroke,
    StrokeKind,
};

use crate::theme::{self, Palette};

use super::{home, library, AppState, Effect, Route};

/// Titlebar band height (comp window-chrome ≈6% of 768 px) — M1 value.
pub const TITLEBAR_HEIGHT: f32 = 46.0;
/// Sidebar width as a fraction of the window (comp region: 0.225–0.23) — M1 value.
pub const SIDEBAR_FRACTION: f32 = 0.23;
/// Left inset reserved for the native traffic lights — M1 value.
pub const TRAFFIC_LIGHT_INSET: f32 = 78.0;
/// Right edge reserved for the integrated toolbar glyphs (play + export; the
/// record control moved to the floating pill in M8.5, so this is smaller than
/// the M1 three-glyph value).
pub const TOOLBAR_RESERVED: f32 = 96.0;
/// Inner padding of the main pane (comp: content starts at 25% of window width).
pub const PANE_PAD_X: f32 = 26.0;

/// Paints the whole shell for one frame and returns the effects it requested.
pub fn draw(ctx: &egui::Context, state: &mut AppState) -> Vec<Effect> {
    let mut effects = Vec::new();
    let palette = theme::palette();
    let screen = ctx.content_rect();
    let sidebar_width = (screen.width() * SIDEBAR_FRACTION).clamp(190.0, 360.0);

    egui::TopBottomPanel::top("suramak-titlebar")
        .exact_height(TITLEBAR_HEIGHT)
        .frame(egui::Frame::NONE.fill(palette.chrome))
        .show(ctx, |ui| titlebar(ui, &palette));

    egui::SidePanel::left("suramak-sidebar")
        .exact_width(sidebar_width)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(palette.sidebar))
        .show(ctx, |ui| library::ui(ui, &palette, state, &mut effects));

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(palette.pane))
        .show(ctx, |ui| match state.route {
            Route::Home => home::ui(ui, &palette, state, &mut effects),
            Route::Transcribe => super::transcribe::ui(ui, &palette, state, &mut effects),
            Route::Editor => super::editor::ui(ui, &palette, state, &mut effects),
            Route::Record => super::record::ui(ui, &palette, state, &mut effects),
            Route::Models => super::models::ui(ui, &palette, state, &mut effects),
            Route::Settings => super::settings::ui(ui, &palette, state, &mut effects),
        });

    // The floating record control rides above every panel (M8.5).
    float_control(ctx, &palette, state, &mut effects);

    effects
}

/// Window chrome: drag surface, centred title, integrated play/export glyphs.
/// (M1 composition and glyphs; the record dot moved to the floating control.)
fn titlebar(ui: &mut egui::Ui, palette: &Palette) {
    let rect = ui.max_rect();

    // Drag anywhere in the chrome except the traffic-light inset and the glyphs.
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
        palette.text_dim,
    );

    let center_y = rect.center().y;
    let mut x = rect.right() - 30.0;
    export_glyph(&painter, pos2(x, center_y), palette);
    x -= 42.0;
    play_glyph(&painter, pos2(x, center_y), palette);

    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0_f32, palette.hairline),
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
fn float_control(
    ctx: &egui::Context,
    palette: &Palette,
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
            let radius = CornerRadius::same((rect.height() * 0.5).round().clamp(0.0, 255.0) as u8);

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
                    .as_shape(rect, radius),
                );
                painter.rect_filled(rect, radius, palette.float_fill);
                painter.rect_stroke(
                    rect,
                    radius,
                    Stroke::new(1.0_f32, palette.hairline),
                    StrokeKind::Inside,
                );
            }

            let center_y = rect.center().y;
            if recording {
                // Stop affordance + the live clock (mm:ss).
                let elapsed = match &state.record {
                    super::RecordState::Recording(rec) => rec.elapsed,
                    _ => std::time::Duration::ZERO,
                };
                let clock = suramak_playback::format_mmss(elapsed.as_millis() as u32);
                let galley = painter.layout_no_wrap(
                    clock,
                    FontId::new(19.0, FontFamily::Monospace),
                    palette.text,
                );
                let glyph_size = 16.0;
                let gap = 12.0;
                let total = glyph_size + gap + galley.size().x;
                let left = rect.center().x - total / 2.0;
                let glyph_color = if stopping || !enabled {
                    palette.record_red.gamma_multiply(0.4)
                } else {
                    palette.record_red
                };
                painter.rect_filled(
                    Rect::from_center_size(
                        pos2(left + glyph_size / 2.0, center_y),
                        vec2(glyph_size, glyph_size),
                    ),
                    CornerRadius::same(3),
                    glyph_color,
                );
                painter.galley(
                    pos2(left + glyph_size + gap, center_y - galley.size().y / 2.0),
                    galley,
                    palette.text,
                );
            } else {
                // Red dot + "Record" label.
                let label_color = if enabled {
                    palette.text
                } else {
                    palette.text_dim
                };
                let galley = painter.layout_no_wrap(
                    "Record".to_owned(),
                    FontId::new(15.0, FontFamily::Proportional),
                    label_color,
                );
                let dot_radius = 6.5;
                let gap = 10.0;
                let total = dot_radius * 2.0 + gap + galley.size().x;
                let left = rect.center().x - total / 2.0;
                let dot_color = if enabled {
                    palette.record_red
                } else {
                    palette.record_red.gamma_multiply(0.35)
                };
                painter.circle_filled(pos2(left + dot_radius, center_y), dot_radius, dot_color);
                painter.galley(
                    pos2(
                        left + dot_radius * 2.0 + gap,
                        center_y - galley.size().y / 2.0,
                    ),
                    galley,
                    label_color,
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

/// Play triangle (M1 glyph).
fn play_glyph(painter: &Painter, center: Pos2, palette: &Palette) {
    let points = vec![
        pos2(center.x - 5.0, center.y - 7.0),
        pos2(center.x - 5.0, center.y + 7.0),
        pos2(center.x + 7.0, center.y),
    ];
    painter.add(egui::Shape::convex_polygon(
        points,
        palette.text,
        Stroke::NONE,
    ));
}

/// Export: a tray with an arrow rising out of it (M1 glyph).
fn export_glyph(painter: &Painter, center: Pos2, palette: &Palette) {
    let stroke = Stroke::new(1.6_f32, palette.text_dim);
    let tray = Rect::from_center_size(pos2(center.x, center.y + 3.5), vec2(15.0, 8.0));
    painter.rect_stroke(tray, CornerRadius::same(2), stroke, StrokeKind::Inside);
    painter.line_segment(
        [
            pos2(center.x, center.y - 8.0),
            pos2(center.x, center.y + 1.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            pos2(center.x - 4.0, center.y - 4.0),
            pos2(center.x, center.y - 8.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            pos2(center.x + 4.0, center.y - 4.0),
            pos2(center.x, center.y - 8.0),
        ],
        stroke,
    );
}

/// Serif pane title (comp type scale: 34 px).
pub fn pane_title(ui: &mut egui::Ui, text: &str, palette: &Palette) {
    ui.add_space(12.0);
    ui.label(
        egui::RichText::new(text)
            .font(FontId::new(34.0, theme::serif_family()))
            .color(palette.text),
    );
}

/// Small dim caption line under a pane title.
pub fn pane_caption(ui: &mut egui::Ui, text: &str, palette: &Palette, error: bool) {
    let color = if error {
        palette.record_red
    } else {
        palette.text_dim
    };
    ui.label(
        egui::RichText::new(text)
            .size(12.0)
            .color(color)
            .family(FontFamily::Proportional),
    );
}

/// Section heading inside a card (serif, 20 px).
pub fn section_title(ui: &mut egui::Ui, text: &str, palette: &Palette) {
    ui.label(
        egui::RichText::new(text)
            .font(FontId::new(20.0, theme::serif_family()))
            .color(palette.text),
    );
}
