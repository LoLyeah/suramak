//! M24B notes tests against a hand-rolled OpenAI-compatible mock endpoint.
//!
//! One test per preset: the request carries the real segments (with their
//! canonical starts), the model's Markdown lands as the stored body, and a
//! fabricated timestamp in the reply is removed by the validator while a real
//! one survives.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::thread;

use suramak_core::notes::{generate_notes, NotePreset};
use suramak_core::settings::PolishSettings;
use suramak_core::transcript::Segment;

/// The environment is process-global; every test that sets the key holds this.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_key<T>(f: impl FnOnce() -> T) -> T {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("SURAMAK_OPENAI_API_KEY");
    std::env::set_var("SURAMAK_OPENAI_API_KEY", "test-key");
    let out = f();
    match previous {
        Some(value) => std::env::set_var("SURAMAK_OPENAI_API_KEY", value),
        None => std::env::remove_var("SURAMAK_OPENAI_API_KEY"),
    }
    out
}

fn roster() -> Vec<Segment> {
    vec![
        Segment {
            idx: 0,
            start_ms: 0,
            end_ms: 8_000,
            text: "Good morning, everyone.".to_string(),
        },
        Segment {
            idx: 1,
            start_ms: 12_000,
            end_ms: 20_000,
            text: "We decided to ship on Friday.".to_string(),
        },
        Segment {
            idx: 2,
            start_ms: 65_000,
            end_ms: 78_000,
            text: "Budi will own the migration.".to_string(),
        },
    ]
}

fn cfg_for(base_url: &str) -> PolishSettings {
    PolishSettings {
        base_url: base_url.to_string(),
        model: "mock-model".to_string(),
        send_audio: false,
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    let mut expected: Option<(usize, usize)> = None;
    loop {
        if expected.is_none() {
            if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&data[..pos]).to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                expected = Some((pos + 4, length));
            }
        }
        if let Some((body_at, length)) = expected {
            if data.len() >= body_at + length {
                break;
            }
        }
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&data).to_string()
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes()).ok();
}

/// A chat-completions body carrying `content` as the assistant message.
fn chat_body(content: &str) -> String {
    serde_json::json!({
        "choices": [{ "message": { "role": "assistant", "content": content } }]
    })
    .to_string()
}

/// Serves `expected` requests with `handler`; returns the base URL and the
/// shared request log (filled as requests arrive).
fn spawn_mock<F>(
    expected: usize,
    handler: F,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>)
where
    F: Fn(usize) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let addr = listener.local_addr().unwrap();
    eprintln!("mock listening on {addr}");
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let record = std::sync::Arc::clone(&seen);
    thread::spawn(move || {
        for index in 0..expected {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let request = read_request(&mut stream);
            let content = handler(index);
            record.lock().unwrap().push(request);
            respond(&mut stream, "200 OK", &chat_body(&content));
        }
    });
    (format!("http://{addr}/v1"), seen)
}

/// The user message's JSON payload.
fn payload(request: &str) -> serde_json::Value {
    let at = request.find("\r\n\r\n").expect("headers") + 4;
    let body: serde_json::Value = serde_json::from_str(request[at..].trim_end()).expect("JSON");
    let content = body["messages"][1]["content"]
        .as_str()
        .expect("a string user content");
    serde_json::from_str(content).expect("the payload JSON")
}

/// A reply with one real citation and one fabricated one.
fn reply() -> String {
    "## Decisions\n\n- Ship on Friday [00:12].\n- The budget was approved [13:00].\n\n\
     ## Follow-ups\n\n- Budi owns the migration [01:05].\n"
        .to_string()
}

