//! M13e — import and background transcription behind the Android service.
//!
//! The shape mirrors [`crate::model_host`]: Kotlin owns the platform objects
//! (the SAF descriptor, the `Service`, the notification, the wake lock), Rust
//! owns the state machine, the queue and the library write. The two meet in one
//! JSON command ([`JobHost::dispatch`]).
//!
//! ## The loop this module implements
//!
//! ```text
//! share / SAF pick
//!   → begin_import   (journal written BEFORE any byte is copied)
//!   → copy           (Kotlin reads the fd; progress coalesced here)
//!   → import_done    (size verified, journal marked copy_complete)
//!   → queue           (M16.1 `QueueDriver`: one inference slot, ≤2 decodes ahead)
//!   → decode          (`FileDecoder` → `suramak_core::decode` — the M15.2
//!                      shared path, never a platform decoder)
//!   → transcribe      (whisper via the existing worker)
//!   → Store::insert_transcript  (atomic; the only write of transcript text)
//!   → journal entry removed
//! ```
//!
//! ## What survives a process kill
//!
//! Exactly one file: `files/jobs/journal.json`, rewritten atomically on every
//! phase change (and at most every [`PROGRESS_STEP_BYTES`] of copy progress, and
//! every 5 % of transcription). It holds the **source name, the copied file, the
//! model id, the language, the phase and the last percent** — and no transcript
//! text of any kind: the text is only ever written to the library, in one
//! transaction, after the engine finished (the M16.1 rule: a partial result is
//! never persisted). A job found in the journal at launch is offered as
//! *resume* (the copy completed) or *discard* (the copy was cut short — its
//! bytes cannot be re-read, the SAF grant is gone).
//!
//! Resume restarts the decode; the percent is kept for the prompt, not as
//! resumable audio state (a partial decode buffer is never stored).
//!
//! ## Cancellation
//!
//! `cancel` is the user's intent: the cooperative token stops the decode inside
//! the packet loop / the engine at its next stage boundary, the copied media is
//! deleted and no library row is written. `pause` is the system's intent
//! (`onTimeout`, `onStopJob`): same cooperative stop, but the copy and the
//! journal stay so the next launch can resume.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use suramak_asr::queue::driver::InferenceBackend;
use suramak_asr::queue::driver::Prefetcher;
use suramak_asr::{FileDecoder, JobOptions, QueueDriver, QueueEvent, ThreadedPrefetch, Worker};
use suramak_core::models;
use suramak_core::settings::{EngineSetting, LangMode};
use suramak_core::store::Store;
use suramak_core::transcript::{Source, Transcript};

/// Copy progress is forwarded to the host at most once per this many bytes —
/// the "bounded progress channel": a 500 MB import produces ~2000 events, not
/// one per read() call, and the journal is written no more often than that.
pub const PROGRESS_STEP_BYTES: u64 = 256 * 1024;

/// Transcription progress is journaled at 5 % steps (a notification may update
/// more often; the journal exists for recovery, not for animation).
const PROGRESS_STEP_PERCENT: u8 = 5;

/// The journal's on-disk schema.
pub const JOURNAL_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Platform facts
// ---------------------------------------------------------------------------

/// The foreground-service type a transcription job must declare, per API level.
///
/// The plan's rule, encoded once so Kotlin cannot drift from it: `mediaProcessing`
/// from API 35 (Android 15 added the type), `dataSync` on 34, no type below
/// (a `startForeground` without a type is the pre-34 shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForegroundType {
    MediaProcessing,
    DataSync,
    Untyped,
}

impl ForegroundType {
    /// The name the bridge reports and the manifest comment uses.
    pub fn name(self) -> &'static str {
        match self {
            ForegroundType::MediaProcessing => "mediaProcessing",
            ForegroundType::DataSync => "dataSync",
            ForegroundType::Untyped => "untyped",
        }
    }

    /// The `FOREGROUND_SERVICE_*` permission this type requires, if any.
    pub fn permission(self) -> Option<&'static str> {
        match self {
            ForegroundType::MediaProcessing => {
                Some("android.permission.FOREGROUND_SERVICE_MEDIA_PROCESSING")
            }
            ForegroundType::DataSync => Some("android.permission.FOREGROUND_SERVICE_DATA_SYNC"),
            ForegroundType::Untyped => None,
        }
    }
}

/// The type for a `Build.VERSION.SDK_INT`. Pure, and the only place the rule
/// lives (Kotlin maps the name to the `ServiceInfo` constant).
pub fn foreground_type_for_api(api: i32) -> ForegroundType {
    if api >= 35 {
        ForegroundType::MediaProcessing
    } else if api >= 34 {
        ForegroundType::DataSync
    } else {
        ForegroundType::Untyped
    }
}

// ---------------------------------------------------------------------------
// Media naming
// ---------------------------------------------------------------------------

/// Extensions the shared decoder probes by content anyway; the extension is a
/// hint, but it is the hint the platform's own pickers and players use.
const KNOWN_EXTENSIONS: &[&str] = &[
    "wav", "mp3", "flac", "ogg", "oga", "opus", "m4a", "mp4", "mov", "m4v", "mkv", "webm", "3gp",
    "aac",
];

/// The extension for a copied import: the display name's own extension when it
/// is one the decode path knows, otherwise the one the mime type implies.
pub fn media_extension(name: &str, mime: &str) -> String {
    if let Some(ext) = Path::new(name).extension().and_then(|e| e.to_str()) {
        let lower = ext.to_ascii_lowercase();
        if KNOWN_EXTENSIONS.contains(&lower.as_str()) {
            return lower;
        }
    }
    let base = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "audio/ogg" | "application/ogg" | "application/x-ogg" | "audio/vorbis" => "ogg",
        "audio/opus" | "audio/x-opus" => "opus",
        "audio/webm" | "video/webm" => "webm",
        "audio/mp4" | "video/mp4" => "mp4",
        "audio/x-m4a" | "audio/m4a" => "m4a",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/flac" | "audio/x-flac" => "flac",
        "video/x-matroska" => "mkv",
        "video/quicktime" => "mov",
        "audio/3gpp" | "video/3gpp" => "3gp",
        _ => "audio",
    }
    .to_string()
}

