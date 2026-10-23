//! Window, wgpu surface and the frame loop — Android only.
//!
//! The shape is the one every winit 0.30 Android app has: `android_main` builds
//! an `EventLoop` bound to the `AndroidApp`, an `ApplicationHandler` creates the
//! window on `resumed`, and a `ControlFlow::Poll` loop draws every vsync.
//!
//! Lifecycle (v2.1, audited): the counters live in a process-wide
//! `OnceLock<Mutex<_>>` ([`state`]), *not* in this handler, because
//! `android_main` runs again after an Activity recreation. The whole **GPU
//! generation** — surface, device, queue, `egui_wgpu::Renderer`,
//! `egui_winit::State` **and the `egui::Context`** — is dropped on `suspended`
//! and rebuilt from scratch on `resumed`; the `Window` is kept, because winit's
//! Android window is a *logical* handle (`{ AndroidApp, redraw_requester }`) that
//! re-reads the current `ANativeWindow` from the `AndroidApp` on every
//! `raw_window_handle` call, and returns `HandleError::Unavailable` while
//! suspended. What v1 did *not* do — and what this file fixes — is stop the
//! event loop when the Activity is destroyed: winit 0.30.13 swallows
//! `MainEvent::Destroy` (`warn!("TODO: forward onDestroy notification")`), so a
//! `Poll` loop ran forever after the Activity died, holding an `AndroidApp` (and
//! its JNI global ref) per recreation. `MainActivity.onDestroy` now tells us
//! through `platform::activity_destroyed()`, and `about_to_wait` exits.
//!
//! **Why the context goes with the renderer (v2.1 fix).** v2 kept the
//! `egui::Context` on the handler and rebuilt only the GPU objects, which made
//! the resume path emit texture *updates* for ids the fresh renderer had never
//! allocated: `egui-wgpu-0.33.3/src/renderer.rs:657` panics with "Tried to update
//! a texture that has not been allocated yet." on the first frame after every
//! successful resume-rebuild (field report: crash #4, `thread <unnamed>`, first
//! frame after a resume). egui's context owns the texture *bookkeeping* and the
//! renderer owns the GPU copies: they are one pair, so the context now lives in
//! [`Gpu`] and is built by [`gpu_context::new_context`] on every build. No
//! `egui::Context` exists outside this module and [`crate::ui`]'s `&Context`
//! parameters, so "a context that outlives its renderer" is unrepresentable
//! rather than merely discouraged. The alternative — keeping the context and
//! teaching egui to re-send full texture state — needs API that does not exist
//! (a context cannot be told to forget its textures), so this is a full rebuild
//! by choice; the cost is egui's per-context UI state (caret, scroll, hover),
//! not the spike's, which is app-owned in [`SpikeState`]. See
//! `docs/reports/M13b3-spike-v2.1.md` §2.
//!
//! **v2.2: the surface belongs to the same generation — for one epoch only.**
//! The owner's next pass killed the texture panic and exposed the surface one
//! (`wgpu_core.rs:3793 Surface::configure: Surface does not support the
//! adapter's queue family`, on resume). The trace is in
//! `docs/WORKORDER-M13B4.md`: a retry pass at `3.40s` built a *new* gpu from the
//! window that `surfaceDestroyed` had just killed (`3.32s`), the resume at
//! `11.91s` found `self.gpu` already `Some` and kept it, and the outdated
//! handler then `configure()`d that stale `Surface`. Three rules now hold, and
//! they are enforced by [`crate::surface_lifecycle`] rather than by comment:
//!
//! 1. `suspended` drops the whole generation, **surface included** (`Gpu::drop`
//!    logs the release), and every `resumed` opens a new epoch and rebuilds
//!    through the single [`Handler::build_gpu`] entry — never by adopting a gpu
//!    that outlived a suspend. `Gpu::new` creates its surface from the
//!    `ANativeWindow` current *at that moment*, inside that resume.
//! 2. No gpu work while suspended: [`Handler::redraw`]'s first act is the frame
//!    gate, and a refused frame is counted, named and dropped before any input,
//!    timing or build; the retry loop asks the same policy before it starts.
//!    The gate closes on whichever surface signal lands first — Kotlin's
//!    `surfaceDestroyed` line (the Java callback runs ~80 ms before
//!    android-activity's looper hands winit its `Suspended`, and the field
//!    trace's raced init lived in that gap) or winit's `Suspended` itself. The
//!    `3.40s` raced init is unrepresentable, and if a path ever tries, the log
//!    says `build refused`. `Gpu::new` ends in `surface.configure`, so the build
//!    itself runs inside a panic boundary too: a configure against a window that
//!    died mid-build is recorded as an init failure, never an unwind that
//!    finishes the Activity.
//! 3. A dead surface (`Outdated`/`Lost`/`Timeout`) is **never** reconfigured:
//!    it is dropped and rebuilt exactly like a resume (fresh surface, fresh egui
//!    context), capped at [`MAX_AUTO_REBUILDS_PER_EVENT`] per lifecycle event,
//!    logged `rebuild (outdated) — fresh surface from the current window`.
//!
//! Every failure here is recorded, not fatal: GPU init failures are retried and
//! surfaced, a panic inside the frame is caught at the frame boundary, and the
//! surface can be rebuilt on demand from the UI ("Restart surface").

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::android::EventLoopBuilderExtAndroid;
use winit::window::{Window, WindowId};

