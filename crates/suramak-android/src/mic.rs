//! M13F — microphone recording behind the Android foreground service.
//!
//! The shape mirrors [`crate::jobs`]: Kotlin owns the platform objects (the
//! `RECORD_AUDIO` grant, the `microphone`-typed service, the persistent
//! notification, the audio-focus/device listeners), Rust owns the recorder, the
//! interruption state machine, the journal and the hand-off into the M13E
//! inbox. The two meet in one JSON command ([`MicHost::dispatch`]).
//!
//! ## The loop
//!
//! ```text
//! Record (foreground only)
//!   → reserve <files>/media/<key>.wav      (the M13E inbox path)
//!   → journal: state=recording             (before a sample is captured)
//!   → AAudio capture (cpal/oboe)
//!        callback: mono mixdown → SPSC ring          (the M14B2 path)
//!        consumer: 1024-sample chunks → 16 kHz PCM16  (capture_stream)
//!   → Stop (user / call / focus loss / Bluetooth / service / timeout / error)
//!        finalize the WAV (the desktop bytes, see capture_stream)
//!        journal: state=stopped, stop_reason=<kind>
//!        adopt into the M13E job inbox → the existing transcription flow
//! ```
//!
//! ## Interruptions never continue silently
//!
//! Every one of the plan's interruption kinds is a **clean stop**: the stream is
//! dropped, the ring is drained, the WAV is finalized, and the journal records
//! the reason. A duplicate stop keeps the first reason (a call that also loses
//! audio focus is one stop, attributed to the call). A capture thread that ends
//! on its own while the machine is still `Recording` is settled as
//! [`StopReason::StreamError`] — the state machine has no path from `Recording`
//! to a silent continuation.
//!
//! ## What survives a process kill
//!
//! `files/mic/journal.json` is rewritten atomically when a take opens, at most
//! every [`ELAPSED_JOURNAL_STEP_MS`] while it records, and when it closes. The
//! WAV itself is streamed to `files/media/<key>.wav` with a canonical header, so
//! a kill leaves every captured frame. At the next init the journal replays the
//! open take as [`StopReason::ProcessKilled`], [`crate::capture_stream::repair`]
//! patches the WAV header, and the take is adopted into the inbox — the abrupt
//! end is both journaled and surfaced in [`MicStatus::recovered`].
//!
//! ## Quit
//!
//! [`MicHost::quit`] stops (and finalizes/hands off) any live capture before it
//! returns the steps Kotlin must take next (`stop_service`, then `exit`). The
//! ordered [`QuitOutcome`] is the Rust half of the owner's capture-first Quit
//! invariant; Kotlin's `quit()` executes the steps in that order.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use suramak_asr::capture_stream::{self, StreamedWav};

/// The mic journal's schema version.
pub const MIC_JOURNAL_VERSION: u32 = 1;

/// While recording, the journal records elapsed time at most this often. The
/// notification updates far more frequently; the journal exists for recovery.
pub const ELAPSED_JOURNAL_STEP_MS: u64 = 5_000;

/// How many finished sessions the journal keeps (the newest first). Open and
/// not-yet-handed-off sessions are never pruned.
const JOURNAL_KEEP: usize = 20;

/// How long [`MicHost::start`] waits for the platform to open the microphone.
const START_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the capture thread refreshes the meter while recording.
#[cfg(target_os = "android")]
const METER_POLL: Duration = Duration::from_millis(200);

// ---------------------------------------------------------------------------
// Stop reasons
// ---------------------------------------------------------------------------

/// Why a take ended. The kind is persisted in the journal and named in the
/// notification: an interrupted recording is never an unexplained truncation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The user pressed Stop.
    User,
    /// An incoming (or started) call took the audio path.
    Call,
    /// Another app took audio focus.
    AudioFocusLoss,
    /// The Bluetooth input device disappeared or the input source changed.
    BluetoothDisconnect,
    /// The system ended the service (task removed, low memory, explicit stop).
    ServiceStop,
    /// Android's foreground-service time limit.
    Timeout,
    /// The capture stream failed.
    StreamError,
    /// Recovery only: the process died while the take was open.
    ProcessKilled,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::User => "user",
            StopReason::Call => "call",
            StopReason::AudioFocusLoss => "audio_focus_loss",
            StopReason::BluetoothDisconnect => "bluetooth_disconnect",
            StopReason::ServiceStop => "service_stop",
            StopReason::Timeout => "timeout",
            StopReason::StreamError => "stream_error",
            StopReason::ProcessKilled => "process_killed",
        }
    }

    /// The kinds the plan calls interruptions (the ones the UI explains).
    pub fn is_interruption(self) -> bool {
        matches!(
            self,
            StopReason::Call | StopReason::AudioFocusLoss | StopReason::BluetoothDisconnect
        )
    }

    /// Whether the take was cut by something outside the app.
    pub fn is_abrupt(self) -> bool {
        matches!(self, StopReason::ProcessKilled | StopReason::StreamError)
    }

    /// Parses the bridge's spelling (snake_case, plus the short aliases a
    /// notification action or a test may use).
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "user" | "stop" => Some(StopReason::User),
            "call" | "phone_call" | "incoming_call" => Some(StopReason::Call),
            "audio_focus_loss" | "focus_loss" | "focus" => Some(StopReason::AudioFocusLoss),
            "bluetooth_disconnect" | "bluetooth" | "bt_disconnect" | "headset" => {
                Some(StopReason::BluetoothDisconnect)
            }
            "service_stop" | "service" | "task_removed" => Some(StopReason::ServiceStop),
            "timeout" | "timed_out" => Some(StopReason::Timeout),
            "stream_error" | "error" => Some(StopReason::StreamError),
            "process_killed" | "killed" | "abrupt" => Some(StopReason::ProcessKilled),
            _ => None,
        }
    }

    /// The journal / notification sentence for this reason.
    pub fn note(self) -> &'static str {
        match self {
            StopReason::User => "Stopped. What was recorded is kept.",
            StopReason::Call => {
                "Stopped because a call took the audio path. What was recorded is kept."
            }
            StopReason::AudioFocusLoss => {
                "Stopped because another app took audio focus. What was recorded is kept."
            }
            StopReason::BluetoothDisconnect => {
                "Stopped because the Bluetooth headset disconnected. What was recorded is kept."
            }
            StopReason::ServiceStop => {
                "Stopped because the system ended the recording service. What was recorded is kept."
            }
            StopReason::Timeout => {
                "Stopped at the system's foreground-service time limit. What was recorded is kept."
            }
            StopReason::StreamError => {
                "The microphone stream failed; the part that was captured is kept."
            }
            StopReason::ProcessKilled => {
                "The process ended while recording; the saved part was recovered from disk."
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The state machine
// ---------------------------------------------------------------------------

/// The recorder's observable phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicPhase {
    Idle,
    Starting,
    Recording,
    Stopping,
    Finished,
}

impl MicPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            MicPhase::Idle => "idle",
            MicPhase::Starting => "starting",
            MicPhase::Recording => "recording",
            MicPhase::Stopping => "stopping",
            MicPhase::Finished => "finished",
        }
    }

    /// Whether the take is live (opening or capturing).
    pub fn is_live(self) -> bool {
        matches!(self, MicPhase::Starting | MicPhase::Recording)
    }
}

