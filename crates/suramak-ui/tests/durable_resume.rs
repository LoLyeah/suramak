//! M22A2 — the desktop host's durable queue wiring.
//!
//! This test proves the host, not the M22A mechanism: it seeds the store with a
//! job a killed process left `transcribing` (the M22A fixture), builds the
//! desktop host's queue through [`DurableQueue`] — the exact constructor
//! `Runtime::new` uses — and asserts the job runs to completion with the
//! journal's `resumed` marker. A deterministic backend replaces the model, so
//! no audio engine or window is involved.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use suramak_asr::queue::driver::{DecodeDone, DecodeTask};
use suramak_asr::{EngineId, InferenceBackend, Job, JobOptions, Prefetcher, WorkerEvent};
use suramak_core::settings::{EngineSetting, LangMode};
use suramak_core::store::{JobRecord, JobStatus, Store};
use suramak_core::transcript::{Segment, Source, Transcript};
use suramak_core::Uuid;
use suramak_ui::durable_queue::DurableQueue;

fn root(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("suramak-ui-durable-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn segments(from: i64, count: u32, next_idx: u32) -> Vec<Segment> {
    (0..count)
        .map(|offset| Segment {
            idx: next_idx + offset,
            start_ms: from + i64::from(offset) * 1_000,
            end_ms: from + i64::from(offset + 1) * 1_000,
            text: format!("segment {}", next_idx + offset),
        })
        .collect()
}

/// A store left by a killed process: three committed segments at 3 s.
fn seed_interrupted(db: &Path, source: &Path, model: &Path) {
    let store = Store::open(db).unwrap();
    let now = suramak_core::Utc::now();
    let record = JobRecord {
        id: Uuid::new_v4(),
        path: source.display().to_string(),
        model: model.display().to_string(),
        status: JobStatus::Transcribing,
        created_at: now,
        updated_at: now,
        transcript_id: Some(Uuid::new_v4()),
        last_committed_segment: 0,
        last_committed_ms: 0,
        resumed: false,
        failure: None,
    };
    store.insert_job(&record).unwrap();
    let transcript = Transcript::new(
        "lecture.wav".to_string(),
        Source::File {
            name: "lecture.wav".to_string(),
        },
        "tiny".to_string(),
        Some("en".to_string()),
        Some(source.to_path_buf()),
        segments(0, 3, 0),
    );
    store.commit_job_checkpoint(record.id, &transcript).unwrap();
    let row = store.job(record.id).unwrap().unwrap();
    assert_eq!(row.last_committed_segment, 3);
}

/// A prefetch pool that answers immediately with one second of silence. The
/// fake engine ignores the samples; the driver only needs the callback to move.
#[derive(Default)]
struct InstantPrefetch {
    done: Vec<DecodeDone>,
}

impl Prefetcher for InstantPrefetch {
    fn request(&mut self, task: DecodeTask) {
        self.done.push(DecodeDone {
            id: task.id,
            result: Ok(vec![0.0; 16_000]),
        });
    }

    fn try_recv(&mut self) -> Option<DecodeDone> {
        if self.done.is_empty() {
            None
        } else {
            Some(self.done.remove(0))
        }
    }
}

/// A deterministic engine: returns the committed prefix the driver handed it
/// plus one new segment, exactly what the real pipeline returns on a resume.
struct ResumedBackend {
    submitted: Option<Job>,
}

impl InferenceBackend for ResumedBackend {
    fn submit(&mut self, job: Job) -> Result<(), String> {
        self.submitted = Some(job);
        Ok(())
    }

    fn try_recv(&mut self) -> Option<WorkerEvent> {
        let job = self.submitted.take()?;
        let durable = job.durable.expect("the host journals every job");
        let resume = durable.resume.expect("the job resumed at a boundary");
        let mut transcript = Transcript::new(
            durable.title.clone(),
            durable.source.clone(),
            "tiny".to_string(),
            Some("en".to_string()),
            Some(job.input.clone()),
            durable.committed.clone(),
        );
        transcript
            .segments
            .extend(segments(resume.from_ms, 1, resume.next_idx));
        Some(WorkerEvent::Finished {
            job_id: job.id,
            transcript: Box::new(transcript),
            words: Vec::new(),
            engine_used: EngineId::Cpu,
            warnings: Vec::new(),
        })
    }
}

#[test]
fn the_desktop_host_resumes_an_interrupted_job_to_completion() {
    let dir = root("resume");
    let source = dir.join("lecture.wav");
    let model = dir.join("ggml-tiny.bin");
    std::fs::write(&source, b"audio").unwrap();
    std::fs::write(&model, b"model").unwrap();
    let db = dir.join("library.db");
    seed_interrupted(&db, &source, &model);

    let options = JobOptions {
        model_path: model.clone(),
        lang: LangMode::Auto,
        initial_prompt: None,
        engine: EngineSetting::Auto,
        fast_engine: false,
    };

    // The host's queue creation — exactly what Runtime::new calls.
    let queue = DurableQueue::open(
        ResumedBackend { submitted: None },
        InstantPrefetch::default(),
        options,
        &db,
    )
    .expect("the host opens the journal");
    assert!(queue.journaled());
    assert_eq!(queue.resumed.len(), 1);
    let (mut driver, _resumed) = queue.into_parts();
    assert_eq!(
        driver.queue().items().len(),
        1,
        "the interrupted row is in the driver's queue"
    );
    assert!(driver.queue().items()[0].resumed);

    // Drive the host's queue the way the bridge thread does.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !driver.is_idle() {
        driver.run_once();
        if Instant::now() > deadline {
            panic!("the resumed job did not finish");
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    let store = Store::open(&db).unwrap();
    let row = store
        .jobs()
        .unwrap()
        .into_iter()
        .find(|job| job.path == source.display().to_string())
        .expect("the seeded row");
    assert_eq!(row.status, JobStatus::Done);
    assert!(row.resumed, "the `resumed` marker lands on the row");
    let transcript = store.get_transcript(row.transcript_id.unwrap()).unwrap();
    assert_eq!(transcript.segments.len(), 4, "prefix + the resumed tail");
    assert_eq!(transcript.segments.last().unwrap().idx, 3);
    assert_eq!(
        transcript.segments.last().unwrap().start_ms,
        3_000,
        "the tail starts at the committed boundary"
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
