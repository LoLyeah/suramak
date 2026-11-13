//! M25C: a real three-part synthetic session — the session-absolute timeline,
//! whole-session notes through a mock endpoint, and the one-file export.
//!
//! Run with `--nocapture` for the report's verbatim transcript:
//! `cargo test -p suramak-core --test m25c_session_notes -- --nocapture`

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use suramak_core::export::to_session_markdown;
use suramak_core::notes::{generate_session_notes, NotePreset};
use suramak_core::session::{session_incomplete_refusal, session_inputs, session_timeline};
use suramak_core::settings::PolishSettings;
use suramak_core::store::{SessionIdentity, Store};
use suramak_core::transcript::{Segment, Source, Transcript};
use uuid::Uuid;

fn part(index: u32, title: &str, duration_ms: i64, rows: &[(i64, i64, &str)]) -> Transcript {
    let segments = rows
        .iter()
        .enumerate()
        .map(|(idx, (start, end, text))| Segment {
            idx: idx as u32,
            start_ms: *start,
            end_ms: *end,
            text: (*text).to_string(),
        })
        .collect();
    let mut transcript = Transcript::new(
        title.to_string(),
        Source::File {
            name: format!("Meeting — Part {index} of 3.wav"),
        },
        "tiny".to_string(),
        Some("en".to_string()),
        None,
        segments,
    );
    // The synthetic WAV's exact length: the part's audio, including its tail.
    transcript.segments.last_mut().unwrap().end_ms = duration_ms;
    transcript
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        if data.windows(4).any(|window| window == b"\r\n\r\n") {
            let text = String::from_utf8_lossy(&data).to_string();
            if let Some(split) = text.find("\r\n\r\n") {
                let length = text[..split]
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if data.len() >= split + 4 + length {
                    break;
                }
            }
        }
    }
    String::from_utf8_lossy(&data).to_string()
}

fn spawn_mock(reply: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _request = read_request(&mut stream);
        let body = serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": reply } }]
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).ok();
    });
    format!("http://{addr}/v1")
}

fn with_key<T>(f: impl FnOnce() -> T) -> T {
    let previous = std::env::var_os("SURAMAK_OPENAI_API_KEY");
    std::env::set_var("SURAMAK_OPENAI_API_KEY", "test-key");
    let out = f();
    match previous {
        Some(value) => std::env::set_var("SURAMAK_OPENAI_API_KEY", value),
        None => std::env::remove_var("SURAMAK_OPENAI_API_KEY"),
    }
    out
}

const REPLY: &str = "## Decisions\n\n\
- Ship on Friday [00:00].\n\
- The budget was approved [50:00].\n\
- Budi owns the migration [55:00].\n\
- The launch date is certain [1:00:01].\n\n\
## Follow-ups\n\n\
- Budi sends the plan [55:00].\n";

