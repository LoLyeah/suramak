// M14b: the GUI binary is a Windows-subsystem app so launching it never opens a
// console window (the CLI, `suramak-cli`, stays a console binary). Off Windows
// the attribute is absent — `cfg_attr` keeps one source root for both.
#![cfg_attr(windows, windows_subsystem = "windows")]

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
//! * `SURAMAK_SMOKE_QUIT_ANSWER=stop-save|discard|keep` (M9e, smoke modes only)
//!   — answers the quit confirm deterministically the moment it appears (the
//!   scripted quit-while-recording smoke); inert in normal runs.
//!
//! Quit path (WORKORDER-HOTFIX-QUIT): every quit flavor — menu Quit, Cmd-Q, the
//! window's close button, a smoke run's `event_loop.exit()` — dispatches winit's
//! `LoopExiting`, which runs [`App::quit`]: best-effort teardown through the
//! existing teardown paths, then a hard exit that cannot run ggml's static
//! Metal-device destructor (`shutdown.rs` documents why that exit must be
//! `_exit(2)` on macOS and not `std::process::exit`). Smoke exit codes are
//! unchanged (0 / 1).
//!
//! M9e (P1-1): with a live capture session, BOTH quit paths first present the
//! in-app confirm — the window's close button is ignored and turned into
//! [`app::QuitAction::ShowConfirm`] here, and Cmd-Q/menu Quit is vetoed inside
//! AppKit (`applicationShouldTerminate:` → `NSTerminateCancel`, installed by
//! `suramak-glass`); the vetoed request arrives as a pending flag the frame
//! loop drains into the same funnel. The three answers: Stop & Save finalizes
//! the wav + writes the pending-transcription entry and then exits through the
//! unchanged funnel; Discard exits exactly as before; Keep aborts the quit.

mod app;
mod cadence;
mod framestats;
mod instance;
/// M14b (D2): the Windows named-mutex guard; the unix `flock` path is `instance`.
#[cfg(windows)]
mod instance_windows;
mod motion;
mod pending_transcription;
/// M14b: "reveal in folder" (`explorer /select,`). Compiled on every target so
/// the host build type-checks the whole call path; the spawn is `cfg(windows)`
/// and the stub elsewhere reports `Unsupported` (never reached — the only caller
/// checks `cfg!(windows)`).
mod reveal;
mod runtime;
mod shell;
mod shutdown;
mod theme;
mod updates;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use app::{Effect, Job, QuitAction, QuitAnswer, QuitOutcome, QuitSource, RecordState};
use runtime::{PendingResume, Runtime};
use shell::Shell;
use suramak_core::paths;
use suramak_core::store::Store;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

/// The winit user-event payload the AccessKit platform adapter delivers through
/// its `EventLoopProxy` (the `egui-winit` wiring pattern, named after the eframe
/// convention for the same plumbing). [`App::user_event`] forwards the action
/// requests into the egui state, so a screen reader's "press" lands on the same
/// painted control a mouse would.
#[derive(Debug)]
pub struct AccessKitEnabler(egui_winit::accesskit_winit::Event);

impl From<egui_winit::accesskit_winit::Event> for AccessKitEnabler {
    fn from(event: egui_winit::accesskit_winit::Event) -> Self {
        Self(event)
    }
}

/// The winit user-event payload: [`AccessKitEnabler`]'s events (the `egui-winit`
/// wiring pattern) plus the **asynchronous repaint wake**.
///
/// The repaint wake must be async: egui's repaint callback runs while the
/// context lock is held, so a synchronous hop to the main thread from the
/// bridge thread deadlocks against a main thread already waiting for that lock
/// (the macOS CI stack sample caught the cycle: `redraw_or_log →
/// take_egui_input → lock_shared_slow` on main, `run_bridge → request_repaint →
/// dispatch_sync(main)` on the bridge thread). `EventLoopProxy::send_event`
/// never blocks; the loop turns `Repaint` into a redraw on the main thread.
#[derive(Debug)]
pub enum UserEvent {
    /// M9b: an AccessKit platform event for the egui state.
    AccessKit(AccessKitEnabler),
    /// M12: a repaint requested from another thread (worker/bridge).
    Repaint,
    /// M14b (D2): another launch of Suramak asked this instance to come to the
    /// front (the Windows activation event, `instance_windows`). Windows-only:
    /// no other platform has the event, and an unconstructed variant is a
    /// `dead_code` error under `-D warnings` (the `GuardError` precedent).
    #[cfg(windows)]
    Activate,
}

