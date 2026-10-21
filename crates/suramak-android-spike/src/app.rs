//! Window, wgpu surface and the frame loop — Android only.
//!
//! The shape is the one every winit 0.30 Android app has: `android_main` builds
//! an `EventLoop` bound to the `AndroidApp`, an `ApplicationHandler` creates the
//! window on `resumed`, and a `ControlFlow::Poll` loop draws every vsync.
//!
//! Lifecycle (v2, audited): the counters live in a process-wide
//! `OnceLock<Mutex<_>>` ([`state`]), *not* in this handler, because
//! `android_main` runs again after an Activity recreation. The GPU objects
//! (surface, device, queue, renderer, `egui_winit::State`) are dropped on
//! `suspended` and rebuilt on `resumed`; the `Window` is kept, because winit's
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
use crate::platform;
use crate::ui::{self, SpikeState};

static STATE: OnceLock<Mutex<SpikeState>> = OnceLock::new();
/// How many times this process built a `Gpu` (first build vs. rebuilds).
static GPU_BUILDS: AtomicU32 = AtomicU32::new(0);
/// How many times `android_main` was entered (one per Activity instance).
static MAIN_ENTRIES: AtomicU32 = AtomicU32::new(0);

/// Attempts at GPU init per `android_main`, and the pause between them.
const GPU_ATTEMPTS: u32 = 3;
const GPU_RETRY_DELAY: Duration = Duration::from_secs(2);

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
        egui_ctx: egui_context(),
        last_frame: Instant::now(),
        gpu_attempts: 0,
        next_gpu_retry: None,
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

fn egui_context() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_visuals(egui::Visuals::dark());
    // Phone-sized defaults: the spike is read at arm's length.
    ctx.style_mut(|style| {
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(17.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(16.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(24.0));
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 10.0);
    });
    ctx
}

struct Handler {
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    egui_ctx: egui::Context,
    last_frame: Instant,
    /// GPU init attempts in this `android_main` (0 = never tried).
    gpu_attempts: u32,
    next_gpu_retry: Option<Instant>,
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
        log::info!("event: resumed (#{resumes})");
        // Back to a frame-per-vsync loop: `suspended` parked it on `Wait`.
        event_loop.set_control_flow(ControlFlow::Poll);
        // A resume after a suspend gap must not feed a multi-second dt into the
        // frame-time window.
        self.last_frame = Instant::now();

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

        if self.gpu.is_none() {
            self.build_gpu("resume");
        }
        window.request_redraw();
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        // The ANativeWindow behind the surface is going away; drop every wgpu
        // object that references it. The `Window` itself is kept: it is a
        // logical handle over `AndroidApp` and re-reads the native window on the
        // next `raw_window_handle` (see the module docs).
        let had_gpu = self.gpu.is_some();
        log::info!("event: suspended — releasing wgpu objects (had gpu: {had_gpu})");
        self.gpu = None;
        if had_gpu {
            log::info!("event: suspended — surface/device/queue/renderer dropped");
        }
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
            gpu.resize(*size);
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
        self.maybe_retry_gpu(event_loop);
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.window.request_redraw();
        }
    }
}

