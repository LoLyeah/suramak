//! The surface lifecycle contract for the spike's wgpu generation — pure policy,
//! host-testable.
//!
//! **The field trace this encodes (v2.1, owner's Vivo X300 / Android 16).** The
//! egui fix worked (zero texture panics), and the one remaining crash was
//! `wgpu_core.rs:3793 Surface::configure: Surface does not support the adapter's
//! queue family`, fired on resume. The chain was:
//!
//! 1. `0.18s` gpu ready, first frame presented;
//! 2. `3.29s onPause → 3.32s surfaceDestroyed → 3.40s suspended` — the handler
//!    dropped device/queue/renderer/egui context;
//! 3. **`3.40s` a retry pass built a *new* gpu with a surface created from the
//!    window that had just been destroyed** — a frame's event-loop turn during
//!    the suspend path, i.e. gpu work while suspended;
//! 4. `11.91s` real resume; the handler saw `gpu.is_some()` and kept that stale
//!    generation, so the resume *configured* a surface bound to the dead
//!    `ANativeWindow` — and Mali's WSI rejected it.
//!
//! Two invariants make that chain unrepresentable, and both live here:
//!
//! * **One epoch owns the generation.** Every `resumed` opens a new epoch;
//!   every `suspended` closes it. A `Gpu` (surface, device, queue, renderer,
//!   egui context) may only be built while the epoch is open, and can only be
//!   adopted by the epoch it was built in ([`SurfaceLifecycle::begin_build`] /
//!   [`SurfaceLifecycle::adopt`]). The 3.40s build is refused; if it somehow
//!   existed, the 11.91s resume could not adopt it.
//! * **The gate counts instead of racing.** A redraw that arrives while
//!   suspended is dropped by [`SurfaceLifecycle::gate_frame`] and counted — it
//!   never reaches a build. The retry loop asks
//!   [`SurfaceLifecycle::retry_allowed`] before it starts anything.
//!
//! The third rule is the crash path itself: an outdated/lost/timed-out surface
//! does **not** get `configure()`d. [`SurfaceLifecycle::on_surface_fault`]
//! invalidates the generation and asks for the same full rebuild a resume does
//! (fresh surface from the window current *now*, fresh egui context), capped at
//! [`MAX_AUTO_REBUILDS_PER_EVENT`] per lifecycle event so a repeatable fault
//! cannot spin.
//!
//! **Why a separate module.** `app.rs` is `#![cfg(target_os = "android")]`, so
//! nothing in it can be unit-tested on this host: the wgpu dependency is
//! Android-only and `android-activity` refuses to compile off-device. The policy
//! therefore lives here, without dependencies, and `app.rs` executes its
//! decisions verbatim — the tests below pin the same state machine the phone
//! runs, not a parallel model of it. What they cannot prove is the Mali driver's
//! queue-family probe itself; that is owner-device-only (report §6).

/// Every object that dies with one GPU generation.
///
/// The list is the drop inventory, not documentation: [`SuspendReport::dropped`]
/// returns exactly this slice when a generation was live, so the suspend log line
/// names every object and a missing one is a visible gap on the device rather
/// than an assumption.
pub const GPU_SCOPE_OBJECTS: [&str; 5] =
    ["surface", "device", "queue", "renderer", "egui::Context"];

/// How many automatic rebuilds one lifecycle event may spend. The v2.1 footer
/// showed `rebuilds 4` on a session that had two resumes: the extra two were the
/// stale-surface reconfigure loop. Two per resume is enough to survive one
/// driver hiccup and small enough that a permanent fault stops and says so.
pub const MAX_AUTO_REBUILDS_PER_EVENT: u32 = 2;

/// `Suspended` is `surfaceDestroyed .. resumed`; `Running` is `resumed ..
/// suspended`. Before the first `resumed` the surface does not exist either, so
/// a fresh [`SurfaceLifecycle`] starts suspended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Suspended,
    Running,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Suspended => "SUSPENDED",
            Self::Running => "RUNNING",
        }
    }

    pub fn is_running(self) -> bool {
        self == Self::Running
    }
}

