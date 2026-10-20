//! The M13b Go/No-Go screen.
//!
//! Pure egui: no platform types, so this module compiles (and its tests run) on
//! the host as well as on Android. `app.rs` owns the window/surface and feeds
//! the state below; `platform.rs` owns the JNI surface that fills it.
//!
//! The four sections map 1:1 to the four things that can kill option A
//! (`docs/PLAN-M13-ANDROID.md` §2.1 / §M13b):
//!
//! 1. text input through the soft keyboard (GameTextInput → winit IME → egui);
//! 2. lifecycle — state survives surface teardown and activity recreation;
//! 3. motion — a 60 fps animation with a measured frame-time readout;
//! 4. insets and the back gesture, delivered from the Kotlin shim over JNI.

use std::collections::VecDeque;

/// Which of the four checks is on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Text,
    Lifecycle,
    Motion,
    Insets,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Text, Tab::Lifecycle, Tab::Motion, Tab::Insets];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Text => "1 · TEXT",
            Tab::Lifecycle => "2 · LIFECYCLE",
            Tab::Motion => "3 · MOTION",
            Tab::Insets => "4 · INSETS/BACK",
        }
    }
}

/// How a check stands, as far as the spike itself can tell. `Go` means the
/// expected evidence arrived; the owner still has to judge quality (that is the
/// difference between "events arrive" and "text editing is usable").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Wait,
    Go,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Wait => "WAIT",
            Verdict::Go => "GO",
        }
    }
}

/// Insets reported by `MainActivity` (physical pixels; 0 until the first
/// `onApplyWindowInsets` callback).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Insets {
    pub top: i32,
    pub bottom: i32,
    pub left: i32,
    pub right: i32,
    /// IME (soft-keyboard) bottom inset — 0 while the keyboard is hidden.
    pub ime_bottom: i32,
    /// Bumped every time Kotlin pushes a new set.
    pub updates: u32,
}

/// Everything the spike counts. Lives in a `static` on Android so it survives
/// the activity/surface being destroyed and recreated.
#[derive(Debug)]
pub struct SpikeState {
    pub tab: Tab,
    // (1) text input
    pub text: String,
    pub chars_received: u64,
    pub deletions_seen: u64,
    last_text_chars: usize,
    /// The Kotlin `stateChanged` → JNI bridge is the only path that delivers
    /// soft-keyboard text here: winit 0.30.13 consumes android-activity's
    /// `InputEvent::TextEvent` and drops it (see docs/reports/M13b.md).
    /// Off = observe the raw winit path (no characters arrive).
    pub ime_bridge: bool,
    /// Full-text updates received from the Kotlin IME listener.
    pub ime_updates: u64,
    /// Where the IME says the caret/selection is, in characters.
    pub ime_cursor: Option<(usize, usize)>,
    /// Characters that arrived as winit `KeyboardInput.text` (hardware keys,
    /// and whatever a soft keyboard chooses to send as key events).
    pub winit_text_chars: u64,
    // (2) lifecycle
    pub probe: String,
    pub resumes: u32,
    pub surface_recreates: u32,
    pub recreate_requests: u32,
    // (3) motion
    pub anim_seconds: f64,
    frame_times: VecDeque<f32>,
    pub frames: u64,
    // (4) insets + back
    pub insets: Insets,
    pub back_presses: u32,
    pub recreate_requested: bool,
    // diagnostics
    pub backend: String,
    pub gpu_error: Option<String>,
}

/// Stable widget id for the text field, so the IME bridge can move its cursor.
pub const TEXT_FIELD_ID: &str = "m13b-text-field";

impl Default for SpikeState {
    fn default() -> Self {
        Self {
            tab: Tab::Text,
            text: String::new(),
            chars_received: 0,
            deletions_seen: 0,
            last_text_chars: 0,
            ime_bridge: true,
            ime_updates: 0,
            ime_cursor: None,
            winit_text_chars: 0,
            probe: String::new(),
            resumes: 0,
            surface_recreates: 0,
            recreate_requests: 0,
            anim_seconds: 0.0,
            frame_times: VecDeque::with_capacity(MAX_FRAME_SAMPLES),
            frames: 0,
            insets: Insets::default(),
            back_presses: 0,
            recreate_requested: false,
            backend: "not initialised".to_owned(),
            gpu_error: None,
        }
    }
}

/// Frame-time window for the average fps readout (~2 s at 60 fps).
pub const MAX_FRAME_SAMPLES: usize = 120;

