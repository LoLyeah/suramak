//! M15.1 — persistence and provenance gates over the live store.
//!
//! The fixture is built **in the test** from the exact v1 DDL that app
//! 0.3.0/0.3.1 shipped (the `Store::init` schema before M15.1), through the same
//! `rusqlite` binding the store uses. Nothing binary is committed: the upgrade
//! under test is `Store::open` finding a real, populated v1 file and migrating it
//! in place.
//!
//! Covered here:
//! - v1 → v2 upgrade keeps every transcript, segment and byte of text;
//! - `source_text` is backfilled from `text` and survives editor/polish rewrites;
//! - backup (`VACUUM INTO`) → mutate → restore round-trips every row;
//! - corrupt backups are refused without touching the live library;
//! - the search contract: FTS5 tokens (M15.7), not the old LIKE substring scan.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use suramak_core::store::{Store, SCHEMA_VERSION};
use suramak_core::transcript::{Segment, Source, Transcript};

/// The shipped v1 schema (app 0.3.0/0.3.1), verbatim. Kept as SQL rather than a
/// committed `.db` so a reviewer can read exactly what "v0.3 database" means.
const V1_SCHEMA: &str = "
    PRAGMA journal_mode=WAL;
    CREATE TABLE meta (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE transcripts (
        id          TEXT PRIMARY KEY,
        title       TEXT NOT NULL,
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL,
        source_json TEXT NOT NULL,
        model       TEXT NOT NULL,
        language    TEXT,
        audio_path  TEXT
    );
    CREATE TABLE segments (
        transcript_id TEXT    NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
        idx           INTEGER NOT NULL,
        start_ms      INTEGER NOT NULL,
        end_ms        INTEGER NOT NULL,
        text          TEXT    NOT NULL,
        PRIMARY KEY (transcript_id, idx)
    );
    INSERT INTO meta(key, value) VALUES ('schema_version', '1');
";

/// A fixture transcript's fixed id + rows, written the way v0.3 wrote them.
struct V1Fixture {
    id: &'static str,
    title: &'static str,
    created_at: &'static str,
    source_json: &'static str,
    language: Option<&'static str>,
    segments: Vec<(i64, i64, &'static str)>,
}

fn v1_fixtures() -> Vec<V1Fixture> {
    vec![
        V1Fixture {
            id: "11111111-1111-4111-8111-111111111111",
            title: "voice-note-en.ogg",
            created_at: "2026-09-01T08:00:00+00:00",
            source_json: r#"{"type":"file","name":"voice-note-en.ogg"}"#,
            language: Some("en"),
            segments: vec![
                (0, 1_800, "The quick brown fox jumps over the lazy dog."),
                (1_800, 3_600, "Hello, world.\nSecond line — still here."),
                (
                    3_600,
                    5_200,
                    "Emoji survives: 🎙️ and café (e + combining acute).",
                ),
            ],
        },
        V1Fixture {
            id: "22222222-2222-4222-8222-222222222222",
            title: "catatan-suara-id.opus",
            created_at: "2026-09-02T09:30:00+00:00",
            source_json: r#"{"type":"recording","started_at":"2026-09-02T09:30:00+00:00","duration_ms":6200}"#,
            language: Some("id"),
            segments: vec![
                (0, 2_400, "Selamat pagi! Nama saya Suramak."),
                (
                    2_400,
                    6_200,
                    "Aplikasi transkripsi lokal yang mendukung bahasa Indonesia dengan baik.",
                ),
            ],
        },
    ]
}

/// Creates `path` as a populated v1 library (the shipped app-0.3.0 shape).
fn create_v1_library(path: &Path) {
    let conn = Connection::open(path).expect("create the v1 fixture");
    conn.execute_batch(V1_SCHEMA).expect("apply the v1 schema");
    let tx = conn.unchecked_transaction().expect("transaction");
    for fixture in v1_fixtures() {
        tx.execute(
            "INSERT INTO transcripts(id, title, created_at, updated_at, source_json, model, language, audio_path)
             VALUES (?1, ?2, ?3, ?3, ?4, 'tiny', ?5, NULL)",
            params![
                fixture.id,
                fixture.title,
                fixture.created_at,
                fixture.source_json,
                fixture.language,
            ],
        )
        .expect("insert header");
        for (idx, (start, end, text)) in fixture.segments.iter().enumerate() {
            tx.execute(
                "INSERT INTO segments(transcript_id, idx, start_ms, end_ms, text)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![fixture.id, idx as i64, start, end, text],
            )
            .expect("insert segment");
        }
    }
    tx.commit().expect("commit the v1 fixture");
    // Fold the fixture's WAL into the file so later opens see exactly one file.
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .expect("checkpoint the fixture");
}

fn columns(path: &Path) -> Vec<String> {
    let conn = Connection::open(path).expect("open raw");
    let mut stmt = conn
        .prepare("PRAGMA table_info(segments)")
        .expect("prepare");
    stmt.query_map([], |row| row.get::<_, String>(1))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("columns")
}

fn sample(title: &str, texts: &[&str]) -> Transcript {
    Transcript::new(
        title.to_string(),
        Source::File {
            name: title.to_string(),
        },
        "tiny".to_string(),
        Some("id".to_string()),
        None,
        texts
            .iter()
            .enumerate()
            .map(|(idx, text)| Segment {
                idx: idx as u32,
                start_ms: idx as i64 * 1_000,
                end_ms: idx as i64 * 1_000 + 1_000,
                text: (*text).to_string(),
            })
            .collect(),
    )
}

#[test]
fn opening_a_populated_v1_library_upgrades_it_in_place_without_losing_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("suramak.db");
    create_v1_library(&path);

    // Precondition: the fixture really is the shipped v1 shape.
    assert_eq!(
        columns(&path),
        vec!["transcript_id", "idx", "start_ms", "end_ms", "text"],
        "the test fixture must start without the v2 column"
    );

    let store = Store::open(&path).expect("v1 must open");
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    // v1 → v2 (source_text + backfill) → v3 (derived marker, M15.5 tables) →
    // v4 (the FTS5 search index) → v5 (the M15.8 bilingual columns) → v6 (the
    // M15.6 word-timing table) → v7 (M16I's diarization tables) → v8 (M22A's
    // v9 (M22C's segment index/waveforms) → v10 (M23A's manual
    // speaker edit stamp) → v11 (M24A's watched-files ledger) → v12 (M24B's
    // notes table) → v13 (M25C's session identity): the upgrade walks the whole
    // chain in one open.
    assert_eq!(
        SCHEMA_VERSION, 13,
        "a new migration must be recorded here and in the store's history — \
         this line is the one place that notices a forgotten bump"
    );
    assert_eq!(store.integrity_check().unwrap(), "ok");
    // v4 backfilled the search index for the rows the fixture carried.
    let stats = store.search_index_stats().expect("stats");
    assert_eq!(stats.segments, 5, "every v1 segment is searchable");
    assert_eq!(stats.titles, 2);

    // Every byte of source text survives the upgrade, in both languages.
    let transcripts = store.list_transcripts().expect("list");
    assert_eq!(transcripts.len(), 2);
    let en = transcripts
        .iter()
        .find(|t| t.title == "voice-note-en.ogg")
        .expect("EN fixture");
    let loaded = store.get_transcript(en.id).expect("load EN");
    assert_eq!(loaded.segments.len(), 3);
    assert_eq!(
        loaded.segments[0].text,
        "The quick brown fox jumps over the lazy dog."
    );
    assert_eq!(
        loaded.segments[1].text,
        "Hello, world.\nSecond line — still here."
    );
    assert_eq!(
        loaded.segments[2].text,
        "Emoji survives: 🎙️ and café (e + combining acute)."
    );
    assert_eq!(loaded.language.as_deref(), Some("en"));

    let id = transcripts
        .iter()
        .find(|t| t.title == "catatan-suara-id.opus")
        .expect("ID fixture");
    let loaded_id = store.get_transcript(id.id).expect("load ID");
    assert_eq!(loaded_id.segments.len(), 2);
    assert_eq!(
        loaded_id.segments[1].text,
        "Aplikasi transkripsi lokal yang mendukung bahasa Indonesia dengan baik."
    );

    // The backfill is the v1 text itself — the only inference copy that existed.
    for seg in &loaded.segments {
        assert_eq!(
            store
                .segment_source_text(loaded.id, seg.idx)
                .unwrap()
                .as_deref(),
            Some(seg.text.as_str()),
            "idx {} must be backfilled from its v1 text",
            seg.idx
        );
    }

    // The column now exists physically, and a second open is a no-op.
    drop(store);
    assert!(columns(&path).contains(&"source_text".to_string()));
    assert!(
        columns(&path).contains(&"derived".to_string()),
        "the v3 marker is physically present"
    );
    let again = Store::open(&path).expect("re-open");
    assert_eq!(again.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(
        again.get_transcript(loaded.id).unwrap().segments[2].text,
        "Emoji survives: 🎙️ and café (e + combining acute)."
    );
    // The v3 tables were created for an upgraded library too.
    assert!(again.glossary_terms().unwrap().is_empty());
    assert!(again.prompt_measurement("tiny").unwrap().is_none());
}

#[test]
fn editor_and_polish_rewrites_keep_the_first_stored_text() {
    let store = Store::open_in_memory().expect("store");
    let t = sample("keep-source.wav", &["asli satu", "asli dua"]);
    store.insert_transcript(&t).expect("insert");

    // The editor path: `text` changes, the provenance does not.
    store
        .update_segment_text(t.id, 0, "diedit oleh pengguna")
        .expect("edit");
    assert_eq!(
        store.get_transcript(t.id).unwrap().segments[0].text,
        "diedit oleh pengguna"
    );
    assert_eq!(
        store.segment_source_text(t.id, 0).unwrap().as_deref(),
        Some("asli satu")
    );

    // The polish path (`upsert_segments`): same rule for rewritten rows, while a
    // brand-new row gets its own first text as provenance.
    let mut polished = t.segments.clone();
    polished[0].text = "versi poles".to_string();
    polished[1].text = "versi poles dua".to_string();
    polished.push(Segment {
        idx: 2,
        start_ms: 2_000,
        end_ms: 3_000,
        text: "baris baru".to_string(),
    });
    store.upsert_segments(t.id, &polished).expect("polish");
    assert_eq!(
        store.segment_source_text(t.id, 0).unwrap().as_deref(),
        Some("asli satu")
    );
    assert_eq!(
        store.segment_source_text(t.id, 1).unwrap().as_deref(),
        Some("asli dua")
    );
    assert_eq!(
        store.segment_source_text(t.id, 2).unwrap().as_deref(),
        Some("baris baru"),
        "a row that never had inference text records its first text"
    );

    // A stale-tail delete removes the row's provenance with it.
    store.upsert_segments(t.id, &polished[..2]).expect("shrink");
    assert_eq!(store.segment_source_text(t.id, 2).unwrap(), None);
}

#[test]
fn backup_and_restore_round_trip_preserves_every_row_and_source_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("suramak.db");
    let backup = dir.path().join("backups").join("library.db");

    let first = sample(
        "round-trip-id.wav",
        &["Selamat pagi, dunia.", "Baris kedua — dengan émoji 🎧."],
    );
    let second = sample("round-trip-en.wav", &["Line one.", "Line two."]);
    {
        let store = Store::open(&db).expect("store");
        store.insert_transcript(&first).expect("insert first");
        store.insert_transcript(&second).expect("insert second");
        store.backup_to(&backup).expect("backup");
        assert!(backup.is_file());

        // The backup is a complete database on its own.
        let snapshot = Store::open(&backup).expect("open backup");
        assert_eq!(snapshot.integrity_check().unwrap(), "ok");
        assert_eq!(snapshot.list_transcripts().unwrap().len(), 2);
        assert_eq!(
            snapshot.get_transcript(first.id).unwrap().segments[0].text,
            "Selamat pagi, dunia."
        );

        // Damage the live library after the snapshot.
        store
            .update_segment_text(first.id, 0, "EDIT AFTER BACKUP")
            .expect("edit");
        store.delete_transcript(second.id).expect("delete");
    }

    Store::restore_from(&backup, &db).expect("restore");
    let restored = Store::open(&db).expect("reopen");
    assert_eq!(restored.integrity_check().unwrap(), "ok");
    assert_eq!(restored.schema_version().unwrap(), SCHEMA_VERSION);

    let ids: Vec<_> = restored.list_transcripts().unwrap();
    assert_eq!(ids.len(), 2, "the deleted transcript is back");
    let first_again = restored.get_transcript(first.id).expect("first");
    assert_eq!(
        first_again.segments[0].text, "Selamat pagi, dunia.",
        "the post-backup edit is rolled back"
    );
    assert_eq!(
        first_again.segments[1].text, "Baris kedua — dengan émoji 🎧.",
        "non-ASCII text is byte-identical after restore"
    );
    assert_eq!(
        restored
            .segment_source_text(first.id, 0)
            .unwrap()
            .as_deref(),
        Some("Selamat pagi, dunia.")
    );
    assert!(restored.get_transcript(second.id).is_ok());

    // Restoring again is fine (the operation is idempotent over the same
    // snapshot); an existing backup destination is never overwritten silently.
    drop(restored);
    Store::restore_from(&backup, &db).expect("second restore");
    let reopened = Store::open(&db).expect("reopen");
    assert_eq!(reopened.list_transcripts().unwrap().len(), 2);
    let err = reopened.backup_to(&backup).expect_err("must not overwrite");
    assert!(format!("{err}").contains("refusing to overwrite"), "{err}");
}

#[test]
fn a_corrupt_backup_is_refused_and_the_live_library_is_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("suramak.db");
    let junk = dir.path().join("corrupt.db");
    std::fs::write(&junk, b"this is not a sqlite database").unwrap();

    let t = sample("keep-me.wav", &["jangan hilang"]);
    let store = Store::open(&db).expect("store");
    store.insert_transcript(&t).expect("insert");

    let err = Store::restore_from(&junk, &db).expect_err("junk must be refused");
    assert!(
        matches!(&err, suramak_core::SuramakError::Backup(_)),
        "expected a named Backup error, got {err:?}"
    );

    // The live handle still reads its rows and the file is intact.
    assert_eq!(
        store.get_transcript(t.id).unwrap().segments[0].text,
        "jangan hilang"
    );
    assert_eq!(store.integrity_check().unwrap(), "ok");
}

