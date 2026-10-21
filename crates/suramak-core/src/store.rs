use chrono::Utc;
use rusqlite::{params, Connection};
use std::path::Path;
use uuid::Uuid;

use crate::transcript::{Segment, Transcript};
use crate::SuramakError;

/// SQLite-backed transcript library with WAL mode.
pub struct Store {
    conn: Connection,
}

/// What one `PRAGMA wal_checkpoint(PASSIVE)` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checkpoint {
    /// Whether a reader held the checkpoint back (true = incomplete).
    pub busy: bool,
    /// Frames in the WAL log when the checkpoint started.
    pub log_frames: i64,
    /// Frames copied into the database file by this call.
    pub checkpointed_frames: i64,
}

impl Store {
    /// Opens (or creates) the database at `path` and runs migrations.
    pub fn open(path: &Path) -> Result<Self, SuramakError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    /// Opens an in-memory database (for tests).
    pub fn open_in_memory() -> Result<Self, SuramakError> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    fn init(&self) -> Result<(), SuramakError> {
        self.conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS meta (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS transcripts (
                 id          TEXT PRIMARY KEY,
                 title       TEXT NOT NULL,
                 created_at  TEXT NOT NULL,
                 updated_at  TEXT NOT NULL,
                 source_json TEXT NOT NULL,
                 model       TEXT NOT NULL,
                 language    TEXT,
                 audio_path  TEXT
             );
             CREATE TABLE IF NOT EXISTS segments (
                 transcript_id TEXT    NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
                 idx           INTEGER NOT NULL,
                 start_ms      INTEGER NOT NULL,
                 end_ms        INTEGER NOT NULL,
                 text          TEXT    NOT NULL,
                 PRIMARY KEY (transcript_id, idx)
             );
             INSERT OR IGNORE INTO meta(key, value) VALUES ('schema_version', '1');",
        )?;
        Ok(())
    }

