//! Suramak app entry point (M1 shell + M2 wiring).
//!
//! The window/event loop is winit 0.30 `ApplicationHandler` with **manual**
//! `egui-winit`/`egui-wgpu` wiring (eframe is rejected by PLAN §1 — we must own
//! the winit window so `suramak-glass` can reach the `NSWindow`). On macOS
//! `suramak-glass::apply` installs the real AppKit material behind the render
//! surface; on Linux the shell paints an opaque dark window.
//!
//! Smoke modes:
//!
//! * `SURAMAK_SMOKE_SECS=N` (M1, unchanged) — run N seconds of live app, write
//!   `build/smoke_ok.txt` only when at least one frame was presented, exit 0.
//! * `SURAMAK_SMOKE_JOB=<audio path>` (M2) — on startup the app runs the full
//!   in-app pipeline (import → transcribe with the resolved default model → save
//!   to the library DB), asserts the DB row exists through a fresh connection,
//!   prints `SMOKE-JOB-OK`, writes `build/smoke_job_ok.txt` and exits 0.
//!   `SURAMAK_SMOKE_SECS` is the watchdog: exit 1 (no file) when the job has not
//!   completed by then.

mod app;
mod runtime;
mod shell;
mod theme;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use app::{Effect, Job, RecordState};
use runtime::Runtime;
use shell::Shell;
use suramak_core::paths;
use suramak_core::store::Store;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

/// Written on a successful plain smoke run (repo-root relative, like the acceptance block).
const SMOKE_FILE: &str = "build/smoke_ok.txt";
/// Written on a successful scripted job run.
const SMOKE_JOB_FILE: &str = "build/smoke_job_ok.txt";
/// Watchdog fallback when `SURAMAK_SMOKE_JOB` is set without `SURAMAK_SMOKE_SECS`.
const SMOKE_JOB_DEFAULT_WATCHDOG_SECS: u64 = 600;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let smoke = Smoke::from_env();
    let event_loop = EventLoop::new()?;
    let mut app = App::new(smoke);
    event_loop.run_app(&mut app)?;
    if app.failed {
        std::process::exit(1);
    }
    Ok(())
}

fn init_tracing() {
    // Our crate at info, everything else quiet unless RUST_LOG says otherwise.
    // NOTE: the binary target is `suramak` (the package is suramak-ui), so the
    // default filter must be `suramak=info` — `suramak_ui=info` matches no
    // target and silently disabled every log line (found + fixed in M4).
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("suramak=info"));
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

/// Smoke driver state. In plain mode `secs` is the run time; in job mode it is
/// the watchdog deadline for the whole pipeline.
struct Smoke {
    secs: u64,
    /// `Some(path)` → scripted job mode.
    job: Option<PathBuf>,
    /// Set once the window is up, so the clock measures live-app time.
    deadline: Option<Instant>,
    started: Option<Instant>,
    finished: bool,
}

impl Smoke {
    fn from_env() -> Option<Smoke> {
        let secs = smoke_secs_from_env();
        let job = std::env::var_os("SURAMAK_SMOKE_JOB").map(PathBuf::from);
        match (secs, job) {
            (None, None) => None,
            (None, Some(job)) => {
                eprintln!(
                    "suramak: SURAMAK_SMOKE_JOB without SURAMAK_SMOKE_SECS — using a {SMOKE_JOB_DEFAULT_WATCHDOG_SECS} s watchdog"
                );
                Some(Smoke {
                    secs: SMOKE_JOB_DEFAULT_WATCHDOG_SECS,
                    job: Some(job),
                    deadline: None,
                    started: None,
                    finished: false,
                })
            }
            (Some(secs), job) => Some(Smoke {
                secs,
                job,
                deadline: None,
                started: None,
                finished: false,
            }),
        }
    }
}

struct App {
    egui_ctx: egui::Context,
    shell: Option<Shell>,
    runtime: Option<Runtime>,
    smoke: Option<Smoke>,
    failed: bool,
}

impl App {
    fn new(smoke: Option<Smoke>) -> Self {
        Self {
            egui_ctx: egui::Context::default(),
            shell: None,
            runtime: None,
            smoke,
            failed: false,
        }
    }

    /// Drains worker events (queued by the bridge thread), polls the model
    /// download and the capture meter, and runs the debounce tick.
    fn pump(&mut self) {
        if self.runtime.is_none() {
            return;
        }
        let events = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.drain_worker())
            .unwrap_or_default();
        for event in events {
            let effects = match self.shell.as_mut() {
                Some(shell) => shell.app.on_worker_event(event),
                None => return,
            };
            self.apply(effects);
        }

