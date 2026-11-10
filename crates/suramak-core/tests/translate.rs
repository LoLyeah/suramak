//! M15.8 translation client tests — the mock-endpoint proof.
//!
//! Every test here talks to a hand-rolled `TcpListener` on 127.0.0.1 (the same
//! pattern as `tests/polish.rs`); the real endpoint is opt-in and never
//! contacted. The suite pins the request envelope (target language + ids), the
//! EN→ID and ID→EN round trips, cue alignment, and the per-batch failure shape.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use suramak_core::settings::PolishSettings;
use suramak_core::transcript::Segment;
use suramak_core::translate::{
    run_translation, BatchSender, OpenAiCompatTranslate, TargetLanguage, MAX_BATCH_SEGMENTS,
};

fn segment(idx: u32, text: &str) -> Segment {
    Segment {
        idx,
        start_ms: i64::from(idx) * 1_000,
        end_ms: i64::from(idx) * 1_000 + 900,
        text: text.to_string(),
    }
}

fn cfg(base_url: &str) -> PolishSettings {
    PolishSettings {
        base_url: base_url.to_string(),
        model: "mock-model".to_string(),
        send_audio: false,
    }
}

/// Reads the full request (headers + Content-Length body).
fn read_request(stream: &mut TcpStream) -> String {
    let mut data: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    let mut expected: Option<(usize, usize)> = None;
    loop {
        if expected.is_none() {
            if let Some(pos) = find(&data, b"\r\n\r\n") {
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

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes()).ok();
}

/// The payload of a translation request as (target_language, ids, texts).
fn payload(request: &str) -> (String, Vec<u32>, Vec<String>) {
    let at = find(request.as_bytes(), b"\r\n\r\n").expect("headers") + 4;
    let body: serde_json::Value =
        serde_json::from_str(request[at..].trim_end()).expect("request JSON");
    assert_eq!(body["model"], "mock-model");
    let content = body["messages"][1]["content"]
        .as_str()
        .expect("user content");
    let payload: serde_json::Value = serde_json::from_str(content).expect("payload JSON");
    let segments = payload["segments"].as_array().expect("segments");
    (
        payload["target_language"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        segments
            .iter()
            .map(|segment| segment["id"].as_u64().unwrap() as u32)
            .collect(),
        segments
            .iter()
            .map(|segment| segment["text"].as_str().unwrap_or_default().to_string())
            .collect(),
    )
}

/// A chat-completions success body for the given (id, text) pairs.
fn reply(pairs: &[(u32, &str)]) -> String {
    let segments: Vec<serde_json::Value> = pairs
        .iter()
        .map(|(id, text)| serde_json::json!({ "id": id, "text": text }))
        .collect();
    let content = serde_json::json!({ "segments": segments }).to_string();
    serde_json::json!({
        "choices": [{ "message": { "role": "assistant", "content": content } }]
    })
    .to_string()
}

/// Spawns an endpoint that answers `requests` connections with `handler`
/// (request index, request text) → (status, body), counting what it saw.
fn spawn_mock<F>(
    requests: usize,
    handler: F,
) -> (String, Arc<AtomicUsize>, Arc<std::sync::Mutex<Vec<String>>>)
where
    F: Fn(usize, &str) -> (String, String) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap();
    let seen_count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let count = Arc::clone(&seen_count);
    let record = Arc::clone(&seen);
    thread::spawn(move || {
        for index in 0..requests {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let request = read_request(&mut stream);
            count.fetch_add(1, Ordering::SeqCst);
            record.lock().unwrap().push(request.clone());
            let (status, body) = handler(index, &request);
            respond(&mut stream, &status, &body);
        }
    });
    (format!("http://{addr}/v1"), seen_count, seen)
}

fn real_backend() -> OpenAiCompatTranslate {
    // The key is required by the real client; tests set it just for construction
    // and never let the request leave 127.0.0.1.
    std::env::set_var("SURAMAK_OPENAI_API_KEY", "test-key");
    OpenAiCompatTranslate::new().expect("client")
}

#[test]
fn en_to_id_over_a_mock_endpoint() {
    let (url, seen, requests) = spawn_mock(1, |_index, request| {
        let (target, ids, texts) = payload(request);
        assert_eq!(target, "id", "the target language rides the payload");
        assert_eq!(ids, vec![0, 1]);
        assert_eq!(texts, vec!["Hello world.", "This is a test."]);
        (
            "200 OK".to_string(),
            reply(&[(0, "Halo dunia."), (1, "Ini adalah ujian.")]),
        )
    });
    let transcript = vec![segment(0, "Hello world."), segment(1, "This is a test.")];
    let run = run_translation(
        &transcript,
        &cfg(&url),
        TargetLanguage::Indonesian,
        &real_backend(),
        || false,
        |_| {},
    );
    assert!(run.is_complete(), "{:?}", run.failed);
    assert_eq!(
        run.translated
            .iter()
            .map(|translation| translation.text.as_str())
            .collect::<Vec<_>>(),
        vec!["Halo dunia.", "Ini adalah ujian."]
    );
    assert_eq!(run.model, "mock-model");
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    assert_eq!(requests.lock().unwrap().len(), 1);
    let request = &requests.lock().unwrap()[0];
    assert!(
        request.starts_with("POST /v1/chat/completions"),
        "{request}"
    );
}

#[test]
fn id_to_en_over_a_mock_endpoint() {
    let (url, _seen, _requests) = spawn_mock(1, |_index, request| {
        let (target, ids, _) = payload(request);
        assert_eq!(target, "en");
        assert_eq!(ids, vec![0, 1]);
        (
            "200 OK".to_string(),
            reply(&[(0, "Hello world."), (1, "Thank you.")]),
        )
    });
    let transcript = vec![segment(0, "Halo dunia."), segment(1, "Terima kasih.")];
    let run = run_translation(
        &transcript,
        &cfg(&url),
        TargetLanguage::English,
        &real_backend(),
        || false,
        |_| {},
    );
    assert!(run.is_complete());
    assert_eq!(run.translated[0].text, "Hello world.");
    assert_eq!(run.translated[1].text, "Thank you.");
}

#[test]
fn batches_are_capped_and_each_request_carries_exactly_its_ids() {
    let (url, seen, requests) = spawn_mock(2, |_index, request| {
        let (_, ids, _) = payload(request);
        let pairs: Vec<(u32, &str)> = ids.iter().map(|id| (*id, "Halo.")).collect();
        ("200 OK".to_string(), reply(&pairs))
    });
    let transcript: Vec<Segment> = (0..25).map(|idx| segment(idx, "Hello world.")).collect();
    let run = run_translation(
        &transcript,
        &cfg(&url),
        TargetLanguage::Indonesian,
        &real_backend(),
        || false,
        |_| {},
    );
    assert!(run.is_complete());
    assert_eq!(run.translated.len(), 25);
    assert_eq!(run.batches_total, 2);
    assert_eq!(seen.load(Ordering::SeqCst), 2);
    let requests = requests.lock().unwrap();
    assert_eq!(payload(&requests[0]).1.len(), MAX_BATCH_SEGMENTS);
    assert_eq!(payload(&requests[1]).1, vec![20, 21, 22, 23, 24]);
}

#[test]
fn an_endpoint_error_names_every_segment_in_the_batch_and_stops_only_that_batch() {
    let (url, _seen, _requests) = spawn_mock(2, |index, request| {
        if index == 0 {
            (
                "500 Internal Server Error".to_string(),
                r#"{"error":"server fault"}"#.to_string(),
            )
        } else {
            let (_, ids, _) = payload(request);
            let pairs: Vec<(u32, &str)> = ids.iter().map(|id| (*id, "Halo.")).collect();
            ("200 OK".to_string(), reply(&pairs))
        }
    });
    let transcript: Vec<Segment> = (0..21).map(|idx| segment(idx, "Hello world.")).collect();
    let run = run_translation(
        &transcript,
        &cfg(&url),
        TargetLanguage::Indonesian,
        &real_backend(),
        || false,
        |_| {},
    );
    assert!(!run.cancelled);
    assert_eq!(run.translated.len(), 1);
    assert_eq!(run.translated[0].idx, 20);
    assert_eq!(run.failed.len(), MAX_BATCH_SEGMENTS);
    assert_eq!(run.failed_ids(), (0..20).collect::<Vec<u32>>());
    assert!(run.failed[0].message.contains("HTTP 500"));
    assert!(run.failed[0].label().starts_with("segment 0: HTTP 500"));

    // The retry sends exactly the failed ids — one bounded request.
    let (retry_url, retry_seen, retry_requests) = spawn_mock(1, |_index, request| {
        let (_, ids, _) = payload(request);
        let pairs: Vec<(u32, &str)> = ids.iter().map(|id| (*id, "Halo.")).collect();
        ("200 OK".to_string(), reply(&pairs))
    });
    let retry: Vec<Segment> = transcript
        .iter()
        .filter(|segment| run.failed_ids().contains(&segment.idx))
        .cloned()
        .collect();
    let retried = run_translation(
        &retry,
        &cfg(&retry_url),
        TargetLanguage::Indonesian,
        &real_backend(),
        || false,
        |_| {},
    );
    assert!(retried.is_complete());
    assert_eq!(retried.translated.len(), MAX_BATCH_SEGMENTS);
    assert_eq!(retry_seen.load(Ordering::SeqCst), 1);
    assert_eq!(
        payload(&retry_requests.lock().unwrap()[0]).1,
        (0..20).collect::<Vec<u32>>()
    );
}

#[test]
fn a_cancelled_run_sends_no_further_batch() {
    let (url, seen, _requests) = spawn_mock(1, |_index, request| {
        let (_, ids, _) = payload(request);
        let pairs: Vec<(u32, &str)> = ids.iter().map(|id| (*id, "Halo.")).collect();
        ("200 OK".to_string(), reply(&pairs))
    });
    let transcript: Vec<Segment> = (0..45).map(|idx| segment(idx, "Hello world.")).collect();
    let mut calls = 0;
    let run = run_translation(
        &transcript,
        &cfg(&url),
        TargetLanguage::Indonesian,
        &real_backend(),
        || {
            calls += 1;
            // pre-batch 1, post-batch 1, then pre-batch 2 → cancel.
            calls >= 3
        },
        |_| {},
    );
    assert!(run.cancelled);
    assert_eq!(run.batches_done, 1);
    assert_eq!(run.translated.len(), MAX_BATCH_SEGMENTS);
    assert_eq!(seen.load(Ordering::SeqCst), 1, "no second request");
}

#[test]
fn a_sender_that_cannot_reach_the_endpoint_fails_the_batch_not_the_process() {
    struct Dead;
    impl BatchSender for Dead {
        fn send(
            &self,
            _: &[Segment],
            _: &PolishSettings,
            _: TargetLanguage,
        ) -> Result<String, String> {
            Err("the endpoint could not be reached: connection refused".to_string())
        }
    }
    let run = run_translation(
        &[segment(0, "Hello world."), segment(1, "Goodbye.")],
        &cfg("http://127.0.0.1:9/v1"),
        TargetLanguage::Indonesian,
        &Dead,
        || false,
        |_| {},
    );
    assert_eq!(run.translated.len(), 0);
    assert_eq!(run.failed.len(), 2);
    assert!(run.failed[0].message.contains("connection refused"));
}