impl SpikeState {
    /// Advance the animation clock and record one frame's duration.
    pub fn tick(&mut self, dt: f32) {
        self.frames += 1;
        self.anim_seconds += f64::from(dt.max(0.0));
        if dt.is_finite() && dt > 0.0 {
            if self.frame_times.len() == MAX_FRAME_SAMPLES {
                self.frame_times.pop_front();
            }
            self.frame_times.push_back(dt);
        }
    }

    /// Called once per frame *after* the widgets ran: a positive character
    /// delta is text the IME delivered into the buffer.
    pub fn observe_text(&mut self) {
        let now = self.text.chars().count();
        if now > self.last_text_chars {
            self.chars_received += (now - self.last_text_chars) as u64;
        } else if now < self.last_text_chars {
            self.deletions_seen += (self.last_text_chars - now) as u64;
        }
        self.last_text_chars = now;
    }

    /// Measured average frames per second over [`MAX_FRAME_SAMPLES`] frames,
    /// or `None` before there is enough data.
    pub fn avg_fps(&self) -> Option<f32> {
        let total: f32 = self.frame_times.iter().sum();
        if self.frame_times.len() < 10 || total <= 0.0 {
            return None;
        }
        Some(self.frame_times.len() as f32 / total)
    }

    /// Mean frame time in milliseconds over the same window.
    pub fn avg_frame_ms(&self) -> Option<f32> {
        let total: f32 = self.frame_times.iter().sum();
        if self.frame_times.is_empty() {
            return None;
        }
        Some(1000.0 * total / self.frame_times.len() as f32)
    }

    pub fn set_insets(&mut self, top: i32, bottom: i32, left: i32, right: i32, ime_bottom: i32) {
        self.insets = Insets {
            top,
            bottom,
            left,
            right,
            ime_bottom,
            updates: self.insets.updates.saturating_add(1),
        };
    }

    /// The four verdicts, in check order. Each is evidence-based; the quality
    /// judgement stays with the owner (`docs/m13-gate.md`).
    pub fn verdicts(&self) -> [(&'static str, Verdict, String); 4] {
        let text = if self.chars_received > 0 && self.ime_updates > 0 {
            (
                Verdict::Go,
                format!(
                    "{} chars, {} IME updates",
                    self.chars_received, self.ime_updates
                ),
            )
        } else if self.chars_received > 0 {
            (
                Verdict::Go,
                format!("{} chars (winit path)", self.chars_received),
            )
        } else {
            (Verdict::Wait, "no characters yet".to_owned())
        };

        let lifecycle = if self.resumes >= 2 && !self.probe.is_empty() {
            (Verdict::Go, format!("{} resumes, probe kept", self.resumes))
        } else if self.resumes >= 2 {
            (Verdict::Wait, "resumed, probe empty".to_owned())
        } else {
            (Verdict::Wait, format!("{} resume(s)", self.resumes))
        };

        let motion = match self.avg_fps() {
            Some(fps) if fps >= 55.0 => (Verdict::Go, format!("{fps:.1} fps avg")),
            Some(fps) => (Verdict::Wait, format!("{fps:.1} fps avg")),
            None => (Verdict::Wait, "sampling…".to_owned()),
        };

        let insets = if self.insets.updates > 0 && self.back_presses > 0 {
            (
                Verdict::Go,
                format!(
                    "insets x{}, {} back",
                    self.insets.updates, self.back_presses
                ),
            )
        } else if self.insets.updates > 0 {
            (Verdict::Wait, "press back once".to_owned())
        } else {
            (Verdict::Wait, "no insets yet".to_owned())
        };

        [
            ("1 TEXT", text.0, text.1),
            ("2 LIFECYCLE", lifecycle.0, lifecycle.1),
            ("3 MOTION", motion.0, motion.1),
            ("4 INSETS/BACK", insets.0, insets.1),
        ]
    }

    fn reset_counters(&mut self) {
        self.text.clear();
        self.probe.clear();
        self.chars_received = 0;
        self.deletions_seen = 0;
        self.last_text_chars = 0;
        self.ime_updates = 0;
        self.ime_cursor = None;
        self.winit_text_chars = 0;
        self.resumes = 0;
        self.surface_recreates = 0;
        self.recreate_requests = 0;
        self.frame_times.clear();
        self.frames = 0;
        self.back_presses = 0;
        self.insets = Insets::default();
        self.anim_seconds = 0.0;
    }
}

fn verdict_color(v: Verdict) -> egui::Color32 {
    match v {
        Verdict::Go => egui::Color32::from_rgb(80, 200, 120),
        Verdict::Wait => egui::Color32::from_rgb(230, 180, 60),
    }
}

/// Draw one frame. Returns nothing: all state lives in `state`.
pub fn draw(ctx: &egui::Context, state: &mut SpikeState) {
    egui::TopBottomPanel::top("m13b-header").show(ctx, |ui| {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("M13B GO/NO-GO")
                    .size(26.0)
                    .strong()
                    .color(egui::Color32::from_rgb(120, 190, 255)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("reset").clicked() {
                    state.reset_counters();
                }
            });
        });
        ui.label(
            egui::RichText::new("Suramak Android shell spike — egui/wgpu in a GameActivity")
                .size(12.0)
                .color(egui::Color32::from_gray(160)),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for tab in Tab::ALL {
                let selected = state.tab == tab;
                if ui
                    .add_sized(
                        [ui.available_width() / 4.0, 46.0],
                        egui::Button::selectable(selected, tab.title()),
                    )
                    .clicked()
                {
                    state.tab = tab;
                }
            }
        });
        ui.add_space(4.0);
    });

    egui::TopBottomPanel::bottom("m13b-verdicts").show(ctx, |ui| {
        ui.add_space(3.0);
        for (name, verdict, detail) in state.verdicts() {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(verdict.label())
                        .size(13.0)
                        .strong()
                        .color(verdict_color(verdict)),
                );
                ui.label(egui::RichText::new(name).size(13.0).strong());
                ui.label(
                    egui::RichText::new(detail)
                        .size(13.0)
                        .color(egui::Color32::from_gray(170)),
                );
            });
        }
        ui.label(
            egui::RichText::new(format!("gpu: {}", state.backend))
                .size(10.0)
                .color(egui::Color32::from_gray(120)),
        );
        ui.add_space(3.0);
    });

    egui::CentralPanel::default().show(ctx, |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match state.tab {
                Tab::Text => draw_text_check(ui, state),
                Tab::Lifecycle => draw_lifecycle_check(ui, state),
                Tab::Motion => draw_motion_check(ui, state),
                Tab::Insets => draw_insets_check(ui, state),
            });
    });
}

