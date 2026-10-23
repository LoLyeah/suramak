//! The M13b v2 screen — Go/No-Go checks plus the self-diagnostics surface.
//!
//! Pure egui: no platform types, so this module compiles (and its tests run) on
//! the host as well as on Android. `app.rs` owns the window/surface and feeds
//! the state below; `platform.rs` owns the JNI surface that fills it; `diag.rs`
//! owns the crash record and the event ring this module renders.
//!
//! The four sections map 1:1 to the four things that can kill option A
//! (`docs/PLAN-M13-ANDROID.md` §2.1 / §M13b):
//!
//! 1. text input through the soft keyboard (GameTextInput → winit IME → egui);
//! 2. lifecycle — state survives surface teardown and activity recreation, plus
//!    the user-triggerable surface restart;
//! 3. motion — a 60 fps animation with a measured frame-time readout;
//! 4. insets and the back gesture, delivered from the Kotlin shim over JNI.
//!
//! v2 adds three always-present diagnostics elements: a red **crash banner**
//! (this session's panic or the previous launch's, read from the app's files
//! dir), a collapsible **event log** of the last [`crate::diag::RING_CAPACITY`]
//! platform callbacks, and per-tab panic isolation — a check that panics while
//! drawing is disabled on its own, with the panic text on screen and a retry
//! button, instead of taking the whole surface (and the other three checks)
//! down with it.
//!
//! v2.1 changes no check: the banner's file path follows the crash files to the
//! app's external files dir, and the **Share log** route lives on the Kotlin
//! overlay (it has to work when this module cannot draw at all).
//!
//! v2.3 (`docs/WORKORDER-M13B5.md`) makes the "Recreate activity" button a
//! **named limitation instead of an action**: [`RECREATE_UNAVAILABLE`] is the
//! one line it shows, [`RECREATE_EVIDENCE`] is why, and `SpikeState` has no flag
//! left for the frame loop to act on — the tap cannot reach Java, so it cannot
//! arm a relaunch (see `docs/reports/M13b5-spike-v2.3.md`). This module is pure
//! egui with no `platform` import, which is what makes that structural rather
//! than a promise: on the host, JNI does not even exist here.

use std::collections::VecDeque;