#[test]
fn search_is_fts5_tokens_not_a_like_scan() {
    // M15.7 replaces the M15.1 LIKE scan with FTS5. This test pins the
    // *changed* contract deliberately (the M15.1 test said so):
    // - tokens, not substrings: `anggara` no longer finds `anggaran`;
    // - case-insensitive for ASCII and accented Latin (`unicode61`);
    // - no stemming: `rapat` does not find `rapatnya`;
    // - FTS5 operators are ordinary words (every query word is quoted), so a
    //   query can never be a syntax error or an accidental OR;
    // - both `text` and `source_text` are searchable.
    let store = Store::open_in_memory().expect("store");
    let t = sample(
        "Rapat Senin.wav",
        &[
            "Selamat pagi, ini catatan rapat.",
            "Kita bahas anggaran dan jadwal.",
        ],
    );
    store.insert_transcript(&t).expect("insert");

    assert_eq!(store.search("SELAMAT").unwrap().len(), 1);
    assert_eq!(store.search("rapat").unwrap().len(), 1, "title + segment");
    assert_eq!(store.search("rapatnya").unwrap().len(), 0, "no stemming");
    assert_eq!(
        store.search("anggara").unwrap().len(),
        0,
        "whole tokens, not substrings"
    );
    assert_eq!(store.search("anggaran").unwrap().len(), 1);

    // Accented Latin folds both ways now (the LIKE scan could not).
    let accented = sample("café.txt", &["Éclair notes"]);
    store.insert_transcript(&accented).unwrap();
    assert_eq!(store.search("éclair").unwrap().len(), 1);
    assert_eq!(store.search("ÉCLAIR").unwrap().len(), 1);
    assert_eq!(
        store.search("eclair").unwrap().len(),
        1,
        "diacritics folded"
    );
    assert_eq!(store.search("cafe").unwrap().len(), 1, "title café");

    // FTS5 syntax is not interpreted: `OR`, quotes and `-` are words or
    // punctuation, never operators, and no query is a syntax error.
    assert_eq!(store.search("\"Selamat\"").unwrap().len(), 1);
    assert!(store.search("Selamat OR jadwal").unwrap().is_empty());
    assert!(store.search("-").unwrap().is_empty());
    assert!(store.search("NEAR(selamat)").unwrap().is_empty());

    // Both columns are searched: an edit that removes a word from the current
    // text still finds the segment through the preserved inference text.
    store
        .update_segment_text(t.id, 1, "Kita bahas cuaca.")
        .expect("edit");
    assert_eq!(store.search("cuaca").unwrap().len(), 1);
    assert_eq!(store.search("anggaran").unwrap().len(), 1);
    assert_eq!(
        store.segment_source_text(t.id, 1).unwrap().as_deref(),
        Some("Kita bahas anggaran dan jadwal.")
    );
}

#[test]
fn a_library_from_a_newer_build_is_refused_not_guessed_at() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("future.db");
    {
        let store = Store::open(&path).expect("create");
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    }
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE meta SET value = '99' WHERE key = 'schema_version'",
        [],
    )
    .unwrap();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    drop(conn);

    let err = match Store::open(&path) {
        Ok(_) => panic!("a newer schema must be refused"),
        Err(err) => err,
    };
    assert!(
        matches!(&err, suramak_core::SuramakError::Schema(_)),
        "expected Schema, got {err:?}"
    );
    assert!(format!("{err}").contains("newer"), "{err}");
}

/// The restore temp file never survives a completed restore.
#[test]
fn restore_leaves_no_temporary_file_behind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("suramak.db");
    let backup = dir.path().join("backup.db");
    let t = sample("clean.wav", &["bersih"]);
    {
        let store = Store::open(&db).unwrap();
        store.insert_transcript(&t).unwrap();
        store.backup_to(&backup).unwrap();
    }
    Store::restore_from(&backup, &db).unwrap();
    let leftovers: Vec<PathBuf> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("restore-tmp"))
        })
        .collect();
    assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
}