/// The right to build one GPU generation, handed out by
/// [`SurfaceLifecycle::begin_build`]. It carries the epoch it was issued in and a
/// process-local serial for logs; [`SurfaceLifecycle::adopt`] refuses a ticket
/// from an epoch that has since closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildTicket {
    pub epoch: u64,
    pub serial: u64,
}

/// Why [`SurfaceLifecycle::begin_build`] refused. Only one reason exists, on
/// purpose: the fix is "no gpu work while suspended", so the refusal must not
/// grow into a policy matrix nobody tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildRefusal {
    pub phase: Phase,
}

/// What a `resumed` did to the previous generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResumeTicket {
    /// The epoch this resume opened; a generation from any other epoch is stale.
    pub epoch: u64,
    /// `Some` if a generation was still live when the resume arrived — a
    /// contract violation the handler reports (the v2.1 bug in one field).
    pub invalidated: Option<BuildTicket>,
}

/// What a `suspended` released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuspendReport {
    pub had_generation: bool,
    pub epoch: u64,
    /// [`GPU_SCOPE_OBJECTS`] when a generation was live, empty otherwise.
    pub dropped: &'static [&'static str],
}

impl SuspendReport {
    /// The log line's object list, e.g. `surface, device, queue, …`.
    pub fn dropped_list(&self) -> String {
        self.dropped.join(", ")
    }
}

/// The three `wgpu::SurfaceError` variants that mean "this surface object is no
/// longer usable". `OutOfMemory` and `Other` are deliberately absent: they are
/// not a lifecycle statement, and rebuilding on them would hide a real failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceFault {
    Outdated,
    Lost,
    Timeout,
}

impl SurfaceFault {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Outdated => "outdated",
            Self::Lost => "lost",
            Self::Timeout => "timeout",
        }
    }
}

/// What the handler must do after a [`SurfaceFault`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceAction {
    /// Drop the whole generation and rebuild it through the same entry a resume
    /// uses: a fresh surface from the window current *now* plus a fresh egui
    /// context. `attempt` is 1-based within this lifecycle event.
    Rebuild { attempt: u32 },
    /// The per-event budget is spent. Drop the generation, stop the automatic
    /// path (no retry timer either) and leave the reason on screen; the
    /// "Restart surface" button is the way back.
    Contained { attempts: u32 },
}

/// The spike's surface state machine. One instance per `android_main`; the
/// handler owns it and never makes a lifecycle decision without it.
#[derive(Debug)]
pub struct SurfaceLifecycle {
    phase: Phase,
    epoch: u64,
    /// The generation the handler currently holds, as far as the policy knows.
    live: Option<BuildTicket>,
    next_serial: u64,
    auto_rebuilds: u32,
    exhausted: bool,
    gated_frames: u64,
    gated_this_epoch: u64,
    refused_builds: u32,
    stale_builds: u32,
    builds: u32,
    suspends: u32,
}

