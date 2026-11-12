//! M22C — the segment search gate, end to end over the real store.
//!
//! Covers the workorder's verification list at the store boundary: population
//! on write, the one-time rebuild for existing rows, phrase/AND/`speaker:`
//! queries, bracketed ±60-char snippets, limit/ordering, structural edits,
//! deletion, and the waveform cache the library rows draw.

use std::path::PathBuf;

use suramak_core::diarization::{DiarizationRun, Speaker, SpeakerTurn};
use suramak_core::store::{Store, WAVEFORM_BARS};
use suramak_core::transcript::{Segment, Source, Transcript};
use uuid::Uuid;

fn transcript(title: &str, texts: &[&str]) -> Transcript {
    Transcript::new(
        title.to_string(),
        Source::File {
            name: format!("{title}.wav"),
        },
        "base".to_string(),
        Some("id".to_string()),
        None,
        texts
            .iter()
            .enumerate()
            .map(|(idx, text)| Segment {
                idx: idx as u32,
                start_ms: idx as i64 * 4_000,
                end_ms: idx as i64 * 4_000 + 3_800,
                text: (*text).to_string(),
            })
            .collect(),
    )
}

fn db_path(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(format!("{name}.db"));
    (dir, path)
}

#[test]
fn the_index_is_populated_on_insert_and_follows_an_edit() {
    let store = Store::open_in_memory().expect("store");
    let t = transcript(
        "Morning notes",
        &[
            "Selamat pagi semua",
            "Kita bahas anggaran di Jakarta",
            "Terima kasih",
        ],
    );
    store.insert_transcript(&t).expect("insert");
    assert_eq!(store.segment_index_rows().unwrap(), 3);
    assert!(store.segment_index_consistent().unwrap());

    let hits = store.search_segments("jakarta", 10).expect("search");
    assert_eq!(hits.len(), 1);
    let hit = &hits[0];
    assert_eq!(hit.transcript_id, t.id);
    assert_eq!(hit.transcript_title, "Morning notes");
    assert_eq!(hit.segment_idx, 1);
    assert_eq!(hit.start_ms, 4_000);
    assert_eq!(hit.end_ms, 7_800);
    assert_eq!(hit.snippet, "Kita bahas anggaran di [Jakarta]");
    assert!(hit.rank.is_finite());

    store
        .update_segment_text(t.id, 1, "Kita bahas anggaran di Bandung")
        .expect("edit");
    assert!(store
        .search_segments("jakarta", 10)
        .expect("search")
        .is_empty());
    let hits = store.search_segments("bandung", 10).expect("search");
    assert_eq!(hits.len(), 1);
    assert!(hits[0].snippet.contains("[Bandung]"), "{}", hits[0].snippet);
}

#[test]
fn quoted_phrases_keep_order_and_bare_terms_are_anded() {
    let store = Store::open_in_memory().expect("store");
    let t = transcript(
        "Rapat",
        &[
            "kita membahas anggaran kuartal depan",
            "anggaran dan jadwal peluncuran",
            "kuartal anggaran sudah ditutup",
        ],
    );
    store.insert_transcript(&t).expect("insert");

    let phrase = store
        .search_segments("\"anggaran kuartal\"", 10)
        .expect("phrase");
    assert_eq!(phrase.len(), 1, "{phrase:?}");
    assert_eq!(phrase[0].segment_idx, 0);

    let and = store.search_segments("anggaran jadwal", 10).expect("and");
    assert_eq!(and.len(), 1, "{and:?}");
    assert_eq!(and[0].segment_idx, 1);

    // The reversed phrase is not the phrase the user asked for.
    let reversed = store
        .search_segments("\"kuartal anggaran\"", 10)
        .expect("phrase");
    assert_eq!(reversed.len(), 1);
    assert_eq!(reversed[0].segment_idx, 2);
}

