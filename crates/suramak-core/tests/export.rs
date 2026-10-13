use chrono::DateTime;
use suramak_core::export::{to_json, to_markdown, to_srt, to_txt, to_vtt};
use suramak_core::transcript::{Segment, Source, Transcript};
use uuid::Uuid;

/// Returns a fixed 3-segment transcript for golden-string tests.
fn fixed_transcript() -> Transcript {
    let id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let created_at = DateTime::parse_from_rfc3339("2024-01-15T10:00:00Z")
        .unwrap()
        .to_utc();
    Transcript {
        id,
        title: "Test Transcript".to_string(),
        created_at,
        updated_at: created_at,
        source: Source::File {
            name: "intro.mp3".to_string(),
        },
        model: "turbo-q5-0".to_string(),
        language: Some("id".to_string()),
        audio_path: None,
        segments: vec![
            Segment {
                idx: 0,
                start_ms: 0,
                end_ms: 2400,
                text: "Hello world.".to_string(),
            },
            Segment {
                idx: 1,
                start_ms: 2400,
                end_ms: 5000,
                text: "This is a test.".to_string(),
            },
            Segment {
                idx: 2,
                start_ms: 5000,
                end_ms: 8500,
                text: "Goodbye.".to_string(),
            },
        ],
    }
}

#[test]
fn export_txt_golden() {
    let t = fixed_transcript();
    let out = to_txt(&t).unwrap();
    let expected = "\
[00:00:00] Hello world.\n\
[00:00:02] This is a test.\n\
[00:00:05] Goodbye.\n";
    assert_eq!(out, expected);
}

#[test]
fn export_srt_golden() {
    let t = fixed_transcript();
    let out = to_srt(&t).unwrap();
    let expected = "\
1\n\
00:00:00,000 --> 00:00:02,400\n\
Hello world.\n\
\n\
2\n\
00:00:02,400 --> 00:00:05,000\n\
This is a test.\n\
\n\
3\n\
00:00:05,000 --> 00:00:08,500\n\
Goodbye.\n\
\n";
    assert_eq!(out, expected);
}

#[test]
fn export_vtt_golden() {
    let t = fixed_transcript();
    let out = to_vtt(&t).unwrap();
    let expected = "\
WEBVTT\n\
\n\
00:00:00.000 --> 00:00:02.400\n\
Hello world.\n\
\n\
00:00:02.400 --> 00:00:05.000\n\
This is a test.\n\
\n\
00:00:05.000 --> 00:00:08.500\n\
Goodbye.\n\
\n";
    assert_eq!(out, expected);
}

#[test]
fn export_markdown_golden() {
    let t = fixed_transcript();
    let out = to_markdown(&t).unwrap();
    // Check structural elements; exact date formatting is stable.
    assert!(out.starts_with("# Test Transcript\n"), "title line");
    assert!(out.contains("## Transcript"), "transcript heading");
    assert!(
        out.contains("[00:00:00.000 → 00:00:02.400] Hello world."),
        "seg 0"
    );
    assert!(
        out.contains("[00:00:02.400 → 00:00:05.000] This is a test."),
        "seg 1"
    );
    assert!(
        out.contains("[00:00:05.000 → 00:00:08.500] Goodbye."),
        "seg 2"
    );
    assert!(out.contains("- Source: intro.mp3"), "source");
    assert!(out.contains("- Model: turbo-q5-0"), "model");
    assert!(out.contains("- Language: id"), "language");
}

#[test]
fn export_json_golden() {
    let t = fixed_transcript();
    let out = to_json(&t).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["app"], "suramak");
    assert_eq!(v["schema"], 1);
    assert_eq!(v["id"], "00000000-0000-0000-0000-000000000001");
    assert_eq!(v["title"], "Test Transcript");
    assert_eq!(v["model"], "turbo-q5-0");
    assert_eq!(v["language"], "id");
    let segs = v["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 3);
    assert_eq!(segs[0]["id"], 0);
    assert_eq!(segs[0]["start_ms"], 0);
    assert_eq!(segs[0]["end_ms"], 2400);
    assert_eq!(segs[0]["start"], "00:00:00.000");
    assert_eq!(segs[0]["end"], "00:00:02.400");
    assert_eq!(segs[0]["text"], "Hello world.");
    assert_eq!(segs[2]["text"], "Goodbye.");
}