use crate::diag;

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
    /// Number of tabs — also the length of [`SpikeState::tab_errors`].
    pub const COUNT: usize = 4;

    pub fn title(self) -> &'static str {
        match self {
            Tab::Text => "1 · TEXT",
            Tab::Lifecycle => "2 · LIFECYCLE",
            Tab::Motion => "3 · MOTION",
            Tab::Insets => "4 · INSETS/BACK",
        }
    }

    /// Stable slot for per-tab state (panic isolation, resets).
    pub fn index(self) -> usize {
        match self {
            Tab::Text => 0,
            Tab::Lifecycle => 1,
            Tab::Motion => 2,
            Tab::Insets => 3,
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

/// What the "Recreate activity" button last did, shown under it.
///
/// `failed` keeps its v2.1 meaning — *the requested action did not happen* —
/// but the reason is no longer a Java exception: since v2.3 the button makes no
/// Java call at all, so the one thing it can report is [`RECREATE_UNAVAILABLE`].
/// `java_call` is the invariant made visible: the name of the Java method this
/// request invoked, or `None`. In v2.3 nothing can set it to `Some`, because
/// `platform::request_activity_recreate` no longer exists (the compiler, not a
/// test, is the primary guard — the tests pin the words).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecreateStatus {
    pub text: String,
    pub failed: bool,
    /// The Java method invoked for this request, if any. Always `None` here.
    pub java_call: Option<&'static str>,
}

/// The single line the "Recreate activity" button reports (v2.3).
///
/// It has to do three jobs at once: say the capability is not available, say
/// that **no Java call was made** (the whole point of the pass), and name the
/// evidence for why the call could never have worked. It must never read like a
/// VM failure — `Java exception was thrown` is exactly the sentence this pass
/// retires.
pub const RECREATE_UNAVAILABLE: &str =
    "recreate: not available — no Java call, nothing relaunched. The shim has no Activity \
     handle: ndk-context gives android-activity's Application, so MainActivity.postRecreate() \
     can only fail its method lookup and no Activity code ever ran.";

/// The evidence behind [`RECREATE_UNAVAILABLE`], shown under the button.
///
/// These are quotes and file:line facts, not a summary: the owner's device pass
/// has to be able to check the claim from the phone's own screen.
pub const RECREATE_EVIDENCE: [&str; 4] = [
    "v2.2 field: the button reported `MainActivity.postRecreate(): Java exception was thrown`, \
     and the session that tapped it was later ANR-killed after backgrounding (pid 1554, \
     Android 16).",
    "That sentence is jni 0.21.1's Display for *any* pending exception, and a failed \
     `GetMethodID` raises one (NoSuchMethodError) before a single Kotlin line runs — which is \
     why v2.1's exception-proof Kotlin body changed the message not at all.",
    "android-activity 0.6.1 initialises ndk-context with `activity.getApplication()` \
     (`src/init.rs:281`); ndk-context promises only \"in most cases … an Activity, but this \
     isn't guaranteed\". `postRecreate` on an Application is a lookup that cannot succeed.",
    "So no Activity code ran, and no relaunch was armed by that call. v2.3 does not swap in \
     another relaunch route: the tap now makes no JNI call (no attach, no lookup, no pending \
     exception, no relaunch). Whether that removes the ANR is the owner's device pass — the \
     click is the only place it could have come from.",
];

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
    /// "Restart surface" presses (the suspend/resume path, on demand).
    pub surface_restarts: u32,
    /// Redraws the surface gate dropped while the Activity was suspended (v2.2).
    /// Cumulative for the process: v2.1 turned one of these into a gpu init
    /// against a destroyed window, so this counter being non-zero *without* a
    /// rebuild is the fix working.
    pub frames_gated: u64,
    /// Set by the button, consumed by the frame loop.
    pub restart_surface: bool,
    /// Outcome of the last "Recreate activity" press (v2.1; re-purposed in v2.3
    /// as the *named limitation* — see [`RECREATE_UNAVAILABLE`]).
    pub recreate_status: Option<RecreateStatus>,
    // (3) motion
    pub anim_seconds: f64,
    frame_times: VecDeque<f32>,
    pub frames: u64,
    // (4) insets + back
    pub insets: Insets,
    /// `true` while the inset values on screen are known to be stale — set at
    /// start and by both reset buttons (which zero the counters). `app.rs` sees
    /// it and asks Kotlin to push a fresh set (`postPushInsets`); `set_insets`
    /// clears it. Without this, a reset left the panel at 0/0 forever, because
    /// the platform only dispatches insets when they *change*.
    pub insets_stale: bool,
    pub back_presses: u32,
    // diagnostics
    pub backend: String,
    pub gpu_error: Option<String>,
    /// Per-tab panic isolation: a tab that panicked while drawing is marked
    /// here and rendered as an error card instead, so the other checks keep
    /// working.
    pub tab_errors: [Option<String>; Tab::COUNT],
    /// Cached copy of [`crate::diag::ring`], refreshed on revision change.
    pub log_lines: Vec<diag::LogLine>,
    log_rev: u64,
    pub log_open: bool,
    /// Cached crash record (this session's or the previous launch's).
    pub crash: Option<diag::CrashView>,
    crash_rev: u64,
    pub crash_clear_error: Option<String>,
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
            surface_restarts: 0,
            frames_gated: 0,
            restart_surface: false,
            recreate_status: None,
            anim_seconds: 0.0,
            frame_times: VecDeque::with_capacity(MAX_FRAME_SAMPLES),
            frames: 0,
            insets: Insets::default(),
            insets_stale: true,
            back_presses: 0,
            backend: "not initialised".to_owned(),
            gpu_error: None,
            tab_errors: [None, None, None, None],
            log_lines: Vec::new(),
            log_rev: u64::MAX,
            log_open: false,
            crash: None,
            crash_rev: u64::MAX,
            crash_clear_error: None,
        }
    }
}

/// Frame-time window for the average fps readout (~2 s at 60 fps).
pub const MAX_FRAME_SAMPLES: usize = 120;

/// A gap longer than this is a suspend/restart, not a frame — feeding it into
/// the window would show a bogus 0.3 fps for two seconds after every resume.
const MAX_FRAME_GAP_SECS: f32 = 0.5;