use crate::diag;
use crate::gpu_context;
use crate::platform;
use crate::surface_lifecycle::{
    BuildTicket, SurfaceAction, SurfaceFault, SurfaceLifecycle, MAX_AUTO_REBUILDS_PER_EVENT,
};
use crate::ui::{self, SpikeState};

static STATE: OnceLock<Mutex<SpikeState>> = OnceLock::new();
/// How many times this process built a `Gpu` (first build vs. rebuilds).
static GPU_BUILDS: AtomicU32 = AtomicU32::new(0);
/// How many times `android_main` was entered (one per Activity instance).
static MAIN_ENTRIES: AtomicU32 = AtomicU32::new(0);

/// Attempts at GPU init per `android_main`, and the pause between them.
const GPU_ATTEMPTS: u32 = 3;
const GPU_RETRY_DELAY: Duration = Duration::from_secs(2);
/// How often the UI may ask Kotlin for a fresh insets push while its values are
/// known to be stale (see [`Handler::maybe_request_insets`]).
const INSETS_REQUEST_INTERVAL: Duration = Duration::from_secs(1);
/// How many automatic "drop everything, draw one clean frame" rebuilds a
/// contained frame panic gets before the app waits for the button.
const MAX_POST_PANIC_REPAINTS: u32 = 2;
/// The name the lifecycle gate goes by in the log (WORKORDER-M13B4, fix 2: "add
/// a startup-log line naming the gate when it trips"). It is a constant so the
/// report and the device log quote the same words.
const SURFACE_GATE: &str =
    "surface-gate: suspended (no ANativeWindow; the next resume owns the rebuild)";

/// The spike's single source of truth (see the module docs).
pub fn state() -> &'static Mutex<SpikeState> {
    STATE.get_or_init(|| Mutex::new(SpikeState::default()))
}

fn lock_state() -> std::sync::MutexGuard<'static, SpikeState> {
    state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn run(app: android_activity::AndroidApp, generation: u32) {
    init_logging();
    diag::init_panic_hook();

    let entry = MAIN_ENTRIES.fetch_add(1, Ordering::SeqCst) + 1;
    log::info!("android_main entered (entry #{entry}, activity generation {generation})");

    let event_loop = match EventLoop::builder().with_android_app(app).build() {
        Ok(event_loop) => event_loop,
        Err(err) => {
            log::error!("event loop creation failed: {err}");
            return;
        }
    };
    // The animation needs a frame every vsync; present mode Fifo caps it at the
    // panel rate, so this is a 60 fps request, not a busy loop.
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut handler = Handler {
        window: None,
        gpu: None,
        lifecycle: SurfaceLifecycle::new(),
        last_frame: Instant::now(),
        gpu_attempts: 0,
        next_gpu_retry: None,
        next_insets_request: None,
        panic_rebuilds: 0,
        generation,
    };
    if let Err(err) = event_loop.run_app(&mut handler) {
        log::error!("event loop exited with an error: {err}");
    }
    // Drop the observable resources while the Activity is still coherent, then
    // say so: this line (or its absence) is how the next launch knows the loop
    // ended cleanly.
    drop(handler);
    log::info!("M13b spike: android_main returned (entry #{entry})");
}

/// Install the log bridge: everything still reaches logcat, and the spike's own
/// targets are mirrored into the on-screen event ring.
fn init_logging() {
    static LOGGER: OnceLock<SpikeLogger> = OnceLock::new();
    let logger = LOGGER.get_or_init(|| SpikeLogger {
        android: android_logger::AndroidLogger::new(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("SuramakSpike"),
        ),
    });
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

struct SpikeLogger {
    android: android_logger::AndroidLogger,
}

impl log::Log for SpikeLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        log::Log::log(&self.android, record);
        // Only our own crates go into the on-screen ring: wgpu/winit emit enough
        // info-level chatter to push every platform event out of 40 lines.
        if record.target().starts_with("suramak") {
            let level = match record.level() {
                log::Level::Error => diag::Level::Error,
                log::Level::Warn => diag::Level::Warn,
                _ => diag::Level::Info,
            };
            diag::push(level, record.args().to_string());
        }
    }

    fn flush(&self) {}
}

