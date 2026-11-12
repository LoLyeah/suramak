//! M13e — host tests for the import → copy → transcribe → library loop.
//!
//! The Android service, the notification and the SAF descriptor are Kotlin and
//! are not exercised here (no emulator: see the report's owner-device
//! checklist). What *is* exercised is every rule the service depends on:
//!
//! * the import state machine, end to end, with the real `FileDecoder` reading a
//!   real Opus fixture (the M15.2 shared path — not a mock);
//! * one inference slot across a batch (the M16.1 `QueueDriver` discipline);
//! * cancel mid-decode (the decode-ahead task's token) and mid-transcribe;
//! * a process kill during transcription, and the launch-time resume/discard
//!   prompt, including "no transcript text ever sits in the journal";
//! * the manifest's share matrix and service declaration, as declared.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use suramak_android::jobs::{HostDirs, JobHost, Phase};
use suramak_asr::queue::driver::{DecodeDone, DecodeFailure, DecodeTask};
use suramak_asr::{
    EngineId, FileDecoder, InferenceBackend, Job, Prefetcher, ThreadedPrefetch, WorkerEvent,
};
use suramak_core::store::{JobRecord, JobStatus, Store};
use suramak_core::transcript::{Segment, Source, Transcript};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../suramak-core/fixtures/audio")
        .canonicalize()
        .expect("the committed audio fixtures")
}

fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("suramak-jobs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A host directory tree with the `tiny` model "installed" (an empty file: the
/// fake backend never reads it, the accept check only needs it to exist).
fn dirs(name: &str) -> HostDirs {
    let root = root(name);
    let dirs = HostDirs::under(&root);
    std::fs::create_dir_all(&dirs.models_dir).unwrap();
    std::fs::write(dirs.models_dir.join("ggml-tiny.bin"), b"not a real model").unwrap();
    dirs
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

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

struct Control {
    active: AtomicUsize,
    max_active: AtomicUsize,
    auto: bool,
    steps: Mutex<Receiver<()>>,
}

struct Running {
    job: Job,
    step: u8,
}

/// A backend that finishes in two scripted steps (`progress 50`, then
/// `finished`), or immediately in `auto` mode, and always answers a cancelled
/// token with `Cancelled`.
struct FakeBackend {
    control: Arc<Control>,
    current: Mutex<Option<Running>>,
}

impl FakeBackend {
    fn new(auto: bool) -> (Self, Sender<()>, Arc<Control>) {
        let (tx, rx) = mpsc::channel();
        let control = Arc::new(Control {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            auto,
            steps: Mutex::new(rx),
        });
        (
            Self {
                control: control.clone(),
                current: Mutex::new(None),
            },
            tx,
            control,
        )
    }
}

fn fake_transcript(input: &Path) -> Transcript {
    let name = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
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
        *self.current.lock().unwrap() = Some(Running { job, step: 0 });
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
        if !self.control.auto && self.control.steps.lock().unwrap().try_recv().is_err() {
            return None;
        }
        if running.step == 0 {
            running.step = 1;
            return Some(WorkerEvent::Progress {
                job_id: running.job.id,
                pct: 50,
            });
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

/// A prefetcher whose decode results are delivered by the test, so "cancel
/// while the decode is in flight" is deterministic.
#[derive(Clone, Default)]
struct ManualPrefetch {
    tasks: Arc<Mutex<Vec<DecodeTask>>>,
    done: Arc<Mutex<Vec<DecodeDone>>>,
}

impl Prefetcher for ManualPrefetch {
    fn request(&mut self, task: DecodeTask) {
        self.tasks.lock().unwrap().push(task);
    }

    fn try_recv(&mut self) -> Option<DecodeDone> {
        let mut done = self.done.lock().unwrap();
        if done.is_empty() {
            None
        } else {
            Some(done.remove(0))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The whole contract in one flow: a shared voice note becomes a library row,
/// through the real decoder, with the title the user shared.
#[test]
fn a_shared_voice_note_becomes_a_library_row() {
    let dirs = dirs("end-to-end");
    let (backend, _steps, _control) = FakeBackend::new(true);
    let host = JobHost::with_prefetcher(
        backend,
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        dirs.clone(),
    );
    let source = fixtures().join("speech-en.opus");
    let size = std::fs::metadata(&source).unwrap().len();

    let (key, dest) = host
        .begin_import(
            "Ptt 2026-11-10 at 10.32.opus",
            "audio/ogg",
            size,
            "tiny",
            "auto",
        )
        .expect("accepted");
    assert_eq!(dest.parent().unwrap(), dirs.media_dir);
    assert!(
        dirs.journal_path.exists(),
        "the journal is written before a byte is copied"
    );

    let bytes = std::fs::read(&source).unwrap();
    let mut written = 0usize;
    for chunk in bytes.chunks(64 * 1024) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&dest)
            .unwrap();
        file.write_all(chunk).unwrap();
        written += chunk.len();
        host.import_progress(&key, written as u64);
    }

    host.import_done(&key).expect("the copy verified");
    wait_until("the job to finish", || host.status().done == 1);

    let status = host.status();
    assert_eq!(status.failed, 0);
    assert_eq!(status.total, 1);
    let last = status.last.expect("a completed job");
    assert_eq!(last.title, "Ptt 2026-11-10 at 10.32");
    assert_eq!(last.segments, 1);

    let store = Store::open(&dirs.db_path).unwrap();
    // The list is headers only (the library's shape); the segment set is read
    // back through the row.
    let rows = store.list_transcripts().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Ptt 2026-11-10 at 10.32");
    let full = store.get_transcript(rows[0].id).unwrap();
    assert_eq!(full.segments.len(), 1);
    assert_eq!(
        full.segments[0].text,
        "selamat pagi, this is the fake engine"
    );
    assert_eq!(
        rows[0].audio_path.as_deref(),
        Some(dest.as_path()),
        "the row points at the copied media, not the SAF URI"
    );
    match &rows[0].source {
        Source::File { name } => assert_eq!(name, "Ptt 2026-11-10 at 10.32.opus"),
        other => panic!("unexpected source: {other:?}"),
    }
    assert!(
        !dirs.journal_path.exists(),
        "a finished batch leaves no journal"
    );

    let opened = host
        .open_transcript(&last.transcript_id)
        .expect("the landing row");
    assert_eq!(opened.title, "Ptt 2026-11-10 at 10.32");
    assert!(host
        .open_transcript("00000000-0000-0000-0000-000000000000")
        .is_none());
}

/// Several shared files ride one batch, and the M16.1 queue never runs two
/// inferences at once.
#[test]
fn a_batch_keeps_exactly_one_inference_slot() {
    let dirs = dirs("one-slot");
    let (backend, _steps, control) = FakeBackend::new(true);
    let host = JobHost::with_prefetcher(
        backend,
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        dirs.clone(),
    );
    let source = fixtures().join("speech-id.webm");
    let size = std::fs::metadata(&source).unwrap().len();

    let mut keys = Vec::new();
    for index in 0..3 {
        let name = format!("voice note {index}.webm");
        let (key, dest) = host
            .begin_import(&name, "audio/webm", size, "tiny", "auto")
            .unwrap();
        std::fs::copy(&source, &dest).unwrap();
        host.import_progress(&key, size);
        host.import_done(&key).unwrap();
        keys.push(key);
    }
    wait_until("all three to finish", || host.status().done == 3);
    assert_eq!(
        control.max_active.load(Ordering::SeqCst),
        1,
        "the queue must never run two inferences at once"
    );
    let status = host.status();
    assert_eq!(status.total, 3);
    assert_eq!(status.failed, 0);
    assert_eq!(
        Store::open(&dirs.db_path)
            .unwrap()
            .list_transcripts()
            .unwrap()
            .len(),
        3
    );
    let _ = keys;
}

/// Cancel while the decode-ahead task is in flight: the token is set, the
/// buffer is dropped, the copy is deleted and no row is written.
///
/// Two jobs are needed for the decode-ahead path: the first file is the one the
/// engine runs inline, the second is decoded by the prefetch thread while it
/// waits for the slot — the only state in which a cancel can land mid-decode.
#[test]
fn cancelling_mid_decode_stops_the_job_and_writes_no_row() {
    let dirs = dirs("cancel-decode");
    let (backend, _steps, _control) = FakeBackend::new(false);
    let prefetch = ManualPrefetch::default();
    let host = JobHost::with_prefetcher(backend, prefetch.clone(), dirs.clone());
    let source = fixtures().join("speech-en.opus");
    let size = std::fs::metadata(&source).unwrap().len();

    let (first_key, first_dest) = host
        .begin_import("first.opus", "audio/ogg", size, "tiny", "auto")
        .unwrap();
    std::fs::copy(&source, &first_dest).unwrap();
    host.import_done(&first_key).unwrap();
    wait_until("the first job to take the slot", || {
        host.status().current_phase == "decoding" || host.status().current_phase == "transcribing"
    });

    let (key, dest) = host
        .begin_import("second.opus", "audio/ogg", size, "tiny", "auto")
        .unwrap();
    std::fs::copy(&source, &dest).unwrap();
    host.import_done(&key).unwrap();

    wait_until("the decode-ahead request", || {
        !prefetch.tasks.lock().unwrap().is_empty()
    });
    let task = prefetch.tasks.lock().unwrap()[0].clone();
    host.cancel(&key);
    wait_until("the cancelled phase", || host.status().cancelled == 1);
    assert!(
        task.cancel.is_cancelled(),
        "the decode task's token must be set so the packet loop stops"
    );
    // The late buffer arrives after the cancel and must be dropped.
    prefetch.done.lock().unwrap().push(DecodeDone {
        id: task.id,
        result: Err(DecodeFailure::Cancelled),
    });
    std::thread::sleep(Duration::from_millis(50));

    assert!(!dest.exists(), "a cancelled import does not keep its copy");
    let store = Store::open(&dirs.db_path).unwrap();
    assert!(
        store.list_transcripts().unwrap().is_empty(),
        "no partial row"
    );
}

/// Cancel while the engine holds the slot: the cooperative token reaches the
/// backend, the terminal event is `Cancelled`, nothing is written.
#[test]
fn cancelling_mid_transcribe_stops_the_job_and_writes_no_row() {
    let dirs = dirs("cancel-transcribe");
    let (backend, steps, _control) = FakeBackend::new(false);
    let host = JobHost::with_prefetcher(
        backend,
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        dirs.clone(),
    );
    let source = fixtures().join("speech-en.webm");
    let size = std::fs::metadata(&source).unwrap().len();
    let (key, dest) = host
        .begin_import("voice.opus", "audio/webm", size, "tiny", "auto")
        .unwrap();
    std::fs::copy(&source, &dest).unwrap();
    host.import_done(&key).unwrap();

    steps.send(()).unwrap();
    wait_until("the engine to report 50%", || host.status().percent == 50);
    assert_eq!(host.status().current_phase, "transcribing");

    host.cancel(&key);
    wait_until("the cancelled phase", || host.status().cancelled == 1);
    assert!(!dest.exists());
    assert!(!dirs.journal_path.exists());
    assert!(Store::open(&dirs.db_path)
        .unwrap()
        .list_transcripts()
        .unwrap()
        .is_empty());
}

/// M13g1 Quit: the job host pauses every live job (the copy and the journal
/// stay, so the next launch can resume) rather than deleting work. The
/// capture/service/exit steps are Kotlin's; this is the job half.
#[test]
fn quit_pauses_the_live_job_and_keeps_it_resumable() {
    let dirs = dirs("quit");
    let (backend, steps, _control) = FakeBackend::new(false);
    let host = JobHost::with_prefetcher(
        backend,
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        dirs.clone(),
    );
    let source = fixtures().join("speech-en.webm");
    let size = std::fs::metadata(&source).unwrap().len();
    let (key, dest) = host
        .begin_import("voice.webm", "audio/webm", size, "tiny", "auto")
        .unwrap();
    std::fs::copy(&source, &dest).unwrap();
    host.import_done(&key).unwrap();
    steps.send(()).unwrap();
    wait_until("the engine to report 50%", || host.status().percent == 50);

    assert_eq!(host.quit(), 1, "one live job is paused");
    wait_until("the paused job", || host.status().paused == 1);
    assert!(dest.exists(), "a quit keeps the copied audio for resume");
    let journal = std::fs::read_to_string(&dirs.journal_path).unwrap();
    assert!(
        journal.contains("\"paused\""),
        "the journal records the pause"
    );
    assert!(Store::open(&dirs.db_path)
        .unwrap()
        .list_transcripts()
        .unwrap()
        .is_empty());
}

/// A process kill during transcription is visible at the next launch, is
/// offered for resume, and the journal never carried the transcript's text.
#[test]
fn a_killed_job_is_offered_for_resume_and_resumes() {
    let dirs = dirs("recovery");
    let source = fixtures().join("speech-id.opus");
    let size = std::fs::metadata(&source).unwrap().len();
    let key;
    let dest;
    {
        let (backend, steps, _control) = FakeBackend::new(false);
        let host = JobHost::with_prefetcher(
            backend,
            ThreadedPrefetch::new(Box::new(FileDecoder)),
            dirs.clone(),
        );
        let accepted = host
            .begin_import("Rekaman rapat.opus", "audio/ogg", size, "tiny", "id")
            .unwrap();
        key = accepted.0;
        dest = accepted.1;
        std::fs::copy(&source, &dest).unwrap();
        host.import_done(&key).unwrap();
        steps.send(()).unwrap();
        wait_until("the engine to report 50%", || host.status().percent == 50);

        let journal = std::fs::read_to_string(&dirs.journal_path).expect("a live journal");
        assert!(journal.contains("Rekaman rapat"), "the title survives");
        assert!(journal.contains("\"tiny\""), "the model choice survives");
        assert!(journal.contains("\"percent\": 50") || journal.contains("\"percent\":50"));
        assert!(
            !journal.contains("selamat pagi") && !journal.contains("\"text\""),
            "the journal must never carry transcript text: {journal}"
        );
        // The host is dropped while the job is live: the thread stops, the
        // journal on disk is what a killed process leaves behind.
    }
    assert!(dirs.journal_path.exists());
    assert!(!Store::open(&dirs.db_path)
        .unwrap()
        .list_transcripts()
        .unwrap()
        .iter()
        .any(|t| t.title == "Rekaman rapat"));

    // "Next launch": a fresh host over the same files.
    let (backend, _steps, _control) = FakeBackend::new(true);
    let host = JobHost::with_prefetcher(
        backend,
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        dirs.clone(),
    );
    let offered = host.recover();
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].title, "Rekaman rapat");
    assert!(offered[0].resumable, "{}", offered[0].reason);
    assert!(offered[0].percent >= 50);

    host.resume(&offered[0].key).expect("resume accepted");
    wait_until("the resumed job to finish", || host.status().done == 1);
    let rows = Store::open(&dirs.db_path)
        .unwrap()
        .list_transcripts()
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Rekaman rapat");
    assert!(!dirs.journal_path.exists());
}

/// Discard is the other half of the prompt: the copy and the entry go away and
/// nothing enters the library.
#[test]
fn a_recovered_job_can_be_discarded() {
    let dirs = dirs("discard");
    let source = fixtures().join("speech-en.opus");
    let size = std::fs::metadata(&source).unwrap().len();
    let dest;
    {
        let (backend, _steps, _control) = FakeBackend::new(false);
        let host = JobHost::with_prefetcher(
            backend,
            ThreadedPrefetch::new(Box::new(FileDecoder)),
            dirs.clone(),
        );
        let (key, path) = host
            .begin_import("throw away.opus", "audio/ogg", size, "tiny", "auto")
            .unwrap();
        dest = path.clone();
        std::fs::copy(&source, &dest).unwrap();
        host.import_done(&key).unwrap();
        std::thread::sleep(Duration::from_millis(30));
    }

    let (backend, _steps, _control) = FakeBackend::new(true);
    let host = JobHost::with_prefetcher(
        backend,
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        dirs.clone(),
    );
    let offered = host.recover();
    assert_eq!(offered.len(), 1);
    host.discard(&offered[0].key);
    assert!(host.recover().is_empty());
    assert!(!dest.exists(), "the copy is removed");
    assert!(!dirs.journal_path.exists());
    assert!(Store::open(&dirs.db_path)
        .unwrap()
        .list_transcripts()
        .unwrap()
        .is_empty());
}

/// An import cut off before the copy finished cannot be resumed — the SAF grant
/// died with the process — and the prompt says exactly that.
#[test]
fn an_interrupted_copy_is_offered_as_discard_only() {
    let dirs = dirs("interrupted-copy");
    let dest;
    {
        let (backend, _steps, _control) = FakeBackend::new(false);
        let host = JobHost::with_prefetcher(
            backend,
            ThreadedPrefetch::new(Box::new(FileDecoder)),
            dirs.clone(),
        );
        let (_key, path) = host
            .begin_import("half copied.opus", "audio/ogg", 10_000_000, "tiny", "auto")
            .unwrap();
        std::fs::write(&path, vec![0u8; 4096]).unwrap();
        dest = path;
    }
    let (backend, _steps, _control) = FakeBackend::new(true);
    let host = JobHost::with_prefetcher(
        backend,
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        dirs.clone(),
    );
    let offered = host.recover();
    assert_eq!(offered.len(), 1);
    assert!(!offered[0].resumable);
    assert!(offered[0].reason.contains("had not finished copying"));
    assert!(host.resume(&offered[0].key).is_err());
    host.discard(&offered[0].key);
    assert!(!dest.exists());
}

/// A model that is not installed is refused before a byte is copied.
#[test]
fn a_missing_model_is_refused_before_the_copy() {
    let root = root("missing-model");
    let dirs = HostDirs::under(&root);
    let (backend, _steps, _control) = FakeBackend::new(true);
    let host = JobHost::with_prefetcher(
        backend,
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        dirs.clone(),
    );
    let error = host
        .begin_import("note.opus", "audio/ogg", 100, "tiny", "auto")
        .expect_err("refused");
    assert!(error.contains("not installed"), "{error}");
    assert!(!dirs.journal_path.exists());
    assert_eq!(host.status().total, 0);

    let error = host
        .begin_import("note.opus", "text/plain", 100, "tiny", "auto")
        .expect_err("refused");
    assert!(error.contains("not an audio"), "{error}");
    let error = host
        .begin_import("note.opus", "audio/ogg", 100, "gigantic", "auto")
        .expect_err("refused");
    assert!(error.contains("unknown model"), "{error}");
}

/// The copy channel is bounded: 400 reports of 16 KiB rewrite the journal at
/// most once per 256 KiB, not once per call.
#[test]
fn copy_progress_is_coalesced_to_a_bounded_step() {
    let dirs = dirs("progress-bound");
    let (backend, _steps, _control) = FakeBackend::new(true);
    let host = JobHost::with_prefetcher(
        backend,
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        dirs.clone(),
    );
    let total = 8 * 1024 * 1024u64;
    let (key, _dest) = host
        .begin_import("podcast.mp4", "video/mp4", total, "tiny", "auto")
        .unwrap();
    let writes_after_begin = host.journal_writes();
    let step = 16 * 1024u64;
    let mut bytes = 0;
    while bytes < total {
        bytes += step;
        host.import_progress(&key, bytes.min(total));
    }
    let writes = host.journal_writes() - writes_after_begin;
    let ceiling = total / suramak_android::jobs::PROGRESS_STEP_BYTES + 2;
    assert!(
        writes <= ceiling,
        "{writes} journal writes for {total} bytes (ceiling {ceiling})"
    );
    assert!(writes >= 5, "progress must still be persisted: {writes}");
    assert_eq!(host.status().percent, 100);
}

/// The declared share matrix and service are the ones the code assumes.
#[test]
fn the_manifest_declares_the_share_matrix_and_the_service() {
    let manifest = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../android/app/src/main/AndroidManifest.xml"),
    )
    .expect("the Android manifest");

    for needle in [
        "android.intent.action.SEND",
        "android.intent.action.SEND_MULTIPLE",
        "android.intent.category.DEFAULT",
        // The voice-note matrix: WhatsApp (Ogg-Opus), Telegram (.opus, WebM),
        // and the container families the shared decoder accepts.
        "audio/*",
        "application/ogg",
        "application/x-ogg",
        "audio/opus",
        "audio/ogg",
        "audio/webm",
        "video/webm",
        "video/mp4",
        "video/x-matroska",
        // SAF import.
        "android.intent.action.OPEN_DOCUMENT",
        // The service and its per-API types.
        "app.suramak.transcribe.TranscriptionJobService",
        "foregroundServiceType=\"mediaProcessing|dataSync\"",
        "android.permission.BIND_JOB_SERVICE",
        "android.permission.FOREGROUND_SERVICE_MEDIA_PROCESSING",
        "android.permission.FOREGROUND_SERVICE_DATA_SYNC",
        "android.permission.WAKE_LOCK",
        "app.suramak.transcribe.CancelTranscriptionReceiver",
        // M13F: the microphone permission, the microphone-typed foreground
        // service and its normal permission.
        "android.permission.RECORD_AUDIO",
        "android.permission.FOREGROUND_SERVICE_MICROPHONE",
        "app.suramak.mic.RecordingService",
        "foregroundServiceType=\"microphone\"",
    ] {
        assert!(
            manifest.contains(needle),
            "the manifest is missing {needle:?}"
        );
    }
}

/// The bridge protocol: per-API foreground types, closed failure for bad input.
#[test]
fn the_bridge_reports_the_foreground_type_per_api() {
    let dirs = dirs("bridge");
    let (backend, _steps, _control) = FakeBackend::new(true);
    let host =
        JobHost::with_prefetcher(backend, ThreadedPrefetch::new(Box::new(FileDecoder)), dirs);
    for (api, name) in [
        (29, "untyped"),
        (33, "untyped"),
        (34, "dataSync"),
        (35, "mediaProcessing"),
        (36, "mediaProcessing"),
    ] {
        let reply: serde_json::Value = serde_json::from_str(
            &host.dispatch(&format!(r#"{{"op":"foreground_type","api":{api}}}"#)),
        )
        .unwrap();
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["type"], name, "api {api}");
    }
    let bad: serde_json::Value = serde_json::from_str(&host.dispatch("{nope")).unwrap();
    assert_eq!(bad["ok"], false);
    let unknown: serde_json::Value =
        serde_json::from_str(&host.dispatch(r#"{"op":"nope"}"#)).unwrap();
    assert_eq!(unknown["ok"], false);
    let status: serde_json::Value =
        serde_json::from_str(&host.dispatch(r#"{"op":"status"}"#)).unwrap();
    assert_eq!(status["ok"], true);
    assert_eq!(status["total"], 0);
    assert_eq!(status["running"], false);
}

/// Phase is reported while the copy is running, and the queue count is the
/// number the notification prints.
#[test]
fn status_reports_the_current_file_and_the_queue_count() {
    let dirs = dirs("status");
    let (backend, _steps, _control) = FakeBackend::new(false);
    let host =
        JobHost::with_prefetcher(backend, ThreadedPrefetch::new(Box::new(FileDecoder)), dirs);
    let (_key, _dest) = host
        .begin_import("first.opus", "audio/ogg", 1000, "tiny", "auto")
        .unwrap();
    let status = host.status();
    assert!(status.running);
    assert_eq!(status.current_phase, Phase::Copying.as_str());
    assert_eq!(status.total, 1);
    assert_eq!(status.notification_numbers(), (0, 0));
}

/// M22B — a voice-note job carries its kind and the intent's sender into the
/// library shape, and the notification model reads them back through the same
/// store the compact UI uses.
#[test]
fn a_voice_note_notification_is_built_from_the_stored_row() {
    let dirs = dirs("voice-note-notification");
    let (backend, _steps, _control) = FakeBackend::new(true);
    let host = JobHost::with_prefetcher(
        backend,
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        dirs.clone(),
    );
    let source = fixtures().join("speech-en.opus");
    let size = std::fs::metadata(&source).unwrap().len();
    let (key, dest) = host
        .begin_import_kind(
            "Ptt 2026-11-12 at 09.14.opus",
            "audio/ogg",
            size,
            "tiny",
            "en",
            "voice_note",
            Some("Rina"),
        )
        .expect("accepted");
    std::fs::copy(&source, &dest).unwrap();
    host.import_done(&key).unwrap();
    wait_until("the voice-note job to finish", || host.status().done == 1);

    let finished = host.status().last.expect("a completed job");
    assert_eq!(finished.kind, "voice_note");
    assert_eq!(finished.sender.as_deref(), Some("Rina"));

    let model = host
        .transcript_notification(&finished.transcript_id, 0, None)
        .expect("the notification model");
    assert_eq!(
        model.subtext.as_deref(),
        Some("Rina"),
        "the intent's sender"
    );
    assert_eq!(model.title, "Transcript ready · 7 words");
    assert_eq!(model.page, 0);
    assert_eq!(model.page_count, 1);
    assert_eq!(model.text, "selamat pagi, this is the fake engine");
    assert_eq!(model.copy_text, model.text);
    assert!(model
        .actions
        .contains(&suramak_android::voice_note::Action::Copy));
    assert!(model
        .actions
        .contains(&suramak_android::voice_note::Action::ShareQuote));
    assert!(!model
        .actions
        .contains(&suramak_android::voice_note::Action::NextPage));

    // The quote card the action shares is a real staged PNG.
    let staged = host
        .stage_quote_card(&finished.transcript_id, 0, None)
        .expect("a quote card");
    assert_eq!(staged.mime, "image/png");
    assert!(std::path::Path::new(&staged.path).is_file());
    assert!(staged.bytes > 1_000);
    let _ = std::fs::remove_dir_all(dirs.db_path.parent().unwrap());
}

/// M22A2 — the jobs host's driver creation resumes the store journal.
///
/// This is the wiring, not the M22A mechanism: an interrupted row is seeded
/// with three committed segments, the next import creates the host's driver
/// (the exact `open_durable_queue` call `accept` makes), and the resumed job
/// runs to `done` with the journal's `resumed` marker — on the host's existing
/// status/completed surface, with the shared title, not the opaque copy name.
#[test]
fn the_android_host_resumes_a_journal_row_when_the_driver_is_created() {
    let dirs = dirs("durable-resume");
    let source = fixtures().join("speech-en.opus");
    std::fs::create_dir_all(&dirs.media_dir).unwrap();
    std::fs::copy(&source, dirs.media_dir.join("speech-resumed.opus")).unwrap();
    let media = dirs.media_dir.join("speech-resumed.opus");
    let model = dirs.models_dir.join("ggml-tiny.bin");

    let seeded = {
        let store = Store::open(&dirs.db_path).unwrap();
        let now = suramak_core::Utc::now();
        let record = JobRecord {
            id: suramak_core::Uuid::new_v4(),
            path: media.display().to_string(),
            model: model.display().to_string(),
            status: JobStatus::Transcribing,
            created_at: now,
            updated_at: now,
            transcript_id: Some(suramak_core::Uuid::new_v4()),
            last_committed_segment: 0,
            last_committed_ms: 0,
            resumed: false,
            failure: None,
        };
        store.insert_job(&record).unwrap();
        let mut transcript = Transcript::new(
            "speech-resumed".to_string(),
            Source::File {
                name: "speech-resumed.opus".to_string(),
            },
            "tiny".to_string(),
            Some("en".to_string()),
            Some(media.clone()),
            Vec::new(),
        );
        transcript.segments = (0..3)
            .map(|idx| Segment {
                idx,
                start_ms: i64::from(idx) * 1_000,
                end_ms: (i64::from(idx) + 1) * 1_000,
                text: format!("committed {idx}"),
            })
            .collect();
        store.commit_job_checkpoint(record.id, &transcript).unwrap();
        record.id
    };

    // The next launch's host: the first accepted job creates the driver, and
    // the driver's creation resumes the seeded row before the new item.
    let (backend, _steps, _control) = FakeBackend::new(true);
    let host = JobHost::with_prefetcher(
        backend,
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        dirs.clone(),
    );
    let size = std::fs::metadata(&source).unwrap().len();
    let (key, dest) = host
        .begin_import("after-crash.opus", "audio/ogg", size, "tiny", "auto")
        .unwrap();
    std::fs::copy(&source, &dest).unwrap();
    host.import_done(&key).unwrap();

    wait_until("both jobs to finish", || host.status().done == 2);
    let status = host.status();
    assert_eq!(status.failed, 0, "{:?}", status.last_failed);
    assert_eq!(status.total, 2, "the resumed row is on the host's surface");

    let store = Store::open(&dirs.db_path).unwrap();
    let row = store.job(seeded).unwrap().expect("the seeded job row");
    assert_eq!(row.status, JobStatus::Done);
    assert!(row.resumed, "the journal row carries the resumed marker");
    let transcript = store.get_transcript(row.transcript_id.unwrap()).unwrap();
    assert_eq!(
        transcript.title, "speech-resumed",
        "the host's title, not the copied key"
    );
    let _ = std::fs::remove_dir_all(dirs.db_path.parent().unwrap());
}

/// A backend that always fails at the engine boundary — the decode is real
/// (a valid fixture), so Retry re-runs a genuinely decodable job.
#[derive(Default)]
struct FailingBackend {
    submitted: Option<u64>,
}

impl InferenceBackend for FailingBackend {
    fn submit(&mut self, job: Job) -> Result<(), String> {
        self.submitted = Some(job.id);
        Ok(())
    }

    fn try_recv(&mut self) -> Option<WorkerEvent> {
        self.submitted.take().map(|id| WorkerEvent::Error {
            job_id: id,
            message: "the fake engine refused the audio".to_string(),
        })
    }
}

/// M22B — a failure keeps the copy so Retry can honestly re-run the same job,
/// and the failure record names the queue's reason and the sender.
#[test]
fn a_failed_voice_note_keeps_its_copy_and_can_be_retried() {
    let dirs = dirs("voice-note-retry");
    let host = JobHost::with_prefetcher(
        FailingBackend::default(),
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        dirs.clone(),
    );
    let source = fixtures().join("speech-en.opus");
    let size = std::fs::metadata(&source).unwrap().len();
    let (key, dest) = host
        .begin_import_kind(
            "refused.opus",
            "audio/ogg",
            size,
            "tiny",
            "auto",
            "voice_note",
            Some("Rina"),
        )
        .expect("accepted");
    std::fs::copy(&source, &dest).unwrap();
    host.import_done(&key).unwrap();
    wait_until("the engine to fail", || host.status().failed == 1);

    let status = host.status();
    let failure = status.last_failed.expect("a named failure");
    assert_eq!(failure.key, key);
    assert_eq!(failure.kind, "voice_note");
    assert_eq!(failure.sender.as_deref(), Some("Rina"));
    assert!(!failure.reason.is_empty(), "the queue's own reason");
    assert!(dest.exists(), "Retry needs the copy the failure kept");

    host.retry(&key).expect("retry accepted");
    wait_until("the retried job to fail again", || {
        host.status().failed == 2
    });
    assert!(dest.exists(), "the retried job still owns its copy");
    let _ = std::fs::remove_dir_all(dirs.db_path.parent().unwrap());
}
