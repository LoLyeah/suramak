//! M22A — the durable half of the queue: checkpoints, resume planning and the
//! one transaction that keeps text and journal honest.
//!
//! The M16.1 queue is in memory: an OS kill mid-lecture restarts from zero. This
//! module makes a running job survive that kill with the existing SQLite/WAL
//! store (schema v8's `jobs` table + the transcript's normal `segments` rows):
//!
//! * **Checkpoints.** While the engine runs, every finalized segment is handed
//!   to a [`CheckpointWriter`]. Every [`CHECKPOINT_SEGMENTS`] segments (or every
//!   [`CHECKPOINT_AUDIO_MS`] of decoded audio) the writer commits the prefix it
//!   has so far — transcript text *and* the journal count — through
//!   [`suramak_core::store::Store::commit_job_checkpoint`], which is **one
//!   transaction**. The segments land first, the journal second; a crash sees
//!   both or neither.
//! * **Resume.** [`plan_resume`] turns an interrupted [`JobRecord`] into either a
//!   fresh start, a [`Resume`] boundary (`from_ms` + `next_idx`), or a named
//!   `resume_unavailable: …` refusal when the source or the model is gone. The
//!   pipeline re-feeds the model only the window after `from_ms`, so committed
//!   text is never produced twice and no audio is skipped.
//! * **Cancellation.** The writer's pending segments are flushed before the
//!   status becomes `cancelled`: a partial transcript is real text, and it stays.
//!
//! The writer holds no `Store` reference (the driver owns the single
//! connection); it is plain state plus [`CheckpointWriter::flush`], so the
//! policy and the resume arithmetic are pinned by pure unit tests.

use std::path::{Path, PathBuf};

use suramak_core::store::{JobRecord, Store};
use suramak_core::transcript::{Segment, Source, Transcript};
use suramak_core::{DateTime, Utc, Uuid};

/// How many segments may accumulate before a checkpoint is forced.
pub const CHECKPOINT_SEGMENTS: usize = 4;

/// How much audio may accumulate before a checkpoint is forced (~5 s). A single
/// long segment flushes as soon as it is finalized, so the bound is never
/// exceeded by more than one segment's duration.
pub const CHECKPOINT_AUDIO_MS: i64 = 5_000;

/// When a checkpoint is due. Pure, so the rule is testable without a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointPolicy {
    /// Force a checkpoint after this many new segments.
    pub every_segments: usize,
    /// Force a checkpoint once this much audio has been produced since the last
    /// one.
    pub every_ms: i64,
}

impl Default for CheckpointPolicy {
    fn default() -> Self {
        Self {
            every_segments: CHECKPOINT_SEGMENTS,
            every_ms: CHECKPOINT_AUDIO_MS,
        }
    }
}

impl CheckpointPolicy {
    /// Whether `new_segments` since the last commit (spanning `span_ms` of audio)
    /// should be committed now.
    pub fn due(&self, new_segments: usize, span_ms: i64) -> bool {
        new_segments >= self.every_segments.max(1) || span_ms >= self.every_ms
    }
}

/// Where a resumed transcription starts in the decoded audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resume {
    /// The end of the last committed segment: the first sample the model has not
    /// seen. The pipeline slices the decoded buffer here and offsets every new
    /// timestamp by it, so the resumed transcript carries absolute times.
    pub from_ms: i64,
    /// The index the next produced segment must carry (== committed count).
    pub next_idx: u32,
}

/// One job's durable identity plus everything a (re)started run needs.
///
/// The driver builds this at the start of the job and hands it to the worker;
/// the worker uses `committed`/`resume` to continue and `job_id`/`transcript_id`
/// to keep the row identity stable across the crash.
#[derive(Debug, Clone)]
pub struct DurableJob {
    pub job_id: Uuid,
    pub transcript_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub title: String,
    pub source: Source,
    pub model: String,
    pub language: Option<String>,
    pub audio_path: Option<PathBuf>,
    /// Segments already committed to the transcript, in `idx` order.
    pub committed: Vec<Segment>,
    /// `None` for a fresh run; `Some` when continuing an interrupted one.
    pub resume: Option<Resume>,
}

impl DurableJob {
    /// Builds the transcript header this job writes under, with `segments` as
    /// its current text. The id and `created_at` never change across a resume.
    pub fn transcript(&self, segments: Vec<Segment>) -> Transcript {
        Transcript {
            id: self.transcript_id,
            title: self.title.clone(),
            created_at: self.created_at,
            updated_at: Utc::now(),
            source: self.source.clone(),
            model: self.model.clone(),
            language: self.language.clone(),
            audio_path: self.audio_path.clone(),
            segments,
        }
    }