struct Handler {
    window: Option<Arc<Window>>,
    /// The current GPU generation: surface, device, queue, renderer, egui state
    /// **and the `egui::Context`** (see the module docs). `None` while suspended
    /// or after a failed build.
    gpu: Option<Gpu>,
    /// The surface lifecycle policy: which epoch the gpu belongs to, whether a
    /// build or a frame may start at all, and the per-event rebuild budget. Every
    /// decision below goes through it (see [`crate::surface_lifecycle`]).
    lifecycle: SurfaceLifecycle,
    last_frame: Instant,
    /// GPU init attempts in this `android_main` (0 = never tried).
    gpu_attempts: u32,
    next_gpu_retry: Option<Instant>,
    /// Earliest time the UI may ask Kotlin for a fresh insets push.
    next_insets_request: Option<Instant>,
    /// Automatic clean repaints after a contained frame panic (bounded by
    /// [`MAX_POST_PANIC_REPAINTS`]).
    panic_rebuilds: u32,
    /// Which Activity instance this loop belongs to (see
    /// [`platform::activity_destroyed`]).
    generation: u32,
}

impl ApplicationHandler for Handler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let resumes = {
            let mut state = lock_state();
            state.resumes = state.resumes.saturating_add(1);
            state.resumes
        };
        // A resume opens a new surface epoch. Any generation built before this
        // event is bound to the `ANativeWindow` that just went away, so it is
        // dropped here **even when it looks healthy** — that is precisely the
        // v2.1 bug: a gpu built during the suspend path survived into the next
        // resume and was configured across a `surfaceDestroyed`.
        let ticket = self.lifecycle.on_resumed();
        // A hint that arrived with (or after) the resume describes the surface
        // that just came back: the epoch opened above is the authority, and a
        // stale hint would close the gate for the whole session.
        let _ = platform::take_surface_destroyed_hint();
        let had_gpu = self.gpu.take().is_some();
        if let Some(invalidated) = ticket.invalidated {
            log::warn!(
                "resume: gpu generation #{} (epoch {}) was still live at resume — invalidated; its \
                 surface belongs to the pre-resume window",
                invalidated.serial,
                invalidated.epoch
            );
        }
        if had_gpu {
            log::info!(
                "resume: previous gpu generation dropped before the rebuild (a surface is only valid \
                 for the epoch that created it)"
            );
        }
        log::info!(
            "event: resumed (#{resumes}) — surface epoch {} open, frame gate open",
            ticket.epoch
        );
        // Back to a frame-per-vsync loop: `suspended` parked it on `Wait`.
        event_loop.set_control_flow(ControlFlow::Poll);
        // A resume after a suspend gap must not feed a multi-second dt into the
        // frame-time window.
        self.last_frame = Instant::now();
        // Each lifecycle event gets its own init/retry budget: three attempts
        // spent an hour ago must not make this resume's build unretryable.
        self.gpu_attempts = 0;
        self.next_gpu_retry = None;

        if self.window.is_none() {
            match event_loop.create_window(Window::default_attributes().with_title("Suramak Spike"))
            {
                Ok(window) => {
                    log::info!("event: window created");
                    self.window = Some(Arc::new(window));
                }
                Err(err) => {
                    let message = format!("window creation failed: {err}");
                    log::error!("{message}");
                    lock_state().gpu_error = Some(message);
                    return;
                }
            }
        }
        let Some(window) = self.window.clone() else {
            return;
        };

        // Unconditional: the generation that would let us skip this is exactly
        // the one whose surface cannot be trusted. `build_gpu` is the same entry
        // the surface-fault path and the button use.
        self.build_gpu("resume");
        window.request_redraw();
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        // The ANativeWindow behind the surface is going away; drop every wgpu
        // object that references it — surface, device, queue, renderer and the
        // `egui::Context` with them (the context owns the texture bookkeeping
        // for the renderer being dropped). `Gpu::drop` logs the inventory; the
        // lifecycle's report is what proves nothing is left behind.
        let report = self.lifecycle.on_suspended();
        let had_gpu = self.gpu.take().is_some();
        log::info!(
            "event: suspended — surface epoch {} closed, frame gate closed (had gpu: {had_gpu})",
            report.epoch
        );
        if report.had_generation {
            log::info!(
                "event: suspended — dropped {} (the whole generation; the next resume builds a \
                 fresh surface from the window that is current then)",
                report.dropped_list()
            );
        }
        // A retry scheduled before the suspend must not fire into a dead window.
        self.next_gpu_retry = None;
        // There is nothing to draw while suspended, and `ControlFlow::Poll`
        // would spin a core for the whole time the app is in the background
        // (v1 did exactly that). Park on `Wait`; `resumed` restores Poll.
        event_loop.set_control_flow(ControlFlow::Wait);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if matches!(event, WindowEvent::CloseRequested) {
            log::info!("event: CloseRequested — exiting");
            event_loop.exit();
            return;
        }
        match &event {
            WindowEvent::Resized(size) => {
                log::info!("event: resized {}x{}", size.width, size.height);
            }
            WindowEvent::Focused(focused) => {
                log::info!("event: focus {focused}");
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                log::info!("event: scale factor {scale_factor}");
            }
            WindowEvent::Destroyed => {
                log::warn!("event: window destroyed");
            }
            _ => {}
        }
        // Evidence for the report: what (if anything) the winit path delivers.
        // Soft keyboards send characters over the InputConnection, not key
        // events, so this stays 0 while typing with the IME.
        if let WindowEvent::KeyboardInput { event: key, .. } = &event {
            if let Some(text) = &key.text {
                let chars = text.chars().count() as u64;
                if chars > 0 {
                    let mut state = lock_state();
                    state.winit_text_chars = state.winit_text_chars.saturating_add(chars);
                }
            }
        }
        if let (Some(gpu), WindowEvent::Resized(size)) = (self.gpu.as_mut(), &event) {
            // Only record it: the size is applied at the top of the next frame,
            // inside the frame's panic boundary. Configuring here would be a
            // second `surface.configure` call reachable outside that boundary —
            // the shape of the v2.1 panic.
            gpu.pending_size = Some(*size);
        }
        if let Some(gpu) = self.gpu.as_mut() {
            let response = gpu.egui_state.on_window_event(&gpu.window, &event);
            if response.repaint {
                gpu.window.request_redraw();
            }
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            self.redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if platform::activity_destroyed(self.generation) {
            log::info!("event: activity destroyed — exiting the event loop");
            event_loop.exit();
            return;
        }
        // The Java surface signal first: it is the earliest word that the window
        // behind the surface is gone (see `poll_surface_destroyed_hint`), and it
        // must close the gate before the retry loop below is consulted.
        self.poll_surface_destroyed_hint();
        self.maybe_retry_gpu(event_loop);
        // No redraw requests while suspended: there is no surface, and asking for
        // one would only feed the gate. v2.1's 3.40s raced init came from this
        // event-loop turn.
        if !self.lifecycle.phase().is_running() {
            return;
        }
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.window.request_redraw();
        }
    }
}