/// Inputs to [`MicMachine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicEvent {
    /// The user asked to record.
    Start,
    /// The capture session opened.
    Ready,
    /// A stop was requested (user or platform), with its reason.
    Stop(StopReason),
    /// The capture thread ended (it may have ended on its own).
    Finished,
    /// The capture session could not open or died.
    Failed,
}

/// What the host must do next. The machine never performs I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicAction {
    None,
    /// Open the capture session.
    OpenCapture,
    /// Close the capture session with this reason.
    CloseCapture(StopReason),
    /// Settle the take (finalize, journal, hand off) with this reason.
    Settle(StopReason),
}

/// The pure interruption state machine.
///
/// Invariants, each unit-tested:
/// * a stop is accepted in `Starting` (it becomes pending and closes as soon as
///   the session is ready) and in `Recording`;
/// * the first stop reason wins — a call that also loses audio focus is one
///   stop, attributed to the call;
/// * a duplicate stop is a no-op (the notification, the task-removal callback
///   and the audio listener can all fire for one event);
/// * a capture that ends while `Recording` settles as `StreamError`, never as a
///   silent continuation;
/// * `Finished` is reachable only through `Stopping`.
#[derive(Debug, Clone)]
pub struct MicMachine {
    phase: MicPhase,
    pending: Option<StopReason>,
    reason: Option<StopReason>,
}

impl Default for MicMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl MicMachine {
    pub fn new() -> Self {
        Self {
            phase: MicPhase::Idle,
            pending: None,
            reason: None,
        }
    }

    pub fn phase(&self) -> MicPhase {
        self.phase
    }

    pub fn stop_reason(&self) -> Option<StopReason> {
        self.reason
    }

    /// Applies one event.
    pub fn on(&mut self, event: MicEvent) -> MicAction {
        match (self.phase, event) {
            (MicPhase::Idle | MicPhase::Finished, MicEvent::Start) => {
                self.phase = MicPhase::Starting;
                self.pending = None;
                self.reason = None;
                MicAction::OpenCapture
            }
            // A second Start while one is opening/live is ignored: the service
            // can be restarted by the platform without creating two takes.
            (MicPhase::Starting | MicPhase::Recording, MicEvent::Start) => MicAction::None,
            (MicPhase::Starting, MicEvent::Ready) => match self.pending {
                Some(reason) => {
                    self.phase = MicPhase::Stopping;
                    MicAction::CloseCapture(reason)
                }
                None => {
                    self.phase = MicPhase::Recording;
                    MicAction::None
                }
            },
            (MicPhase::Starting, MicEvent::Stop(reason)) => {
                // The session is still opening: remember the first reason and
                // close it the moment it is ready.
                self.pending.get_or_insert(reason);
                MicAction::None
            }
            (MicPhase::Recording, MicEvent::Stop(reason)) => {
                // First reason wins; the pending slot is also the "already
                // stopping" latch for duplicate stops.
                let reason = *self.pending.get_or_insert(reason);
                self.phase = MicPhase::Stopping;
                MicAction::CloseCapture(reason)
            }
            (MicPhase::Stopping, MicEvent::Stop(_)) => MicAction::None,
            (MicPhase::Recording, MicEvent::Finished) => {
                // The capture thread ended without a stop request. That is a
                // stream failure, not a silent end.
                self.reason = Some(StopReason::StreamError);
                self.phase = MicPhase::Finished;
                MicAction::Settle(StopReason::StreamError)
            }
            (MicPhase::Starting, MicEvent::Finished | MicEvent::Failed) => {
                let reason = self.pending.unwrap_or(StopReason::StreamError);
                self.reason = Some(reason);
                self.phase = MicPhase::Finished;
                MicAction::Settle(reason)
            }
            (MicPhase::Recording | MicPhase::Stopping, MicEvent::Failed) => {
                let reason = self.pending.unwrap_or(StopReason::StreamError);
                self.reason = Some(reason);
                self.phase = MicPhase::Finished;
                MicAction::Settle(reason)
            }
            (MicPhase::Stopping, MicEvent::Finished) => {
                let reason = self.pending.unwrap_or(StopReason::StreamError);
                self.reason = Some(reason);
                self.phase = MicPhase::Finished;
                MicAction::Settle(reason)
            }
            // A late stop after the take settled, or capture noise while idle,
            // must not resurrect a session.
            (MicPhase::Idle | MicPhase::Finished, MicEvent::Stop(_))
            | (MicPhase::Idle, MicEvent::Finished | MicEvent::Failed)
            | (MicPhase::Finished, MicEvent::Ready | MicEvent::Finished | MicEvent::Failed)
            | (MicPhase::Idle, MicEvent::Ready)
            | (MicPhase::Recording, MicEvent::Ready)
            | (MicPhase::Stopping, MicEvent::Start | MicEvent::Ready) => MicAction::None,
        }
    }
}

// ---------------------------------------------------------------------------
// Journal
// ---------------------------------------------------------------------------