impl SpikeState {
    /// Advance the animation clock and record one frame's duration.
    pub fn tick(&mut self, dt: f32) {
        self.frames += 1;
        self.anim_seconds += f64::from(dt.max(0.0));
        if dt.is_finite() && dt > 0.0 && dt <= MAX_FRAME_GAP_SECS {
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
        self.insets_stale = false;
    }

    /// The "Recreate activity" button, v2.3: record the named limitation.
    ///
    /// This is the button's *entire* effect. There is no Java call to make (see
    /// [`RECREATE_UNAVAILABLE`]) and no flag for the frame loop to consume, so a
    /// tap cannot reach Android's relaunch machinery at all — the property this
    /// pass is built on. `failed` is `true` because the requested action does not
    /// happen; the line says why, in the same place v2.1's error line lived, so
    /// the legibility contract survives even though the exception is gone.
    pub fn tap_recreate(&mut self) {
        self.recreate_requests = self.recreate_requests.saturating_add(1);
        self.recreate_status = Some(RecreateStatus {
            text: RECREATE_UNAVAILABLE.to_owned(),
            failed: true,
            java_call: None,
        });
    }

    /// Pull the crash record and the event ring only when they changed.
    pub fn refresh_diagnostics(&mut self) {
        let ring_rev = diag::ring_revision();
        if ring_rev != self.log_rev {
            self.log_rev = ring_rev;
            self.log_lines = diag::ring();
        }
        let crash_rev = diag::crash_revision();
        if crash_rev != self.crash_rev {
            self.crash_rev = crash_rev;
            self.crash = diag::crash_for_display();
        }
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
        self.surface_restarts = 0;
        self.frames_gated = 0;
        self.recreate_status = None;
        self.frame_times.clear();
        self.frames = 0;
        self.back_presses = 0;
        self.insets = Insets::default();
        self.insets_stale = true;
        self.anim_seconds = 0.0;
        self.tab_errors = [None, None, None, None];
    }

    /// Clear one check's evidence without touching the others — the isolation
    /// half of "a wedge in one check must not poison the rest".
    pub fn reset_tab(&mut self, tab: Tab) {
        match tab {
            Tab::Text => {
                self.text.clear();
                self.chars_received = 0;
                self.deletions_seen = 0;
                self.last_text_chars = 0;
                self.ime_updates = 0;
                self.ime_cursor = None;
                self.winit_text_chars = 0;
            }
            Tab::Lifecycle => {
                self.probe.clear();
                self.recreate_requests = 0;
                self.surface_restarts = 0;
                self.frames_gated = 0;
                self.recreate_status = None;
            }
            Tab::Motion => {
                self.frame_times.clear();
                self.frames = 0;
                self.anim_seconds = 0.0;
            }
            Tab::Insets => {
                self.insets = Insets::default();
                self.insets_stale = true;
                self.back_presses = 0;
            }
        }
        self.tab_errors[tab.index()] = None;
    }
}

fn verdict_color(v: Verdict) -> egui::Color32 {
    match v {
        Verdict::Go => egui::Color32::from_rgb(80, 200, 120),
        Verdict::Wait => egui::Color32::from_rgb(230, 180, 60),
    }
}

const RED: egui::Color32 = egui::Color32::from_rgb(255, 96, 96);
const AMBER: egui::Color32 = egui::Color32::from_rgb(235, 185, 70);
const DIM: egui::Color32 = egui::Color32::from_gray(150);

fn level_color(level: diag::Level) -> egui::Color32 {
    match level {
        diag::Level::Info => egui::Color32::from_gray(190),
        diag::Level::Warn => AMBER,
        diag::Level::Error => RED,
    }
}

/// Draw one frame. Returns nothing: all state lives in `state`.
pub fn draw(ctx: &egui::Context, state: &mut SpikeState) {
    state.refresh_diagnostics();
    draw_header(ctx, state);
    draw_crash_banner(ctx, state);
    draw_verdicts(ctx, state);
    draw_log_panel(ctx, state);

    egui::CentralPanel::default().show(ctx, |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| draw_tab(ui, state, state.tab));
    });
}

