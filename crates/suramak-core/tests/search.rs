//! M15.7 — the search gate, end to end over the real store.
//!
//! Covers the workorder's verification list: insert/edit/delete synchronisation,
//! split/merge/restore/reopen consistency, migration backfill (bounded chunks
//! and resume), mixed-language + literal Indonesian queries, empty-segment
//! handling, the glossary/dictionary corpus, corrupt-index self-repair, backup
//! round-trip, and the ≥10k-segment responsiveness number recorded on this VPS.

use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::{params, Connection};
use suramak_core::search::{HitKind, SearchOptions};
use suramak_core::store::{Store, SCHEMA_VERSION};
use suramak_core::textrules::{Dictionary, Glossary, TextSettings};
use suramak_core::transcript::{Segment, Source, Transcript};
use uuid::Uuid;

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
                start_ms: idx as i64 * 1_500,
                end_ms: idx as i64 * 1_500 + 1_400,
                text: (*text).to_string(),
            })
            .collect(),
    )
}

fn temp_db(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(format!("{name}.db"));
    (dir, path)
}

#[test]
fn insert_edit_and_delete_reach_the_index() {
    let store = Store::open_in_memory().expect("store");
    let t = sample(
        "Rapat anggaran.wav",
        &[
            "Selamat pagi semua",
            "Kita bahas anggaran di Jakarta",
            "terima kasih",
        ],
    );
    store.insert_transcript(&t).expect("insert");

    let found = store
        .search_hits("jakarta", SearchOptions::default())
        .expect("search");
    assert_eq!(found.total, 1);
    let hit = &found.hits[0];
    assert_eq!(hit.kind, HitKind::Segment);
    assert_eq!(hit.transcript_id, Some(t.id));
    assert_eq!(hit.title.as_deref(), Some("Rapat anggaran.wav"));
    assert_eq!(hit.segment_idx, Some(1));
    assert_eq!(hit.start_ms, Some(1_500), "the hit carries its timestamp");
    assert_eq!(hit.end_ms, Some(2_900));
    assert!(hit.snippet.contains("Jakarta"), "{}", hit.snippet);
    assert!(!hit.from_source_text);

    // Edit: the current text is replaced in the index, the original column
    // keeps the old word — two documents, one row, no stale duplicate.
    store
        .update_segment_text(t.id, 1, "Kita bahas anggaran di Bandung")
        .expect("edit");
    assert_eq!(
        store
            .search_hits("jakarta", SearchOptions::default())
            .unwrap()
            .total,
        1,
        "the preserved original still matches"
    );
    let bandung = store
        .search_hits("bandung", SearchOptions::default())
        .unwrap();
    assert_eq!(bandung.total, 1);
    assert!(!bandung.hits[0].from_source_text);
    assert_eq!(
        store.search_index_stats().unwrap().segments,
        3,
        "an edit rewrites one document, it does not add one"
    );

    // Delete: the header, the rows and every index document go together.
    store.delete_transcript(t.id).expect("delete");
    assert_eq!(
        store
            .search_hits("bandung", SearchOptions::default())
            .unwrap()
            .total,
        0
    );
    assert_eq!(
        store
            .search_hits("rapat", SearchOptions::default())
            .unwrap()
            .total,
        0,
        "the title document is gone too"
    );
    assert_eq!(store.search_index_stats().unwrap().total(), 0);
}

