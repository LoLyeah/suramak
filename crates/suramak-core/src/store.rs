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

    /// Inserts a new transcript (header + segments).
    pub fn insert_transcript(&self, t: &Transcript) -> Result<(), SuramakError> {
        let source_json = serde_json::to_string(&t.source)?;
        let audio_path = t
            .audio_path
            .as_ref()
            .and_then(|p| p.to_str())
            .map(|s| s.to_string());

        self.conn.execute(
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

        self.upsert_segments(t.id, &t.segments)?;
        Ok(())
    }

    /// Upserts all segments for a transcript.
    pub fn upsert_segments(&self, id: Uuid, segments: &[Segment]) -> Result<(), SuramakError> {
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
        Ok(())
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
    pub fn search(&self, query: &str) -> Result<Vec<Transcript>, SuramakError> {
        let pattern = format!("%{}%", query);
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT t.id, t.title, t.created_at, t.updated_at, t.source_json,
                    t.model, t.language, t.audio_path
             FROM transcripts t
             LEFT JOIN segments s ON s.transcript_id = t.id
             WHERE t.title LIKE ?1 OR s.text LIKE ?1
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
