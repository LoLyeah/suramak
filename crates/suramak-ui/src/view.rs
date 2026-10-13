//! Static, comp-faithful placeholder view for the M1 shell.
//!
//! Composition follows the approved north-star comp (`.impeccable/mocks/decision/C-system.png`)
//! and its measured region map (`.impeccable/build/regions.json`):
//!
//! * window chrome band on top — traffic-light inset at the left, the window
//!   title centred where macOS puts it, record / play / export integrated at the
//!   right (the comp's iOS-style floating pill is *not* reproduced: native
//!   conventions win, per the direction contract);
//! * translucent sidebar ≈23% wide with text-only recording rows;
//! * main pane: serif title → wide waveform scrubber strip with time ticks and a
//!   slim playhead → transcript column with mono timestamps.
//!
//! No functionality yet (that is M2): every element is a static drawing.

use egui::{
    pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Painter, Pos2, Rect, Stroke, StrokeKind,
    Vec2,
};

use crate::theme::{self, Palette};

/// Titlebar band height (comp window-chrome ≈6% of 768 px).
const TITLEBAR_HEIGHT: f32 = 46.0;
/// Sidebar width as a fraction of the window (comp region: 0.225–0.23).
const SIDEBAR_FRACTION: f32 = 0.23;
/// Left inset reserved for the native traffic lights.
const TRAFFIC_LIGHT_INSET: f32 = 78.0;
/// Right edge reserved for the integrated toolbar controls.
const TOOLBAR_RESERVED: f32 = 158.0;
/// Inner padding of the main pane (comp: content starts at 25% of window width).
const PANE_PAD_X: f32 = 26.0;

struct Recording {
    title: &'static str,
    meta: &'static str,
}

const RECORDINGS: [Recording; 5] = [
    Recording {
        title: "Voice memo",
        meta: "Kamis 13 Okt · 0:42",
    },
    Recording {
        title: "Wawancara — Klien",
        meta: "Kamis 13 Okt · 24:10",
    },
    Recording {
        title: "Rapat tim",
        meta: "Rabu 12 Okt · 1:15:03",
    },
    Recording {
        title: "Ide pagi",
        meta: "Rabu 12 Okt · 4:20",
    },
    Recording {
        title: "Catatan",
        meta: "Selasa 11 Okt · 9:58",
    },
];

struct TranscriptLine {
    time: &'static str,
    text: &'static str,
}

const TRANSCRIPT: [TranscriptLine; 3] = [
    TranscriptLine {
        time: "00:00",
        text: "The quick brown fox jumps over the lazy dog.",
    },
    TranscriptLine {
        time: "00:04",
        text: "Selamat pagi! Nama saya Suramak, aplikasi transkripsi lokal.",
    },
    TranscriptLine {
        time: "00:09",
        text: "Terima kasih sudah mencoba.",
    },
];

/// The scrubber's time ticks (comp labels) with their positions in minutes of
/// the recording's 24:10 runtime, so the axis is linear in time.
const TICKS: [(&str, f32); 6] = [
    ("0:00", 0.0),
    ("5:00", 5.0),
    ("10:00", 10.0),
    ("15:00", 15.0),
    ("20:00", 20.0),
    ("24:10", 24.0 + 10.0 / 60.0),
];

/// View state for the placeholder (M2 replaces this with the real app state).
pub struct ViewState {
    /// Index of the highlighted sidebar row.
    selected: usize,
}

impl ViewState {
    /// Fresh state: the comp's second row ("Wawancara — Klien") is selected.
    pub fn new() -> Self {
        Self { selected: 1 }
    }
}

/// Paints the whole shell for one frame.
pub fn draw(ctx: &egui::Context, state: &ViewState) {
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
        .show(ctx, |ui| sidebar(ui, &palette, state));

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(palette.pane))
        .show(ctx, |ui| main_pane(ui, &palette));
}

/// Window chrome: drag surface, centred title, integrated record/play/export.
fn titlebar(ui: &mut egui::Ui, palette: &Palette) {
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

    let painter = ui.painter();
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        "Suramak",
        FontId::new(13.0, FontFamily::Proportional),
        palette.text_dim,
    );

    let center_y = rect.center().y;
    let mut x = rect.right() - 30.0;
    export_glyph(painter, pos2(x, center_y), palette);
    x -= 42.0;
    play_glyph(painter, pos2(x, center_y), palette);
    x -= 42.0;
    record_glyph(painter, pos2(x, center_y), palette);

    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0_f32, palette.hairline),
    );
}

/// Red filled circle — the record control.
fn record_glyph(painter: &Painter, center: Pos2, palette: &Palette) {
    painter.circle_filled(center, 6.0, palette.record_red);
}

/// Play triangle.
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

/// Export: a tray with an arrow rising out of it.
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