#[test]
fn a_speaker_prefix_filters_and_follows_a_rename() {
    let store = Store::open_in_memory().expect("store");
    let t = transcript(
        "Interview",
        &[
            "Budi membuka rapat",
            "Sari membahas anggaran",
            "Budi menutup rapat",
        ],
    );
    store.insert_transcript(&t).expect("insert");

    let run = DiarizationRun::new(t.id, 0, "seg", "a", "emb", "b", 2, 0.5, 12_000);
    let speakers = [Speaker::new(0, "Budi", 0), Speaker::new(1, "Sari", 1)];
    let turns = [
        SpeakerTurn::new(0, 0, 3_800, 0),
        SpeakerTurn::new(1, 4_000, 7_800, 1),
        SpeakerTurn::new(2, 8_000, 11_800, 0),
    ];
    store
        .replace_diarization_run(t.id, &run, &speakers, &turns)
        .expect("diarization");

    let budi = store.search_segments("speaker:Budi", 10).expect("filter");
    assert_eq!(budi.len(), 2, "{budi:?}");
    assert_eq!(
        budi.iter().map(|hit| hit.segment_idx).collect::<Vec<_>>(),
        [0, 2]
    );
    assert_eq!(budi[0].speaker.as_deref(), Some("Budi"));

    let sari = store
        .search_segments("rapat speaker:sari", 10)
        .expect("filter");
    assert_eq!(sari.len(), 0, "Sari's row does not say rapat");

    let sari = store
        .search_segments("anggaran speaker:SARI", 10)
        .expect("filter");
    assert_eq!(sari.len(), 1);
    assert_eq!(sari[0].segment_idx, 1);

    // A rename re-points the index: the old name stops matching, the new one
    // starts, without a rebuild.
    store.rename_speaker(t.id, 0, "Andi").expect("rename");
    assert!(store
        .search_segments("speaker:Budi", 10)
        .expect("filter")
        .is_empty());
    let andi = store.search_segments("speaker:Andi", 10).expect("filter");
    assert_eq!(andi.len(), 2);
    assert_eq!(andi[0].speaker.as_deref(), Some("Andi"));
}

#[test]
fn snippets_are_bracketed_and_the_window_is_about_sixty_chars() {
    let store = Store::open_in_memory().expect("store");
    let text = format!("{} anggaran {}", "kata ".repeat(40), "lanjut ".repeat(40));
    let t = transcript("Long", &[&text]);
    store.insert_transcript(&t).expect("insert");

    let hits = store.search_segments("anggaran", 10).expect("search");
    assert_eq!(hits.len(), 1);
    let snippet = &hits[0].snippet;
    assert!(snippet.contains("[anggaran]"), "{snippet}");
    assert!(
        snippet.starts_with('…') && snippet.ends_with('…'),
        "{snippet}"
    );
    let inner = snippet
        .trim_start_matches('…')
        .trim_end_matches('…')
        .replace(['[', ']'], "");
    assert!(
        inner.chars().count() <= 60 * 2 + "anggaran".len(),
        "the window must stay near ±60 chars: {}",
        inner.chars().count()
    );
}

#[test]
fn limits_and_ranks_are_honest_and_garbage_is_empty() {
    let store = Store::open_in_memory().expect("store");
    let texts: Vec<String> = (0..30)
        .map(|index| format!("baris {index} membahas anggaran daerah"))
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    store
        .insert_transcript(&transcript("Banyak", &refs))
        .expect("insert");

    let hits = store.search_segments("anggaran", 5).expect("search");
    assert_eq!(hits.len(), 5, "the limit is honored");
    assert!(
        hits.windows(2).all(|pair| pair[0].rank <= pair[1].rank),
        "bm25 ordering is ascending (best first)"
    );

    for garbage in ["", "   ", "- -- …", "speaker:"] {
        assert!(
            store
                .search_segments(garbage, 10)
                .expect("no error")
                .is_empty(),
            "{garbage:?} is not a query"
        );
    }
}

#[test]
fn a_structural_edit_and_a_shrink_move_the_index() {
    let store = Store::open_in_memory().expect("store");
    let t = transcript("Edit", &["alpha satu", "beta dua", "gamma tiga"]);
    store.insert_transcript(&t).expect("insert");

    let shrunk = vec![
        Segment {
            idx: 0,
            start_ms: 0,
            end_ms: 3_800,
            text: "alpha satu".to_string(),
        },
        Segment {
            idx: 1,
            start_ms: 4_000,
            end_ms: 7_800,
            text: "beta dua".to_string(),
        },
    ];
    store.upsert_segments(t.id, &shrunk).expect("upsert");
    assert_eq!(store.segment_index_rows().unwrap(), 2);
    assert!(store
        .search_segments("gamma", 10)
        .expect("search")
        .is_empty());
    assert_eq!(store.search_segments("beta", 10).expect("search").len(), 1);
}