        // Model-download progress (the download itself runs on its own thread).
        let download = self
            .runtime
            .as_mut()
            .and_then(|runtime| runtime.download_status());
        if let Some(status) = download {
            let effects = match (status.finished, self.shell.as_mut()) {
                (None, Some(shell)) => {
                    shell
                        .app
                        .on_download_progress(status.bytes_done, status.bytes_total);
                    Vec::new()
                }
                (Some(result), Some(shell)) => shell.app.on_download_done(&status.id, result),
                _ => Vec::new(),
            };
            self.apply(effects);
        }

        // Polish result (the HTTP call runs on its own thread; M4).
        let polish = self
            .runtime
            .as_mut()
            .and_then(|runtime| runtime.polish_status());
        if let Some(result) = polish {
            let effects = match self.shell.as_mut() {
                Some(shell) => shell.app.on_polish_done(result),
                None => Vec::new(),
            };
            self.apply(effects);
        }

        // Capture meter sample (only while a session is live).
        let recording = self
            .shell
            .as_ref()
            .is_some_and(|shell| matches!(shell.app.record, RecordState::Recording(_)));
        if recording {
            if let Some(rms) = self
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.capture_rms())
            {
                if let Some(shell) = self.shell.as_mut() {
                    shell.app.record_sample(rms);
                }
            }
        }

        let effects = match self.shell.as_mut() {
            Some(shell) => shell.app.tick(Instant::now()),
            None => return,
        };
        self.apply(effects);
    }

    /// Executes effects through the runtime.
    fn apply(&mut self, effects: Vec<Effect>) {
        let (Some(shell), Some(runtime)) = (self.shell.as_mut(), self.runtime.as_mut()) else {
            return;
        };
        runtime.apply(effects, &mut shell.app);
    }

    /// Draws one frame and executes what it asked for.
    fn redraw(&mut self) {
        let (Some(shell), Some(runtime)) = (self.shell.as_mut(), self.runtime.as_mut()) else {
            return;
        };
        let effects = shell.redraw_or_log();
        runtime.apply(effects, &mut shell.app);
    }

    /// Plain smoke (M1 semantics): at the deadline, require ≥1 presented frame,
    /// write the file, exit. A no-frame run exits 1 *without* the file.
    fn finish_plain_smoke(&mut self, event_loop: &ActiveEventLoop) {
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

    /// Scripted job smoke: check progress every wake-up.
    fn poll_job_smoke(&mut self, event_loop: &ActiveEventLoop, expired: bool) {
        let failure = self.shell.as_ref().and_then(|shell| match &shell.app.job {
            Job::Failed { message, .. } => Some(format!("job failed: {message}")),
            _ => None,
        });
        if let Some(reason) = failure {
            self.smoke_job_failed(event_loop, &reason);
            return;
        }
        let insert = self
            .shell
            .as_ref()
            .and_then(|shell| shell.app.last_insert.clone());
        if let Some(outcome) = insert {
            match &outcome.error {
                Some(err) => {
                    self.smoke_job_failed(event_loop, &format!("library save failed: {err}"))
                }
                None => self.smoke_job_finish(event_loop, &outcome),
            }
            return;
        }
        if expired {
            self.smoke_job_failed(event_loop, "watchdog expired before the job completed");
        }
    }

    fn smoke_job_finish(&mut self, event_loop: &ActiveEventLoop, outcome: &app::SaveOutcome) {
        let (frames, engine) = match &self.shell {
            Some(shell) => (
                shell.frames,
                shell
                    .app
                    .last_engine
                    .map(|engine| engine.as_str().to_string()),
            ),
            None => (0, None),
        };
        if frames == 0 {
            self.smoke_job_failed(event_loop, "no frames were presented");
            return;
        }
        // Assert the DB row through a fresh connection (real read-back, not the
        // in-memory transcript the effect carried).
        let segments = match assert_stored(outcome) {
            Ok(segments) => segments,
            Err(err) => {
                self.smoke_job_failed(event_loop, &err);
                return;
            }
        };
        let elapsed = self
            .smoke
            .as_ref()
            .and_then(|smoke| smoke.started)
            .map(|started| started.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        let input = self
            .smoke
            .as_ref()
            .and_then(|smoke| smoke.job.clone())
            .unwrap_or_default();
        let content = format!(
            "suramak ui smoke job ok\njob={}\ntranscript_id={}\nsegments={}\nmodel={}\nengine={}\nsecs={elapsed:.1}\n",
            input.display(),
            outcome.transcript.id,
            segments,
            outcome.transcript.model,
            engine.as_deref().unwrap_or("unknown"),
        );
        if let Err(err) = std::fs::write(SMOKE_JOB_FILE, &content) {
            self.smoke_job_failed(event_loop, &format!("cannot write {SMOKE_JOB_FILE}: {err}"));
            return;
        }
        println!(
            "suramak: SMOKE-JOB-OK — {} ({} segments, model {}) saved to the library",
            outcome.transcript.title, segments, outcome.transcript.model
        );
        if let Some(smoke) = &mut self.smoke {
            smoke.finished = true;
        }
        event_loop.exit();
    }

    fn smoke_job_failed(&mut self, event_loop: &ActiveEventLoop, reason: &str) {
        if let Some(smoke) = &mut self.smoke {
            if smoke.finished {
                return;
            }
            smoke.finished = true;
        }
        eprintln!("suramak: smoke job FAILED — {reason}");
        self.failed = true;
        event_loop.exit();
    }
}

/// Reads the just-saved row back from the on-disk database.
fn assert_stored(outcome: &app::SaveOutcome) -> Result<usize, String> {
    let db = paths::db_path().map_err(|err| err.to_string())?;
    let store = Store::open(&db).map_err(|err| err.to_string())?;
    let stored = store
        .get_transcript(outcome.transcript.id)
        .map_err(|err| format!("stored row not readable: {err}"))?;
    if stored.segments.is_empty() {
        return Err("the stored transcript has no segments".to_string());
    }
    Ok(stored.segments.len())
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.shell.is_some() {
            return;
        }
        match Shell::new(event_loop, &self.egui_ctx) {
            Ok(mut shell) => {
                let mut runtime = Runtime::new(self.egui_ctx.clone());
                if let Some(err) = runtime.startup_error.clone() {
                    shell
                        .app
                        .set_status(format!("library unavailable: {err}"), true);
                }
                // The settings screen opens on the persisted values (M3).
                shell.app.settings_form = runtime.settings_form();
                // Polish availability is derived from the settings + key policy (M4).
                shell.app.polish_configured = runtime.polish_configured();
                let initial = shell.app.initial_effects();
                runtime.apply(initial, &mut shell.app);

                if let Some(smoke) = &mut self.smoke {
                    let now = Instant::now();
                    smoke.deadline = Some(now + Duration::from_secs(smoke.secs));
                    smoke.started = Some(now);
                    if let Some(input) = smoke.job.clone() {
                        let effects = shell.app.begin_import(input);
                        runtime.apply(effects, &mut shell.app);
                        // A bad job path must fail fast, not idle until the watchdog.
                        if !matches!(shell.app.job, Job::Running(_)) {
                            let reason = shell
                                .app
                                .home
                                .error
                                .clone()
                                .unwrap_or_else(|| "the job did not start".to_string());
                            eprintln!("suramak: smoke job FAILED — {reason}");
                            self.failed = true;
                            event_loop.exit();
                            return;
                        }
                    }
                }

                shell.window.request_redraw();
                self.shell = Some(shell);
                self.runtime = Some(runtime);
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
                if let Some(shell) = &mut self.shell {
                    shell.resize(size);
                }
                self.redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            // Drag-and-drop import works on every platform (PLAN §6 M2).
            WindowEvent::DroppedFile(path) => {
                let effects = match self.shell.as_mut() {
                    Some(shell) => shell.app.begin_import(path),
                    None => Vec::new(),
                };
                self.apply(effects);
            }
            WindowEvent::HoveredFile(_) => {
                if let Some(shell) = &mut self.shell {
                    shell.app.set_drop_hover(true);
                }
            }
            WindowEvent::HoveredFileCancelled => {
                if let Some(shell) = &mut self.shell {
                    shell.app.set_drop_hover(false);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.pump();

        let snapshot = self
            .smoke
            .as_ref()
            .map(|smoke| (smoke.finished, smoke.job.is_some(), smoke.deadline));
        if let Some((finished, is_job, deadline)) = snapshot {
            if !finished {
                let expired = deadline.is_some_and(|deadline| Instant::now() >= deadline);
                if is_job {
                    self.poll_job_smoke(event_loop, expired);
                } else if expired {
                    self.finish_plain_smoke(event_loop);
                }
            }
        }

        // Keep the loop awake while a job runs or edits are pending; otherwise
        // sleep until an event (the bridge thread wakes us for worker events).
        let busy = self.smoke.as_ref().is_some_and(|smoke| !smoke.finished)
            || self.shell.as_ref().is_some_and(|shell| shell.app.busy());
        if busy {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(50),
            ));
            if let Some(shell) = &self.shell {
                shell.window.request_redraw();
            }
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}