#[test]
fn every_structural_edit_keeps_the_index_in_step_with_the_rows() {
    let store = Store::open_in_memory().expect("store");
    let t = sample(
        "struktur.wav",
        &["halo dunia", "selamat pagi semua", "terima kasih"],
    );
    store.insert_transcript(&t).expect("insert");

    let indexed_segments = |store: &Store, id: Uuid| -> i64 {
        let rows = store.segment_rows(id).unwrap();
        let non_empty = rows
            .iter()
            .filter(|row| !row.text.trim().is_empty())
            .count() as i64;
        assert_eq!(
            store.search_index_stats().unwrap().segments,
            non_empty,
            "every non-empty row has exactly one document"
        );
        non_empty
    };
    assert_eq!(indexed_segments(&store, t.id), 3);

    // Split: four rows, four documents, and no document for the vanished
    // parent index.
    let split = store
        .split_segment(
            t.id,
            1,
            7,
            Some(1_900),
            suramak_core::export::ZeroDuration::OneFrame,
        )
        .expect("split");
    assert_eq!(indexed_segments(&store, t.id), 4);
    let selamat = store
        .search_hits("selamat", SearchOptions::default())
        .unwrap();
    assert_eq!(
        selamat.total, 2,
        "both halves inherit the parent's inference text (M15.3 provenance)"
    );
    assert_eq!(
        selamat
            .hits
            .iter()
            .filter(|hit| !hit.from_source_text)
            .count(),
        1,
        "exactly one hit is the current text; the other is provenance"
    );
    let pagi = store.search_hits("pagi", SearchOptions::default()).unwrap();
    assert!(
        pagi.hits.iter().any(|hit| hit.start_ms == Some(1_900)),
        "the right half keeps its own start: {:?}",
        pagi.hits.iter().map(|h| h.start_ms).collect::<Vec<_>>()
    );
    assert!(pagi.hits.iter().all(|hit| hit.start_ms.is_some()));

    // Merge: three rows, three documents.
    store.merge_segments(t.id, 1, 2, " ").expect("merge");
    assert_eq!(indexed_segments(&store, t.id), 3);
    let merged = store
        .search_hits("pagi semua", SearchOptions::default())
        .unwrap();
    assert!(
        merged.hits.iter().any(|hit| !hit.from_source_text),
        "the merged row is findable through its current text"
    );

    // Undo: the exact pre-split row set, and the index follows.
    store
        .restore_segment_rows(t.id, &split.before)
        .expect("undo");
    assert_eq!(indexed_segments(&store, t.id), 3);
    let undone = store.search_hits("pagi", SearchOptions::default()).unwrap();
    assert!(undone.hits.iter().any(|hit| !hit.from_source_text));
    assert_eq!(store.segment_rows(t.id).unwrap(), split.before);

    // Reopen: the index is on disk and already agrees — no rebuild needed.
    drop(store);
}

#[test]
fn a_reopened_store_searches_the_same_rows_without_rebuilding() {
    let (_dir, path) = temp_db("reopen");
    let t = sample("reopen.wav", &["mantap sekali", "sampai jumpa"]);
    {
        let store = Store::open(&path).expect("store");
        store.insert_transcript(&t).expect("insert");
        store.checkpoint().unwrap();
    }
    let reopened = Store::open(&path).expect("reopen");
    assert_eq!(reopened.schema_version().unwrap(), SCHEMA_VERSION);
    let hits = reopened
        .search_hits("mantap", SearchOptions::default())
        .expect("search");
    assert_eq!(hits.total, 1);
    assert_eq!(hits.hits[0].start_ms, Some(0));
    assert_eq!(reopened.search_index_stats().unwrap().segments, 2);
}

#[test]
fn mixed_language_and_literal_indonesian_queries_find_the_right_segments() {
    let store = Store::open_in_memory().expect("store");
    let t = sample(
        "campur.wav",
        &[
            "Kita meeting about the budget besok pagi",
            "saya kayak gitu deh, santai aja",
            "Pegawairungu menandatangani surat penting itu",
            "The quarterly review is next week",
        ],
    );
    store.insert_transcript(&t).expect("insert");

    let one = |query: &str| -> Vec<suramak_core::search::SearchHit> {
        let results = store
            .search_hits(query, SearchOptions::default())
            .unwrap_or_else(|err| panic!("search {query:?}: {err}"));
        assert_eq!(results.total, 1, "query {query:?} -> {results:?}");
        results.hits
    };

    // English word in an Indonesian sentence.
    assert_eq!(one("meeting")[0].segment_idx, Some(0));
    // Indonesian word in the same sentence.
    assert_eq!(one("besok")[0].segment_idx, Some(0));
    // Both language directions in one query, one row.
    assert_eq!(one("meeting budget")[0].segment_idx, Some(0));
    // Literal Indonesian terms find exactly themselves.
    assert_eq!(one("saya")[0].segment_idx, Some(1));
    assert_eq!(one("kayak")[0].segment_idx, Some(1));
    assert_eq!(one("Pegawairungu")[0].segment_idx, Some(2));
    assert_eq!(one("pegawairungu")[0].segment_idx, Some(2), "case folded");
    assert_eq!(
        store
            .search_hits("pegawai rungu", SearchOptions::default())
            .unwrap()
            .total,
        0,
        "two partial words are not the long word"
    );
    // No stemming: `kayak` is not conflated with anything, `rapat` is not `rapatnya`.
    assert_eq!(
        store
            .search_hits("kayaknya", SearchOptions::default())
            .unwrap()
            .total,
        0
    );
    // Prefix is explicit.
    assert_eq!(one("pegawai*")[0].segment_idx, Some(2));
    assert_eq!(
        store
            .search_hits("pegawai", SearchOptions::default())
            .unwrap()
            .total,
        0
    );
    // A query whose words live in different rows finds nothing (AND, not OR).
    assert_eq!(
        store
            .search_hits("meeting jumpa", SearchOptions::default())
            .unwrap()
            .total,
        0
    );
}