fn draw_header(ctx: &egui::Context, state: &mut SpikeState) {
    egui::TopBottomPanel::top("m13b-header").show(ctx, |ui| {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("M13B v2.3 DIAGNOSTICS")
                    .size(24.0)
                    .strong()
                    .color(egui::Color32::from_rgb(120, 190, 255)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("reset").clicked() {
                    state.reset_counters();
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Suramak Android shell spike — egui/wgpu in a GameActivity")
                    .size(11.0)
                    .color(DIM),
            );
            let crashes = diag::crash_count();
            if crashes > 0 {
                ui.label(
                    egui::RichText::new(format!("· {crashes} panic(s) this session"))
                        .size(11.0)
                        .color(RED),
                );
            }
        });
        ui.label(
            egui::RichText::new(
                "build 0.3.0-m13b.2.3 · recreate: named unavailable, no Java call · surface per epoch",
            )
            .size(10.0)
            .color(egui::Color32::from_gray(120)),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for tab in Tab::ALL {
                let selected = state.tab == tab;
                let broken = state.tab_errors[tab.index()].is_some();
                let label = if broken {
                    format!("{} !", tab.title())
                } else {
                    tab.title().to_owned()
                };
                if ui
                    .add_sized(
                        [ui.available_width() / 4.0, 44.0],
                        egui::Button::selectable(selected, label),
                    )
                    .clicked()
                {
                    state.tab = tab;
                }
            }
        });
        ui.add_space(4.0);
    });
}

/// The single most important element: whatever killed the process last time,
/// on screen, in red, until the owner presses **Clear crash**.
fn draw_crash_banner(ctx: &egui::Context, state: &mut SpikeState) {
    let Some(crash) = state.crash.as_ref() else {
        return;
    };
    let title = if crash.live {
        "CRASH — THIS SESSION (caught, app still running)"
    } else {
        "CRASH — LAST LAUNCH (read from the crash file)"
    };
    let body = crash.text.clone();

    egui::TopBottomPanel::top("m13b-crash")
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(58, 14, 14))
                .inner_margin(10.0),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(title).size(16.0).strong().color(RED));
            });
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(
                        egui::RichText::new("Clear crash").size(14.0).strong(),
                    ))
                    .clicked()
                {
                    match diag::clear_crash() {
                        Ok(()) => {
                            state.crash = None;
                            state.crash_clear_error = None;
                            state.crash_rev = diag::crash_revision();
                        }
                        Err(err) => state.crash_clear_error = Some(err),
                    }
                }
                ui.label(
                    egui::RichText::new(
                        "deletes spike-last-crash.txt + the history log (Android/data/\
                         app.suramak.spike/files — share it from the Kotlin banner)",
                    )
                    .size(10.0)
                    .color(DIM),
                );
            });
            egui::ScrollArea::vertical()
                .max_height(170.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(&body)
                            .monospace()
                            .size(10.5)
                            .color(egui::Color32::from_rgb(255, 210, 205)),
                    );
                });
            if let Some(err) = &state.crash_clear_error {
                ui.label(
                    egui::RichText::new(format!("clear failed: {err}"))
                        .size(11.0)
                        .color(AMBER),
                );
            }
        });
}

fn draw_verdicts(ctx: &egui::Context, state: &mut SpikeState) {
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
            egui::RichText::new(format!(
                "gpu: {} · restarts {} · rebuilds {} T{} · uptime {:.0}s",
                state.backend,
                state.surface_restarts,
                state.surface_recreates,
                state.resumes,
                diag::uptime_secs(),
            ))
            .size(10.0)
            .color(egui::Color32::from_gray(120)),
        );
        if let Some(err) = &state.gpu_error {
            ui.label(
                egui::RichText::new(format!("gpu error: {err}"))
                    .size(10.5)
                    .color(RED),
            );
        }
        ui.add_space(3.0);
    });
}