    /// Inserts a new transcript (header + segments) atomically.
    pub fn insert_transcript(&self, t: &Transcript) -> Result<(), SuramakError> {
        let source_json = serde_json::to_string(&t.source)?;
        let audio_path = t
            .audio_path
            .as_ref()
            .and_then(|p| p.to_str())
            .map(|s| s.to_string());

        // Header and segments land atomically: a crash mid-insert must not
        // leave a header row with a partial segment set (autocommit would
        // commit each statement independently).
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO transcripts(id, title, created_at, updated_at, source_json, model, language, audio_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                t.id.to_string(),
                t.title,
                t.created_at.to_rfc3339(),
                t.updated_at.to_rfc3339(),
                source_json,
                t.model,
                t.language,
                audio_path,
            ],
        )?;
        // The row writer only: the caller's `updated_at` is authoritative on
        // insert, so the public `upsert_segments` wrapper is deliberately bypassed.
        self.write_segments(t.id, &t.segments)?; // same connection → joins the transaction
        tx.commit()?;
        Ok(())
    }

    /// Upserts the complete segment set for a transcript, replacing what was there.
    ///
    /// Callers must pass the FULL list: rows with `idx >= segments.len()` are
    /// deleted, so a re-transcribe or polish that shrinks the transcript leaves
    /// no stale tail behind (stale rows would resurface as garbled text when
    /// the segments are read back in idx order).
    ///
    /// Self-contained and atomic (SUR2-NEW-05): called directly by the CLI's
    /// polish command, it runs the inserts, the stale-tail delete and the
    /// `updated_at` touch in one savepoint instead of SQLite's per-statement
    /// autocommit — an interrupted polish leaves the previous segment set intact,
    /// and a wholesale re-polish is visible as a modified transcript in the
    /// library. A savepoint (not `BEGIN`) because this also runs inside
    /// `insert_transcript`'s transaction.
    pub fn upsert_segments(&self, id: Uuid, segments: &[Segment]) -> Result<(), SuramakError> {
        self.conn.execute_batch("SAVEPOINT upsert_segments")?;
        let written = (|| -> Result<(), SuramakError> {
            self.write_segments(id, segments)?;
            self.conn.execute(
                "UPDATE transcripts SET updated_at = ?1 WHERE id = ?2",
                params![Utc::now().to_rfc3339(), id.to_string()],
            )?;
            Ok(())
        })();
        match written {
            Ok(()) => {
                self.conn.execute_batch("RELEASE upsert_segments")?;
                Ok(())
            }
            Err(err) => {
                // Undo the partial upsert; the release pops the savepoint.
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO upsert_segments; RELEASE upsert_segments");
                Err(err)
            }
        }
    }

    /// The segment rows themselves: upsert every row, then drop the stale tail.
    fn write_segments(&self, id: Uuid, segments: &[Segment]) -> Result<(), SuramakError> {
        for seg in segments {
            self.conn.execute(
                "INSERT INTO segments(transcript_id, idx, start_ms, end_ms, text)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(transcript_id, idx) DO UPDATE SET
                     start_ms = excluded.start_ms,
                     end_ms   = excluded.end_ms,
                     text     = excluded.text",
                params![id.to_string(), seg.idx, seg.start_ms, seg.end_ms, seg.text],
            )?;
        }
        self.conn.execute(
            "DELETE FROM segments WHERE transcript_id = ?1 AND idx >= ?2",
            params![id.to_string(), segments.len() as i64],
        )?;
        Ok(())
    }

    /// Flushes the write-ahead log back into the database file.
    ///
    /// The quit path ends in `shutdown::hard_exit` (`libc::_exit`, a deliberate
    /// ggml-teardown workaround — see that module), which runs no destructors:
    /// the connection is never closed and SQLite's own close-time checkpoint
    /// never happens, so the last committed transactions would sit only in
    /// `suramak.db-wal` (SUR-MED-02). Committed data is not lost — SQLite
    /// recovers from the WAL on the next open — but a hard exit immediately
    /// after a long job otherwise leaves a multi-megabyte WAL that the next
    /// start has to replay. `PASSIVE` never blocks on readers, which is what a
    /// best-effort teardown needs.
    ///
    /// A no-op success for an in-memory or non-WAL database (both columns read −1).
    pub fn checkpoint(&self) -> Result<Checkpoint, SuramakError> {
        // `query_row`, not `execute_batch`: the pragma returns a row, and only
        // stepping it runs the checkpoint.
        let outcome = self
            .conn
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                Ok(Checkpoint {
                    busy: row.get::<_, i64>(0)? != 0,
                    log_frames: row.get(1)?,
                    checkpointed_frames: row.get(2)?,
                })
            })?;
        Ok(outcome)
    }

    /// Updates a single segment's text and touches `updated_at` on the transcript.
    pub fn update_segment_text(
        &self,
        transcript_id: Uuid,
        idx: u32,
        text: &str,
    ) -> Result<(), SuramakError> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE segments SET text = ?1 WHERE transcript_id = ?2 AND idx = ?3",
            params![text, transcript_id.to_string(), idx],
        )?;
        self.conn.execute(
            "UPDATE transcripts SET updated_at = ?1 WHERE id = ?2",
            params![now, transcript_id.to_string()],
        )?;
        Ok(())
    }

    /// Returns a transcript by id (with all segments).
    pub fn get_transcript(&self, id: Uuid) -> Result<Transcript, SuramakError> {
        let row = self.conn.query_row(
            "SELECT id, title, created_at, updated_at, source_json, model, language, audio_path
             FROM transcripts WHERE id = ?1",
            params![id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        );

        match row {
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(SuramakError::TranscriptNotFound(id.to_string()))
            }
            Err(e) => Err(SuramakError::Sqlite(e)),
            Ok((
                id_str,
                title,
                created_at,
                updated_at,
                source_json,
                model,
                language,
                audio_path,
            )) => {
                let segments = self.load_segments(
                    Uuid::parse_str(&id_str)
                        .map_err(|_| SuramakError::TranscriptNotFound(id_str.clone()))?,
                )?;
                Ok(Transcript {
                    id: Uuid::parse_str(&id_str)
                        .map_err(|_| SuramakError::TranscriptNotFound(id_str.clone()))?,
                    title,
                    created_at: created_at.parse().map_err(|_| {
                        SuramakError::Sqlite(rusqlite::Error::InvalidColumnName(
                            "created_at".into(),
                        ))
                    })?,
                    updated_at: updated_at.parse().map_err(|_| {
                        SuramakError::Sqlite(rusqlite::Error::InvalidColumnName(
                            "updated_at".into(),
                        ))
                    })?,
                    source: serde_json::from_str(&source_json)?,
                    model,
                    language,
                    audio_path: audio_path.map(std::path::PathBuf::from),
                    segments,
                })
            }
        }
    }

    fn load_segments(&self, id: Uuid) -> Result<Vec<Segment>, SuramakError> {
        let mut stmt = self.conn.prepare(
            "SELECT idx, start_ms, end_ms, text FROM segments
             WHERE transcript_id = ?1 ORDER BY idx",
        )?;
        let rows = stmt.query_map(params![id.to_string()], |row| {
            Ok(Segment {
                idx: row.get::<_, u32>(0)?,
                start_ms: row.get::<_, i64>(1)?,
                end_ms: row.get::<_, i64>(2)?,
                text: row.get::<_, String>(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(SuramakError::Sqlite)
    }

    /// Returns all transcripts (header only, no segments) ordered newest first.
    pub fn list_transcripts(&self) -> Result<Vec<Transcript>, SuramakError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, created_at, updated_at, source_json, model, language, audio_path
             FROM transcripts ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?;

        let mut result = Vec::new();
        for row in rows {
            let (id_str, title, created_at, updated_at, source_json, model, language, audio_path) =
                row.map_err(SuramakError::Sqlite)?;
            let id = Uuid::parse_str(&id_str)
                .map_err(|_| SuramakError::TranscriptNotFound(id_str.clone()))?;
            result.push(Transcript {
                id,
                title,
                created_at: created_at.parse().map_err(|_| {
                    SuramakError::Sqlite(rusqlite::Error::InvalidColumnName("created_at".into()))
                })?,
                updated_at: updated_at.parse().map_err(|_| {
                    SuramakError::Sqlite(rusqlite::Error::InvalidColumnName("updated_at".into()))
                })?,
                source: serde_json::from_str(&source_json)?,
                model,
                language,
                audio_path: audio_path.map(std::path::PathBuf::from),
                segments: Vec::new(),
            });
        }
        Ok(result)
    }

    /// Full-text search over transcript titles and segment text.
    ///
    /// The query is a **literal** substring, not a pattern (SUR-LOW-02): `%`, `_`
    /// and `\` are escaped and the statement says `ESCAPE '\'`, so searching for
    /// "100%" finds that text instead of matching every title starting with "100".
    pub fn search(&self, query: &str) -> Result<Vec<Transcript>, SuramakError> {
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT t.id, t.title, t.created_at, t.updated_at, t.source_json,
                    t.model, t.language, t.audio_path
             FROM transcripts t
             LEFT JOIN segments s ON s.transcript_id = t.id
             WHERE t.title LIKE ?1 ESCAPE '\\' OR s.text LIKE ?1 ESCAPE '\\'
             ORDER BY t.created_at DESC",
        )?;
        let rows = stmt.query_map(params![pattern], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?;

        let mut result = Vec::new();
        for row in rows {
            let (id_str, title, created_at, updated_at, source_json, model, language, audio_path) =
                row.map_err(SuramakError::Sqlite)?;
            let id = Uuid::parse_str(&id_str)
                .map_err(|_| SuramakError::TranscriptNotFound(id_str.clone()))?;
            result.push(Transcript {
                id,
                title,
                created_at: created_at.parse().map_err(|_| {
                    SuramakError::Sqlite(rusqlite::Error::InvalidColumnName("created_at".into()))
                })?,
                updated_at: updated_at.parse().map_err(|_| {
                    SuramakError::Sqlite(rusqlite::Error::InvalidColumnName("updated_at".into()))
                })?,
                source: serde_json::from_str(&source_json)?,
                model,
                language,
                audio_path: audio_path.map(std::path::PathBuf::from),
                segments: Vec::new(),
            });
        }
        Ok(result)
    }

    /// Deletes a transcript and its segments (cascade).
    pub fn delete_transcript(&self, id: Uuid) -> Result<(), SuramakError> {
        let n = self.conn.execute(
            "DELETE FROM transcripts WHERE id = ?1",
            params![id.to_string()],
        )?;
        if n == 0 {
            return Err(SuramakError::TranscriptNotFound(id.to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Source;

    fn sample(title: &str) -> Transcript {
        Transcript::new(
            title.to_string(),
            Source::File {
                name: title.to_string(),
            },
            "tiny".to_string(),
            Some("en".to_string()),
            None,
            vec![
                Segment {
                    idx: 0,
                    start_ms: 0,
                    end_ms: 900,
                    text: "first".to_string(),
                },
                Segment {
                    idx: 1,
                    start_ms: 900,
                    end_ms: 1_800,
                    text: "second".to_string(),
                },
            ],
        )
    }

    /// Direct segment-row count (the cascade is only observable inside the crate).
    fn segment_rows(store: &Store, id: Uuid) -> i64 {
        store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM segments WHERE transcript_id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .expect("count segments")
    }

    #[test]
    fn upserting_fewer_segments_deletes_the_stale_tail() {
        let store = Store::open_in_memory().expect("store");
        let mut t = sample("shrink.wav");
        store.insert_transcript(&t).unwrap();
        assert_eq!(segment_rows(&store, t.id), 2);

        // A polish pass that merges the two segments into one (same id, fewer rows).
        t.segments.truncate(1);
        t.segments[0].text = "merged".to_string();
        store.upsert_segments(t.id, &t.segments).unwrap();

        let got = store.get_transcript(t.id).unwrap();
        assert_eq!(got.segments.len(), 1, "the stale idx=1 row must be deleted");
        assert_eq!(got.segments[0].text, "merged");

        // Growing again works too (no leftover higher-idx conflicts).
        t.segments.push(Segment {
            idx: 1,
            start_ms: 0,
            end_ms: 500,
            text: "new second".to_string(),
        });
        store.upsert_segments(t.id, &t.segments).unwrap();
        let got = store.get_transcript(t.id).unwrap();
        assert_eq!(got.segments.len(), 2);
        assert_eq!(got.segments[1].text, "new second");
    }

    #[test]
    fn checkpoint_drains_the_wal_into_the_database_file() {
        // SUR-MED-02: the quit path's hard exit runs no destructors, so the WAL
        // is checkpointed explicitly instead of at connection close.
        let dir = std::env::temp_dir().join(format!("suramak-store-wal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("suramak.db");

        let store = Store::open(&path).expect("store");
        let t = sample("checkpoint.wav");
        store.insert_transcript(&t).unwrap();
        // Committed rows live in the WAL until something folds them back.
        let wal = dir.join("suramak.db-wal");
        assert!(
            std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0) > 0,
            "a commit must leave WAL frames to checkpoint"
        );

        let outcome = store.checkpoint().expect("checkpoint");
        assert!(!outcome.busy, "no reader may hold the checkpoint back");
        assert!(
            outcome.log_frames > 0,
            "the checkpoint must have seen the committed frames"
        );
        assert_eq!(
            outcome.checkpointed_frames, outcome.log_frames,
            "every WAL frame must be copied into the database file"
        );

        // A second PASSIVE checkpoint finds the WAL reset to its head and
        // nothing left that is not already in the file.
        let again = store.checkpoint().expect("second checkpoint");
        assert!(!again.busy, "no reader may hold the checkpoint back");
        assert_eq!(
            again.checkpointed_frames, again.log_frames,
            "a repeated checkpoint still copies every frame it reports"
        );

        // The rows are in the database file itself: a fresh connection reads them.
        drop(store);
        let reopened = Store::open(&path).expect("reopen");
        assert_eq!(reopened.get_transcript(t.id).unwrap().segments.len(), 2);

        // An in-memory / non-WAL database answers the pragma without failing.
        Store::open_in_memory().unwrap().checkpoint().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_standalone_upsert_touches_updated_at_and_keeps_the_insert_stamp() {
        // SUR2-NEW-05: `insert_transcript` keeps the caller's `updated_at`, but a
        // standalone upsert (the CLI polish path) is a modification and must show
        // as one in the library.
        let store = Store::open_in_memory().expect("store");
        let mut t = sample("polish.wav");
        t.updated_at = "2020-01-01T00:00:00+00:00".parse().expect("timestamp");
        store.insert_transcript(&t).unwrap();
        assert_eq!(
            store.get_transcript(t.id).unwrap().updated_at,
            t.updated_at,
            "insert must keep the caller's stamp"
        );

        t.segments.truncate(1);
        t.segments[0].text = "polished".to_string();
        std::thread::sleep(std::time::Duration::from_millis(2));
        store.upsert_segments(t.id, &t.segments).unwrap();

        let got = store.get_transcript(t.id).unwrap();
        assert_eq!(got.segments.len(), 1);
        assert_eq!(got.segments[0].text, "polished");
        assert!(
            got.updated_at > t.updated_at,
            "a standalone upsert must touch updated_at (got {})",
            got.updated_at
        );
    }

    #[test]
    fn a_failed_standalone_upsert_rolls_back_to_the_previous_segments() {
        // SUR2-NEW-05: the insert loop and the stale-tail delete are one unit of
        // work, so a failure half-way cannot leave a rewritten first row behind.
        let store = Store::open_in_memory().expect("store");
        let t = sample("upsert-rollback.wav");
        store.insert_transcript(&t).unwrap();

        // Abort the *second* row: the first one has already been written when the
        // error fires.
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER refuse_idx_one BEFORE INSERT ON segments
                 WHEN NEW.idx = 1 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();

        let mut replacement = t.segments.clone();
        replacement[0].text = "rewritten".to_string();
        replacement[1].text = "doomed".to_string();
        let err = store
            .upsert_segments(t.id, &replacement)
            .expect_err("the trigger must abort the upsert");
        assert!(format!("{err}").contains("boom"), "unexpected error: {err}");

        let got = store.get_transcript(t.id).unwrap();
        assert_eq!(got.segments.len(), 2, "the previous rows must all survive");
        assert_eq!(
            got.segments[0].text, "first",
            "the row written before the failure must be rolled back"
        );
        assert_eq!(got.segments[1].text, "second");
    }

    #[test]
    fn foreign_keys_are_enforced_on_this_connection() {
        // The schema relies on ON DELETE CASCADE; SQLite ignores it unless
        // `PRAGMA foreign_keys = ON` ran on the connection (init does).
        let store = Store::open_in_memory().expect("store");
        let t = sample("cascade.wav");
        store.insert_transcript(&t).unwrap();
        assert_eq!(segment_rows(&store, t.id), 2);

        store
            .conn
            .execute(
                "DELETE FROM transcripts WHERE id = ?1",
                params![t.id.to_string()],
            )
            .unwrap();
        assert_eq!(segment_rows(&store, t.id), 0, "cascade must have fired");
    }

    #[test]
    fn delete_transcript_removes_its_segments_and_spares_the_others() {
        let store = Store::open_in_memory().expect("store");
        let doomed = sample("doomed.wav");
        let survivor = sample("survivor.wav");
        store.insert_transcript(&doomed).unwrap();
        store.insert_transcript(&survivor).unwrap();
        assert_eq!(segment_rows(&store, doomed.id), 2);

        store.delete_transcript(doomed.id).unwrap();

        // The header row is gone…
        assert!(store.get_transcript(doomed.id).is_err());
        // …and so are its segments (the schema's ON DELETE CASCADE, enforced).
        assert_eq!(
            segment_rows(&store, doomed.id),
            0,
            "deleting a transcript must cascade to its segments"
        );

        // A random other transcript survives intact, header + segments.
        let kept = store
            .get_transcript(survivor.id)
            .expect("survivor readable");
        assert_eq!(kept.segments.len(), 2);
        assert_eq!(segment_rows(&store, survivor.id), 2);
        assert_eq!(store.list_transcripts().unwrap().len(), 1);

        // Deleting again reports not-found instead of silently succeeding.
        assert!(store.delete_transcript(doomed.id).is_err());
    }
}
