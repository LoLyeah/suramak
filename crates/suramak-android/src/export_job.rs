//! M13g2 — the compact export job: render → bounded atomic write → hand-off.
//!
//! The phone's export sheet does not format anything itself. Every byte comes
//! from `suramak_core::export`, the same functions the desktop's export sheet
//! and the CLI call, including the stacked bilingual SRT/VTT paths. This module
//! is the **job** around that render:
//!
//! ```text
//! stage()   render (the M15 serializer) → <files>/exports/<job>/<name>.part
//!           → write in bounded chunks → fsync → rename to <name>
//!           → report {job, path, name, bytes, checksum, mime}
//! Kotlin    copy that one file into the ACTION_CREATE_DOCUMENT frame
//!           (bounded buffer, read-back length check) or hand it to ACTION_SEND
//! finish()  verify the byte count the platform wrote, then delete the staging
//!           directory; a mismatch is an error, never a silent success
//! ```
//!
//! **Why stage at all.** SAF gives the app a write-once stream: a failed write
//! cannot be retried on the same URI and a partial write is visible to the
//! provider. Staging first makes the file's bytes final before the platform
//! stream is opened, so the picker path and the share path deliver identical
//! content and a cancelled share leaves the library untouched.
//!
//! The job is bound to its transcript id and is **single-flight**: starting a
//! second export while one is live is refused rather than queued (the sheet is
//! one action at a time; a queue would hide a stuck job).

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use suramak_core::export::{self, ExportSpeakers};
use suramak_core::transcript::Transcript;
use suramak_core::translate::TranslationRecord;
use suramak_ui::deck::compact::CompactExportFormat;
use uuid::Uuid;

/// The writer's bounded buffer. One read of the rendered string at a time; the
/// file is never built twice in memory.
pub const EXPORT_CHUNK_BYTES: usize = 16 * 1024;

/// A staged export, ready to be copied into the destination the user picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedExport {
    /// The job id (`export_done` must quote it).
    pub job: String,
    /// The transcript the job is bound to.
    pub transcript: Uuid,
    /// The file name inside the staging directory (also the picker suggestion).
    pub name: String,
    /// Absolute path of the staged file.
    pub path: PathBuf,
    pub bytes: u64,
    /// FNV-1a 64 of the exact file bytes, hex — the platform side recomputes it
    /// while copying, so a truncated or altered transfer is caught.
    pub checksum: String,
    pub mime: &'static str,
    pub bilingual: bool,
    /// M23A: whether the bytes carry speaker prefixes.
    pub speakers: bool,
}

impl StagedExport {
    /// The Kotlin `export_done` checks this against the bytes it wrote.
    pub fn matches(&self, written_bytes: u64) -> bool {
        written_bytes == self.bytes
    }
}

/// The single-flight export host, owned by the compact UI host.
pub struct ExportJobs {
    root: PathBuf,
    live: Mutex<Option<StagedExport>>,
}

impl ExportJobs {
    /// `root` is the app's files directory; staging lives under `exports/`.
    /// Stale staging directories from a previous process are pruned here — a
    /// killed share leaves a file that is no longer referenceable.
    pub fn new(root: &Path) -> Self {
        let jobs = Self {
            root: root.join("exports"),
            live: Mutex::new(None),
        };
        jobs.prune();
        jobs
    }

