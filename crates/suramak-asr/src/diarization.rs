//! M16I — the diarization runner: model pins, availability, job lane and the
//! sherpa-onnx adapter.
//!
//! ## The job contract (M16.1's discipline, one slot)
//!
//! Diarization is a **job**, not a synchronous button handler. Exactly one job
//! runs at a time ([`DiarizationLane`]); a second submit is refused while the
//! slot is held, and a job is never started for a transcript that is already
//! queued. The lane is pure (no threads, no clocks), so the slot rule and the
//! cancellation contract are unit-tested without a model.
//!
//! ## Cancellation: discard, not abort
//!
//! The M16H finding is binding: **the sherpa-onnx binding has no mid-run
//! abort** — the progress callback's return value is ignored by the C++
//! pipeline. The product contract is therefore:
//!
//! * a **queued** job cancels immediately and never starts;
//! * a **running** job's token is set, the state shows `cancelling…`, and the
//!   worker keeps the slot until sherpa returns;
//! * at the **next await point** (after decode, after the model load, after
//!   `compute` returns) the worker checks the token again and **discards** the
//!   result — no turns are written, no partial state reaches the store.
//!
//! The UI must not promise an immediate stop. [`CANCEL_CONTRACT`] is the
//! sentence the surface shows.
//!
//! ## Platform availability
//!
//! The model runs on the sherpa-onnx shared libraries the M16F packaging gate
//! bundles into the macOS app. A platform whose build does not ship those
//! libraries reports [`Availability::Unavailable`] with the reason — never a
//! silently absent control. [`availability`] is pure so every platform can be
//! pinned by a test on this Linux box.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use crate::cancel::CancelToken;
use suramak_core::diarization::{self, Speaker, SpeakerTurn, COLOR_SLOTS};
use suramak_core::Uuid;

/// The runtime the pinned models run on (provenance; the M16F dylib bundle is
/// sherpa-onnx 1.12.9 through sherpa-rs 0.6.8).
pub const ENGINE_VERSION: &str = "sherpa-onnx 1.12.9";

/// The one-slot cancellation sentence the surface shows (M16H §6.5).
pub const CANCEL_CONTRACT: &str =
    "Stopping discards this run's result; the model cannot be interrupted mid-file, \
     so the labels disappear only when it finishes.";

/// The hard duration cap: diarization is refused above this length (M16H's
/// "cap input duration" option (a); the 1 h projection is ~10.5 min of one core
/// on the reference box, which is not a job that can look hung in a UI).
pub const MAX_AUDIO_MS: i64 = 30 * 60 * 1000;

/// The pinned segmentation model (M16H §3) — file name, bytes, sha256.
pub const SEGMENTATION_FILE: &str = "sherpa-onnx-pyannote-segmentation-3-0/model.onnx";
pub const SEGMENTATION_BYTES: u64 = 5_992_913;
pub const SEGMENTATION_SHA256: &str =
    "220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079";

/// The pinned embedding model (M16H §3, CAM++; TitaNet was measured and
/// rejected at 34 % DER).
pub const EMBEDDING_FILE: &str = "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx";
pub const EMBEDDING_BYTES: u64 = 28_281_164;
pub const EMBEDDING_SHA256: &str =
    "aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2";

/// The two model files, resolved under one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiarizationModels {
    pub segmentation: PathBuf,
    pub embedding: PathBuf,
}

impl DiarizationModels {
    /// The layout the Model Manager installs into: `<dir>/segmentation/model.onnx`
    /// and `<dir>/embedding/<embedding>.onnx`. A caller that already has the two
    /// paths can build the struct directly.
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            segmentation: dir.join(SEGMENTATION_FILE),
            embedding: dir.join(EMBEDDING_FILE),
        }
    }
}

/// What a model-file check found. A refusal always names the file and the
/// number that failed (M16H §6.5: the binding's own error has no path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCheck {
    Ready,
    Missing {
        path: PathBuf,
    },
    WrongSize {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    WrongSha {
        path: PathBuf,
    },
}

impl ModelCheck {
    pub fn is_ready(&self) -> bool {
        matches!(self, ModelCheck::Ready)
    }

    /// The user-facing reason (a missing/corrupt install is actionable).
    pub fn reason(&self) -> Option<String> {
        match self {
            ModelCheck::Ready => None,
            ModelCheck::Missing { path } => Some(format!(
                "the diarization model {} is not installed",
                path.display()
            )),
            ModelCheck::WrongSize {
                path,
                expected,
                actual,
            } => Some(format!(
                "the diarization model {} is {actual} bytes, expected {expected} — \
                 re-download it",
                path.display()
            )),
            ModelCheck::WrongSha { path } => Some(format!(
                "the diarization model {} failed its sha256 check — re-download it",
                path.display()
            )),
        }
    }
}

/// Checks one file's size and sha256 (the pin, not a guess).
pub fn check_model_file(path: &Path, expected_bytes: u64, expected_sha: &str) -> ModelCheck {
    let Ok(meta) = std::fs::metadata(path) else {
        return ModelCheck::Missing {
            path: path.to_path_buf(),
        };
    };
    if meta.len() != expected_bytes {
        return ModelCheck::WrongSize {
            path: path.to_path_buf(),
            expected: expected_bytes,
            actual: meta.len(),
        };
    }
    if suramak_core::download::verify_sha256(path, expected_sha).is_err() {
        return ModelCheck::WrongSha {
            path: path.to_path_buf(),
        };
    }
    ModelCheck::Ready
}

/// Size-only check: the cheap pre-flight the availability probe runs on the UI
/// thread (hashing 34 MB per probe would be a visible hitch). A wrong-content
/// file of the right size is caught by [`check_models`] on the worker, where
/// the cost is hidden behind the model load.
pub fn check_models_size(models: &DiarizationModels) -> ModelCheck {
    for (path, expected) in [
        (&models.segmentation, SEGMENTATION_BYTES),
        (&models.embedding, EMBEDDING_BYTES),
    ] {
        match std::fs::metadata(path) {
            Err(_) => return ModelCheck::Missing { path: path.clone() },
            Ok(meta) if meta.len() != expected => {
                return ModelCheck::WrongSize {
                    path: path.clone(),
                    expected,
                    actual: meta.len(),
                }
            }
            Ok(_) => {}
        }
    }
    ModelCheck::Ready
}