#[test]
fn segments_with_no_text_are_not_results() {
    let store = Store::open_in_memory().expect("store");
    let t = sample("kosong.wav", &["halo dunia", "", "   ", "selamat pagi"]);
    store.insert_transcript(&t).expect("insert");
    assert_eq!(store.search_index_stats().unwrap().segments, 2);
    assert_eq!(
        store
            .search_hits("halo", SearchOptions::default())
            .unwrap()
            .total,
        1
    );
    assert!(
        store.search_index_consistent().unwrap(),
        "the empty rule must agree with the SQL count, or every open rebuilds"
    );

    // Erasing a segment's text removes its document; restoring it brings the
    // document (and the preserved original) back.
    store.update_segment_text(t.id, 0, "   ").expect("erase");
    assert_eq!(store.search_index_stats().unwrap().segments, 1);
    assert_eq!(
        store
            .search_hits("halo", SearchOptions::default())
            .unwrap()
            .total,
        0
    );
    assert!(store.search_index_consistent().unwrap());
    store
        .update_segment_text(t.id, 0, "halo lagi")
        .expect("restore");
    assert_eq!(store.search_index_stats().unwrap().segments, 2);
    assert_eq!(
        store
            .search_hits("lagi", SearchOptions::default())
            .unwrap()
            .total,
        1
    );

    // Newline/tab-only text is empty too — in Rust and in the SQL count.
    store.update_segment_text(t.id, 1, " \n\t ").expect("blank");
    assert_eq!(store.search_index_stats().unwrap().segments, 2);
    assert!(store.search_index_consistent().unwrap());
}

#[test]
fn the_glossary_and_dictionary_are_searchable_and_follow_their_edits() {
    let store = Store::open_in_memory().expect("store");
    let settings = TextSettings {
        glossary: Glossary::parse("BPJS Kesehatan\nKecamatan Cilandak"),
        dictionary: Dictionary::parse("Kemsos = Kementerian Sosial"),
        ..TextSettings::default()
    };
    store.save_text_settings(&settings).expect("save");
    let stats = store.search_index_stats().unwrap();
    assert_eq!(stats.glossary, 2);
    assert_eq!(stats.dictionary, 1);

    let glossary = store
        .search_hits("BPJS", SearchOptions::default())
        .expect("search");
    assert_eq!(glossary.total, 1);
    assert_eq!(glossary.hits[0].kind, HitKind::Glossary);
    assert_eq!(glossary.hits[0].transcript_id, None);
    assert!(glossary.hits[0].snippet.contains("BPJS"));

    // A dictionary entry is found by either side of the mapping.
    let by_from = store
        .search_hits("kemsos", SearchOptions::default())
        .expect("search");
    assert_eq!(by_from.hits[0].kind, HitKind::Dictionary);
    let by_to = store
        .search_hits("Sosial", SearchOptions::default())
        .expect("search");
    assert_eq!(by_to.hits[0].kind, HitKind::Dictionary);
    assert!(by_to.hits[0].from_source_text);

    // Replacing the settings replaces the corpus documents (no orphans).
    let replaced = TextSettings {
        glossary: Glossary::parse("Jakarta"),
        ..TextSettings::default()
    };
    store.save_text_settings(&replaced).expect("replace");
    assert_eq!(store.search_index_stats().unwrap().glossary, 1);
    assert_eq!(store.search_index_stats().unwrap().dictionary, 0);
    assert_eq!(
        store
            .search_hits("BPJS", SearchOptions::default())
            .unwrap()
            .total,
        0
    );
    assert_eq!(
        store
            .search_hits("Jakarta", SearchOptions::default())
            .unwrap()
            .total,
        1
    );
}

