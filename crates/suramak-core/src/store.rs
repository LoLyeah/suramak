use chrono::Utc;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::path::Path;
use uuid::Uuid;

use crate::export::ZeroDuration;
use crate::segment_edit::{merge_rows, split_rows, EditError, SegmentRow, SegmentSnapshot};
use crate::textrules::{Correction, Dictionary, Glossary, PromptMeasurement, TextSettings};
use crate::transcript::{Segment, Transcript};
use crate::SuramakError;

/// The library schema this build writes and reads.
///
/// - **v1** — the schema shipped in app 0.3.0/0.3.1 (`meta.schema_version = 1`):
///   transcript header + segments, no provenance column.
/// - **v2** — M15.1, additive only: `segments.source_text` keeps the first text
///   ever stored for a row (the inference output), while `segments.text` carries
///   what the editor/polish may rewrite. No table is dropped or rebuilt.
/// - **v3** — M15.3/M15.5, additive only: `segments.derived` marks how a row's
///   `text` came to be (`split`/`merge`/…); `glossary_terms`,
///   `dictionary_entries`, `text_prefs` and `prompt_budgets` hold M15.5's own
///   data. No table is dropped or rebuilt.
pub const SCHEMA_VERSION: i64 = 3;

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
                 derived       TEXT    NOT NULL DEFAULT '',
                 PRIMARY KEY (transcript_id, idx)
             );
             -- M15.5: the user's own data. Additive; nothing here is derived
             -- from a transcript.
             CREATE TABLE IF NOT EXISTS glossary_terms (
                 position   INTEGER PRIMARY KEY,
                 term       TEXT NOT NULL,
                 created_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS dictionary_entries (
                 position   INTEGER PRIMARY KEY,
                 from_text  TEXT NOT NULL,
                 to_text    TEXT NOT NULL,
                 created_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS text_prefs (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS prompt_budgets (
                 model_id    TEXT PRIMARY KEY,
                 n_text_ctx  INTEGER NOT NULL,
                 cap_tokens  INTEGER NOT NULL,
                 measured_at TEXT NOT NULL
             );",
        )?;
        self.migrate()
    }

    /// Brings an existing library up to [`SCHEMA_VERSION`], in place.
    ///
    /// Every step is additive and idempotent: no table is rebuilt and no row is
    /// deleted. Each step runs in its own transaction and stamps the version it
    /// reaches, so an interrupted upgrade lands on a coherent older schema that
    /// the next open continues from (never a half-altered one).
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
            // A fresh database (the DDL above already created v3) or a very old
            // one without the marker: both are brought forward step by step.
            None => 0,
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
        if version < 2 {
            self.upgrade_to_v2()?;
        }
        if version < 3 {
            self.upgrade_to_v3()?;
        }
        Ok(())
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
            params!["2"],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// v2 → v3, in one transaction: the `derived` marker plus M15.5's own
    /// tables. All additive; `CREATE TABLE IF NOT EXISTS` makes a database
    /// created by this build's DDL a no-op here.
    fn upgrade_to_v3(&self) -> Result<(), SuramakError> {
        let tx = self.conn.unchecked_transaction()?;
        if !Self::column_exists(&tx, "segments", "derived")? {
            tx.execute_batch("ALTER TABLE segments ADD COLUMN derived TEXT NOT NULL DEFAULT ''")?;
        }
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS glossary_terms (
                 position   INTEGER PRIMARY KEY,
                 term       TEXT NOT NULL,
                 created_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS dictionary_entries (
                 position   INTEGER PRIMARY KEY,
                 from_text  TEXT NOT NULL,
                 to_text    TEXT NOT NULL,
                 created_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS text_prefs (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS prompt_budgets (
                 model_id    TEXT PRIMARY KEY,
                 n_text_ctx  INTEGER NOT NULL,
                 cap_tokens  INTEGER NOT NULL,
                 measured_at TEXT NOT NULL
             );",
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

    // ---- M15.3: transactional segment structure ---------------------------

    /// Every row of a transcript, provenance included, in `idx` order.
    pub fn segment_rows(&self, id: Uuid) -> Result<Vec<SegmentRow>, SuramakError> {
        Self::read_rows(&self.conn, id)
    }

    fn read_rows(conn: &Connection, id: Uuid) -> Result<Vec<SegmentRow>, SuramakError> {
        let mut stmt = conn.prepare(
            "SELECT idx, start_ms, end_ms, text, source_text, derived FROM segments
             WHERE transcript_id = ?1 ORDER BY idx",
        )?;
        let rows = stmt.query_map(params![id.to_string()], |row| {
            Ok(SegmentRow {
                idx: row.get(0)?,
                start_ms: row.get(1)?,
                end_ms: row.get(2)?,
                text: row.get(3)?,
                source_text: row.get(4)?,
                derived: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(SuramakError::Sqlite)
    }

    /// Replaces the whole row set of one transcript (inside the caller's
    /// transaction): every row is written with its own `source_text` and
    /// `derived`, never through the `COALESCE` upsert path, because a structural
    /// edit has already decided the provenance of each resulting row.
    fn replace_rows(conn: &Connection, id: Uuid, rows: &[SegmentRow]) -> Result<(), SuramakError> {
        conn.execute(
            "DELETE FROM segments WHERE transcript_id = ?1",
            params![id.to_string()],
        )?;
        let mut stmt = conn.prepare(
            "INSERT INTO segments(transcript_id, idx, start_ms, end_ms, text, source_text, derived)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for row in rows {
            stmt.execute(params![
                id.to_string(),
                row.idx,
                row.start_ms,
                row.end_ms,
                row.text,
                row.source_text,
                row.derived,
            ])?;
        }
        Ok(())
    }

    /// The one transactional frame every structural edit shares.
    ///
    /// Reads the row set, lets `mutate` derive the next one (pure, fallible),
    /// then writes it back with the `updated_at` touch **in one savepoint**. A
    /// refusal from `mutate` returns before any write; a write failure rolls the
    /// whole set back. Either way the transcript is the old set or the new one,
    /// never a mix — and a fresh app session reads exactly one of them, because
    /// SQLite commits the savepoint atomically.
    fn transact_rows<F>(&self, id: Uuid, mutate: F) -> Result<SegmentSnapshot, SuramakError>
    where
        F: FnOnce(&[SegmentRow]) -> Result<Vec<SegmentRow>, EditError>,
    {
        self.conn.execute_batch("SAVEPOINT segment_tx")?;
        let written = (|| -> Result<SegmentSnapshot, SuramakError> {
            let before = Self::read_rows(&self.conn, id)?;
            if before.is_empty() {
                // An unknown transcript and an empty one are the same refusal:
                // nothing to edit, nothing to write.
                return Err(SuramakError::SegmentEdit(EditError::NotFound(0)));
            }
            let after = mutate(&before)?;
            Self::replace_rows(&self.conn, id, &after)?;
            self.conn.execute(
                "UPDATE transcripts SET updated_at = ?1 WHERE id = ?2",
                params![Utc::now().to_rfc3339(), id.to_string()],
            )?;
            Ok(SegmentSnapshot {
                transcript_id: id,
                before,
                after,
            })
        })();
        match written {
            Ok(snapshot) => {
                self.conn.execute_batch("RELEASE segment_tx")?;
                Ok(snapshot)
            }
            Err(err) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO segment_tx; RELEASE segment_tx");
                Err(err)
            }
        }
    }

    /// Splits segment `idx` at a byte caret in its **current** text, in one
    /// transaction, and returns the before/after rows for the undo log.
    ///
    /// All validation is [`crate::segment_edit::split_rows`]'s; a refusal writes
    /// nothing. `at_ms` defaults to the span midpoint; a zero/negative-duration
    /// segment follows `policy` (the exporter's own [`ZeroDuration`]).
    pub fn split_segment(
        &self,
        id: Uuid,
        idx: u32,
        caret: usize,
        at_ms: Option<i64>,
        policy: ZeroDuration,
    ) -> Result<SegmentSnapshot, SuramakError> {
        self.transact_rows(id, |rows| split_rows(rows, idx, caret, at_ms, policy))
    }

    /// Merges `count` contiguous rows starting at `first`, in one transaction.
    pub fn merge_segments(
        &self,
        id: Uuid,
        first: u32,
        count: u32,
        separator: &str,
    ) -> Result<SegmentSnapshot, SuramakError> {
        self.transact_rows(id, |rows| merge_rows(rows, first, count, separator))
    }

    /// Writes a row set back verbatim (undo/redo of a [`SegmentSnapshot`]).
    ///
    /// This is the exact inverse of a structural edit: the rows are the ones the
    /// transaction recorded, so no re-derivation can drift. One savepoint,
    /// `updated_at` touched like any other modification.
    pub fn restore_segment_rows(&self, id: Uuid, rows: &[SegmentRow]) -> Result<(), SuramakError> {
        self.conn.execute_batch("SAVEPOINT segment_restore")?;
        let written = (|| -> Result<(), SuramakError> {
            Self::replace_rows(&self.conn, id, rows)?;
            self.conn.execute(
                "UPDATE transcripts SET updated_at = ?1 WHERE id = ?2",
                params![Utc::now().to_rfc3339(), id.to_string()],
            )?;
            Ok(())
        })();
        match written {
            Ok(()) => {
                self.conn.execute_batch("RELEASE segment_restore")?;
                Ok(())
            }
            Err(err) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO segment_restore; RELEASE segment_restore");
                Err(err)
            }
        }
    }

    // ---- M15.5: the stored text rules --------------------------------------

    /// The stored glossary terms, in stored order.
    pub fn glossary_terms(&self) -> Result<Vec<String>, SuramakError> {
        let mut stmt = self
            .conn
            .prepare("SELECT term FROM glossary_terms ORDER BY position")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(SuramakError::Sqlite)
    }

    /// The stored dictionary entries, in stored order.
    pub fn dictionary_entries(&self) -> Result<Vec<Correction>, SuramakError> {
        let mut stmt = self
            .conn
            .prepare("SELECT from_text, to_text FROM dictionary_entries ORDER BY position")?;
        let rows = stmt.query_map([], |row| {
            Ok(Correction {
                from: row.get(0)?,
                to: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(SuramakError::Sqlite)
    }

    /// Reads a text preference (`None` when it was never set).
    pub fn text_pref(&self, key: &str) -> Result<Option<String>, SuramakError> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM text_prefs WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(value)
    }

    /// Writes a text preference (upsert).
    pub fn set_text_pref(&self, key: &str, value: &str) -> Result<(), SuramakError> {
        self.conn.execute(
            "INSERT INTO text_prefs(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Every M15.5 setting, in one read.
    pub fn text_settings(&self) -> Result<TextSettings, SuramakError> {
        let glossary = Glossary::parse(&self.glossary_terms()?.join("\n"));
        let dictionary = Dictionary::parse(
            &self
                .dictionary_entries()?
                .iter()
                .map(|entry| format!("{} = {}", entry.from, entry.to))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        Ok(TextSettings {
            glossary,
            dictionary,
            normalize: self.text_pref("normalize")?.as_deref() == Some("1"),
            clean_dictionary: self.text_pref("clean_dictionary")?.as_deref() == Some("1"),
            clean_whitespace: self.text_pref("clean_whitespace")?.as_deref() == Some("1"),
            merge_separator: self
                .text_pref("merge_separator")?
                .unwrap_or_else(|| " ".to_string()),
        })
    }

    /// Replaces every M15.5 setting in one transaction (form save).
    pub fn save_text_settings(&self, settings: &TextSettings) -> Result<(), SuramakError> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute_batch("SAVEPOINT text_settings")?;
        let written = (|| -> Result<(), SuramakError> {
            self.conn.execute("DELETE FROM glossary_terms", params![])?;
            for (position, term) in settings.glossary.terms().iter().enumerate() {
                self.conn.execute(
                    "INSERT INTO glossary_terms(position, term, created_at) VALUES (?1, ?2, ?3)",
                    params![position as i64, term, now],
                )?;
            }
            self.conn
                .execute("DELETE FROM dictionary_entries", params![])?;
            for (position, entry) in settings.dictionary.entries().iter().enumerate() {
                self.conn.execute(
                    "INSERT INTO dictionary_entries(position, from_text, to_text, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![position as i64, entry.from, entry.to, now],
                )?;
            }
            for (key, value) in [
                ("normalize", settings.normalize),
                ("clean_dictionary", settings.clean_dictionary),
                ("clean_whitespace", settings.clean_whitespace),
            ] {
                self.conn.execute(
                    "INSERT INTO text_prefs(key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, if value { "1" } else { "0" }],
                )?;
            }
            self.conn.execute(
                "INSERT INTO text_prefs(key, value) VALUES ('merge_separator', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![settings.merge_separator],
            )?;
            Ok(())
        })();
        match written {
            Ok(()) => {
                self.conn.execute_batch("RELEASE text_settings")?;
                Ok(())
            }
            Err(err) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO text_settings; RELEASE text_settings");
                Err(err)
            }
        }
    }

    /// The cached prompt-budget measurement for `model_id`, when one exists.
    pub fn prompt_measurement(
        &self,
        model_id: &str,
    ) -> Result<Option<PromptMeasurement>, SuramakError> {
        let row = self
            .conn
            .query_row(
                "SELECT n_text_ctx, cap_tokens, measured_at FROM prompt_budgets WHERE model_id = ?1",
                params![model_id],
                |row| {
                    Ok((
                        row.get::<_, i32>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((n_text_ctx, cap_tokens, measured_at)) = row else {
            return Ok(None);
        };
        let measured_at = measured_at.parse().map_err(|_| {
            SuramakError::Schema(format!(
                "prompt_budgets.measured_at is not a timestamp ({measured_at:?})"
            ))
        })?;
        Ok(Some(PromptMeasurement {
            model_id: model_id.to_string(),
            n_text_ctx,
            cap_tokens: cap_tokens as usize,
            measured_at,
        }))
    }

    /// Records a measurement (upsert; one row per model).
    pub fn record_prompt_measurement(
        &self,
        measurement: &PromptMeasurement,
    ) -> Result<(), SuramakError> {
        self.conn.execute(
            "INSERT INTO prompt_budgets(model_id, n_text_ctx, cap_tokens, measured_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(model_id) DO UPDATE SET
                 n_text_ctx  = excluded.n_text_ctx,
                 cap_tokens  = excluded.cap_tokens,
                 measured_at = excluded.measured_at",
            params![
                measurement.model_id,
                measurement.n_text_ctx,
                measurement.cap_tokens as i64,
                measurement.measured_at.to_rfc3339(),
            ],
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

    // ---- M15.3: transactional split/merge -----------------------------------

    /// A three-row transcript whose texts all contain searchable words.
    fn split_sample() -> Transcript {
        Transcript::new(
            "split.wav".to_string(),
            Source::File {
                name: "split.wav".to_string(),
            },
            "tiny".to_string(),
            Some("id".to_string()),
            None,
            vec![
                Segment {
                    idx: 0,
                    start_ms: 0,
                    end_ms: 1_000,
                    text: "halo dunia".to_string(),
                },
                Segment {
                    idx: 1,
                    start_ms: 1_000,
                    end_ms: 2_000,
                    text: "selamat pagi semua".to_string(),
                },
                Segment {
                    idx: 2,
                    start_ms: 2_000,
                    end_ms: 3_000,
                    text: "terima kasih".to_string(),
                },
            ],
        )
    }

    fn row_texts(rows: &[SegmentRow]) -> Vec<&str> {
        rows.iter().map(|row| row.text.as_str()).collect()
    }

    #[test]
    fn a_split_persists_two_rows_and_leaves_every_other_row_untouched() {
        let store = Store::open_in_memory().expect("store");
        let t = split_sample();
        store.insert_transcript(&t).unwrap();
        let before = store.segment_rows(t.id).unwrap();

        let snapshot = store
            .split_segment(t.id, 1, 7, Some(1_500), ZeroDuration::OneFrame)
            .expect("split");
        assert_eq!(
            snapshot.before, before,
            "the snapshot is the pre-edit row set"
        );
        assert_eq!(snapshot.after.len(), 4);
        assert_eq!(snapshot.delta(), 1);

        let rows = store.segment_rows(t.id).unwrap();
        assert_eq!(
            row_texts(&rows),
            vec!["halo dunia", "selamat", "pagi semua", "terima kasih"]
        );
        assert_eq!(
            rows.iter().map(|row| row.idx).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        assert_eq!((rows[1].start_ms, rows[1].end_ms), (1_000, 1_500));
        assert_eq!((rows[2].start_ms, rows[2].end_ms), (1_500, 2_000));
        assert_eq!(rows[1].derived, "split");
        assert_eq!(
            rows[1].source_text.as_deref(),
            Some("selamat pagi semua"),
            "the parent's inference text survives on the left half"
        );
        assert_eq!(
            rows[2].source_text.as_deref(),
            Some("selamat pagi semua"),
            "…and on the right half"
        );
        // Unrelated rows are byte-identical, provenance included.
        assert_eq!(rows[0], before[0]);
        assert_eq!(rows[3].text, before[2].text);
        assert_eq!(rows[3].source_text, before[2].source_text);

        // The store's own read path agrees with the row read.
        let reread = store.get_transcript(t.id).unwrap();
        assert_eq!(reread.segments.len(), 4);
        assert_eq!(reread.segments[2].text, "pagi semua");
    }

    #[test]
    fn search_stays_consistent_after_each_structural_edit() {
        let store = Store::open_in_memory().expect("store");
        let t = split_sample();
        store.insert_transcript(&t).unwrap();
        assert_eq!(store.search("pagi").unwrap().len(), 1, "before the split");

        // Split: both halves are in the same transcript, so the transcript is
        // still one hit; the scanner reads the new rows, not a stale index.
        store
            .split_segment(t.id, 1, 7, None, ZeroDuration::OneFrame)
            .unwrap();
        let hits = store.search("pagi").unwrap();
        assert_eq!(hits.len(), 1);
        let rows = store.segment_rows(t.id).unwrap();
        assert_eq!(
            rows.iter().filter(|row| row.text.contains("pagi")).count(),
            1,
            "the word still lives in exactly one row after the split"
        );
        assert_eq!(store.search("selamat").unwrap().len(), 1);

        // A word only an edited row had before is gone from search once the
        // edit removes it — no index is holding the old text.
        store.update_segment_text(t.id, 0, "hai").unwrap();
        assert!(store.search("halo").unwrap().is_empty());
        assert_eq!(store.search("hai").unwrap().len(), 1);

        // Merge the two halves back: the joined text is searchable as one row.
        store.merge_segments(t.id, 1, 2, " ").unwrap();
        assert_eq!(store.search("pagi semua").unwrap().len(), 1);
        assert_eq!(store.search("selamat pagi").unwrap().len(), 1);
        assert_eq!(store.segment_rows(t.id).unwrap().len(), 3);
    }

    #[test]
    fn a_refused_split_writes_nothing_and_reports_the_rule() {
        let store = Store::open_in_memory().expect("store");
        let t = split_sample();
        store.insert_transcript(&t).unwrap();
        let before = store.segment_rows(t.id).unwrap();

        // No whitespace in the whole text: no word boundary to cut.
        store
            .update_segment_text(t.id, 2, "pertanggungjawaban")
            .unwrap();
        let after_edit = store.segment_rows(t.id).unwrap();
        let err = store
            .split_segment(t.id, 2, 5, None, ZeroDuration::OneFrame)
            .expect_err("no boundary");
        assert!(
            matches!(err, SuramakError::SegmentEdit(EditError::NoWordBoundary)),
            "{err}"
        );
        assert_eq!(store.segment_rows(t.id).unwrap(), after_edit);

        // A caret outside the text is an empty half, not a panic.
        let err = store
            .split_segment(t.id, 1, 0, None, ZeroDuration::OneFrame)
            .expect_err("empty side");
        assert!(matches!(
            err,
            SuramakError::SegmentEdit(EditError::EmptySide)
        ));
        // A cut outside the span is a crossing, not a negative cue.
        let err = store
            .split_segment(t.id, 1, 7, Some(9_000), ZeroDuration::OneFrame)
            .expect_err("crossing");
        assert!(matches!(
            err,
            SuramakError::SegmentEdit(EditError::CrossingBoundary { .. })
        ));
        assert_eq!(
            store.segment_rows(t.id).unwrap()[..2],
            before[..2],
            "refusals leave the transcript exactly as it was"
        );
    }

    #[test]
    fn a_zero_duration_segment_follows_the_skip_policy_in_the_store() {
        let store = Store::open_in_memory().expect("store");
        let mut t = split_sample();
        t.segments[1].end_ms = t.segments[1].start_ms;
        store.insert_transcript(&t).unwrap();

        let err = store
            .split_segment(t.id, 1, 7, None, ZeroDuration::Skip)
            .expect_err("Skip refuses");
        assert!(matches!(
            err,
            SuramakError::SegmentEdit(EditError::ZeroDuration { .. })
        ));
        // OneFrame widens to 40 ms and splits it.
        let snapshot = store
            .split_segment(t.id, 1, 7, None, ZeroDuration::OneFrame)
            .expect("OneFrame splits");
        let rows = &snapshot.after;
        assert_eq!(rows[1].start_ms, 1_000);
        assert_eq!(rows[2].end_ms, 1_040);
    }

    #[test]
    fn a_merge_persists_one_row_and_keeps_provenance() {
        let store = Store::open_in_memory().expect("store");
        let t = split_sample();
        store.insert_transcript(&t).unwrap();

        let snapshot = store.merge_segments(t.id, 0, 2, "\n").expect("merge");
        assert_eq!(snapshot.after.len(), 2);
        let rows = store.segment_rows(t.id).unwrap();
        assert_eq!(
            row_texts(&rows),
            vec!["halo dunia\nselamat pagi semua", "terima kasih"]
        );
        assert_eq!((rows[0].start_ms, rows[0].end_ms), (0, 2_000));
        assert_eq!(rows[0].derived, "merge");
        assert_eq!(
            rows[0].source_text.as_deref(),
            Some("halo dunia\nselamat pagi semua")
        );
        // The tail row's content is untouched; only its idx shifted down.
        assert_eq!(rows[1].text, snapshot.before[2].text);
        assert_eq!(rows[1].start_ms, snapshot.before[2].start_ms);
        assert_eq!(rows[1].source_text, snapshot.before[2].source_text);
        assert_eq!(rows[1].derived, "");
    }

    #[test]
    fn undo_restores_the_exact_rows_and_redo_puts_them_back() {
        let store = Store::open_in_memory().expect("store");
        let t = split_sample();
        store.insert_transcript(&t).unwrap();
        let original = store.segment_rows(t.id).unwrap();

        let split = store
            .split_segment(t.id, 0, 5, None, ZeroDuration::OneFrame)
            .unwrap();
        let split_rows_now = store.segment_rows(t.id).unwrap();
        assert_eq!(split_rows_now.len(), 4);

        // Undo = write the recorded `before` set verbatim.
        store.restore_segment_rows(t.id, &split.before).unwrap();
        assert_eq!(store.segment_rows(t.id).unwrap(), original);
        // The transcript is still one row set for search and reads.
        assert_eq!(store.get_transcript(t.id).unwrap().segments.len(), 3);

        // Redo = write the recorded `after` set.
        store.restore_segment_rows(t.id, &split.after).unwrap();
        assert_eq!(store.segment_rows(t.id).unwrap(), split_rows_now);

        // A merge's snapshot round-trips the same way.
        let merged = store.merge_segments(t.id, 0, 3, " ").unwrap();
        store.restore_segment_rows(t.id, &merged.before).unwrap();
        assert_eq!(store.segment_rows(t.id).unwrap(), split_rows_now);
    }

    #[test]
    fn a_failed_structural_write_rolls_the_whole_set_back() {
        let store = Store::open_in_memory().expect("store");
        let t = split_sample();
        store.insert_transcript(&t).unwrap();
        let before = store.segment_rows(t.id).unwrap();

        // Abort the *second* insert: the delete and the first row are already
        // written when the trigger fires.
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER refuse_second_row BEFORE INSERT ON segments
                 WHEN NEW.idx = 2 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        let err = store
            .split_segment(t.id, 1, 7, None, ZeroDuration::OneFrame)
            .expect_err("the trigger aborts the split");
        assert!(format!("{err}").contains("boom"), "{err}");
        assert_eq!(
            store.segment_rows(t.id).unwrap(),
            before,
            "no partially-applied transaction may survive"
        );
    }

    #[test]
    fn a_reopened_store_reads_only_committed_rows() {
        // The "fresh app session" gate: a split is committed or it is not, and
        // a second connection sees exactly the committed set.
        let dir = std::env::temp_dir().join(format!("suramak-split-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("suramak.db");

        let t = split_sample();
        {
            let store = Store::open(&path).expect("store");
            store.insert_transcript(&t).unwrap();
            store
                .split_segment(t.id, 1, 7, Some(1_500), ZeroDuration::OneFrame)
                .unwrap();
            store.checkpoint().unwrap();
        }
        let reopened = Store::open(&path).expect("reopen");
        let rows = reopened.segment_rows(t.id).unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].text, "selamat");
        assert_eq!(rows[2].text, "pagi semua");
        assert_eq!(reopened.get_transcript(t.id).unwrap().segments.len(), 4);
        drop(reopened);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_v2_database_upgrades_to_v3_additively() {
        let dir = std::env::temp_dir().join(format!("suramak-v2v3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("v2.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE transcripts (
                     id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at TEXT NOT NULL,
                     updated_at TEXT NOT NULL, source_json TEXT NOT NULL,
                     model TEXT NOT NULL, language TEXT, audio_path TEXT);
                 CREATE TABLE segments (
                     transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
                     idx INTEGER NOT NULL, start_ms INTEGER NOT NULL, end_ms INTEGER NOT NULL,
                     text TEXT NOT NULL, source_text TEXT,
                     PRIMARY KEY (transcript_id, idx));
                 INSERT INTO meta(key, value) VALUES ('schema_version', '2');
                 INSERT INTO transcripts VALUES (
                     '00000000-0000-0000-0000-000000000001', 'old.wav',
                     '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00',
                     '{\"type\":\"file\",\"name\":\"old.wav\"}', 'tiny', 'id', NULL);
                 INSERT INTO segments VALUES (
                     '00000000-0000-0000-0000-000000000001', 0, 0, 1000, 'halo', 'halo');",
            )
            .unwrap();
        }

        let store = Store::open(&path).expect("upgrade");
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        let rows = store
            .segment_rows(Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap())
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "halo");
        assert_eq!(rows[0].source_text.as_deref(), Some("halo"));
        assert_eq!(rows[0].derived, "", "existing rows start unmarked");
        // The v3 tables exist and answer.
        assert!(store.glossary_terms().unwrap().is_empty());
        assert!(store.text_settings().unwrap().dictionary.is_empty());
        assert_eq!(store.prompt_measurement("tiny").unwrap(), None);

        // Idempotent: a second open is a no-op at the same version.
        drop(store);
        let again = Store::open(&path).expect("reopen");
        assert_eq!(again.schema_version().unwrap(), SCHEMA_VERSION);
        drop(again);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- M15.5: stored text rules -------------------------------------------

    #[test]
    fn the_text_settings_round_trip_through_the_store() {
        let store = Store::open_in_memory().expect("store");
        let settings = TextSettings {
            glossary: Glossary::parse("Kecamatan Cilandak\nBPJS Kesehatan"),
            dictionary: Dictionary::parse("BPJS = Badan Penyelenggara Jaminan Sosial"),
            normalize: true,
            clean_dictionary: true,
            clean_whitespace: false,
            merge_separator: " · ".to_string(),
        };
        store.save_text_settings(&settings).unwrap();
        let loaded = store.text_settings().unwrap();
        assert_eq!(loaded, settings);

        // Replacing is a replacement, not an append.
        let replaced = TextSettings {
            glossary: Glossary::parse("Jakarta"),
            ..TextSettings::default()
        };
        store.save_text_settings(&replaced).unwrap();
        assert_eq!(store.glossary_terms().unwrap(), ["Jakarta"]);
        assert!(store.dictionary_entries().unwrap().is_empty());
        assert!(!store.text_settings().unwrap().normalize);
    }

    #[test]
    fn a_prompt_measurement_round_trips_and_upserts() {
        let store = Store::open_in_memory().expect("store");
        assert_eq!(store.prompt_measurement("ggml-tiny").unwrap(), None);

        let first = PromptMeasurement::new("ggml-tiny", 448);
        store.record_prompt_measurement(&first).unwrap();
        let loaded = store.prompt_measurement("ggml-tiny").unwrap().expect("row");
        assert_eq!(loaded.n_text_ctx, 448);
        assert_eq!(loaded.cap_tokens, 224);
        assert_eq!(loaded.budget().cap_tokens(), 224);
        assert_eq!(loaded.model_id, "ggml-tiny");

        // A re-measurement overwrites the row rather than duplicating it.
        let second = PromptMeasurement::new("ggml-tiny", 448);
        store.record_prompt_measurement(&second).unwrap();
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM prompt_budgets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