    /// The resume boundary, when there is committed text to continue after.
    pub fn resume(&self) -> Option<Resume> {
        self.resume
    }
}

/// What a launch should do with an interrupted job.
#[derive(Debug, Clone, PartialEq)]
pub enum ResumePlan {
    /// Nothing was committed (or there is no transcript yet): start at zero.
    Fresh,
    /// Continue after the committed prefix.
    Resume {
        committed: Vec<Segment>,
        from_ms: i64,
        next_idx: u32,
    },
    /// The job cannot be resumed; the caller fails it with this named reason.
    Unavailable { reason: String },
}

/// Decides how to continue `job`, checking that its inputs still exist.
///
/// The source and model are checked **before** any work starts, so a job whose
/// file or model disappeared becomes `failed(resume_unavailable: …)` instead of
/// silently vanishing from the queue — the honesty rule of the slice.
pub fn plan_resume(
    store: &Store,
    job: &JobRecord,
    model_path: &Path,
) -> Result<ResumePlan, suramak_core::SuramakError> {
    let source = Path::new(&job.path);
    if !source.is_file() {
        return Ok(ResumePlan::Unavailable {
            reason: format!("the source file is gone ({})", source.display()),
        });
    }
    if !model_path.is_file() {
        return Ok(ResumePlan::Unavailable {
            reason: format!("the model is gone ({})", model_path.display()),
        });
    }
    let (Some(transcript_id), count) = (job.transcript_id, job.last_committed_segment) else {
        return Ok(ResumePlan::Fresh);
    };
    if count == 0 {
        return Ok(ResumePlan::Fresh);
    }
    let committed = store.committed_segments(transcript_id, count)?;
    if committed.len() != count as usize {
        return Ok(ResumePlan::Unavailable {
            reason: format!(
                "the committed text is missing (the journal says {count} segments, the library has {})",
                committed.len()
            ),
        });
    }
    if job.last_committed_ms <= 0 {
        // A boundary at zero would re-feed the whole file and duplicate the
        // committed text. Refuse honestly instead of guessing.
        return Ok(ResumePlan::Unavailable {
            reason: "the checkpoint has no usable audio boundary (0 ms)".to_string(),
        });
    }
    Ok(ResumePlan::Resume {
        committed,
        from_ms: job.last_committed_ms,
        next_idx: count,
    })
}

/// Accumulates newly finalized segments and decides when to commit them.
///
/// The driver owns one per running job. `push` is cheap (no I/O); `flush` is the
/// only place that touches the store, and it always writes the whole prefix so
/// the transcript and the journal count describe the same thing.
pub struct CheckpointWriter {
    job: DurableJob,
    policy: CheckpointPolicy,
    /// Everything received so far (the committed prefix + uncommitted tail).
    segments: Vec<Segment>,
    /// New segments since the last successful flush.
    since_flush: usize,
    /// `end_ms` of the last segment that reached the store.
    last_flush_ms: i64,
}

impl CheckpointWriter {
    /// Starts a writer at the job's current committed prefix.
    pub fn new(job: DurableJob, policy: CheckpointPolicy) -> Self {
        let last_flush_ms = job.committed.last().map(|seg| seg.end_ms).unwrap_or(0);
        Self {
            segments: job.committed.clone(),
            job,
            policy,
            since_flush: 0,
            last_flush_ms,
        }
    }

    /// The job this writer commits for.
    pub fn job(&self) -> &DurableJob {
        &self.job
    }

    /// Adds one finalized segment. Returns `true` when a checkpoint is due and
    /// [`CheckpointWriter::flush`] must be called.
    pub fn push(&mut self, segment: Segment) -> bool {
        self.segments.push(segment);
        self.since_flush += 1;
        let span_ms = self
            .segments
            .last()
            .map(|seg| seg.end_ms - self.last_flush_ms)
            .unwrap_or(0);
        self.policy.due(self.since_flush, span_ms)
    }

    /// Whether there are segments the store has not seen.
    pub fn has_pending(&self) -> bool {
        self.since_flush > 0
    }