/// The always-visible (collapsible) event ring. Collapsed, the header still
/// shows the newest line, so a screenshot of any tab carries the last event.
fn draw_log_panel(ctx: &egui::Context, state: &mut SpikeState) {
    let open = state.log_open;
    let count = state.log_lines.len();
    let dropped = diag::dropped_lines();
    let newest = state
        .log_lines
        .last()
        .map(|line| (format!("[{:7.2}s]", line.at), line.level, line.text.clone()));

    egui::TopBottomPanel::bottom("m13b-log")
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(18, 18, 20))
                .inner_margin(6.0),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let arrow = if open { "▾" } else { "▸" };
                let header = format!("{arrow} LOG — {count} events");
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(header)
                                .monospace()
                                .size(12.0)
                                .color(egui::Color32::from_rgb(120, 190, 255)),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    state.log_open = !open;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("↻ surface").clicked() {
                        state.restart_surface = true;
                    }
                    if dropped > 0 {
                        ui.label(
                            egui::RichText::new(format!("{dropped} dropped"))
                                .size(9.0)
                                .color(egui::Color32::from_gray(110)),
                        );
                    }
                });
            });

            if open {
                egui::ScrollArea::vertical()
                    .max_height(150.0)
                    .stick_to_bottom(true)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if state.log_lines.is_empty() {
                            ui.label(
                                egui::RichText::new("no events yet")
                                    .size(10.5)
                                    .color(egui::Color32::from_gray(110)),
                            );
                        }
                        for line in &state.log_lines {
                            ui.label(
                                egui::RichText::new(format!(
                                    "[{:7.2}s] {} {}",
                                    line.at,
                                    line.level.tag(),
                                    line.text
                                ))
                                .monospace()
                                .size(10.0)
                                .color(level_color(line.level)),
                            );
                        }
                    });
            } else if let Some((at, level, text)) = newest {
                ui.label(
                    egui::RichText::new(format!("{at} {} {text}", level.tag()))
                        .monospace()
                        .size(10.0)
                        .color(level_color(level)),
                );
            }
        });
}