/// Both model files, segmentation first (the order the error should name).
pub fn check_models(models: &DiarizationModels) -> ModelCheck {
    let segmentation = check_model_file(
        &models.segmentation,
        SEGMENTATION_BYTES,
        SEGMENTATION_SHA256,
    );
    if !segmentation.is_ready() {
        return segmentation;
    }
    check_model_file(&models.embedding, EMBEDDING_BYTES, EMBEDDING_SHA256)
}

/// A platform, decoupled from `cfg!` so the availability matrix is testable
/// everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Linux,
    Android,
}

/// Whether this platform's shipped build bundles the sherpa-onnx dylibs.
///
/// M16F bundles them on macOS only; Windows/Linux/Android follow the per-
/// platform packaging when each ships. This is the *product* fact, not whether
/// a developer build happens to link a prebuilt copy.
pub const fn ships_sherpa_dylibs(platform: Platform) -> bool {
    matches!(platform, Platform::MacOs)
}

/// The running binary's platform.
pub const fn current_platform() -> Platform {
    #[cfg(target_os = "macos")]
    {
        Platform::MacOs
    }
    #[cfg(target_os = "windows")]
    {
        Platform::Windows
    }
    #[cfg(target_os = "android")]
    {
        Platform::Android
    }
    #[cfg(all(
        not(target_os = "macos"),
        not(target_os = "windows"),
        not(target_os = "android")
    ))]
    {
        Platform::Linux
    }
}

/// Whether this build linked sherpa-rs at all. Diarization rides the same
/// `parakeet` feature that links sherpa-rs and whose M16F dylibs the bundle
/// ships — no second runtime, no second feature, no new dependency.
pub const fn engine_linked() -> bool {
    cfg!(feature = "parakeet")
}

/// Why the feature is or is not usable on this build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Ready,
    Unavailable { reason: String },
}

impl Availability {
    pub fn is_ready(&self) -> bool {
        matches!(self, Availability::Ready)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Availability::Ready => None,
            Availability::Unavailable { reason } => Some(reason),
        }
    }
}

/// The pure availability rule, in the order a user can act on it: the build's
/// linkage, the platform's packaging, then the model install.
pub fn availability(platform: Platform, linked: bool, models: &ModelCheck) -> Availability {
    if !linked {
        return Availability::Unavailable {
            reason: "this build was compiled without the sherpa-onnx engine".to_string(),
        };
    }
    if !ships_sherpa_dylibs(platform) {
        return Availability::Unavailable {
            reason: format!(
                "the speech libraries are bundled in the macOS app only today; \
                 {platform:?} follows when its packaging ships them"
            ),
        };
    }
    if let Some(reason) = models.reason() {
        return Availability::Unavailable { reason };
    }
    Availability::Ready
}

/// The availability of this binary for `models` (size-only: the full sha256
/// check runs on the worker at load, where it cannot hitch the frame).
pub fn current_availability(models: &DiarizationModels) -> Availability {
    availability(
        current_platform(),
        engine_linked(),
        &check_models_size(models),
    )
}

/// The clustering parameters. The binding's `DiarizeConfig::default()` forces
/// four clusters and must never be used (M16H §6.3); [`DiarizeParams::auto`] is
/// the documented safe default (threshold 0.6, the S0 candidate).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiarizeParams {
    /// `0` = threshold/auto; `2..=6` = an explicit count.
    pub num_clusters: i32,
    pub threshold: f32,
    pub min_duration_on: f32,
    pub min_duration_off: f32,
}

impl Default for DiarizeParams {
    fn default() -> Self {
        Self::auto()
    }
}

impl DiarizeParams {
    pub fn auto() -> Self {
        Self {
            num_clusters: 0,
            threshold: 0.6,
            min_duration_on: 0.0,
            min_duration_off: 0.0,
        }
    }

    pub fn with_count(count: u32) -> Result<Self, DiarizationError> {
        if !(2..=diarization::MAX_SPEAKERS).contains(&count) {
            return Err(DiarizationError::Params(format!(
                "speaker count {count} is outside 2..={}",
                diarization::MAX_SPEAKERS
            )));
        }
        Ok(Self {
            num_clusters: count as i32,
            ..Self::auto()
        })
    }

    pub fn validate(&self) -> Result<(), DiarizationError> {
        if self.num_clusters != 0
            && !(2..=diarization::MAX_SPEAKERS as i32).contains(&self.num_clusters)
        {
            return Err(DiarizationError::Params(format!(
                "speaker count {} is outside 2..={} (0 = auto)",
                self.num_clusters,
                diarization::MAX_SPEAKERS
            )));
        }
        if !(0.0..=1.0).contains(&self.threshold) {
            return Err(DiarizationError::Params(format!(
                "threshold {} is outside 0..=1",
                self.threshold
            )));
        }
        Ok(())
    }
}

/// The runner's errors. Each names what the user can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiarizationError {
    Unavailable(String),
    Params(String),
    Decode(String),
    TooLong { audio_ms: i64, cap_ms: i64 },
    Model(String),
    Engine(String),
    Cancelled,
}

impl std::fmt::Display for DiarizationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiarizationError::Unavailable(reason) => write!(formatter, "{reason}"),
            DiarizationError::Params(reason) => write!(formatter, "{reason}"),
            DiarizationError::Decode(reason) => {
                write!(formatter, "cannot decode the audio: {reason}")
            }
            DiarizationError::TooLong { audio_ms, cap_ms } => write!(
                formatter,
                "this recording is {:.1} min; diarization is capped at {:.0} min",
                *audio_ms as f64 / 60_000.0,
                *cap_ms as f64 / 60_000.0
            ),
            DiarizationError::Model(reason) => write!(formatter, "{reason}"),
            DiarizationError::Engine(reason) => write!(formatter, "diarization failed: {reason}"),
            DiarizationError::Cancelled => write!(formatter, "cancelled"),
        }
    }
}

impl std::error::Error for DiarizationError {}

