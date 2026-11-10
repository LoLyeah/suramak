use chrono::Utc;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use uuid::Uuid;

use crate::export::ZeroDuration;
use crate::search::{self, HitKind, SearchHit, SearchOptions, SearchResults};
use crate::segment_edit::{merge_rows, split_rows, EditError, SegmentRow, SegmentSnapshot};
use crate::textrules::{Correction, Dictionary, Glossary, PromptMeasurement, TextSettings};
use crate::transcript::{Segment, Transcript, WordTiming};
use crate::translate::TranslationRecord;
use crate::SuramakError;

/// Encodes a translation's source-id set for its one `TEXT` column (M15.8).
/// A JSON array keeps the set ordered and extensible without a join table.
fn encode_source_ids(ids: &[u32]) -> String {
    serde_json::to_string(ids).unwrap_or_else(|_| "[]".to_string())
}

/// Decodes [`encode_source_ids`]; a malformed legacy value reads as an empty
/// set (the provenance line then says "no source ids") instead of failing the
/// whole transcript load.
fn decode_source_ids(raw: Option<String>) -> Vec<u32> {
    raw.and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// How many documents one backfill/rebuild transaction writes. A 100k-segment
/// library is indexed as ~200 bounded transactions instead of one giant write,
/// so a crash mid-backfill loses at most one chunk and the cursor resumes.
const SEARCH_BACKFILL_CHUNK: usize = 500;

/// The FTS5 table. `unicode61` is SQLite's Unicode-aware tokenizer with **no
/// stemming** (unlike `porter`, which would conflate Indonesian words);
/// `remove_diacritics 1` folds accents, so `cafe` finds `café` and `ÉCLAIR`
/// finds `éclair`. `body` is the current text, `original` the preserved
/// inference text (NULL when identical, so it is not indexed twice).
const SEARCH_FTS_DDL: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5(
        body,
        original,
        tokenize = 'unicode61 remove_diacritics 1'
    );";

/// The metadata side of the index: one row per indexed document, keyed by a
/// stable string (`seg:<uuid>:<idx>`, `title:<uuid>`, `glossary:<pos>`,
/// `dict:<pos>`) and carrying the FTS rowid. The mapping is what makes a
/// delete or an edit O(log n) instead of a scan of the FTS content table.
const SEARCH_DOCS_DDL: &str = "CREATE TABLE IF NOT EXISTS search_docs (
        rowid         INTEGER PRIMARY KEY,
        doc_key       TEXT NOT NULL UNIQUE,
        kind          TEXT NOT NULL,
        transcript_id TEXT,
        segment_idx   INTEGER
    );
    CREATE INDEX IF NOT EXISTS search_docs_transcript ON search_docs(transcript_id);";

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
/// - **v4** — M15.7, additive only: `search_fts` (the FTS5 index) and
///   `search_docs` (its document map). Existing tables are untouched; the index
///   is backfilled in bounded chunks and can be rebuilt deterministically.
/// - **v5** — M15.8, additive only: `segments.translation*` — the opt-in
///   derived bilingual text, its provenance (model, timestamp, source segment
///   id set) and the sticky stale marker. Existing rows keep `NULL`/`0`; no
///   table is dropped or rebuilt.
/// - **v6** — M15.6, additive only: `word_timings` — the estimated per-word
///   start/end that whisper's token spans yield, keyed to the segment text
///   they were computed from (`aligned_text`) with a sticky stale marker for
///   any later source edit. Existing tables and rows are untouched; a library
///   that never re-aligns simply has no rows here (and gets segment seeking).
pub const SCHEMA_VERSION: i64 = 6;

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

/// How many documents of each kind the search index holds (M15.7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchIndexStats {
    pub segments: i64,
    pub titles: i64,
    pub glossary: i64,
    pub dictionary: i64,
}

impl SearchIndexStats {
    /// Every indexed document, all kinds.
    pub fn total(&self) -> i64 {
        self.segments + self.titles + self.glossary + self.dictionary
    }
}

/// How many word rows a write actually landed, and how many segment groups it
/// refused (M15.6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WordWriteStats {
    /// Word rows inserted.
    pub written: usize,
    /// Segment groups dropped because the segment was gone or its text did not
    /// match the words.
    pub skipped_segments: usize,
}

/// A transcript's usable word timing (M15.6).
///
/// `current` holds only the segments whose rows are both un-stale and still
/// aligned to the text the segment carries; `stale` names the segments that
/// have rows which no longer qualify. Both are per segment, so one edit never
/// costs an untouched segment its karaoke.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WordAlignment {
    /// Usable words, per segment index, in word order.
    pub current: BTreeMap<u32, Vec<WordTiming>>,
    /// Segments that have word rows but must not use them (edited/stale).
    pub stale: BTreeSet<u32>,
}

impl WordAlignment {
    /// Whether any segment can drive word-level seeking at all.
    pub fn is_empty(&self) -> bool {
        self.current.is_empty()
    }

    /// Wraps a freshly produced word list as all-current (a just-finished job,
    /// before the store round-trip). Rows keep their segment/word order.
    pub fn from_words(words: &[WordTiming]) -> Self {
        let mut current: BTreeMap<u32, Vec<WordTiming>> = BTreeMap::new();
        for word in words {
            current
                .entry(word.segment_idx)
                .or_default()
                .push(word.clone());
        }
        Self {
            current,
            stale: BTreeSet::new(),
        }
    }

    /// The words of one segment, when they are current.
    pub fn segment(&self, idx: u32) -> Option<&[WordTiming]> {
        self.current.get(&idx).map(Vec::as_slice)
    }

    /// Every usable word, in segment/word order.
    pub fn words(&self) -> impl Iterator<Item = &WordTiming> {
        self.current.values().flatten()
    }