/// One take as the journal stores it. No audio and no transcript text — just
/// the state needed to recover, explain, and hand off.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MicSession {
    pub id: String,
    pub title: String,
    pub source_name: String,
    pub path: PathBuf,
    pub model: String,
    pub language: String,
    pub state: SessionState,
    pub stop_reason: Option<StopReason>,
    pub started_unix_ms: u64,
    pub elapsed_ms: u64,
    pub device_rate: u32,
    pub device_name: String,
    pub frames: u64,
    pub bytes: u64,
    /// `true` once the WAV was adopted into the M13E job inbox.
    pub handed_off: bool,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// The take was open when the journal was last written.
    Recording,
    /// A clean stop (user or platform), WAV finalized.
    Stopped,
    /// The process ended while the take was open; recovered from disk.
    Abrupt,
    /// The take could not be kept (no samples, unreadable file).
    Failed,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionState::Recording => "recording",
            SessionState::Stopped => "stopped",
            SessionState::Abrupt => "abrupt",
            SessionState::Failed => "failed",
        }
    }

    fn is_terminal(self) -> bool {
        matches!(
            self,
            SessionState::Stopped | SessionState::Abrupt | SessionState::Failed
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalFile {
    version: u32,
    sessions: Vec<MicSession>,
}

/// The mic journal: `files/mic/journal.json`, rewritten atomically.
#[derive(Debug)]
pub struct MicJournal {
    path: PathBuf,
    sessions: Vec<MicSession>,
    writes: u64,
}

impl MicJournal {
    /// Loads the journal, moving a torn/corrupt file aside instead of failing
    /// the launch (the same rule as the M13E job journal).
    pub fn load(path: &Path) -> Self {
        let mut journal = Self {
            path: path.to_path_buf(),
            sessions: Vec::new(),
            writes: 0,
        };
        let Ok(bytes) = fs::read(path) else {
            return journal;
        };
        let parsed: Result<JournalFile, _> = serde_json::from_slice(&bytes);
        match parsed {
            Ok(file) if file.version == MIC_JOURNAL_VERSION => {
                journal.sessions = file.sessions;
            }
            Ok(_) | Err(_) => {
                let _ = fs::rename(path, path.with_extension("json.corrupt"));
            }
        }
        journal
    }

    pub fn sessions(&self) -> &[MicSession] {
        &self.sessions
    }

    pub fn writes(&self) -> u64 {
        self.writes
    }

    pub fn find(&self, id: &str) -> Option<&MicSession> {
        self.sessions.iter().find(|session| session.id == id)
    }

    fn find_mut(&mut self, id: &str) -> Option<&mut MicSession> {
        self.sessions.iter_mut().find(|session| session.id == id)
    }

    /// Writes a new open take **before** capture starts.
    pub fn open(&mut self, session: MicSession) -> io::Result<()> {
        self.sessions.retain(|entry| entry.id != session.id);
        self.sessions.push(session);
        self.prune();
        self.save()
    }

    /// Records elapsed time at the bounded step; no write when the step has not
    /// advanced (the service polls once a second, the journal writes every 5 s).
    pub fn note_elapsed(&mut self, id: &str, elapsed_ms: u64) -> io::Result<bool> {
        let Some(session) = self.find_mut(id) else {
            return Ok(false);
        };
        if session.state != SessionState::Recording {
            return Ok(false);
        }
        if elapsed_ms.saturating_sub(session.elapsed_ms) < ELAPSED_JOURNAL_STEP_MS {
            return Ok(false);
        }
        session.elapsed_ms = elapsed_ms;
        self.save()?;
        Ok(true)
    }

    /// Closes a take with its final state and outcome.
    #[allow(clippy::too_many_arguments)]
    pub fn close(
        &mut self,
        id: &str,
        state: SessionState,
        reason: StopReason,
        elapsed_ms: u64,
        outcome: Option<&StreamedWav>,
        handed_off: bool,
        note: &str,
    ) -> io::Result<()> {
        let Some(session) = self.find_mut(id) else {
            return Ok(());
        };
        session.state = state;
        session.stop_reason = Some(reason);
        session.elapsed_ms = elapsed_ms;
        if let Some(wav) = outcome {
            session.frames = wav.frames;
            session.bytes = wav.bytes;
        }
        session.handed_off = handed_off;
        if !note.is_empty() {
            session.note = note.to_string();
        }
        self.prune();
        self.save()
    }

    /// Marks a take as adopted by the M13E inbox.
    pub fn mark_handed_off(&mut self, id: &str) -> io::Result<()> {
        let Some(session) = self.find_mut(id) else {
            return Ok(());
        };
        session.handed_off = true;
        self.save()
    }

    /// Appends a diagnostic note (Doze pressure, a trim warning, …) to a take.
    pub fn append_note(&mut self, id: &str, text: &str) -> io::Result<()> {
        let Some(session) = self.find_mut(id) else {
            return Ok(());
        };
        if !session.note.is_empty() {
            session.note.push(' ');
        }
        session.note.push_str(text);
        self.save()
    }

    /// Replays process death: every still-open take becomes `Abrupt` with
    /// [`StopReason::ProcessKilled`]. Returns the ids that were replayed.
    pub fn replay_open(&mut self) -> io::Result<Vec<String>> {
        let ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|session| session.state == SessionState::Recording)
            .map(|session| session.id.clone())
            .collect();
        for id in &ids {
            if let Some(session) = self.find_mut(id) {
                session.state = SessionState::Abrupt;
                session.stop_reason = Some(StopReason::ProcessKilled);
                session.note = StopReason::ProcessKilled.note().to_string();
            }
        }
        if !ids.is_empty() {
            self.save()?;
        }
        Ok(ids)
    }

    /// Removes one entry (a discarded recovery).
    pub fn remove(&mut self, id: &str) -> io::Result<()> {
        self.sessions.retain(|session| session.id != id);
        self.save()
    }

    /// Keeps the newest finished sessions; never prunes an open or
    /// not-yet-handed-off take.
    fn prune(&mut self) {
        let mut finished: Vec<usize> = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, session)| {
                (session.state.is_terminal() && session.handed_off)
                    // A failed start with no audio is never hand-off material;
                    // keeping it forever would grow the journal without bound.
                    || (session.state == SessionState::Failed && session.frames == 0)
            })
            .map(|(index, _)| index)
            .collect();
        if finished.len() <= JOURNAL_KEEP {
            return;
        }
        finished.truncate(finished.len() - JOURNAL_KEEP);
        let drop: std::collections::HashSet<usize> = finished.into_iter().collect();
        let mut index = 0;
        self.sessions.retain(|_| {
            let keep = !drop.contains(&index);
            index += 1;
            keep
        });
    }

    fn save(&mut self) -> io::Result<()> {
        let file = JournalFile {
            version: MIC_JOURNAL_VERSION,
            sessions: self.sessions.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
        write_atomic(&self.path, &bytes)?;
        self.writes += 1;
        Ok(())
    }
}

/// Atomic file replace: `write tmp → fsync → rename → fsync dir` (the M13E rule).
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Capture plumbing
// ---------------------------------------------------------------------------

/// What the platform reports once the microphone is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedTake {
    pub device_name: String,
    pub sample_rate: u32,
}

/// The live meter, shared between the capture thread and status calls.
#[derive(Debug, Default)]
pub struct Meter {
    rms_bits: AtomicU32,
    dropped: AtomicU64,
}

impl Meter {
    pub fn rms(&self) -> f32 {
        f32::from_bits(self.rms_bits.load(Ordering::Relaxed))
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// A capture thread: the recorder itself stays on that thread (`cpal::Stream`
/// is `!Send`), this handle is what the host and the JNI calls hold.
pub struct CaptureThread {
    stop: Arc<AtomicBool>,
    ready: Receiver<Result<StartedTake, String>>,
    handle: Option<JoinHandle<Result<StreamedWav, String>>>,
    error: Arc<Mutex<Option<String>>>,
    finished: Arc<AtomicBool>,
    meter: Arc<Meter>,
}

impl CaptureThread {
    /// Builds a capture thread from its parts.
    ///
    /// This is the spawner contract: production ([`android_spawner`]) and the
    /// host tests' synthetic recorder both hand back exactly these pieces, so
    /// the interruption/recovery/hand-off rules run against the same handle
    /// the phone uses.
    pub fn from_parts(
        stop: Arc<AtomicBool>,
        ready: Receiver<Result<StartedTake, String>>,
        handle: JoinHandle<Result<StreamedWav, String>>,
        error: Arc<Mutex<Option<String>>>,
        finished: Arc<AtomicBool>,
        meter: Arc<Meter>,
    ) -> Self {
        Self {
            stop,
            ready,
            handle: Some(handle),
            error,
            finished,
            meter,
        }
    }

    /// Waits for the platform to open the microphone.
    pub fn wait_ready(&self, timeout: Duration) -> Result<StartedTake, String> {
        match self.ready.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                Err("the microphone did not open in time".to_string())
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err("the capture thread ended before opening the microphone".to_string())
            }
        }
    }