#[test]
fn snippets_use_the_configured_window_and_the_original_column() {
    let store = Store::open_in_memory().expect("store");
    let long = "Kita membahas anggaran rumah tangga untuk bulan depan dengan sangat rinci \
                karena setiap pos perlu disetujui bersama sebelum rapat berikutnya";
    let t = sample("anggaran.wav", &[long]);
    store.insert_transcript(&t).expect("insert");

    let narrow = store
        .search_hits(
            "rinci",
            SearchOptions {
                context_chars: 12,
                limit: 10,
            },
        )
        .expect("search");
    let wide = store
        .search_hits(
            "rinci",
            SearchOptions {
                context_chars: 200,
                limit: 10,
            },
        )
        .expect("search");
    assert!(narrow.hits[0].snippet.len() < wide.hits[0].snippet.len());
    assert!(
        narrow.hits[0].snippet.starts_with('…'),
        "{}",
        narrow.hits[0].snippet
    );
    assert!(
        narrow.hits[0].snippet.ends_with('…'),
        "{}",
        narrow.hits[0].snippet
    );
    assert!(wide.hits[0].snippet.contains("rapat berikutnya"));
    assert!(!wide.hits[0].snippet.ends_with('…'));

    // The highlight span really is the matched word.
    let (start, len) = narrow.hits[0].highlight.expect("highlighted");
    assert_eq!(&narrow.hits[0].snippet[start..start + len], "rinci");

    // A phrase that only survives in `source_text` is shown from there.
    store
        .update_segment_text(t.id, 0, "Kita membahas cuaca.")
        .expect("edit");
    let original = store
        .search_hits("rumah tangga", SearchOptions::default())
        .expect("search");
    assert_eq!(original.total, 1);
    assert!(original.hits[0].from_source_text);
    assert!(original.hits[0].snippet.contains("rumah tangga"));
}

#[test]
fn a_transcript_without_audio_still_has_timestamped_hits() {
    // Missing media must not remove rows from search — the UI's seek is the
    // no-op, not the result list.
    let store = Store::open_in_memory().expect("store");
    let mut t = sample("hilang.wav", &["rekaman penting"]);
    t.audio_path = Some(PathBuf::from("/nonexistent/suramak/missing.wav"));
    store.insert_transcript(&t).expect("insert");
    let hits = store
        .search_hits("penting", SearchOptions::default())
        .expect("search");
    assert_eq!(hits.total, 1);
    assert_eq!(hits.hits[0].start_ms, Some(0));
    assert!(hits.hits[0].title.is_some());
}

// ---- migration -----------------------------------------------------------