#[test]
fn a_three_part_session_renders_whole_meeting_notes_and_one_markdown() {
    let store = Store::open_in_memory().expect("store");
    let session = Uuid::new_v4();

    // 45:00 + 5:00 + 10:00 = 60:00, the exact cap arithmetic the splitter uses.
    let parts = [
        part(
            1,
            "Project Atlas — Part 1 of 3",
            2_700_000,
            &[
                (0, 8_000, "Good morning, everyone."),
                (1_200_000, 1_220_000, "We will ship on Friday."),
            ],
        ),
        part(
            2,
            "Project Atlas — Part 2 of 3",
            300_000,
            &[
                (0, 6_000, "Part two begins after the pause."),
                (120_000, 140_000, "The budget is approved."),
            ],
        ),
        part(
            3,
            "Project Atlas — Part 3 of 3",
            600_000,
            &[
                (0, 7_000, "Part three starts after the second pause."),
                (300_000, 320_000, "Budi owns the migration."),
            ],
        ),
    ];
    for (index, transcript) in parts.iter().enumerate() {
        store.insert_transcript(transcript).unwrap();
        store
            .set_transcript_session(
                transcript.id,
                Some(SessionIdentity::new(session, index as u32 + 1, 3)),
            )
            .unwrap();
    }

    let stored_parts = store.session_parts(session).unwrap();
    let identities = store.session_identities().unwrap();
    let mut inputs = session_inputs(&stored_parts);
    for input in &mut inputs {
        let identity = identities[&input.transcript_id];
        input.part_index = identity.part_index;
        input.part_count = identity.part_count;
    }
    let timeline = session_timeline(&inputs);
    println!("--- parts ---");
    for span in &timeline.parts {
        println!(
            "Part {}: offset {} ms, duration {} ms",
            span.part_index, span.offset_ms, span.duration_ms
        );
    }
    println!("total: {} ms", timeline.total_ms);
    println!("--- absolute segment starts ---");
    for segment in &timeline.segments {
        println!("[{}] idx {}", segment.start_ms, segment.idx);
    }
    assert_eq!(
        timeline
            .parts
            .iter()
            .map(|span| span.offset_ms)
            .collect::<Vec<_>>(),
        vec![0, 2_700_000, 3_000_000]
    );
    assert_eq!(timeline.total_ms, 3_600_000);
    assert_eq!(timeline.segments[4].start_ms, 3_000_000, "part 3 row 0");
    assert_eq!(timeline.segments[5].start_ms, 3_300_000, "part 3 row 1");

    let url = spawn_mock(REPLY);
    let outcome = with_key(|| {
        generate_session_notes(
            "Project Atlas",
            Some("en"),
            &inputs,
            &[],
            NotePreset::Minutes,
            &PolishSettings {
                base_url: url,
                model: "mock-model".to_string(),
                send_audio: false,
            },
        )
        .expect("the mock answers")
    });
    println!("--- mock reply ---\n{REPLY}");
    println!("--- stored session note ---\n{}", outcome.body_md);
    println!(
        "checked={} removed={} breakdown={:?} parts={}",
        outcome.citations_checked, outcome.citations_removed, outcome.breakdown, outcome.parts
    );
    assert_eq!(outcome.citations_checked, 5);
    // One real time names no segment start (removed), and the second `[55:00]`
    // is a duplicate of the first (the validator cites each row once).
    assert_eq!(outcome.citations_removed, 2, "{:?}", outcome.breakdown);
    assert_eq!(outcome.breakdown.removed, 1);
    assert_eq!(outcome.breakdown.duplicate, 1);
    assert!(outcome.body_md.contains("[50:00]"), "{}", outcome.body_md);
    assert!(outcome.body_md.contains("[55:00]"), "{}", outcome.body_md);
    assert!(
        !outcome.body_md.contains("[1:00:01]"),
        "{}",
        outcome.body_md
    );

    // The note is stored once, keyed by (session, preset), and reads back.
    store
        .save_note(&suramak_core::store::NoteRecord {
            transcript_id: stored_parts[0].id,
            preset: outcome.preset,
            body_md: outcome.body_md.clone(),
            citations_checked: outcome.citations_checked,
            citations_removed: outcome.citations_removed,
            model: outcome.model.clone(),
            created_at: suramak_core::Utc::now(),
            session_id: Some(session),
        })
        .unwrap();
    let saved = store.notes_for_session(session).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].session_id, Some(session));
    assert!(saved[0].body_md.contains("[50:00]"));

    let markdown = to_session_markdown(
        "Project Atlas",
        stored_parts[0].created_at,
        &stored_parts[0].source,
        &stored_parts[0].model,
        stored_parts[0].language.as_deref(),
        &timeline,
    );
    println!("--- one-file session export ---\n{markdown}");
    assert!(markdown.contains("## Part 1 — 00:00"));
    assert!(markdown.contains("## Part 2 — 45:00"));
    assert!(markdown.contains("## Part 3 — 50:00"));
    assert!(markdown
        .contains("[00:50:00.000 → 00:50:07.000] Part three starts after the second pause."));

    // A failed third part refuses the whole run by name, with the count.
    let incomplete = session_incomplete_refusal(2, 3).unwrap();
    assert!(incomplete.starts_with("session_incomplete:"));
    assert!(incomplete.contains("2 of 3 parts transcribed"));
    println!("--- incomplete refusal ---\n{incomplete}");
}