/// Draw one tab with panic isolation: a panic inside a check disables that tab
/// (with its message on screen) and leaves the other three untouched.
fn draw_tab(ui: &mut egui::Ui, state: &mut SpikeState, tab: Tab) {
    let index = tab.index();
    if let Some(message) = state.tab_errors[index].clone() {
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(format!("{} is disabled after a panic", tab.title()))
                .size(18.0)
                .strong()
                .color(RED),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(message)
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(255, 200, 195)),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Retry this tab").clicked() {
                state.tab_errors[index] = None;
            }
            if ui.button("Reset this check").clicked() {
                state.reset_tab(tab);
            }
        });
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "The other checks are unaffected: each tab keeps its own state and is drawn \
                 inside its own catch_unwind. The panic is in the event log and the crash files.",
            )
            .size(11.0)
            .color(DIM),
        );
        return;
    }

    // SAFETY (unwind safety): the closure mutates only `SpikeState` fields that
    // are counters and buffers; a panic mid-draw leaves a value stale, never a
    // dangling one. egui rebuilds its per-frame layout state each frame, so a
    // partially laid-out `Ui` is discarded by the next `Context::run`.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match tab {
        Tab::Text => draw_text_check(ui, state),
        Tab::Lifecycle => draw_lifecycle_check(ui, state),
        Tab::Motion => draw_motion_check(ui, state),
        Tab::Insets => draw_insets_check(ui, state),
    }));
    if let Err(payload) = result {
        let message = diag::payload_text(payload.as_ref());
        diag::note_contained("tab draw");
        state.tab_errors[index] = Some(message);
    }
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
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Reset this check").clicked() {
            state.reset_tab(Tab::Text);
        }
    });
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
         destroy/recreate. \"Recreate activity\" is NOT available in this shim and makes no Java \
         call at all — see the evidence under the button.",
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
        egui::RichText::new(format!(
            "surface restarts (button): {}",
            state.surface_restarts
        ))
        .size(17.0)
        .monospace(),
    );
    ui.label(
        egui::RichText::new(format!(
            "frames gated while suspended: {}",
            state.frames_gated
        ))
        .size(17.0)
        .monospace(),
    );
    ui.label(
        egui::RichText::new(format!("recreate requests: {}", state.recreate_requests))
            .size(17.0)
            .monospace(),
    );
    ui.label(
        egui::RichText::new("recreate java calls: 0 — the button has no Java path (v2.3)")
            .size(12.0)
            .monospace()
            .color(egui::Color32::from_gray(150)),
    );

    ui.add_space(10.0);
    // v2.3: the capability is gone, the *report* is not. The button stays
    // tappable because the owner's pass taps exactly this control, and a
    // greyed-out widget would swallow the tap and leave nothing to read; what
    // changed is the action: `SpikeState::tap_recreate` only writes the reason
    // into the state below. There is no `recreate_requested` flag any more, so
    // even a future edit to the frame loop has nothing to consume.
    if ui
        .add_sized(
            [ui.available_width(), 52.0],
            egui::Button::new("Recreate activity — NOT AVAILABLE (no Java call)"),
        )
        .clicked()
    {
        state.tap_recreate();
        diag::info(
            "recreate: not available — the button made no JNI/Java call (no Activity handle in \
             ndk-context; Activity.recreate() is not attempted on this path)",
        );
    }
    // The outcome, in the same place v2.1's error line lived. v2 said nothing
    // here while the shim was throwing; v2.3 says exactly what did not happen
    // and why — with no exception to quote, because no Java call is made.
    if let Some(status) = &state.recreate_status {
        ui.label(
            egui::RichText::new(&status.text)
                .size(11.5)
                .color(if status.failed { RED } else { AMBER }),
        );
        ui.label(
            egui::RichText::new(match status.java_call {
                Some(method) => format!("java call: {method}"),
                None => "java call: none".to_owned(),
            })
            .size(11.0)
            .monospace()
            .color(egui::Color32::from_gray(150)),
        );
    }
    ui.add_space(6.0);
    for (index, line) in RECREATE_EVIDENCE.iter().enumerate() {
        ui.label(
            egui::RichText::new(format!("{}. {line}", index + 1))
                .size(11.0)
                .color(egui::Color32::from_gray(150)),
        );
        ui.add_space(2.0);
    }
    ui.label(
        egui::RichText::new(
            "What replaces it is the path the device already verified: Home → return (resumes +1, \
             same live process) and \"Restart surface\" (drop + rebuild the whole gpu generation on \
             demand). Neither asks Android to relaunch the Activity.",
        )
        .size(11.5)
        .color(egui::Color32::from_gray(150)),
    );
    ui.label(
        egui::RichText::new(
            "v2.3 removed the call rather than dressing it up: there is no `postRecreate` left in \
             the Kotlin shim and no `request_activity_recreate` left in Rust, so the button cannot \
             reach Android's relaunch machinery even by accident. The recreate path is a documented \
             shim limitation — later milestones never need Activity.recreate on Android.",
        )
        .size(11.5)
        .color(egui::Color32::from_gray(150)),
    );
    ui.add_space(6.0);
    if ui
        .add_sized(
            [ui.available_width(), 52.0],
            egui::Button::new("Restart surface (drop + rebuild wgpu)"),
        )
        .clicked()
    {
        state.restart_surface = true;
    }
    ui.label(
        egui::RichText::new(
            "Restart surface drops the wgpu surface, device, queue, renderer and egui context and \
             rebuilds them against the current window — the same path a suspend/resume takes, on \
             demand. The context goes with the renderer on purpose: a kept context patches texture \
             ids the new renderer never allocated (the v2 crash). If it fails, the reason is in the \
             log line and the gpu error above.",
        )
        .size(11.5)
        .color(egui::Color32::from_gray(150)),
    );

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "Go if: the probe text is still there after Home → return, with the resume counter at 2 \
             or more and the app still the same live process (no fresh start, no crash banner).",
        )
        .size(12.0)
        .color(egui::Color32::from_gray(150)),
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Reset this check").clicked() {
            state.reset_tab(Tab::Lifecycle);
        }
    });
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
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Reset this check").clicked() {
            state.reset_tab(Tab::Motion);
        }
    });
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
    if state.insets_stale && state.insets.updates == 0 {
        ui.label(
            egui::RichText::new(
                "no push yet — asked Kotlin for one; it also re-pushes on resume, focus gain and \
                 every insets dispatch",
            )
            .size(11.0)
            .color(AMBER),
        );
    }
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
             \"ime bottom\", and the back gesture only increments the counter. Back is intercepted \
             on all three Android paths (OnBackInvokedCallback, OnBackPressedDispatcher, back key \
             events) — every press must count once and the app must never leave.",
        )
        .size(12.0)
        .color(egui::Color32::from_gray(150)),
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Reset this check").clicked() {
            state.reset_tab(Tab::Insets);
        }
    });
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
    fn a_suspend_gap_is_not_a_frame_time() {
        let mut state = SpikeState::default();
        for _ in 0..30 {
            state.tick(1.0 / 60.0);
        }
        // A 30 s resume gap must not enter the window.
        state.tick(30.0);
        let fps = state.avg_fps().unwrap();
        assert!((fps - 60.0).abs() < 1.0, "{fps}");
        assert_eq!(state.frames, 31);
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
        state.set_insets(64, 48, 0, 0, 0);
        state.reset_counters();
        assert_eq!(state.tab, Tab::Insets);
        assert_eq!(state.chars_received, 0);
        assert_eq!(state.back_presses, 0);
        // The reset zeroed the insets, so they are stale until Kotlin pushes
        // again — that is what keeps the panel from sitting at 0/0.
        assert!(state.insets_stale);
    }

    #[test]
    fn tab_indices_are_unique_and_cover_the_array() {
        let mut seen = [false; Tab::COUNT];
        for tab in Tab::ALL {
            let index = tab.index();
            assert!(index < Tab::COUNT);
            assert!(!seen[index], "duplicate index for {tab:?}");
            seen[index] = true;
        }
        assert!(seen.iter().all(|slot| *slot));
    }

    #[test]
    fn reset_tab_touches_only_that_check() {
        let mut state = SpikeState {
            text: "halo".to_owned(),
            probe: "lifecycle-123".to_owned(),
            back_presses: 3,
            tab_errors: [Some("boom".to_owned()), None, None, None],
            ..Default::default()
        };
        state.observe_text();
        state.set_insets(64, 48, 0, 0, 0);
        assert!(!state.insets_stale, "a push must clear the stale flag");

        state.reset_tab(Tab::Text);
        assert_eq!(state.chars_received, 0);
        assert_eq!(state.text, "");
        // Other checks are untouched.
        assert_eq!(state.probe, "lifecycle-123");
        assert_eq!(state.back_presses, 3);
        assert_eq!(state.insets.top, 64);
        // And the tab's own error flag is cleared, not the others'.
        assert_eq!(state.tab_errors[0], None);

        state.reset_tab(Tab::Insets);
        assert_eq!(state.back_presses, 0);
        assert_eq!(state.insets.top, 0);
        // The zeroed values are stale by definition: the frame loop must ask
        // Kotlin for a fresh push, or the panel stays 0/0 forever (v2.1 field
        // report).
        assert!(
            state.insets_stale,
            "an insets reset must ask for a fresh push"
        );
        assert_eq!(state.probe, "lifecycle-123");
    }

    /// v2.3: the recreate button is a *named limitation*, not an action. What
    /// must never come back is the Java call — and with it the sentence
    /// `Java exception was thrown`, which was a method-lookup failure dressed up
    /// as an Activity failure. This is the v2.1 legibility test, extended: the
    /// line still has to say why, the reset still has to clear it.
    #[test]
    fn a_recreate_tap_names_the_limitation_and_records_no_java_call() {
        let mut state = SpikeState::default();
        assert!(
            state.recreate_status.is_none(),
            "nothing is claimed before the button is pressed"
        );

        state.tap_recreate();
        let status = state.recreate_status.clone().expect("a status line");
        assert!(status.failed, "the requested recreate did not happen");
        assert_eq!(
            status.java_call, None,
            "v2.3 makes no Java call for this button"
        );
        assert!(status.text.contains("no Java call"), "{}", status.text);
        assert!(
            status.text.contains("ndk-context"),
            "the line must name the evidence, not just the verdict: {}",
            status.text
        );
        // The v2.1/v2.2 wording is retired: there is no VM exception to quote,
        // and quoting one would claim a call that never happened.
        assert!(!status.text.contains("Java exception"), "{}", status.text);
        assert!(!status.text.contains("recreate failed:"), "{}", status.text);
        assert_eq!(state.recreate_requests, 1, "the tap is still counted");

        // Pressing twice counts twice and says the same thing.
        state.tap_recreate();
        assert_eq!(state.recreate_requests, 2);
        assert_eq!(state.recreate_status.as_ref().unwrap().java_call, None);

        state.reset_tab(Tab::Lifecycle);
        assert!(
            state.recreate_status.is_none(),
            "reset clears the status line"
        );
        assert_eq!(state.recreate_requests, 0);
    }

    /// The tap's only effects are the two it documents. Anything else would mean
    /// an "unavailable" button had started an action — the exact shape of the
    /// v2.2 defect this pass removes.
    #[test]
    fn a_recreate_tap_touches_nothing_the_frame_loop_consumes() {
        let mut state = SpikeState {
            probe: "lifecycle-123".to_owned(),
            resumes: 4,
            frames_gated: 7,
            ..Default::default()
        };
        state.tap_recreate();
        assert!(
            !state.restart_surface,
            "a recreate tap must not start a surface restart"
        );
        assert_eq!(state.probe, "lifecycle-123", "the probe is app state");
        assert_eq!(state.resumes, 4);
        assert_eq!(state.frames_gated, 7);
        assert_eq!(state.text, "");
    }

    #[test]
    fn a_panicking_tab_is_disabled_without_taking_the_others_down() {
        let mut state = SpikeState::default();
        // Simulate what `draw_tab` does on a panic, without an egui context.
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| panic!("draw blew up")));
        if let Err(payload) = result {
            state.tab_errors[Tab::Text.index()] = Some(crate::diag::payload_text(payload.as_ref()));
        }
        assert!(state.tab_errors[0].is_some());
        assert!(state.tab_errors[1..].iter().all(Option::is_none));
        state.reset_tab(Tab::Text);
        assert!(state.tab_errors[0].is_none());
    }

    /// Drive the real `draw` for a few frames at phone size. egui needs no GPU
    /// for layout + tessellation, so this is the only place the panels, the
    /// crash banner, the log panel and all four tab bodies are executed off the
    /// device — a panic here would be a panic on the phone.
    fn draw_frames(ctx: &egui::Context, state: &mut SpikeState, frames: usize) {
        for _ in 0..frames {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 360.0),
                )),
                ..Default::default()
            };
            let output = ctx.run(raw, |ctx| draw(ctx, state));
            assert!(!output.shapes.is_empty(), "nothing was drawn");
        }
    }

    #[test]
    fn the_whole_screen_draws_headlessly_on_every_tab() {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(2.75);
        // The crash banner only exists when the diag buffer holds a record:
        // make one the same way the process does, then draw with it present.
        crate::diag::init_panic_hook();
        let _ = std::panic::catch_unwind(|| panic!("headless banner fixture"));
        crate::diag::info("jni: nativeSetInsets(top=64, bottom=48, left=0, right=0, ime=0)");

        let mut state = SpikeState {
            text: "Saya sedang menguji papan ketik.".to_owned(),
            probe: "lifecycle-123".to_owned(),
            ..Default::default()
        };
        state.observe_text();
        state.set_insets(64, 48, 0, 0, 0);
        state.back_presses = 2;
        // Exercise the v2.3 status line too: an unavailable recreate is a draw
        // path like any other, and its evidence block is the longest text on the
        // lifecycle tab.
        state.tap_recreate();
        for _ in 0..30 {
            state.tick(1.0 / 60.0);
        }

        for open in [false, true] {
            state.log_open = open;
            for tab in Tab::ALL {
                state.tab = tab;
                draw_frames(&ctx, &mut state, 2);
            }
        }
        assert!(state.crash.is_some(), "the banner had no crash to show");
        assert!(!state.log_lines.is_empty(), "the log panel had no events");
    }

    #[test]
    fn a_disabled_tab_draws_its_error_card_and_the_rest_still_draw() {
        let ctx = egui::Context::default();
        let mut state = SpikeState::default();
        state.tab_errors[Tab::Motion.index()] = Some("frame math exploded".to_owned());
        state.tab = Tab::Motion;
        draw_frames(&ctx, &mut state, 2);
        assert!(state.tab_errors[Tab::Motion.index()].is_some());

        // The other tabs still draw with the same state.
        for tab in [Tab::Text, Tab::Lifecycle, Tab::Insets] {
            state.tab = tab;
            draw_frames(&ctx, &mut state, 1);
        }
    }
}