/// One raw segment from the engine, in milliseconds (the crate's unit).
#[derive(Debug, Clone, PartialEq)]
pub struct RawTurn {
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker: i32,
}

/// How the worker builds an engine for a job (a model load, or a fake in
/// tests). Boxed because the trait object owns its sessions.
pub type EngineFactory = Box<
    dyn Fn(
            &DiarizationModels,
            &DiarizeParams,
        ) -> Result<Box<dyn DiarizationEngine>, DiarizationError>
        + Send,
>;

/// The engine seam. The production implementation is [`SherpaDiarizer`]; tests
/// drive a fake that returns on command.
pub trait DiarizationEngine: Send {
    fn run(
        &mut self,
        samples: &[f32],
        params: &DiarizeParams,
        progress: &mut (dyn FnMut(u8) + Send),
    ) -> Result<Vec<RawTurn>, DiarizationError>;
}

/// Converts raw engine segments into the store's rows: sorted by start, empty
/// spans dropped, times clamped into `audio_ms`, raw labels renumbered densely
/// **in order of first appearance** (0..n), and a default label + colour slot
/// per speaker. Deterministic for a deterministic segment list.
pub fn turns_from_raw(
    raw: &[RawTurn],
    audio_ms: i64,
) -> Result<(Vec<Speaker>, Vec<SpeakerTurn>), DiarizationError> {
    let audio_ms = audio_ms.max(0);
    let mut sorted: Vec<&RawTurn> = raw
        .iter()
        .filter(|turn| turn.end_ms > turn.start_ms)
        .collect();
    sorted.sort_by_key(|turn| (turn.start_ms, turn.end_ms, turn.speaker));

    let mut mapping: std::collections::BTreeMap<i32, u32> = std::collections::BTreeMap::new();
    let mut order: Vec<i32> = Vec::new();
    for turn in &sorted {
        if let std::collections::btree_map::Entry::Vacant(entry) = mapping.entry(turn.speaker) {
            if order.len() as u32 >= diarization::MAX_SPEAKERS {
                return Err(DiarizationError::Engine(format!(
                    "the model returned more than {} speakers",
                    diarization::MAX_SPEAKERS
                )));
            }
            let next = order.len() as u32;
            entry.insert(next);
            order.push(turn.speaker);
        }
    }
    let speakers: Vec<Speaker> = order
        .iter()
        .enumerate()
        .map(|(index, _)| {
            Speaker::new(
                index as u32,
                diarization::default_label(index as u32),
                (index as u8) % COLOR_SLOTS,
            )
        })
        .collect();

    let mut turns = Vec::with_capacity(sorted.len());
    for turn in sorted {
        let start = turn.start_ms.max(0).min(audio_ms);
        let end = turn.end_ms.max(0).min(audio_ms);
        if end <= start {
            continue;
        }
        // A clamped-away row must not leave a hole in the dense turn ids.
        let turn_idx = turns.len() as u32;
        turns.push(SpeakerTurn::new(
            turn_idx,
            start,
            end,
            mapping[&turn.speaker],
        ));
    }
    if speakers.is_empty() || turns.is_empty() {
        // Silence is not a failure: no turns, no speakers, the plain view.
        return Ok((Vec::new(), Vec::new()));
    }
    Ok((speakers, turns))
}

/// Which terminal answer the lane has for the caller.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum LaneStatus {
    #[default]
    Idle,
    Queued {
        transcript_id: Uuid,
    },
    Running {
        transcript_id: Uuid,
        pct: u8,
        /// The user asked to stop; the worker still holds the slot until
        /// sherpa returns and the result is discarded.
        cancelling: bool,
    },
    Done {
        transcript_id: Uuid,
    },
    Failed {
        transcript_id: Uuid,
        reason: String,
    },
    Cancelled {
        transcript_id: Uuid,
        /// True when a result existed and was thrown away (the honest
        /// "cancelled after compute" case); false when it never started.
        discarded: bool,
    },
}

impl LaneStatus {
    pub fn transcript_id(&self) -> Option<Uuid> {
        match self {
            LaneStatus::Idle => None,
            LaneStatus::Queued { transcript_id }
            | LaneStatus::Running { transcript_id, .. }
            | LaneStatus::Done { transcript_id }
            | LaneStatus::Failed { transcript_id, .. }
            | LaneStatus::Cancelled { transcript_id, .. } => Some(*transcript_id),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            LaneStatus::Done { .. } | LaneStatus::Failed { .. } | LaneStatus::Cancelled { .. }
        )
    }
}

/// One accepted job.
#[derive(Debug, Clone)]
pub struct DiarizationJob {
    pub transcript_id: Uuid,
    pub audio: PathBuf,
    pub params: DiarizeParams,
    pub models: DiarizationModels,
}

/// What the lane decided about a submitted job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitDecision {
    Accepted,
    /// The single slot is held; the caller waits (M16.1: one job at a time).
    Busy,
    /// This transcript already has a job queued or running.
    Duplicate,
}

/// The answer to a cancel request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelDecision {
    /// A queued job: removed, never started.
    Removed,
    /// A running job: the token is set; the slot stays held until the worker
    /// discards the result (there is no mid-run abort).
    Running,
    /// Already asked; idempotent.
    AlreadyRequested,
    /// Nothing to cancel.
    Inert,
}

/// The outcome of handing a finished result back to the lane.
#[derive(Debug, Clone, PartialEq)]
pub enum Admit {
    /// The turns are the job's result and should be written.
    Stored {
        transcript_id: Uuid,
        speakers: Vec<Speaker>,
        turns: Vec<SpeakerTurn>,
    },
    /// The user cancelled while sherpa ran; the result was thrown away.
    Discarded { transcript_id: Uuid },
    /// A failure (or silence, which is `Ok` + `Done` with no turns).
    Failed { transcript_id: Uuid, reason: String },
    /// No job was running (a late result from a previous job).
    Stale,
}

/// The one-slot lane. Pure: the worker and the tests drive the same state
/// machine, so "one slot" and "discard at the await point" are provable without
/// a model.
#[derive(Debug, Default)]
pub struct DiarizationLane {
    queued: Option<DiarizationJob>,
    running: Option<(DiarizationJob, CancelToken)>,
    status: LaneStatus,
    last: Option<LaneStatus>,
}

