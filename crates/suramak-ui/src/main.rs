//! Suramak shell (M1): `winit` 0.30 `ApplicationHandler` + **manual** `egui-winit`
//! and `egui-wgpu` wiring. eframe is rejected by PLAN §1 — we must own the winit
//! window so `suramak-glass` can reach the `NSWindow`.
//!
//! The window is created transparent with a `{0,0,0,0}` clear color; on macOS
//! `suramak-glass::apply` installs the real AppKit material behind the render
//! surface. On Linux the shell paints an opaque dark window (glass is a
//! macOS-only no-op there) — Linux exists for layout truth and this smoke.
//!
//! `SURAMAK_SMOKE_SECS=N` runs the app for N seconds, writes `build/smoke_ok.txt`
//! (only when at least one frame was actually presented) and exits 0.

mod shell;
mod theme;
mod view;

use std::time::{Duration, Instant};

use shell::Shell;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

/// Written on a successful smoke run (repo-root relative, like the acceptance block).
const SMOKE_FILE: &str = "build/smoke_ok.txt";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let smoke_secs = smoke_secs_from_env();
    let event_loop = EventLoop::new()?;
    let mut app = App::new(smoke_secs);
    event_loop.run_app(&mut app)?;
    if app.failed {
        std::process::exit(1);
    }
    Ok(())
}

fn init_tracing() {
    // Our crate at info, everything else quiet unless RUST_LOG says otherwise.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("suramak_ui=info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// `SURAMAK_SMOKE_SECS=N` → run for N seconds, then write the smoke file and exit 0.
fn smoke_secs_from_env() -> Option<u64> {
    let raw = std::env::var("SURAMAK_SMOKE_SECS").ok()?;
    match raw.trim().parse::<u64>() {
        Ok(secs) if secs > 0 => Some(secs),
        _ => {
            eprintln!("suramak: SURAMAK_SMOKE_SECS={raw:?} is not a positive integer — ignoring");
            None
        }
    }
}

struct Smoke {
    secs: u64,
    /// Set once the window is up, so N seconds means N seconds of live app.
    deadline: Option<Instant>,
    finished: bool,
}

struct App {
    egui_ctx: egui::Context,
    shell: Option<Shell>,
    smoke: Option<Smoke>,
    failed: bool,
}

impl App {
    fn new(smoke_secs: Option<u64>) -> Self {
        Self {
            egui_ctx: egui::Context::default(),
            shell: None,
            smoke: smoke_secs.map(|secs| Smoke {
                secs,
                deadline: None,
                finished: false,
            }),
            failed: false,
        }
    }

    /// Smoke mode: at the deadline, require ≥1 presented frame, write the file, exit.
    fn finish_smoke(&mut self, event_loop: &ActiveEventLoop) {
        let Some(smoke) = &mut self.smoke else { return };
        if smoke.finished {
            return;
        }
        smoke.finished = true;

        let frames = self.shell.as_ref().map_or(0, |shell| shell.frames);
        if frames == 0 {
            eprintln!("suramak: smoke FAILED — no frames were presented");
            self.failed = true;
        } else if let Some(shell) = &self.shell {
            let report = shell.smoke_report(frames, smoke.secs);
            match std::fs::write(SMOKE_FILE, report) {
                Ok(()) => println!("suramak: smoke ok — {frames} frames, wrote {SMOKE_FILE}"),
                Err(err) => {
                    eprintln!("suramak: smoke FAILED — cannot write {SMOKE_FILE}: {err}");
                    self.failed = true;
                }
            }
        }
        event_loop.exit();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.shell.is_some() {
            return;
        }
        match Shell::new(event_loop, &self.egui_ctx) {
            Ok(shell) => {
                if let Some(smoke) = &mut self.smoke {
                    smoke.deadline = Some(Instant::now() + Duration::from_secs(smoke.secs));
                }
                shell.window.request_redraw();
                self.shell = Some(shell);
            }
            Err(err) => {
                eprintln!("suramak: cannot start: {err}");
                tracing::error!("cannot start: {err}");
                self.failed = true;
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(shell) = self.shell.as_mut() else {
            return;
        };
        let response = shell.egui_state.on_window_event(&shell.window, &event);
        if response.repaint {
            shell.window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                shell.resize(size);
                shell.redraw_or_log();
            }
            WindowEvent::RedrawRequested => shell.redraw_or_log(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(smoke) = &self.smoke else { return };
        let Some(deadline) = smoke.deadline else {
            return;
        };
        let now = Instant::now();
        if now < deadline {
            event_loop.set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(50)));
            if let Some(shell) = &self.shell {
                shell.window.request_redraw();
            }
            return;
        }
        self.finish_smoke(event_loop);
    }
}
