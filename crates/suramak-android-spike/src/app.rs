//! Window, wgpu surface and the frame loop — Android only.
//!
//! The shape is the one every winit 0.30 Android app has: `android_main` builds
//! an `EventLoop` bound to the `AndroidApp`, an `ApplicationHandler` creates the
//! window on `resumed`, and a `ControlFlow::Poll` loop draws every vsync.
//!
//! Lifecycle: the spike's counters live in a process-wide `OnceLock<Mutex<_>>`
//! ([`state`]), *not* in this handler, because `android_main` runs again after
//! an Activity recreation and a fresh handler is built each time. That is what
//! makes check (2) meaningful: the text you typed is still there after the
//! surface — and the whole Activity — has been torn down.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::android::EventLoopBuilderExtAndroid;
use winit::window::{Window, WindowId};

use crate::platform;
use crate::ui::{self, SpikeState};

static STATE: OnceLock<Mutex<SpikeState>> = OnceLock::new();

/// The spike's single source of truth (see the module docs).
pub fn state() -> &'static Mutex<SpikeState> {
    STATE.get_or_init(|| Mutex::new(SpikeState::default()))
}

fn lock_state() -> std::sync::MutexGuard<'static, SpikeState> {
    state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn run(app: android_activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("SuramakSpike"),
    );
    log::info!("M13b spike: android_main entered");

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
    };
    if let Err(err) = event_loop.run_app(&mut handler) {
        log::error!("event loop exited with an error: {err}");
    }
    log::info!("M13b spike: android_main returned");
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
}

impl ApplicationHandler for Handler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        {
            let mut state = lock_state();
            state.resumes = state.resumes.saturating_add(1);
        }

        if self.window.is_none() {
            match event_loop.create_window(Window::default_attributes().with_title("Suramak Spike"))
            {
                Ok(window) => self.window = Some(Arc::new(window)),
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
            match Gpu::new(window.clone(), self.egui_ctx.clone()) {
                Ok(gpu) => {
                    let adapter = gpu.adapter_info.clone();
                    log::info!("gpu ready: {adapter}");
                    let mut state = lock_state();
                    state.backend = adapter;
                    // A GPU that is built after the first resume is a rebuild:
                    // either a suspend/resume within this activity, or a whole
                    // new activity (`android_main` re-entered, same process).
                    if state.resumes > 1 {
                        state.surface_recreates = state.surface_recreates.saturating_add(1);
                    }
                    drop(state);
                    self.gpu = Some(gpu);
                }
                Err(err) => {
                    log::error!("gpu init failed: {err}");
                    lock_state().gpu_error = Some(err);
                }
            }
        }
        window.request_redraw();
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // The ANativeWindow behind the surface is going away; drop every wgpu
        // object that references it. The `Window` itself is kept and re-used:
        // winit refreshes its native handle before the next `resumed`.
        log::info!("suspended: releasing the wgpu surface");
        self.gpu = None;
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if matches!(event, WindowEvent::CloseRequested) {
            event_loop.exit();
            return;
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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.window.request_redraw();
        }
    }
}

impl Handler {
    fn redraw(&mut self) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        let recreate = {
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

            gpu.frame(&self.egui_ctx, &mut state);
            state.observe_text();
            let requested = std::mem::take(&mut state.recreate_requested);
            requested
        };

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

        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }
}