#[test]
fn minutes_request_carries_the_segments_and_strips_a_fabricated_time() {
    let (url, seen) = spawn_mock(1, |_| reply());
    with_key(|| {
        let outcome = generate_notes(
            "Weekly sync",
            Some("en"),
            &roster(),
            &[(2, "Budi".to_string())],
            NotePreset::Minutes,
            &cfg_for(&url),
        )
        .expect("the mock answers");

        assert_eq!(outcome.parts, 1);
        assert_eq!(outcome.model, "mock-model");
        assert!(outcome.body_md.starts_with("# Minutes\n\n"));
        assert!(outcome.body_md.contains("- Ship on Friday [00:12]."));
        assert!(outcome.body_md.contains("[01:05]"));
        assert!(
            !outcome.body_md.contains("[13:00]"),
            "a fabricated citation must be stripped: {}",
            outcome.body_md
        );
        assert_eq!(outcome.citations_checked, 3);
        assert_eq!(outcome.citations_removed, 1);
        assert_eq!(outcome.breakdown.past_end, 1);

        // The report's verbatim source: the mock endpoint's reply and the
        // stored note after the validator (run with `--nocapture` to capture).
        println!("--- before (mock reply) ---\n{}", reply());
        println!("--- after (stored body_md) ---\n{}", outcome.body_md);
        println!(
            "--- counts ---\nchecked={} removed={} breakdown={:?} parts={}",
            outcome.citations_checked, outcome.citations_removed, outcome.breakdown, outcome.parts
        );

        let seen = seen.lock().unwrap();
        let request = &seen[0];
        assert!(request.contains("POST /v1/chat/completions"));
        let body: serde_json::Value =
            serde_json::from_str(request[request.find("\r\n\r\n").unwrap() + 4..].trim_end())
                .unwrap();
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("meeting minutes"), "the Minutes prompt ran");
        assert!(system.contains("Never invent a timestamp"));
        let payload = payload(request);
        assert_eq!(payload["title"], "Weekly sync");
        assert_eq!(payload["segments"][1]["start"], "00:12");
        assert_eq!(payload["segments"][1]["start_ms"], 12_000);
        assert_eq!(payload["segments"][2]["speaker"], "Budi");
        assert_eq!(payload["segments"][0]["text"], "Good morning, everyone.");
    });
}

#[test]
fn action_items_request_carries_the_segments_and_strips_a_fabricated_time() {
    let (url, seen) = spawn_mock(1, |_| {
        "## Actions\n\n- **Budi** — own the migration [01:05]\n\
         - **Unassigned** — book the room [02:30]\n"
            .to_string()
    });
    with_key(|| {
        let outcome = generate_notes(
            "Weekly sync",
            Some("en"),
            &roster(),
            &[],
            NotePreset::ActionItems,
            &cfg_for(&url),
        )
        .expect("the mock answers");
        assert!(outcome.body_md.starts_with("# Action items\n\n"));
        assert!(outcome.body_md.contains("[01:05]"));
        assert!(!outcome.body_md.contains("[02:30]"));
        assert_eq!(outcome.citations_checked, 2);
        assert_eq!(outcome.citations_removed, 1);

        let seen = seen.lock().unwrap();
        let system = payload(&seen[0]).to_string();
        assert!(system.contains("Weekly sync"));
        let body: serde_json::Value =
            serde_json::from_str(seen[0][seen[0].find("\r\n\r\n").unwrap() + 4..].trim_end())
                .unwrap();
        let prompt = body["messages"][0]["content"].as_str().unwrap();
        assert!(
            prompt.contains("action item"),
            "the Action items prompt ran"
        );
        assert!(prompt.contains("Unassigned"));
        assert_eq!(payload(&seen[0])["segments"].as_array().unwrap().len(), 3);
    });
}