    /// How many segments can drive word-level seeking.
    pub fn current_segments(&self) -> usize {
        self.current.len()
    }
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
                 -- M15.8: the opt-in bilingual derived text + provenance. NULL
                 -- translation = never asked for one.
                 translation        TEXT,
                 translation_model  TEXT,
                 translation_at     TEXT,
                 translation_sources TEXT,
                 translation_stale  INTEGER NOT NULL DEFAULT 0,
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
             );
             -- M15.7: the search index. The virtual table and its document map
             -- are additive; the backfill below fills them for an older library.
             CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5(
                 body,
                 original,
                 tokenize = 'unicode61 remove_diacritics 1'
             );
             CREATE TABLE IF NOT EXISTS search_docs (
                 rowid         INTEGER PRIMARY KEY,
                 doc_key       TEXT NOT NULL UNIQUE,
                 kind          TEXT NOT NULL,
                 transcript_id TEXT,
                 segment_idx   INTEGER
             );
             CREATE INDEX IF NOT EXISTS search_docs_transcript ON search_docs(transcript_id);
             -- M15.6: estimated word timing. `aligned_text` is the segment text
             -- the rows were computed from; a row whose segment no longer says
             -- exactly that is stale, and `stale` is the sticky marker any
             -- source edit sets (never cleared except by a fresh alignment).
             CREATE TABLE IF NOT EXISTS word_timings (
                 transcript_id TEXT    NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
                 segment_idx   INTEGER NOT NULL,
                 word_idx      INTEGER NOT NULL,
                 start_ms      INTEGER NOT NULL,
                 end_ms        INTEGER NOT NULL,
                 text          TEXT    NOT NULL,
                 aligned_text  TEXT    NOT NULL,
                 stale         INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (transcript_id, segment_idx, word_idx)
             );",
        )?;
        self.migrate()?;
        self.ensure_search_index()
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
            // A fresh database (the DDL above already created the current
            // schema) or a very old one without the marker: both are brought
            // forward step by step.
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
        if version < 4 {
            self.upgrade_to_v4()?;
        }
        if version < 5 {
            self.upgrade_to_v5()?;
        }
        if version < 6 {
            self.upgrade_to_v6()?;
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
            params!["3"],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// v3 → v4: the FTS5 index and its document map, then a bounded backfill.
    ///
    /// Three phases, so a crash can never leave a half-stamped schema:
    ///
    /// 1. the objects, one transaction (`IF NOT EXISTS`, so a re-run is a
    ///    no-op);
    /// 2. the backfill, [`SEARCH_BACKFILL_CHUNK`] documents per transaction,
    ///    with a `meta` cursor committed **with** each chunk — an interrupted
    ///    backfill resumes at the last committed document instead of redoing
    ///    the corpus, and a crash loses at most one chunk;
    /// 3. the v4 stamp, one transaction, only once every document is in.
    ///
    /// An interrupted upgrade is therefore still a coherent v3 library (the
    /// version is raised last), which this build's next open continues and an
    /// older build can still open.
    fn upgrade_to_v4(&self) -> Result<(), SuramakError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(SEARCH_FTS_DDL)?;
        tx.execute_batch(SEARCH_DOCS_DDL)?;
        tx.commit()?;

        self.backfill_search_index()?;

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM meta WHERE key IN ('search_backfill_segments', 'search_backfill_titles')",
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

    /// v4 → v5, one transaction, additive only: the opt-in bilingual derived
    /// columns (M15.8). Existing rows read back exactly as before (`NULL`
    /// translation, `0` stale); a re-run is a no-op, which matters because the
    /// DDL above may already have created the columns on a fresh database.
    fn upgrade_to_v5(&self) -> Result<(), SuramakError> {
        let tx = self.conn.unchecked_transaction()?;
        for (column, ddl) in [
            (
                "translation",
                "ALTER TABLE segments ADD COLUMN translation TEXT",
            ),
            (
                "translation_model",
                "ALTER TABLE segments ADD COLUMN translation_model TEXT",
            ),
            (
                "translation_at",
                "ALTER TABLE segments ADD COLUMN translation_at TEXT",
            ),
            (
                "translation_sources",
                "ALTER TABLE segments ADD COLUMN translation_sources TEXT",
            ),
            (
                "translation_stale",
                "ALTER TABLE segments ADD COLUMN translation_stale INTEGER NOT NULL DEFAULT 0",
            ),
        ] {
            if !Self::column_exists(&tx, "segments", column)? {
                tx.execute_batch(ddl)?;
            }
        }
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![SCHEMA_VERSION.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// v5 → v6, one transaction, additive only: the estimated word timing
    /// table (M15.6). A fresh database already has it from the DDL above, and
    /// a re-run is a no-op; an older library simply starts with no word rows.
    fn upgrade_to_v6(&self) -> Result<(), SuramakError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS word_timings (
                 transcript_id TEXT    NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
                 segment_idx   INTEGER NOT NULL,
                 word_idx      INTEGER NOT NULL,
                 start_ms      INTEGER NOT NULL,
                 end_ms        INTEGER NOT NULL,
                 text          TEXT    NOT NULL,
                 aligned_text  TEXT    NOT NULL,
                 stale         INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (transcript_id, segment_idx, word_idx)
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

    // ---- M15.7: the search index ------------------------------------------

    /// The key of one segment document (`idx` zero-padded so a string cursor
    /// orders exactly like the integer it encodes).
    fn segment_doc_key(id: Uuid, idx: u32) -> String {
        format!("seg:{id}:{idx:010}")
    }

    /// The key of one transcript-title document.
    fn title_doc_key(id: Uuid) -> String {
        format!("title:{id}")
    }

    fn meta_value(&self, key: &str) -> Result<Option<String>, SuramakError> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(value)
    }

    /// Writes (or rewrites) one index document and points its rowid at it.
    ///
    /// The document map is inserted first (that is where the rowid comes
    /// from); any stale FTS row with that rowid is removed before the new one
    /// lands, so an edit can never leave two versions of a document behind.
    #[allow(clippy::too_many_arguments)]
    fn put_search_doc(
        conn: &Connection,
        doc_key: &str,
        kind: HitKind,
        transcript_id: Option<&str>,
        segment_idx: Option<u32>,
        body: &str,
        original: Option<&str>,
    ) -> Result<(), SuramakError> {
        conn.execute(
            "INSERT INTO search_docs(doc_key, kind, transcript_id, segment_idx)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(doc_key) DO UPDATE SET
                 kind          = excluded.kind,
                 transcript_id = excluded.transcript_id,
                 segment_idx   = excluded.segment_idx",
            params![doc_key, kind.as_str(), transcript_id, segment_idx],
        )?;
        let rowid: i64 = conn.query_row(
            "SELECT rowid FROM search_docs WHERE doc_key = ?1",
            params![doc_key],
            |row| row.get(0),
        )?;
        conn.execute("DELETE FROM search_fts WHERE rowid = ?1", params![rowid])?;
        conn.execute(
            "INSERT INTO search_fts(rowid, body, original) VALUES (?1, ?2, ?3)",
            params![rowid, body, original],
        )?;
        Ok(())
    }

    /// Removes one document (both halves: the map row and the FTS row).
    fn delete_search_doc(conn: &Connection, doc_key: &str) -> Result<(), SuramakError> {
        let rowid = conn
            .query_row(
                "SELECT rowid FROM search_docs WHERE doc_key = ?1",
                params![doc_key],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if let Some(rowid) = rowid {
            conn.execute("DELETE FROM search_fts WHERE rowid = ?1", params![rowid])?;
            conn.execute("DELETE FROM search_docs WHERE rowid = ?1", params![rowid])?;
        }
        Ok(())
    }

    /// Deletes the FTS rows whose `search_docs` rows match `select`, then the
    /// map rows themselves. One helper so no caller can delete half a document.
    fn delete_docs_matching(
        conn: &Connection,
        select: &str,
        param: &dyn rusqlite::ToSql,
    ) -> Result<usize, SuramakError> {
        let rowids: Vec<i64> = {
            let mut stmt = conn.prepare(select)?;
            let rows = stmt.query_map([param], |row| row.get::<_, i64>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for rowid in &rowids {
            conn.execute("DELETE FROM search_fts WHERE rowid = ?1", params![rowid])?;
        }
        Ok(rowids.len())
    }

    /// Every segment document of one transcript (used when a structural edit
    /// rewrites the whole row set and indices shift).
    fn delete_all_segment_docs(conn: &Connection, id: Uuid) -> Result<(), SuramakError> {
        let id = id.to_string();
        Self::delete_docs_matching(
            conn,
            "SELECT rowid FROM search_docs WHERE transcript_id = ?1 AND kind = 'segment'",
            &id,
        )?;
        conn.execute(
            "DELETE FROM search_docs WHERE transcript_id = ?1 AND kind = 'segment'",
            params![id],
        )?;
        Ok(())
    }

    /// Segment documents at or after `from_idx` (the stale tail of an upsert).
    fn delete_segment_docs_from(
        conn: &Connection,
        id: Uuid,
        from_idx: u32,
    ) -> Result<(), SuramakError> {
        let id = id.to_string();
        let rowids: Vec<i64> = {
            let mut stmt = conn.prepare(
                "SELECT rowid FROM search_docs
                 WHERE transcript_id = ?1 AND kind = 'segment' AND segment_idx >= ?2",
            )?;
            let rows = stmt.query_map(params![id, from_idx], |row| row.get::<_, i64>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for rowid in &rowids {
            conn.execute("DELETE FROM search_fts WHERE rowid = ?1", params![rowid])?;
        }
        conn.execute(
            "DELETE FROM search_docs
             WHERE transcript_id = ?1 AND kind = 'segment' AND segment_idx >= ?2",
            params![id, from_idx],
        )?;
        Ok(())
    }

    /// Everything a transcript owns — its segment documents and its title.
    fn delete_docs_for_transcript(conn: &Connection, id: Uuid) -> Result<(), SuramakError> {
        let id = id.to_string();
        Self::delete_docs_matching(
            conn,
            "SELECT rowid FROM search_docs WHERE transcript_id = ?1",
            &id,
        )?;
        conn.execute(
            "DELETE FROM search_docs WHERE transcript_id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Whether a stored text is searchable at all (M15.7's empty rule).
    ///
    /// One predicate for Rust and SQL: ASCII whitespace only, which is what a
    /// transcript's empty rows actually contain. It is deliberately *not*
    /// `str::trim` (Unicode) — the consistency check runs the same test in SQL
    /// (`length(trim(text, char(9)||char(10)||char(13)||' ')) > 0`), and the
    /// two must agree exactly or every open would see a phantom mismatch.
    fn has_searchable_text(text: &str) -> bool {
        !text
            .trim_matches(|c: char| c.is_ascii_whitespace())
            .is_empty()
    }

    /// Re-indexes one segment from the row the store now holds.
    ///
    /// Reads the row back instead of trusting the caller: `source_text` is
    /// `COALESCE`-preserved by the upsert, so only the stored value tells the
    /// truth about what the original column now says.
    fn sync_segment_doc(conn: &Connection, id: Uuid, idx: u32) -> Result<(), SuramakError> {
        let row = conn
            .query_row(
                "SELECT text, source_text FROM segments WHERE transcript_id = ?1 AND idx = ?2",
                params![id.to_string(), idx],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?;
        let Some((text, source)) = row else {
            return Ok(());
        };
        let key = Self::segment_doc_key(id, idx);
        if !Self::has_searchable_text(&text) {
            // M15.7 empty rule: a segment with no current text is not a result.
            return Self::delete_search_doc(conn, &key);
        }
        let original = source
            .as_deref()
            .filter(|source| Self::has_searchable_text(source) && *source != text);
        Self::put_search_doc(
            conn,
            &key,
            HitKind::Segment,
            Some(&id.to_string()),
            Some(idx),
            &text,
            original,
        )
    }

    /// Re-indexes the glossary + dictionary corpus (small, replaced wholesale).
    fn sync_corpus_docs(conn: &Connection) -> Result<usize, SuramakError> {
        let rowids: Vec<i64> = {
            let mut stmt = conn.prepare(
                "SELECT rowid FROM search_docs WHERE kind IN ('glossary', 'dictionary')",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for rowid in &rowids {
            conn.execute("DELETE FROM search_fts WHERE rowid = ?1", params![rowid])?;
        }
        conn.execute(
            "DELETE FROM search_docs WHERE kind IN ('glossary', 'dictionary')",
            [],
        )?;

        let mut written = 0;
        let terms: Vec<(i64, String)> = {
            let mut stmt =
                conn.prepare("SELECT position, term FROM glossary_terms ORDER BY position")?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for (position, term) in terms {
            if !Self::has_searchable_text(&term) {
                continue;
            }
            Self::put_search_doc(
                conn,
                &format!("glossary:{position:010}"),
                HitKind::Glossary,
                None,
                None,
                &term,
                None,
            )?;
            written += 1;
        }
        let entries: Vec<(i64, String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT position, from_text, to_text FROM dictionary_entries ORDER BY position",
            )?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for (position, from_text, to_text) in entries {
            if !Self::has_searchable_text(&from_text) && !Self::has_searchable_text(&to_text) {
                continue;
            }
            Self::put_search_doc(
                conn,
                &format!("dict:{position:010}"),
                HitKind::Dictionary,
                None,
                None,
                &from_text,
                Some(to_text.as_str())
                    .filter(|to| Self::has_searchable_text(to) && **to != from_text),
            )?;
            written += 1;
        }
        Ok(written)
    }

    /// Fills the index for everything the store holds, in bounded chunks.
    ///
    /// Resumes from the `meta` cursors the last committed chunk left, so both
    /// the migration and a manual [`Store::rebuild_search_index`] share it.
    fn backfill_search_index(&self) -> Result<usize, SuramakError> {
        let mut written = 0;
        let mut cursor = self.meta_value("search_backfill_segments")?;
        loop {
            let (count, next) = self.index_segment_chunk(cursor.as_deref())?;
            if count == 0 {
                break;
            }
            written += count;
            cursor = next;
        }
        let mut cursor = self.meta_value("search_backfill_titles")?;
        loop {
            let (count, next) = self.index_title_chunk(cursor.as_deref())?;
            if count == 0 {
                break;
            }
            written += count;
            cursor = next;
        }
        written += self.index_corpus_docs()?;
        Ok(written)
    }

    /// Indexes one bounded chunk of segments, after `after` (a doc key).
    fn index_segment_chunk(
        &self,
        after: Option<&str>,
    ) -> Result<(usize, Option<String>), SuramakError> {
        let rows: Vec<(String, i64, String, Option<String>, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT transcript_id, idx, text, source_text,
                        ('seg:' || transcript_id || ':' || printf('%010d', idx))
                 FROM segments
                 WHERE length(trim(text, char(9) || char(10) || char(13) || ' ')) > 0
                   AND (?1 IS NULL
                        OR ('seg:' || transcript_id || ':' || printf('%010d', idx)) > ?1)
                 ORDER BY ('seg:' || transcript_id || ':' || printf('%010d', idx))
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![after, SEARCH_BACKFILL_CHUNK as i64], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let Some(last) = rows.last().map(|row| row.4.clone()) else {
            return Ok((0, None));
        };
        let tx = self.conn.unchecked_transaction()?;
        for (transcript_id, idx, text, source, key) in &rows {
            let original = source
                .as_deref()
                .filter(|source| !source.trim().is_empty() && *source != *text);
            Self::put_search_doc(
                &tx,
                key,
                HitKind::Segment,
                Some(transcript_id),
                Some(*idx as u32),
                text,
                original,
            )?;
        }
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('search_backfill_segments', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![last],
        )?;
        tx.commit()?;
        Ok((rows.len(), Some(last)))
    }

    /// Indexes one bounded chunk of transcript titles, after `after`.
    fn index_title_chunk(
        &self,
        after: Option<&str>,
    ) -> Result<(usize, Option<String>), SuramakError> {
        let rows: Vec<(String, String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, title, ('title:' || id)
                 FROM transcripts
                 WHERE (?1 IS NULL OR ('title:' || id) > ?1)
                 ORDER BY ('title:' || id)
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![after, SEARCH_BACKFILL_CHUNK as i64], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let Some(last) = rows.last().map(|row| row.2.clone()) else {
            return Ok((0, None));
        };
        let tx = self.conn.unchecked_transaction()?;
        for (id, title, key) in &rows {
            Self::put_search_doc(&tx, key, HitKind::Title, Some(id), None, title, None)?;
        }
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('search_backfill_titles', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![last],
        )?;
        tx.commit()?;
        Ok((rows.len(), Some(last)))
    }

    /// Indexes the glossary + dictionary corpus, one transaction.
    fn index_corpus_docs(&self) -> Result<usize, SuramakError> {
        let tx = self.conn.unchecked_transaction()?;
        let written = Self::sync_corpus_docs(&tx)?;
        tx.commit()?;
        Ok(written)
    }

    /// Drops the index and builds it again from the store rows.
    ///
    /// Deterministic on purpose: a corrupt or partial index is never "repaired
    /// in place", it is replaced. The drop/recreate is one transaction, the
    /// backfill then runs in bounded chunks like the migration's.
    pub fn rebuild_search_index(&self) -> Result<usize, SuramakError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch("DROP TABLE IF EXISTS search_fts;")?;
        tx.execute_batch(SEARCH_FTS_DDL)?;
        tx.execute("DELETE FROM search_docs", [])?;
        tx.execute(
            "DELETE FROM meta WHERE key IN ('search_backfill_segments', 'search_backfill_titles')",
            [],
        )?;
        tx.commit()?;
        self.backfill_search_index()
    }

    /// Whether the index agrees with the store rows and passes FTS5's own
    /// integrity check.
    ///
    /// The count comparison catches a document that went missing (an
    /// interrupted old build, a killed process) or a document left behind by a
    /// deleted row; the FTS5 `integrity-check` command catches structural
    /// damage inside the index. A healthy store answers `true` on every open —
    /// the tests pin that, so the empty-text predicate and the sync path can
    /// never drift into a rebuild-every-open loop.
    pub fn search_index_consistent(&self) -> Result<bool, SuramakError> {
        let expected: [(i64, i64); 4] = [
            (
                self.conn.query_row(
                    "SELECT count(*) FROM segments
                     WHERE length(trim(text, char(9) || char(10) || char(13) || ' ')) > 0",
                    [],
                    |row| row.get(0),
                )?,
                self.conn.query_row(
                    "SELECT count(*) FROM search_docs WHERE kind = 'segment'",
                    [],
                    |row| row.get(0),
                )?,
            ),
            (
                self.conn
                    .query_row("SELECT count(*) FROM transcripts", [], |row| row.get(0))?,
                self.conn.query_row(
                    "SELECT count(*) FROM search_docs WHERE kind = 'title'",
                    [],
                    |row| row.get(0),
                )?,
            ),
            (
                self.conn.query_row(
                    "SELECT count(*) FROM glossary_terms
                     WHERE length(trim(term, char(9) || char(10) || char(13) || ' ')) > 0",
                    [],
                    |row| row.get(0),
                )?,
                self.conn.query_row(
                    "SELECT count(*) FROM search_docs WHERE kind = 'glossary'",
                    [],
                    |row| row.get(0),
                )?,
            ),
            (
                self.conn.query_row(
                    "SELECT count(*) FROM dictionary_entries
                     WHERE length(trim(from_text, char(9) || char(10) || char(13) || ' ')) > 0
                        OR length(trim(to_text, char(9) || char(10) || char(13) || ' ')) > 0",
                    [],
                    |row| row.get(0),
                )?,
                self.conn.query_row(
                    "SELECT count(*) FROM search_docs WHERE kind = 'dictionary'",
                    [],
                    |row| row.get(0),
                )?,
            ),
        ];
        if expected.iter().any(|(want, got)| want != got) {
            return Ok(false);
        }
        // The map and the FTS half must agree too: a dropped/recreated index or
        // a document whose FTS row went missing is a partial index even when
        // every map row is present.
        let documents: i64 =
            self.conn
                .query_row("SELECT count(*) FROM search_docs", [], |row| row.get(0))?;
        let indexed: i64 = self
            .conn
            .query_row("SELECT count(*) FROM search_fts", [], |row| row.get(0))?;
        if documents != indexed {
            return Ok(false);
        }
        match self
            .conn
            .execute_batch("INSERT INTO search_fts(search_fts) VALUES('integrity-check')")
        {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// The self-repair gate every [`Store::open`] passes through: an index that
    /// disagrees with the store — or fails FTS5's integrity check — is rebuilt
    /// once, with a single log line naming why.
    fn ensure_search_index(&self) -> Result<(), SuramakError> {
        match self.search_index_consistent() {
            Ok(true) => Ok(()),
            Ok(false) => {
                tracing::warn!(
                    "search index disagrees with the library; rebuilding it from the store rows"
                );
                self.rebuild_search_index()?;
                Ok(())
            }
            Err(err) => {
                tracing::warn!("search index is unreadable ({err}); rebuilding it");
                self.rebuild_search_index()?;
                Ok(())
            }
        }
    }

    /// How many documents of each kind the index holds (the report's and the
    /// tests' window into the index).
    pub fn search_index_stats(&self) -> Result<SearchIndexStats, SuramakError> {
        let count = |kind: &str| -> Result<i64, SuramakError> {
            Ok(self.conn.query_row(
                "SELECT count(*) FROM search_docs WHERE kind = ?1",
                params![kind],
                |row| row.get(0),
            )?)
        };
        Ok(SearchIndexStats {
            segments: count("segment")?,
            titles: count("title")?,
            glossary: count("glossary")?,
            dictionary: count("dictionary")?,
        })
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
        self.insert_transcript_with_words(t, &[])
    }

    /// Inserts a transcript and its estimated word timing in one transaction
    /// (M15.6).
    ///
    /// The alignment is *derived* data about the text that was just stored, so
    /// it lands with it: a crash can never leave a transcript whose word rows
    /// describe a different segment set. The word rows are dropped by the same
    /// rule the store applies everywhere — a segment whose text they do not
    /// spell gets none (see [`Store::replace_word_timings`]).
    pub fn insert_transcript_with_words(
        &self,
        t: &Transcript,
        words: &[WordTiming],
    ) -> Result<(), SuramakError> {
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
                                                 // M15.7: the title is a search document of its own (a title-only match
                                                 // must still find the transcript).
        Self::put_search_doc(
            &tx,
            &Self::title_doc_key(t.id),
            HitKind::Title,
            Some(&t.id.to_string()),
            None,
            &t.title,
            None,
        )?;
        Self::write_word_rows(&tx, t.id, words)?;
        tx.commit()?;
        Ok(())
    }

    /// Replaces a transcript's estimated word timing (M15.6).
    ///
    /// The whole set is rewritten in one savepoint: a re-alignment either
    /// lands completely or the previous rows stay. Only segments that exist
    /// **and** whose text is exactly the words joined by single spaces are
    /// written — the store refuses to persist timing for text it cannot
    /// reproduce, which is the same rule the mapper applies before handing
    /// rows over. Skipped groups are counted so a caller can report them.
    pub fn replace_word_timings(
        &self,
        id: Uuid,
        words: &[WordTiming],
    ) -> Result<WordWriteStats, SuramakError> {
        self.conn.execute_batch("SAVEPOINT replace_word_timings")?;
        let written = (|| -> Result<WordWriteStats, SuramakError> {
            self.conn.execute(
                "DELETE FROM word_timings WHERE transcript_id = ?1",
                params![id.to_string()],
            )?;
            Self::write_word_rows(&self.conn, id, words)
        })();
        match written {
            Ok(stats) => {
                self.conn.execute_batch("RELEASE replace_word_timings")?;
                Ok(stats)
            }
            Err(err) => {
                let _ = self.conn.execute_batch(
                    "ROLLBACK TO replace_word_timings; RELEASE replace_word_timings",
                );
                Err(err)
            }
        }
    }

    /// The usable word timing of a transcript, grouped per segment (M15.6).
    ///
    /// A row counts as **current** only when its sticky `stale` flag is clear
    /// *and* the segment still says exactly the text the row was aligned to;
    /// anything else is reported as stale, never silently used. A segment with
    /// no rows at all is simply absent — the UI's segment fallback.
    pub fn word_alignment(&self, id: Uuid) -> Result<WordAlignment, SuramakError> {
        let mut stmt = self.conn.prepare(
            "SELECT w.segment_idx, w.word_idx, w.start_ms, w.end_ms, w.text, w.stale,
                    s.text = w.aligned_text
             FROM word_timings w
             JOIN segments s
               ON s.transcript_id = w.transcript_id AND s.idx = w.segment_idx
             WHERE w.transcript_id = ?1
             ORDER BY w.segment_idx, w.word_idx",
        )?;
        let rows = stmt.query_map(params![id.to_string()], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)? != 0,
                row.get::<_, i64>(6)? != 0,
            ))
        })?;

        let mut alignment = WordAlignment::default();
        for row in rows {
            let (segment_idx, word_idx, start_ms, end_ms, text, stale, text_matches) = row?;
            if stale || !text_matches {
                alignment.stale.insert(segment_idx);
                continue;
            }
            alignment
                .current
                .entry(segment_idx)
                .or_default()
                .push(WordTiming {
                    segment_idx,
                    word_idx,
                    start_ms,
                    end_ms,
                    text,
                });
        }
        Ok(alignment)
    }

    /// Writes one word-row set for `id` on `conn` (inside the caller's
    /// transaction), dropping any group that does not spell its segment.
    fn write_word_rows(
        conn: &Connection,
        id: Uuid,
        words: &[WordTiming],
    ) -> Result<WordWriteStats, SuramakError> {
        let mut stats = WordWriteStats::default();
        let mut insert = conn.prepare(
            "INSERT INTO word_timings(transcript_id, segment_idx, word_idx,
                                      start_ms, end_ms, text, aligned_text, stale)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
        )?;
        let mut text_of =
            conn.prepare("SELECT text FROM segments WHERE transcript_id = ?1 AND idx = ?2")?;

        let mut groups: BTreeMap<u32, Vec<&WordTiming>> = BTreeMap::new();
        for word in words {
            groups.entry(word.segment_idx).or_default().push(word);
        }
        for (segment_idx, group) in groups {
            let text: Option<String> = text_of
                .query_row(params![id.to_string(), segment_idx], |row| row.get(0))
                .optional()?;
            let Some(text) = text else {
                // The segment vanished (a structural edit raced the job): the
                // rows would describe nothing.
                stats.skipped_segments += 1;
                continue;
            };
            let spoken: Vec<&str> = group.iter().map(|word| word.text.as_str()).collect();
            let stored: Vec<&str> = text.split_whitespace().collect();
            if spoken != stored {
                stats.skipped_segments += 1;
                continue;
            }
            for word in group {
                insert.execute(params![
                    id.to_string(),
                    word.segment_idx,
                    word.word_idx,
                    word.start_ms,
                    word.end_ms,
                    word.text,
                    text,
                ])?;
                stats.written += 1;
            }
        }
        Ok(stats)
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
    ///
    /// M15.8: a wholesale text rewrite through this path (a re-polish) also
    /// marks an existing translation stale in the same statement — the derived
    /// text no longer describes what the row says, and the badge must say so.
    fn write_segments(&self, id: Uuid, segments: &[Segment]) -> Result<(), SuramakError> {
        for seg in segments {
            self.conn.execute(
                "INSERT INTO segments(transcript_id, idx, start_ms, end_ms, text, source_text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(transcript_id, idx) DO UPDATE SET
                     start_ms    = excluded.start_ms,
                     end_ms      = excluded.end_ms,
                     text        = excluded.text,
                     source_text = COALESCE(segments.source_text, excluded.source_text),
                     translation_stale = CASE
                         WHEN segments.translation IS NOT NULL
                              AND segments.text IS NOT excluded.text
                         THEN 1 ELSE segments.translation_stale END",
                params![id.to_string(), seg.idx, seg.start_ms, seg.end_ms, seg.text],
            )?;
            // M15.7: read the row back — `source_text` is COALESCE-preserved,
            // so the upserted value is not necessarily what `seg` carried.
            Self::sync_segment_doc(&self.conn, id, seg.idx)?;
        }
        let deleted = self.conn.execute(
            "DELETE FROM segments WHERE transcript_id = ?1 AND idx >= ?2",
            params![id.to_string(), segments.len() as i64],
        )?;
        if deleted > 0 {
            Self::delete_segment_docs_from(&self.conn, id, segments.len() as u32)?;
        }
        // M15.6: any row whose text is no longer exactly the text its word
        // timing was computed from becomes stale — in the same transaction as
        // the text write, so a reader can never see new text next to word
        // rows that still claim to describe it.
        Self::mark_alignment_stale(&self.conn, id)?;
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
    ///
    /// M15.8: when the row carries an opt-in translation, any real text change
    /// marks it stale in the same transaction — the badge and the re-translate
    /// affordance are the user's, never a silent overwrite.
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
                "UPDATE segments SET
                     translation_stale = CASE
                         WHEN translation IS NOT NULL AND text IS NOT ?1 THEN 1
                         ELSE translation_stale END,
                     text = ?1
                 WHERE transcript_id = ?2 AND idx = ?3",
                params![text, transcript_id.to_string(), idx],
            )?;
            // M15.7: the index follows the same transaction — an edit that
            // fails to reindex must fail as a whole, not leave search stale.
            Self::sync_segment_doc(&self.conn, transcript_id, idx)?;
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
            "SELECT idx, start_ms, end_ms, text, source_text, derived,
                    translation, translation_model, translation_at,
                    translation_sources, translation_stale
             FROM segments WHERE transcript_id = ?1 ORDER BY idx",
        )?;
        let rows = stmt.query_map(params![id.to_string()], |row| {
            let translation: Option<String> = row.get(6)?;
            let translation = match translation {
                Some(text) => Some(TranslationRecord {
                    text,
                    model: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                    translated_at: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                    source_ids: decode_source_ids(row.get::<_, Option<String>>(9)?),
                    stale: row.get::<_, i64>(10)? != 0,
                }),
                None => None,
            };
            Ok(SegmentRow {
                idx: row.get(0)?,
                start_ms: row.get(1)?,
                end_ms: row.get(2)?,
                text: row.get(3)?,
                source_text: row.get(4)?,
                derived: row.get(5)?,
                translation,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(SuramakError::Sqlite)
    }

    /// Removes a file with a short retry/backoff for Windows transient lockers
    /// (AV scanner, search indexer) that surface as `Access is denied`;
    /// NotFound counts as done, and unix behavior is a single attempt.
    fn remove_file_with_retry(path: &Path) -> Result<(), SuramakError> {
        let mut outcome: Option<std::io::Error> = None;
        for attempt in 0..5u32 {
            if attempt > 0 && outcome.is_some() {
                std::thread::sleep(std::time::Duration::from_millis(
                    50 + u64::from(attempt) * 50,
                ));
            }
            match std::fs::remove_file(path) {
                Ok(()) => return Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied && attempt < 4 => {
                    outcome = Some(err);
                }
                Err(err) => return Err(SuramakError::Io(err)),
            }
        }
        match outcome {
            Some(err) => Err(SuramakError::Backup(format!(
                "remove_file({}) stayed locked: {err}",
                path.display()
            ))),
            _ => Ok(()),
        }
    }

    /// The Windows replace falls back to remove-then-rename; the rename can
    /// The Windows replace falls back to remove-then-rename; the rename can
    /// also lose a transient lock (`Access is denied` on Windows for an
    /// existing destination), so it gets the same bounded backoff.
    #[cfg(windows)]
    fn rename_with_retry(from: &Path, to: &Path) -> Result<(), SuramakError> {
        let mut outcome: Option<std::io::Error> = None;
        for attempt in 0..5u32 {
            if attempt > 0 && outcome.is_some() {
                std::thread::sleep(std::time::Duration::from_millis(
                    50 + u64::from(attempt) * 50,
                ));
            }
            match std::fs::rename(from, to) {
                Ok(()) => return Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied && attempt < 4 => {
                    outcome = Some(err);
                }
                Err(err) => return Err(SuramakError::Io(err)),
            }
        }
        match outcome {
            Some(err) => Err(SuramakError::Backup(format!(
                "rename({}, {}) stayed locked: {err}",
                from.display(),
                to.display()
            ))),
            _ => Ok(()),
        }
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
        // M15.7: a structural edit rewrites every index that follows an `idx`
        // (a split shifts the tail down, a merge up), so the transcript's
        // segment documents are replaced with the new row set in one go. The
        // title document is untouched (same id, same title).
        Self::delete_all_segment_docs(conn, id)?;
        let mut stmt = conn.prepare(
            "INSERT INTO segments(transcript_id, idx, start_ms, end_ms, text, source_text, derived,
                                  translation, translation_model, translation_at,
                                  translation_sources, translation_stale)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )?;
        for row in rows {
            let (translation, model, at, sources, stale) = match &row.translation {
                Some(record) => (
                    Some(record.text.as_str()),
                    Some(record.model.as_str()),
                    Some(record.translated_at.as_str()),
                    Some(encode_source_ids(&record.source_ids)),
                    if record.stale { 1 } else { 0 },
                ),
                None => (None, None, None, None, 0),
            };
            stmt.execute(params![
                id.to_string(),
                row.idx,
                row.start_ms,
                row.end_ms,
                row.text,
                row.source_text,
                row.derived,
                translation,
                model,
                at,
                sources,
                stale,
            ])?;
            if !Self::has_searchable_text(&row.text) {
                continue;
            }
            let original = row
                .source_text
                .as_deref()
                .filter(|source| Self::has_searchable_text(source) && *source != row.text);
            Self::put_search_doc(
                conn,
                &Self::segment_doc_key(id, row.idx),
                HitKind::Segment,
                Some(&id.to_string()),
                Some(row.idx),
                &row.text,
                original,
            )?;
        }
        // M15.6: a structural edit rewrites the row set (a split shifts the tail
        // down, a merge up), so any word rows that no longer sit on a segment
        // with exactly their text are stale. Same rule as `write_segments`.
        Self::mark_alignment_stale(conn, id)?;
        Ok(())
    }

    /// Marks every word-timing row whose segment no longer says exactly the
    /// text the row was computed from (M15.6).
    ///
    /// One statement, one rule, every mutation path: an edited row, a split, a
    /// merge, a wholesale re-polish and an undo of any of those all land here.
    /// Rows that still match stay usable (an unrelated segment keeps its
    /// karaoke), and the flag is only ever set — a fresh alignment is the one
    /// thing that clears it, exactly like the bilingual stale marker.
    fn mark_alignment_stale(conn: &Connection, id: Uuid) -> Result<(), SuramakError> {
        conn.execute(
            "UPDATE word_timings SET stale = 1
             WHERE transcript_id = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM segments s
                   WHERE s.transcript_id = word_timings.transcript_id
                     AND s.idx = word_timings.segment_idx
                     AND s.text = word_timings.aligned_text
               )",
            params![id.to_string()],
        )?;
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

    // ---- M15.8: the opt-in bilingual derived text --------------------------

    /// Writes one translation run's records (one `(idx, record)` per row) in a
    /// single savepoint, touching `updated_at` once.
    ///
    /// The record carries its own provenance; the store never invents a model
    /// or a timestamp. A row that no longer exists is skipped rather than
    /// resurrected (the transcript may have been restructured while the run was
    /// in flight); the returned count is what actually landed.
    pub fn set_translations(
        &self,
        id: Uuid,
        records: &[(u32, TranslationRecord)],
    ) -> Result<usize, SuramakError> {
        self.conn.execute_batch("SAVEPOINT set_translations")?;
        let written = (|| -> Result<usize, SuramakError> {
            let mut stmt = self.conn.prepare(
                "UPDATE segments SET
                     translation = ?1,
                     translation_model = ?2,
                     translation_at = ?3,
                     translation_sources = ?4,
                     translation_stale = ?5
                 WHERE transcript_id = ?6 AND idx = ?7",
            )?;
            let mut written = 0usize;
            for (idx, record) in records {
                written += stmt.execute(params![
                    record.text,
                    record.model,
                    record.translated_at,
                    encode_source_ids(&record.source_ids),
                    if record.stale { 1 } else { 0 },
                    id.to_string(),
                    idx,
                ])?;
            }
            self.conn.execute(
                "UPDATE transcripts SET updated_at = ?1 WHERE id = ?2",
                params![Utc::now().to_rfc3339(), id.to_string()],
            )?;
            Ok(written)
        })();
        match written {
            Ok(written) => {
                self.conn.execute_batch("RELEASE set_translations")?;
                Ok(written)
            }
            Err(err) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO set_translations; RELEASE set_translations");
                Err(err)
            }
        }
    }

    /// Rewrites one row's **derived** text (the editable translation column)
    /// without touching the source text, the provenance or the stale flag.
    ///
    /// Editing a translation is not an event that invalidates itself; the
    /// provenance stays exactly the model/timestamp/ids that produced the row,
    /// and a stale row stays visibly stale until the user re-translates it.
    pub fn update_segment_translation(
        &self,
        id: Uuid,
        idx: u32,
        text: &str,
    ) -> Result<(), SuramakError> {
        self.conn.execute_batch("SAVEPOINT update_translation")?;
        let written = (|| -> Result<(), SuramakError> {
            let changed = self.conn.execute(
                "UPDATE segments SET translation = ?1
                 WHERE transcript_id = ?2 AND idx = ?3 AND translation IS NOT NULL",
                params![text, id.to_string(), idx],
            )?;
            if changed == 0 {
                return Err(SuramakError::TranscriptNotFound(format!(
                    "no translation for segment {idx}"
                )));
            }
            self.conn.execute(
                "UPDATE transcripts SET updated_at = ?1 WHERE id = ?2",
                params![Utc::now().to_rfc3339(), id.to_string()],
            )?;
            Ok(())
        })();
        match written {
            Ok(()) => {
                self.conn.execute_batch("RELEASE update_translation")?;
                Ok(())
            }
            Err(err) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO update_translation; RELEASE update_translation");
                Err(err)
            }
        }
    }

    /// Every stored translation for a transcript, in `idx` order.
    pub fn segment_translations(
        &self,
        id: Uuid,
    ) -> Result<Vec<(u32, TranslationRecord)>, SuramakError> {
        let mut stmt = self.conn.prepare(
            "SELECT idx, translation, translation_model, translation_at,
                    translation_sources, translation_stale
             FROM segments
             WHERE transcript_id = ?1 AND translation IS NOT NULL
             ORDER BY idx",
        )?;
        let rows = stmt.query_map(params![id.to_string()], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                TranslationRecord {
                    text: row.get(1)?,
                    model: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    translated_at: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    source_ids: decode_source_ids(row.get::<_, Option<String>>(4)?),
                    stale: row.get::<_, i64>(5)? != 0,
                },
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(SuramakError::Sqlite)
    }

    /// Writes a row set back verbatim (undo/redo of a [`SegmentSnapshot`]).
    ///
    /// This is the exact inverse of a structural edit: the rows are the ones the
    /// transaction recorded, so no re-derivation can drift. One savepoint,
    /// `updated_at` touched like any other modification.
    ///
    /// M15.8: stale markers **survive the restore**. A row position that is
    /// currently stale stays stale even when the restored snapshot predates the
    /// edit that made it stale; re-translation is the only thing that clears a
    /// marker. The restore can therefore only be conservative (it may keep a
    /// badge that a strict history replay would drop), never silently vouch for
    /// derived text it did not re-check.
    pub fn restore_segment_rows(&self, id: Uuid, rows: &[SegmentRow]) -> Result<(), SuramakError> {
        // The stale flags in the *current* row set, read before the replace.
        let current: std::collections::HashMap<u32, bool> = Self::read_rows(&self.conn, id)?
            .into_iter()
            .filter_map(|row| row.translation.map(|record| (row.idx, record.stale)))
            .collect();
        let mut rows = rows.to_vec();
        for row in &mut rows {
            if let Some(record) = &mut row.translation {
                if current.get(&row.idx).copied().unwrap_or(false) {
                    record.stale = true;
                }
            }
        }
        self.conn.execute_batch("SAVEPOINT segment_restore")?;
        let written = (|| -> Result<(), SuramakError> {
            Self::replace_rows(&self.conn, id, &rows)?;
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
            // M15.7: the glossary/dictionary corpus is searchable too; it is
            // small, so it is re-indexed wholesale inside this transaction.
            Self::sync_corpus_docs(&self.conn)?;
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

    /// Full-text search over transcript titles, segment text and the text-rule
    /// corpus (M15.7).
    ///
    /// Returns transcript headers only, newest first — the shape the pre-M15.7
    /// LIKE scan returned, so existing callers keep working. [`Store::search_hits`]
    /// is the richer surface the library view uses: snippets, timestamps and
    /// the glossary/dictionary rows.
    ///
    /// A hit is a **document**, not a transcript: one transcript with three
    /// matching segments yields three rows in `search_hits` and appears once
    /// here. Both `segments.text` and `segments.source_text` are indexed: an
    /// edit that removes a word still finds the segment through the preserved
    /// inference text (the hit says which column matched).
    pub fn search(&self, query: &str) -> Result<Vec<Transcript>, SuramakError> {
        let Some(query) = search::match_query(query) else {
            return Ok(Vec::new());
        };
        let ids: HashSet<Uuid> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT d.transcript_id
                 FROM search_fts
                 JOIN search_docs d ON d.rowid = search_fts.rowid
                 WHERE search_fts MATCH ?1 AND d.transcript_id IS NOT NULL",
            )?;
            let rows = stmt.query_map(params![query.expression], |row| row.get::<_, String>(0))?;
            let mut ids = HashSet::new();
            for row in rows {
                let raw = row.map_err(SuramakError::Sqlite)?;
                if let Ok(id) = Uuid::parse_str(&raw) {
                    ids.insert(id);
                }
            }
            ids
        };
        let mut list = self.list_transcripts()?;
        list.retain(|transcript| ids.contains(&transcript.id));
        Ok(list)
    }

    /// The library search: ranked hits with snippets, timestamps and corpus
    /// rows (M15.7).
    ///
    /// Every result is one document — a segment (with the transcript title and
    /// its start/end), a transcript title, or a glossary/dictionary entry —
    /// ordered by FTS5's `bm25` (best first). `total` is the number of matches
    /// before `options.limit`, so a caller can say "showing 50 of 214".
    ///
    /// A segment's snippet prefers the current `text`; when only the preserved
    /// `source_text` matches, the snippet comes from that column and
    /// [`SearchHit::from_source_text`] says so.
    pub fn search_hits(
        &self,
        query_text: &str,
        options: SearchOptions,
    ) -> Result<SearchResults, SuramakError> {
        let Some(query) = search::match_query(query_text) else {
            return Ok(SearchResults::default());
        };
        let context = options.context();
        let limit = options.limit() as i64;
        let total: i64 = self.conn.query_row(
            "SELECT count(*) FROM search_fts WHERE search_fts MATCH ?1",
            params![query.expression],
            |row| row.get(0),
        )?;

        let mut stmt = self.conn.prepare(
            "SELECT d.kind, d.transcript_id, d.segment_idx, t.title,
                    search_fts.body, search_fts.original,
                    s.start_ms, s.end_ms
             FROM search_fts
             JOIN search_docs d ON d.rowid = search_fts.rowid
             LEFT JOIN transcripts t ON t.id = d.transcript_id
             LEFT JOIN segments s
                    ON s.transcript_id = d.transcript_id AND s.idx = d.segment_idx
             WHERE search_fts MATCH ?1
             ORDER BY bm25(search_fts)
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query.expression, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<u32>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<i64>>(7)?,
            ))
        })?;

        let mut hits = Vec::new();
        for row in rows {
            let (kind, transcript_id, segment_idx, title, body, original, start_ms, end_ms) =
                row.map_err(SuramakError::Sqlite)?;
            let Some(kind) = HitKind::parse(&kind) else {
                continue;
            };
            let mut from_source_text = false;
            let mut found = search::snippet(&body, &query.terms, context);
            if found.highlight.is_none() {
                if let Some(original) = original.as_deref() {
                    let alternative = search::snippet(original, &query.terms, context);
                    if alternative.highlight.is_some() {
                        found = alternative;
                        from_source_text = true;
                    }
                }
            }
            hits.push(SearchHit {
                kind,
                transcript_id: transcript_id.and_then(|raw| Uuid::parse_str(&raw).ok()),
                title,
                segment_idx,
                start_ms,
                end_ms,
                snippet: found.text,
                highlight: found.highlight,
                from_source_text,
            });
        }
        Ok(SearchResults {
            hits,
            total: total as usize,
        })
    }

    /// Deletes a transcript and its segments (cascade).
    pub fn delete_transcript(&self, id: Uuid) -> Result<(), SuramakError> {
        // M15.7: the FTS rows have no foreign key to cascade through, so the
        // delete is one savepoint: index documents and the header row go
        // together or not at all.
        self.conn.execute_batch("SAVEPOINT delete_transcript")?;
        let deleted = (|| -> Result<usize, SuramakError> {
            Self::delete_docs_for_transcript(&self.conn, id)?;
            Ok(self.conn.execute(
                "DELETE FROM transcripts WHERE id = ?1",
                params![id.to_string()],
            )?)
        })();
        match deleted {
            Ok(0) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO delete_transcript; RELEASE delete_transcript");
                Err(SuramakError::TranscriptNotFound(id.to_string()))
            }
            Ok(_) => {
                self.conn.execute_batch("RELEASE delete_transcript")?;
                Ok(())
            }
            Err(err) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO delete_transcript; RELEASE delete_transcript");
                Err(err)
            }
        }
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
        // Windows note: `File::open` (read-only) then `sync_all` fails with
        // ERROR_ACCESS_DENIED — Windows requires GENERIC_WRITE for
        // FlushFileBuffers. Open read+write so the durability fence is real on
        // every platform (the file is ours as of the copy a line above).
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&tmp)?
            .sync_all()?;

        // The old file's WAL belongs to the old database; leaving it beside the
        // restored file would replay unrelated frames into the new one.
        for suffix in ["-wal", "-shm"] {
            let sidecar = std::path::PathBuf::from(format!("{}{suffix}", db.display()));
            Self::remove_file_with_retry(&sidecar)?;
        }
        // Unix `rename` replaces atomically. Windows `std::fs::rename` refuses
        // an existing destination, so there the replace is remove-then-rename;
        // a crash between the two leaves the validated temp file behind, never
        // a half-written database. Windows also loses the race to transient
        // file locks (watchdog/indexer/AV briefly holding the db or the tmp
        // copy → "Access is denied", code 5), so the fallback retries a few
        // times with a short backoff before surfacing the error.
        match std::fs::rename(&tmp, db) {
            Ok(()) => Ok(()),
            Err(_err) if cfg!(windows) && db.exists() => {
                #[cfg(windows)]
                {
                    Self::remove_file_with_retry(db)?;
                    Self::rename_with_retry(&tmp, db)
                }
                #[cfg(not(windows))]
                {
                    unreachable!("the fallback only runs on windows")
                }
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
        // still one hit; the index reads the new rows, not a stale set.
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

        // A word the edit removes from the current text is still findable
        // through the preserved inference text (M15.7's "either column"), and
        // the hit says which column it came from.
        store.update_segment_text(t.id, 0, "hai").unwrap();
        let hits = store
            .search_hits("halo", SearchOptions::default())
            .expect("search");
        assert_eq!(hits.total, 1, "the preserved original still matches");
        assert!(hits.hits[0].from_source_text);
        assert!(
            hits.hits[0].snippet.contains("halo"),
            "the snippet comes from the column that matched: {}",
            hits.hits[0].snippet
        );
        let current = store
            .search_hits("hai", SearchOptions::default())
            .expect("search");
        assert_eq!(current.total, 1);
        assert!(!current.hits[0].from_source_text);

        // Merge the two halves back: the joined text is one row again.
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

    // ---- M15.8: the opt-in bilingual derived text ---------------------------

    fn translated(idx: u32, text: &str) -> (u32, TranslationRecord) {
        (
            idx,
            TranslationRecord::new(text, "mock-model", "2026-11-10T10:00:00+00:00", vec![idx]),
        )
    }

    #[test]
    fn translations_round_trip_with_their_provenance() {
        let store = Store::open_in_memory().unwrap();
        let t = sample("bilingual");
        let id = t.id;
        store.insert_transcript(&t).unwrap();
        assert!(store.segment_translations(id).unwrap().is_empty());

        let written = store
            .set_translations(id, &[translated(0, "pertama"), translated(1, "kedua")])
            .unwrap();
        assert_eq!(written, 2);
        let rows = store.segment_translations(id).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, 0);
        assert_eq!(rows[0].1.text, "pertama");
        assert_eq!(rows[0].1.model, "mock-model");
        assert_eq!(rows[0].1.translated_at, "2026-11-10T10:00:00+00:00");
        assert_eq!(rows[0].1.source_ids, vec![0]);
        assert!(!rows[0].1.stale);

        // A fresh read through the row path sees the same record.
        let row = &store.segment_rows(id).unwrap()[0];
        assert_eq!(row.translation.as_ref().unwrap().text, "pertama");
        assert_eq!(row.translation.as_ref().unwrap().source_ids, vec![0]);
    }

    #[test]
    fn editing_the_source_marks_the_translation_stale_but_editing_the_translation_does_not() {
        let store = Store::open_in_memory().unwrap();
        let t = sample("stale");
        let id = t.id;
        store.insert_transcript(&t).unwrap();
        store
            .set_translations(id, &[translated(0, "pertama")])
            .unwrap();

        store
            .update_segment_text(id, 0, "first, corrected")
            .unwrap();
        let rows = store.segment_translations(id).unwrap();
        assert!(
            rows[0].1.stale,
            "a source edit marks the derived text stale"
        );

        // Editing the derived text itself keeps provenance and the stale flag
        // (it is still the same machine translation, just user-corrected).
        store
            .update_segment_translation(id, 0, "pertama (edited)")
            .unwrap();
        let rows = store.segment_translations(id).unwrap();
        assert_eq!(rows[0].1.text, "pertama (edited)");
        assert!(
            rows[0].1.stale,
            "editing the translation does not clear stale"
        );
        assert_eq!(rows[0].1.model, "mock-model");

        // A re-translate clears it and replaces the text + provenance.
        store
            .set_translations(
                id,
                &[(
                    0,
                    TranslationRecord::new(
                        "pertama lagi",
                        "mock-model-2",
                        "2026-11-10T11:00:00+00:00",
                        vec![0],
                    ),
                )],
            )
            .unwrap();
        let rows = store.segment_translations(id).unwrap();
        assert!(!rows[0].1.stale);
        assert_eq!(rows[0].1.text, "pertama lagi");
        assert_eq!(rows[0].1.model, "mock-model-2");
    }

    #[test]
    fn a_no_op_text_save_does_not_mark_a_translation_stale() {
        let store = Store::open_in_memory().unwrap();
        let t = sample("noop");
        let id = t.id;
        store.insert_transcript(&t).unwrap();
        store
            .set_translations(id, &[translated(0, "pertama")])
            .unwrap();
        // Saving the same text (the autosave can race a no-op) must not badge.
        store.update_segment_text(id, 0, "first").unwrap();
        assert!(!store.segment_translations(id).unwrap()[0].1.stale);

        // Neither does a wholesale upsert that leaves the text byte-identical.
        let mut t = store.get_transcript(id).unwrap();
        store.upsert_segments(id, &t.segments).unwrap();
        assert!(!store.segment_translations(id).unwrap()[0].1.stale);
        // …but a rewritten one does.
        t.segments[0].text = "changed".to_string();
        store.upsert_segments(id, &t.segments).unwrap();
        assert!(store.segment_translations(id).unwrap()[0].1.stale);
    }

    #[test]
    fn split_duplicates_the_translation_and_marks_both_halves_stale() {
        let store = Store::open_in_memory().unwrap();
        let mut t = sample("split");
        t.segments[0].text = "halo dunia".to_string();
        let id = t.id;
        store.insert_transcript(&t).unwrap();
        store
            .set_translations(id, &[translated(0, "hello world")])
            .unwrap();

        store
            .split_segment(id, 0, 4, Some(400), ZeroDuration::OneFrame)
            .unwrap();
        let rows = store.segment_rows(id).unwrap();
        assert_eq!(rows.len(), 3);
        for row in &rows[..2] {
            let record = row.translation.as_ref().expect("both halves keep it");
            assert_eq!(record.text, "hello world");
            assert!(record.stale, "a split invalidates the alignment");
            assert_eq!(record.source_ids, vec![0], "provenance is the parent's");
        }
        assert!(rows[2].translation.is_none());
    }

    #[test]
    fn merge_joins_translations_and_marks_the_result_stale() {
        let store = Store::open_in_memory().unwrap();
        let t = sample("merge");
        let id = t.id;
        store.insert_transcript(&t).unwrap();
        store
            .set_translations(id, &[translated(0, "pertama"), translated(1, "kedua")])
            .unwrap();

        store.merge_segments(id, 0, 2, " ").unwrap();
        let rows = store.segment_rows(id).unwrap();
        assert_eq!(rows.len(), 1);
        let record = rows[0].translation.as_ref().unwrap();
        assert_eq!(record.text, "pertama kedua");
        assert!(record.stale);
        assert_eq!(record.model, "mock-model");
        assert_eq!(record.source_ids, vec![0, 1]);
    }

    #[test]
    fn the_stale_marker_survives_restore_and_is_only_cleared_by_retranslation() {
        let store = Store::open_in_memory().unwrap();
        let mut t = sample("restore");
        t.segments[0].text = "halo dunia".to_string();
        let id = t.id;
        store.insert_transcript(&t).unwrap();
        store
            .set_translations(id, &[translated(0, "hello world")])
            .unwrap();

        let before = store.segment_rows(id).unwrap();
        let snapshot = store
            .split_segment(id, 0, 4, Some(400), ZeroDuration::OneFrame)
            .unwrap();
        assert!(
            store.segment_rows(id).unwrap()[0]
                .translation
                .as_ref()
                .unwrap()
                .stale
        );

        // Undo writes the pre-split rows back, but the stale badge survives:
        // the marker is monotonic across a restore.
        store.restore_segment_rows(id, &before).unwrap();
        let restored = store.segment_rows(id).unwrap();
        assert_eq!(restored.len(), 2);
        let record = restored[0].translation.as_ref().unwrap();
        assert_eq!(record.text, "hello world");
        assert!(record.stale, "the stale marker survives restore");
        let _ = snapshot;

        // Re-translation is the one action that clears it.
        store
            .set_translations(id, &[translated(0, "hello world")])
            .unwrap();
        assert!(!store.segment_translations(id).unwrap()[0].1.stale);
    }

    #[test]
    fn a_v5_database_upgrades_to_v6_additively() {
        let dir = std::env::temp_dir().join(format!("suramak-v6-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("suramak.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE transcripts (
                     id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at TEXT NOT NULL,
                     updated_at TEXT NOT NULL, source_json TEXT NOT NULL, model TEXT NOT NULL,
                     language TEXT, audio_path TEXT);
                 CREATE TABLE segments (
                     transcript_id TEXT NOT NULL, idx INTEGER NOT NULL, start_ms INTEGER NOT NULL,
                     end_ms INTEGER NOT NULL, text TEXT NOT NULL, source_text TEXT,
                     derived TEXT NOT NULL DEFAULT '', translation TEXT,
                     translation_model TEXT, translation_at TEXT, translation_sources TEXT,
                     translation_stale INTEGER NOT NULL DEFAULT 0,
                     PRIMARY KEY (transcript_id, idx));
                 INSERT INTO meta(key, value) VALUES ('schema_version', '5');
                 INSERT INTO transcripts VALUES (
                     '00000000-0000-0000-0000-000000000001', 'old', '2024-01-01T00:00:00Z',
                     '2024-01-01T00:00:00Z', '{\"type\":\"file\",\"name\":\"old.wav\"}', 'tiny', 'en', NULL);
                 INSERT INTO segments VALUES (
                     '00000000-0000-0000-0000-000000000001', 0, 0, 1000, 'halo dunia', 'halo dunia', '',
                     NULL, NULL, NULL, NULL, 0);",
            )
            .unwrap();
        }
        let store = Store::open(&path).expect("upgrade");
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        let id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        // The library it carried is untouched, and it starts with no alignment.
        assert_eq!(store.segment_rows(id).unwrap()[0].text, "halo dunia");
        assert!(store.word_alignment(id).unwrap().is_empty());
        // The new table accepts rows after the upgrade.
        let stats = store
            .replace_word_timings(
                id,
                &[word(0, 0, "halo", 0, 400), word(0, 1, "dunia", 400, 1000)],
            )
            .unwrap();
        assert_eq!(stats.written, 2);
        assert_eq!(
            store.word_alignment(id).unwrap().segment(0).unwrap().len(),
            2
        );
        drop(store);
        let again = Store::open(&path).expect("reopen");
        assert_eq!(again.schema_version().unwrap(), SCHEMA_VERSION);
        drop(again);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- M15.6: estimated word timing --------------------------------------

    fn word(segment_idx: u32, word_idx: u32, text: &str, start_ms: i64, end_ms: i64) -> WordTiming {
        WordTiming {
            segment_idx,
            word_idx,
            start_ms,
            end_ms,
            text: text.to_string(),
        }
    }

    /// One word per segment — `sample`'s own text, so the store's "must spell
    /// the segment" rule accepts them.
    fn sample_words() -> Vec<WordTiming> {
        vec![
            word(0, 0, "first", 0, 900),
            word(1, 0, "second", 900, 1_800),
        ]
    }

    #[test]
    fn word_timing_round_trips_and_reads_back_current() {
        let store = Store::open_in_memory().unwrap();
        let t = sample("words");
        store
            .insert_transcript_with_words(&t, &sample_words())
            .unwrap();
        let alignment = store.word_alignment(t.id).unwrap();
        assert!(!alignment.is_empty());
        assert_eq!(alignment.segment(0).unwrap(), &sample_words()[..1]);
        assert_eq!(alignment.segment(1).unwrap(), &sample_words()[1..]);
        assert!(alignment.stale.is_empty());
        // The insert wrote the rows in the same transaction as the segments.
        assert_eq!(alignment.words().count(), 2);
    }

    #[test]
    fn a_word_group_that_does_not_spell_its_segment_is_refused() {
        let store = Store::open_in_memory().unwrap();
        let t = sample("mismatch");
        store.insert_transcript(&t).unwrap();
        let stats = store
            .replace_word_timings(
                t.id,
                &[
                    word(0, 0, "something", 0, 100),
                    word(0, 1, "else", 100, 200),
                ],
            )
            .unwrap();
        assert_eq!(stats.written, 0, "unaligned text must never be persisted");
        assert_eq!(stats.skipped_segments, 1);
        assert!(store.word_alignment(t.id).unwrap().is_empty());
        // …and a row for a segment that does not exist is refused too.
        let stats = store
            .replace_word_timings(t.id, &[word(9, 0, "first", 0, 100)])
            .unwrap();
        assert_eq!(stats.written, 0);
        assert_eq!(stats.skipped_segments, 1);
    }

    #[test]
    fn editing_a_segment_marks_only_that_segments_words_stale() {
        let store = Store::open_in_memory().unwrap();
        let mut t = sample("edit-stales-words");
        t.segments[1].text = "another".to_string();
        store
            .insert_transcript_with_words(
                &t,
                &[
                    word(0, 0, "first", 0, 400),
                    word(1, 0, "another", 900, 1_800),
                ],
            )
            .unwrap();
        store.update_segment_text(t.id, 0, "changed").unwrap();
        let alignment = store.word_alignment(t.id).unwrap();
        assert!(
            alignment.segment(0).is_none(),
            "the edited row lost its timing"
        );
        assert!(alignment.stale.contains(&0));
        assert!(
            alignment.segment(1).is_some(),
            "an untouched segment keeps its timing"
        );
    }

    #[test]
    fn a_no_op_text_save_keeps_the_alignment_current() {
        let store = Store::open_in_memory().unwrap();
        let t = sample("noop");
        store
            .insert_transcript_with_words(&t, &sample_words())
            .unwrap();
        store.update_segment_text(t.id, 0, "first").unwrap();
        assert!(store.word_alignment(t.id).unwrap().stale.is_empty());
    }

    #[test]
    fn a_split_marks_the_alignment_stale_and_only_realignment_clears_it() {
        let store = Store::open_in_memory().unwrap();
        let mut t = sample("split");
        t.segments[0].text = "hello world".to_string();
        store
            .insert_transcript_with_words(
                &t,
                &[word(0, 0, "hello", 0, 400), word(0, 1, "world", 400, 900)],
            )
            .unwrap();
        let before = store.segment_rows(t.id).unwrap();
        store
            .split_segment(t.id, 0, 5, Some(400), ZeroDuration::OneFrame)
            .unwrap();
        let alignment = store.word_alignment(t.id).unwrap();
        assert!(alignment.stale.contains(&0), "a split invalidates the row");
        assert!(alignment.segment(0).is_none());

        // Undo restores the text, but the marker is monotonic — the alignment
        // must be re-derived, not silently trusted again.
        store.restore_segment_rows(t.id, &before).unwrap();
        let alignment = store.word_alignment(t.id).unwrap();
        assert!(alignment.segment(0).is_none());
        assert!(alignment.stale.contains(&0));

        // Re-alignment is the one action that clears it.
        store
            .replace_word_timings(
                t.id,
                &[word(0, 0, "hello", 0, 400), word(0, 1, "world", 400, 900)],
            )
            .unwrap();
        let alignment = store.word_alignment(t.id).unwrap();
        assert!(alignment.stale.is_empty());
        assert_eq!(alignment.segment(0).unwrap().len(), 2);
    }

    #[test]
    fn a_structurally_moved_row_cannot_reuse_its_word_timing() {
        // Merge row 0 into row 1 by rewriting the set: the row at idx 1 now
        // says something else, so the timing that sat there is stale.
        let store = Store::open_in_memory().unwrap();
        let t = sample("moved");
        store
            .insert_transcript_with_words(
                &t,
                &[
                    word(0, 0, "first", 0, 400),
                    word(1, 0, "second", 900, 1_800),
                ],
            )
            .unwrap();
        store.merge_segments(t.id, 0, 2, " ").unwrap();
        let alignment = store.word_alignment(t.id).unwrap();
        assert!(alignment.is_empty());
        assert!(alignment.stale.contains(&0));
    }

    #[test]
    fn deleting_a_transcript_cascades_to_its_word_rows() {
        let store = Store::open_in_memory().unwrap();
        let t = sample("cascade");
        store
            .insert_transcript_with_words(&t, &sample_words())
            .unwrap();
        store.delete_transcript(t.id).unwrap();
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM word_timings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "the foreign key must cascade");
    }

    #[test]
    fn replace_word_timings_is_all_or_nothing() {
        let store = Store::open_in_memory().unwrap();
        let t = sample("atomic");
        store
            .insert_transcript_with_words(&t, &sample_words())
            .unwrap();
        // A write whose second group is refused still lands the first one —
        // and never leaves the transcript with a half-updated row set.
        let stats = store
            .replace_word_timings(
                t.id,
                &[word(1, 0, "second", 900, 1_800), word(0, 0, "wrong", 0, 10)],
            )
            .unwrap();
        assert_eq!(stats.written, 1);
        assert_eq!(stats.skipped_segments, 1);
        let alignment = store.word_alignment(t.id).unwrap();
        assert_eq!(alignment.segment(1).unwrap().len(), 1);
        assert!(alignment.segment(0).is_none());
    }

    #[test]
    fn a_v4_database_upgrades_to_v5_additively() {
        let dir = std::env::temp_dir().join(format!("suramak-v5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("suramak.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE transcripts (
                     id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at TEXT NOT NULL,
                     updated_at TEXT NOT NULL, source_json TEXT NOT NULL, model TEXT NOT NULL,
                     language TEXT, audio_path TEXT);
                 CREATE TABLE segments (
                     transcript_id TEXT NOT NULL, idx INTEGER NOT NULL, start_ms INTEGER NOT NULL,
                     end_ms INTEGER NOT NULL, text TEXT NOT NULL, source_text TEXT,
                     derived TEXT NOT NULL DEFAULT '',
                     PRIMARY KEY (transcript_id, idx));
                 INSERT INTO meta(key, value) VALUES ('schema_version', '4');
                 INSERT INTO transcripts VALUES (
                     '00000000-0000-0000-0000-000000000001', 'old', '2024-01-01T00:00:00Z',
                     '2024-01-01T00:00:00Z', '{\"type\":\"file\",\"name\":\"old.wav\"}', 'tiny', 'en', NULL);
                 INSERT INTO segments VALUES (
                     '00000000-0000-0000-0000-000000000001', 0, 0, 1000, 'halo', 'halo', '');",
            )
            .unwrap();
        }
        let store = Store::open(&path).expect("upgrade");
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        let id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        assert!(store.segment_translations(id).unwrap().is_empty());
        let rows = store.segment_rows(id).unwrap();
        assert_eq!(rows[0].text, "halo");
        assert!(rows[0].translation.is_none());
        drop(store);
        let again = Store::open(&path).expect("reopen");
        assert_eq!(again.schema_version().unwrap(), SCHEMA_VERSION);
        drop(again);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
