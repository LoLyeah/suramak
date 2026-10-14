//! Window + wgpu surface + manual egui wiring for the Suramak shell.
//!
//! M8.5: the shell also owns the window-material handle and keeps the floating
//! record control's glass glued to the shared `suramak_glass::float_rect`
//! (applied on creation, on every resize and after any geometry change).

use std::sync::Arc;
use std::time::Instant;

use egui_wgpu::wgpu;
use suramak_core::settings::Appearance;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::{Window, WindowAttributes};

use crate::app::{self, AppState};
use crate::framestats::FrameStats;
use crate::theme::{self, AppTheme};
use crate::AccessKitEnabler;

const WINDOW_TITLE: &str = "Suramak";
const INITIAL_SIZE: LogicalSize<f64> = LogicalSize::new(1280.0, 800.0);
const MIN_SIZE: LogicalSize<f64> = LogicalSize::new(900.0, 600.0);

/// Everything that exists once the event loop resumes: the winit window, the
/// transparent wgpu surface, and the egui input/render pair.
pub struct Shell {
    pub window: Arc<Window>,
    pub egui_state: egui_winit::State,
    pub frames: u64,
    /// M9b: the per-state frame tally printed at exit under `SURAMAK_FRAME_STATS`
    /// (`FrameStats::summary`).
    pub frame_stats: FrameStats,
    pub adapter_info: String,
    pub backend: &'static str,
    pub config: wgpu::SurfaceConfiguration,
    /// App state (routes, job, editor, library) drawn every frame.
    pub app: AppState,
    /// The window-material handle (M8.5): drives the floating record control's
    /// glass. `None` on platforms without the macOS material (see
    /// [`apply_glass`]) — the UI then paints its fallback pill.
    glass: Option<suramak_glass::GlassHandle>,
    /// The window geometry the float rect was last applied for (physical
    /// width, height, scale-factor bits). The per-frame sync re-applies the
    /// rect whenever this changes — a scale-factor change can arrive without a
    /// `Resized` event.
    float_applied: Option<(u32, u32, u32)>,
    egui_ctx: egui::Context,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
}