impl DiarizationLane {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(&self) -> &LaneStatus {
        &self.status
    }

    /// The last terminal status, kept after the lane returns to idle so the UI
    /// can show "labels ready" / "stopped" once.
    pub fn last_status(&self) -> Option<&LaneStatus> {
        self.last.as_ref()
    }

    pub fn is_idle(&self) -> bool {
        self.queued.is_none() && self.running.is_none()
    }

    /// Accepts a job. One slot, one job per transcript.
    pub fn submit(&mut self, job: DiarizationJob) -> SubmitDecision {
        if self.running.is_some() {
            return SubmitDecision::Busy;
        }
        if let Some(queued) = &self.queued {
            if queued.transcript_id == job.transcript_id {
                return SubmitDecision::Duplicate;
            }
            return SubmitDecision::Busy;
        }
        self.status = LaneStatus::Queued {
            transcript_id: job.transcript_id,
        };
        self.queued = Some(job);
        SubmitDecision::Accepted
    }

    /// The job currently holding the slot (params/provenance for the run row a
    /// successful result will write).
    pub fn running_job(&self) -> Option<&DiarizationJob> {
        self.running.as_ref().map(|(job, _)| job)
    }

    /// The worker takes the queued job (None when the lane is empty). The
    /// returned token is the one a cancel sets.
    pub fn take(&mut self) -> Option<(DiarizationJob, CancelToken)> {
        let job = self.queued.take()?;
        let token = CancelToken::new();
        self.status = LaneStatus::Running {
            transcript_id: job.transcript_id,
            pct: 0,
            cancelling: false,
        };
        self.running = Some((job.clone(), token.clone()));
        Some((job, token))
    }

    /// Progress from the engine (0–100). Ignored unless a job is running.
    pub fn on_progress(&mut self, pct: u8) {
        if let LaneStatus::Running {
            transcript_id,
            pct: current,
            cancelling,
        } = self.status
        {
            self.status = LaneStatus::Running {
                transcript_id,
                pct: pct.min(100).max(current),
                cancelling,
            };
        }
    }

    /// Cancels the queued or running job.
    pub fn cancel(&mut self) -> CancelDecision {
        match self.status {
            LaneStatus::Running { .. } => {
                if let Some((_, token)) = &self.running {
                    if token.is_cancelled() {
                        return CancelDecision::AlreadyRequested;
                    }
                    token.cancel();
                }
                if let LaneStatus::Running {
                    transcript_id, pct, ..
                } = self.status
                {
                    self.status = LaneStatus::Running {
                        transcript_id,
                        pct,
                        cancelling: true,
                    };
                }
                CancelDecision::Running
            }
            LaneStatus::Queued { transcript_id } => {
                self.queued = None;
                self.status = LaneStatus::Cancelled {
                    transcript_id,
                    discarded: false,
                };
                self.last = Some(self.status.clone());
                CancelDecision::Removed
            }
            _ => CancelDecision::Inert,
        }
    }

    /// Hands the result (or failure) back. This is the **await point**: a
    /// cancelled job's turns are dropped here, never written.
    pub fn admit(
        &mut self,
        result: Result<(Vec<Speaker>, Vec<SpeakerTurn>), DiarizationError>,
    ) -> Admit {
        let Some((job, token)) = self.running.take() else {
            return Admit::Stale;
        };
        let transcript_id = job.transcript_id;
        self.queued = None;
        if token.is_cancelled() {
            self.status = LaneStatus::Cancelled {
                transcript_id,
                discarded: true,
            };
            self.last = Some(self.status.clone());
            return Admit::Discarded { transcript_id };
        }
        match result {
            Ok((speakers, turns)) => {
                self.status = LaneStatus::Done { transcript_id };
                self.last = Some(self.status.clone());
                Admit::Stored {
                    transcript_id,
                    speakers,
                    turns,
                }
            }
            Err(err) => {
                let reason = err.to_string();
                self.status = LaneStatus::Failed {
                    transcript_id,
                    reason: reason.clone(),
                };
                self.last = Some(self.status.clone());
                Admit::Failed {
                    transcript_id,
                    reason,
                }
            }
        }
    }

    /// Returns the lane to `Idle` after the UI has consumed a terminal status.
    pub fn acknowledge(&mut self) {
        if self.status.is_terminal() {
            self.status = LaneStatus::Idle;
        }
    }
}

/// Executes one job's inference at the engine seam, with the cancellation
/// checks at every await point the binding allows.
///
/// The worker calls this; it is also the deterministic test seam (a fake
/// engine, no thread).
pub fn execute(
    engine: &mut dyn DiarizationEngine,
    samples: &[f32],
    audio_ms: i64,
    params: &DiarizeParams,
    cancel: &CancelToken,
    progress: &mut (dyn FnMut(u8) + Send),
) -> Result<(Vec<Speaker>, Vec<SpeakerTurn>, i64), DiarizationError> {
    params.validate()?;
    if audio_ms > MAX_AUDIO_MS {
        return Err(DiarizationError::TooLong {
            audio_ms,
            cap_ms: MAX_AUDIO_MS,
        });
    }
    // Await point 1: before any work. A queued-then-cancelled job never gets
    // here (the lane removes it), but a cancellation that landed between take
    // and this call must not start the model.
    if cancel.is_cancelled() {
        return Err(DiarizationError::Cancelled);
    }
    let raw = engine.run(samples, params, progress)?;
    // Await point 2: sherpa returned. A cancel during `compute` is answered
    // here — the turns are discarded by the lane, never normalised or stored.
    if cancel.is_cancelled() {
        return Err(DiarizationError::Cancelled);
    }
    let (speakers, turns) = turns_from_raw(&raw, audio_ms)?;
    Ok((speakers, turns, audio_ms))
}

// ---------------------------------------------------------------------------
// The production adapter (sherpa-onnx, the `parakeet` linkage)
// ---------------------------------------------------------------------------