#[test]
fn bilingual_request_carries_the_segments_and_strips_a_fabricated_time() {
    let (url, seen) = spawn_mock(1, |_| {
        "## Ringkasan\n\n- Kirim hari Jumat [00:12]\n- Anggaran disetujui [14:00]\n\n\
         ## Summary (English)\n\nOne decision and one owner were recorded [01:05].\n"
            .to_string()
    });
    with_key(|| {
        let outcome = generate_notes(
            "Rapat mingguan",
            Some("id"),
            &roster(),
            &[],
            NotePreset::BilingualBrief,
            &cfg_for(&url),
        )
        .expect("the mock answers");
        assert!(outcome.body_md.starts_with("# Brief (EN↔ID)\n\n"));
        assert!(outcome.body_md.contains("[00:12]"));
        assert!(!outcome.body_md.contains("[14:00]"));
        assert_eq!(outcome.citations_checked, 3);
        assert_eq!(outcome.citations_removed, 1);
        assert_eq!(outcome.breakdown.past_end, 1);

        let seen = seen.lock().unwrap();
        let body: serde_json::Value =
            serde_json::from_str(seen[0][seen[0].find("\r\n\r\n").unwrap() + 4..].trim_end())
                .unwrap();
        let prompt = body["messages"][0]["content"].as_str().unwrap();
        assert!(prompt.contains("Bahasa Indonesia"), "the brief prompt ran");
        assert_eq!(payload(&seen[0])["language"], "id");
        assert_eq!(payload(&seen[0])["segments"][2]["start"], "01:05");
    });
}

#[test]
fn a_transcript_past_the_chunk_budget_runs_one_request_per_part_and_merges() {
    let tall: Vec<Segment> = (0..5)
        .map(|idx| Segment {
            idx,
            start_ms: i64::from(idx) * 60_000,
            end_ms: i64::from(idx) * 60_000 + 50_000,
            text: format!("{idx} {}", "word ".repeat(1_400)),
        })
        .collect();
    let (url, seen) = spawn_mock(2, |index| {
        let clock = if index == 0 { "00:00" } else { "01:00" };
        format!("## Part {index}\n\n- point [{clock}] and a made-up one [09:9{index}]\n")
    });
    with_key(|| {
        let outcome = generate_notes(
            "Long meeting",
            Some("en"),
            &tall,
            &[],
            NotePreset::Minutes,
            &cfg_for(&url),
        )
        .expect("both parts answer");
        assert_eq!(outcome.parts, 2, "the tall transcript chunks");
        assert!(outcome.is_chunked());
        assert_eq!(NotePreset::path_label(outcome.parts), "2 chunks merged");
        // Part 2's `[09:92]` is malformed (seconds 92) and part 1's `[09:90]`
        // likewise: both are removed, and each part contributes a real `[00:0x]`.
        assert_eq!(outcome.citations_checked, 4);
        assert_eq!(outcome.citations_removed, 2);
        assert_eq!(outcome.breakdown.malformed, 2);
        assert!(outcome.body_md.contains("## Part 0"));
        assert!(outcome.body_md.contains("## Part 1"));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        let first = payload(&seen[0]);
        let second = payload(&seen[1]);
        assert_eq!(first["part"], 1);
        assert_eq!(first["parts"], 2);
        assert_eq!(second["part"], 2);
        assert_eq!(second["parts"], 2);
        // Every segment goes to exactly one part, in order.
        let mut ids: Vec<u64> = first["segments"]
            .as_array()
            .unwrap()
            .iter()
            .chain(second["segments"].as_array().unwrap())
            .map(|segment| segment["id"].as_u64().unwrap())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, (0..5).collect::<Vec<u64>>());
    });
}

#[test]
fn an_endpoint_error_names_the_status_and_the_endpoint_message() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = read_request(&mut stream);
        respond(
            &mut stream,
            "503 Service Unavailable",
            r#"{"error":"model overloaded"}"#,
        );
    });
    with_key(|| {
        let error = generate_notes(
            "Weekly sync",
            Some("en"),
            &roster(),
            &[],
            NotePreset::Minutes,
            &cfg_for(&format!("http://{addr}/v1")),
        )
        .expect_err("HTTP 503 must fail");
        let message = error.to_string();
        assert!(message.contains("HTTP 503"), "unexpected: {message}");
        assert!(
            message.contains("model overloaded"),
            "the endpoint's message rides along: {message}"
        );
    });
}