impl Shell {
    /// Creates the window, picks a wgpu adapter (explicit backend chain), wires
    /// egui, and applies the macOS material.
    ///
    /// M9b: the window is created **invisible** and shown right after the
    /// AccessKit adapter is created — `accesskit_winit` requires the adapter to
    /// exist before the window is first shown (it panics otherwise), which is
    /// also what makes the a11y tree live from the first frame.
    pub fn new(
        event_loop: &ActiveEventLoop,
        egui_ctx: &egui::Context,
        accesskit_proxy: Option<EventLoopProxy<AccessKitEnabler>>,
    ) -> Result<Self, String> {
        let attributes: WindowAttributes = Window::default_attributes()
            .with_title(WINDOW_TITLE)
            .with_inner_size(INITIAL_SIZE)
            .with_min_inner_size(MIN_SIZE)
            .with_transparent(true)
            .with_visible(false);
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|err| format!("window creation failed: {err}"))?,
        );

        let init = init_wgpu(window.clone())?;
        let adapter_info = init.adapter.get_info();
        tracing::info!(
            backend = init.backend,
            adapter = %adapter_info.name,
            device_type = ?adapter_info.device_type,
            "wgpu adapter acquired"
        );

        let (device, queue) =
            pollster::block_on(init.adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("suramak-ui"),
                ..Default::default()
            }))
            .map_err(|err| format!("device request failed: {err}"))?;

        let caps = init.surface.get_capabilities(&init.adapter);
        if caps.formats.is_empty() {
            return Err("surface reported no texture formats".to_owned());
        }
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);
        let alpha_mode = pick_alpha_mode(&caps.alpha_modes);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: Vec::new(),
        };
        init.surface.configure(&device, &config);
        tracing::info!(
            ?format,
            ?alpha_mode,
            "surface configured (clear color 0,0,0,0)"
        );

        let renderer =
            egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
        let mut egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        // M9b: AccessKit — the platform adapter starts here, before the window
        // is shown (its constructor requires that). VoiceOver / AT-SPI then see
        // the egui tree from the first frame; the adapter's action requests
        // arrive through the winit user event (`App::user_event`) and the tree
        // updates ride `handle_platform_output` below. On Linux the same wiring
        // is active (AT-SPI); it degrades silently on a headless machine.
        if let Some(proxy) = accesskit_proxy {
            egui_state.init_accesskit::<AccessKitEnabler>(event_loop, window.as_ref(), proxy);
            tracing::info!("accesskit adapter initialised");
        }
        window.set_visible(true);

        theme::install_fonts(egui_ctx);
        // A sane first ladder before `sync_appearance` runs (it resolves the
        // persisted appearance against the OS theme right after startup).
        theme::install(egui_ctx, AppTheme::Dark, false);

        // Wake the winit loop for egui-side repaint requests (worker events
        // arrive on the bridge thread and call `Context::request_repaint`).
        {
            let window = window.clone();
            egui_ctx.set_request_repaint_callback(move |info| {
                // `debug`: every egui-side repaint request with its delay — the
                // frame-stats seam's companion when tracing why a frame came.
                tracing::debug!(delay = ?info.delay, "egui repaint requested");
                window.request_redraw();
            });
        }

        let glass = apply_glass(&window);

        let mut shell = Self {
            window,
            egui_state,
            frames: 0,
            frame_stats: FrameStats::new(),
            adapter_info: format!(
                "{} ({:?}, driver {})",
                adapter_info.name, adapter_info.device_type, adapter_info.driver
            ),
            backend: init.backend,
            config,
            app: AppState::new(),
            glass,
            float_applied: None,
            egui_ctx: egui_ctx.clone(),
            surface: init.surface,
            device,
            queue,
            renderer,
        };
        // M8.5: tell the painter whether a real material stands behind the
        // floating record control, then place that material at the shared
        // float rect (creates it on macOS 26+; no-op elsewhere).
        shell.app.float_glass = shell
            .glass
            .as_ref()
            .is_some_and(|glass| glass.float_glass_available());
        shell.sync_float_rect();
        Ok(shell)
    }

    /// Reconfigures the surface after a resize; the floating record control's
    /// glass follows (M8.5) — bottom-centered, re-applied on every resize.
    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        // Applied immediately; the next frame's sync re-applies it with settled
        // AppKit bounds in case the views had not caught up with the resize yet.
        self.apply_float_rect();
    }

    /// M9a: resolve the effective appearance and apply it everywhere it
    /// matters — the egui token visuals, the macOS window + glass material
    /// (`suramak-glass::set_appearance`), and the Reduce Motion policy.
    ///
    /// `System` follows the OS theme live (winit `Window::theme()`; Linux
    /// System → Light is pinned in [`theme::effective_theme`]); forced modes
    /// override. Cheap and idempotent — called at startup, on
    /// `WindowEvent::ThemeChanged` and after every effect batch (a settings
    /// save lands through the same path); it only touches anything when the
    /// resolved theme or the Reduce Motion flag actually changed.
    pub fn sync_appearance(&mut self, setting: Appearance) {
        let system = match self.window.theme() {
            Some(winit::window::Theme::Light) => Some(theme::SystemTheme::Light),
            Some(winit::window::Theme::Dark) => Some(theme::SystemTheme::Dark),
            None => None,
        };
        let effective = theme::effective_theme(setting, system);
        // Read on the main thread; the settings screen's open/apply moments are
        // exactly when this runs (startup, settings save, system theme change).
        let reduce = suramak_glass::reduce_motion();
        self.app.appearance = setting;
        if effective == self.app.theme && reduce == self.app.reduce_motion {
            return;
        }
        self.app.theme = effective;
        self.app.reduce_motion = reduce;
        theme::install(&self.egui_ctx, effective, reduce);
        if let Some(glass) = self.glass.as_mut() {
            glass.set_appearance(match setting {
                Appearance::System => suramak_glass::ThemeMode::System,
                Appearance::Light => suramak_glass::ThemeMode::Light,
                Appearance::Dark => suramak_glass::ThemeMode::Dark,
            });
        }
        tracing::info!(
            setting = ?setting,
            effective = ?effective,
            reduce_motion = reduce,
            "appearance applied"
        );
        self.window.request_redraw();
    }

    /// Re-applies the floating control's rect whenever the window's logical
    /// geometry changed (M8.5). Runs on every frame — the work is skipped while
    /// nothing changed, so this also catches scale-factor moves that arrive
    /// without a `Resized` event.
    fn sync_float_rect(&mut self) {
        let size = self.window.inner_size();
        let ppp = self.window.scale_factor() as f32;
        let key = (size.width, size.height, ppp.to_bits());
        if self.float_applied == Some(key) {
            return;
        }
        self.apply_float_rect();
        self.float_applied = Some(key);
    }

    /// Hands the shared float geometry to the glass handle (no-op without a
    /// material). `suramak_glass::float_rect` is the one definition both the
    /// egui painter and the macOS glass use, so the material sits behind
    /// exactly the painted pixels.
    fn apply_float_rect(&mut self) {
        let Some(glass) = self.glass.as_mut() else {
            return;
        };
        let size = self.window.inner_size();
        let ppp = self.window.scale_factor() as f32;
        let ppp = if ppp > 0.0 { ppp } else { 1.0 };
        glass.set_float_rect(Some(suramak_glass::float_rect(
            (size.width as f32 / ppp, size.height as f32 / ppp),
            ppp,
        )));
    }

    /// Renders one frame; recoverable surface errors reconfigure and skip.
    /// Returns the effects the frame requested (empty when the frame was skipped).
    pub fn redraw_or_log(&mut self) -> Vec<app::Effect> {
        // Keep the floating control's material glued to the painted rect
        // (M8.5): cheap no-op unless the window geometry changed.
        self.sync_float_rect();
        match self.render() {
            Ok(effects) => effects,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                Vec::new()
            }
            Err(err) => {
                tracing::warn!("frame skipped: {err}");
                Vec::new()
            }
        }
    }

    /// Contents of `build/smoke_ok.txt` on a successful smoke run.
    pub fn smoke_report(&self, frames: u64, secs: u64) -> String {
        format!(
            "suramak ui smoke ok\nframes={frames}\nsecs={secs}\n\
             adapter={}\nbackend={}\nalpha_mode={:?}\nsize={}x{}\npixels_per_point={}\n",
            self.adapter_info,
            self.backend,
            self.config.alpha_mode,
            self.config.width,
            self.config.height,
            egui_winit::pixels_per_point(&self.egui_ctx, &self.window),
        )
    }

    fn render(&mut self) -> Result<Vec<app::Effect>, wgpu::SurfaceError> {
        let mut raw_input = self.egui_state.take_egui_input(&self.window);
        // M9b keyboard map: dispatched on the raw input BEFORE egui sees it — a
        // matched shortcut's key event is consumed here, so a focused widget,
        // egui's own focus movement and the keymap can never both act on one
        // press. The guard (a focused text edit) is read from the same focus
        // state the pass would see.
        let key_effects =
            app::keymap::dispatch(&self.egui_ctx, &mut self.app, &mut raw_input.events);
        let egui_ctx = self.egui_ctx.clone();
        let app = &mut self.app;
        // egui may run the UI closure more than once per frame (passes); the
        // last pass's effects win (assignment, not accumulation). The keyboard
        // effects were produced before the passes and are prepended after.
        let mut frame_effects = Vec::new();
        let full_output = egui_ctx.run(raw_input, |ctx| {
            frame_effects = app::views::draw(ctx, app);
        });
        let mut effects = key_effects;
        effects.extend(frame_effects);

        // Viewport commands first (they borrow `full_output`), then the moved
        // platform output.
        handle_viewport_commands(&self.window, &full_output);
        // macOS 27 + winit 0.30.13: the IME enable→disable round-trip crashes
        // inside `WinitView::set_ime_allowed` (SIGSEGV releasing the marked-text
        // cell, object literal `0x3`, crash 48A5EFA3 on the owner's Mac,
        // 2026-10-13 — see docs/PLAN-NOTES.md). Until winit is fixed, the app
        // never asks for IME: with `ime = None` every frame and the debounce
        // state left at its `false` default, egui-winit issues no
        // `set_ime_allowed` calls at all. en/id typing needs no IME (text
        // arrives as regular key events). Re-enable when upstream handles it.
        #[cfg(target_os = "macos")]
        let platform_output = {
            let mut output = full_output.platform_output;
            output.ime = None;
            output
        };
        #[cfg(not(target_os = "macos"))]
        let platform_output = full_output.platform_output;
        self.egui_state
            .handle_platform_output(&self.window, platform_output);
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
                label: Some("suramak-egui"),
            });
        let user_commands = self.renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        self.window.pre_present_notify();
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("suramak-egui-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            // Transparent clear every frame — never an opaque root.
                            // On macOS the AppKit material behind the surface shows
                            // through; on Linux the shell paints its own dark base.
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
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
        frame.present();

        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        self.frames += 1;
        // M9b: attribute this presented frame to the visible state it was drawn
        // in (the `SURAMAK_FRAME_STATS` summary prints at exit).
        self.frame_stats.on_frame(&self.app, Instant::now());
        Ok(effects)
    }
}

