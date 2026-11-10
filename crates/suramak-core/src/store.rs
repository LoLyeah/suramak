use chrono::Utc;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::path::Path;
use uuid::Uuid;

use crate::transcript::{Segment, Transcript};
use crate::SuramakError;

/// The library schema this build writes and reads.
///
/// - **v1** — the schema shipped in app 0.3.0/0.3.1 (`meta.schema_version = 1`):
///   transcript header + segments, no provenance column.
/// - **v2** — M15.1, additive only: `segments.source_text` keeps the first text
///   ever stored for a row (the inference output), while `segments.text` carries
///   what the editor/polish may rewrite. No table is dropped or rebuilt.
pub const SCHEMA_VERSION: i64 = 2;

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
                 source_text   TEXT,
                 PRIMARY KEY (transcript_id, idx)
             );",
        )?;
        self.migrate()
    }

    /// Brings an existing library up to [`SCHEMA_VERSION`], in place.
    ///
    /// Only one migration exists today: v1 → v2 adds `segments.source_text` and
    /// backfills it from `text` (the only copy of the inference output a v1
    /// database has). It is additive — no table is rebuilt and no row is
    /// deleted — and idempotent: a fresh database is created at v2, a v1
    /// database is altered once, and a v2 database is left untouched.
    ///
    /// A database from a *newer* build is refused instead of guessed at: the
    /// older binary must not write to a schema it does not understand.
    fn migrate(&self) -> Result<(), SuramakError> {
        let version: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;

        let version = match version {
            None => {
                // Fresh database (the DDL above already created v2) or a very
                // old one without the marker: `upgrade` is idempotent, so both
                // land on v2 with the column present and backfilled.
                self.upgrade_to_v2()?;
                return Ok(());
            }
            Some(raw) => raw.parse::<i64>().map_err(|_| {
                SuramakError::Schema(format!(
                    "meta.schema_version is not a number ({raw:?}); refusing to guess"
                ))
            })?,
        };

        if version > SCHEMA_VERSION {
            return Err(SuramakError::Schema(format!(
                "database is schema v{version}, this build supports up to v{SCHEMA_VERSION} — \
                 open it with the newer Suramak build"
            )));
        }
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        self.upgrade_to_v2()
    }

    /// v0/v1 → v2, in one transaction: add `source_text` when absent, backfill it
    /// from `text`, then stamp the version. An interrupted upgrade rolls back to
    /// the untouched v1 database.
    fn upgrade_to_v2(&self) -> Result<(), SuramakError> {
        let tx = self.conn.unchecked_transaction()?;
        if !Self::column_exists(&tx, "segments", "source_text")? {
            tx.execute_batch("ALTER TABLE segments ADD COLUMN source_text TEXT")?;
        }
        tx.execute(
            "UPDATE segments SET source_text = text WHERE source_text IS NULL",
            [],
        )?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![SCHEMA_VERSION.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Whether `table` has `column` (the upgrade's idempotence guard).
    fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, SuramakError> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The schema version the open database reports (after migration: [`SCHEMA_VERSION`]).
    pub fn schema_version(&self) -> Result<i64, SuramakError> {
        let value: String = self.conn.query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )?;
        value
            .parse()
            .map_err(|_| SuramakError::Schema(format!("schema_version is not a number: {value:?}")))
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
    ///
    /// `source_text` is written only when the row does not exist yet and is
    /// never overwritten afterwards (`COALESCE(segments.source_text, …)`): a
    /// polish pass or an editor save rewrites `text`, while the first stored
    /// text — the inference output — stays readable as provenance.
    fn write_segments(&self, id: Uuid, segments: &[Segment]) -> Result<(), SuramakError> {
        for seg in segments {
            self.conn.execute(
                "INSERT INTO segments(transcript_id, idx, start_ms, end_ms, text, source_text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(transcript_id, idx) DO UPDATE SET
                     start_ms    = excluded.start_ms,
                     end_ms      = excluded.end_ms,
                     text        = excluded.text,
                     source_text = COALESCE(segments.source_text, excluded.source_text)",
                params![id.to_string(), seg.idx, seg.start_ms, seg.end_ms, seg.text],
            )?;
        }
        self.conn.execute(
            "DELETE FROM segments WHERE transcript_id = ?1 AND idx >= ?2",
            params![id.to_string(), segments.len() as i64],
        )?;
        Ok(())
    }

    /// The first text ever stored for a segment (the inference output), even
    /// after the editor or a polish pass rewrote `text`.
    ///
    /// `None` when the transcript or the index does not exist. A v1 database
    /// upgraded by [`Store::open`] answers with the text it carried at upgrade
    /// time — the only copy that ever existed there.
    pub fn segment_source_text(
        &self,
        transcript_id: Uuid,
        idx: u32,
    ) -> Result<Option<String>, SuramakError> {
        let text = self
            .conn
            .query_row(
                "SELECT source_text FROM segments WHERE transcript_id = ?1 AND idx = ?2",
                params![transcript_id.to_string(), idx],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(text.flatten())
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
    ///
    /// The two statements are one savepoint: a failure after the text write (a
    /// constraint, a crash) must not leave the row rewritten while the
    /// transcript still claims its old `updated_at`. The caller's text is the
    /// *current* text; `source_text` is deliberately not touched (M15.1).
    pub fn update_segment_text(
        &self,
        transcript_id: Uuid,
        idx: u32,
        text: &str,
    ) -> Result<(), SuramakError> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute_batch("SAVEPOINT update_segment_text")?;
        let written = (|| -> Result<(), SuramakError> {
            self.conn.execute(
                "UPDATE segments SET text = ?1 WHERE transcript_id = ?2 AND idx = ?3",
                params![text, transcript_id.to_string(), idx],
            )?;
            self.conn.execute(
                "UPDATE transcripts SET updated_at = ?1 WHERE id = ?2",
                params![now, transcript_id.to_string()],
            )?;
            Ok(())
        })();
        match written {
            Ok(()) => {
                self.conn.execute_batch("RELEASE update_segment_text")?;
                Ok(())
            }
            Err(err) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO update_segment_text; RELEASE update_segment_text");
                Err(err)
            }
        }
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

    /// SQLite's own integrity report for the open database (`"ok"` when sound).
    pub fn integrity_check(&self) -> Result<String, SuramakError> {
        let report: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        Ok(report)
    }

    /// Writes a consistent, single-file snapshot of the library to `dest`.
    ///
    /// `VACUUM INTO` reads through the live connection (WAL included) inside one
    /// read transaction, so the copy is a coherent database even while the app
    /// keeps writing; it also compacts the file and leaves no `-wal`/`-shm`
    /// sidecars behind. `dest` must not exist yet — an existing file is refused
    /// rather than silently overwritten.
    ///
    /// This is a library API (M15.1); no UI surface calls it yet.
    pub fn backup_to(&self, dest: &Path) -> Result<(), SuramakError> {
        if dest.exists() {
            return Err(SuramakError::Backup(format!(
                "refusing to overwrite the existing backup {}",
                dest.display()
            )));
        }
        if let Some(parent) = dest.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let dest = dest.to_str().ok_or_else(|| {
            SuramakError::Backup("the backup path is not valid UTF-8".to_string())
        })?;
        self.conn.execute("VACUUM INTO ?1", params![dest])?;
        Ok(())
    }

    /// Restores `backup` over `db`, atomically, after validating the backup.
    ///
    /// The caller must have **no open [`Store`]** for `db`: replace a database
    /// file under a live connection and SQLite would keep writing to the old
    /// inode. The sequence is: open the backup read-only, run
    /// `PRAGMA integrity_check`, refuse a schema newer than this build, copy to
    /// a sibling temporary file, fsync it, drop the target's stale `-wal`/`-shm`
    /// sidecars, then rename the temporary file over the target (atomic on one
    /// filesystem). A failure before the rename leaves the target untouched.
    ///
    /// The restored file keeps the backup's schema; the next [`Store::open`]
    /// runs the ordinary in-place migration if the backup is older.
    pub fn restore_from(backup: &Path, db: &Path) -> Result<(), SuramakError> {
        let conn = Connection::open_with_flags(backup, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(
            |err| SuramakError::Backup(format!("cannot open {}: {err}", backup.display())),
        )?;
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|err| SuramakError::Backup(format!("integrity check failed: {err}")))?;
        if integrity != "ok" {
            return Err(SuramakError::Backup(format!(
                "{} failed its integrity check: {integrity}",
                backup.display()
            )));
        }
        let version: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| SuramakError::Backup(format!("cannot read the backup schema: {err}")))?;
        if let Some(raw) = version {
            let version: i64 = raw.parse().map_err(|_| {
                SuramakError::Backup(format!("backup schema version is not a number: {raw:?}"))
            })?;
            if version > SCHEMA_VERSION {
                return Err(SuramakError::Backup(format!(
                    "backup is schema v{version}, this build supports up to v{SCHEMA_VERSION}"
                )));
            }
        }
        drop(conn);

        if let Some(parent) = db.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let tmp = db.with_extension("restore-tmp");
        std::fs::copy(backup, &tmp)?;
        std::fs::File::open(&tmp)?.sync_all()?;

        // The old file's WAL belongs to the old database; leaving it beside the
        // restored file would replay unrelated frames into the new one.
        for suffix in ["-wal", "-shm"] {
            let sidecar = std::path::PathBuf::from(format!("{}{suffix}", db.display()));
            match std::fs::remove_file(&sidecar) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(SuramakError::Io(err)),
            }
        }
        // Unix `rename` replaces atomically. Windows `std::fs::rename` refuses
        // an existing destination, so there the replace is remove-then-rename:
        // a crash between the two leaves the validated temp file behind, never
        // a half-written database.
        match std::fs::rename(&tmp, db) {
            Ok(()) => Ok(()),
            Err(_err) if cfg!(windows) && db.exists() => {
                std::fs::remove_file(db)?;
                std::fs::rename(&tmp, db)?;
                Ok(())
            }
            Err(err) => Err(SuramakError::Io(err)),
        }
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
    fn a_failed_segment_edit_rolls_back_the_text_and_the_stamp() {
        // M15.1: the edit is two statements; a failure on the second must not
        // leave the new text behind with the old `updated_at`.
        let store = Store::open_in_memory().expect("store");
        let t = sample("edit-rollback.wav");
        store.insert_transcript(&t).unwrap();
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER refuse_transcript_touch BEFORE UPDATE ON transcripts
                 BEGIN SELECT RAISE(ABORT, 'no stamp'); END;",
            )
            .unwrap();

        let err = store
            .update_segment_text(t.id, 0, "must not stick")
            .expect_err("the trigger must abort the edit");
        assert!(format!("{err}").contains("no stamp"), "{err}");
        assert_eq!(
            store.get_transcript(t.id).unwrap().segments[0].text,
            "first",
            "the first statement must be rolled back"
        );
        assert_eq!(
            store.segment_source_text(t.id, 0).unwrap().as_deref(),
            Some("first")
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