impl Handler {
    /// Take the Kotlin `surfaceDestroyed` signal and close the epoch with it.
    ///
    /// winit's `Suspended` is derived from android-activity's
    /// `APP_CMD_TERM_WINDOW`, which the looper processes *after* the Java
    /// callback ran — 80 ms in the owner's v2.1 trace, with the raced gpu init
    /// inside that gap. Acting on the Java line closes the gate one turn
    /// earlier; winit's own `suspended` then finds nothing left to drop and is
    /// the same transition (see `surface_lifecycle`).
    fn poll_surface_destroyed_hint(&mut self) {
        if !platform::take_surface_destroyed_hint() {
            return;
        }
        let was_running = self.lifecycle.phase().is_running();
        let report = self.lifecycle.on_surface_destroyed_hint();
        let had_gpu = self.gpu.take().is_some();
        if !was_running && !had_gpu {
            // A late duplicate of a transition already handled (winit's
            // `suspended` can land before the Kotlin line): nothing changed.
            return;
        }
        log::info!(
            "event: surfaceDestroyed (Kotlin) — surface epoch {} closed early, frame gate closed \
             (had gpu: {had_gpu}); winit's suspended may still be a turn away",
            report.epoch
        );
        if report.had_generation {
            log::info!(
                "event: surfaceDestroyed — dropped {} (the whole generation)",
                report.dropped_list()
            );
        }
        self.next_gpu_retry = None;
    }