    /// Removes every staging directory not held by the live job. Returns how
    /// many were removed.
    pub fn prune(&self) -> usize {
        let live = self
            .live
            .lock()
            .ok()
            .and_then(|live| live.as_ref().map(|job| job.job.clone()));
        let Ok(entries) = fs::read_dir(&self.root) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if live.as_deref() == path.file_name().and_then(|name| name.to_str()) {
                continue;
            }
            if fs::remove_dir_all(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    /// Renders and stages one export.
    #[allow(clippy::too_many_arguments)]
    pub fn stage(
        &self,
        transcript: &Transcript,
        translations: &[(u32, TranslationRecord)],
        format: CompactExportFormat,
        bilingual: bool,
        speakers: Option<&ExportSpeakers>,
        name: &str,
    ) -> Result<StagedExport, String> {
        // The lock is held across the render and the write: the host serialises
        // its commands anyway, and this makes the single-flight promise true
        // even if a future caller fans out.
        let mut live = self.live.lock().map_err(|_| "export lock poisoned")?;
        if let Some(current) = live.as_ref() {
            return Err(format!("an export is already in flight ({})", current.name));
        }
        let text = render(transcript, translations, format, bilingual, speakers)?;
        let job = Uuid::new_v4().to_string();
        let dir = self.root.join(&job);
        fs::create_dir_all(&dir)
            .map_err(|err| format!("cannot create the export staging dir: {err}"))?;
        let final_path = dir.join(name);
        let part_path = dir.join(format!("{name}.part"));
        write_atomic(&part_path, &final_path, text.as_bytes())?;
        let bytes = fs::metadata(&final_path)
            .map_err(|err| format!("cannot stat the staged export: {err}"))?
            .len();
        let staged = StagedExport {
            job,
            transcript: transcript.id,
            name: name.to_string(),
            path: final_path,
            bytes,
            checksum: checksum_hex(fnv1a64(text.as_bytes())),
            mime: format.mime(),
            bilingual,
            speakers: speakers.is_some(),
        };
        *live = Some(staged.clone());
        Ok(staged)
    }

    /// The platform wrote `written_bytes` into the user's frame. The count must
    /// match the staged file exactly; on success the staging directory is
    /// removed (the user's copy is the only one left).
    pub fn finish(&self, job: &str, written_bytes: u64) -> Result<StagedExport, String> {
        let mut live = self.live.lock().map_err(|_| "export lock poisoned")?;
        let Some(staged) = live.take() else {
            return Err("no export is in flight".to_string());
        };
        if staged.job != job {
            *live = Some(staged);
            return Err("that export job is not the live one".to_string());
        }
        if !staged.matches(written_bytes) {
            // Put it back: the user can retry the same staged bytes.
            let expected = staged.bytes;
            *live = Some(staged);
            return Err(format!(
                "the destination has {written_bytes} bytes; the export is {expected}"
            ));
        }
        let _ = fs::remove_dir_all(staged.path.parent().unwrap_or(Path::new(".")));
        Ok(staged)
    }

    /// `ACTION_SEND` has the URI: the job is done, but the staged file must
    /// outlive this call — the receiving app reads it after the chooser closes.
    /// It is cleaned by the next process's [`ExportJobs::prune`].
    pub fn release_for_share(&self, job: &str) -> Result<StagedExport, String> {
        let mut live = self.live.lock().map_err(|_| "export lock poisoned")?;
        let Some(staged) = live.take() else {
            return Err("no export is in flight".to_string());
        };
        if staged.job != job {
            *live = Some(staged);
            return Err("that export job is not the live one".to_string());
        }
        Ok(staged)
    }

    /// The picker was dismissed (or the share chooser returned): drop the job
    /// and its staging directory. Never an error — an abandoned export is not
    /// a failure.
    pub fn cancel(&self, job: &str) {
        let Ok(mut live) = self.live.lock() else {
            return;
        };
        let Some(staged) = live.take() else {
            return;
        };
        if staged.job == job {
            let _ = fs::remove_dir_all(staged.path.parent().unwrap_or(Path::new(".")));
        } else {
            *live = Some(staged);
        }
    }

    /// Whether a job is live (the host's status line / tests).
    pub fn live(&self) -> Option<StagedExport> {
        self.live.lock().ok().and_then(|live| live.clone())
    }
}

/// The one render function both the picker path and the share path call.
///
/// Bilingual is only meaningful for the stacked subtitle formats and only when
/// the transcript actually has translations; both are refused with an honest
/// line rather than silently exporting one language.
pub fn render(
    transcript: &Transcript,
    translations: &[(u32, TranslationRecord)],
    format: CompactExportFormat,
    bilingual: bool,
    speakers: Option<&ExportSpeakers>,
) -> Result<String, String> {
    if bilingual {
        if !format.supports_bilingual() {
            return Err(format!(
                "{} has no bilingual variant — choose SRT or VTT",
                format.label()
            ));
        }
        if translations.is_empty() {
            return Err("this transcript has no translation to include".to_string());
        }
    }
    // M23A: speakers ride the same serializers. A format without a speaker
    // variant is refused loudly rather than exporting a silently plain file.
    if speakers.is_some() && !format.supports_speakers() {
        return Err(format!("{} has no speaker variant", format.label()));
    }
    let rendered = match (format, bilingual) {
        (CompactExportFormat::Md, _) => export::to_markdown_with_speakers(transcript, speakers),
        (CompactExportFormat::Txt, _) => export::to_txt(transcript),
        (CompactExportFormat::Json, _) => export::to_json(transcript),
        (CompactExportFormat::Srt, false) => export::to_srt_with_speakers(transcript, speakers),
        (CompactExportFormat::Srt, true) => {
            export::to_srt_bilingual_with_speakers(transcript, translations, speakers)
        }
        (CompactExportFormat::Vtt, false) => export::to_vtt_with_speakers(transcript, speakers),
        (CompactExportFormat::Vtt, true) => {
            export::to_vtt_bilingual_with_speakers(transcript, translations, speakers)
        }
        (CompactExportFormat::Tsv, _) => export::to_tsv(transcript),
    };
    rendered.map_err(|err| err.to_string())
}

/// Writes `bytes` through a `.part` file in bounded chunks, syncs it, then
/// renames it into place. A reader never sees a partial file.
fn write_atomic(part: &Path, final_path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file =
        File::create(part).map_err(|err| format!("cannot create {}: {err}", part.display()))?;
    for chunk in bytes.chunks(EXPORT_CHUNK_BYTES) {
        file.write_all(chunk)
            .map_err(|err| format!("cannot write the staged export: {err}"))?;
    }
    file.sync_all()
        .map_err(|err| format!("cannot flush the staged export: {err}"))?;
    drop(file);
    fs::rename(part, final_path).map_err(|err| format!("cannot finalise the staged export: {err}"))
}

/// FNV-1a 64 — a small, dependency-free content hash (not cryptography; the
/// job only needs to detect a truncated or altered transfer).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn checksum_hex(hash: u64) -> String {
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_core::transcript::{Segment, Source};

    fn transcript() -> Transcript {
        Transcript::new(
            "Voice note".to_string(),
            Source::File {
                name: "note.m4a".to_string(),
            },
            "base".to_string(),
            Some("en".to_string()),
            None,
            vec![
                Segment {
                    idx: 0,
                    start_ms: 0,
                    end_ms: 2_000,
                    text: "Hello world.".to_string(),
                },
                Segment {
                    idx: 1,
                    start_ms: 2_000,
                    end_ms: 4_000,
                    text: "Second line.".to_string(),
                },
            ],
        )
    }

    fn translations() -> Vec<(u32, TranslationRecord)> {
        vec![
            (
                0,
                TranslationRecord::new("Halo dunia.", "m", "2026-01-01T00:00:00Z", vec![0]),
            ),
            (
                1,
                TranslationRecord::new("Baris kedua.", "m", "2026-01-01T00:00:00Z", vec![1]),
            ),
        ]
    }

    #[test]
    fn every_format_stages_exactly_the_core_serializer_bytes() {
        let dir = std::env::temp_dir().join(format!("suramak-export-{}", Uuid::new_v4()));
        let jobs = ExportJobs::new(&dir);
        let t = transcript();
        for format in CompactExportFormat::ALL {
            let expected = render(&t, &[], format, false, None).expect("render");
            let staged = jobs
                .stage(
                    &t,
                    &[],
                    format,
                    false,
                    None,
                    &format!("note.{}", format.extension()),
                )
                .expect("stage");
            let bytes = fs::read(&staged.path).expect("read staged");
            assert_eq!(bytes, expected.as_bytes(), "{format:?} byte parity");
            assert_eq!(staged.bytes, expected.len() as u64);
            assert_eq!(staged.checksum, checksum_hex(fnv1a64(expected.as_bytes())));
            assert_eq!(staged.mime, format.mime());
            jobs.finish(&staged.job, staged.bytes).expect("finish");
            assert!(!staged.path.exists(), "the staging dir is cleaned");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bilingual_staging_matches_the_stacked_serializer() {
        let dir = std::env::temp_dir().join(format!("suramak-export-{}", Uuid::new_v4()));
        let jobs = ExportJobs::new(&dir);
        let t = transcript();
        let expected = export::to_srt_bilingual(&t, &translations()).expect("render");
        let staged = jobs
            .stage(
                &t,
                &translations(),
                CompactExportFormat::Srt,
                true,
                None,
                "note.srt",
            )
            .expect("stage");
        assert_eq!(fs::read(&staged.path).expect("read"), expected.as_bytes());
        assert!(staged.bilingual);
        jobs.finish(&staged.job, staged.bytes).expect("finish");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bilingual_is_refused_for_a_flat_format_or_missing_translation() {
        let t = transcript();
        assert!(render(&t, &translations(), CompactExportFormat::Md, true, None).is_err());
        assert!(render(&t, &[], CompactExportFormat::Srt, true, None).is_err());
    }

    #[test]
    fn a_byte_count_mismatch_fails_and_keeps_the_job_retryable() {
        let dir = std::env::temp_dir().join(format!("suramak-export-{}", Uuid::new_v4()));
        let jobs = ExportJobs::new(&dir);
        let t = transcript();
        let staged = jobs
            .stage(&t, &[], CompactExportFormat::Txt, false, None, "note.txt")
            .expect("stage");
        let err = jobs
            .finish(&staged.job, staged.bytes - 1)
            .expect_err("a short write must fail");
        assert!(err.contains("bytes"));
        assert!(jobs.live().is_some(), "the staged bytes stay for a retry");
        assert!(jobs.finish(&staged.job, staged.bytes).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_speaker_staging_carries_the_prefixes_and_flags_it() {
        let dir = std::env::temp_dir().join(format!("suramak-export-{}", Uuid::new_v4()));
        let jobs = ExportJobs::new(&dir);
        let t = transcript();
        let speakers = ExportSpeakers::new(
            vec![(0, "Budi".to_string())],
            vec![suramak_core::diarization::SpeakerTurn::new(0, 0, 4_000, 0)],
        );
        let staged = jobs
            .stage(
                &t,
                &[],
                CompactExportFormat::Srt,
                false,
                Some(&speakers),
                "note.srt",
            )
            .expect("stage");
        let text = fs::read_to_string(&staged.path).expect("read");
        assert!(text.contains("Budi: Hello world."), "{text}");
        assert!(staged.speakers);
        jobs.finish(&staged.job, staged.bytes).expect("finish");
        // A machine format refuses the request instead of dropping it.
        assert!(render(&t, &[], CompactExportFormat::Json, false, Some(&speakers)).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_export_while_one_is_live_is_refused() {
        let dir = std::env::temp_dir().join(format!("suramak-export-{}", Uuid::new_v4()));
        let jobs = ExportJobs::new(&dir);
        let t = transcript();
        let first = jobs
            .stage(&t, &[], CompactExportFormat::Txt, false, None, "note.txt")
            .expect("first");
        let err = jobs
            .stage(&t, &[], CompactExportFormat::Txt, false, None, "again.txt")
            .expect_err("single-flight");
        assert!(err.contains("in flight"));
        jobs.cancel(&first.job);
        assert!(jobs.live().is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