/// Whether an incoming mime type is one this app accepts. The manifest's share
/// matrix is the platform-level copy of this rule; SAF itself filters the picker
/// with `audio/*` + `video/*`, so this is a defence against a malformed intent.
pub fn is_supported_mime(mime: &str) -> bool {
    let base = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    base.starts_with("audio/")
        || base.starts_with("video/")
        || matches!(base.as_str(), "application/ogg" | "application/x-ogg")
}

/// The library title for an import: the display name without its extension.
pub fn title_for(name: &str) -> String {
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.chars().any(|c| c != '.'));
    match stem {
        Some(stem) => stem.to_string(),
        None => "Voice note".to_string(),
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// One job's lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// The bytes are being copied out of the SAF/share descriptor.
    Copying,
    /// Copied and waiting for the inference slot.
    Queued,
    /// Holds the slot, decoding the container.
    Decoding,
    /// Holds the slot, in the engine.
    Transcribing,
    /// Stopped by the system (`onTimeout` / `onStopJob`); the copy is kept and
    /// the next launch offers to resume.
    Paused,
    Done,
    Failed,
    Cancelled,
}

impl Phase {
    /// Whether this job is work that is still going to move on its own.
    pub fn is_live(self) -> bool {
        matches!(
            self,
            Phase::Copying | Phase::Queued | Phase::Decoding | Phase::Transcribing
        )
    }

    /// Whether the job is terminal *and* its media is not going to be used
    /// again by this process.
    pub fn is_terminal(self) -> bool {
        matches!(self, Phase::Done | Phase::Failed | Phase::Cancelled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Copying => "copying",
            Phase::Queued => "queued",
            Phase::Decoding => "decoding",
            Phase::Transcribing => "transcribing",
            Phase::Paused => "paused",
            Phase::Done => "done",
            Phase::Failed => "failed",
            Phase::Cancelled => "cancelled",
        }
    }
}

/// One import/transcription job. Serialized into the journal *without* the
/// transient fields (queue ids, the transcript id, free-text messages) — the
/// journal is state, not a transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub key: String,
    pub title: String,
    /// The name the sharing app reported (kept for `Source::File`).
    pub source_name: String,
    pub media_path: PathBuf,
    pub model: String,
    /// `"auto"` or a language code.
    pub language: String,
    pub phase: Phase,
    /// 0–100 of the current file.
    pub percent: u8,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Set only after `import_done` verified the copy; an interrupted copy is
    /// never resumable (the SAF grant died with the process).
    pub copy_complete: bool,
    #[serde(skip)]
    pub queue_id: Option<u64>,
    #[serde(skip)]
    pub transcript_id: Option<String>,
    #[serde(skip)]
    pub message: String,
    /// `true` when the stop is a system pause rather than a user cancel.
    #[serde(skip)]
    pub pausing: bool,
    #[serde(skip)]
    last_journal_pct: u8,
    #[serde(skip)]
    last_journal_bytes: u64,
}

impl JobRecord {
    fn new(
        key: String,
        title: String,
        source_name: String,
        media_path: PathBuf,
        model: String,
        language: String,
        bytes_total: u64,
    ) -> Self {
        Self {
            key,
            title,
            source_name,
            media_path,
            model,
            language,
            phase: Phase::Copying,
            percent: 0,
            bytes_done: 0,
            bytes_total,
            copy_complete: false,
            queue_id: None,
            transcript_id: None,
            message: String::new(),
            pausing: false,
            last_journal_pct: 0,
            last_journal_bytes: 0,
        }
    }
}

/// The completed job the notification's tap target names.
#[derive(Debug, Clone, Serialize)]
pub struct Completed {
    pub transcript_id: String,
    pub title: String,
    pub segments: usize,
}

/// A snapshot for the service's notification poll and the Activity's prompt.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub running: bool,
    pub current_key: Option<String>,
    pub current_title: String,
    pub current_phase: String,
    pub percent: u8,
    pub waiting: usize,
    pub paused: usize,
    pub done: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub total: usize,
    pub message: String,
    pub last: Option<Completed>,
}

impl Status {
    /// The notification's two numbers: the current file's percent and how many
    /// jobs are still waiting behind it.
    pub fn notification_numbers(&self) -> (u8, usize) {
        (self.percent, self.waiting)
    }
}

/// One entry of the launch-time recovery prompt.
#[derive(Debug, Clone, Serialize)]
pub struct RecoveryEntry {
    pub key: String,
    pub title: String,
    pub model: String,
    pub percent: u8,
    /// `true` when `resume` can work: the copy finished and the file is still
    /// there.
    pub resumable: bool,
    pub reason: String,
}

/// Where the host writes. Derived from the app's `filesDir` in production;
/// explicit in tests.
#[derive(Debug, Clone)]
pub struct HostDirs {
    pub media_dir: PathBuf,
    pub models_dir: PathBuf,
    pub db_path: PathBuf,
    pub journal_path: PathBuf,
}