impl From<egui_winit::accesskit_winit::Event> for UserEvent {
    fn from(event: egui_winit::accesskit_winit::Event) -> Self {
        Self::AccessKit(AccessKitEnabler(event))
    }
}

/// Written on a successful plain smoke run (repo-root relative, like the acceptance block).
const SMOKE_FILE: &str = "build/smoke_ok.txt";
/// Written on a successful scripted job run.
const SMOKE_JOB_FILE: &str = "build/smoke_job_ok.txt";
/// Watchdog fallback when `SURAMAK_SMOKE_JOB` is set without `SURAMAK_SMOKE_SECS`.
const SMOKE_JOB_DEFAULT_WATCHDOG_SECS: u64 = 600;

/// Writes a smoke status file, creating `build/` when needed — the directory is
/// gitignored, so fresh clones (CI, the owner's Mac) do not have it.
fn write_smoke_file(path: &str, content: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, content)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let smoke = Smoke::from_env();
    // One Suramak at a time (skipped in smoke modes — transient runs).
    acquire_instance_lock(smoke.is_none());
    // M9b: the user-event channel carries AccessKit's action requests back into
    // the loop (the proxy is handed to the shell's adapter at startup).
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    // M14b (D2): if this process owns the mutex, listen for the activation event
    // a later launch signals (the secondary side exited before this point).
    // Smoke modes skip the guard, so they skip the watcher too — a transient
    // smoke run must not swallow a real launch's activation.
    #[cfg(windows)]
    if smoke.is_none() {
        watch_for_activation(&event_loop);
    }
    let mut app = App::new(smoke, Some(event_loop.create_proxy()));
    // Every quit flavor ends in `App::exiting` (winit dispatches `LoopExiting`
    // on the way out), which tears down and hard-exits — see `App::quit`. For
    // Cmd-Q that happens inside AppKit's `applicationWillTerminate:`, *before*
    // AppKit's own `exit()`; so this function normally never returns. If the
    // loop itself fails to run, the same shutdown still applies (exit 1).
    if let Err(err) = event_loop.run_app(&mut app) {
        eprintln!("suramak: event loop error: {err}");
        app.failed = true;
    }
    app.quit()
}

/// Starts the D2 activation watcher, first instance only (Windows).
///
/// The watcher thread waits on `Local\Suramak-Activate-<SID>`; every signal posts
/// `UserEvent::Activate`, which [`App::user_event`] turns into a raise on the
/// main thread. A failure here is not fatal: the single-instance guarantee is the
/// mutex, and only the "second launch raises the window" nicety is lost.
#[cfg(windows)]
fn watch_for_activation(event_loop: &EventLoop<UserEvent>) {
    let proxy = event_loop.create_proxy();
    if let Err(err) = instance_windows::watch_activation(move || {
        let _ = proxy.send_event(UserEvent::Activate);
    }) {
        eprintln!(
            "suramak: warning: cannot listen for the activation event ({err}) — a second launch will not raise this window"
        );
    }
}

/// Exits with code 2 when another instance holds the data-directory lock; a
/// lock that cannot even be created only warns (an unusual data dir should not
/// block the app).
#[cfg(not(windows))]
fn acquire_instance_lock(active: bool) {
    if !active {
        return;
    }
    match paths::data_dir() {
        Ok(dir) => match instance::acquire(&dir.join("suramak.lock")) {
            Ok(()) => {}
            // The flock guard (and its AlreadyRunning case) exists on unix only;
            // elsewhere `acquire` cannot fail this way — see `instance`.
            #[cfg(unix)]
            Err(instance::GuardError::AlreadyRunning) => {
                eprintln!(
                    "suramak: another Suramak instance is already running — quit it and start again."
                );
                std::process::exit(2);
            }
            Err(err) => {
                eprintln!("suramak: warning: {err} — continuing without the instance guard");
            }
        },
        Err(err) => {
            eprintln!(
                "suramak: warning: cannot resolve the data directory ({err}) — continuing without the instance guard"
            );
        }
    }
}

