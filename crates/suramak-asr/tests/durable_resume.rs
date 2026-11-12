//! M22A — the crash proof: a worker process is SIGKILLed mid-transcription and
//! a fresh one resumes from the journal to a **byte-equal** transcript.
//!
//! ## Why a child process
//!
//! A checkpoint is only worth anything if the bytes survive the loss of the
//! process that wrote them. This test therefore runs the real
//! [`QueueDriver`] + journal in a child of this test binary, kills it with
//! `SIGKILL` (no destructors, no flush, no WAL close) in the middle of the job,
//! and starts a second child that reads the journal. Both children share the
//! production queue/checkpoint/resume code; only the engine is a deterministic
//! fixture — no whisper model is checked into the repo, and a real model's tail
//! output after a mid-file restart is not byte-reproducible by design. The
//! fixture's segments depend only on absolute audio time, so any duplication or
//! gap introduced by the checkpoint bookkeeping changes the bytes.
//!
//! The test is skipped (returns) when the child environment is set, so a child
//! invocation that accidentally runs the whole binary cannot recurse.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use suramak_asr::engine::EngineId;
use suramak_asr::queue::driver::{
    DecodeDone, DecodeTask, InferenceBackend, JobOptions, Prefetcher, QueueDriver, QueueEvent,
};
use suramak_asr::worker::{Job, JobStage, WorkerEvent};
use suramak_core::settings::{EngineSetting, LangMode};
use suramak_core::store::{JobStatus, Store};
use suramak_core::transcript::{Segment, Transcript};
use suramak_core::{Utc, Uuid};

const CHILD_ENV: &str = "SURAMAK_M22A_CHILD";
const DB_ENV: &str = "SURAMAK_M22A_DB";
const WAV_ENV: &str = "SURAMAK_M22A_WAV";
const MODEL_ENV: &str = "SURAMAK_M22A_MODEL";
const DELAY_ENV: &str = "SURAMAK_M22A_DELAY_MS";

/// The synthetic job's length: 30 one-second segments.
const SEGMENTS: usize = 30;

// ---------------------------------------------------------------------------
// The deterministic engine
// ---------------------------------------------------------------------------

/// Produces one segment per second of **absolute** audio time. `from_ms` and
/// `next_idx` are applied exactly as the real pipeline applies them, so a
/// resumed run that starts at the wrong boundary would duplicate or skip text.
struct FixtureBackend {
    events: Receiver<WorkerEvent>,
    tx: Sender<WorkerEvent>,
    /// Per-segment delay, so the parent has time to SIGKILL mid-job.
    delay: Duration,
}

impl FixtureBackend {
    fn new(delay: Duration) -> Self {
        let (tx, events) = mpsc::channel();
        Self { events, tx, delay }
    }
}

impl InferenceBackend for FixtureBackend {
    fn submit(&mut self, job: Job) -> Result<(), String> {
        let durable = job
            .durable
            .clone()
            .ok_or_else(|| "the fixture backend needs a durable job".to_string())?;
        let tx = self.tx.clone();
        let delay = self.delay;
        let job_id = job.id;
        std::thread::spawn(move || {
            let from_ms = durable.resume.map(|resume| resume.from_ms).unwrap_or(0);
            let next_idx = durable.resume.map(|resume| resume.next_idx).unwrap_or(0);
            let start_second = (from_ms.max(0) as usize) / 1_000;
            let mut produced = Vec::new();
            for second in start_second..SEGMENTS {
                let segment = Segment {
                    idx: next_idx + produced.len() as u32,
                    start_ms: second as i64 * 1_000,
                    end_ms: (second as i64 + 1) * 1_000,
                    text: format!("segment {second:02}"),
                };
                let _ = tx.send(WorkerEvent::Checkpoint {
                    job_id,
                    segments: vec![segment.clone()],
                });
                produced.push(segment);
                std::thread::sleep(delay);
            }
            let mut segments = durable.committed.clone();
            segments.extend(produced);
            let transcript = Transcript {
                id: durable.transcript_id,
                title: durable.title.clone(),
                created_at: durable.created_at,
                updated_at: Utc::now(),
                source: durable.source.clone(),
                model: durable.model.clone(),
                language: durable.language.clone(),
                audio_path: durable.audio_path.clone(),
                segments,
            };
            let _ = tx.send(WorkerEvent::Stage {
                job_id,
                stage: JobStage::Transcribing,
            });
            let _ = tx.send(WorkerEvent::Segments {
                job_id,
                segments: transcript.segments.clone(),
            });
            let _ = tx.send(WorkerEvent::Finished {
                job_id,
                transcript: Box::new(transcript),
                words: Vec::new(),
                engine_used: EngineId::Cpu,
                warnings: Vec::new(),
            });
        });
        Ok(())
    }