/// `window.drag_window()` is how a winit-owned custom titlebar drags the window
/// (PLAN §1); the view asks for it via `ViewportCommand::StartDrag`.
fn handle_viewport_commands(window: &Window, output: &egui::FullOutput) {
    for viewport in output.viewport_output.values() {
        for command in &viewport.commands {
            if matches!(command, egui::ViewportCommand::StartDrag) {
                if let Err(err) = window.drag_window() {
                    tracing::warn!("drag_window failed: {err}");
                }
            }
        }
    }
}

struct WgpuInit {
    surface: wgpu::Surface<'static>,
    adapter: wgpu::Adapter,
    backend: &'static str,
}

/// Explicit backend chain (Vulkan → GL on Linux, Metal on macOS, DX12 → Vulkan
/// on Windows), overridable with `SURAMAK_WGPU_BACKENDS=vulkan,gl`. Each entry
/// is tried with a normal and a fallback (software) adapter request, and every
/// failure reason is printed — no silent "no adapter".
fn init_wgpu(window: Arc<Window>) -> Result<WgpuInit, String> {
    let mut attempts: Vec<String> = Vec::new();
    for (backends, backend_name) in backend_chain() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let surface = match instance.create_surface(window.clone()) {
            Ok(surface) => surface,
            Err(err) => {
                attempts.push(format!("{backend_name}: create_surface failed: {err}"));
                continue;
            }
        };
        for force_fallback in [false, true] {
            let options = wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::None,
                force_fallback_adapter: force_fallback,
                compatible_surface: Some(&surface),
            };
            match pollster::block_on(instance.request_adapter(&options)) {
                Ok(adapter) => {
                    return Ok(WgpuInit {
                        surface,
                        adapter,
                        backend: backend_name,
                    });
                }
                Err(err) => attempts.push(format!(
                    "{backend_name}{}: request_adapter failed: {err}",
                    if force_fallback { " (fallback)" } else { "" }
                )),
            }
        }
    }
    Err(format!(
        "no wgpu adapter on any backend:\n  {}",
        attempts.join("\n  ")
    ))
}