fn section(ui: &mut egui::Ui, title: &str, body: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(title).size(19.0).strong());
    ui.label(
        egui::RichText::new(body)
            .size(14.0)
            .color(egui::Color32::from_gray(185)),
    );
    ui.add_space(6.0);
}

fn draw_text_check(ui: &mut egui::Ui, state: &mut SpikeState) {
    section(
        ui,
        "(1) Text input",
        "Type Indonesian below with the keyboard's autocorrect ON. Backspace across whole words. \
         The counter must move for every character the IME delivers.",
    );

    ui.checkbox(
        &mut state.ime_bridge,
        "GameTextInput → JNI bridge (winit 0.30.13 drops IME text events)",
    );
    ui.label(
        egui::RichText::new(
            "With the bridge off, soft-keyboard text has no path into egui at all — that is a \
             winit gap, not an egui verdict. See docs/m13-gate.md.",
        )
        .size(11.0)
        .color(egui::Color32::from_gray(140)),
    );
    ui.add_space(4.0);

    ui.add(
        egui::TextEdit::multiline(&mut state.text)
            .id(egui::Id::new(TEXT_FIELD_ID))
            .hint_text("Ketik di sini — misalnya: Saya sedang menguji papan ketik Android.")
            .desired_rows(6)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Body),
    );

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(format!(
            "characters received: {}   ·   deletions seen: {}",
            state.chars_received, state.deletions_seen
        ))
        .size(16.0)
        .monospace(),
    );
    ui.label(
        egui::RichText::new(format!(
            "IME updates: {}   ·   winit key-text chars: {}   ·   buffer: {} chars",
            state.ime_updates,
            state.winit_text_chars,
            state.text.chars().count()
        ))
        .size(13.0)
        .monospace()
        .color(egui::Color32::from_gray(165)),
    );
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "Go if: the buffer accepts a full sentence in Indonesian, autocorrect suggestions \
             can be accepted, one backspace deletes one character (and word-delete works after a \
             space), and the caret stays where you type.",
        )
        .size(12.0)
        .color(egui::Color32::from_gray(150)),
    );
}

/// Convert a Java UTF-16 index (what `State.selectionStart` reports) into an
/// egui character index. Identity for BMP text; clamps to the end.
pub fn utf16_to_char_index(text: &str, utf16_index: usize) -> usize {
    let mut units = 0usize;
    for (chars, ch) in text.chars().enumerate() {
        if units >= utf16_index {
            return chars;
        }
        units += ch.len_utf16();
    }
    text.chars().count()
}