/// Translucent sidebar: text-only recording rows, one selected.
fn sidebar(ui: &mut egui::Ui, palette: &Palette, state: &ViewState) {
    let rect = ui.max_rect();
    let painter = ui.painter();

    let row_height = 46.0;
    let mut y = rect.top() + 12.0;
    for (index, recording) in RECORDINGS.iter().enumerate() {
        let row = Rect::from_min_size(
            pos2(rect.left() + 8.0, y),
            vec2(rect.width() - 16.0, row_height - 6.0),
        );
        if index == state.selected {
            painter.rect_filled(row, CornerRadius::same(6), palette.row_selected);
        }
        painter.text(
            pos2(row.left() + 10.0, row.top() + 7.0),
            Align2::LEFT_TOP,
            recording.title,
            FontId::new(13.0, FontFamily::Proportional),
            palette.text,
        );
        painter.text(
            pos2(row.left() + 10.0, row.top() + 25.0),
            Align2::LEFT_TOP,
            recording.meta,
            FontId::new(11.0, FontFamily::Proportional),
            palette.text_dim,
        );
        y += row_height;
    }
}

/// Main pane: serif title → waveform scrubber → transcript column.
fn main_pane(ui: &mut egui::Ui, palette: &Palette) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    let left = rect.left() + PANE_PAD_X;
    let right = rect.right() - PANE_PAD_X;

    painter.text(
        pos2(left, rect.top() + 14.0),
        Align2::LEFT_TOP,
        "Wawancara — Klien",
        FontId::new(34.0, theme::serif_family()),
        palette.text,
    );

    let strip = Rect::from_min_size(pos2(left, rect.top() + 84.0), vec2(right - left, 104.0));
    painter.rect_filled(strip, CornerRadius::same(10), palette.card);
    painter.rect_stroke(
        strip,
        CornerRadius::same(10),
        Stroke::new(1.0_f32, palette.hairline),
        StrokeKind::Inside,
    );
    waveform(painter, strip, palette);

    let mut y = strip.bottom() + 24.0;
    for line in &TRANSCRIPT {
        painter.text(
            pos2(left, y + 3.0),
            Align2::LEFT_TOP,
            line.time,
            FontId::new(11.5, FontFamily::Monospace),
            palette.text_dim,
        );
        painter.text(
            pos2(left + 74.0, y),
            Align2::LEFT_TOP,
            line.text,
            FontId::new(16.0, theme::serif_family()),
            palette.text,
        );
        y += 42.0;
    }
}

/// Static waveform bars + time ticks + the slim playhead at 0:00 (the signature object).
fn waveform(painter: &Painter, strip: Rect, palette: &Palette) {
    let bars = strip.shrink2(Vec2::new(16.0, 10.0));
    let tick_row = 16.0;
    let center_y = bars.center().y - tick_row * 0.5;
    let max_half = (bars.height() - tick_row) * 0.5 - 4.0;

    let bar_width = 2.0;
    let pitch = 5.0;
    let mut x = bars.left();
    let mut index = 0usize;
    while x + bar_width <= bars.right() {
        let half = 2.0 + bar_height(index) * max_half;
        let bar = Rect::from_center_size(
            pos2(x + bar_width * 0.5, center_y),
            vec2(bar_width, half * 2.0),
        );
        let color = if index % 9 == 0 {
            palette.wave_dim
        } else {
            palette.wave
        };
        painter.rect_filled(bar, CornerRadius::same(1), color);
        x += pitch;
        index += 1;
    }

    // Time ticks along the bottom of the strip, positioned linearly in time.
    let tick_y = bars.bottom() + tick_row * 0.5;
    let last = TICKS.len() - 1;
    let total_minutes = TICKS[last].1;
    for (index, (tick, minutes)) in TICKS.iter().enumerate() {
        let t = *minutes / total_minutes;
        let x = bars.left() + (bars.width() - 22.0) * t;
        let anchor = if index == 0 {
            Align2::LEFT_CENTER
        } else if index == last {
            Align2::RIGHT_CENTER
        } else {
            Align2::CENTER_CENTER
        };
        painter.text(
            pos2(x, tick_y),
            anchor,
            *tick,
            FontId::new(10.5, FontFamily::Monospace),
            palette.text_dim,
        );
    }

    // Playhead parked at the start of the recording.
    let playhead = Rect::from_min_max(
        pos2(bars.left() + 1.0, bars.top() - 2.0),
        pos2(bars.left() + 3.0, bars.bottom() - 2.0),
    );
    painter.rect_filled(playhead, CornerRadius::same(1), palette.accent);
}

/// Deterministic pseudo-random bar silhouette (no RNG dependency).
fn bar_height(index: usize) -> f32 {
    let mut hash = (index as u32).wrapping_mul(2_654_435_761);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2c1b_3c6d);
    hash ^= hash >> 12;
    let noise = (hash & 0xffff) as f32 / 65_535.0;
    let swell = 0.45 + 0.55 * ((index as f32) * 0.045).sin().abs();
    (swell * (0.25 + 0.75 * noise)).clamp(0.0, 1.0)
}
