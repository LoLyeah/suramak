//! The Compact HUD window (M16B): a borderless, always-on-top, **non-activating
//! on show** card rendered by the existing egui/wgpu stack.
//!
//! The window is created hidden and shown with AppKit's `orderFrontRegardless`
//! (see [`super::panel`]), so pressing the global chord while another app is
//! focused puts the card on screen **without taking focus from that app**. The
//! user can click a button (and then the app activates, necessarily, for
//! keyboard focus) or press the chord again on a result to focus the card.
//!
//! It borrows the main window's wgpu device/queue (one GPU device for the whole
//! app) and owns its own surface, egui context and renderer. A separate egui
//! context keeps the HUD's ids and viewport state out of the main window's —
//! the two windows run independent passes.

use std::sync::Arc;

use egui_wgpu::wgpu;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::{Window, WindowAttributes, WindowLevel};

use crate::hud::panel;
use crate::hud::state::{HudChrome, HudIntent, HudState};
use crate::hud::view;
use crate::hud::{HUD_TOP_MARGIN, HUD_WIDTH};
use crate::motion::Motion;
use crate::shell::{self, GpuHandles};
use crate::theme::{self, AppTheme};
use crate::UserEvent;

/// The HUD's window title (VoiceOver announces the window by it).
pub const HUD_WINDOW_TITLE: &str = "Suramak Dictation";

pub struct HudWindow {
    pub window: Arc<Window>,
    pub egui_state: egui_winit::State,
    pub ctx: egui::Context,
    config: wgpu::SurfaceConfiguration,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    shown: bool,
    theme: AppTheme,
    reduce_motion: bool,
}