    /// Build (or rebuild) one GPU generation: surface, device, queue, renderer,
    /// egui state **and a fresh `egui::Context`**.
    ///
    /// This is the *only* place that constructs a [`Gpu`], for every path —
    /// `resumed`, the retry loop, "Restart surface", a surface fault and the
    /// post-panic repaint — and the build only starts with a ticket from the
    /// lifecycle, which is issued only while an epoch is open. A suspended build
    /// is refused here even if a caller forgot to check (the 3.40s raced init:
    /// counted, named, never executed).
    fn build_gpu(&mut self, reason: &str) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let ticket = match self.lifecycle.begin_build() {
            Ok(ticket) => ticket,
            Err(refusal) => {
                log::warn!(
                    "gpu: build refused (reason={reason}) — frame gate [{SURFACE_GATE}] is closed \
                     (lifecycle={}, refused builds: {}); no surface is created from a destroyed \
                     window",
                    refusal.phase.as_str(),
                    self.lifecycle.refused_builds()
                );
                return;
            }
        };
        self.gpu_attempts = self.gpu_attempts.saturating_add(1);
        let attempt = self.gpu_attempts;
        let started = Instant::now();
        // `Gpu::new` ends in `surface.configure`, and configuring a surface
        // whose native window died mid-build is exactly the Mali panic the field
        // report carries. A build therefore gets the same boundary a frame has:
        // a panic becomes a recorded init failure, not an unwind through winit
        // (which would finish the Activity).
        let built =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Gpu::new(window, ticket)));
        match built {
            Ok(Ok(gpu)) => {
                // The epoch can only close while a build runs if the build
                // somehow spans a lifecycle event; refuse to adopt rather than
                // hand out a surface from a dead epoch.
                if !self.lifecycle.adopt(ticket) {
                    log::error!(
                        "gpu: generation #{} built for epoch {} but that epoch is closed — dropped \
                         instead of adopted",
                        ticket.serial,
                        ticket.epoch
                    );
                    return;
                }
                let adapter = gpu.adapter_info.clone();
                let previous_builds = GPU_BUILDS.fetch_add(1, Ordering::SeqCst);
                log::info!(
                    "gpu ready ({reason}, attempt {attempt}, {:.0} ms, generation #{}, epoch {}): \
                     {adapter}",
                    started.elapsed().as_secs_f64() * 1000.0,
                    ticket.serial,
                    ticket.epoch
                );
                log::info!(
                    "egui context rebuilt with it (fresh texture registry: every id re-registers \
                     through a full-image create)"
                );
                {
                    let mut state = lock_state();
                    state.backend = adapter;
                    state.gpu_error = None;
                    // The first build in the process is not a rebuild; anything
                    // after it is (suspend/resume or a new Activity).
                    if previous_builds > 0 {
                        state.surface_recreates = state.surface_recreates.saturating_add(1);
                    }
                }
                self.next_gpu_retry = None;
                self.gpu = Some(gpu);
            }
            Ok(Err(err)) => {
                log::error!("gpu init failed ({reason}, attempt {attempt}): {err}");
                let mut state = lock_state();
                state.gpu_error = Some(format!("{reason}: {err}"));
                drop(state);
                self.next_gpu_retry = Some(Instant::now() + GPU_RETRY_DELAY);
            }
            Err(payload) => {
                // Contained, not fatal: the surface is gone/being replaced and
                // the retry (or the next resume) is the recovery.
                let text = diag::payload_text(payload.as_ref());
                diag::note_contained("gpu init (surface)");
                log::error!(
                    "gpu init panicked ({reason}, attempt {attempt}): {text} — contained; no \
                     generation was adopted"
                );
                lock_state().gpu_error = Some(format!("{reason}: gpu init panicked: {text}"));
                self.next_gpu_retry = Some(Instant::now() + GPU_RETRY_DELAY);
            }
        }
    }

    /// Retry GPU init a couple of times before giving up — a driver that loses
    /// the surface once (a common OEM behaviour around suspend) usually accepts
    /// the next attempt, and without this the app would sit on a black screen
    /// with no way back.
    fn maybe_retry_gpu(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() || self.window.is_none() || self.gpu_attempts == 0 {
            return;
        }
        if self.gpu_attempts >= GPU_ATTEMPTS {
            return;
        }
        // The gate the v2.1 trace walked through: the retry must not start while
        // suspended (no window), and it must not restart the loop after the
        // surface-fault budget ran out. `begin_build` would refuse the first
        // case anyway; checking here keeps the timer from even being consulted.
        if !self.lifecycle.retry_allowed() {
            return;
        }
        let now = Instant::now();
        if let Some(at) = self.next_gpu_retry {
            if now < at {
                return;
            }
        }
        log::warn!(
            "gpu: retrying surface init (attempt {} of {GPU_ATTEMPTS})",
            self.gpu_attempts + 1
        );
        self.build_gpu("retry");
        if self.gpu.is_none() && self.gpu_attempts >= GPU_ATTEMPTS {
            log::error!("gpu: giving up after {GPU_ATTEMPTS} attempts — the Kotlin crash overlay is the fallback");
            // Nothing left to draw: stop the busy loop. Anything the owner can
            // still read is Kotlin's overlay, drawn on the UI thread.
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    /// Drop the wgpu surface and everything that depends on it — the egui
    /// context included — then rebuild against the current `ANativeWindow`.
    /// This is the suspend/resume path made user-triggerable, so check (2) can
    /// be repeated on demand instead of requiring an app switch.
    fn restart_surface(&mut self, reason: &str) {
        let Some(window) = self.window.clone() else {
            log::warn!("surface restart requested ({reason}) but there is no window");
            return;
        };
        // A human asking for a rebuild is new information: restore the automatic
        // budget so a session that spent it can recover on its own again.
        self.lifecycle.reset_auto_budget();
        let restarts = {
            let mut state = lock_state();
            state.surface_restarts = state.surface_restarts.saturating_add(1);
            state.surface_restarts
        };
        log::info!(
            "surface restart #{restarts} requested ({reason}) — dropping the whole gpu generation"
        );
        let dropped = self.gpu.take().is_some();
        self.lifecycle.release();
        log::info!(
            "surface restart #{restarts}: gpu dropped = {dropped}, rebuilding from the current window"
        );
        self.build_gpu("restart");
        match &self.gpu {
            Some(_) => log::info!("surface restart #{restarts}: complete"),
            None => log::error!("surface restart #{restarts}: FAILED — see gpu error"),
        }
        window.request_redraw();
    }

    fn redraw(&mut self) {
        // The Java `surfaceDestroyed` signal first (it can precede winit's
        // `Suspended`), then the gate.
        self.poll_surface_destroyed_hint();

        // The frame gate (v2.2) — the first act of every frame, before timing,
        // before the input drain, before any gpu work. A redraw that arrives
        // between `suspended` and the next `resumed` is a resume-in-waiting, not
        // a new gpu: v2.1's `3.40s gpu: retrying surface init (attempt 2 of 3)`
        // came from this turn, and the surface it built is the one the next
        // resume could not configure.
        if let Some(gated) = self.lifecycle.gate_frame() {
            let total = self.lifecycle.gated_frames();
            lock_state().frames_gated = total;
            // Name the gate on the first trip of a suspend (and sparsely after,
            // so a wedged loop stays visible without flooding the ring).
            if gated == 1 || gated % 120 == 0 {
                log::info!(
                    "frame gate tripped: [{SURFACE_GATE}] — redraw dropped ({gated} this suspend, \
                     {total} total); no gpu init or retry can start while suspended"
                );
            }
            return;
        }

        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        let (restart, recreate, ime_caret) = {
            let mut state = lock_state();
            state.tick(dt);
            let (insets, back_presses) = platform::drain();
            if let Some((top, bottom, left, right, ime_bottom)) = insets {
                state.set_insets(top, bottom, left, right, ime_bottom);
            }
            state.back_presses = state.back_presses.saturating_add(back_presses);

            // The IME's editing state is authoritative for the text field: it
            // arrives from the Kotlin `stateChanged` listener because winit
            // drops android-activity's text events. The buffer is app state, so
            // it is applied whether or not a surface exists; the caret can only
            // move inside a live egui context (see below).
            let mut caret = None;
            if state.ime_bridge {
                if let Some(ime) = platform::take_ime_text() {
                    state.ime_updates = state.ime_updates.saturating_add(1);
                    state.text = ime.text;
                    state.ime_cursor = Some((ime.selection_start, ime.selection_end));
                    caret = Some((ime.selection_start, ime.selection_end));
                }
            } else {
                // Keep the channel from replaying a stale buffer if the bridge
                // is switched back on later.
                let _ = platform::take_ime_text();
            }

            (
                std::mem::take(&mut state.restart_surface),
                std::mem::take(&mut state.recreate_requested),
                caret,
            )
        };

        // The caret position is egui *per-context* widget state, so it has to go
        // through the context that will draw the next frame. After a rebuild
        // that state is empty (documented cost); the next IME callback re-pushes
        // it, and `ui::set_text_cursor` is a no-op until the field has state.
        if let (Some((start, end)), Some(gpu)) = (ime_caret, self.gpu.as_ref()) {
            ui::set_text_cursor(&gpu.egui_ctx, start, end);
        }

        if restart {
            self.restart_surface("button");
        }

        if let Some(gpu) = self.gpu.as_mut() {
            // A panic while laying out or submitting a frame must not unwind
            // through winit into `android_main`: android-activity would then
            // finish the Activity and the app would close with no visible
            // reason. Contain it here, drop the poisoned generation (context
            // included), and let "Restart surface" (or the retry path) rebuild.
            //
            // The frame also *reports* a dead surface (`Outdated`/`Lost`/
            // `Timeout`) instead of handling it: `wgpu::Surface` is only valid
            // for the window it was created from, so the answer is a new
            // generation, never another `configure()`.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut state = lock_state();
                let fault = gpu.frame(&mut state);
                state.observe_text();
                fault
            }));
            match outcome {
                Ok(None) => {}
                Ok(Some(fault)) => self.on_surface_fault(fault),
                Err(payload) => {
                    let text = diag::payload_text(payload.as_ref());
                    diag::note_contained("frame (egui/wgpu)");
                    log::error!(
                        "frame panicked: {text} — dropping the whole gpu generation (egui context \
                         included); the rebuild draws one clean full repaint"
                    );
                    lock_state().gpu_error = Some(format!("frame panic: {text}"));
                    self.gpu = None;
                    self.lifecycle.release();
                    // One clean repaint, immediately: rebuild the generation
                    // (which drops the old context, its stale texture ids and the
                    // half-rendered frame) and let the next redraw present from
                    // scratch. Bounded, so a repeatable panic cannot spin here.
                    if self.panic_rebuilds < MAX_POST_PANIC_REPAINTS {
                        self.panic_rebuilds = self.panic_rebuilds.saturating_add(1);
                        log::warn!(
                            "frame: post-panic repaint {} of {MAX_POST_PANIC_REPAINTS} — rebuilding \
                             a fresh gpu generation now",
                            self.panic_rebuilds
                        );
                        self.build_gpu("post-panic repaint");
                    } else {
                        log::error!(
                            "frame: {MAX_POST_PANIC_REPAINTS} post-panic repaints already spent — \
                             tap Restart surface once the cause is understood"
                        );
                        lock_state().gpu_error =
                            Some(format!("frame panic: {text} — tap Restart surface"));
                        self.next_gpu_retry = Some(Instant::now() + GPU_RETRY_DELAY);
                    }
                }
            }
        }

        // Outside the state lock while the JNI call runs: it can block on the UI
        // thread. The outcome goes to the lifecycle tab's status line, because a
        // recreate that fails only in the log is indistinguishable from one that
        // worked (v2's did exactly that: `Java exception was thrown`, nothing on
        // screen, no recreate).
        if recreate {
            let outcome = platform::request_activity_recreate();
            let mut state = lock_state();
            match outcome {
                Ok(()) => {
                    log::info!("activity recreate requested — Kotlin posts Activity.recreate()");
                    state.note_recreate(Ok(()));
                }
                Err(err) => {
                    log::error!("activity recreate failed: {err}");
                    state.note_recreate(Err(&err));
                }
            }
        }

        self.maybe_request_insets();
    }

    /// A frame's surface is unusable (`Outdated`, `Lost`, `Timeout`).
    ///
    /// v2.1 answered this by calling `configure()` on the same `wgpu::Surface`.
    /// That object is bound to the `ANativeWindow` it was created from, so once
    /// that window is gone the call is not a recovery — it is the panic the
    /// owner's phone hit (`Surface does not support the adapter's queue
    /// family`). Here the generation is dropped and rebuilt through the same
    /// entry a resume uses, which creates the surface from the window that is
    /// current *now* and a fresh egui context with it.
    fn on_surface_fault(&mut self, fault: SurfaceFault) {
        match self.lifecycle.on_surface_fault(fault) {
            SurfaceAction::Rebuild { attempt } => {
                log::warn!(
                    "rebuild (outdated) — fresh surface from the current window (surface {}, \
                     auto rebuild {attempt} of {MAX_AUTO_REBUILDS_PER_EVENT}, epoch {}); the dead \
                     surface is never configured",
                    fault.as_str(),
                    self.lifecycle.epoch()
                );
                self.gpu = None;
                self.build_gpu("surface fault");
                if self.gpu.is_some() {
                    log::info!(
                        "rebuild (outdated): fresh surface + fresh egui context ready — first frame \
                         of the new generation presents a full repaint"
                    );
                }
            }
            SurfaceAction::Contained { attempts } => {
                log::error!(
                    "surface {} — {attempts} automatic rebuilds already spent in this lifecycle \
                     event; contained instead of spinning. Tap Restart surface.",
                    fault.as_str()
                );
                self.gpu = None;
                lock_state().gpu_error = Some(format!(
                    "surface {}: automatic rebuild budget spent ({attempts} of \
                     {MAX_AUTO_REBUILDS_PER_EVENT}) — tap Restart surface",
                    fault.as_str()
                ));
            }
        }
    }

    /// Ask Kotlin to re-push the window insets when the state says its values are
    /// stale (a reset, or a first push that never arrived).
    ///
    /// The platform only dispatches insets when they *change*, so a reset that
    /// zeroes the counters would otherwise leave the panel at 0/0 for the rest of
    /// the session — the v2.1 field report. At most one request per second, and
    /// the flag only clears when a real push arrives (`SpikeState::set_insets`),
    /// so a missed request retries instead of giving up.
    fn maybe_request_insets(&mut self) {
        let stale = lock_state().insets_stale;
        if !stale {
            return;
        }
        let now = Instant::now();
        if let Some(at) = self.next_insets_request {
            if now < at {
                return;
            }
        }
        self.next_insets_request = Some(now + INSETS_REQUEST_INTERVAL);
        match platform::request_insets_push() {
            Ok(()) => log::info!("insets: stale — asked Kotlin for a fresh push"),
            Err(err) => log::warn!("insets: stale and the push request failed: {err}"),
        }
    }
}