const V3_SCHEMA: &str = "
    CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE transcripts (
        id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL, source_json TEXT NOT NULL,
        model TEXT NOT NULL, language TEXT, audio_path TEXT);
    CREATE TABLE segments (
        transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
        idx INTEGER NOT NULL, start_ms INTEGER NOT NULL, end_ms INTEGER NOT NULL,
        text TEXT NOT NULL, source_text TEXT,
        derived TEXT NOT NULL DEFAULT '', PRIMARY KEY (transcript_id, idx));
    CREATE TABLE glossary_terms (position INTEGER PRIMARY KEY, term TEXT NOT NULL, created_at TEXT NOT NULL);
    CREATE TABLE dictionary_entries (position INTEGER PRIMARY KEY, from_text TEXT NOT NULL, to_text TEXT NOT NULL, created_at TEXT NOT NULL);
    CREATE TABLE text_prefs (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE prompt_budgets (model_id TEXT PRIMARY KEY, n_text_ctx INTEGER NOT NULL, cap_tokens INTEGER NOT NULL, measured_at TEXT NOT NULL);
    INSERT INTO meta(key, value) VALUES ('schema_version', '3');
";

/// Builds a populated v3 library (the M15-B shape) with `segments` rows.
fn create_v3_library(path: &Path, id: Uuid, segments: usize) {
    let conn = Connection::open(path).expect("create v3");
    conn.execute_batch(V3_SCHEMA).expect("apply v3 schema");
    let tx = conn.unchecked_transaction().expect("tx");
    tx.execute(
        "INSERT INTO transcripts(id, title, created_at, updated_at, source_json, model, language, audio_path)
         VALUES (?1, 'lama.wav', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00',
                 '{\"type\":\"file\",\"name\":\"lama.wav\"}', 'tiny', 'id', NULL)",
        params![id.to_string()],
    )
    .unwrap();
    for idx in 0..segments {
        let text = if idx % 2 == 0 {
            format!("segmen {idx} membahas anggaran dan jadwal")
        } else {
            format!("segment {idx} covers the quarterly budget")
        };
        tx.execute(
            "INSERT INTO segments(transcript_id, idx, start_ms, end_ms, text, source_text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![
                id.to_string(),
                idx as i64,
                idx as i64 * 1_000,
                idx as i64 * 1_000 + 900,
                text
            ],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
}

#[test]
fn a_v3_library_backfills_in_bounded_chunks_and_resumes_where_it_stopped() {
    let (_dir, path) = temp_db("v3-backfill");
    let id = Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap();
    create_v3_library(&path, id, 1_200);

    // Materialise the v4 objects and a trigger that aborts the *second* chunk,
    // simulating a crash mid-backfill. The first chunk must already be
    // committed — that is what "bounded" means.
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE search_fts USING fts5(
                 body, original, tokenize = 'unicode61 remove_diacritics 1');
             CREATE TABLE search_docs (
                 rowid INTEGER PRIMARY KEY, doc_key TEXT NOT NULL UNIQUE,
                 kind TEXT NOT NULL, transcript_id TEXT, segment_idx INTEGER);
             CREATE TRIGGER stop_second_chunk BEFORE INSERT ON search_docs
             WHEN NEW.doc_key = 'seg:33333333-3333-4333-8333-333333333333:0000000699'
             BEGIN SELECT RAISE(ABORT, 'test: stop the backfill'); END;",
        )
        .unwrap();
    }

    let err = match Store::open(&path) {
        Ok(_) => panic!("the trigger must abort the backfill"),
        Err(err) => err,
    };
    assert!(format!("{err}").contains("stop the backfill"), "{err}");

    {
        let conn = Connection::open(&path).unwrap();
        let committed: i64 = conn
            .query_row(
                "SELECT count(*) FROM search_docs WHERE kind = 'segment'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            committed, 500,
            "the first bounded chunk committed on its own"
        );
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, "3", "an interrupted upgrade never claims v4");
        let cursor: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'search_backfill_segments'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            cursor, "seg:33333333-3333-4333-8333-333333333333:0000000499",
            "the cursor advanced with the committed chunk"
        );
        conn.execute_batch("DROP TRIGGER stop_second_chunk;")
            .unwrap();
    }

    // Reopen: the upgrade resumes from the cursor, finishes, and stamps v4.
    let store = Store::open(&path).expect("resume");
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(store.search_index_stats().unwrap().segments, 1_200);
    assert_eq!(
        store
            .search_hits("anggaran", SearchOptions::default())
            .unwrap()
            .total,
        600
    );
    let total = store
        .search_hits("budget", SearchOptions::default())
        .unwrap();
    assert_eq!(total.total, 600);
    assert!(
        total.hits.iter().all(|hit| hit.start_ms.is_some()),
        "every hit carries its timestamp"
    );
    assert_eq!(
        store
            .search_hits(
                "anggaran",
                SearchOptions {
                    context_chars: 80,
                    limit: 50
                }
            )
            .unwrap()
            .hits
            .len(),
        50,
        "the hit list is bounded; total still reports the full count"
    );
}

