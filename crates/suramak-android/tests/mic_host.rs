//! M13F — host tests for the microphone state machine, journal replay and the
//! record → transcribe hand-off.
//!
//! No microphone and no Android: the capture spawner is synthetic (it streams
//! known samples through the **real** `StreamingWav` writer), and the inbox is
//! either a call log or the **real** M13E `JobHost` with a fake inference
//! backend. That is the same split the production code uses: Kotlin owns the
//! platform, Rust owns the state, and the two meet at the WAV.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use suramak_android::jobs::{HostDirs, JobHost};
use suramak_android::mic::{
    CaptureSpawner, CaptureThread, InboxFn, Meter, MicDirs, MicHost, MicJournal, MicSession,
    SessionState, StartedTake, StopReason,
};
use suramak_asr::capture_stream::{StreamedWav, StreamingWav};
use suramak_asr::{EngineId, FileDecoder, InferenceBackend, Job, ThreadedPrefetch, WorkerEvent};
use suramak_core::store::Store;
use suramak_core::transcript::{Segment, Source, Transcript};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("suramak-mic-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The mic journal, the inbox and the model share one app `filesDir`.
fn dirs(name: &str) -> (PathBuf, MicDirs, HostDirs) {
    let root = root(name);
    let mic = MicDirs::under(&root);
    std::fs::create_dir_all(&mic.models_dir).unwrap();
    std::fs::write(mic.models_dir.join("ggml-tiny.bin"), b"not a real model").unwrap();
    let jobs = HostDirs::under(&root);
    std::fs::create_dir_all(&jobs.media_dir).unwrap();
    (root, mic, jobs)
}

fn sine(rate: u32, seconds: f32, freq: f32) -> Vec<f32> {
    let count = (rate as f32 * seconds) as usize;
    (0..count)
        .map(|index| {
            let t = index as f32 / rate as f32;
            (t * freq * std::f32::consts::TAU).sin() * 0.35
        })
        .collect()
}

/// A capture thread that streams `samples` through the real WAV writer at
/// `rate`, exactly as the AAudio consumer does, and stops when signalled.
fn writer_spawner(samples: Vec<f32>, rate: u32) -> CaptureSpawner {
    Arc::new(move |path: PathBuf, stop: Arc<AtomicBool>| {
        let (ready_tx, ready_rx) = mpsc::channel();
        let error = Arc::new(Mutex::new(None));
        let finished = Arc::new(AtomicBool::new(false));
        let meter = Arc::new(Meter::default());
        let samples = samples.clone();
        let thread_stop = Arc::clone(&stop);
        let thread_finished = Arc::clone(&finished);
        let handle = std::thread::Builder::new()
            .name("synthetic-mic".to_string())
            .spawn(move || {
                let result = (|| -> Result<StreamedWav, String> {
                    let mut wav =
                        StreamingWav::create(&path, rate).map_err(|err| err.to_string())?;
                    // The whole take is pushed before Ready: these tests assert
                    // exact bytes, so the synthetic device delivers a complete
                    // fixture rather than racing the stop flag.
                    for chunk in samples.chunks(4800) {
                        wav.push(chunk).map_err(|err| err.to_string())?;
                    }
                    let _ = ready_tx.send(Ok(StartedTake {
                        device_name: "Synthetic AAudio".to_string(),
                        sample_rate: rate,
                    }));
                    while !thread_stop.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    wav.finish().map_err(|err| err.to_string())
                })();
                thread_finished.store(true, Ordering::Release);
                result
            })
            .unwrap();
        Ok(CaptureThread::from_parts(
            stop, ready_rx, handle, error, finished, meter,
        ))
    })
}

/// A spawner whose capture ends on its own right after opening (the stream-
/// error shape): it writes an empty WAV and returns.
fn ending_spawner() -> CaptureSpawner {
    Arc::new(move |path: PathBuf, stop: Arc<AtomicBool>| {
        let (ready_tx, ready_rx) = mpsc::channel();
        let error = Arc::new(Mutex::new(None));
        let finished = Arc::new(AtomicBool::new(false));
        let meter = Arc::new(Meter::default());
        let thread_finished = Arc::clone(&finished);
        let handle = std::thread::Builder::new()
            .name("ending-mic".to_string())
            .spawn(move || {
                let result = (|| -> Result<StreamedWav, String> {
                    let wav = StreamingWav::create(&path, 16_000).map_err(|e| e.to_string())?;
                    let _ = ready_tx.send(Ok(StartedTake {
                        device_name: "Ending AAudio".to_string(),
                        sample_rate: 16_000,
                    }));
                    wav.finish().map_err(|e| e.to_string())
                })();
                thread_finished.store(true, Ordering::Release);
                result
            })
            .unwrap();
        Ok(CaptureThread::from_parts(
            stop, ready_rx, handle, error, finished, meter,
        ))
    })
}

#[derive(Default)]
struct InboxLog {
    calls: Mutex<Vec<(String, String, PathBuf)>>,
}

fn log_inbox(log: Arc<InboxLog>) -> InboxFn {
    Arc::new(move |key, title, _source, path, _model, _language| {
        log.calls
            .lock()
            .unwrap()
            .push((key.to_string(), title.to_string(), path.to_path_buf()));
        Ok(())
    })
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

/// Compares two files without dumping megabytes into a failure message.
fn assert_same_bytes(left: &[u8], right: &[u8], what: &str) {
    assert_eq!(left.len(), right.len(), "{what}: byte lengths differ");
    if let Some(index) = left.iter().zip(right).position(|(a, b)| a != b) {
        panic!(
            "{what}: first differing byte at {index}: {} vs {}",
            left[index], right[index]
        );
    }
}

fn journal_sessions(root: &Path) -> Vec<serde_json::Value> {
    let path = root.join("mic").join("journal.json");
    let text = std::fs::read_to_string(path).expect("the mic journal");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    value["sessions"].as_array().expect("sessions").clone()
}

// ---------------------------------------------------------------------------
// Fakes for the real M13E JobHost
// ---------------------------------------------------------------------------

struct Control {
    active: AtomicUsize,
    max_active: AtomicUsize,
}

struct Running {
    job: Job,
}

/// Auto-finishing fake inference (the same shape `jobs_host.rs` uses): one
/// finished event with a deterministic segment.
struct FakeBackend {
    control: Arc<Control>,
    current: Mutex<Option<Running>>,
}

impl FakeBackend {
    fn new() -> (Self, Arc<Control>) {
        let control = Arc::new(Control {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        });
        (
            Self {
                control: control.clone(),
                current: Mutex::new(None),
            },
            control,
        )
    }
}

fn fake_transcript(input: &Path) -> Transcript {
    let name = input
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Transcript::new(
        name.clone(),
        Source::File { name },
        "tiny".to_string(),
        Some("en".to_string()),
        Some(input.to_path_buf()),
        vec![Segment {
            idx: 0,
            start_ms: 0,
            end_ms: 1_500,
            text: "selamat pagi, this is the fake engine".to_string(),
        }],
    )
}

impl InferenceBackend for FakeBackend {
    fn submit(&mut self, job: Job) -> Result<(), String> {
        let active = self.control.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.control.max_active.fetch_max(active, Ordering::SeqCst);
        *self.current.lock().unwrap() = Some(Running { job });
        Ok(())
    }

    fn try_recv(&mut self) -> Option<WorkerEvent> {
        let mut guard = self.current.lock().unwrap();
        let running = guard.as_mut()?;
        if running.job.cancel.is_cancelled() {
            let id = running.job.id;
            *guard = None;
            self.control.active.fetch_sub(1, Ordering::SeqCst);
            return Some(WorkerEvent::Cancelled { job_id: id });
        }
        let id = running.job.id;
        let input = running.job.input.clone();
        *guard = None;
        self.control.active.fetch_sub(1, Ordering::SeqCst);
        Some(WorkerEvent::Finished {
            job_id: id,
            transcript: Box::new(fake_transcript(&input)),
            words: Vec::new(),
            engine_used: EngineId::Cpu,
            warnings: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A call mid-record stops cleanly, keeps the audio, notes the kind and hands
/// the exact WAV to the inbox.
#[test]
fn a_call_interruption_stops_cleanly_and_hands_off() {
    let (root, mic_dirs, _) = dirs("call");
    let log = Arc::new(InboxLog::default());
    let host = MicHost::with_parts(
        mic_dirs,
        writer_spawner(sine(48_000, 1.5, 440.0), 48_000),
        log_inbox(Arc::clone(&log)),
    );
    let status = host
        .start(true, true, "Recording call test", "tiny", "auto")
        .expect("start");
    assert!(status.recording);
    assert_eq!(status.phase, "recording");
    assert_eq!(status.input_device, "Synthetic AAudio");
    assert_eq!(status.sample_rate, 48_000);

    std::thread::sleep(Duration::from_millis(30));
    let stopped = host.stop(StopReason::Call).expect("stop");
    assert!(!stopped.recording);
    let last = stopped.last_stop.expect("a stop record");
    assert_eq!(last.reason, "call");
    assert!(last.interruption);
    assert!(last.frames > 0, "the take must have audio");
    assert!(last.handed_off, "a call stop still queues the recording");
    assert_eq!(
        last.frames * 2 + 44,
        std::fs::metadata(&last.path).unwrap().len()
    );

    let calls = log.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, last.id);
    assert_eq!(calls[0].1, "Recording call test");
    drop(calls);

    // The journal names the interruption kind and keeps no transcript text.
    let sessions = journal_sessions(&root);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["state"], "stopped");
    assert_eq!(sessions[0]["stop_reason"], "call");
    assert_eq!(sessions[0]["handed_off"], true);
    assert!(sessions[0]["frames"].as_u64().unwrap() > 0);
    let text = std::fs::read_to_string(root.join("mic").join("journal.json")).unwrap();
    assert!(
        !text.contains("segments"),
        "the mic journal never holds text"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Every interruption kind the plan names closes the stream, keeps the take and
/// journals its own reason — none continues silently.
#[test]
fn every_interruption_kind_stops_and_is_journaled() {
    for (name, reason) in [
        ("focus", StopReason::AudioFocusLoss),
        ("bt", StopReason::BluetoothDisconnect),
    ] {
        let (root, mic_dirs, _) = dirs(name);
        let log = Arc::new(InboxLog::default());
        let host = MicHost::with_parts(
            mic_dirs,
            writer_spawner(sine(48_000, 0.8, 300.0), 48_000),
            log_inbox(Arc::clone(&log)),
        );
        host.start(true, true, "Recording interruption", "tiny", "auto")
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let stopped = host.stop(reason).unwrap();
        let last = stopped.last_stop.unwrap();
        assert_eq!(last.reason, reason.as_str());
        assert!(last.interruption);
        assert!(last.frames > 0);
        assert_eq!(log.calls.lock().unwrap().len(), 1);
        let sessions = journal_sessions(&root);
        assert_eq!(sessions[0]["stop_reason"], reason.as_str());
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// The first reason wins: a call that also takes audio focus is one stop.
#[test]
fn the_first_stop_reason_wins_and_duplicate_stops_are_no_ops() {
    let (root, mic_dirs, _) = dirs("first-reason");
    let log = Arc::new(InboxLog::default());
    let host = MicHost::with_parts(
        mic_dirs,
        writer_spawner(sine(48_000, 0.6, 440.0), 48_000),
        log_inbox(Arc::clone(&log)),
    );
    host.start(true, true, "Recording once", "tiny", "auto")
        .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let first = host.stop(StopReason::Call).unwrap();
    let second = host.stop(StopReason::AudioFocusLoss).unwrap();
    assert_eq!(
        first.last_stop.as_ref().unwrap().reason,
        second.last_stop.as_ref().unwrap().reason
    );
    assert_eq!(second.last_stop.unwrap().reason, "call");
    assert_eq!(log.calls.lock().unwrap().len(), 1, "one hand-off, not two");
    assert_eq!(journal_sessions(&root)[0]["stop_reason"], "call");
    let _ = std::fs::remove_dir_all(&root);
}

/// A process kill during a take: the journal replays it as `process_killed`,
/// the WAV header is repaired, every captured frame survives and the take is
/// adopted once.
#[test]
fn a_killed_take_replays_repairs_and_adopts_once() {
    let (root, mic_dirs, _) = dirs("replay");
    let media = mic_dirs.media_dir.clone();
    std::fs::create_dir_all(&media).unwrap();
    let path = media.join("killed.wav");
    let samples = sine(48_000, 2.0, 220.0);
    {
        let mut wav = StreamingWav::create(&path, 48_000).unwrap();
        for chunk in samples.chunks(3200) {
            wav.push(chunk).unwrap();
        }
        // No finish: the process dies here.
    }
    let raw_len = std::fs::metadata(&path).unwrap().len();
    assert!(raw_len > 44);

    let mut journal = MicJournal::load(&mic_dirs.journal_path);
    journal
        .open(MicSession {
            id: "killed".to_string(),
            title: "Recording killed".to_string(),
            source_name: "Recording killed.wav".to_string(),
            path: path.clone(),
            model: "tiny".to_string(),
            language: "auto".to_string(),
            state: SessionState::Recording,
            stop_reason: None,
            started_unix_ms: 1,
            elapsed_ms: 7_500,
            device_rate: 48_000,
            device_name: "Synthetic AAudio".to_string(),
            frames: 0,
            bytes: 0,
            handed_off: false,
            note: String::new(),
        })
        .unwrap();

    let log = Arc::new(InboxLog::default());
    let spawner: CaptureSpawner =
        Arc::new(|_, _| Err("capture must not open on recovery".to_string()));
    let host = MicHost::with_parts(mic_dirs, spawner, log_inbox(Arc::clone(&log)));
    let status = host.status();
    assert_eq!(status.recovered.len(), 1);
    let take = &status.recovered[0];
    assert_eq!(take.id, "killed");
    assert_eq!(take.reason, "process_killed");
    assert!(take.frames > 0, "the partial take must keep its frames");
    assert!(take.handed_off);
    assert!(take.note.contains("recovered"));
    assert_eq!(log.calls.lock().unwrap().len(), 1);

    // The repaired file is a valid WAV whose data chunk is whole samples.
    let repaired = std::fs::read(&path).unwrap();
    assert_eq!(repaired.len() as u64, raw_len);
    let data = u32::from_le_bytes([repaired[40], repaired[41], repaired[42], repaired[43]]);
    assert_eq!(u64::from(data), take.frames * 2);
    assert_eq!(u64::from(data) + 44, raw_len);

    // A second launch does not adopt the same take again.
    let mic_dirs2 = MicDirs::under(&root);
    let log2 = Arc::new(InboxLog::default());
    let spawner2: CaptureSpawner =
        Arc::new(|_, _| Err("capture must not open on recovery".to_string()));
    let host2 = MicHost::with_parts(mic_dirs2, spawner2, log_inbox(Arc::clone(&log2)));
    assert!(
        host2.status().recovered.is_empty(),
        "nothing left to recover"
    );
    assert!(log2.calls.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

/// Quit stops (and finalizes/hands off) the capture **before** it returns the
/// service/exit steps — the owner's capture-first invariant.
#[test]
fn quit_stops_the_capture_before_the_service_step() {
    let (root, mic_dirs, _) = dirs("quit");
    let log = Arc::new(InboxLog::default());
    let host = MicHost::with_parts(
        mic_dirs,
        writer_spawner(sine(48_000, 1.0, 440.0), 48_000),
        log_inbox(Arc::clone(&log)),
    );
    host.start(true, true, "Recording quit", "tiny", "auto")
        .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let outcome = host.quit();
    assert!(outcome.was_recording);
    assert!(outcome.handoff_key.is_some());
    assert_eq!(outcome.next, vec!["stop_service", "exit"]);
    let status = host.status();
    assert!(!status.recording);
    assert_eq!(status.last_stop.as_ref().unwrap().reason, "user");
    assert!(status.last_stop.as_ref().unwrap().handed_off);
    assert_eq!(log.calls.lock().unwrap().len(), 1);
    let sessions = journal_sessions(&root);
    assert_eq!(sessions[0]["stop_reason"], "user");
    assert!(sessions[0]["bytes"].as_u64().unwrap() > 44);
    let _ = std::fs::remove_dir_all(&root);
}

/// A second start while one is live is refused; a stop when idle writes nothing.
#[test]
fn start_is_refused_while_live_and_idle_stop_is_a_no_op() {
    let (root, mic_dirs, _) = dirs("idempotent");
    let log = Arc::new(InboxLog::default());
    let host = MicHost::with_parts(
        mic_dirs,
        writer_spawner(sine(48_000, 0.5, 440.0), 48_000),
        log_inbox(Arc::clone(&log)),
    );
    // Idle stop: no journal, no inbox, no error.
    let status = host.stop(StopReason::User).unwrap();
    assert!(!status.recording);
    assert!(status.last_stop.is_none());
    assert!(!root.join("mic").join("journal.json").exists());

    host.start(true, true, "Recording once", "tiny", "auto")
        .unwrap();
    let refused = host
        .start(true, true, "Recording twice", "tiny", "auto")
        .expect_err("a second take must be refused");
    assert!(refused.contains("already running"), "{refused}");
    host.stop(StopReason::User).unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

/// The honest denial states: no foreground claim, no microphone permission,
/// no installed model — each with its own sentence and no file/journal.
#[test]
fn start_refusals_name_the_reason_and_write_nothing() {
    let (root, mic_dirs, _) = dirs("refusals");
    let log = Arc::new(InboxLog::default());
    let host = MicHost::with_parts(
        mic_dirs.clone(),
        writer_spawner(vec![], 48_000),
        log_inbox(Arc::clone(&log)),
    );

    let foreground = host
        .start(false, true, "Recording", "tiny", "auto")
        .expect_err("foreground is required");
    assert!(foreground.contains("foreground"), "{foreground}");
    assert!(foreground.contains("Android 14"), "{foreground}");

    let permission = host
        .start(true, false, "Recording", "tiny", "auto")
        .expect_err("permission is required");
    assert!(permission.contains("RECORD_AUDIO"), "{permission}");
    assert!(permission.contains("Settings"), "{permission}");

    let model = host
        .start(true, true, "Recording", "base", "auto")
        .expect_err("a missing model is refused");
    assert!(model.contains("not installed"), "{model}");

    let unknown = host
        .start(true, true, "Recording", "nope", "auto")
        .expect_err("an unknown model is refused");
    assert!(unknown.contains("unknown model"), "{unknown}");

    assert!(!root.join("mic").join("journal.json").exists());
    assert!(log.calls.lock().unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(&mic_dirs.media_dir).unwrap().count(),
        0,
        "a refused start leaves no WAV"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A capture thread that ends on its own becomes a clean `stream_error` stop —
/// never a silent continuation.
#[test]
fn a_capture_that_dies_is_settled_as_a_stream_error() {
    let (root, mic_dirs, _) = dirs("stream-error");
    let log = Arc::new(InboxLog::default());
    let host = MicHost::with_parts(mic_dirs, ending_spawner(), log_inbox(Arc::clone(&log)));
    host.start(true, true, "Recording dies", "tiny", "auto")
        .unwrap();
    // The thread has already finished by the time status reconciles it.
    let status = host.status();
    assert!(!status.recording);
    let last = status.last_stop.expect("a settled stop");
    assert_eq!(last.reason, "stream_error");
    assert_eq!(last.frames, 0);
    assert!(!last.handed_off);
    assert!(
        last.path.is_empty() || !Path::new(&last.path).exists(),
        "an empty take is removed"
    );
    let sessions = journal_sessions(&root);
    assert_eq!(sessions[0]["state"], "failed");
    assert_eq!(sessions[0]["stop_reason"], "stream_error");
    let _ = std::fs::remove_dir_all(&root);
}

/// **The hand-off equivalence:** a recording's WAV is byte-identical to the
/// desktop output for the same samples, and an import of that same fixture
/// lands byte-identical bytes in the same M13E inbox — then both transcribe
/// through the existing queue.
#[test]
fn a_recording_hands_off_bytes_identical_to_an_import() {
    let (root, mic_dirs, job_dirs) = dirs("handoff");
    let (backend, control) = FakeBackend::new();
    let jobs = Arc::new(JobHost::with_prefetcher(
        backend,
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        job_dirs.clone(),
    ));
    let jobs_for_inbox = Arc::clone(&jobs);
    let inbox: InboxFn = Arc::new(
        move |key: &str, title: &str, source: &str, path: &Path, model: &str, language: &str| {
            jobs_for_inbox.adopt_recording(key, title, source, path, model, language)
        },
    );

    let samples = sine(48_000, 1.2, 440.0);
    let host = MicHost::with_parts(
        mic_dirs,
        writer_spawner(samples.clone(), 48_000),
        Arc::clone(&inbox),
    );
    host.start(true, true, "Recording handoff", "tiny", "auto")
        .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let stopped = host.stop(StopReason::User).unwrap();
    let recording = stopped.last_stop.expect("stop record");
    assert!(recording.handed_off);
    wait_until("the recorded job", || jobs.status().done == 1);

    // A recovery replaying the same adoption (the journal write was cut) is
    // idempotent: same key + same inbox path is the same take, not a duplicate.
    jobs.adopt_recording(
        &recording.id,
        "Recording handoff",
        "Recording handoff.wav",
        Path::new(&recording.path),
        "tiny",
        "auto",
    )
    .expect("re-adopting the same take is a no-op");
    assert_eq!(jobs.status().total, 1);

    // The desktop tail for the same samples: resample once, write with the
    // desktop writer.
    let fixture = root.join("desktop.wav");
    let resampled = suramak_asr::capture::resample_16k(&samples, 48_000).unwrap();
    suramak_asr::capture::write_recording_wav(&fixture, &resampled, 16_000).unwrap();
    let recorded_bytes = std::fs::read(&recording.path).unwrap();
    let desktop_bytes = std::fs::read(&fixture).unwrap();
    assert_same_bytes(
        &recorded_bytes,
        &desktop_bytes,
        "the recorded WAV must be the desktop WAV byte for byte",
    );

    // The same fixture through the import path: copy → import_done → queue.
    let size = std::fs::metadata(&fixture).unwrap().len();
    let (key, dest) = jobs
        .begin_import("desktop.wav", "audio/wav", size, "tiny", "auto")
        .unwrap();
    std::fs::copy(&fixture, &dest).unwrap();
    jobs.import_progress(&key, size);
    jobs.import_done(&key).unwrap();
    wait_until("the imported job", || jobs.status().done == 2);

    assert_same_bytes(
        &std::fs::read(&recording.path).unwrap(),
        &std::fs::read(&dest).unwrap(),
        "recording and import must land the same bytes in the inbox",
    );
    let store = Store::open(&job_dirs.db_path).unwrap();
    let rows = store.list_transcripts().unwrap();
    assert_eq!(rows.len(), 2);
    let recorded_row = rows
        .iter()
        .find(|row| row.title == "Recording handoff")
        .expect("the recording row");
    let imported_row = rows
        .iter()
        .find(|row| row.title == "desktop")
        .expect("the import row");
    for row in [recorded_row, imported_row] {
        let full = store.get_transcript(row.id).unwrap();
        assert_eq!(
            full.segments[0].text,
            "selamat pagi, this is the fake engine"
        );
        assert_eq!(
            full.audio_path
                .as_deref()
                .and_then(|path| std::fs::read(path).ok()),
            Some(recorded_bytes.clone()),
            "every row points at WAV bytes identical to the fixture"
        );
    }
    assert_eq!(jobs.status().failed, 0);
    assert_eq!(
        control.max_active.load(Ordering::SeqCst),
        1,
        "the recording and the import share the one inference slot"
    );
    let _ = std::fs::remove_dir_all(&root);
}