struct Gpu {
    /// Which lifecycle epoch this generation belongs to, and its process-local
    /// build serial. The surface below is only valid for `ticket.epoch` — that
    /// is the whole v2.2 invariant, and it is checked at adoption rather than
    /// trusted.
    ticket: BuildTicket,
    window: Arc<Window>,
    /// Created inside [`Gpu::new`] from the window handle current at that
    /// moment. Dropped with the rest of the generation on suspend (see
    /// [`impl Drop for Gpu`]).
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// The size a `Resized` event reported and no frame has applied yet. Kept
    /// out of the event handler so `surface.configure` has exactly two call
    /// sites, both inside the frame's panic boundary.
    pending_size: Option<winit::dpi::PhysicalSize<u32>>,
    /// The egui context *this* renderer serves. Owned here, deliberately: egui
    /// keeps the texture bookkeeping, `renderer` keeps the GPU textures, and a
    /// context that outlives its renderer emits patches for unallocated ids
    /// (`egui-wgpu` `renderer.rs:657`). One `Gpu` = one context = one registry.
    egui_ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    egui_state: egui_winit::State,
    /// The generation's first presented frame still owes a *clean* full repaint
    /// (see the module docs and `docs/reports/M13b3-spike-v2.1.md` §2.4). It is
    /// logged once, so the device log proves the repaint happened after a
    /// suspend or a contained panic — the frames that used to ghost.
    first_frame: bool,
    adapter_info: String,
}