    pub fn signal_stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|mut slot| slot.take())
    }

    pub fn meter(&self) -> &Meter {
        &self.meter
    }

    /// Signals stop and joins the capture thread, returning the finalized WAV.
    pub fn join(mut self) -> Result<StreamedWav, String> {
        self.signal_stop();
        match self.handle.take() {
            Some(handle) => match handle.join() {
                Ok(result) => result,
                Err(_) => {
                    Err("the capture thread panicked; the recording may be short".to_string())
                }
            },
            None => Err("the capture thread was already joined".to_string()),
        }
    }
}

/// Opens one take. Production is the AAudio recorder; tests inject a synthetic
/// one, which is why the interruption/journal/hand-off rules are host-testable
/// without a microphone.
pub type CaptureSpawner =
    Arc<dyn Fn(PathBuf, Arc<AtomicBool>) -> Result<CaptureThread, String> + Send + Sync>;

/// Adopts a finalized WAV into the M13E inbox.
pub type InboxFn =
    Arc<dyn Fn(&str, &str, &str, &Path, &str, &str) -> Result<(), String> + Send + Sync>;

/// Spawns the real AAudio capture thread (Android only).
#[cfg(target_os = "android")]
pub fn android_spawner() -> CaptureSpawner {
    use suramak_asr::capture_android::AndroidRecorder;

    Arc::new(|path: PathBuf, stop: Arc<AtomicBool>| {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let finished = Arc::new(AtomicBool::new(false));
        let meter = Arc::new(Meter::default());
        let thread_error = Arc::clone(&error);
        let thread_finished = Arc::clone(&finished);
        let thread_meter = Arc::clone(&meter);
        let thread_stop = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("suramak-mic-capture".to_string())
            .spawn(move || {
                let result = (|| -> Result<StreamedWav, String> {
                    let recorder = AndroidRecorder::start(&path).map_err(|err| err.to_string())?;
                    let _ = ready_tx.send(Ok(StartedTake {
                        device_name: recorder.device_name().to_string(),
                        sample_rate: recorder.sample_rate(),
                    }));
                    loop {
                        thread_meter
                            .rms_bits
                            .store(recorder.rms().to_bits(), Ordering::Relaxed);
                        thread_meter
                            .dropped
                            .store(recorder.dropped(), Ordering::Relaxed);
                        if let Some(err) = recorder.take_error() {
                            if let Ok(mut slot) = thread_error.lock() {
                                *slot = Some(err);
                            }
                            break;
                        }
                        if thread_stop.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(METER_POLL);
                    }
                    recorder.stop().map_err(|err| err.to_string())
                })();
                if let Err(err) = &result {
                    // A failed open never sends Ready: tell the waiter why
                    // instead of making it time out.
                    let _ = ready_tx.send(Err(err.clone()));
                    if let Ok(mut slot) = thread_error.lock() {
                        slot.get_or_insert_with(|| err.clone());
                    }
                }
                thread_finished.store(true, Ordering::Release);
                result
            })
            .map_err(|err| format!("could not start the microphone thread: {err}"))?;
        Ok(CaptureThread::from_parts(
            stop, ready_rx, handle, error, finished, meter,
        ))
    })
}

// ---------------------------------------------------------------------------
// Host dirs + status
// ---------------------------------------------------------------------------

/// Where the mic host writes. Derived from the app's `filesDir` in production;
/// explicit in tests.
#[derive(Debug, Clone)]
pub struct MicDirs {
    pub media_dir: PathBuf,
    pub models_dir: PathBuf,
    pub journal_path: PathBuf,
}

impl MicDirs {
    pub fn under(root: &Path) -> Self {
        Self {
            media_dir: root.join("media"),
            models_dir: root.join("models"),
            journal_path: root.join("mic").join("journal.json"),
        }
    }

    fn ensure(&self) -> io::Result<()> {
        fs::create_dir_all(&self.media_dir)?;
        if let Some(parent) = self.journal_path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }
}

/// The final stop, as the notification/UI read it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StoppedTake {
    pub id: String,
    pub title: String,
    pub path: String,
    pub reason: String,
    pub interruption: bool,
    pub elapsed_ms: u64,
    pub frames: u64,
    pub bytes: u64,
    pub handed_off: bool,
    pub note: String,
}

/// A take the journal replayed after a process death.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecoveredTake {
    pub id: String,
    pub title: String,
    pub path: String,
    pub reason: String,
    pub elapsed_ms: u64,
    pub frames: u64,
    pub bytes: u64,
    pub handed_off: bool,
    pub note: String,
}

/// The recorder's full snapshot: what the notification paints and what the
/// record button reflects.
#[derive(Debug, Clone, Serialize)]
pub struct MicStatus {
    pub phase: String,
    pub recording: bool,
    pub elapsed_ms: u64,
    pub started_unix_ms: u64,
    pub input_device: String,
    pub sample_rate: u32,
    pub bytes: u64,
    pub rms: f32,
    pub dropped: u64,
    pub title: String,
    pub last_stop: Option<StoppedTake>,
    pub recovered: Vec<RecoveredTake>,
    pub message: String,
}

impl MicStatus {
    pub fn notification_numbers(&self) -> (u64, u64) {
        (self.elapsed_ms / 1000, self.bytes)
    }
}

/// The ordered steps after a Quit. Kotlin executes them in order; Rust has
/// already stopped and finalized the capture when this is returned.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QuitOutcome {
    pub was_recording: bool,
    pub handoff_key: Option<String>,
    pub next: Vec<&'static str>,
}

// ---------------------------------------------------------------------------
// The host
// ---------------------------------------------------------------------------

struct Inner {
    machine: MicMachine,
    session: Option<MicSession>,
    capture: Option<CaptureThread>,
    started: Option<Instant>,
    started_unix_ms: u64,
    device_name: String,
    sample_rate: u32,
    bytes: u64,
    message: String,
    last_stop: Option<StoppedTake>,
    recovered: Vec<RecoveredTake>,
    journal: MicJournal,
}

impl Inner {
    fn status(&self) -> MicStatus {
        let (elapsed_ms, rms, dropped, bytes) = if let Some(started) = self.started {
            let capture = self.capture.as_ref();
            (
                started.elapsed().as_millis() as u64,
                capture.map(|thread| thread.meter().rms()).unwrap_or(0.0),
                capture.map(|thread| thread.meter().dropped()).unwrap_or(0),
                self.bytes,
            )
        } else {
            (0, 0.0, 0, self.bytes)
        };
        MicStatus {
            phase: self.machine.phase().as_str().to_string(),
            recording: self.machine.phase() == MicPhase::Recording,
            elapsed_ms,
            started_unix_ms: self.started_unix_ms,
            input_device: self.device_name.clone(),
            sample_rate: self.sample_rate,
            bytes,
            rms,
            dropped,
            title: self
                .session
                .as_ref()
                .map(|session| session.title.clone())
                .unwrap_or_default(),
            last_stop: self.last_stop.clone(),
            recovered: self.recovered.clone(),
            message: self.message.clone(),
        }
    }

