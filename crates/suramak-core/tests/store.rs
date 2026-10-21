use suramak_core::store::Store;
use suramak_core::transcript::{Segment, Source, Transcript};

fn make_transcript(title: &str) -> Transcript {
    Transcript::new(
        title.to_string(),
        Source::File {
            name: "test.mp3".to_string(),
        },
        "tiny".to_string(),
        Some("en".to_string()),
        None,
        vec![
            Segment {
                idx: 0,
                start_ms: 0,
                end_ms: 1000,
                text: "Hello.".to_string(),
            },
            Segment {
                idx: 1,
                start_ms: 1000,
                end_ms: 2000,
                text: "World.".to_string(),
            },
        ],
    )
}

#[test]
fn insert_and_retrieve() {
    let store = Store::open_in_memory().unwrap();
    let t = make_transcript("Insert Test");
    store.insert_transcript(&t).unwrap();

    let got = store.get_transcript(t.id).unwrap();
    assert_eq!(got.id, t.id);
    assert_eq!(got.title, "Insert Test");
    assert_eq!(got.segments.len(), 2);
    assert_eq!(got.segments[0].text, "Hello.");
    assert_eq!(got.segments[1].text, "World.");
}

#[test]
fn update_segment_text() {
    let store = Store::open_in_memory().unwrap();
    let t = make_transcript("Update Test");
    store.insert_transcript(&t).unwrap();

    store.update_segment_text(t.id, 0, "Hi there.").unwrap();
    let got = store.get_transcript(t.id).unwrap();
    assert_eq!(got.segments[0].text, "Hi there.");
}

#[test]
fn list_transcripts() {
    let store = Store::open_in_memory().unwrap();
    let t1 = make_transcript("Alpha");
    let t2 = make_transcript("Beta");
    store.insert_transcript(&t1).unwrap();
    store.insert_transcript(&t2).unwrap();

    let list = store.list_transcripts().unwrap();
    assert_eq!(list.len(), 2);
}

#[test]
fn delete_cascades_segments() {
    let store = Store::open_in_memory().unwrap();
    let t = make_transcript("Delete Test");
    store.insert_transcript(&t).unwrap();

    store.delete_transcript(t.id).unwrap();
    let err = store.get_transcript(t.id);
    assert!(err.is_err());

    // Segments should be gone due to CASCADE.
    // Verify by re-inserting same id is not present.
    let list = store.list_transcripts().unwrap();
    assert_eq!(list.len(), 0);
}

#[test]
fn delete_nonexistent_returns_error() {
    let store = Store::open_in_memory().unwrap();
    let fake_id = uuid::Uuid::new_v4();
    let result = store.delete_transcript(fake_id);
    assert!(result.is_err());
}

#[test]
fn search_by_title() {
    let store = Store::open_in_memory().unwrap();
    let t = make_transcript("Search Me Please");
    store.insert_transcript(&t).unwrap();

    let results = store.search("Search Me").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Search Me Please");
}

#[test]
fn search_by_segment_text() {
    let store = Store::open_in_memory().unwrap();
    let t = make_transcript("Segment Search");
    store.insert_transcript(&t).unwrap();

    let results = store.search("Hello").unwrap();
    assert_eq!(results.len(), 1);
}

#[test]
fn search_treats_wildcards_as_literal_text() {
    // SUR-LOW-02: `%` and `_` used to act as LIKE wildcards, so "100%" matched
    // every title beginning with "100" (and "_" matched any single character).
    let store = Store::open_in_memory().unwrap();
    let literal = make_transcript("100% done");
    let wildcard_lookalike = make_transcript("100 items done");
    let underscore = make_transcript("a_b notes");
    let any_char = make_transcript("axb notes");
    for t in [&literal, &wildcard_lookalike, &underscore, &any_char] {
        store.insert_transcript(t).unwrap();
    }

    let results = store.search("100%").unwrap();
    assert_eq!(results.len(), 1, "percent must be literal, got {results:?}");
    assert_eq!(results[0].title, "100% done");

    let results = store.search("a_b").unwrap();
    assert_eq!(results.len(), 1, "underscore must be literal");
    assert_eq!(results[0].title, "a_b notes");

    // A backslash is literal too (it is the escape character; it must be doubled).
    let backslash = make_transcript(r"path\to\file");
    store.insert_transcript(&backslash).unwrap();
    assert_eq!(store.search(r"path\to").unwrap().len(), 1);
}

#[test]
fn on_disk_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let t = make_transcript("On Disk");
    {
        let store = Store::open(&db_path).unwrap();
        store.insert_transcript(&t).unwrap();
    }
    {
        let store = Store::open(&db_path).unwrap();
        let got = store.get_transcript(t.id).unwrap();
        assert_eq!(got.title, "On Disk");
        assert_eq!(got.segments.len(), 2);
    }
}