impl HudWindow {
    /// Creates the hidden card window and its surface. The caller shows it via
    /// [`Self::present`] when the state becomes visible.
    pub fn new(
        event_loop: &ActiveEventLoop,
        gpu: GpuHandles,
        proxy: Option<EventLoopProxy<UserEvent>>,
        theme: AppTheme,
        reduce_motion: bool,
    ) -> Result<Self, String> {
        let attributes = hud_attributes();
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|err| format!("HUD window creation failed: {err}"))?,
        );

        let surface = gpu
            .instance
            .create_surface(window.clone())
            .map_err(|err| format!("HUD surface creation failed: {err}"))?;
        let caps = surface.get_capabilities(&gpu.adapter);
        if caps.formats.is_empty() {
            return Err("HUD surface reported no texture formats".to_string());
        }
        let format = GpuHandles::pick_format(&caps);
        let alpha_mode = shell::pick_alpha_mode(&caps.alpha_modes);
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
        surface.configure(&gpu.device, &config);
        let renderer =
            egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions::default());

        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        theme::install(&ctx, theme, reduce_motion);
        let mut egui_state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        if let Some(proxy) = proxy {
            egui_state.init_accesskit::<UserEvent>(event_loop, window.as_ref(), proxy);
            tracing::info!("HUD accesskit adapter initialised");
        }
        // The HUD's own repaint requests wake the winit loop through the same
        // asynchronous channel as the main window's (M12: never a sync hop).
        let request_redraw = {
            let window = window.clone();
            move || window.request_redraw()
        };
        // The HUD's egui context lives on the main thread and its repaint
        // callback fires inside `Context::run`, so a direct redraw request is
        // the same thread — no cross-thread hop, no M12 lock cycle.
        ctx.set_request_repaint_callback(move |_| request_redraw());

        panel::configure_hud_window(&window)?;
        place_top_center(&window, event_loop);

        Ok(Self {
            window,
            egui_state,
            ctx,
            config,
            surface,
            device: gpu.device,
            queue: gpu.queue,
            renderer,
            shown: false,
            theme,
            reduce_motion,
        })
    }

    pub fn window_id(&self) -> winit::window::WindowId {
        self.window.id()
    }

    /// Shows or hides the card. Showing never activates Suramak (macOS
    /// `orderFrontRegardless`); hiding keeps the window alive for the next run.
    /// Keeps the window exactly as tall as the current card wants to be (the
    /// HUD is compact per state, not one fixed frame with dead space).
    pub fn fit(&mut self, state: &HudState) {
        let scale = self.window.scale_factor().max(0.1);
        let wanted = view::height_for(state);
        let current = self.window.inner_size().height as f64 / scale;
        if (current - wanted as f64).abs() > 1.0 {
            let _ = self
                .window
                .request_inner_size(LogicalSize::new(HUD_WIDTH as f64, wanted as f64));
        }
    }

    pub fn present(&mut self, visible: bool) {
        if visible == self.shown {
            return;
        }
        if visible {
            if let Err(err) = panel::present_inactive(&self.window) {
                tracing::warn!("HUD present failed: {err}");
                // Fall back to the ordinary show so the indicator is never
                // invisible while the mic is open.
                self.window.set_visible(true);
            }
            self.shown = true;
            tracing::debug!(
                id = ?self.window.id(),
                visible = ?self.window.is_visible(),
                size = ?self.window.inner_size(),
                "compact HUD presented"
            );
        } else {
            panel::hide(&self.window);
            self.shown = false;
        }
    }

    pub fn focus(&self) {
        panel::focus(&self.window);
    }

    /// Re-resolves the token ladder + Reduce Motion. A no-op when unchanged.
    pub fn sync_appearance(&mut self, theme: AppTheme, reduce_motion: bool) {
        if theme == self.theme && reduce_motion == self.reduce_motion {
            return;
        }
        self.theme = theme;
        self.reduce_motion = reduce_motion;
        theme::install(&self.ctx, theme, reduce_motion);
        self.window.request_redraw();
    }

    pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Paints one frame and returns the intents the user produced (buttons and
    /// the HUD's keyboard shortcuts). Recoverable surface errors reconfigure
    /// and skip.
    pub fn render(&mut self, state: &HudState, chrome: &HudChrome) -> Vec<HudIntent> {
        match self.render_inner(state, chrome) {
            Ok(intents) => intents,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                Vec::new()
            }
            Err(err) => {
                tracing::warn!("HUD frame skipped: {err}");
                Vec::new()
            }
        }
    }

    fn render_inner(
        &mut self,
        state: &HudState,
        chrome: &HudChrome,
    ) -> Result<Vec<HudIntent>, wgpu::SurfaceError> {
        let mut raw_input = self.egui_state.take_egui_input(&self.window);
        let mut intents = take_keyboard_intents(&mut raw_input.events, state);
        let motion = Motion::new(self.reduce_motion);
        let ctx = self.ctx.clone();
        // An opaque surface cannot show a drop shadow: it would paint black.
        let chrome = chrome
            .clone()
            .with_card_shadow(self.config.alpha_mode != wgpu::CompositeAlphaMode::Opaque);
        let mut frame_intents = Vec::new();
        let full_output = ctx.run(raw_input, |ctx| {
            frame_intents = view::draw(ctx, state, &chrome, &motion);
        });
        intents.extend(frame_intents);

        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);
        let pixels_per_point = full_output.pixels_per_point;
        let paint_jobs = ctx.tessellate(full_output.shapes, pixels_per_point);
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
                label: Some("suramak-hud-egui"),
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
        // An opaque surface would render every transparent pixel black (the
        // Linux/Xvfb layout runs). Clear to the card fill there so the window
        // has no black margin; real macOS transparency keeps the soft shadow.
        let clear = if self.config.alpha_mode == wgpu::CompositeAlphaMode::Opaque {
            let fill = theme::tokens(self.theme).sheet;
            wgpu::Color {
                r: fill.r() as f64 / 255.0,
                g: fill.g() as f64 / 255.0,
                b: fill.b() as f64 / 255.0,
                a: 1.0,
            }
        } else {
            wgpu::Color::TRANSPARENT
        };
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("suramak-hud-egui-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(clear),
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
        Ok(intents)
    }
}

/// The window attributes shared by every platform. macOS gets the panel-ish
/// extras in [`hud_attributes`]'s cfg block.
fn hud_attributes() -> WindowAttributes {
    let attributes = Window::default_attributes()
        .with_title(HUD_WINDOW_TITLE)
        .with_inner_size(LogicalSize::new(
            view::card_size().x as f64,
            view::card_size().y as f64,
        ))
        .with_resizable(false)
        .with_decorations(false)
        .with_transparent(true)
        .with_visible(false)
        .with_active(false)
        .with_window_level(WindowLevel::AlwaysOnTop);
    #[cfg(target_os = "macos")]
    let attributes = {
        use winit::platform::macos::WindowAttributesExtMacOS;
        attributes
            // The first click lands on the button even though the app is not
            // active — required for the Copy/Insert controls to work on the
            // first try while the user stays in their app.
            .with_accepts_first_mouse(true)
            // A floating card's shadow would be drawn over other apps; keep the
            // silhouette to the painted rounded rect.
            .with_has_shadow(false)
            // Drag the card by its own surface (no titlebar exists).
            .with_movable_by_window_background(true)
    };
    attributes
}

