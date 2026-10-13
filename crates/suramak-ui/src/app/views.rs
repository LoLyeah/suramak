//! Frame dispatcher + the M1 chrome strip (visuals unchanged), split out of the
//! old static `view.rs` placeholder. Every frame draws the titlebar, the sidebar
//! (library) and the route's main pane; the returned [`Effect`]s are executed by
//! `crate::runtime` after the frame.
//!
//! M3: the record dot is live — it starts a session when capture is compiled in
//! (and the palette/glyph stay the M1 ones; only the unavailable state dims it).

use std::time::Instant;

use egui::{
    pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Painter, Pos2, Rect, Stroke, StrokeKind,
};

use crate::theme::{self, Palette};

use super::{home, library, AppState, Effect, Route};

/// Titlebar band height (comp window-chrome ≈6% of 768 px) — M1 value.
pub const TITLEBAR_HEIGHT: f32 = 46.0;
/// Sidebar width as a fraction of the window (comp region: 0.225–0.23) — M1 value.
pub const SIDEBAR_FRACTION: f32 = 0.23;
/// Left inset reserved for the native traffic lights — M1 value.
pub const TRAFFIC_LIGHT_INSET: f32 = 78.0;
/// Right edge reserved for the integrated toolbar controls — M1 value.
pub const TOOLBAR_RESERVED: f32 = 158.0;
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
        .show(ctx, |ui| titlebar(ui, &palette, state, &mut effects));

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

    effects
}

/// Window chrome: drag surface, centred title, integrated record/play/export.
/// (M1 composition and glyphs; the record dot is now interactive.)
fn titlebar(ui: &mut egui::Ui, palette: &Palette, state: &mut AppState, effects: &mut Vec<Effect>) {
    let rect = ui.max_rect();

    // Drag anywhere in the chrome except the traffic-light inset and the controls.
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
    x -= 42.0;
    let record_center = pos2(x, center_y);

    // The record control: M1 red dot, live when capture is compiled in.
    let recording = matches!(state.record, super::RecordState::Recording(_));
    let enabled = state.can_record();
    let record_response = ui.interact(
        Rect::from_center_size(record_center, vec2(28.0, 28.0)),
        egui::Id::new("suramak-record-dot"),
        egui::Sense::click(),
    );
    record_glyph(&painter, record_center, palette, enabled || recording);
    if record_response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if record_response.clicked() {
        if enabled {
            let date = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            effects.extend(state.start_recording(Instant::now(), &date));
        } else if recording {
            // Already recording: the Stop button in the record view ends it.
        } else if matches!(state.job, super::Job::Running(_)) {
            state.set_status("recording is disabled while a job is running", false);
        } else {
            state.set_status(
                "recording is not compiled into this build (feature `capture`)",
                true,
            );
        }
    }

    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0_f32, palette.hairline),
    );
}

/// Red filled circle — the record control (M1 glyph; dimmed when unavailable).
fn record_glyph(painter: &Painter, center: Pos2, palette: &Palette, enabled: bool) {
    let color = if enabled {
        palette.record_red
    } else {
        palette.record_red.gamma_multiply(0.35)
    };
    painter.circle_filled(center, 6.0, color);
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