impl Default for SurfaceLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceLifecycle {
    pub fn new() -> Self {
        Self {
            phase: Phase::Suspended,
            epoch: 0,
            live: None,
            // Serials are 1-based so a log line reads `generation #1` for the
            // first build rather than `#0`.
            next_serial: 1,
            auto_rebuilds: 0,
            exhausted: false,
            gated_frames: 0,
            gated_this_epoch: 0,
            refused_builds: 0,
            stale_builds: 0,
            builds: 0,
            suspends: 0,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn live(&self) -> Option<BuildTicket> {
        self.live
    }

    /// Frames dropped by the gate over the whole process.
    pub fn gated_frames(&self) -> u64 {
        self.gated_frames
    }

    /// Frames dropped by the gate since the last `resumed` (the number the
    /// gate's log line quotes).
    pub fn gated_this_epoch(&self) -> u64 {
        self.gated_this_epoch
    }

    /// Builds refused because the lifecycle was suspended.
    pub fn refused_builds(&self) -> u32 {
        self.refused_builds
    }

    /// Builds whose ticket belonged to an epoch that had already closed.
    pub fn stale_builds(&self) -> u32 {
        self.stale_builds
    }

    /// Generations actually adopted (the first build counts).
    pub fn builds(&self) -> u32 {
        self.builds
    }

    pub fn suspends(&self) -> u32 {
        self.suspends
    }

    pub fn auto_rebuilds(&self) -> u32 {
        self.auto_rebuilds
    }

    /// `true` while the generation is gone because the auto-rebuild budget ran
    /// out — the automatic retry loop must stay parked until a resume or a
    /// manual restart clears it.
    pub fn is_exhausted(&self) -> bool {
        self.exhausted
    }

    /// `resumed`: close the previous epoch, open a new one. Any generation the
    /// handler still holds is invalid from this moment — the caller drops it and
    /// rebuilds through [`Self::begin_build`], which is why this cannot be a
    /// no-op when `self.live` is `None`.
    pub fn on_resumed(&mut self) -> ResumeTicket {
        let invalidated = self.live.take();
        self.epoch = self.epoch.saturating_add(1);
        self.phase = Phase::Running;
        self.auto_rebuilds = 0;
        self.exhausted = false;
        self.gated_this_epoch = 0;
        ResumeTicket {
            epoch: self.epoch,
            invalidated,
        }
    }

    /// `suspended`: nothing may be built or drawn until the next `resumed`.
    ///
    /// Idempotent: when the Java-side signal
    /// ([`Self::on_surface_destroyed_hint`]) already closed the epoch, winit's
    /// later `Suspended` is the same transition and is not counted twice.
    pub fn on_suspended(&mut self) -> SuspendReport {
        let had_generation = self.live.take().is_some();
        if self.phase.is_running() {
            self.phase = Phase::Suspended;
            self.suspends = self.suspends.saturating_add(1);
        }
        SuspendReport {
            had_generation,
            epoch: self.epoch,
            dropped: if had_generation {
                &GPU_SCOPE_OBJECTS
            } else {
                &[]
            },
        }
    }

    /// `surfaceDestroyed`, as the Kotlin shim reports it.
    ///
    /// This is the surface half of the suspend and it arrives *before* winit's
    /// `Suspended`: android-activity only forwards `APP_CMD_TERM_WINDOW`, and
    /// the Java callback runs first (`3.32s` vs `3.40s` in the field trace —
    /// exactly where the raced init lived). Closing the epoch here means the
    /// gate is shut while the native window pointer may still look alive.
    pub fn on_surface_destroyed_hint(&mut self) -> SuspendReport {
        self.on_suspended()
    }

    /// Ask for the right to build a generation. The only refusal is
    /// [`Phase::Suspended`] — and it is counted, because a refused build is
    /// exactly the 3.40s event the field trace shows.
    pub fn begin_build(&mut self) -> Result<BuildTicket, BuildRefusal> {
        if !self.phase.is_running() {
            self.refused_builds = self.refused_builds.saturating_add(1);
            return Err(BuildRefusal { phase: self.phase });
        }
        let ticket = BuildTicket {
            epoch: self.epoch,
            serial: self.next_serial,
        };
        self.next_serial = self.next_serial.saturating_add(1);
        Ok(ticket)
    }

    /// Make a successfully built generation live. Returns `false` — and leaves
    /// the generation dead — when the ticket belongs to an epoch that has since
    /// closed (a build that straddled a suspend), which is the v2.1
    /// stale-surface adoption made impossible.
    pub fn adopt(&mut self, ticket: BuildTicket) -> bool {
        if ticket.epoch != self.epoch || !self.phase.is_running() {
            self.stale_builds = self.stale_builds.saturating_add(1);
            return false;
        }
        self.live = Some(ticket);
        self.builds = self.builds.saturating_add(1);
        true
    }

    /// The handler dropped the generation (contained frame panic, fault,
    /// suspend). Idempotent.
    pub fn release(&mut self) {
        self.live = None;
    }

    /// One redraw arrived. `Some(n)` means the gate is closed: the frame was
    /// dropped and `n` counts this epoch's dropped frames; `None` means draw.
    /// This is the only way a frame can ask about the lifecycle, so "a frame
    /// started a gpu init while suspended" cannot be written by accident.
    pub fn gate_frame(&mut self) -> Option<u64> {
        if self.phase.is_running() {
            return None;
        }
        self.gated_frames = self.gated_frames.saturating_add(1);
        self.gated_this_epoch = self.gated_this_epoch.saturating_add(1);
        Some(self.gated_this_epoch)
    }

    /// May the automatic retry loop start a build? False while suspended (the
    /// window is gone), while a generation is live, and after the budget ran
    /// out.
    pub fn retry_allowed(&self) -> bool {
        self.phase.is_running() && self.live.is_none() && !self.exhausted
    }

    /// A surface fault is a lifecycle statement, not a configure() hint: the
    /// generation that produced it is dead either way, and the answer is a full
    /// rebuild (same entry as a resume) while the per-event budget lasts.
    pub fn on_surface_fault(&mut self, fault: SurfaceFault) -> SurfaceAction {
        let _ = fault;
        self.live = None;
        if self.auto_rebuilds >= MAX_AUTO_REBUILDS_PER_EVENT {
            self.exhausted = true;
            return SurfaceAction::Contained {
                attempts: self.auto_rebuilds,
            };
        }
        self.auto_rebuilds = self.auto_rebuilds.saturating_add(1);
        SurfaceAction::Rebuild {
            attempt: self.auto_rebuilds,
        }
    }

    /// Give the automatic path a fresh budget. Called by the manual "Restart
    /// surface" button: a human asking for a rebuild is new information, and
    /// the alternative is a session that can never recover automatically again.
    pub fn reset_auto_budget(&mut self) {
        self.auto_rebuilds = 0;
        self.exhausted = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the state machine into "running with a live generation", the state
    /// every real frame runs in.
    fn running_with_generation(lifecycle: &mut SurfaceLifecycle) -> BuildTicket {
        lifecycle.on_resumed();
        let ticket = lifecycle
            .begin_build()
            .expect("running epoch must allow a build");
        assert!(lifecycle.adopt(ticket));
        ticket
    }

    /// Contract (1): the suspend path drops **all** gpu-scope objects — surface
    /// included — and leaves nothing behind for the next resume to configure.
    #[test]
    fn suspending_drops_every_gpu_scope_object_and_leaves_nothing_behind() {
        let mut lifecycle = SurfaceLifecycle::new();
        // Before the first resume there is no window: nothing may be built.
        assert_eq!(lifecycle.phase(), Phase::Suspended);
        assert!(lifecycle.begin_build().is_err());

        let ticket = running_with_generation(&mut lifecycle);
        assert_eq!(lifecycle.live(), Some(ticket));

        let report = lifecycle.on_suspended();
        assert!(report.had_generation);
        assert_eq!(report.epoch, ticket.epoch);
        // The inventory is the contract: a surface left out of the drop would
        // be the stale object the field crash configured on the next resume.
        assert_eq!(report.dropped, &GPU_SCOPE_OBJECTS);
        assert!(report.dropped.contains(&"surface"));
        assert!(report.dropped.contains(&"egui::Context"));
        assert_eq!(
            report.dropped_list(),
            "surface, device, queue, renderer, egui::Context"
        );
        assert_eq!(lifecycle.live(), None);
        assert_eq!(lifecycle.phase(), Phase::Suspended);

        // A second suspend has nothing to drop — no double-drop, no leak.
        let again = lifecycle.on_suspended();
        assert!(!again.had_generation);
        assert!(again.dropped.is_empty());
        // Two signals for one transition (winit may deliver `Suspended` after
        // the Kotlin `surfaceDestroyed` hint): counted once.
        assert_eq!(lifecycle.suspends(), 1);
    }

    /// The v2.1 field bug in one assertion: a generation built for an epoch that
    /// has closed (the 3.40s raced init) can never become live in the next one.
    #[test]
    fn a_generation_built_for_an_ended_epoch_can_never_become_live() {
        let mut lifecycle = SurfaceLifecycle::new();
        lifecycle.on_resumed();
        let stale = lifecycle.begin_build().expect("epoch 1 is open");

        // The suspend lands before the build finishes (the 3.40s case, where the
        // window was already destroyed).
        lifecycle.on_suspended();
        let resume = lifecycle.on_resumed();
        assert_eq!(resume.epoch, stale.epoch + 1);
        assert_eq!(
            resume.invalidated, None,
            "the suspend had no live generation"
        );

        assert!(
            !lifecycle.adopt(stale),
            "an old-epoch build must not be adopted"
        );
        assert_eq!(lifecycle.live(), None);
        assert_eq!(lifecycle.stale_builds(), 1);
        assert_eq!(lifecycle.builds(), 0);

        // The current epoch's build is adopted, and it is a different serial.
        let fresh = lifecycle.begin_build().expect("epoch 2 is open");
        assert_eq!(fresh.epoch, stale.epoch + 1);
        assert!(fresh.serial > stale.serial);
        assert!(lifecycle.adopt(fresh));
        assert_eq!(lifecycle.live(), Some(fresh));
    }

    /// Contract (2): a frame arriving while suspended is counted by the gate and
    /// starts no gpu work — neither the frame path nor the retry loop.
    #[test]
    fn a_frame_while_suspended_is_gated_and_cannot_start_gpu_work() {
        let mut lifecycle = SurfaceLifecycle::new();
        running_with_generation(&mut lifecycle);
        lifecycle.on_suspended();

        assert_eq!(lifecycle.gate_frame(), Some(1));
        assert_eq!(lifecycle.gate_frame(), Some(2));
        assert_eq!(lifecycle.gate_frame(), Some(3));
        assert_eq!(lifecycle.gated_this_epoch(), 3);
        assert_eq!(lifecycle.gated_frames(), 3);

        // The retry loop and any direct build request are refused, counted.
        assert!(!lifecycle.retry_allowed());
        let refusal = lifecycle.begin_build().expect_err("suspended must refuse");
        assert_eq!(refusal.phase, Phase::Suspended);
        lifecycle.begin_build().expect_err("still refused");
        assert_eq!(lifecycle.refused_builds(), 2);
        assert_eq!(lifecycle.live(), None);

        // The next resume opens the gate and resets the per-epoch counter.
        lifecycle.on_resumed();
        assert_eq!(lifecycle.gate_frame(), None);
        assert_eq!(lifecycle.gated_this_epoch(), 0);
        assert_eq!(lifecycle.gated_frames(), 3, "the process total is kept");
        assert!(lifecycle.retry_allowed());
        assert!(lifecycle.begin_build().is_ok());
    }

    /// Contract (3): outdated/lost/timeout invalidate the generation and ask for
    /// a full rebuild — a *new* build ticket, never a `configure()` of the dead
    /// one — and the per-event cap stops the loop at two.
    #[test]
    fn surface_faults_rebuild_a_fresh_generation_and_stop_after_two() {
        let mut lifecycle = SurfaceLifecycle::new();
        let first = running_with_generation(&mut lifecycle);

        for (fault, attempt) in [(SurfaceFault::Outdated, 1), (SurfaceFault::Lost, 2)] {
            let action = lifecycle.on_surface_fault(fault);
            assert_eq!(action, SurfaceAction::Rebuild { attempt });
            // The dead generation is not live any more: the only way back is a
            // new build, which is what makes a `configure()` on it impossible.
            assert_eq!(lifecycle.live(), None);
            assert!(lifecycle.retry_allowed());
            let ticket = lifecycle.begin_build().expect("running epoch");
            assert!(ticket.serial > first.serial);
            assert!(lifecycle.adopt(ticket));
        }

        // Third fault in the same lifecycle event: contained, no automatic path.
        let action = lifecycle.on_surface_fault(SurfaceFault::Timeout);
        assert_eq!(action, SurfaceAction::Contained { attempts: 2 });
        assert!(lifecycle.is_exhausted());
        assert!(
            !lifecycle.retry_allowed(),
            "the retry loop must stay parked"
        );
        assert_eq!(lifecycle.live(), None);
        // The manual button still gets through: a human is new information.
        lifecycle.reset_auto_budget();
        assert!(lifecycle.retry_allowed());
        assert_eq!(
            lifecycle.on_surface_fault(SurfaceFault::Outdated),
            SurfaceAction::Rebuild { attempt: 1 }
        );
    }

    /// The cap is per lifecycle event, not per process: the next resume starts
    /// with a full budget even if the previous one spent it.
    #[test]
    fn the_auto_rebuild_budget_resets_on_the_next_lifecycle_event() {
        let mut lifecycle = SurfaceLifecycle::new();
        running_with_generation(&mut lifecycle);
        assert_eq!(
            lifecycle.on_surface_fault(SurfaceFault::Outdated),
            SurfaceAction::Rebuild { attempt: 1 }
        );
        assert_eq!(
            lifecycle.on_surface_fault(SurfaceFault::Outdated),
            SurfaceAction::Rebuild { attempt: 2 }
        );
        assert_eq!(
            lifecycle.on_surface_fault(SurfaceFault::Outdated),
            SurfaceAction::Contained { attempts: 2 }
        );

        let resume = lifecycle.on_resumed();
        assert_eq!(resume.invalidated, None);
        assert_eq!(lifecycle.auto_rebuilds(), 0);
        assert!(!lifecycle.is_exhausted());
        let ticket = lifecycle.begin_build().expect("new epoch");
        assert!(lifecycle.adopt(ticket));
        assert_eq!(
            lifecycle.on_surface_fault(SurfaceFault::Lost),
            SurfaceAction::Rebuild { attempt: 1 }
        );
    }

    /// The gate accepts two signals for one transition: winit's `Suspended` and
    /// the Kotlin `surfaceDestroyed` line, whichever lands first.
    #[test]
    fn the_java_surface_destroyed_hint_closes_the_epoch_before_winit_reports_it() {
        let mut lifecycle = SurfaceLifecycle::new();
        running_with_generation(&mut lifecycle);

        let hint = lifecycle.on_surface_destroyed_hint();
        assert!(hint.had_generation);
        assert_eq!(lifecycle.phase(), Phase::Suspended);
        assert_eq!(
            lifecycle.gate_frame(),
            Some(1),
            "the gate is shut immediately"
        );
        assert!(lifecycle.begin_build().is_err(), "no build in the gap");
        assert!(!lifecycle.retry_allowed());

        // winit's `Suspended`, one event-loop turn later: the same transition.
        let late = lifecycle.on_suspended();
        assert!(!late.had_generation);
        assert_eq!(lifecycle.suspends(), 1, "one transition, two signals");
        assert_eq!(lifecycle.gated_this_epoch(), 1);
    }

    /// A generation that survives into a resume is reported, not silently
    /// adopted: that is the v2.1 state (a gpu still `Some` at resume) named at
    /// the one place that can see it.
    #[test]
    fn a_generation_alive_at_resume_is_invalidated_and_reported() {
        let mut lifecycle = SurfaceLifecycle::new();
        let ticket = running_with_generation(&mut lifecycle);
        // The handler failed to drop (or the suspend never reached it).
        let resume = lifecycle.on_resumed();
        assert_eq!(resume.epoch, ticket.epoch + 1);
        assert_eq!(resume.invalidated, Some(ticket));
        assert_eq!(lifecycle.live(), None);
        assert_eq!(
            lifecycle.stale_builds(),
            0,
            "invalidating is not a stale build"
        );
    }
}
