//! Window + wgpu surface + manual egui wiring for the Suramak shell.

use std::sync::Arc;

use egui_wgpu::wgpu;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes};

use crate::theme;
use crate::view::{self, ViewState};

const WINDOW_TITLE: &str = "Suramak";
const INITIAL_SIZE: LogicalSize<f64> = LogicalSize::new(1280.0, 800.0);
const MIN_SIZE: LogicalSize<f64> = LogicalSize::new(900.0, 600.0);

/// Everything that exists once the event loop resumes: the winit window, the
/// transparent wgpu surface, and the egui input/render pair.
pub struct Shell {
    pub window: Arc<Window>,
    pub egui_state: egui_winit::State,
    pub frames: u64,
    pub adapter_info: String,
    pub backend: &'static str,
    pub config: wgpu::SurfaceConfiguration,
    egui_ctx: egui::Context,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    view_state: ViewState,
}

impl Shell {
    /// Creates the window, picks a wgpu adapter (explicit backend chain), wires
    /// egui, and applies the macOS material.
    pub fn new(event_loop: &ActiveEventLoop, egui_ctx: &egui::Context) -> Result<Self, String> {
        let attributes: WindowAttributes = Window::default_attributes()
            .with_title(WINDOW_TITLE)
            .with_inner_size(INITIAL_SIZE)
            .with_min_inner_size(MIN_SIZE)
            .with_transparent(true);
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
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        theme::install_fonts(egui_ctx);
        egui_ctx.set_theme(egui::ThemePreference::Dark);

        #[cfg(target_os = "macos")]
        apply_glass(&window);

        Ok(Self {
            window,
            egui_state,
            frames: 0,
            adapter_info: format!(
                "{} ({:?}, driver {})",
                adapter_info.name, adapter_info.device_type, adapter_info.driver
            ),
            backend: init.backend,
            config,
            egui_ctx: egui_ctx.clone(),
            surface: init.surface,
            device,
            queue,
            renderer,
            view_state: ViewState::new(),
        })
    }

    /// Reconfigures the surface after a resize.
    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Renders one frame; recoverable surface errors reconfigure and skip.
    pub fn redraw_or_log(&mut self) {
        match self.render() {
            Ok(()) => {}
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
            }
            Err(err) => tracing::warn!("frame skipped: {err}"),
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

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let egui_ctx = self.egui_ctx.clone();
        let view_state = &mut self.view_state;
        let full_output = egui_ctx.run(raw_input, |ctx| view::draw(ctx, view_state));

        // Viewport commands first (they borrow `full_output`), then the moved
        // platform output.
        handle_viewport_commands(&self.window, &full_output);
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
        Ok(())
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

#[cfg(target_os = "macos")]
fn apply_glass(window: &Window) {
    match suramak_glass::apply(window, suramak_glass::GlassConfig::default()) {
        Ok(()) => tracing::info!("AppKit window material applied"),
        Err(err) => tracing::warn!("window material not applied: {err}"),
    }
}