    /// How many segments are in the prefix (committed + pending).
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The prefix as it stands, committed or not.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Commits the whole prefix and the journal count in one transaction.
    ///
    /// A no-op when nothing new arrived. An error is the caller's to surface —
    /// a disk that cannot take the checkpoint must not be reported as a
    /// successful job.
    pub fn flush(&mut self, store: &Store) -> Result<(), suramak_core::SuramakError> {
        if self.since_flush == 0 {
            return Ok(());
        }
        let transcript = self.job.transcript(self.segments.clone());
        store.commit_job_checkpoint(self.job.job_id, &transcript)?;
        self.since_flush = 0;
        self.last_flush_ms = self.segments.last().map(|seg| seg.end_ms).unwrap_or(0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_core::store::{JobStatus, RESUME_UNAVAILABLE_PREFIX};

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("suramak-journal-unit-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn synthetic_segment(idx: u32) -> Segment {
        Segment {
            idx,
            start_ms: i64::from(idx) * 1_000,
            end_ms: (i64::from(idx) + 1) * 1_000,
            text: format!("spoken {idx:02}"),
        }
    }

    fn record(source: &Path, model: &Path) -> JobRecord {
        let now = Utc::now();
        JobRecord {
            id: Uuid::new_v4(),
            path: source.display().to_string(),
            model: model.display().to_string(),
            status: JobStatus::Transcribing,
            created_at: now,
            updated_at: now,
            transcript_id: None,
            last_committed_segment: 0,
            last_committed_ms: 0,
            resumed: false,
            failure: None,
        }
    }

    fn durable_job(record: &JobRecord) -> DurableJob {
        DurableJob {
            job_id: record.id,
            transcript_id: Uuid::new_v4(),
            created_at: record.created_at,
            title: "lecture.wav".to_string(),
            source: Source::File {
                name: "lecture.wav".to_string(),
            },
            model: "tiny".to_string(),
            language: Some("en".to_string()),
            audio_path: Some(PathBuf::from("/audio/lecture.wav")),
            committed: Vec::new(),
            resume: None,
        }
    }

    /// The documented rule: N segments **or** ~5 s of audio, whichever comes
    /// first.
    #[test]
    fn the_checkpoint_policy_fires_on_the_segment_count_or_the_audio_span() {
        let policy = CheckpointPolicy::default();
        assert!(!policy.due(1, 1_000));
        assert!(!policy.due(CHECKPOINT_SEGMENTS - 1, 4_999));
        assert!(policy.due(CHECKPOINT_SEGMENTS, 100));
        assert!(policy.due(1, CHECKPOINT_AUDIO_MS));
        assert!(policy.due(1, CHECKPOINT_AUDIO_MS + 3_000));
        // A zero/one policy can never loop forever waiting for a segment.
        let eager = CheckpointPolicy {
            every_segments: 0,
            every_ms: 0,
        };
        assert!(eager.due(1, 0));
    }

    /// The resume arithmetic on a synthetic 30-segment job: a crash after
    /// segment 12 resumes at 12 s / index 12, and the finished text is exactly
    /// the uninterrupted run's — no duplicate, no gap, byte-equal.
    #[test]
    fn a_30_segment_job_resumes_from_segment_12_without_duplicates_or_gaps() {
        let store = Store::open_in_memory().unwrap();
        let dir = scratch_dir("resume");
        let source = dir.join("lecture.wav");
        let model = dir.join("ggml-tiny.bin");
        std::fs::write(&source, b"audio").unwrap();
        std::fs::write(&model, b"model").unwrap();

        let record = record(&source, &model);
        store.insert_job(&record).unwrap();
        let job = durable_job(&record);
        store
            .set_job_status(record.id, JobStatus::Transcribing, None)
            .unwrap();

        // The interrupted run: twelve segments reach the store, then the process
        // dies (the writer is dropped without a finish).
        let mut writer = CheckpointWriter::new(job.clone(), CheckpointPolicy::default());
        for idx in 0..12 {
            writer.push(synthetic_segment(idx));
        }
        writer.flush(&store).unwrap();
        drop(writer);
        let interrupted = store.job(record.id).unwrap().unwrap();
        assert_eq!(interrupted.last_committed_segment, 12);
        assert_eq!(interrupted.last_committed_ms, 12_000);

        // A fresh launch reads the journal and plans the continuation.
        let plan = plan_resume(&store, &interrupted, &model).unwrap();
        let ResumePlan::Resume {
            committed,
            from_ms,
            next_idx,
        } = plan
        else {
            panic!("expected a resume, got {plan:?}");
        };
        assert_eq!(from_ms, 12_000);
        assert_eq!(next_idx, 12);
        assert_eq!(committed.len(), 12);
        assert_eq!(committed.last().unwrap().text, "spoken 11");

        // The resumed run produces only the remaining window.
        let resumed_job = DurableJob {
            committed: committed.clone(),
            resume: Some(Resume { from_ms, next_idx }),
            ..job.clone()
        };
        let mut writer = CheckpointWriter::new(resumed_job, CheckpointPolicy::default());
        for idx in next_idx..30 {
            let segment = synthetic_segment(idx);
            if writer.push(segment) {
                writer.flush(&store).unwrap();
            }
        }
        writer.flush(&store).unwrap();
        let full = writer.segments().to_vec();
        let transcript = job.transcript(full.clone());
        store.finish_job(record.id, &transcript, &[]).unwrap();

        // No duplicate, no gap: 0..29 exactly once.
        let stored = store.get_transcript(job.transcript_id).unwrap().segments;
        assert_eq!(stored.len(), 30);
        for (index, segment) in stored.iter().enumerate() {
            assert_eq!(segment.idx as usize, index);
            assert_eq!(segment.text, format!("spoken {index:02}"));
            assert_eq!(segment.start_ms, index as i64 * 1_000);
        }
        let durable = store.job(record.id).unwrap().unwrap();
        assert_eq!(durable.status, JobStatus::Done);
        assert_eq!(durable.last_committed_segment, 30);

        // Byte-equal to an uninterrupted run's segment list.
        let uninterrupted: Vec<Segment> = (0..30).map(synthetic_segment).collect();
        assert_eq!(
            serde_json::to_vec(&stored).unwrap(),
            serde_json::to_vec(&uninterrupted).unwrap(),
            "the resumed transcript must be byte-equal to the uninterrupted one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A source that disappeared is a named refusal, not a silent drop.
    #[test]
    fn a_missing_source_is_resume_unavailable() {
        let store = Store::open_in_memory().unwrap();
        let dir = scratch_dir("missing-source");
        let model = dir.join("ggml-tiny.bin");
        std::fs::write(&model, b"model").unwrap();
        let record = record(&dir.join("gone.wav"), &model);

        let plan = plan_resume(&store, &record, &model).unwrap();
        match plan {
            ResumePlan::Unavailable { reason } => {
                assert!(reason.contains("source file is gone"), "{reason}");
                store.fail_job_unavailable(record.id, &reason).unwrap();
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A model that disappeared is equally named.
    #[test]
    fn a_missing_model_is_resume_unavailable() {
        let store = Store::open_in_memory().unwrap();
        let dir = scratch_dir("missing-model");
        let source = dir.join("lecture.wav");
        std::fs::write(&source, b"audio").unwrap();
        let model = dir.join("ggml-tiny.bin");
        let record = record(&source, &model);

        let plan = plan_resume(&store, &record, &model).unwrap();
        match plan {
            ResumePlan::Unavailable { reason } => {
                assert!(reason.contains("model is gone"), "{reason}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The named failure keeps the `resume_unavailable:` prefix the product
    /// reports.
    #[test]
    fn the_named_failure_carries_the_resume_unavailable_prefix() {
        let store = Store::open_in_memory().unwrap();
        let dir = scratch_dir("named");
        let model = dir.join("ggml-tiny.bin");
        let record = record(&dir.join("gone.wav"), &model);
        store.insert_job(&record).unwrap();

        let plan = plan_resume(&store, &record, &model).unwrap();
        let ResumePlan::Unavailable { reason } = plan else {
            panic!("expected Unavailable");
        };
        store.fail_job_unavailable(record.id, &reason).unwrap();
        let row = store.job(record.id).unwrap().unwrap();
        assert_eq!(row.status, JobStatus::Failed);
        assert!(row
            .failure
            .as_deref()
            .unwrap()
            .starts_with(RESUME_UNAVAILABLE_PREFIX));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cancelled run keeps every segment that reached the writer — including
    /// the last partial batch — because those segments are real text.
    #[test]
    fn a_cancelled_writer_flushes_its_partial_batch_and_keeps_it() {
        let store = Store::open_in_memory().unwrap();
        let dir = scratch_dir("cancel");
        let source = dir.join("lecture.wav");
        let model = dir.join("ggml-tiny.bin");
        std::fs::write(&source, b"audio").unwrap();
        std::fs::write(&model, b"model").unwrap();
        let record = record(&source, &model);
        store.insert_job(&record).unwrap();
        let job = durable_job(&record);
        let transcript_id = job.transcript_id;

        let mut writer = CheckpointWriter::new(job, CheckpointPolicy::default());
        writer.push(synthetic_segment(0));
        writer.push(synthetic_segment(1));
        assert!(writer.has_pending());
        writer.flush(&store).unwrap();
        store
            .set_job_status(record.id, JobStatus::Cancelled, None)
            .unwrap();

        let row = store.job(record.id).unwrap().unwrap();
        assert_eq!(row.status, JobStatus::Cancelled);
        assert_eq!(row.last_committed_segment, 2);
        assert_eq!(
            store.get_transcript(transcript_id).unwrap().segments.len(),
            2
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