    fn persist(&mut self) {
        // Journal writes are already atomic; a failure here must not lose the
        // in-memory state, so it is surfaced as a message instead of a panic.
        if self.journal.save().is_err() {
            self.message = "the recording journal could not be written".to_string();
        }
    }
}

/// The process-wide mic host behind `MicBridge`.
pub struct MicHost {
    dirs: MicDirs,
    inner: Mutex<Inner>,
    spawner: CaptureSpawner,
    inbox: InboxFn,
}

impl MicHost {
    /// Builds a host around an explicit capture spawner and inbox — the shape
    /// every host test uses. Production is [`MicHost::production`].
    pub fn with_parts(dirs: MicDirs, spawner: CaptureSpawner, inbox: InboxFn) -> Self {
        let _ = dirs.ensure();
        let mut journal = MicJournal::load(&dirs.journal_path);
        let recovered = recover_sessions(&mut journal, &inbox);
        Self {
            dirs,
            inner: Mutex::new(Inner {
                machine: MicMachine::new(),
                session: None,
                capture: None,
                started: None,
                started_unix_ms: 0,
                device_name: String::new(),
                sample_rate: 0,
                bytes: 0,
                message: String::new(),
                last_stop: None,
                recovered,
                journal,
            }),
            spawner,
            inbox,
        }
    }

    /// The production host: AAudio capture, the M13E job inbox, the app's
    /// `filesDir`.
    #[cfg(target_os = "android")]
    pub fn production(root: &Path) -> Self {
        let inbox: InboxFn = Arc::new(
            |key: &str,
             title: &str,
             source_name: &str,
             path: &Path,
             model: &str,
             language: &str| {
                let host = crate::jobs_host()
                    .ok_or_else(|| "the transcription host is not initialised yet".to_string())?;
                host.adopt_recording(key, title, source_name, path, model, language)
            },
        );
        Self::with_parts(MicDirs::under(root), android_spawner(), inbox)
    }

    pub fn dirs(&self) -> &MicDirs {
        &self.dirs
    }

    pub fn journal_writes(&self) -> u64 {
        self.inner
            .lock()
            .map(|inner| inner.journal.writes())
            .unwrap_or(0)
    }

    /// Starts a take. `foreground` is Kotlin's answer to "is the app in the
    /// foreground right now?" — Android 14+ only permits a `microphone`
    /// foreground service to start while the app is visible, so the host
    /// refuses a start that does not claim it (defence in depth; the service
    /// also promotes itself before calling this).
    pub fn start(
        &self,
        foreground: bool,
        permission_granted: bool,
        title: &str,
        model_id: &str,
        language: &str,
    ) -> Result<MicStatus, String> {
        if !foreground {
            return Err(
                "the microphone service must start from the foreground (Android 14+ \
                 while-in-use rule); bring Suramak to the screen and try again"
                    .to_string(),
            );
        }
        if !permission_granted {
            return Err(
                "RECORD_AUDIO is not granted — enable the microphone for Suramak in Settings"
                    .to_string(),
            );
        }
        let info = suramak_core::models::find_model(model_id)
            .ok_or_else(|| format!("unknown model '{model_id}'"))?;
        if !self.dirs.models_dir.join(info.hf_file).is_file() {
            return Err(format!(
                "the {model_id} model is not installed on this phone yet"
            ));
        }

        let id = new_key();
        let title = if title.trim().is_empty() {
            "Recording".to_string()
        } else {
            title.trim().to_string()
        };
        let source_name = format!("{title}.wav");
        let path = self.dirs.media_dir.join(format!("{id}.wav"));
        let started_unix_ms = unix_ms();
        let session = MicSession {
            id: id.clone(),
            title: title.clone(),
            source_name,
            path: path.clone(),
            model: model_id.to_string(),
            language: language.to_string(),
            state: SessionState::Recording,
            stop_reason: None,
            started_unix_ms,
            elapsed_ms: 0,
            device_rate: 0,
            device_name: String::new(),
            frames: 0,
            bytes: 0,
            handed_off: false,
            note: String::new(),
        };

        {
            let mut inner = self.inner.lock().expect("mic state lock");
            if inner.machine.phase().is_live() {
                return Err("a recording is already running".to_string());
            }
            if inner.machine.on(MicEvent::Start) != MicAction::OpenCapture {
                return Err("the recorder is not ready to start".to_string());
            }
            inner.session = Some(session.clone());
            inner.started = None;
            inner.started_unix_ms = started_unix_ms;
            inner.device_name.clear();
            inner.sample_rate = 0;
            inner.bytes = 0;
            inner.message.clear();
            // The launch-time recovery notice has been seen by the time the
            // user starts a new take; the journal still holds the record.
            inner.recovered.clear();
            inner
                .journal
                .open(session)
                .map_err(|err| format!("the recording journal could not be written: {err}"))?;
        }

        let stop = Arc::new(AtomicBool::new(false));
        let capture = match (self.spawner)(path.clone(), Arc::clone(&stop)) {
            Ok(capture) => capture,
            Err(err) => {
                self.abort_start(&id, &path, &err);
                return Err(err);
            }
        };
        let started = match capture.wait_ready(START_TIMEOUT) {
            Ok(started) => started,
            Err(err) => {
                let _ = capture.join();
                self.abort_start(&id, &path, &err);
                return Err(err);
            }
        };

        let mut inner = self.inner.lock().expect("mic state lock");
        if let MicAction::CloseCapture(reason) = inner.machine.on(MicEvent::Ready) {
            // A stop raced the open. Settle it now rather than leaving a live
            // stream the caller does not know about.
            drop(inner);
            let _ = capture.join();
            self.stop(reason)?;
            return Err("the recording was stopped while it was opening".to_string());
        }
        let recording_title = inner
            .session
            .as_ref()
            .map(|session| session.title.clone())
            .unwrap_or_default();
        inner.capture = Some(capture);
        inner.started = Some(Instant::now());
        inner.device_name = started.device_name.clone();
        inner.sample_rate = started.sample_rate;
        if let Some(session) = inner.session.as_mut() {
            session.device_name = started.device_name;
            session.device_rate = started.sample_rate;
        }
        inner.message = format!("Recording {recording_title}");
        inner.persist();
        Ok(inner.status())
    }

    /// Settles a start that failed before a session was live.
    fn abort_start(&self, id: &str, path: &Path, reason: &str) {
        let mut inner = self.inner.lock().expect("mic state lock");
        let _ = inner.machine.on(MicEvent::Failed);
        let _ = inner.journal.close(
            id,
            SessionState::Failed,
            StopReason::StreamError,
            0,
            None,
            false,
            reason,
        );
        let _ = fs::remove_file(path);
        inner.session = None;
        inner.capture = None;
        inner.started = None;
        inner.message = reason.to_string();
    }