/// Move the egui text cursor to the selection the IME reported (character
/// indices, already converted from the Java UTF-16 indices by `platform`).
pub fn set_text_cursor(ctx: &egui::Context, start: usize, end: usize) {
    let id = egui::Id::new(TEXT_FIELD_ID);
    if let Some(mut edit) = egui::widgets::text_edit::TextEditState::load(ctx, id) {
        let (min, max) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let range = egui::text::CCursorRange::two(
            egui::text::CCursor::new(min),
            egui::text::CCursor::new(max),
        );
        edit.cursor.set_char_range(Some(range));
        edit.store(ctx, id);
    }
}

fn draw_lifecycle_check(ui: &mut egui::Ui, state: &mut SpikeState) {
    section(
        ui,
        "(2) Lifecycle",
        "Type a marker into the probe, send the app to the background (Home / app switch), then \
         bring it back. The probe text and the resume counter must both survive the surface \
         destroy/recreate. \"Recreate activity\" calls Activity.recreate() through the shim.",
    );

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("probe:").size(15.0));
        ui.add(
            egui::TextEdit::singleline(&mut state.probe)
                .hint_text("type something here, then background the app")
                .desired_width(f32::INFINITY),
        );
    });

    ui.add_space(10.0);
    ui.label(
        egui::RichText::new(format!("resumes: {}", state.resumes))
            .size(17.0)
            .monospace(),
    );
    ui.label(
        egui::RichText::new(format!("surface rebuilds: {}", state.surface_recreates))
            .size(17.0)
            .monospace(),
    );
    ui.label(
        egui::RichText::new(format!("recreate requests: {}", state.recreate_requests))
            .size(17.0)
            .monospace(),
    );

    ui.add_space(10.0);
    if ui
        .add_sized(
            [ui.available_width(), 52.0],
            egui::Button::new("Recreate activity now"),
        )
        .clicked()
    {
        state.recreate_requested = true;
        state.recreate_requests += 1;
    }

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "Go if: the probe text is still there after Home → return AND after the recreate \
             button, with the resume counter at 2 or more.",
        )
        .size(12.0)
        .color(egui::Color32::from_gray(150)),
    );
}

fn draw_motion_check(ui: &mut egui::Ui, state: &mut SpikeState) {
    section(
        ui,
        "(3) Motion",
        "A 60 fps animation driven from the frame loop. Watch it for a few seconds: the measured \
         average must stay near the display rate and the motion must not stutter.",
    );

    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 96.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        egui::CornerRadius::same(10),
        egui::Color32::from_gray(28),
    );
    let phase = (state.anim_seconds * 0.6).sin() as f32 * 0.5 + 0.5;
    let bar_w = 64.0;
    let x = rect.left() + 10.0 + phase * (rect.width() - bar_w - 20.0);
    let bar = egui::Rect::from_min_size(
        egui::pos2(x, rect.top() + 22.0),
        egui::vec2(bar_w, rect.height() - 44.0),
    );
    painter.rect_filled(
        bar,
        egui::CornerRadius::same(8),
        egui::Color32::from_rgb(120, 190, 255),
    );

    ui.add_space(10.0);
    let fps = state
        .avg_fps()
        .map(|fps| format!("{fps:.1} fps"))
        .unwrap_or_else(|| "measuring…".to_owned());
    let frame_ms = state
        .avg_frame_ms()
        .map(|ms| format!("{ms:.2} ms/frame"))
        .unwrap_or_else(|| "—".to_owned());
    ui.label(egui::RichText::new(fps).size(24.0).strong().monospace());
    ui.label(egui::RichText::new(frame_ms).size(15.0).monospace());
    ui.label(
        egui::RichText::new(format!("frames drawn: {}", state.frames))
            .size(13.0)
            .color(egui::Color32::from_gray(160)),
    );

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "Go if: the bar moves smoothly at roughly 60 fps on this device and the reading stays \
             within a few fps of the panel rate while the keyboard is hidden.",
        )
        .size(12.0)
        .color(egui::Color32::from_gray(150)),
    );
}