fn backend_chain() -> Vec<(wgpu::Backends, &'static str)> {
    let default: &[(wgpu::Backends, &'static str)] = if cfg!(target_os = "macos") {
        &[(wgpu::Backends::METAL, "metal")]
    } else if cfg!(target_os = "windows") {
        &[
            (wgpu::Backends::DX12, "dx12"),
            (wgpu::Backends::VULKAN, "vulkan"),
        ]
    } else {
        &[
            (wgpu::Backends::VULKAN, "vulkan"),
            (wgpu::Backends::GL, "gl"),
        ]
    };

    if let Ok(spec) = std::env::var("SURAMAK_WGPU_BACKENDS") {
        let mut chain: Vec<(wgpu::Backends, &'static str)> = Vec::new();
        for part in spec.split(',') {
            match part.trim().to_ascii_lowercase().as_str() {
                "vulkan" => chain.push((wgpu::Backends::VULKAN, "vulkan")),
                "gl" | "gles" => chain.push((wgpu::Backends::GL, "gl")),
                "metal" => chain.push((wgpu::Backends::METAL, "metal")),
                "dx12" => chain.push((wgpu::Backends::DX12, "dx12")),
                "primary" => chain.push((wgpu::Backends::PRIMARY, "primary")),
                "all" => chain.push((wgpu::Backends::all(), "all")),
                "" => {}
                other => {
                    eprintln!("suramak: unknown SURAMAK_WGPU_BACKENDS entry {other:?} — ignoring")
                }
            }
        }
        if !chain.is_empty() {
            return chain;
        }
    }
    default.to_vec()
}

/// PLAN §1 step 7: `PostMultiplied` when the surface offers it, else
/// `PreMultiplied`, else whatever is available (Linux/Xvfb surfaces usually
/// offer only `Opaque`, which is fine — Linux is layout-truth only).
fn pick_alpha_mode(modes: &[wgpu::CompositeAlphaMode]) -> wgpu::CompositeAlphaMode {
    use wgpu::CompositeAlphaMode as Mode;
    [
        Mode::PostMultiplied,
        Mode::PreMultiplied,
        Mode::Inherit,
        Mode::Auto,
        Mode::Opaque,
    ]
    .into_iter()
    .find(|candidate| modes.contains(candidate))
    .unwrap_or(Mode::Opaque)
}

/// Applies the window material and hands back the handle that drives the
/// floating record control's glass (M8.5).
#[cfg(target_os = "macos")]
fn apply_glass(window: &Window) -> Option<suramak_glass::GlassHandle> {
    match suramak_glass::apply(window, suramak_glass::GlassConfig::default()) {
        Ok(handle) => {
            tracing::info!("AppKit window material applied");
            Some(handle)
        }
        Err(err) => {
            tracing::warn!("window material not applied: {err}");
            None
        }
    }
}

/// Non-macOS: no window material, so no float-glass handle — the UI paints its
/// fallback pill. (Linux is a deliberate no-op per PLAN §1; the Windows DWM
/// path stays compile-checked-only per M4, so `apply` is not called there.)
#[cfg(not(target_os = "macos"))]
fn apply_glass(_window: &Window) -> Option<suramak_glass::GlassHandle> {
    None
}