/// Windows (M14b, D2): a named mutex guards the single instance, and a second
/// launch **raises the running window and exits 0** — it did what the user asked
/// (the unix guard's exit 2 carries a message to a terminal; a GUI launch has no
/// console to write to, and the subsystem is `windows`).
///
/// A guard that cannot even be created only warns, exactly like the unix path: an
/// unusual logon should not block the app.
#[cfg(windows)]
fn acquire_instance_lock(active: bool) {
    if !active {
        return;
    }
    match instance_windows::claim() {
        Ok(instance_windows::Claim::Primary) => {}
        Ok(instance_windows::Claim::Secondary) => {
            // The launch's intent (Suramak in front) is satisfied either way, so
            // the code comes from the protocol's plan (0) — also when the signal
            // itself failed: this process must not linger.
            let code = match instance_windows::activate_running_instance() {
                Ok(report) => {
                    tracing::info!(
                        signalled = report.signalled,
                        foreground_granted = report.foreground_granted,
                        "another instance is running — activation signalled"
                    );
                    report.exit_code
                }
                Err(err) => {
                    eprintln!(
                        "suramak: another instance is running but could not be signalled ({err}) — exiting"
                    );
                    0
                }
            };
            std::process::exit(code);
        }
        Err(err) => {
            eprintln!("suramak: warning: {err} — continuing without the instance guard");
        }
    }
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
    /// Set once the quit teardown ran — [`App::shutdown`] is idempotent, so the
    /// `exiting` hook and the post-loop belt-and-braces can both call it.
    shutdown_done: bool,
    /// The AccessKit user-event proxy, handed to the shell when it starts
    /// (M9b: `Shell::new` creates the platform adapter before the window shows).
    accesskit_proxy: Option<EventLoopProxy<UserEvent>>,
    /// The next scheduled cadence repaint (M9b). Set while the state asks for an
    /// interval (recording / playing / download / autosave due), cleared when it
    /// does not; a wake that finds the deadline passed repaints and re-arms.
    repaint_due: Option<Instant>,
    /// M9e (P1-1): the quit confirm's answer, waiting for the frame loop to
    /// execute it (only the loop can exit the process).
    pending_quit: Option<QuitOutcome>,
    /// M9e (P1-1): the smoke-only auto-answer for the quit confirm
    /// (`SURAMAK_SMOKE_QUIT_ANSWER`), consumed once. `None` in normal runs.
    smoke_quit_answer: Option<QuitAnswer>,
    /// M12: the in-app updater (macOS + the `updates` feature; an honest stub
    /// everywhere else). `None` until [`App::resumed`] started it — the crate
    /// requires a live AppKit loop and the main thread, so `App::new` must not
    /// touch it. Kept for the app's lifetime (the crate's contract).
    updates: Option<updates::Updater>,
}

impl App {
    fn new(smoke: Option<Smoke>, accesskit_proxy: Option<EventLoopProxy<UserEvent>>) -> Self {
        // M9e smoke harness: a scripted answer for the quit confirm, only when
        // a smoke mode is active (same discipline as SURAMAK_SMOKE_RECORD).
        let smoke_quit_answer = if smoke.is_some() {
            match std::env::var("SURAMAK_SMOKE_QUIT_ANSWER").as_deref() {
                Ok("stop-save") => Some(QuitAnswer::StopAndSave),
                Ok("discard") => Some(QuitAnswer::Discard),
                Ok("keep") => Some(QuitAnswer::Keep),
                _ => None,
            }
        } else {
            None
        };
        Self {
            egui_ctx: egui::Context::default(),
            shell: None,
            runtime: None,
            smoke,
            failed: false,
            shutdown_done: false,
            accesskit_proxy,
            repaint_due: None,
            pending_quit: None,
            smoke_quit_answer,
            updates: None,
        }
    }