impl Handler {
    /// Build (or rebuild) the wgpu surface and renderer.
    fn build_gpu(&mut self, reason: &str) {
        let Some(window) = self.window.clone() else {
            return;
        };
        self.gpu_attempts = self.gpu_attempts.saturating_add(1);
        let attempt = self.gpu_attempts;
        let started = Instant::now();
        match Gpu::new(window, self.egui_ctx.clone()) {
            Ok(gpu) => {
                let adapter = gpu.adapter_info.clone();
                let previous_builds = GPU_BUILDS.fetch_add(1, Ordering::SeqCst);
                log::info!(
                    "gpu ready ({reason}, attempt {attempt}, {:.0} ms): {adapter}",
                    started.elapsed().as_secs_f64() * 1000.0
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
            Err(err) => {
                log::error!("gpu init failed ({reason}, attempt {attempt}): {err}");
                let mut state = lock_state();
                state.gpu_error = Some(format!("{reason}: {err}"));
                drop(state);
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

    /// Drop the wgpu surface and everything that depends on it, then rebuild
    /// against the current `ANativeWindow`. This is the suspend/resume path made
    /// user-triggerable, so check (2) can be repeated on demand instead of
    /// requiring an app switch.
    fn restart_surface(&mut self, reason: &str) {
        let Some(window) = self.window.clone() else {
            log::warn!("surface restart requested ({reason}) but there is no window");
            return;
        };
        let restarts = {
            let mut state = lock_state();
            state.surface_restarts = state.surface_restarts.saturating_add(1);
            state.surface_restarts
        };
        log::info!("surface restart #{restarts} requested ({reason}) — dropping wgpu objects");
        let dropped = self.gpu.take().is_some();
        log::info!("surface restart #{restarts}: gpu dropped = {dropped}, rebuilding");
        self.build_gpu("restart");
        match &self.gpu {
            Some(_) => log::info!("surface restart #{restarts}: complete"),
            None => log::error!("surface restart #{restarts}: FAILED — see gpu error"),
        }
        window.request_redraw();
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        let (restart, recreate) = {
            let mut state = lock_state();
            state.tick(dt);
            let (insets, back_presses) = platform::drain();
            if let Some((top, bottom, left, right, ime_bottom)) = insets {
                state.set_insets(top, bottom, left, right, ime_bottom);
            }
            state.back_presses = state.back_presses.saturating_add(back_presses);

            // The IME's editing state is authoritative for the text field: it
            // arrives from the Kotlin `stateChanged` listener because winit
            // drops android-activity's text events. Apply it *before* the
            // widgets run, and move egui's caret to the IME's selection, so the
            // next keystroke lands where the user sees the cursor.
            if state.ime_bridge {
                if let Some(ime) = platform::take_ime_text() {
                    state.ime_updates = state.ime_updates.saturating_add(1);
                    state.text = ime.text;
                    state.ime_cursor = Some((ime.selection_start, ime.selection_end));
                    ui::set_text_cursor(&self.egui_ctx, ime.selection_start, ime.selection_end);
                }
            } else {
                // Keep the channel from replaying a stale buffer if the bridge
                // is switched back on later.
                let _ = platform::take_ime_text();
            }

            (
                std::mem::take(&mut state.restart_surface),
                std::mem::take(&mut state.recreate_requested),
            )
        };

        if restart {
            self.restart_surface("button");
        }

        if let Some(gpu) = self.gpu.as_mut() {
            let egui_ctx = self.egui_ctx.clone();
            // A panic while laying out or submitting a frame must not unwind
            // through winit into `android_main`: android-activity would then
            // finish the Activity and the app would close with no visible
            // reason. Contain it here, drop the poisoned surface, and let
            // "Restart surface" (or the retry path) rebuild.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut state = lock_state();
                gpu.frame(&egui_ctx, &mut state);
                state.observe_text();
            }));
            if let Err(payload) = outcome {
                let text = diag::payload_text(payload.as_ref());
                diag::note_contained("frame (egui/wgpu)");
                log::error!("frame panicked: {text}");
                lock_state().gpu_error = Some(format!("frame panic: {text} — tap Restart surface"));
                self.gpu = None;
                self.next_gpu_retry = Some(Instant::now() + GPU_RETRY_DELAY);
            }
        }

        // Outside the state lock: the JNI call can block on the UI thread.
        if recreate {
            match platform::request_activity_recreate() {
                Ok(()) => log::info!("activity recreate requested"),
                Err(err) => log::error!("activity recreate failed: {err}"),
            }
        }
    }
}

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: egui_wgpu::Renderer,
    egui_state: egui_winit::State,
    adapter_info: String,
}

impl Gpu {
    fn new(window: Arc<Window>, egui_ctx: egui::Context) -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window.clone())
            .map_err(|err| format!("create_surface: {err}"))?;

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
            egui_ctx,
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
            window,
            surface,
            device,
            queue,
            config,
            renderer,
            egui_state,
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

    fn frame(&mut self, egui_ctx: &egui::Context, state: &mut SpikeState) {
        // Some Android paths (first frame after a resume, or a rotation that
        // does not deliver `Resized` before the redraw) leave the surface at a
        // stale size; syncing here keeps the viewport correct either way.
        let size = self.window.inner_size();
        if size.width > 0
            && size.height > 0
            && (size.width != self.config.width || size.height != self.config.height)
        {
            self.resize(size);
        }

        let raw_input = self.egui_state.take_egui_input(&self.window);
        let full_output = egui_ctx.run(raw_input, |ctx| ui::draw(ctx, state));
        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);
        let pixels_per_point = full_output.pixels_per_point;
        let paint_jobs = egui_ctx.tessellate(full_output.shapes, pixels_per_point);

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
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                log::warn!("surface outdated/lost — reconfiguring");
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(err) => {
                log::error!("get_current_texture: {err}");
                return;
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
    }
}