fn draw_insets_check(ui: &mut egui::Ui, state: &mut SpikeState) {
    section(
        ui,
        "(4) Insets + back",
        "Values pushed from MainActivity over JNI (physical pixels). The back gesture must \
         increment the counter and must NOT leave the app.",
    );

    let ppp = ui.ctx().pixels_per_point().max(0.1);
    let rows = [
        ("top", state.insets.top),
        ("bottom", state.insets.bottom),
        ("left", state.insets.left),
        ("right", state.insets.right),
        ("ime bottom", state.insets.ime_bottom),
    ];
    egui::Grid::new("inset-grid")
        .num_columns(3)
        .spacing([18.0, 6.0])
        .show(ui, |ui| {
            ui.label(egui::RichText::new("inset").size(14.0).strong());
            ui.label(egui::RichText::new("px").size(14.0).strong());
            ui.label(egui::RichText::new("dp").size(14.0).strong());
            ui.end_row();
            for (name, px) in rows {
                ui.label(egui::RichText::new(name).size(15.0));
                ui.label(egui::RichText::new(px.to_string()).size(15.0).monospace());
                ui.label(
                    egui::RichText::new(format!("{:.1}", px as f32 / ppp))
                        .size(15.0)
                        .monospace(),
                );
                ui.end_row();
            }
        });

    ui.add_space(10.0);
    ui.label(
        egui::RichText::new(format!("inset updates: {}", state.insets.updates))
            .size(15.0)
            .monospace(),
    );
    ui.label(
        egui::RichText::new(format!("back presses intercepted: {}", state.back_presses))
            .size(20.0)
            .strong()
            .monospace(),
    );
    if let Some(err) = &state.gpu_error {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!("gpu error: {err}"))
                .size(12.0)
                .color(egui::Color32::from_rgb(240, 120, 120)),
        );
    }

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "Go if: top/bottom are non-zero with the system bars visible, the keyboard raises \
             \"ime bottom\", and the back gesture only increments the counter.",
        )
        .size(12.0)
        .color(egui::Color32::from_gray(150)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_counter_tracks_ime_insertions_and_deletions() {
        let mut state = SpikeState {
            text: "Saya".to_owned(),
            ..Default::default()
        };
        state.observe_text();
        assert_eq!(state.chars_received, 4);

        // Autocorrect replacing text: the buffer shrinks, then grows.
        state.text.clear();
        state.observe_text();
        state.text.push_str("Saya sedang");
        state.observe_text();
        assert_eq!(state.chars_received, 15);
        assert_eq!(state.deletions_seen, 4);
    }

    #[test]
    fn verdicts_start_waiting_and_turn_go_with_evidence() {
        let empty = SpikeState::default();
        assert!(empty.verdicts().iter().all(|(_, v, _)| *v == Verdict::Wait));

        let mut state = SpikeState {
            chars_received: 3,
            resumes: 2,
            probe: "marker".to_owned(),
            back_presses: 1,
            ime_updates: 1,
            ..Default::default()
        };
        state.set_insets(64, 48, 0, 0, 0);
        for _ in 0..30 {
            state.tick(1.0 / 60.0);
        }
        let verdicts = state.verdicts();
        assert!(
            verdicts.iter().all(|(_, v, _)| *v == Verdict::Go),
            "{verdicts:?}"
        );
    }

    #[test]
    fn fps_reading_needs_a_window_and_matches_the_fed_rate() {
        let mut state = SpikeState::default();
        assert_eq!(state.avg_fps(), None);
        for _ in 0..MAX_FRAME_SAMPLES {
            state.tick(0.02);
        }
        let fps = state.avg_fps().unwrap();
        assert!((fps - 50.0).abs() < 0.5, "{fps}");
        assert!((state.avg_frame_ms().unwrap() - 20.0).abs() < 0.5);
    }

    #[test]
    fn java_utf16_indices_map_to_char_indices() {
        assert_eq!(utf16_to_char_index("halo", 0), 0);
        assert_eq!(utf16_to_char_index("halo", 4), 4);
        // Out-of-range clamps to the end instead of panicking.
        assert_eq!(utf16_to_char_index("halo", 99), 4);
        // A non-BMP character counts as two UTF-16 units but one char.
        assert_eq!(utf16_to_char_index("a😀b", 3), 2);
        assert_eq!(utf16_to_char_index("a😀b", 1), 1);
    }

    #[test]
    fn reset_keeps_the_selected_tab_but_clears_evidence() {
        let mut state = SpikeState {
            tab: Tab::Insets,
            text: "halo".to_owned(),
            ..Default::default()
        };
        state.observe_text();
        state.back_presses = 2;
        state.reset_counters();
        assert_eq!(state.tab, Tab::Insets);
        assert_eq!(state.chars_received, 0);
        assert_eq!(state.back_presses, 0);
    }
}