    /// Stops the take cleanly, finalizes the WAV and hands it to the inbox.
    ///
    /// Idempotent: a stop when nothing is live reports `was_recording=false`
    /// and writes nothing.
    pub fn stop(&self, reason: StopReason) -> Result<MicStatus, String> {
        let (capture, id, title) = {
            let mut inner = self.inner.lock().expect("mic state lock");
            if !inner.machine.phase().is_live() {
                inner.message = "not recording".to_string();
                return Ok(inner.status());
            }
            let action = inner.machine.on(MicEvent::Stop(reason));
            if !matches!(action, MicAction::CloseCapture(_)) {
                return Ok(inner.status());
            }
            let capture = inner.capture.take();
            let id = inner
                .session
                .as_ref()
                .map(|session| session.id.clone())
                .unwrap_or_default();
            let title = inner
                .session
                .as_ref()
                .map(|session| session.title.clone())
                .unwrap_or_else(|| "Recording".to_string());
            (capture, id, title)
        };

        let elapsed_ms = {
            let inner = self.inner.lock().expect("mic state lock");
            inner
                .started
                .map(|started| started.elapsed().as_millis() as u64)
                .unwrap_or(0)
        };

        // Close the stream first; the join returns only after the WAV is final.
        let joined = capture.map(CaptureThread::join);
        let (outcome, failure) = match joined {
            Some(Ok(wav)) => (Some(wav), None),
            Some(Err(err)) => {
                // Keep what the file has: a stream that died mid-take is still
                // a take the user asked for.
                let path = {
                    let inner = self.inner.lock().expect("mic state lock");
                    inner
                        .session
                        .as_ref()
                        .map(|session| session.path.clone())
                        .unwrap_or_default()
                };
                (capture_stream::repair(&path).ok(), Some(err))
            }
            None => (
                None,
                Some("the capture session was already gone".to_string()),
            ),
        };

        let mut handed_off = false;
        let mut handoff_error: Option<String> = None;
        let mut note = reason.note().to_string();
        if let Some(wav) = &outcome {
            if wav.frames > 0 {
                let source_name = format!("{title}.wav");
                let path = wav.path.clone();
                let model = {
                    let inner = self.inner.lock().expect("mic state lock");
                    inner
                        .session
                        .as_ref()
                        .map(|session| session.model.clone())
                        .unwrap_or_default()
                };
                let language = {
                    let inner = self.inner.lock().expect("mic state lock");
                    inner
                        .session
                        .as_ref()
                        .map(|session| session.language.clone())
                        .unwrap_or_else(|| "auto".to_string())
                };
                match (self.inbox)(&id, &title, &source_name, &path, &model, &language) {
                    Ok(()) => handed_off = true,
                    Err(err) => {
                        handoff_error = Some(err.clone());
                        note =
                            format!("{note} It could not be queued for transcription yet: {err}");
                    }
                }
            } else {
                note = format!("{}. Nothing was captured.", reason.note());
            }
        }
        if let Some(err) = &failure {
            note = format!("{note} ({err})");
        }

        let mut inner = self.inner.lock().expect("mic state lock");
        let state = if outcome.as_ref().is_some_and(|wav| wav.frames > 0) {
            if reason == StopReason::ProcessKilled {
                SessionState::Abrupt
            } else {
                SessionState::Stopped
            }
        } else {
            SessionState::Failed
        };
        let _ = inner.journal.close(
            &id,
            state,
            reason,
            elapsed_ms,
            outcome.as_ref(),
            handed_off,
            &note,
        );
        let final_path = inner
            .session
            .as_ref()
            .map(|session| session.path.display().to_string())
            .unwrap_or_default();
        let (frames, bytes) = outcome
            .as_ref()
            .map(|wav| (wav.frames, wav.bytes))
            .unwrap_or((0, 0));
        // A take with no audio is deleted: it is not hand-off material and it
        // must not look like a recording in the inbox.
        if frames == 0 {
            if let Some(session) = inner.session.as_ref() {
                let _ = fs::remove_file(&session.path);
            }
        }
        let stopped = StoppedTake {
            id: id.clone(),
            title: title.clone(),
            path: final_path,
            reason: reason.as_str().to_string(),
            interruption: reason.is_interruption(),
            elapsed_ms,
            frames,
            bytes,
            handed_off,
            note: note.clone(),
        };
        inner.last_stop = Some(stopped.clone());
        inner.bytes = bytes;
        inner.capture = None;
        inner.started = None;
        inner.message = if handed_off {
            format!("{title} is saved and queued for transcription.")
        } else if frames > 0 {
            format!("{title} is saved. {note}")
        } else {
            note.clone()
        };
        let _ = inner.machine.on(MicEvent::Finished);
        let mut status = inner.status();
        if let Some(err) = handoff_error {
            status.message = format!("{} ({err})", status.message);
        }
        Ok(status)
    }

    /// Stops any live capture, then returns the ordered Quit steps.
    ///
    /// **The invariant:** capture first, service second, exit third. The
    /// capture has been stopped, finalized and handed off by the time this
    /// returns; `next` is what Kotlin executes.
    pub fn quit(&self) -> QuitOutcome {
        let was_recording = self
            .inner
            .lock()
            .map(|inner| inner.machine.phase().is_live())
            .unwrap_or(false);
        let mut handoff_key = None;
        if was_recording {
            if let Ok(status) = self.stop(StopReason::User) {
                handoff_key =
                    status.last_stop.and_then(
                        |stop| {
                            if stop.handed_off {
                                Some(stop.id)
                            } else {
                                None
                            }
                        },
                    );
            }
        }
        QuitOutcome {
            was_recording,
            handoff_key,
            next: vec!["stop_service", "exit"],
        }
    }

    /// Records a diagnostic note against the live take (Doze pressure, a
    /// trimmed cache, a device switch the listeners saw but did not stop).
    pub fn note(&self, text: &str) {
        let mut inner = self.inner.lock().expect("mic state lock");
        if let Some(session) = inner.session.as_ref() {
            let id = session.id.clone();
            let _ = inner.journal.append_note(&id, text);
        }
        inner.message = text.to_string();
    }

    /// The launch-time recovery list (process deaths and hand-offs that could
    /// not reach the inbox at stop time are retried here).
    pub fn recover(&self) -> Vec<RecoveredTake> {
        let pending: Vec<MicSession> = {
            let inner = self.inner.lock().expect("mic state lock");
            inner
                .journal
                .sessions()
                .iter()
                .filter(|session| !session.handed_off && session.frames > 0)
                .cloned()
                .collect()
        };
        for session in pending {
            let source_name = session.source_name.clone();
            if (self.inbox)(
                &session.id,
                &session.title,
                &source_name,
                &session.path,
                &session.model,
                &session.language,
            )
            .is_ok()
            {
                let mut inner = self.inner.lock().expect("mic state lock");
                let _ = inner.journal.mark_handed_off(&session.id);
            }
        }
        let inner = self.inner.lock().expect("mic state lock");
        inner.recovered.clone()
    }

    /// Discards one recovered take and its file.
    pub fn discard(&self, id: &str) {
        let path = {
            let mut inner = self.inner.lock().expect("mic state lock");
            let path = inner.journal.find(id).map(|session| session.path.clone());
            let _ = inner.journal.remove(id);
            inner.recovered.retain(|take| take.id != id);
            path
        };
        if let Some(path) = path {
            let _ = fs::remove_file(path);
        }
    }