    /// The quit path (WORKORDER-HOTFIX-QUIT): tear down best-effort, then
    /// hard-exit. Never returns.
    ///
    /// This is the one funnel for every quit flavor. Menu Quit / Cmd-Q reach it
    /// through AppKit's `applicationWillTerminate:` — winit's delegate runs
    /// `internal_exit()`, which dispatches `LoopExiting` — **before** AppKit's
    /// own `exit()`, so the process is already gone when that (and with it
    /// ggml's static Metal-device destructor) would run; a window close and a
    /// smoke run's `event_loop.exit()` reach it through the same dispatch.
    ///
    /// The exit code is the app's usual one (1 after a failed start / job /
    /// smoke run, else 0), so the smoke modes keep their codes.
    fn quit(&mut self) -> ! {
        // M9b: the frame-stats seam — with `SURAMAK_FRAME_STATS` set, the
        // per-state frame summary prints on the way out (stdout + the log).
        if framestats::enabled() {
            if let Some(shell) = &self.shell {
                let summary = shell.frame_stats.summary();
                print!("{summary}");
                tracing::info!(target: "suramak::frames", "\n{summary}");
            }
        }
        self.shutdown();
        shutdown::hard_exit(self.exit_code())
    }

    /// 1 after a failed start / job / smoke run, else 0.
    fn exit_code(&self) -> i32 {
        if self.failed {
            1
        } else {
            0
        }
    }

    /// Best-effort teardown of everything that owns a whisper context or a
    /// device, through the existing teardown paths (WORKORDER-HOTFIX-QUIT).
    ///
    /// Idempotent, bounded (`shutdown::TEARDOWN_GRACE`), never panics: capture
    /// is stopped (streams and taps down), the live decode thread and the
    /// worker bridge are signalled and waited for at most the grace — an idle
    /// thread exits and drops its whisper context (and the Metal residency sets
    /// behind it) right away — and playback gets its `Stop`. A job in flight is
    /// never interrupted; [`App::quit`]'s hard exit covers it. The
    /// window/surface is left to process teardown (dropping it inside AppKit's
    /// terminate notification would be a risk for no gain — the process ends
    /// immediately after).
    fn shutdown(&mut self) {
        if self.shutdown_done {
            return;
        }
        self.shutdown_done = true;
        let Some(runtime) = self.runtime.as_mut() else {
            return;
        };
        let workers_stopped = runtime.shutdown(shutdown::TEARDOWN_GRACE);
        tracing::debug!(workers_stopped, "quit: teardown finished");
    }