/// Top-centre of the primary screen, just under the menu bar. The HUD is
/// deliberately predictable: it appears in the same place every time.
fn place_top_center(window: &Window, event_loop: &ActiveEventLoop) {
    let Some(monitor) = window
        .current_monitor()
        .or_else(|| event_loop.primary_monitor())
    else {
        return;
    };
    let scale = monitor.scale_factor();
    let screen = monitor.size();
    let position = monitor.position();
    let width = (HUD_WIDTH as f64 * scale).round() as i32;
    let margin = (HUD_TOP_MARGIN * scale).round() as i32;
    let x = position.x + ((screen.width as i32 - width) / 2).max(0);
    let y = position.y + margin;
    window.set_outer_position(PhysicalPosition::new(x, y));
}

/// HUD-local keyboard shortcuts, consumed before egui sees them:
/// `Esc` cancels/dismisses, `⌘C` copies a result, `⌘Return` inserts a result.
fn take_keyboard_intents(events: &mut Vec<egui::Event>, state: &HudState) -> Vec<HudIntent> {
    use egui::{Event, Key};
    let mut intents = Vec::new();
    events.retain(|event| {
        let Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = event
        else {
            return true;
        };
        let in_result = matches!(state, HudState::Result(_));
        match key {
            Key::Escape => {
                intents.push(if in_result {
                    HudIntent::Dismiss
                } else {
                    HudIntent::Cancel
                });
                false
            }
            Key::C if modifiers.command && in_result => {
                intents.push(HudIntent::Copy);
                false
            }
            Key::Enter if modifiers.command && in_result => {
                intents.push(HudIntent::Insert);
                false
            }
            _ => true,
        }
    });
    intents
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;

    fn key(key: egui::Key, command: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                command,
                ..Modifiers::default()
            },
        }
    }

    #[test]
    fn escape_cancels_outside_a_result_and_dismisses_one() {
        let mut events = vec![key(egui::Key::Escape, false)];
        let intents = take_keyboard_intents(&mut events, &HudState::idle());
        assert_eq!(intents, [HudIntent::Cancel]);
        assert!(events.is_empty(), "the HUD consumed the key");

        let mut state = HudState::idle();
        state.on_transcript(
            "text".to_string(),
            None,
            &HudChrome::default(),
            std::time::Instant::now(),
        );
        let mut events = vec![key(egui::Key::Escape, false)];
        assert_eq!(
            take_keyboard_intents(&mut events, &state),
            [HudIntent::Dismiss]
        );
    }

    #[test]
    fn command_c_and_return_are_result_shortcuts_only() {
        let mut state = HudState::idle();
        state.on_transcript(
            "text".to_string(),
            None,
            &HudChrome::default(),
            std::time::Instant::now(),
        );
        let mut events = vec![key(egui::Key::C, true), key(egui::Key::Enter, true)];
        assert_eq!(
            take_keyboard_intents(&mut events, &state),
            [HudIntent::Copy, HudIntent::Insert]
        );
        let mut events = vec![key(egui::Key::C, true)];
        assert!(take_keyboard_intents(&mut events, &HudState::idle()).is_empty());
        assert_eq!(events.len(), 1, "outside a result the key passes through");
    }

    #[test]
    fn plain_typing_passes_through_untouched() {
        let mut events = vec![key(egui::Key::A, false)];
        assert!(take_keyboard_intents(&mut events, &HudState::idle()).is_empty());
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn the_window_attributes_are_borderless_floating_and_hidden_at_first() {
        let attributes = hud_attributes();
        assert!(!attributes.resizable);
        assert!(!attributes.decorations);
        assert!(attributes.transparent);
        assert!(!attributes.visible);
        assert!(!attributes.active);
        assert_eq!(attributes.window_level, WindowLevel::AlwaysOnTop);
        assert_eq!(
            attributes.inner_size,
            Some(winit::dpi::Size::Logical(LogicalSize::new(460.0, 286.0)))
        );
    }
}