    pub fn status(&self) -> MicStatus {
        self.reconcile();
        let mut inner = self.inner.lock().expect("mic state lock");
        if inner.machine.phase() == MicPhase::Recording {
            let elapsed = inner
                .started
                .map(|started| started.elapsed().as_millis() as u64)
                .unwrap_or(0);
            let id = inner
                .session
                .as_ref()
                .map(|session| session.id.clone())
                .unwrap_or_default();
            let _ = inner.journal.note_elapsed(&id, elapsed);
            if let Some(session) = inner.session.as_ref() {
                inner.bytes = fs::metadata(&session.path)
                    .map(|meta| meta.len())
                    .unwrap_or(inner.bytes);
            }
        }
        inner.status()
    }

    /// If the capture thread ended while the machine still believes it is
    /// recording (stream error, OS death), settle it as a stream error. This is
    /// the "never a silent continuation" rule at the host level.
    fn reconcile(&self) {
        let ended = {
            let inner = self.inner.lock().expect("mic state lock");
            inner.machine.phase() == MicPhase::Recording
                && inner
                    .capture
                    .as_ref()
                    .map(CaptureThread::is_finished)
                    .unwrap_or(true)
        };
        if ended {
            let _ = self.stop(StopReason::StreamError);
        }
    }

    // -- JNI-facing protocol ------------------------------------------------

    pub fn dispatch(&self, request: &str) -> String {
        let Ok(value) = serde_json::from_str::<Value>(request) else {
            return json!({ "ok": false, "error": "invalid JSON request" }).to_string();
        };
        let op = value.get("op").and_then(Value::as_str).unwrap_or_default();
        let text = |name: &str| value.get(name).and_then(Value::as_str).unwrap_or_default();
        let flag = |name: &str| value.get(name).and_then(Value::as_bool).unwrap_or(false);
        let with_ok = |status: MicStatus| {
            let mut reply = serde_json::to_value(&status).unwrap_or_else(|_| json!({}));
            reply["ok"] = json!(true);
            reply.to_string()
        };
        match op {
            "status" => with_ok(self.status()),
            "start" => match self.start(
                flag("foreground"),
                flag("permission"),
                text("title"),
                text("model"),
                text("language"),
            ) {
                Ok(status) => with_ok(status),
                Err(reason) => json!({ "ok": false, "refused": reason }).to_string(),
            },
            "stop" => {
                let reason = StopReason::parse(text("reason")).unwrap_or(StopReason::User);
                match self.stop(reason) {
                    Ok(status) => with_ok(status),
                    Err(reason) => json!({ "ok": false, "error": reason }).to_string(),
                }
            }
            "note" => {
                self.note(text("text"));
                json!({ "ok": true }).to_string()
            }
            "quit" => json!({ "ok": true, "quit": self.quit() }).to_string(),
            "recover" => {
                let takes = self.recover();
                json!({ "ok": true, "takes": takes }).to_string()
            }
            "discard" => {
                self.discard(text("id"));
                json!({ "ok": true }).to_string()
            }
            "stop_reason" => {
                let parsed = StopReason::parse(text("reason"));
                json!({ "ok": true, "known": parsed.is_some(), "reason": parsed.map(StopReason::as_str) })
                    .to_string()
            }
            "version" => json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }).to_string(),
            other => json!({ "ok": false, "error": format!("unknown op '{other}'") }).to_string(),
        }
    }
}