/// The sherpa-onnx diarizer. Compiled only where sherpa-rs is linked (the
/// `parakeet` feature); elsewhere [`SherpaDiarizer::load`] is the honest
/// refusal and [`engine_linked`] is false.
pub struct SherpaDiarizer {
    #[cfg(feature = "parakeet")]
    inner: sherpa_rs::diarize::Diarize,
    #[cfg(feature = "parakeet")]
    provider: String,
}

impl std::fmt::Debug for SherpaDiarizer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SherpaDiarizer")
            .field("linked", &engine_linked())
            .finish_non_exhaustive()
    }
}

impl SherpaDiarizer {
    /// Loads both ONNX sessions. The model files are verified (size + sha256)
    /// before sherpa sees them, so a missing/corrupt install is a named error
    /// rather than the binding's pathless init failure.
    #[cfg(feature = "parakeet")]
    pub fn load(
        models: &DiarizationModels,
        params: &DiarizeParams,
    ) -> Result<Self, DiarizationError> {
        params.validate()?;
        match check_models(models) {
            ModelCheck::Ready => {}
            other => {
                return Err(DiarizationError::Model(
                    other
                        .reason()
                        .unwrap_or_else(|| "model check failed".to_string()),
                ))
            }
        }
        let provider = crate::parakeet::provider().to_string();
        let config = sherpa_rs::diarize::DiarizeConfig {
            // 0 means threshold clustering; `Some(0)` is explicitly *not* the
            // binding's 4-cluster default (M16H §6.3).
            num_clusters: Some(params.num_clusters),
            threshold: Some(params.threshold),
            min_duration_on: Some(params.min_duration_on),
            min_duration_off: Some(params.min_duration_off),
            provider: Some(provider.clone()),
            debug: false,
        };
        let inner =
            sherpa_rs::diarize::Diarize::new(&models.segmentation, &models.embedding, config)
                .map_err(|err| {
                    DiarizationError::Model(format!(
                        "the diarization models could not be loaded ({} + {}): {err}",
                        models.segmentation.display(),
                        models.embedding.display()
                    ))
                })?;
        Ok(Self { inner, provider })
    }

    #[cfg(not(feature = "parakeet"))]
    pub fn load(
        _models: &DiarizationModels,
        _params: &DiarizeParams,
    ) -> Result<Self, DiarizationError> {
        Err(DiarizationError::Unavailable(
            "this build was compiled without the sherpa-onnx engine".to_string(),
        ))
    }

    /// The provider the sessions were created with (provenance/debug).
    pub fn provider(&self) -> &str {
        #[cfg(feature = "parakeet")]
        {
            &self.provider
        }
        #[cfg(not(feature = "parakeet"))]
        {
            "none"
        }
    }
}

#[cfg(feature = "parakeet")]
impl DiarizationEngine for SherpaDiarizer {
    fn run(
        &mut self,
        samples: &[f32],
        _params: &DiarizeParams,
        progress: &mut (dyn FnMut(u8) + Send),
    ) -> Result<Vec<RawTurn>, DiarizationError> {
        // The binding stores its callback and invokes it from its own thread,
        // so the callback must be `Send + 'static` — it cannot borrow the
        // caller's progress sink. Bridge it with a channel: the caller's
        // progress sink is driven by a pump on a scoped thread that drains the
        // channel for exactly as long as `compute` runs (the sender drops with
        // the callback when `compute` returns, ending the pump).
        let (tx, rx) = std::sync::mpsc::channel::<u8>();
        let callback: Box<dyn Fn(i32, i32) -> i32 + Send + 'static> =
            Box::new(move |done: i32, total: i32| {
                if total > 0 {
                    let _ = tx.send(((done as f32 / total as f32) * 100.0).round() as u8);
                }
                // The value is deliberately 0: a non-zero return would promise
                // an abort the C++ pipeline does not perform.
                0
            });
        let segments = std::thread::scope(|scope| {
            let pump = scope.spawn(|| {
                for pct in rx {
                    progress(pct);
                }
            });
            let out = self
                .inner
                .compute(samples.to_vec(), Some(callback))
                .map_err(|err| {
                    let message = err.to_string();
                    // M16H §6.5: silence is `Err("No segments found…")`, not a
                    // failure. It maps to zero turns ("no speech"), not
                    // "failed".
                    if message.contains("No segments found") {
                        DiarizationError::Engine("__silence__".to_string())
                    } else {
                        DiarizationError::Engine(message)
                    }
                });
            // `compute` returning drops the callback, which drops the sender,
            // which ends the pump — join so the borrow outlives the scope.
            let _ = pump.join();
            out
        });
        match segments {
            Ok(segments) => Ok(segments
                .into_iter()
                .map(|segment| RawTurn {
                    start_ms: (segment.start * 1000.0).round() as i64,
                    end_ms: (segment.end * 1000.0).round() as i64,
                    speaker: segment.speaker,
                })
                .collect()),
            Err(DiarizationError::Engine(message)) if message == "__silence__" => Ok(Vec::new()),
            Err(err) => Err(err),
        }
    }
}