#[test]
fn a_corrupt_or_partial_index_rebuilds_itself_on_the_next_open() {
    let (_dir, path) = temp_db("corrupt-index");
    let t = sample(
        "rusak.wav",
        &["selamat pagi", "kita mulai rapat", "sampai jumpa"],
    );
    {
        let store = Store::open(&path).expect("store");
        store.insert_transcript(&t).expect("insert");
        store.checkpoint().unwrap();
    }
    // Delete one FTS row behind the store's back: a partial index.
    {
        let conn = Connection::open(&path).unwrap();
        let rowid: i64 = conn
            .query_row("SELECT min(rowid) FROM search_fts", [], |r| r.get(0))
            .unwrap();
        conn.execute("DELETE FROM search_fts WHERE rowid = ?1", params![rowid])
            .unwrap();
    }
    let reopened = Store::open(&path).expect("self-repair");
    assert_eq!(
        reopened
            .search_hits("selamat", SearchOptions::default())
            .unwrap()
            .total,
        1,
        "the rebuilt index finds what the damaged one lost"
    );
    assert_eq!(reopened.search_index_stats().unwrap().segments, 3);

    // A dropped table is the stronger corruption: the next open recreates and
    // rebuilds it from the rows.
    drop(reopened);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE search_fts;").unwrap();
    }
    let rebuilt = Store::open(&path).expect("recreate");
    assert_eq!(
        rebuilt
            .search_hits("rapat", SearchOptions::default())
            .unwrap()
            .total,
        1
    );
    assert_eq!(rebuilt.search_index_stats().unwrap().total(), 4); // 3 segments + 1 title
}

#[test]
fn search_survives_backup_and_restore() {
    let (dir, path) = temp_db("backup-search");
    let backup = dir.path().join("backup.db");
    let t = sample("arsip.wav", &["kata kunci asli"]);
    {
        let store = Store::open(&path).expect("store");
        store.insert_transcript(&t).expect("insert");
        store.backup_to(&backup).expect("backup");
    }
    {
        let store = Store::open(&path).expect("reopen");
        store.delete_transcript(t.id).expect("delete");
        assert_eq!(
            store
                .search_hits("kunci", SearchOptions::default())
                .unwrap()
                .total,
            0
        );
    }
    drop(Store::open(&path).unwrap());
    Store::restore_from(&backup, &path).expect("restore");
    let restored = Store::open(&path).expect("open restored");
    assert_eq!(
        restored
            .search_hits("kunci", SearchOptions::default())
            .unwrap()
            .total,
        1,
        "the restored file carries a working index (or rebuilds it on open)"
    );
}

// ---- responsiveness ------------------------------------------------------

/// Phrases drawn from the product's actual EN+ID material.
const PHRASES: [&str; 8] = [
    "Kita membahas anggaran rapat hari ini dengan seluruh tim",
    "The meeting covered the quarterly budget and the hiring plan",
    "saya kayak gitu deh, santai aja sampai besok pagi",
    "Pegawairungu menandatangani surat penting itu kemarin",
    "Selamat pagi semua, terima kasih sudah hadir tepat waktu",
    "We reviewed the transcript, the subtitles and the export settings",
    "anggaran rumah tangga dan jadwal perjalanan dinas bulan depan",
    "the quick brown fox jumps over the lazy dog again and again",
];

fn big_library(segments: usize, per_transcript: usize) -> Vec<Transcript> {
    let mut all = Vec::new();
    let mut remaining = segments;
    let mut index = 0;
    while remaining > 0 {
        let count = remaining.min(per_transcript);
        let rows: Vec<Segment> = (0..count)
            .map(|idx| {
                let phrase = PHRASES[(index + idx) % PHRASES.len()];
                Segment {
                    idx: idx as u32,
                    start_ms: idx as i64 * 2_000,
                    end_ms: idx as i64 * 2_000 + 1_900,
                    text: format!("{phrase} (bagian {} dari rapat)", index + idx),
                }
            })
            .collect();
        all.push(Transcript::new(
            format!("rapat-{index:05}.wav"),
            Source::File {
                name: format!("rapat-{index:05}.wav"),
            },
            "tiny".to_string(),
            Some("id".to_string()),
            None,
            rows,
        ));
        remaining -= count;
        index += count;
    }
    all
}