/// Replays open takes after a process death and adopts everything the inbox has
/// not seen yet. Returns the recovery list the UI shows.
fn recover_sessions(journal: &mut MicJournal, inbox: &InboxFn) -> Vec<RecoveredTake> {
    let replayed = journal.replay_open().unwrap_or_default();
    let mut recovered = Vec::new();
    let candidates: Vec<MicSession> = journal
        .sessions()
        .iter()
        // Only this launch's replays and takes still waiting for the inbox are
        // surfaced/retried; an old, already-adopted take is not a new notice.
        .filter(|session| {
            replayed.contains(&session.id) || (!session.handed_off && session.frames > 0)
        })
        .cloned()
        .collect();
    for session in candidates {
        let mut entry = session.clone();
        if entry.state == SessionState::Abrupt {
            match capture_stream::repair(&entry.path) {
                Ok(wav) => {
                    entry.frames = wav.frames;
                    entry.bytes = wav.bytes;
                    if wav.frames == 0 {
                        entry.note = format!("{}. The file held no audio.", entry.note);
                    } else {
                        if !entry.handed_off {
                            match inbox(
                                &entry.id,
                                &entry.title,
                                &entry.source_name,
                                &entry.path,
                                &entry.model,
                                &entry.language,
                            ) {
                                Ok(()) => entry.handed_off = true,
                                Err(err) => {
                                    entry.note =
                                        format!("{} It could not be queued yet: {err}", entry.note);
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    entry.note = format!("{} The saved file could not be read: {err}", entry.note);
                    entry.frames = 0;
                }
            }
            // Persist the repaired sizes/hand-off so a second launch does not
            // repeat the work.
            if let Some(stored) = journal.find_mut(&entry.id) {
                stored.frames = entry.frames;
                stored.bytes = entry.bytes;
                stored.handed_off = entry.handed_off;
                stored.note = entry.note.clone();
            }
            let _ = journal.save();
        } else if !entry.handed_off
            && entry.frames > 0
            && inbox(
                &entry.id,
                &entry.title,
                &entry.source_name,
                &entry.path,
                &entry.model,
                &entry.language,
            )
            .is_ok()
        {
            entry.handed_off = true;
            let _ = journal.mark_handed_off(&entry.id);
        }
        recovered.push(RecoveredTake {
            id: entry.id,
            title: entry.title,
            path: entry.path.display().to_string(),
            reason: entry
                .stop_reason
                .map(StopReason::as_str)
                .unwrap_or("unknown")
                .to_string(),
            elapsed_ms: entry.elapsed_ms,
            frames: entry.frames,
            bytes: entry.bytes,
            handed_off: entry.handed_off,
            note: entry.note,
        });
    }
    recovered
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn new_key() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:04x}", nanos, COUNTER.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording() -> MicMachine {
        let mut machine = MicMachine::new();
        assert_eq!(machine.on(MicEvent::Start), MicAction::OpenCapture);
        assert_eq!(machine.on(MicEvent::Ready), MicAction::None);
        assert_eq!(machine.phase(), MicPhase::Recording);
        machine
    }

    /// The plan's three interruption kinds all close the capture — and a second
    /// reason for the same event cannot rewrite the first.
    #[test]
    fn every_interruption_closes_the_capture_once_with_the_first_reason() {
        for reason in [
            StopReason::Call,
            StopReason::AudioFocusLoss,
            StopReason::BluetoothDisconnect,
        ] {
            let mut machine = recording();
            assert_eq!(
                machine.on(MicEvent::Stop(reason)),
                MicAction::CloseCapture(reason)
            );
            assert_eq!(machine.phase(), MicPhase::Stopping);
            // A second listener firing for the same event is a no-op.
            assert_eq!(
                machine.on(MicEvent::Stop(StopReason::AudioFocusLoss)),
                MicAction::None
            );
            assert_eq!(
                machine.on(MicEvent::Finished),
                MicAction::Settle(reason),
                "the first reason must win"
            );
            assert_eq!(machine.phase(), MicPhase::Finished);
            assert_eq!(machine.stop_reason(), Some(reason));
        }
    }

    /// A stop while the microphone is still opening is not lost: it closes as
    /// soon as the session reports ready.
    #[test]
    fn a_stop_before_ready_closes_the_moment_the_session_opens() {
        let mut machine = MicMachine::new();
        assert_eq!(machine.on(MicEvent::Start), MicAction::OpenCapture);
        assert_eq!(
            machine.on(MicEvent::Stop(StopReason::Call)),
            MicAction::None
        );
        assert_eq!(
            machine.on(MicEvent::Ready),
            MicAction::CloseCapture(StopReason::Call)
        );
        assert_eq!(
            machine.on(MicEvent::Finished),
            MicAction::Settle(StopReason::Call)
        );
    }

    /// A capture thread that ends while the machine still thinks it records is
    /// a stream error, never a silent continuation.
    #[test]
    fn a_capture_that_ends_while_recording_settles_as_a_stream_error() {
        let mut machine = recording();
        assert_eq!(
            machine.on(MicEvent::Finished),
            MicAction::Settle(StopReason::StreamError)
        );
        assert_eq!(machine.phase(), MicPhase::Finished);
    }

    /// Idle stops and late events cannot resurrect a session.
    #[test]
    fn idle_and_late_events_are_no_ops() {
        let mut machine = MicMachine::new();
        assert_eq!(
            machine.on(MicEvent::Stop(StopReason::User)),
            MicAction::None
        );
        assert_eq!(machine.on(MicEvent::Finished), MicAction::None);
        let mut machine = recording();
        let _ = machine.on(MicEvent::Stop(StopReason::User));
        let _ = machine.on(MicEvent::Finished);
        assert_eq!(machine.on(MicEvent::Ready), MicAction::None);
        assert_eq!(machine.phase(), MicPhase::Finished);
        // A finished machine can start again (the next take).
        assert_eq!(
            machine.on(MicEvent::Start),
            MicAction::OpenCapture,
            "a second take must be possible"
        );
    }

    /// A duplicate Start does not open a second stream.
    #[test]
    fn a_second_start_is_ignored_while_live() {
        let mut machine = MicMachine::new();
        assert_eq!(machine.on(MicEvent::Start), MicAction::OpenCapture);
        assert_eq!(machine.on(MicEvent::Start), MicAction::None);
        assert_eq!(machine.on(MicEvent::Ready), MicAction::None);
        assert_eq!(machine.on(MicEvent::Start), MicAction::None);
        assert_eq!(machine.phase(), MicPhase::Recording);
    }

    fn session(id: &str, state: SessionState) -> MicSession {
        MicSession {
            id: id.to_string(),
            title: "Recording test".to_string(),
            source_name: "Recording test.wav".to_string(),
            path: PathBuf::from(format!("/tmp/{id}.wav")),
            model: "tiny".to_string(),
            language: "auto".to_string(),
            state,
            stop_reason: None,
            started_unix_ms: 1,
            elapsed_ms: 0,
            device_rate: 48_000,
            device_name: "Synthetic AAudio".to_string(),
            frames: 0,
            bytes: 0,
            handed_off: false,
            note: String::new(),
        }
    }

    fn journal_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("suramak-mic-journal-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The journal round-trips, and a live state on disk replays as an abrupt
    /// process kill.
    #[test]
    fn a_live_journal_entry_replays_as_process_killed() {
        let dir = journal_dir("replay");
        let path = dir.join("mic").join("journal.json");
        let mut journal = MicJournal::load(&path);
        journal
            .open(session("open", SessionState::Recording))
            .unwrap();
        journal
            .open(session("done", SessionState::Stopped))
            .unwrap();

        let mut loaded = MicJournal::load(&path);
        let replayed = loaded.replay_open().unwrap();
        assert_eq!(replayed, vec!["open".to_string()]);
        let entry = loaded.find("open").unwrap();
        assert_eq!(entry.state, SessionState::Abrupt);
        assert_eq!(entry.stop_reason, Some(StopReason::ProcessKilled));
        assert!(entry.note.contains("process ended"));
        assert_eq!(
            loaded.find("done").unwrap().state,
            SessionState::Stopped,
            "a closed take is not replayed"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Elapsed journaling is bounded: the service polls every second, the
    /// journal writes every five.
    #[test]
    fn elapsed_journal_writes_follow_the_bounded_step() {
        let dir = journal_dir("elapsed");
        let path = dir.join("mic").join("journal.json");
        let mut journal = MicJournal::load(&path);
        journal.open(session("s", SessionState::Recording)).unwrap();
        let before = journal.writes();
        assert!(!journal.note_elapsed("s", 1_000).unwrap());
        assert!(!journal.note_elapsed("s", 4_999).unwrap());
        assert_eq!(journal.writes(), before);
        assert!(journal.note_elapsed("s", 5_001).unwrap());
        assert_eq!(journal.writes(), before + 1);
        assert_eq!(journal.find("s").unwrap().elapsed_ms, 5_001);
        // A closed take never accepts elapsed writes again.
        journal
            .close(
                "s",
                SessionState::Stopped,
                StopReason::User,
                6_000,
                None,
                false,
                "done",
            )
            .unwrap();
        assert!(!journal.note_elapsed("s", 20_000).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    /// A torn journal is moved aside instead of failing the launch.
    #[test]
    fn a_torn_mic_journal_is_set_aside() {
        let dir = journal_dir("torn");
        let path = dir.join("mic").join("journal.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{not json").unwrap();
        let journal = MicJournal::load(&path);
        assert!(journal.sessions().is_empty());
        assert!(!path.exists());
        assert!(dir.join("mic").join("journal.json.corrupt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// The bridge's spellings parse, including the notification/test aliases.
    #[test]
    fn stop_reasons_parse_their_bridge_spellings() {
        assert_eq!(StopReason::parse("call"), Some(StopReason::Call));
        assert_eq!(
            StopReason::parse("audio_focus_loss"),
            Some(StopReason::AudioFocusLoss)
        );
        assert_eq!(
            StopReason::parse("focus_loss"),
            Some(StopReason::AudioFocusLoss)
        );
        assert_eq!(
            StopReason::parse("bluetooth_disconnect"),
            Some(StopReason::BluetoothDisconnect)
        );
        assert_eq!(
            StopReason::parse("process_killed"),
            Some(StopReason::ProcessKilled)
        );
        assert_eq!(StopReason::parse("nonsense"), None);
        assert_eq!(StopReason::Call.as_str(), "call");
        assert!(StopReason::Call.is_interruption());
        assert!(!StopReason::User.is_interruption());
    }
}