#[test]
fn deleting_a_transcript_removes_its_index_rows() {
    let store = Store::open_in_memory().expect("store");
    let a = transcript("A", &["jakarta satu"]);
    let b = transcript("B", &["jakarta dua"]);
    store.insert_transcript(&a).expect("insert a");
    store.insert_transcript(&b).expect("insert b");
    assert_eq!(store.search_segments("jakarta", 10).unwrap().len(), 2);

    store.delete_transcript(a.id).expect("delete");
    let hits = store.search_segments("jakarta", 10).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].transcript_id, b.id);
    assert_eq!(store.segment_index_rows().unwrap(), 1);
}

#[test]
fn an_empty_index_is_rebuilt_on_open_one_transcript_at_a_time() {
    let (dir, path) = db_path("segment-rebuild");
    let t = transcript("Rebuild", &["jakarta satu", "jakarta dua"]);
    {
        let store = Store::open(&path).expect("store");
        store.insert_transcript(&t).expect("insert");
        assert_eq!(store.segment_index_rows().unwrap(), 2);
    }

    // Simulate an interrupted/upgraded library: the rows are there, the index
    // is empty, and the version marker says the rebuild still has to happen.
    {
        let conn = rusqlite::Connection::open(&path).expect("conn");
        conn.execute("DELETE FROM segment_fts", []).unwrap();
        conn.execute("DELETE FROM segment_fts_map", []).unwrap();
        // A v9 library whose index was lost (an interrupted rebuild): leaving
        // the version at 9 exercises the open-time gate, not the migration.
    }

    let reopened = Store::open(&path).expect("reopen");
    assert!(reopened.segment_index_consistent().unwrap());
    assert_eq!(reopened.segment_index_rows().unwrap(), 2);
    assert_eq!(
        reopened
            .search_segments("jakarta", 10)
            .expect("search")
            .len(),
        2
    );
    let _ = dir;
}

#[test]
fn a_v8_library_upgrades_and_backfills_the_segment_index() {
    let (dir, path) = db_path("segment-v8");
    let t = transcript("Old library", &["jakarta lama", "bandung lama"]);
    {
        let store = Store::open(&path).expect("store");
        store.insert_transcript(&t).expect("insert");
    }
    {
        let conn = rusqlite::Connection::open(&path).expect("conn");
        conn.execute("DELETE FROM segment_fts", []).unwrap();
        conn.execute("DELETE FROM segment_fts_map", []).unwrap();
        conn.execute(
            "UPDATE meta SET value = '8' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    }
    let store = Store::open(&path).expect("upgrade");
    assert_eq!(store.schema_version().unwrap(), 11);
    assert_eq!(store.segment_index_rows().unwrap(), 2);
    assert_eq!(store.search_segments("jakarta", 10).unwrap().len(), 1);
    let _ = dir;
}

#[test]
fn the_waveform_cache_round_trips_and_cascades() {
    let store = Store::open_in_memory().expect("store");
    let t = transcript("Wave", &["satu"]);
    store.insert_transcript(&t).expect("insert");

    assert_eq!(store.waveform(t.id).expect("read"), None);
    let bars: Vec<f32> = (0..WAVEFORM_BARS)
        .map(|index| index as f32 / WAVEFORM_BARS as f32)
        .collect();
    store.set_waveform(t.id, &bars).expect("store");
    let round = store.waveform(t.id).expect("read").expect("stored");
    assert_eq!(round.len(), WAVEFORM_BARS);
    for (want, got) in bars.iter().zip(&round) {
        assert!((want - got).abs() < 1e-6);
    }
    assert_eq!(store.waveforms().expect("all").len(), 1);

    store.delete_transcript(t.id).expect("delete");
    assert_eq!(store.waveform(t.id).expect("read"), None);
}

#[test]
fn a_missing_transcript_is_not_invented() {
    let store = Store::open_in_memory().expect("store");
    assert!(store
        .search_segments("anything", 10)
        .expect("search")
        .is_empty());
    assert_eq!(store.waveform(Uuid::nil()).unwrap(), None);
}