impl HostDirs {
    /// `<root>/media`, `<root>/models`, `<root>/suramak.db`, `<root>/jobs/journal.json`.
    pub fn under(root: &Path) -> Self {
        Self {
            media_dir: root.join("media"),
            models_dir: root.join("models"),
            db_path: root.join("suramak.db"),
            journal_path: root.join("jobs").join("journal.json"),
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

// ---------------------------------------------------------------------------
// The host
// ---------------------------------------------------------------------------

struct InnerState {
    jobs: Vec<JobRecord>,
    /// Finished jobs stay in memory (so `status` can report them) but never in
    /// the journal.
    done: usize,
    failed: usize,
    cancelled: usize,
    total: usize,
    message: String,
    last: Option<Completed>,
}

impl InnerState {
    fn new() -> Self {
        Self {
            jobs: Vec::new(),
            done: 0,
            failed: 0,
            cancelled: 0,
            total: 0,
            message: String::new(),
            last: None,
        }
    }

    fn job_mut(&mut self, key: &str) -> Option<&mut JobRecord> {
        self.jobs.iter_mut().find(|job| job.key == key)
    }

    fn job(&self, key: &str) -> Option<&JobRecord> {
        self.jobs.iter().find(|job| job.key == key)
    }

    fn by_queue(&self, id: u64) -> Option<&JobRecord> {
        self.jobs.iter().find(|job| job.queue_id == Some(id))
    }

    fn by_queue_mut(&mut self, id: u64) -> Option<&mut JobRecord> {
        self.jobs.iter_mut().find(|job| job.queue_id == Some(id))
    }

    /// Keeps the newest 16 terminal records and drops older ones. A service can
    /// run for days; the counter fields (done/failed/cancelled) keep the batch
    /// totals even after the per-job record is gone.
    fn prune(&mut self) {
        const KEEP: usize = 16;
        let terminal = self
            .jobs
            .iter()
            .filter(|job| job.phase.is_terminal())
            .count();
        if terminal <= KEEP {
            return;
        }
        let mut excess = terminal - KEEP;
        self.jobs.retain(|job| {
            if job.phase.is_terminal() && excess > 0 {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }

    fn status(&self) -> Status {
        let current = self
            .jobs
            .iter()
            .find(|job| matches!(job.phase, Phase::Decoding | Phase::Transcribing))
            .or_else(|| self.jobs.iter().find(|job| job.phase.is_live()));
        let waiting = self
            .jobs
            .iter()
            .filter(|job| matches!(job.phase, Phase::Queued))
            .count();
        let paused = self
            .jobs
            .iter()
            .filter(|job| job.phase == Phase::Paused)
            .count();
        Status {
            running: current.is_some(),
            current_key: current.map(|job| job.key.clone()),
            current_title: current.map(|job| job.title.clone()).unwrap_or_default(),
            current_phase: current
                .map(|job| job.phase.as_str())
                .unwrap_or("idle")
                .to_string(),
            percent: current.map(|job| job.percent).unwrap_or(0),
            waiting,
            paused,
            done: self.done,
            failed: self.failed,
            cancelled: self.cancelled,
            total: self.total,
            message: self.message.clone(),
            last: self.last.clone(),
        }
    }
}

struct Shared {
    dirs: HostDirs,
    state: Mutex<InnerState>,
    /// Journal writes so far (a diagnostic the tests assert on).
    writes: AtomicU64,
}

impl Shared {
    /// Rewrites the journal from the live jobs. Called with the state lock held
    /// so the file can never describe a state the process has not reached.
    fn persist(&self, state: &InnerState) {
        let live: Vec<&JobRecord> = state
            .jobs
            .iter()
            .filter(|job| !job.phase.is_terminal())
            .collect();
        let path = &self.dirs.journal_path;
        if live.is_empty() {
            // No live work: remove the file instead of leaving an empty journal
            // (a launch must not show a prompt for nothing).
            let _ = fs::remove_file(path);
            self.writes.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let journal = json!({ "version": JOURNAL_VERSION, "jobs": live });
        if let Ok(bytes) = serde_json::to_vec_pretty(&journal) {
            if write_atomic(path, &bytes).is_ok() {
                self.writes.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn status(&self) -> Status {
        self.state.lock().expect("job state lock").status()
    }
}

/// Atomic file replace: `write tmp → fsync → rename → fsync dir`.
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

fn load_journal(path: &Path) -> Vec<JobRecord> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let parsed: Result<Value, _> = serde_json::from_slice(&bytes);
    let Ok(value) = parsed else {
        // A torn journal is not an error the app can act on: keep it for
        // forensics and start clean rather than failing the launch.
        let _ = fs::rename(path, path.with_extension("json.corrupt"));
        return Vec::new();
    };
    if value.get("version").and_then(Value::as_u64) != Some(u64::from(JOURNAL_VERSION)) {
        return Vec::new();
    }
    let Some(entries) = value.get("jobs").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| serde_json::from_value::<JobRecord>(entry.clone()).ok())
        .map(|mut job| {
            // Anything live on disk was cut by process death: it is a pause now,
            // not a run this process owns.
            if job.phase.is_live() {
                job.phase = Phase::Paused;
            }
            job.queue_id = None;
            job
        })
        .collect()
}

enum Command {
    Accept(String),
    Cancel(String),
    Pause(String),
    Shutdown,
}

/// The process-wide host behind the JNI surface.
pub type AndroidJobHost = JobHost<Worker>;

/// The import + transcription host.
///
/// Generic over the inference backend and the prefetcher so the concurrency
/// rules can be pinned without a model (a fake backend) and without a race (a
/// manual prefetcher); production is [`AndroidJobHost`], which is exactly
/// `JobHost<Worker, ThreadedPrefetch>` with the real decoder.
pub struct JobHost<
    B: InferenceBackend + Send + 'static,
    P: Prefetcher + Send + 'static = ThreadedPrefetch,
> {
    shared: Arc<Shared>,
    tx: Sender<Command>,
    worker: Mutex<Option<JoinHandle<()>>>,
    _marker: std::marker::PhantomData<fn() -> (B, P)>,
}

impl JobHost<Worker> {
    /// The production host: a real whisper worker and the shared file decoder.
    pub fn production(dirs: HostDirs) -> Self {
        Self::with_prefetcher(
            Worker::spawn(),
            ThreadedPrefetch::new(Box::new(FileDecoder)),
            dirs,
        )
    }
}

impl<B: InferenceBackend + Send + 'static, P: Prefetcher + Send + 'static> JobHost<B, P> {
    /// Builds a host around an explicit prefetcher (the production constructor
    /// is [`JobHost::production`]).
    pub fn with_prefetcher(backend: B, prefetcher: P, dirs: HostDirs) -> Self {
        let _ = dirs.ensure();
        let recovered = load_journal(&dirs.journal_path);
        let mut state = InnerState::new();
        state.jobs = recovered;
        let shared = Arc::new(Shared {
            dirs,
            state: Mutex::new(state),
            writes: AtomicU64::new(0),
        });
        let (tx, rx) = mpsc::channel();
        let thread_shared = shared.clone();
        let handle = thread::Builder::new()
            .name("suramak-android-jobs".to_string())
            .spawn(move || run_worker(thread_shared, rx, Some(backend), prefetcher))
            .expect("failed to spawn the Android job thread");
        Self {
            shared,
            tx,
            worker: Mutex::new(Some(handle)),
            _marker: std::marker::PhantomData,
        }
    }

    fn send(&self, command: Command) -> bool {
        self.tx.send(command).is_ok()
    }

    /// Journal writes so far — a diagnostic, pinned by the progress-bound test.
    pub fn journal_writes(&self) -> u64 {
        self.shared.writes.load(Ordering::Relaxed)
    }

    pub fn dirs(&self) -> &HostDirs {
        &self.shared.dirs
    }

    // -- import ------------------------------------------------------------

    /// Creates a job and its destination path. **The journal is written here,
    /// before a byte is copied**, so a kill during the copy is still visible at
    /// the next launch.
    pub fn begin_import(
        &self,
        name: &str,
        mime: &str,
        bytes_total: u64,
        model_id: &str,
        language: &str,
    ) -> Result<(String, PathBuf), String> {
        if !is_supported_mime(mime) {
            return Err(format!(
                "'{mime}' is not an audio or video type this build imports"
            ));
        }
        let info =
            models::find_model(model_id).ok_or_else(|| format!("unknown model '{model_id}'"))?;
        let model_path = self.shared.dirs.models_dir.join(info.hf_file);
        if !model_path.is_file() {
            return Err(format!(
                "the {model_id} model is not installed on this phone yet"
            ));
        }
        let key = new_key();
        let ext = media_extension(name, mime);
        let media_path = self.shared.dirs.media_dir.join(format!("{key}.{ext}"));

        let mut state = self.shared.state.lock().expect("job state lock");
        state.prune();
        let job = JobRecord::new(
            key.clone(),
            title_for(name),
            name.to_string(),
            media_path.clone(),
            model_id.to_string(),
            language.to_string(),
            bytes_total,
        );
        state.total += 1;
        state.jobs.push(job);
        self.shared.persist(&state);
        Ok((key, media_path))
    }

    /// Bounded copy progress: the host records every call (cheap) but only
    /// rewrites the journal when the byte count moved by at least
    /// [`PROGRESS_STEP_BYTES`] (or the percent did).
    pub fn import_progress(&self, key: &str, bytes_done: u64) {
        let mut state = self.shared.state.lock().expect("job state lock");
        let Some(job) = state.job_mut(key) else {
            return;
        };
        if job.phase != Phase::Copying {
            return;
        }
        job.bytes_done = bytes_done;
        job.percent = percent_of(bytes_done, job.bytes_total);
        // One journal write per PROGRESS_STEP_BYTES of copy: the percent is
        // derived from the byte count, so it cannot add writes of its own.
        if job.bytes_done.saturating_sub(job.last_journal_bytes) < PROGRESS_STEP_BYTES {
            return;
        }
        job.last_journal_bytes = job.bytes_done;
        job.last_journal_pct = job.percent;
        self.shared.persist(&state);
    }

    /// The copy finished: verify it, mark it complete and queue it.
    pub fn import_done(&self, key: &str) -> Result<(), String> {
        let mut state = self.shared.state.lock().expect("job state lock");
        let (ok, message, media) = {
            let Some(job) = state.job_mut(key) else {
                return Err(format!("unknown job '{key}'"));
            };
            if job.phase != Phase::Copying {
                return Err(format!("job '{key}' is not copying"));
            }
            let size = fs::metadata(&job.media_path).map(|m| m.len()).unwrap_or(0);
            if size == 0 {
                job.phase = Phase::Failed;
                job.message = "the imported file was empty".to_string();
                (false, job.message.clone(), job.media_path.clone())
            } else if job.bytes_total > 0 && size != job.bytes_total {
                job.phase = Phase::Failed;
                job.message = format!(
                    "the copied file is {size} bytes but the source declared {}",
                    job.bytes_total
                );
                (false, job.message.clone(), job.media_path.clone())
            } else {
                job.copy_complete = true;
                job.bytes_done = size;
                job.bytes_total = size;
                job.percent = 100;
                job.phase = Phase::Queued;
                (true, String::new(), job.media_path.clone())
            }
        };
        if !ok {
            state.failed += 1;
            self.shared.persist(&state);
            drop(state);
            let _ = fs::remove_file(media);
            return Err(message);
        }
        self.shared.persist(&state);
        drop(state);
        if !self.send(Command::Accept(key.to_string())) {
            return Err("the job thread is gone".to_string());
        }
        Ok(())
    }

    /// The copy failed (I/O, a revoked grant): drop the partial file.
    pub fn import_failed(&self, key: &str, reason: &str) {
        let mut state = self.shared.state.lock().expect("job state lock");
        let Some(job) = state.job_mut(key) else {
            return;
        };
        if job.phase.is_terminal() {
            return;
        }
        if let Err(err) = fs::remove_file(&job.media_path) {
            if err.kind() != io::ErrorKind::NotFound {
                job.message =
                    format!("{reason} (and the partial copy could not be removed: {err})");
            }
        }
        if job.phase != Phase::Failed {
            job.phase = Phase::Failed;
            job.message = reason.to_string();
            state.failed += 1;
        }
        self.shared.persist(&state);
    }

    // -- control -----------------------------------------------------------

    /// The user cancelled: stop, delete the copy, never write a row.
    pub fn cancel(&self, key: &str) {
        {
            let mut state = self.shared.state.lock().expect("job state lock");
            let Some(job) = state.job_mut(key) else {
                return;
            };
            job.pausing = false;
            if job.phase == Phase::Copying || job.phase == Phase::Paused {
                // Nothing is running in the queue for these: finish it here.
                let media = job.media_path.clone();
                job.phase = Phase::Cancelled;
                state.cancelled += 1;
                self.shared.persist(&state);
                let _ = fs::remove_file(media);
                return;
            }
            if job.phase.is_terminal() {
                return;
            }
        }
        let _ = self.send(Command::Cancel(key.to_string()));
    }

    /// The system stopped us (`onTimeout`, `onStopJob`): keep the copy.
    pub fn pause(&self, key: &str) {
        {
            let mut state = self.shared.state.lock().expect("job state lock");
            let Some(job) = state.job_mut(key) else {
                return;
            };
            if job.phase.is_terminal() {
                return;
            }
            job.pausing = true;
            if job.phase == Phase::Copying {
                // A cut-short copy cannot be resumed; it is paused-but-discardable.
                job.phase = Phase::Paused;
                self.shared.persist(&state);
                return;
            }
        }
        let _ = self.send(Command::Pause(key.to_string()));
    }

    /// Pauses everything live — the `onTimeout` path has no key of its own.
    pub fn pause_all(&self) {
        let keys: Vec<String> = {
            let state = self.shared.state.lock().expect("job state lock");
            state
                .jobs
                .iter()
                .filter(|job| job.phase.is_live())
                .map(|job| job.key.clone())
                .collect()
        };
        for key in keys {
            self.pause(&key);
        }
    }

    /// Recovery: resume a journaled job.
    pub fn resume(&self, key: &str) -> Result<(), String> {
        {
            let mut state = self.shared.state.lock().expect("job state lock");
            let Some(job) = state.job_mut(key) else {
                return Err(format!("unknown job '{key}'"));
            };
            if job.phase != Phase::Paused {
                return Err(format!("job '{key}' is not paused"));
            }
            if !job.copy_complete || !job.media_path.is_file() {
                return Err("the imported file was never fully copied".to_string());
            }
            job.phase = Phase::Queued;
            job.percent = 0;
            job.last_journal_pct = 0;
            self.shared.persist(&state);
        }
        if !self.send(Command::Accept(key.to_string())) {
            return Err("the job thread is gone".to_string());
        }
        Ok(())
    }

    /// Recovery: discard a journaled job and its copy.
    pub fn discard(&self, key: &str) {
        let mut state = self.shared.state.lock().expect("job state lock");
        let Some(index) = state.jobs.iter().position(|job| job.key == key) else {
            return;
        };
        let media = state.jobs[index].media_path.clone();
        state.jobs.remove(index);
        self.shared.persist(&state);
        let _ = fs::remove_file(media);
    }

    /// The launch-time prompt's data: every journaled, unfinished job.
    pub fn recover(&self) -> Vec<RecoveryEntry> {
        let state = self.shared.state.lock().expect("job state lock");
        state
            .jobs
            .iter()
            .filter(|job| job.phase == Phase::Paused)
            .map(|job| {
                let present = job.media_path.is_file();
                let resumable = job.copy_complete && present;
                let reason = if resumable {
                    String::new()
                } else if !job.copy_complete {
                    "the import had not finished copying; its bytes cannot be read again"
                        .to_string()
                } else {
                    "the imported file is no longer on the device".to_string()
                };
                RecoveryEntry {
                    key: job.key.clone(),
                    title: job.title.clone(),
                    model: job.model.clone(),
                    percent: job.percent,
                    resumable,
                    reason,
                }
            })
            .collect()
    }

    pub fn status(&self) -> Status {
        self.shared.status()
    }

    /// The completion-notification landing target: the library row, if it
    /// exists. This is the one path M13G's compact library will keep.
    pub fn open_transcript(&self, id: &str) -> Option<Transcript> {
        let store = Store::open(&self.shared.dirs.db_path).ok()?;
        let uuid = uuid_parse(id)?;
        store.get_transcript(uuid).ok()
    }

    // -- JNI-facing protocol ----------------------------------------------

    /// Runs one JSON command; never panics on bad input, always replies with an
    /// object.
    pub fn dispatch(&self, request: &str) -> String {
        let Ok(value) = serde_json::from_str::<Value>(request) else {
            return json!({ "ok": false, "error": "invalid JSON request" }).to_string();
        };
        let op = value.get("op").and_then(Value::as_str).unwrap_or_default();
        let text = |name: &str| value.get(name).and_then(Value::as_str).unwrap_or_default();
        let number = |name: &str| value.get(name).and_then(Value::as_u64).unwrap_or(0);
        match op {
            "begin_import" => match self.begin_import(
                text("name"),
                text("mime"),
                number("bytes_total"),
                text("model"),
                text("language"),
            ) {
                Ok((key, path)) => json!({
                    "ok": true,
                    "key": key,
                    "media_path": path.to_string_lossy(),
                    "title": title_for(text("name")),
                })
                .to_string(),
                Err(reason) => json!({ "ok": false, "refused": reason }).to_string(),
            },
            "import_progress" => {
                self.import_progress(text("key"), number("bytes"));
                json!({ "ok": true }).to_string()
            }
            "import_done" => match self.import_done(text("key")) {
                Ok(()) => json!({ "ok": true }).to_string(),
                Err(reason) => json!({ "ok": false, "error": reason }).to_string(),
            },
            "import_failed" => {
                self.import_failed(text("key"), text("reason"));
                json!({ "ok": true }).to_string()
            }
            "cancel" => {
                self.cancel(text("key"));
                json!({ "ok": true }).to_string()
            }
            "pause" => {
                self.pause(text("key"));
                json!({ "ok": true }).to_string()
            }
            "pause_all" => {
                self.pause_all();
                json!({ "ok": true }).to_string()
            }
            "resume" => match self.resume(text("key")) {
                Ok(()) => json!({ "ok": true }).to_string(),
                Err(reason) => json!({ "ok": false, "error": reason }).to_string(),
            },
            "discard" => {
                self.discard(text("key"));
                json!({ "ok": true }).to_string()
            }
            "recover" => {
                let entries = self.recover();
                json!({ "ok": true, "jobs": entries }).to_string()
            }
            "status" => {
                let status = self.status();
                let mut reply = serde_json::to_value(&status).unwrap_or_else(|_| json!({}));
                reply["ok"] = json!(true);
                reply.to_string()
            }
            "open" => {
                let id = text("transcript_id");
                match self.open_transcript(id) {
                    Some(transcript) => json!({
                        "ok": true,
                        "found": true,
                        "title": transcript.title,
                        "segments": transcript.segments.len(),
                        "model": transcript.model,
                    })
                    .to_string(),
                    None => json!({ "ok": true, "found": false }).to_string(),
                }
            }
            "foreground_type" => {
                let api = value.get("api").and_then(Value::as_i64).unwrap_or(29) as i32;
                let kind = foreground_type_for_api(api);
                json!({
                    "ok": true,
                    "type": kind.name(),
                    "permission": kind.permission(),
                })
                .to_string()
            }
            "version" => json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }).to_string(),
            other => json!({ "ok": false, "error": format!("unknown op '{other}'") }).to_string(),
        }
    }
}

impl<B: InferenceBackend + Send + 'static, P: Prefetcher + Send + 'static> Drop for JobHost<B, P> {
    fn drop(&mut self) {
        let _ = self.send(Command::Shutdown);
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(handle) = worker.take() {
                let _ = handle.join();
            }
        }
    }
}

fn percent_of(done: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((done.min(total) * 100) / total) as u8
}

fn new_key() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:04x}", nanos, COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn uuid_parse(text: &str) -> Option<uuid::Uuid> {
    uuid::Uuid::parse_str(text).ok()
}

// ---------------------------------------------------------------------------
// The worker
// ---------------------------------------------------------------------------

fn run_worker<B: InferenceBackend + Send + 'static, P: Prefetcher + Send + 'static>(
    shared: Arc<Shared>,
    rx: Receiver<Command>,
    backend: Option<B>,
    prefetcher: P,
) {
    let mut driver: Option<QueueDriver<B, P>> = None;
    let mut backend = backend;
    let mut prefetcher = Some(prefetcher);
    let mut store: Option<Store> = None;
    let mut batch_model: Option<String> = None;

    loop {
        // Commands first: a cancel must be applied before the next pump step.
        loop {
            match rx.try_recv() {
                Ok(command) => {
                    if !handle_command(
                        &shared,
                        &mut driver,
                        &mut backend,
                        &mut prefetcher,
                        &mut batch_model,
                        command,
                    ) {
                        return;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        if let Some(active) = driver.as_mut() {
            active.run_once();
            for event in active.drain_events() {
                apply_event(&shared, &mut store, event);
            }
            let idle = active.is_idle();
            if !idle {
                thread::sleep(Duration::from_millis(20));
            }
        }

        let idle = driver.as_ref().map(QueueDriver::is_idle).unwrap_or(true);
        let pending = {
            let state = shared.state.lock().expect("job state lock");
            state.jobs.iter().any(|job| job.phase == Phase::Queued)
        };
        if idle && !pending {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(command) => {
                    if !handle_command(
                        &shared,
                        &mut driver,
                        &mut backend,
                        &mut prefetcher,
                        &mut batch_model,
                        command,
                    ) {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_command<B: InferenceBackend + Send + 'static, P: Prefetcher + Send + 'static>(
    shared: &Arc<Shared>,
    driver: &mut Option<QueueDriver<B, P>>,
    backend: &mut Option<B>,
    prefetcher: &mut Option<P>,
    batch_model: &mut Option<String>,
    command: Command,
) -> bool {
    match command {
        Command::Shutdown => {
            let state = shared.state.lock().expect("job state lock");
            shared.persist(&state);
            false
        }
        Command::Accept(key) => {
            accept(shared, driver, backend, prefetcher, batch_model, &key);
            true
        }
        Command::Cancel(key) => {
            stop_running(shared, driver, &key, false);
            true
        }
        Command::Pause(key) => {
            stop_running(shared, driver, &key, true);
            true
        }
    }
}

/// Moves a `Queued` job into the driver's queue.
#[allow(clippy::too_many_arguments)]
fn accept<B: InferenceBackend + Send + 'static, P: Prefetcher + Send + 'static>(
    shared: &Arc<Shared>,
    driver: &mut Option<QueueDriver<B, P>>,
    backend: &mut Option<B>,
    prefetcher: &mut Option<P>,
    batch_model: &mut Option<String>,
    key: &str,
) {
    let (path, model, language) = {
        let state = shared.state.lock().expect("job state lock");
        let Some(job) = state.job(key) else {
            return;
        };
        if job.phase != Phase::Queued {
            return;
        }
        (
            job.media_path.clone(),
            job.model.clone(),
            job.language.clone(),
        )
    };

    if batch_model
        .as_deref()
        .is_some_and(|current| current != model)
    {
        fail(shared, key, "a second model cannot join a running batch");
        return;
    }
    if !path.is_file() {
        fail(shared, key, "the imported file is no longer on the device");
        return;
    }

    if driver.is_none() {
        let Some(engine) = backend.take() else {
            fail(shared, key, "the inference worker is not available");
            return;
        };
        let info = match models::find_model(&model) {
            Some(info) => info,
            None => {
                fail(shared, key, "the selected model is unknown");
                return;
            }
        };
        let lang = if language == "auto" || language.is_empty() {
            LangMode::Auto
        } else {
            LangMode::Fixed(language)
        };
        let options = JobOptions {
            model_path: shared.dirs.models_dir.join(info.hf_file),
            lang,
            initial_prompt: None,
            engine: EngineSetting::Auto,
            fast_engine: false,
        };
        *batch_model = Some(model);
        // The prefetcher is created before this thread exists and handed to the
        // driver exactly once, at the first accepted job.
        let Some(prefetcher) = prefetcher.take() else {
            fail(shared, key, "the decode pool is not available");
            return;
        };
        *driver = Some(QueueDriver::new(engine, prefetcher, options));
    }

    let Some(active) = driver.as_mut() else {
        return;
    };
    let queue_id = active.enqueue(path);
    let mut state = shared.state.lock().expect("job state lock");
    if let Some(job) = state.job_mut(key) {
        job.queue_id = Some(queue_id);
        job.phase = Phase::Queued;
        job.percent = 0;
    }
    shared.persist(&state);
}

fn stop_running<B: InferenceBackend + Send + 'static, P: Prefetcher + Send + 'static>(
    shared: &Arc<Shared>,
    driver: &mut Option<QueueDriver<B, P>>,
    key: &str,
    pausing: bool,
) {
    let mut state = shared.state.lock().expect("job state lock");
    let Some(job) = state.job_mut(key) else {
        return;
    };
    if job.phase.is_terminal() {
        return;
    }
    job.pausing = pausing;
    let media = job.media_path.clone();
    if job.phase == Phase::Copying || job.queue_id.is_none() {
        // Not in the queue yet: settle it here. This also covers a job whose
        // `Accept` command has not been processed yet — settling it now means
        // the later `accept()` sees a non-`Queued` phase and refuses to start
        // it, so a Cancel that races ahead of Accept cannot come true anyway.
        job.phase = if pausing {
            Phase::Paused
        } else {
            Phase::Cancelled
        };
        if !pausing {
            state.cancelled += 1;
            shared.persist(&state);
            drop(state);
            let _ = fs::remove_file(media);
            return;
        }
        shared.persist(&state);
        return;
    }
    let queue_id = job.queue_id;
    drop(state);
    if let (Some(active), Some(id)) = (driver.as_mut(), queue_id) {
        active.cancel(id);
    }
}

fn fail(shared: &Arc<Shared>, key: &str, reason: &str) {
    let mut state = shared.state.lock().expect("job state lock");
    if let Some(job) = state.job_mut(key) {
        if job.phase.is_terminal() {
            return;
        }
        job.phase = Phase::Failed;
        job.message = reason.to_string();
        state.failed += 1;
        state.message = reason.to_string();
    }
    shared.persist(&state);
}

fn apply_event(shared: &Arc<Shared>, store: &mut Option<Store>, event: QueueEvent) {
    match event {
        QueueEvent::Started { id, .. } => {
            let mut state = shared.state.lock().expect("job state lock");
            if let Some(job) = state.by_queue_mut(id) {
                if job.phase == Phase::Queued {
                    job.phase = Phase::Decoding;
                }
            }
            shared.persist(&state);
        }
        QueueEvent::Stage { id, stage, .. } => {
            let mut state = shared.state.lock().expect("job state lock");
            if let Some(job) = state.by_queue_mut(id) {
                job.phase = match stage {
                    suramak_asr::JobStage::Decoding => Phase::Decoding,
                    suramak_asr::JobStage::Transcribing => Phase::Transcribing,
                };
            }
            shared.persist(&state);
        }
        QueueEvent::Progress { id, pct, .. } => {
            let mut state = shared.state.lock().expect("job state lock");
            let mut persist = false;
            if let Some(job) = state.by_queue_mut(id) {
                job.phase = Phase::Transcribing;
                job.percent = pct;
                if pct.saturating_sub(job.last_journal_pct) >= PROGRESS_STEP_PERCENT || pct == 100 {
                    job.last_journal_pct = pct;
                    persist = true;
                }
            }
            if persist {
                shared.persist(&state);
            }
        }
        QueueEvent::Finished {
            id,
            transcript,
            words,
            ..
        } => {
            finish(shared, store, id, *transcript, &words);
        }
        QueueEvent::Failed { id, reason, .. } => {
            let mut state = shared.state.lock().expect("job state lock");
            if let Some(job) = state.by_queue_mut(id) {
                job.phase = Phase::Failed;
                job.message = reason.clone();
                let media = job.media_path.clone();
                drop(state);
                let _ = fs::remove_file(media);
                let mut state = shared.state.lock().expect("job state lock");
                state.failed += 1;
                state.message = reason;
                shared.persist(&state);
            }
        }
        QueueEvent::Cancelled { id, .. } => {
            let mut state = shared.state.lock().expect("job state lock");
            if let Some(job) = state.by_queue_mut(id) {
                if job.pausing {
                    job.phase = Phase::Paused;
                    job.message = "stopped by the system; the next launch can resume".to_string();
                    shared.persist(&state);
                } else {
                    job.phase = Phase::Cancelled;
                    let media = job.media_path.clone();
                    drop(state);
                    let _ = fs::remove_file(media);
                    let mut state = shared.state.lock().expect("job state lock");
                    state.cancelled += 1;
                    shared.persist(&state);
                }
            }
        }
    }
}

fn finish(
    shared: &Arc<Shared>,
    store: &mut Option<Store>,
    id: u64,
    mut transcript: Transcript,
    words: &[suramak_core::transcript::WordTiming],
) {
    let (key, title, source_name, media, model) = {
        let state = shared.state.lock().expect("job state lock");
        let Some(job) = state.by_queue(id) else {
            return;
        };
        (
            job.key.clone(),
            job.title.clone(),
            job.source_name.clone(),
            job.media_path.clone(),
            job.model.clone(),
        )
    };
    // The worker's transcript is named after the copied file (`<key>.opus`).
    // The library row must carry what the user shared, not our uuid.
    transcript.title = title.clone();
    transcript.source = Source::File { name: source_name };
    transcript.audio_path = Some(media);
    if transcript.model.is_empty() {
        transcript.model = model;
    }

    if store.is_none() {
        match Store::open(&shared.dirs.db_path) {
            Ok(opened) => *store = Some(opened),
            Err(err) => {
                fail(
                    shared,
                    &key,
                    &format!("the library could not be opened: {err}"),
                );
                return;
            }
        }
    }
    let Some(library) = store.as_ref() else {
        fail(shared, &key, "the library could not be opened");
        return;
    };
    match library.insert_transcript_with_words(&transcript, words) {
        Ok(()) => {
            let mut state = shared.state.lock().expect("job state lock");
            let segments = transcript.segments.len();
            if let Some(job) = state.job_mut(&key) {
                job.phase = Phase::Done;
                job.percent = 100;
                job.transcript_id = Some(transcript.id.to_string());
            }
            state.done += 1;
            state.message = format!("{title} is ready");
            state.last = Some(Completed {
                transcript_id: transcript.id.to_string(),
                title,
                segments,
            });
            shared.persist(&state);
        }
        Err(err) => fail(
            shared,
            &key,
            &format!("the transcript could not be saved: {err}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_types_follow_the_api_matrix() {
        assert_eq!(foreground_type_for_api(29), ForegroundType::Untyped);
        assert_eq!(foreground_type_for_api(33), ForegroundType::Untyped);
        assert_eq!(foreground_type_for_api(34), ForegroundType::DataSync);
        assert_eq!(foreground_type_for_api(35), ForegroundType::MediaProcessing);
        assert_eq!(foreground_type_for_api(36), ForegroundType::MediaProcessing);
        assert_eq!(
            ForegroundType::MediaProcessing.permission(),
            Some("android.permission.FOREGROUND_SERVICE_MEDIA_PROCESSING")
        );
        assert_eq!(
            ForegroundType::DataSync.permission(),
            Some("android.permission.FOREGROUND_SERVICE_DATA_SYNC")
        );
        assert_eq!(ForegroundType::Untyped.permission(), None);
    }

    #[test]
    fn media_extensions_prefer_the_name_then_the_mime() {
        assert_eq!(media_extension("Ptt 2026-11-10.opus", "audio/ogg"), "opus");
        assert_eq!(media_extension("note.OGG", "audio/ogg"), "ogg");
        assert_eq!(media_extension("voice-note.opus", "audio/ogg"), "opus");
        assert_eq!(
            media_extension("no-extension", "audio/ogg; codecs=opus"),
            "ogg"
        );
        assert_eq!(media_extension("clip", "application/ogg"), "ogg");
        assert_eq!(media_extension("clip", "audio/webm"), "webm");
        assert_eq!(media_extension("VID_1", "video/mp4"), "mp4");
        assert_eq!(media_extension("movie", "video/x-matroska"), "mkv");
        assert_eq!(media_extension("whatsapp.bin", "audio/ogg"), "ogg");
        assert_eq!(
            media_extension("thing", "application/octet-stream"),
            "audio"
        );
    }

    #[test]
    fn the_share_matrix_matches_the_decoder() {
        for mime in [
            "audio/ogg",
            "audio/ogg; codecs=opus",
            "audio/mpeg",
            "audio/mp4",
            "audio/webm",
            "audio/opus",
            "application/ogg",
            "application/x-ogg",
            "video/mp4",
            "video/webm",
            "video/x-matroska",
            "video/quicktime",
        ] {
            assert!(is_supported_mime(mime), "{mime} must be accepted");
        }
        assert!(!is_supported_mime("application/octet-stream"));
        assert!(!is_supported_mime("text/plain"));
        assert!(!is_supported_mime("image/png"));
    }

    #[test]
    fn titles_drop_the_extension_and_never_go_empty() {
        assert_eq!(
            title_for("Ptt 2026-11-10 at 10.32.opus"),
            "Ptt 2026-11-10 at 10.32"
        );
        assert_eq!(title_for("  spaced  .ogg"), "spaced");
        assert_eq!(title_for(""), "Voice note");
        assert_eq!(title_for("..."), "Voice note");
    }

    #[test]
    fn a_torn_journal_is_set_aside_instead_of_failing_the_launch() {
        let dir = std::env::temp_dir().join(format!("suramak-journal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("journal.json");
        fs::write(&path, b"{not json").unwrap();
        assert!(load_journal(&path).is_empty());
        assert!(!path.exists(), "the torn journal is moved aside");
        assert!(dir.join("journal.json.corrupt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn journal_round_trip_keeps_state_and_never_a_transcript() {
        let dir = std::env::temp_dir().join(format!("suramak-journal-rt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("journal.json");
        let mut job = JobRecord::new(
            "k1".into(),
            "Voice note".into(),
            "Ptt.opus".into(),
            PathBuf::from("/tmp/k1.opus"),
            "tiny".into(),
            "auto".into(),
            4096,
        );
        job.phase = Phase::Transcribing;
        job.percent = 42;
        job.copy_complete = true;
        let journal = json!({ "version": JOURNAL_VERSION, "jobs": [job] });
        write_atomic(&path, &serde_json::to_vec(&journal).unwrap()).unwrap();
        let loaded = load_journal(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].percent, 42);
        assert_eq!(
            loaded[0].phase,
            Phase::Paused,
            "a live phase on disk is a pause"
        );
        assert!(loaded[0].copy_complete);
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("segments"),
            "the journal must not carry text"
        );
        assert!(!text.contains("\"text\""));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn percent_is_integer_and_bounded() {
        assert_eq!(percent_of(0, 0), 0);
        assert_eq!(percent_of(1, 3), 33);
        assert_eq!(percent_of(3, 3), 100);
        assert_eq!(percent_of(9, 3), 100);
    }
}