impl Drop for Gpu {
    /// One line per generation death, naming the inventory. This is the
    /// device-visible half of the "the surface joins the generation" contract:
    /// every suspend, fault and rebuild prints exactly what it released, so a
    /// surface left behind would be a missing word in the log.
    fn drop(&mut self) {
        log::info!(
            "gpu generation #{} (epoch {}) dropped — surface, device, queue, renderer and egui \
             context released",
            self.ticket.serial,
            self.ticket.epoch
        );
    }
}

/// Map the three lifecycle `SurfaceError` variants; `OutOfMemory`/`Other` are
/// not lifecycle statements and stay on the log-and-retry path.
fn surface_fault(err: &wgpu::SurfaceError) -> Option<SurfaceFault> {
    match err {
        wgpu::SurfaceError::Outdated => Some(SurfaceFault::Outdated),
        wgpu::SurfaceError::Lost => Some(SurfaceFault::Lost),
        wgpu::SurfaceError::Timeout => Some(SurfaceFault::Timeout),
        _ => None,
    }
}

impl Gpu {
    fn new(window: Arc<Window>, ticket: BuildTicket) -> Result<Self, String> {
        // Built here, dropped with `self`: the context is part of the GPU
        // generation, not of the app (see the module docs).
        let egui_ctx = gpu_context::new_context();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            ..Default::default()
        });
        // The one line that binds a generation to a native window. It runs only
        // inside a `resumed`-owned build (the lifecycle issues the ticket), so
        // the `ANativeWindow` winit hands over here is the current one — never
        // the window a previous generation was created from.
        let surface = instance
            .create_surface(window.clone())
            .map_err(|err| format!("create_surface: {err}"))?;
        log::info!(
            "surface created for generation #{} (epoch {}) from the current window handle",
            ticket.serial,
            ticket.epoch
        );

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .map_err(|err| format!("request_adapter: {err}"))?;
        let info = adapter.get_info();
        let adapter_info = format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type);

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("suramak-android-spike"),
            ..Default::default()
        }))
        .map_err(|err| format!("request_device: {err}"))?;

        let caps = surface.get_capabilities(&adapter);
        if caps.formats.is_empty() {
            return Err("surface reported no texture formats".to_owned());
        }
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: Vec::new(),
        };
        surface.configure(&device, &config);

        let renderer =
            egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        log::info!(
            "surface configured: {format:?} {}x{}",
            config.width,
            config.height
        );

        Ok(Self {
            ticket,
            window,
            surface,
            device,
            queue,
            config,
            pending_size: None,
            egui_ctx,
            renderer,
            egui_state,
            first_frame: true,
            adapter_info,
        })
    }

    fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Draw and present one frame. `Some(fault)` means the surface itself is
    /// unusable and the *caller* must replace this whole generation — the frame
    /// deliberately does not touch a dying surface (v2.1's `configure()` on it
    /// is the field crash).
    fn frame(&mut self, state: &mut SpikeState) -> Option<SurfaceFault> {
        // Some Android paths (first frame after a resume, or a rotation that
        // does not deliver `Resized` before the redraw) leave the surface at a
        // stale size; syncing here keeps the viewport correct either way. This
        // is the only place a live generation reconfigures, and it runs inside
        // the caller's panic boundary.
        let pending = self.pending_size.take();
        let size = pending.unwrap_or_else(|| self.window.inner_size());
        if size.width > 0
            && size.height > 0
            && (size.width != self.config.width || size.height != self.config.height)
        {
            self.resize(size);
        }

        let raw_input = self.egui_state.take_egui_input(&self.window);
        let full_output = self.egui_ctx.run(raw_input, |ctx| ui::draw(ctx, state));
        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);
        let pixels_per_point = full_output.pixels_per_point;
        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, pixels_per_point);

        for (id, image_delta) in &full_output.textures_delta.set {
            self.renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }

        // Release the textures egui retired *before* anything can return early
        // (SUR2-NEW-07). The free list is one-shot: if a frame bails out on a
        // surface error first, the ids are never offered again and the renderer's
        // cache keeps those textures for the life of the surface. Freeing here is
        // safe — egui only frees textures no live paint job references, and every
        // paint job still holding an id was just re-uploaded above.
        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point,
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("suramak-spike-egui"),
            });
        let user_commands = self.renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(err) => {
                if let Some(fault) = surface_fault(&err) {
                    // No configure() here, by contract: report it and let the
                    // handler replace the generation (fresh surface, fresh egui
                    // context) the way a resume would.
                    return Some(fault);
                }
                log::error!("get_current_texture: {err}");
                return None;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("suramak-spike-egui-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            self.renderer
                .render(&mut pass, &paint_jobs, &screen_descriptor);
        }
        self.queue
            .submit(user_commands.into_iter().chain(Some(encoder.finish())));
        self.window.pre_present_notify();
        frame.present();

        if self.first_frame {
            self.first_frame = false;
            // The one clean repaint every generation owes: the pass cleared the
            // surface, the renderer allocated every texture from a full image and
            // the context had no stale ids to patch. This is the frame that used
            // to ghost (double-rendered glyphs) when a rebuild reused the old
            // context's atlas.
            log::info!(
                "first frame of this gpu generation presented — full repaint (cleared surface, \
                 fresh texture registry, no stale ids)"
            );
        }
        None
    }
}