#[cfg(not(feature = "parakeet"))]
impl DiarizationEngine for SherpaDiarizer {
    fn run(
        &mut self,
        _samples: &[f32],
        _params: &DiarizeParams,
        _progress: &mut (dyn FnMut(u8) + Send),
    ) -> Result<Vec<RawTurn>, DiarizationError> {
        Err(DiarizationError::Unavailable(
            "this build was compiled without the sherpa-onnx engine".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// The worker
// ---------------------------------------------------------------------------

/// One event the UI polls per frame. Mirrors [`LaneStatus`] plus the turns a
/// successful run produced (the runtime writes them through the store).
#[derive(Debug, Clone)]
pub enum DiarizationEvent {
    Progress {
        transcript_id: Uuid,
        pct: u8,
    },
    Done {
        transcript_id: Uuid,
        speakers: Vec<Speaker>,
        turns: Vec<SpeakerTurn>,
        /// The decoded audio length the turns describe (the run's provenance).
        audio_ms: i64,
    },
    Failed {
        transcript_id: Uuid,
        reason: String,
    },
    Cancelled {
        transcript_id: Uuid,
        discarded: bool,
    },
}

/// The threaded runner: one worker thread, one job at a time, events over a
/// channel. The frame loop polls [`DiarizationWorker::try_recv`]; a quit drops
/// the sender (the thread finishes its current job and exits — a sherpa job is
/// not interruptible, the documented contract).
pub struct DiarizationWorker {
    jobs: Option<Sender<(DiarizationJob, CancelToken)>>,
    events: Receiver<DiarizationEvent>,
    handle: Option<JoinHandle<()>>,
    busy: bool,
}

impl DiarizationWorker {
    /// The production worker (loads the models per job, so a re-run with a
    /// different count always uses fresh sessions).
    pub fn new() -> Self {
        Self::with_factory(Box::new(|models, params| {
            SherpaDiarizer::load(models, params)
                .map(|diarizer| Box::new(diarizer) as Box<dyn DiarizationEngine>)
        }))
    }

    /// A worker over an injected engine factory (tests: a fake engine, no
    /// model, no thread-length dependency on sherpa).
    pub fn with_factory(factory: EngineFactory) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<(DiarizationJob, CancelToken)>();
        let (event_tx, event_rx) = mpsc::channel::<DiarizationEvent>();
        let handle = thread::Builder::new()
            .name("suramak-diarization".to_string())
            .spawn(move || {
                while let Ok((job, token)) = job_rx.recv() {
                    let transcript_id = job.transcript_id;
                    let engine = match factory(&job.models, &job.params) {
                        Ok(engine) => engine,
                        Err(err) => {
                            let _ = event_tx.send(DiarizationEvent::Failed {
                                transcript_id,
                                reason: err.to_string(),
                            });
                            continue;
                        }
                    };
                    // Decoding lives with the caller (the runtime already has
                    // the audio path); the worker runs inference and reports.
                    let mut engine = engine;
                    let callback = |pct: u8| {
                        let _ = event_tx.send(DiarizationEvent::Progress { transcript_id, pct });
                    };
                    // The worker's own samples are supplied by the job.
                    match job.decode_samples().and_then(|(samples, audio_ms)| {
                        let mut emitter = callback;
                        execute(
                            engine.as_mut(),
                            &samples,
                            audio_ms,
                            &job.params,
                            &token,
                            &mut emitter,
                        )
                    }) {
                        Ok((speakers, turns, audio_ms)) => {
                            let _ = event_tx.send(DiarizationEvent::Done {
                                transcript_id,
                                speakers,
                                turns,
                                audio_ms,
                            });
                        }
                        Err(DiarizationError::Cancelled) => {
                            let _ = event_tx.send(DiarizationEvent::Cancelled {
                                transcript_id,
                                discarded: true,
                            });
                        }
                        Err(err) => {
                            let _ = event_tx.send(DiarizationEvent::Failed {
                                transcript_id,
                                reason: err.to_string(),
                            });
                        }
                    }
                }
            })
            .expect("failed to spawn the suramak-diarization thread");
        Self {
            jobs: Some(job_tx),
            events: event_rx,
            handle: Some(handle),
            busy: false,
        }
    }

    /// Submits one job with the token a cancel sets. The caller enforces the
    /// one-slot rule first (the lane).
    pub fn submit(&mut self, job: DiarizationJob, token: CancelToken) -> Result<(), String> {
        let jobs = self
            .jobs
            .as_ref()
            .ok_or_else(|| "the worker is shut down".to_string())?;
        jobs.send((job, token))
            .map_err(|_| "the worker is shut down".to_string())?;
        self.busy = true;
        Ok(())
    }

    pub fn try_recv(&mut self) -> Option<DiarizationEvent> {
        match self.events.try_recv() {
            Ok(event) => {
                if !matches!(event, DiarizationEvent::Progress { .. }) {
                    self.busy = false;
                }
                Some(event)
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }
}

impl Default for DiarizationWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DiarizationWorker {
    fn drop(&mut self) {
        // Closing the job channel lets the thread finish the current job and
        // exit; the join is skipped (a model run cannot be interrupted).
        self.jobs.take();
        self.handle.take();
    }
}

impl DiarizationJob {
    /// Decodes the job's audio and converts it to the 16 kHz mono buffer the
    /// pipeline requires (M16H §2.2: passing 8 kHz silently halves every
    /// timestamp, so the conversion is mandatory, not an optimisation).
    pub fn decode_samples(&self) -> Result<(Vec<f32>, i64), DiarizationError> {
        let decoded = suramak_core::decode::decode_file(&self.audio)
            .map_err(|err| DiarizationError::Decode(err.to_string()))?;
        let samples = suramak_core::resample::to_16khz_mono(&decoded)
            .map_err(|err| DiarizationError::Decode(err.to_string()))?;
        let audio_ms = (samples.len() as i64 * 1000) / 16_000;
        Ok((samples, audio_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_job(id: Uuid) -> DiarizationJob {
        DiarizationJob {
            transcript_id: id,
            audio: PathBuf::from("/tmp/does-not-exist.wav"),
            params: DiarizeParams::auto(),
            models: DiarizationModels::in_dir(Path::new("/tmp/no-models")),
        }
    }

    struct FakeEngine {
        turns: Vec<RawTurn>,
        err: Option<DiarizationError>,
        progress: u8,
    }

    impl FakeEngine {
        fn with(turns: Vec<RawTurn>) -> Self {
            Self {
                turns,
                err: None,
                progress: 100,
            }
        }
    }

    impl DiarizationEngine for FakeEngine {
        fn run(
            &mut self,
            _samples: &[f32],
            _params: &DiarizeParams,
            progress: &mut (dyn FnMut(u8) + Send),
        ) -> Result<Vec<RawTurn>, DiarizationError> {
            progress(self.progress);
            if let Some(err) = &self.err {
                return Err(err.clone());
            }
            Ok(self.turns.clone())
        }
    }

    #[test]
    fn the_binding_default_is_never_the_production_parameter() {
        let params = DiarizeParams::default();
        assert_eq!(params.num_clusters, 0, "auto, never the binding's 4");
        assert_eq!(params.threshold, 0.6, "the S0 candidate threshold");
        assert!(DiarizeParams::with_count(2).is_ok());
        assert!(DiarizeParams::with_count(6).is_ok());
        assert!(DiarizeParams::with_count(1).is_err());
        assert!(DiarizeParams::with_count(7).is_err());
        let mut bad = DiarizeParams::auto();
        bad.threshold = 1.5;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn raw_turns_become_dense_speakers_in_first_appearance_order() {
        let raw = vec![
            RawTurn {
                start_ms: 5_000,
                end_ms: 7_000,
                speaker: 7,
            },
            RawTurn {
                start_ms: 0,
                end_ms: 2_000,
                speaker: 3,
            },
            RawTurn {
                start_ms: 2_000,
                end_ms: 5_000,
                speaker: 7,
            },
            RawTurn {
                start_ms: 7_000,
                end_ms: 7_000,
                speaker: 3,
            }, // empty: dropped
        ];
        let (speakers, turns) = turns_from_raw(&raw, 8_000).unwrap();
        // Sorted by start: speaker 3 appears first → app speaker 0.
        assert_eq!(speakers.len(), 2);
        assert_eq!(speakers[0].label, "Speaker 1");
        assert_eq!(speakers[1].label, "Speaker 2");
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].speaker_id, 0);
        assert_eq!(turns[1].speaker_id, 1);
        assert_eq!(turns[2].speaker_id, 1);
        assert_eq!(
            turns.iter().map(|turn| turn.turn_idx).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        // Times are clamped into the audio.
        let (_, clamped) = turns_from_raw(
            &[RawTurn {
                start_ms: -100,
                end_ms: 99_000,
                speaker: 0,
            }],
            4_000,
        )
        .unwrap();
        assert_eq!((clamped[0].start_ms, clamped[0].end_ms), (0, 4_000));
    }

    #[test]
    fn silence_is_no_turns_not_a_failure() {
        assert_eq!(
            turns_from_raw(&[], 5_000).unwrap(),
            (Vec::new(), Vec::new())
        );
        // Rows entirely past the audio clamp away.
        let (speakers, turns) = turns_from_raw(
            &[RawTurn {
                start_ms: 9_000,
                end_ms: 10_000,
                speaker: 0,
            }],
            5_000,
        )
        .unwrap();
        assert!(speakers.is_empty() && turns.is_empty());
    }

    #[test]
    fn one_slot_and_one_job_per_transcript() {
        let mut lane = DiarizationLane::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert_eq!(lane.submit(sample_job(a)), SubmitDecision::Accepted);
        assert_eq!(lane.submit(sample_job(a)), SubmitDecision::Duplicate);
        assert_eq!(lane.submit(sample_job(b)), SubmitDecision::Busy);
        assert!(matches!(lane.status(), LaneStatus::Queued { .. }));
        let (_, token) = lane.take().expect("the job starts");
        assert!(!token.is_cancelled());
        assert!(matches!(lane.status(), LaneStatus::Running { pct: 0, .. }));
        assert_eq!(lane.submit(sample_job(b)), SubmitDecision::Busy);
        lane.on_progress(40);
        assert!(matches!(lane.status(), LaneStatus::Running { pct: 40, .. }));
    }

    #[test]
    fn cancelling_a_queued_job_removes_it_and_it_never_starts() {
        let mut lane = DiarizationLane::new();
        let a = Uuid::new_v4();
        assert_eq!(lane.submit(sample_job(a)), SubmitDecision::Accepted);
        assert_eq!(lane.cancel(), CancelDecision::Removed);
        assert_eq!(lane.take().map(|(job, _)| job.transcript_id), None);
        assert_eq!(
            lane.status(),
            &LaneStatus::Cancelled {
                transcript_id: a,
                discarded: false
            }
        );
        assert_eq!(lane.cancel(), CancelDecision::Inert);
    }

    /// The core contract: cancel while the model is running is answered at the
    /// next await point by **discarding** the result — never a partial write.
    #[test]
    fn a_cancel_during_compute_discards_the_result_at_the_await_point() {
        let mut lane = DiarizationLane::new();
        let a = Uuid::new_v4();
        lane.submit(sample_job(a));
        let (job, token) = lane.take().expect("running");
        lane.on_progress(60);
        assert_eq!(lane.cancel(), CancelDecision::Running);
        assert_eq!(lane.cancel(), CancelDecision::AlreadyRequested);
        assert!(
            matches!(
                lane.status(),
                LaneStatus::Running {
                    pct: 60,
                    cancelling: true,
                    ..
                }
            ),
            "the slot stays held while sherpa runs"
        );
        assert!(token.is_cancelled());
        // The fake engine finished anyway (no mid-run abort). The lane must
        // throw the turns away.
        let mut engine = FakeEngine::with(vec![RawTurn {
            start_ms: 0,
            end_ms: 1_000,
            speaker: 0,
        }]);
        let result = execute(
            &mut engine,
            &[0.0; 16_000],
            1_000,
            &job.params,
            &token,
            &mut |_| {},
        );
        assert_eq!(result, Err(DiarizationError::Cancelled));
        let admitted = lane.admit(result.map(|(speakers, turns, _)| (speakers, turns)));
        assert_eq!(admitted, Admit::Discarded { transcript_id: a });
        assert_eq!(
            lane.status(),
            &LaneStatus::Cancelled {
                transcript_id: a,
                discarded: true
            }
        );
        assert!(lane.is_idle(), "the slot is free again");
    }

    #[test]
    fn progress_reaches_one_hundred_and_a_finished_run_returns_turns() {
        let mut lane = DiarizationLane::new();
        let a = Uuid::new_v4();
        lane.submit(sample_job(a));
        let (job, token) = lane.take().unwrap();
        let mut engine = FakeEngine::with(vec![RawTurn {
            start_ms: 0,
            end_ms: 1_000,
            speaker: 3,
        }]);
        let mut seen = 0u8;
        let result = execute(
            &mut engine,
            &[0.0; 16_000],
            1_000,
            &job.params,
            &token,
            &mut |pct| seen = pct,
        )
        .unwrap();
        assert_eq!(seen, 100);
        let admitted = lane.admit(Ok((result.0, result.1)));
        match admitted {
            Admit::Stored {
                transcript_id,
                speakers,
                turns,
            } => {
                assert_eq!(transcript_id, a);
                assert_eq!(speakers.len(), 1);
                assert_eq!(turns[0].speaker_id, 0);
            }
            other => panic!("expected stored, got {other:?}"),
        }
        assert_eq!(lane.status(), &LaneStatus::Done { transcript_id: a });
        lane.acknowledge();
        assert_eq!(lane.status(), &LaneStatus::Idle);
        assert!(lane.last_status().is_some());
    }

    #[test]
    fn an_engine_failure_is_named_and_does_not_wedge_the_slot() {
        let mut lane = DiarizationLane::new();
        let a = Uuid::new_v4();
        lane.submit(sample_job(a));
        let (job, token) = lane.take().unwrap();
        let mut engine = FakeEngine {
            turns: Vec::new(),
            err: Some(DiarizationError::Engine("boom".to_string())),
            progress: 0,
        };
        let result = execute(
            &mut engine,
            &[0.0; 16_000],
            1_000,
            &job.params,
            &token,
            &mut |_| {},
        );
        assert_eq!(result, Err(DiarizationError::Engine("boom".to_string())));
        assert!(matches!(
            lane.admit(result.map(|(speakers, turns, _)| (speakers, turns))),
            Admit::Failed { reason, .. } if reason.contains("boom")
        ));
        assert!(lane.is_idle());
        let b = Uuid::new_v4();
        assert_eq!(lane.submit(sample_job(b)), SubmitDecision::Accepted);
    }

    #[test]
    fn the_duration_cap_refuses_before_the_engine_runs() {
        let params = DiarizeParams::auto();
        let mut engine = FakeEngine::with(Vec::new());
        let err = execute(
            &mut engine,
            &[],
            MAX_AUDIO_MS + 1,
            &params,
            &CancelToken::new(),
            &mut |_| {},
        )
        .expect_err("over the cap");
        assert_eq!(
            err,
            DiarizationError::TooLong {
                audio_ms: MAX_AUDIO_MS + 1,
                cap_ms: MAX_AUDIO_MS
            }
        );
        // At the cap it runs.
        assert!(execute(
            &mut engine,
            &[],
            MAX_AUDIO_MS,
            &params,
            &CancelToken::new(),
            &mut |_| {}
        )
        .is_ok());
    }

    #[test]
    fn model_checks_name_the_file_and_the_failing_number() {
        let dir = std::env::temp_dir().join(format!("suramak-diar-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.onnx");
        assert_eq!(
            check_model_file(&path, 3, "abc"),
            ModelCheck::Missing { path: path.clone() }
        );
        std::fs::write(&path, b"ab").unwrap();
        assert!(matches!(
            check_model_file(&path, 3, "abc"),
            ModelCheck::WrongSize {
                actual: 2,
                expected: 3,
                ..
            }
        ));
        std::fs::write(&path, b"abc").unwrap();
        // sha256("abc")
        let sha = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(check_model_file(&path, 3, sha), ModelCheck::Ready);
        assert!(matches!(
            check_model_file(&path, 3, "deadbeef"),
            ModelCheck::WrongSha { .. }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_size_probe_is_cheap_and_the_full_check_catches_a_wrong_sha() {
        let dir = std::env::temp_dir().join(format!("suramak-diar-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let models = DiarizationModels {
            segmentation: dir.join("model.onnx"),
            embedding: dir.join("embedding.onnx"),
        };
        std::fs::write(&models.segmentation, vec![0u8; SEGMENTATION_BYTES as usize]).unwrap();
        std::fs::write(&models.embedding, vec![0u8; EMBEDDING_BYTES as usize]).unwrap();
        // Right size, wrong bytes: the UI probe says ready (cheap), the worker's
        // full check refuses with the file named.
        assert_eq!(check_models_size(&models), ModelCheck::Ready);
        assert!(matches!(check_models(&models), ModelCheck::WrongSha { .. }));
        std::fs::remove_file(&models.embedding).unwrap();
        assert!(matches!(
            check_models_size(&models),
            ModelCheck::Missing { .. }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_platform_without_the_dylibs_reports_unavailable_and_names_why() {
        let ready = ModelCheck::Ready;
        assert_eq!(
            availability(Platform::MacOs, true, &ready),
            Availability::Ready
        );
        // A platform whose packaging does not ship the dylibs is a designed
        // unavailable state — never a silently absent control.
        for platform in [Platform::Windows, Platform::Linux, Platform::Android] {
            let state = availability(platform, true, &ready);
            assert!(!state.is_ready(), "{platform:?} must be unavailable");
            assert!(
                state.reason().unwrap().contains("macOS app only"),
                "{platform:?}: {:?}",
                state.reason()
            );
        }
        // A build that never linked sherpa says so first.
        assert!(availability(Platform::MacOs, false, &ready)
            .reason()
            .unwrap()
            .contains("compiled without"));
        // A missing model is named.
        let missing = ModelCheck::Missing {
            path: PathBuf::from("/models/model.onnx"),
        };
        let state = availability(Platform::MacOs, true, &missing);
        assert!(state.reason().unwrap().contains("/models/model.onnx"));
        // The current build reports the same rule.
        assert_eq!(
            current_availability(&DiarizationModels::in_dir(Path::new("/tmp/no-models")))
                .is_ready(),
            engine_linked() && cfg!(target_os = "macos")
        );
    }

    #[test]
    fn the_worker_reports_a_missing_file_as_a_named_failure_and_frees_itself() {
        let mut worker = DiarizationWorker::with_factory(Box::new(|_, _| {
            Ok(Box::new(FakeEngine::with(Vec::new())) as Box<dyn DiarizationEngine>)
        }));
        let job = sample_job(Uuid::new_v4());
        worker.submit(job, CancelToken::new()).unwrap();
        assert!(worker.is_busy());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut event = None;
        while std::time::Instant::now() < deadline && event.is_none() {
            event = worker.try_recv();
            if event.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        match event {
            Some(DiarizationEvent::Failed { reason, .. }) => {
                assert!(reason.contains("decode"), "{reason}");
            }
            other => panic!("expected a named decode failure, got {other:?}"),
        }
        assert!(!worker.is_busy(), "a terminal event frees the worker");
    }
}