fn percentile_ms(samples: &mut [f64]) -> (f64, f64) {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = samples[samples.len() / 2];
    let worst = *samples.last().unwrap();
    (median, worst)
}

#[test]
fn ten_thousand_segments_stay_responsive_for_a_bounded_query() {
    let (_dir, path) = temp_db("perf");
    let transcripts = big_library(10_000, 500);
    let insert_start = Instant::now();
    {
        let store = Store::open(&path).expect("store");
        for transcript in &transcripts {
            store.insert_transcript(transcript).expect("insert");
        }
        store.checkpoint().expect("checkpoint");
    }
    let insert = insert_start.elapsed();

    // Reopening runs the consistency gate (counts + FTS5 integrity check) over
    // the whole index — the startup cost this slice adds.
    let open_start = Instant::now();
    let store = Store::open(&path).expect("reopen");
    let open_ms = open_start.elapsed().as_secs_f64() * 1_000.0;

    let stats = store.search_index_stats().unwrap();
    assert_eq!(stats.segments, 10_000);
    assert_eq!(stats.titles, 20);

    let options = SearchOptions {
        context_chars: 80,
        limit: 50,
    };
    // What the corpus should answer, computed from the same generator rather
    // than guessed.
    let expected_anggaran = transcripts
        .iter()
        .flat_map(|t| &t.segments)
        .filter(|s| s.text.split_whitespace().any(|w| w == "anggaran"))
        .count();
    assert!(
        expected_anggaran > 1_000,
        "a meaningful fraction of the corpus"
    );

    // Warm the statement cache and the FTS index pages, then time five runs.
    let warm = store.search_hits("anggaran", options).expect("warm");
    assert_eq!(warm.total, expected_anggaran);
    assert_eq!(warm.hits.len(), 50);

    let mut samples = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let results = store.search_hits("anggaran", options).expect("search");
        let elapsed = start.elapsed();
        assert_eq!(results.total, expected_anggaran);
        assert_eq!(results.hits.len(), 50);
        samples.push(elapsed.as_secs_f64() * 1_000.0);
    }
    let (median_ms, worst_ms) = percentile_ms(&mut samples);

    // A mixed-language, two-term query over the same corpus.
    let mut mixed = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let results = store
            .search_hits("anggaran rapat", options)
            .expect("search");
        mixed.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(
            results.total, expected_anggaran,
            "every such row says rapat"
        );
    }
    let (mixed_ms, _) = percentile_ms(&mut mixed);

    // A word every row carries is the worst case for ranking.
    let mut common = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        let results = store.search_hits("rapat", options).expect("search");
        common.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(results.hits.len(), 50);
        assert!(results.total >= 10_000);
    }
    let (common_ms, _) = percentile_ms(&mut common);

    let db_bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    println!(
        "M15.7 perf: 10k segments, 20 transcripts | db {:.1} MB | insert {:.0} ms | \
         reopen+check {:.1} ms | 'anggaran' median {:.2} ms worst {:.2} ms | \
         'anggaran rapat' median {:.2} ms | 'rapat' (all) median {:.2} ms",
        db_bytes as f64 / 1_048_576.0,
        insert.as_secs_f64() * 1_000.0,
        open_ms,
        median_ms,
        worst_ms,
        mixed_ms,
        common_ms
    );

    // A deliberately generous ceiling: the gate is "responsive on this VPS",
    // and a debug-build test must not flake on a busy machine. The recorded
    // number lives in docs/reports/M15c-search.md.
    assert!(
        median_ms < 500.0,
        "a bounded 10k-segment query took {median_ms:.1} ms (worst {worst_ms:.1} ms)"
    );
    assert!(
        common_ms < 1_500.0,
        "the all-matching query took {common_ms:.1} ms"
    );
}