    fn try_recv(&mut self) -> Option<WorkerEvent> {
        self.events.try_recv().ok()
    }
}

/// No decode-ahead: the fixture engine owns the audio, and a real decode would
/// only add a file dependency to the crash test.
struct NullPrefetch;

impl Prefetcher for NullPrefetch {
    fn request(&mut self, _task: DecodeTask) {}
    fn try_recv(&mut self) -> Option<DecodeDone> {
        None
    }
}

// ---------------------------------------------------------------------------
// Child process
// ---------------------------------------------------------------------------

fn child_main(mode: &str) {
    let db = PathBuf::from(std::env::var(DB_ENV).expect("child db"));
    let wav = PathBuf::from(std::env::var(WAV_ENV).expect("child wav"));
    let model = PathBuf::from(std::env::var(MODEL_ENV).expect("child model"));
    let delay_ms: u64 = std::env::var(DELAY_ENV)
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(0);

    // The parent polls `crash.db` every 10 ms; a cross-process open can lose a
    // busy race under a loaded test host. Retry briefly instead of turning a
    // lock into a child panic (the journal itself is unchanged).
    let store = {
        let mut last = None;
        let mut opened = None;
        for _ in 0..100 {
            match Store::open(&db) {
                Ok(store) => {
                    opened = Some(store);
                    break;
                }
                Err(err) => {
                    last = Some(err);
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        match (opened, last) {
            (Some(store), _) => store,
            (None, Some(err)) => panic!("open journal: {err}"),
            (None, None) => unreachable!("the retry loop always sets one"),
        }
    };
    let options = JobOptions {
        model_path: model,
        lang: LangMode::Fixed("en".to_string()),
        initial_prompt: None,
        engine: EngineSetting::Cpu,
        fast_engine: false,
    };
    let mut driver = QueueDriver::new(
        FixtureBackend::new(Duration::from_millis(delay_ms)),
        NullPrefetch,
        options,
    )
    .with_journal(store);

    match mode {
        "run" => {
            driver.enqueue(wav);
        }
        "resume" => {
            let outcomes = driver.resume_pending();
            if outcomes.is_empty() {
                eprintln!("child: no pending job to resume");
                std::process::exit(4);
            }
            eprintln!("child: resume_pending {:?}", outcomes);
        }
        other => {
            eprintln!("child: unknown mode {other}");
            std::process::exit(5);
        }
    }

    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        driver.run_once();
        for event in driver.drain_events() {
            if let QueueEvent::Finished { transcript, .. } = event {
                eprintln!(
                    "child: finished {} segments (job {})",
                    transcript.segments.len(),
                    transcript.id
                );
            }
        }
        if driver.is_idle() {
            break;
        }
        if Instant::now() > deadline {
            eprintln!("child: timed out");
            std::process::exit(6);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The child entry point. A normal test run (no `SURAMAK_M22A_CHILD`) returns
/// immediately; the parent spawns this binary with `--exact sigkill_child_entry`
/// and the mode environment.
#[test]
fn sigkill_child_entry() {
    let Ok(mode) = std::env::var(CHILD_ENV) else {
        return;
    };
    child_main(&mode);
}

// ---------------------------------------------------------------------------
// The parent
// ---------------------------------------------------------------------------

fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("suramak-m22a-sigkill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn spawn_child(
    mode: &str,
    db: &Path,
    wav: &Path,
    model: &Path,
    delay_ms: u64,
) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "sigkill_child_entry", "--nocapture"])
        .env(CHILD_ENV, mode)
        .env(DB_ENV, db)
        .env(WAV_ENV, wav)
        .env(MODEL_ENV, model)
        .env(DELAY_ENV, delay_ms.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the child worker")
}

fn transcript_bytes(store: &Store, transcript_id: Uuid) -> Vec<u8> {
    serde_json::to_vec(&store.get_transcript(transcript_id).unwrap().segments).unwrap()
}

#[test]
fn an_interrupted_run_resumes_to_a_byte_equal_transcript() {
    // A child invocation must never run the parent test (no recursion).
    if std::env::var(CHILD_ENV).is_ok() {
        return;
    }

    let dir = scratch_dir();
    let wav = dir.join("lecture.wav");
    let model = dir.join("ggml-tiny.bin");
    // The fixture engine owns the audio; the files exist because the journal
    // checks its inputs before letting a resume start.
    std::fs::write(&wav, b"RIFF....WAVEfmt ").unwrap();
    std::fs::write(&model, b"model").unwrap();

    // 1. The uninterrupted reference run.
    let reference_db = dir.join("reference.db");
    let child = spawn_child("run", &reference_db, &wav, &model, 0);
    let output = child.wait_with_output().expect("reference run");
    assert!(
        output.status.success(),
        "the reference run failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let reference = Store::open(&reference_db).unwrap();
    let reference_job = reference.jobs().unwrap().pop().expect("a job row");
    assert_eq!(reference_job.status, JobStatus::Done);
    let reference_segments = reference
        .get_transcript(reference_job.transcript_id.unwrap())
        .unwrap();
    assert_eq!(reference_segments.segments.len(), SEGMENTS);
    let reference_bytes = transcript_bytes(&reference, reference_job.transcript_id.unwrap());
    drop(reference);

    // 2. The interrupted run: slow enough to be killed mid-transcription.
    let crash_db = dir.join("crash.db");
    let mut child = spawn_child("run", &crash_db, &wav, &model, 40);
    let deadline = Instant::now() + Duration::from_secs(60);
    let committed_at_kill: u32;
    let job_id;
    loop {
        assert!(
            Instant::now() < deadline,
            "the child never reached its first checkpoint"
        );
        if let Some(child_status) = child.try_wait().expect("poll the child") {
            panic!("the child finished before it could be killed ({child_status})");
        }
        if let Ok(store) = Store::open(&crash_db) {
            if let Ok(rows) = store.resumable_jobs() {
                if let Some(row) = rows.first() {
                    if row.last_committed_segment >= 4 {
                        committed_at_kill = row.last_committed_segment;
                        job_id = row.id;
                        break;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // SIGKILL: no destructors, no final flush, no clean WAL close.
    child.kill().expect("SIGKILL the child");
    let _ = child.wait();
    assert!(committed_at_kill >= 4);

    // 3. The bytes that survived: the journal and the transcript agree, and the
    // transcript never claims more than the journal.
    let store = Store::open(&crash_db).unwrap();
    let row = store.job(job_id).unwrap().expect("the interrupted row");
    assert_eq!(row.status, JobStatus::Transcribing);
    assert_eq!(row.last_committed_segment, committed_at_kill);
    let committed_rows = store
        .get_transcript(row.transcript_id.unwrap())
        .unwrap()
        .segments;
    assert_eq!(
        committed_rows.len(),
        committed_at_kill as usize,
        "a crash must never leave the transcript claiming more than the journal"
    );
    drop(store);

    // 4. A fresh process resumes from the journal.
    let child = spawn_child("resume", &crash_db, &wav, &model, 0);
    let output = child.wait_with_output().expect("resume run");
    assert!(
        output.status.success(),
        "the resumed run failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("resume_pending"),
        "the child must take the launch-resume path"
    );

    // 5. Byte equality with the uninterrupted run.
    let resumed = Store::open(&crash_db).unwrap();
    let row = resumed.job(job_id).unwrap().unwrap();
    assert_eq!(row.status, JobStatus::Done);
    assert!(row.resumed, "the resumed marker lands on the row");
    assert_eq!(row.last_committed_segment, SEGMENTS as u32);
    let resumed_segments = resumed.get_transcript(row.transcript_id.unwrap()).unwrap();
    assert_eq!(resumed_segments.segments.len(), SEGMENTS);
    let resumed_bytes = transcript_bytes(&resumed, row.transcript_id.unwrap());
    assert_eq!(
        resumed_bytes, reference_bytes,
        "the resumed transcript must be byte-equal to the uninterrupted one"
    );
    // And it is one contiguous 0..29 run: no duplicate, no gap.
    for (index, segment) in resumed_segments.segments.iter().enumerate() {
        assert_eq!(segment.idx as usize, index);
        assert_eq!(segment.text, format!("segment {index:02}"));
    }
    drop(resumed);
    let _ = std::fs::remove_dir_all(&dir);
}