    /// Drains worker events (queued by the bridge thread), polls the model
    /// download, the polish run, the playback engine, the capture meter and the
    /// waveform-peaks job, and runs the debounce tick.
    ///
    /// Returns whether anything **discrete** changed (M9b): queued events were
    /// drained, a background result landed, a terminal transport transition
    /// happened, a live transcript line arrived, or a tick fired effects.
    /// Continuously moving values (the playback position, the live meter) are
    /// deliberately not reported here — their repaints come from the cadence
    /// policy ([`cadence::repaint_interval`]), so a busy state cannot turn the
    /// change flag into a busy loop.
    fn pump(&mut self) -> bool {
        let mut changed = false;
        if self.runtime.is_none() {
            return false;
        }
        let events = self
            .runtime
            .as_mut()
            .map(|runtime| runtime.drain_worker())
            .unwrap_or_default();
        if !events.is_empty() {
            changed = true;
        }
        for event in events {
            let effects = match self.shell.as_mut() {
                Some(shell) => shell.app.on_worker_event(event),
                None => return changed,
            };
            self.apply(effects);
        }

        // Model-download progress (the download itself runs on its own thread).
        let download = self
            .runtime
            .as_mut()
            .and_then(|runtime| runtime.download_status());
        if let Some(status) = download {
            changed = true;
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
            changed = true;
            let effects = match self.shell.as_mut() {
                Some(shell) => shell.app.on_polish_done(result),
                None => Vec::new(),
            };
            self.apply(effects);
        }

        // Waveform peaks (M9b: decoded once per source load, off this thread).
        let peaks = self
            .runtime
            .as_mut()
            .and_then(|runtime| runtime.peaks_status());
        if let Some((transcript, result)) = peaks {
            changed = true;
            if let Some(shell) = self.shell.as_mut() {
                shell.app.on_peaks_loaded(transcript, result);
            }
        }

        // Playback position (M8): polled per frame while an engine exists — the
        // capture-RMS pattern, but for the moving playhead. The position moves
        // continuously (the cadence policy repaints for it); only the terminal
        // transitions are discrete changes.
        let playback = self
            .runtime
            .as_mut()
            .and_then(|runtime| runtime.playback_status());
        if let Some(status) = playback {
            if status.finished || status.error.is_some() {
                changed = true;
            }
            let effects = match self.shell.as_mut() {
                Some(shell) => shell.app.on_playback_status(status),
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

            // Live provisional transcript (M7): per-frame while recording —
            // feed new audio through the live scheduler and publish what the
            // decoder merged. Stop tears it down; the final pass is unaffected.
            // A published line is a discrete change (repaint on arrival).
            if let (Some(shell), Some(runtime)) = (self.shell.as_mut(), self.runtime.as_mut()) {
                changed |= runtime.live_tick(&mut shell.app);
            }
        }

        let effects = match self.shell.as_mut() {
            Some(shell) => shell.app.tick(Instant::now()),
            None => return changed,
        };
        if !effects.is_empty() {
            changed = true;
        }
        self.apply(effects);
        changed
    }

    /// Executes effects through the runtime, then re-syncs the appearance
    /// (M9a): a settings save may have changed `settings.ui.appearance`, and
    /// `sync_appearance` is a no-op unless the resolved theme or the Reduce
    /// Motion flag actually moved.
    ///
    /// M9e (P1-1): [`Effect::Quit`] is filtered out here — only the frame loop
    /// can exit the process — and parked in `pending_quit` for
    /// [`App::drain_quit`] at the end of the event handler.
    fn apply(&mut self, effects: Vec<Effect>) {
        let (Some(shell), Some(runtime)) = (self.shell.as_mut(), self.runtime.as_mut()) else {
            return;
        };
        let mut quit = None;
        let rest: Vec<Effect> = effects
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Quit(outcome) => {
                    quit = Some(outcome);
                    None
                }
                other => Some(other),
            })
            .collect();
        runtime.apply(rest, &mut shell.app);
        shell.sync_appearance(runtime.appearance());
        // M12: a settings save may have flipped "Automatic updates" — re-apply
        // the policy to the updater (a no-op when unchanged or when the
        // updater is off in this run).
        if let Some(updater) = self.updates.as_mut() {
            updater.set_automatic_checks(runtime.updates_auto_check());
        }
        if quit.is_some() {
            self.pending_quit = quit;
        }
    }

    /// M9e (P1-1): a quit was requested — the window's close button or a
    /// vetoed AppKit termination (Cmd-Q / menu Quit). With a live capture
    /// session the request becomes the in-app confirm; otherwise the quit
    /// proceeds exactly as before this slice.
    fn handle_quit_request(&mut self, event_loop: &ActiveEventLoop, source: QuitSource) {
        let action = self
            .shell
            .as_mut()
            .map(|shell| shell.app.request_quit(source));
        match action {
            Some(QuitAction::ShowConfirm) => {
                tracing::info!(?source, "quit: live recording — showing the confirm");
                if let Some(shell) = &self.shell {
                    shell.window.request_redraw();
                }
            }
            _ => {
                // No shell (startup), no runtime, or no live recording: the
                // pre-M9e behavior — exit through the one quit funnel.
                event_loop.exit();
            }
        }
    }

    /// M9e (P1-1): drains a vetoed AppKit termination (the flag the glass
    /// callback set) into the same funnel as the window close.
    fn poll_terminate_veto(&mut self, event_loop: &ActiveEventLoop) {
        let pending = self
            .shell
            .as_ref()
            .is_some_and(|shell| shell.quit_veto.take_pending());
        if pending {
            self.handle_quit_request(event_loop, QuitSource::AppTerminate);
        }
    }

    /// M9e (P1-1): executes an answered quit confirm. Stop & Save finalizes +
    /// persists first (bounded — no transcription runs here), then both
    /// exiting outcomes go through `event_loop.exit()` → `exiting()` → the
    /// unchanged HOTFIX-QUIT funnel (bounded teardown + `_exit`).
    fn drain_quit(&mut self, event_loop: &ActiveEventLoop) {
        let Some(outcome) = self.pending_quit.take() else {
            return;
        };
        match outcome {
            QuitOutcome::Stay => {
                // Keep Recording: nothing happens; the card fades out.
                if let Some(shell) = &self.shell {
                    shell.window.request_redraw();
                }
            }
            QuitOutcome::ExitNow => {
                tracing::info!("quit: discard answered — exiting as before");
                event_loop.exit();
            }
            QuitOutcome::SaveThenExit { title } => {
                self.save_recording_for_quit(&title);
                event_loop.exit();
            }
        }
    }

    /// M9e (P1-1): the Stop & Save save step — the recording's wav and the
    /// pending-transcription entry, timed (the smoke's bounded-exit number).
    /// A failure is logged and the quit continues: the user asked to quit and
    /// the app must not hold the process hostage to a disk error.
    fn save_recording_for_quit(&mut self, title: &str) {
        let (Some(shell), Some(runtime)) = (self.shell.as_mut(), self.runtime.as_mut()) else {
            return;
        };
        let started = Instant::now();
        match runtime.save_recording_for_quit(&mut shell.app) {
            Ok(Some(wav)) => tracing::info!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                wav = %wav.display(),
                title,
                "quit: stop & save finished"
            ),
            Ok(None) => tracing::info!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                title,
                "quit: stop & save finished (no capture session — entry only)"
            ),
            Err(err) => tracing::warn!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                error = %err,
                "quit: stop & save failed — exiting anyway"
            ),
        }
    }

    /// M9e (P1-1) smoke harness: answers the quit confirm automatically when
    /// `SURAMAK_SMOKE_QUIT_ANSWER` is set and a smoke mode is active (the
    /// scripted quit-while-recording smoke). Inert in normal runs.
    fn smoke_answer_quit(&mut self) {
        let Some(answer) = self.smoke_quit_answer else {
            return;
        };
        let Some(shell) = self.shell.as_mut() else {
            return;
        };
        if shell.app.quit_prompt.is_none() {
            return;
        }
        self.smoke_quit_answer = None; // one-shot
        tracing::info!(?answer, "smoke: auto-answering the quit confirm");
        if let Some(outcome) = shell.app.answer_quit(answer) {
            self.pending_quit = Some(outcome);
        }
    }

    /// Draws one frame and executes what it asked for. Effects go through
    /// [`App::apply`], so a quit answer produced by the frame (the confirm
    /// card's controls) is parked for [`App::drain_quit`] like any other.
    fn redraw(&mut self) {
        let effects = match self.shell.as_mut() {
            Some(shell) => shell.redraw_or_log(),
            None => return,
        };
        self.apply(effects);
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
            match write_smoke_file(SMOKE_FILE, &report) {
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
            "suramak ui smoke job ok\njob={}\ntranscript_id={}\nsegments={}\nmodel={}\nengine={}\nframes={frames}\nsecs={elapsed:.1}\n",
            input.display(),
            outcome.transcript.id,
            segments,
            outcome.transcript.model,
            engine.as_deref().unwrap_or("unknown"),
        );
        if let Err(err) = write_smoke_file(SMOKE_JOB_FILE, &content) {
            self.smoke_job_failed(event_loop, &format!("cannot write {SMOKE_JOB_FILE}: {err}"));
            return;
        }
        println!(
            "suramak: SMOKE-JOB-OK — {} ({} segments, model {}) saved to the library ({frames} frames)",
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

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.shell.is_some() {
            return;
        }
        match Shell::new(event_loop, &self.egui_ctx, self.accesskit_proxy.take()) {
            Ok(mut shell) => {
                let mut runtime = Runtime::new(self.egui_ctx.clone());
                if let Some(err) = runtime.startup_error.clone() {
                    shell
                        .app
                        .set_status(format!("library unavailable: {err}"), true);
                }
                // The settings screen opens on the persisted values (M3).
                shell.app.settings_form = runtime.settings_form();
                // M9a: resolve + apply the persisted appearance (System follows
                // the OS theme; forced modes override) before the first frame.
                shell.sync_appearance(runtime.appearance());
                // Polish availability is derived from the settings + key policy (M4).
                shell.app.polish_configured = runtime.polish_configured();
                let initial = shell.app.initial_effects();
                runtime.apply(initial, &mut shell.app);

                // M9e (P1-1): a quit-mid-recording Stop & Save left a pending
                // entry — auto-run its transcription now (the status line says
                // where it came from). An entry whose audio is gone is cleared
                // by the runtime and reported honestly here instead.
                match runtime.pending_transcription() {
                    Some(PendingResume::Resume(entry)) => {
                        let effects = shell.app.begin_pending_resume(&entry);
                        runtime.apply(effects, &mut shell.app);
                    }
                    Some(PendingResume::MissingAudio { title }) => {
                        tracing::warn!(
                            title,
                            "pending transcription: the entry had no audio — reported to the user"
                        );
                        shell.app.set_status(
                            "a recording saved at quit has no audio on disk — nothing to transcribe",
                            false,
                        );
                    }
                    None => {}
                }

                // M12: start the in-app updater — main thread, after the AppKit
                // window is up and the settings are loaded (the crate requires
                // a live app bundle + event loop; outside one it logs and stays
                // off). The persisted "Automatic updates" policy is applied to
                // Sparkle at start and re-applied after every settings save.
                self.updates = Some(updates::Updater::start(runtime.updates_auto_check()));

                if let Some(smoke) = &mut self.smoke {
                    let now = Instant::now();
                    smoke.deadline = Some(now + Duration::from_secs(smoke.secs));
                    smoke.started = Some(now);
                    // M9a harness: seed a demo recording so the protected
                    // Record composition can be captured on builds without
                    // capture (Linux/Xvfb). Smoke modes only — a normal run
                    // never sees this.
                    if std::env::var_os("SURAMAK_SMOKE_RECORD").is_some() {
                        shell.app.seed_demo_recording();
                    }
                    // M9c harness: seed the model-manager table with a
                    // mid-flight download (state only — no network, no real
                    // download) so the download state can be captured
                    // headlessly. Smoke modes only.
                    if std::env::var_os("SURAMAK_SMOKE_MODELS").is_some() {
                        shell.app.model_manager.rows = runtime.model_rows();
                        shell.app.model_manager.refresh_epoch = 1;
                        shell.app.model_manager.begin_download("turbo-q5-0");
                        shell.app.on_download_progress(180_000_000, 574_041_195);
                        shell.app.route = app::Route::Models;
                    }
                    // M12 harness: open the Settings route so the Updates
                    // section can be captured headlessly (smoke modes only).
                    if std::env::var_os("SURAMAK_SMOKE_SETTINGS").is_some() {
                        shell.app.route = app::Route::Settings;
                    }
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
        // egui-winit reports `repaint` for input-ish events — and also for
        // `RedrawRequested` itself. Re-requesting a redraw from the redraw
        // handler is a self-perpetuating full-window loop: the M9a build did
        // exactly that, which is why an "idle" run measured ~29 fps on
        // llvmpipe. Redraws are scheduled by the cadence policy in
        // `about_to_wait`, by egui's own repaint callback and by the smoke
        // harness; input events still repaint right here.
        if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
            shell.window.request_redraw();
        }
        match event {
            // M9e (P1-1): a close request with a live capture session is
            // ignored and turned into the in-app confirm (nothing is discarded
            // until it is answered); without one it exits as before.
            WindowEvent::CloseRequested => {
                self.handle_quit_request(event_loop, QuitSource::WindowClose)
            }
            WindowEvent::ThemeChanged(_) => {
                // M9a: the OS appearance changed — re-resolve (System follows;
                // forced modes keep their pin) and repaint.
                if let Some(shell) = &mut self.shell {
                    let setting = shell.app.appearance;
                    shell.sync_appearance(setting);
                }
            }
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
        // M9e (P1-1): a frame or a click may have answered the confirm.
        self.drain_quit(event_loop);
    }

    /// M9b: AccessKit's platform adapter posts its events through the winit
    /// user-event channel (the `EventLoopProxy<UserEvent>` `Shell::new` handed
    /// to it). `Repaint` wakes the loop for an off-thread repaint request (the
    /// main-thread-only redraw happens here — see [`UserEvent`]); AccessKit
    /// action requests — a screen reader "pressing" a node — go back into the
    /// egui state, which turns them into `Event::AccessKitActionRequest`s for
    /// the next pass; the initial-tree request just needs a frame.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(shell) = self.shell.as_mut() else {
            return;
        };
        let enabler = match event {
            UserEvent::Repaint => {
                shell.window.request_redraw();
                return;
            }
            // M14b (D2): a second launch — restore, raise and focus on the main
            // thread, then paint the frame that shows it.
            #[cfg(windows)]
            UserEvent::Activate => {
                shell.activate_window();
                return;
            }
            UserEvent::AccessKit(enabler) => enabler,
        };
        if enabler.0.window_id != shell.window.id() {
            return;
        }
        match enabler.0.window_event {
            egui_winit::accesskit_winit::WindowEvent::ActionRequested(request) => {
                shell.egui_state.on_accesskit_action_request(request);
                shell.window.request_redraw();
            }
            egui_winit::accesskit_winit::WindowEvent::InitialTreeRequested => {
                shell.window.request_redraw();
            }
            egui_winit::accesskit_winit::WindowEvent::AccessibilityDeactivated => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // M9e (P1-1): a vetoed AppKit termination (Cmd-Q during a recording)
        // arrives as a flag the glass callback set; it goes through the same
        // funnel as the window close.
        self.poll_terminate_veto(event_loop);
        // M9e smoke harness: answer the confirm when the scripted smoke asked.
        self.smoke_answer_quit();
        // M9e (P1-1): the frame loop executes quit answers (Stop & Save's
        // save + the exit itself) — before any further pumping.
        self.drain_quit(event_loop);

        let changed = self.pump();

        let smoke_active = self.smoke.as_ref().is_some_and(|smoke| !smoke.finished);
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

        // M9b cadence policy: idle waits for events; recording meters at ~10 fps;
        // playback moves at ≤30 fps; a download wakes for its progress; unsaved
        // edits wake at the autosave due time. A repaint happens when the
        // policy's deadline is due or when the pump saw a discrete change —
        // never on every wake (the M9a busy loop).
        let now = Instant::now();
        let cadence = self
            .shell
            .as_ref()
            .map(|shell| cadence::repaint_interval(&shell.app, now))
            .unwrap_or(cadence::Cadence::EventDriven);
        let mut repaint = changed;
        match cadence {
            cadence::Cadence::Interval(interval) => {
                let due = self.repaint_due.unwrap_or(now + interval);
                if now >= due {
                    repaint = true;
                    self.repaint_due = Some(now + interval);
                } else {
                    self.repaint_due = Some(due);
                }
            }
            cadence::Cadence::EventDriven => {
                self.repaint_due = None;
            }
        }
        if repaint {
            if let Some(shell) = &self.shell {
                shell.window.request_redraw();
            }
        }

        // The wake schedule: the smoke watchdogs need 50 ms granularity (they
        // check deadlines here, not in a frame); the app otherwise sleeps until
        // its cadence deadline or the next event.
        let wake = if smoke_active {
            Some(Duration::from_millis(50))
        } else {
            cadence.interval()
        };
        match wake {
            Some(interval) => {
                event_loop.set_control_flow(ControlFlow::WaitUntil(now + interval));
            }
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    /// The one quit funnel (WORKORDER-HOTFIX-QUIT): winit dispatches
    /// `LoopExiting` on every way out of the loop — AppKit's
    /// `applicationWillTerminate:` (menu Quit / Cmd-Q) reaches it *before* the
    /// AppKit-level `exit()`, and `event_loop.exit()` (window close, smoke)
    /// reaches it on the way to `run_app` returning. Tear down and hard-exit;
    /// see [`App::quit`] and `shutdown.rs`.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.quit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The quit teardown is a no-op when nothing runs (no shell, no runtime)
    /// and idempotent — it runs once per quit, but `exiting` and the
    /// post-loop belt-and-braces may both call it (WORKORDER-HOTFIX-QUIT).
    #[test]
    fn shutdown_without_a_shell_or_runtime_is_a_no_op_and_idempotent() {
        let mut app = App::new(None, None);
        app.shutdown();
        app.shutdown();
        assert!(!app.failed, "teardown must not change the exit code");
        assert_eq!(app.exit_code(), 0);
    }

    /// The quit exit code: 1 after a failed start / job / smoke run (the smoke
    /// modes' existing codes), else 0.
    #[test]
    fn the_exit_code_follows_the_failure_flag() {
        let mut app = App::new(None, None);
        assert_eq!(app.exit_code(), 0);
        app.failed = true;
        assert_eq!(app.exit_code(), 1);
    }
}
