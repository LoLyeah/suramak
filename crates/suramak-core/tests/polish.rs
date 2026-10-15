//! Polish client tests — M4 (text-only) + the M11 audio-aware suite.
//!
//! No test ever hits a real endpoint: everything network-shaped runs against a
//! hand-rolled `TcpListener` mock (the pattern this file has always used). The
//! M11 suite pins the exact request shape, the chunker's coverage/order/segment
//! alignment/caps, and the graceful ladder (missing recording → text-only +
//! note; endpoint 4xx → one text-only retry + note).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use base64::Engine as _;
use suramak_core::polish::{
    build_body, chunk_wav, parse_and_validate, sample_of_ms, AudioChunk, AudioRequest,
    OpenAiCompat, Pcm16Wav, PolishBackend, AUDIO_CHUNK_CAP_BYTES, SYSTEM_PROMPT,
};
use suramak_core::settings::PolishSettings;
use suramak_core::transcript::Segment;

fn make_segments() -> Vec<Segment> {
    vec![
        Segment {
            idx: 0,
            start_ms: 0,
            end_ms: 1000,
            text: "helo wrold".to_string(),
        },
        Segment {
            idx: 1,
            start_ms: 1000,
            end_ms: 2000,
            text: "this iz a tset".to_string(),
        },
    ]
}

/// A segment with explicit times (M11 tests).
fn segment(idx: u32, start_ms: i64, end_ms: i64) -> Segment {
    Segment {
        idx,
        start_ms,
        end_ms,
        text: format!("segment {idx}"),
    }
}

fn cfg_for(base_url: &str) -> PolishSettings {
    PolishSettings {
        base_url: base_url.to_string(),
        model: "mock-model".to_string(),
        send_audio: true,
    }
}

/// Reads HTTP request headers, returns (method+path, Range header).
fn read_headers(stream: &mut TcpStream) -> String {
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf).unwrap_or(0);
    String::from_utf8_lossy(&buf[..n]).to_string()
}

/// Sends an HTTP/1.1 response.
fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes()).ok();
}

/// Canned success response body.
fn success_body() -> String {
    serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": r#"{"segments":[{"id":0,"text":"Hello world."},{"id":1,"text":"This is a test."}]}"#
            }
        }]
    })
    .to_string()
}

/// Canned success response with ```json fence.
fn fenced_body() -> String {
    let content = "```json\n{\"segments\":[{\"id\":0,\"text\":\"Hello world.\"},{\"id\":1,\"text\":\"This is a test.\"}]}\n```";
    serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": content
            }
        }]
    })
    .to_string()
}

/// Spawns a mock server that serves exactly `n` connections using `handler`.
fn spawn_server<F>(handler: F) -> std::net::SocketAddr
where
    F: Fn(&mut TcpStream, &str) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let req = read_headers(&mut stream);
            handler(&mut stream, &req);
        }
    });
    addr
}

/// Makes a minimal HTTP POST to the mock server and returns the response body.
fn http_post(addr: std::net::SocketAddr, path: &str, json_body: &str) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    let req = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Authorization: Bearer ***\n\
         Connection: close\r\n\r\n{json_body}",
        json_body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    resp
}

// ---------- parse_and_validate unit tests (no network) ----------

#[test]
fn parse_success_plain_json() {
    let segments = make_segments();
    let content =
        r#"{"segments":[{"id":0,"text":"Hello world."},{"id":1,"text":"This is a test."}]}"#;
    let result = parse_and_validate(content, &segments).unwrap();
    assert_eq!(result[0].text, "Hello world.");
    assert_eq!(result[1].text, "This is a test.");
    assert_eq!(result[0].start_ms, 0);
    assert_eq!(result[1].end_ms, 2000);
}

#[test]
fn parse_fenced_json() {
    let segments = make_segments();
    let content = "```json\n{\"segments\":[{\"id\":0,\"text\":\"Hello world.\"},{\"id\":1,\"text\":\"This is a test.\"}]}\n```";
    let result = parse_and_validate(content, &segments).unwrap();
    assert_eq!(result[0].text, "Hello world.");
}

#[test]
fn parse_id_mismatch_returns_error() {
    let segments = make_segments();
    // Wrong ids (swapped)
    let content =
        r#"{"segments":[{"id":1,"text":"Hello world."},{"id":0,"text":"This is a test."}]}"#;
    let err = parse_and_validate(content, &segments).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("id mismatch"),
        "expected id mismatch, got: {msg}"
    );
}

#[test]
fn parse_empty_text_returns_error() {
    let segments = make_segments();
    let content = r#"{"segments":[{"id":0,"text":""},{"id":1,"text":"This is a test."}]}"#;
    let err = parse_and_validate(content, &segments).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("empty text"),
        "expected empty text error, got: {msg}"
    );
}

#[test]
fn parse_count_mismatch_returns_error() {
    let segments = make_segments();
    let content = r#"{"segments":[{"id":0,"text":"Only one."}]}"#;
    let err = parse_and_validate(content, &segments).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("count mismatch"),
        "expected count mismatch, got: {msg}"
    );
}

// ---------- mock server tests for OpenAiCompat ----------

#[test]
fn mock_server_success() {
    let addr = spawn_server(|stream, _req| {
        respond(stream, "200 OK", &success_body());
    });

    let _resp = http_post(
        addr,
        "/v1/chat/completions",
        r#"{"model":"gpt-4","messages":[]}"#,
    );
    // Just verify the server responds successfully; actual client integration
    // is tested via parse_and_validate above.
}

#[test]
fn mock_server_500_propagates() {
    let addr = spawn_server(|stream, _req| {
        respond(
            stream,
            "500 Internal Server Error",
            r#"{"error":"server fault"}"#,
        );
    });

    // Direct HTTP call to verify the mock returns 500.
    let resp = http_post(addr, "/v1/chat/completions", "{}");
    assert!(
        resp.contains("500"),
        "expected 500 in response, got: {resp}"
    );
}

#[test]
fn mock_server_fenced_response() {
    let addr = spawn_server(|stream, _req| {
        respond(stream, "200 OK", &fenced_body());
    });

    let _resp = http_post(
        addr,
        "/v1/chat/completions",
        r#"{"model":"gpt-4","messages":[]}"#,
    );
    // parse_and_validate handles fences; tested above.
}

// ===========================================================================
// M11 — audio-aware polish
// ===========================================================================

// ---- helpers ---------------------------------------------------------------

/// The environment (`SURAMAK_OPENAI_API_KEY` on Linux) is process-global; every
/// test that needs the key holds this lock.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_key<T>(f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK
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

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Reads one complete HTTP request (headers + the full Content-Length body).
fn read_full_request(stream: &mut TcpStream) -> String {
    let mut data: Vec<u8> = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    let mut expected: Option<(usize, usize)> = None; // (body offset, body length)
    loop {
        if expected.is_none() {
            if let Some(pos) = find_bytes(&data, b"\r\n\r\n") {
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

/// Spawns a mock endpoint that serves `expected` connections with `handler`
/// (request index + raw request → (status line, body)) and records every raw
/// request it saw.
fn spawn_endpoint<F>(expected: usize, handler: F) -> (String, Arc<Mutex<Vec<String>>>)
where
    F: Fn(usize, &str) -> (String, String) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let addr = listener.local_addr().unwrap();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    thread::spawn(move || {
        for index in 0..expected {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let request = read_full_request(&mut stream);
            let (status, body) = handler(index, &request);
            record.lock().unwrap().push(request);
            respond(&mut stream, &status, &body);
        }
    });
    (format!("http://{addr}/v1"), seen)
}

/// The raw request body (everything after the header block).
fn raw_body(request: &str) -> &str {
    let at = find_bytes(request.as_bytes(), b"\r\n\r\n").expect("headers") + 4;
    &request[at..]
}

fn request_body(request: &str) -> serde_json::Value {
    serde_json::from_str(raw_body(request).trim_end()).expect("JSON body")
}

/// A 16-bit PCM WAV file with `samples` (interleaved for >1 channel).
fn wav_bytes(samples: &[i16], rate: u32, channels: u16) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).expect("wav writer");
        for sample in samples {
            writer.write_sample(*sample).expect("write sample");
        }
        writer.finalize().expect("finalize");
    }
    cursor.into_inner()
}

/// A WAV of `frames` silent samples at 16 kHz mono.
fn silent_wav(frames: usize) -> Vec<u8> {
    wav_bytes(&vec![0i16; frames], 16_000, 1)
}

/// Writes `bytes` to a fresh temp file and returns (dir, path).
fn temp_audio(tag: &str, bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join(format!("{tag}.wav"));
    std::fs::write(&path, bytes).expect("write temp audio");
    (dir, path)
}

/// OpenAI-style success body carrying `content` as the assistant message.
fn chat_body(content: &str) -> String {
    serde_json::json!({
        "choices": [{ "message": { "role": "assistant", "content": content } }]
    })
    .to_string()
}

/// A canned valid reply for the given segment ids ("Polished N.").
fn polished_reply(ids: &[u32]) -> String {
    let segments: Vec<serde_json::Value> = ids
        .iter()
        .map(|id| serde_json::json!({ "id": id, "text": format!("Polished {id}.") }))
        .collect();
    chat_body(&serde_json::json!({ "segments": segments }).to_string())
}

/// The ids of the payload the request's user message carries (works for both
/// the text-only string content and the audio-aware content-part array).
fn payload_ids(body: &serde_json::Value) -> Vec<u32> {
    let payload: serde_json::Value =
        serde_json::from_str(content_text(&body["messages"][1]["content"])).expect("payload JSON");
    payload["segments"]
        .as_array()
        .expect("segments array")
        .iter()
        .map(|segment| segment["id"].as_u64().expect("id") as u32)
        .collect()
}

/// The user message's text: a plain string (text-only) or the first `text`
/// content part (audio-aware).
fn content_text(content: &serde_json::Value) -> &str {
    match content {
        serde_json::Value::String(text) => text,
        serde_json::Value::Array(parts) => parts
            .iter()
            .find_map(|part| {
                if part["type"] == "text" {
                    part["text"].as_str()
                } else {
                    None
                }
            })
            .expect("a text part"),
        other => panic!("unexpected user content: {other}"),
    }
}

/// The `input_audio` part of the user message, if any: (base64 data, format).
fn audio_part(content: &serde_json::Value) -> Option<(String, String)> {
    let parts = content.as_array()?;
    let part = parts.iter().find(|part| part["type"] == "input_audio")?;
    Some((
        part["input_audio"]["data"].as_str()?.to_string(),
        part["input_audio"]["format"].as_str()?.to_string(),
    ))
}

/// Decodes a chunk's `input_audio` payload back to the raw PCM16 bytes.
fn decoded_audio(content: &serde_json::Value) -> Vec<u8> {
    let (data, format) = audio_part(content).expect("an input_audio part");
    assert_eq!(format, "wav", "the payload must declare the wav format");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .expect("base64 audio");
    Pcm16Wav::parse(&bytes).expect("a readable WAV chunk").data
}

/// The pre-M11 request body, verbatim (M4's code) — the byte-identity anchor.
fn todays_text_only_body(segments: &[Segment], cfg: &PolishSettings) -> serde_json::Value {
    let payload = serde_json::json!({
        "segments": segments
            .iter()
            .map(|s| serde_json::json!({ "id": s.idx, "text": s.text }))
            .collect::<Vec<_>>(),
    });
    let user_content = serde_json::to_string(&payload).expect("payload JSON");
    serde_json::json!({
        "model": cfg.model,
        "temperature": 0,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user",   "content": user_content },
        ],
    })
}

// ---- the chunker (pure) ----------------------------------------------------

#[test]
fn chunker_tiles_every_sample_exactly_once_in_order() {
    // 10 s of 16 kHz mono; cuts at 3 s and 6 s; cap 100 000 raw bytes
    // (= 50 000 frames = 3.125 s) — smaller than a segment span, so the cap
    // forces one cut mid-segment (documented) and boundaries take the rest.
    let wav = Pcm16Wav::parse(&silent_wav(160_000)).expect("parse");
    let segments = vec![
        segment(0, 0, 3_000),
        segment(1, 3_000, 6_000),
        segment(2, 6_000, 10_000),
    ];
    let chunks = chunk_wav(&wav, &segments, 100_000);
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| (chunk.start_sample, chunk.end_sample))
            .collect::<Vec<_>>(),
        vec![
            (0, 48_000),
            (48_000, 96_000),
            (96_000, 146_000),
            (146_000, 160_000)
        ],
        "cuts land on the segment starts that fit; the cap forces the rest"
    );

    // Coverage: the decoded chunks concatenate to the original data, exactly
    // once, in order.
    let mut joined = Vec::new();
    for chunk in &chunks {
        let parsed = Pcm16Wav::parse(&chunk.wav).expect("chunk parses");
        assert_eq!(parsed.sample_rate, 16_000);
        assert_eq!(parsed.channels, 1);
        assert!(parsed.data.len() <= 100_000, "chunk within the cap");
        joined.extend_from_slice(&parsed.data);
    }
    assert_eq!(joined, wav.data, "every sample exactly once, in order");
    assert_eq!(joined.len(), 320_000);
}

#[test]
fn chunker_never_splits_mid_segment_when_boundaries_allow() {
    // 16 s of 16 kHz mono as eight 2 s segments; cap 50 000 frames = 3.125 s.
    // The greedy cut always takes the latest segment start that fits, so no
    // chunk ever ends mid-segment.
    let wav = Pcm16Wav::parse(&silent_wav(256_000)).expect("parse");
    let segments: Vec<Segment> = (0..8u32)
        .map(|i| segment(i, i64::from(i) * 2_000, i64::from(i + 1) * 2_000))
        .collect();
    let chunks = chunk_wav(&wav, &segments, 100_000);
    let boundaries: Vec<usize> = chunks
        .iter()
        .map(|chunk| chunk.start_sample)
        .skip(1)
        .collect();
    // Every 2 s boundary fits within the cap, so the greedy cut takes each —
    // one chunk per segment, no mid-segment cut anywhere.
    assert_eq!(
        boundaries,
        vec![32_000, 64_000, 96_000, 128_000, 160_000, 192_000, 224_000]
    );
    // Every internal boundary is a segment start — no mid-segment cut.
    for boundary in &boundaries {
        assert!(
            segments
                .iter()
                .any(|segment| sample_of_ms(segment.start_ms, 16_000) == *boundary),
            "boundary {boundary} is a segment start"
        );
    }
    let total: usize = chunks
        .iter()
        .map(|chunk| chunk.end_sample - chunk.start_sample)
        .sum();
    assert_eq!(total, 256_000, "the chunks tile the recording");
}

#[test]
fn chunker_force_splits_only_a_segment_longer_than_the_cap() {
    // One 10 s segment; cap 3.125 s → the only possible cuts are the cap
    // itself (mid-segment, documented) — and they stay within the cap.
    let wav = Pcm16Wav::parse(&silent_wav(160_000)).expect("parse");
    let segments = vec![segment(0, 0, 10_000)];
    let chunks = chunk_wav(&wav, &segments, 100_000);
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| (chunk.start_sample, chunk.end_sample))
            .collect::<Vec<_>>(),
        vec![
            (0, 50_000),
            (50_000, 100_000),
            (100_000, 150_000),
            (150_000, 160_000)
        ]
    );
    let mut joined = Vec::new();
    for chunk in &chunks {
        let parsed = Pcm16Wav::parse(&chunk.wav).expect("chunk parses");
        assert!(parsed.data.len() <= 100_000);
        joined.extend_from_slice(&parsed.data);
    }
    assert_eq!(joined, wav.data);

    // Mixed: a >cap segment then a normal one — the second cut is the
    // segment's start (boundary alignment returns as soon as it can).
    let segments = vec![segment(0, 0, 5_000), segment(1, 5_000, 10_000)];
    let chunks = chunk_wav(&wav, &segments, 100_000);
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| (chunk.start_sample, chunk.end_sample))
            .collect::<Vec<_>>(),
        vec![
            (0, 50_000),
            (50_000, 80_000),
            (80_000, 130_000),
            (130_000, 160_000)
        ],
        "the cut at 80 000 (segment 1's start) is taken when it fits"
    );
}

#[test]
fn chunker_covers_what_a_recording_never_exceeds_and_edges_hold() {
    // Empty payload → no chunks.
    let empty = Pcm16Wav {
        sample_rate: 16_000,
        channels: 1,
        data: Vec::new(),
    };
    assert!(chunk_wav(&empty, &make_segments(), 100).is_empty());

    // A recording that fits the cap is one whole chunk — no gratuitous splits.
    let small = Pcm16Wav::parse(&silent_wav(1_000)).expect("parse");
    let chunks = chunk_wav(&small, &make_segments(), AUDIO_CHUNK_CAP_BYTES);
    assert_eq!(chunks.len(), 1);
    assert_eq!((chunks[0].start_sample, chunks[0].end_sample), (0, 1_000));

    // Out-of-range, duplicated and unsorted segment starts are ignored
    // (they cannot be cuts) — no empty chunks, no reordering. Here only the
    // 4 s start survives as a cut (the 40 s one is past the audio's end and
    // the 0 s one cannot cut), so the cap forces the rest.
    let wav = Pcm16Wav::parse(&silent_wav(160_000)).expect("parse");
    let segments = vec![
        segment(0, 40_000, 50_000),
        segment(1, 0, 10_000),
        segment(2, 4_000, 20_000),
        segment(3, 4_000, 20_000),
        segment(4, 500_000, 600_000), // starts past the audio's end
    ];
    let chunks = chunk_wav(&wav, &segments, 100_000);
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| (chunk.start_sample, chunk.end_sample))
            .collect::<Vec<_>>(),
        vec![
            (0, 50_000),
            (50_000, 64_000),
            (64_000, 114_000),
            (114_000, 160_000)
        ]
    );
    for chunk in &chunks {
        assert!(chunk.end_sample > chunk.start_sample, "no empty chunk");
        assert!(
            Pcm16Wav::parse(&chunk.wav).expect("parses").data.len() <= 100_000,
            "cap holds"
        );
    }

    // The rate is honored: a 48 kHz WAV's 1 s cut is at 48 000 frames.
    let wav48 = Pcm16Wav::parse(&wav_bytes(&vec![0i16; 96_000], 48_000, 1)).expect("parse");
    let chunks = chunk_wav(
        &wav48,
        &[segment(0, 0, 1_000), segment(1, 1_000, 2_000)],
        100_000,
    );
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| chunk.start_sample)
            .collect::<Vec<_>>(),
        vec![0, 48_000]
    );
}

// ---- the request builder (pure) --------------------------------------------

#[test]
fn build_body_text_only_is_todays_exact_request() {
    let segments = make_segments();
    let cfg = cfg_for("http://localhost:9/v1");
    let body = build_body(&segments, &cfg, None).expect("body");
    assert_eq!(
        serde_json::to_string(&body).unwrap(),
        serde_json::to_string(&todays_text_only_body(&segments, &cfg)).unwrap(),
        "OFF must be byte-identical to the pre-M11 text-only request"
    );
    // The shape, named explicitly (so a reader sees what is pinned).
    assert_eq!(body["temperature"], 0);
    assert_eq!(body["model"], "mock-model");
    assert_eq!(body["messages"][0]["role"], "system");
    assert!(body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .starts_with("You are a transcript cleanup engine."));
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(
        body["messages"][1]["content"].as_str().unwrap(),
        r#"{"segments":[{"id":0,"text":"helo wrold"},{"id":1,"text":"this iz a tset"}]}"#
    );
}

#[test]
fn build_body_audio_pins_the_exact_json_shape() {
    let segments = make_segments();
    let cfg = cfg_for("http://localhost:9/v1");
    let chunk = AudioChunk {
        start_sample: 0,
        end_sample: 4,
        wav: wav_bytes(&[1, 2, 3, 4], 16_000, 1),
    };
    let body = build_body(&segments, &cfg, Some(&chunk)).expect("body");

    let expected_b64 = base64::engine::general_purpose::STANDARD.encode(&chunk.wav);
    let expected = serde_json::json!({
        "model": "mock-model",
        "temperature": 0,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": [
                { "type": "text", "text": r#"{"segments":[{"id":0,"text":"helo wrold"},{"id":1,"text":"this iz a tset"}]}"# },
                { "type": "input_audio", "input_audio": { "data": expected_b64, "format": "wav" } },
            ]},
        ],
    });
    assert_eq!(body, expected, "the exact JSON shape, pinned");
    // The text part is exactly the text-only user content (same payload).
    assert_eq!(
        body["messages"][1]["content"][0]["text"].as_str().unwrap(),
        build_body(&segments, &cfg, None).unwrap()["messages"][1]["content"]
            .as_str()
            .unwrap()
    );
}

#[test]
fn pcm16_wav_roundtrips_and_scales_like_the_recording_writer() {
    let wav = Pcm16Wav::parse(&wav_bytes(&[-32_768, -1, 0, 1, 32_767], 16_000, 1)).expect("parse");
    assert!(wav.is_16k_mono());
    let bytes = wav.to_bytes().expect("encode");
    assert_eq!(Pcm16Wav::parse(&bytes).unwrap(), wav);

    // f32 → i16 mirrors decode::write_wav_i16 (clamp + round to nearest).
    let converted = Pcm16Wav::from_f32_16k_mono(&[0.0, 1.0, -1.0, 2.0]);
    assert!(converted.is_16k_mono());
    let samples: Vec<i16> = converted
        .data
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    assert_eq!(samples, vec![0, 32_767, -32_767, 32_767]);
}

// ---- the client against a mock endpoint ------------------------------------

#[test]
fn audio_off_is_byte_identical_to_todays_request_on_the_wire() {
    let (url, seen) = spawn_endpoint(2, |_index, request| {
        assert!(request.contains("POST /v1/chat/completions"));
        ("200 OK".to_string(), polished_reply(&[0, 1]))
    });
    let segments = make_segments();
    let cfg = cfg_for(&url);

    let outcome = with_key(|| {
        let backend = OpenAiCompat::new().expect("client");
        // The M4 entry point (the CLI's path)…
        let plain = backend.polish(&segments, &cfg).expect("text-only polish");
        assert_eq!(plain[0].text, "Polished 0.");
        // …and the M11 entry with audio OFF.
        backend
            .polish_with_audio(&segments, &cfg, AudioRequest::Off)
            .expect("off polish")
    });
    assert!(outcome.notes.is_empty(), "OFF carries no notes");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "one request each");
    let expected = serde_json::to_string(&todays_text_only_body(&segments, &cfg)).unwrap();
    assert_eq!(
        raw_body(&seen[0]).trim_end(),
        expected,
        "the M4 request is byte-identical to the pre-M11 body"
    );
    assert_eq!(
        raw_body(&seen[1]).trim_end(),
        expected,
        "OFF must be byte-identical to the pre-M11 body"
    );
    assert_eq!(seen[0], seen[1], "byte-identical requests");
}

#[test]
fn audio_aware_requests_carry_the_chunks_with_full_coverage() {
    // 140 s of 16 kHz mono (4 480 000 raw bytes > the 4 MB cap): segments at
    // 0 s, 60 s and 120 s → two chunks, split at the 120 s boundary.
    let frames = 140 * 16_000;
    let wav = Pcm16Wav::parse(&silent_wav(frames)).expect("parse");
    let segments = vec![
        segment(0, 0, 60_000),
        segment(1, 60_000, 120_000),
        segment(2, 120_000, 140_000),
    ];

    let (url, seen) = spawn_endpoint(2, |_index, request| {
        let body = request_body(request);
        let ids = payload_ids(&body);
        ("200 OK".to_string(), polished_reply(&ids))
    });
    let cfg = cfg_for(&url);
    let (_dir, path) = temp_audio("big", &wav.to_bytes().expect("encode"));
    let outcome = with_key(|| {
        OpenAiCompat::new()
            .expect("client")
            .polish_with_audio(&segments, &cfg, AudioRequest::Recording(path))
            .expect("audio-aware polish")
    });

    // The corrected outputs, concatenated in order.
    assert!(outcome.notes.is_empty(), "no degradation notes");
    assert_eq!(
        outcome
            .segments
            .iter()
            .map(|segment| (segment.idx, segment.text.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "Polished 0."), (1, "Polished 1."), (2, "Polished 2.")]
    );

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "one request per chunk");
    let bodies: Vec<serde_json::Value> = seen.iter().map(|r| request_body(r)).collect();

    // Each request: system message unchanged + text part + one input_audio part.
    for body in &bodies {
        assert_eq!(body["messages"][0]["content"], SYSTEM_PROMPT);
        assert_eq!(body["messages"][1]["content"][0]["type"], "text");
        let (_, format) = audio_part(&body["messages"][1]["content"]).expect("audio part");
        assert_eq!(format, "wav");
        assert_eq!(
            body["messages"][1]["content"].as_array().unwrap().len(),
            2,
            "exactly one audio part per request"
        );
    }
    // The chunk split: [0 s, 120 s) + [120 s, end).
    assert_eq!(payload_ids(&bodies[0]), vec![0, 1]);
    assert_eq!(payload_ids(&bodies[1]), vec![2]);

    // Coverage: the chunks' decoded audio concatenates to the recording's PCM,
    // exactly once, in order — and each chunk is within the cap.
    let mut joined = Vec::new();
    for body in &bodies {
        let audio = decoded_audio(&body["messages"][1]["content"]);
        assert!(
            audio.len() <= AUDIO_CHUNK_CAP_BYTES,
            "raw chunk within the 4 MB cap"
        );
        joined.extend_from_slice(&audio);
    }
    assert_eq!(joined, wav.data, "every sample exactly once, in order");
    assert_eq!(joined.len(), 4_480_000);
}

#[test]
fn a_single_chunk_recording_sends_one_request_with_all_segments() {
    let wav = Pcm16Wav::parse(&silent_wav(32_000)).expect("parse");
    let segments = make_segments();
    let (url, seen) = spawn_endpoint(1, |_index, request| {
        let ids = payload_ids(&request_body(request));
        ("200 OK".to_string(), polished_reply(&ids))
    });
    let cfg = cfg_for(&url);
    let (_dir, path) = temp_audio("single", &wav.to_bytes().expect("encode"));
    let outcome = with_key(|| {
        OpenAiCompat::new()
            .expect("client")
            .polish_with_audio(&segments, &cfg, AudioRequest::Recording(path))
            .expect("polish")
    });
    assert!(outcome.notes.is_empty());
    assert_eq!(outcome.segments[0].text, "Polished 0.");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let body = request_body(&seen[0]);
    assert_eq!(
        payload_ids(&body),
        vec![0, 1],
        "all segments in the one chunk"
    );
    assert!(audio_part(&body["messages"][1]["content"]).is_some());
    // The chunk carries the whole recording's PCM.
    assert_eq!(
        decoded_audio(&body["messages"][1]["content"]),
        wav.data,
        "the single chunk covers everything"
    );
}

#[test]
fn a_4xx_on_the_audio_request_retries_text_only_once_with_a_note() {
    let (url, seen) = spawn_endpoint(2, |index, request| {
        if index == 0 {
            assert!(
                audio_part(&request_body(request)["messages"][1]["content"]).is_some(),
                "the first attempt must carry the recording"
            );
            (
                "400 Bad Request".to_string(),
                r#"{"error":{"message":"this model does not accept audio input"}}"#.to_string(),
            )
        } else {
            let ids = payload_ids(&request_body(request));
            ("200 OK".to_string(), polished_reply(&ids))
        }
    });
    let segments = make_segments();
    let cfg = cfg_for(&url);
    let wav = Pcm16Wav::parse(&silent_wav(32_000)).expect("parse");
    let (_dir, path) = temp_audio("fallback", &wav.to_bytes().expect("encode"));

    let outcome = with_key(|| {
        OpenAiCompat::new()
            .expect("client")
            .polish_with_audio(&segments, &cfg, AudioRequest::Recording(path))
            .expect("the ladder never fails hard on a 4xx")
    });

    // The retry's output is the whole transcript, and the note names the reason.
    assert_eq!(
        outcome
            .segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>(),
        vec!["Polished 0.", "Polished 1."]
    );
    assert_eq!(outcome.notes.len(), 1, "exactly one note");
    let note = &outcome.notes[0];
    assert!(note.contains("HTTP 400"), "names the status: {note}");
    assert!(
        note.contains("does not accept audio input"),
        "names the endpoint's reason: {note}"
    );
    assert!(note.contains("retried without"), "names the retry: {note}");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "exactly one retry");
    let retry = request_body(&seen[1]);
    assert!(
        audio_part(&retry["messages"][1]["content"]).is_none(),
        "the retry is text-only"
    );
    assert_eq!(
        raw_body(&seen[1]).trim_end(),
        serde_json::to_string(&todays_text_only_body(&segments, &cfg)).unwrap(),
        "the retry is byte-identical to today's text-only request"
    );
}

#[test]
fn a_non_4xx_failure_on_the_audio_request_still_fails_the_run() {
    let (url, seen) = spawn_endpoint(1, |_index, _request| {
        (
            "500 Internal Server Error".to_string(),
            r#"{"error":"server fault"}"#.to_string(),
        )
    });
    let segments = make_segments();
    let cfg = cfg_for(&url);
    let wav = Pcm16Wav::parse(&silent_wav(32_000)).expect("parse");
    let (_dir, path) = temp_audio("five-hundred", &wav.to_bytes().expect("encode"));

    let err = with_key(|| {
        OpenAiCompat::new()
            .expect("client")
            .polish_with_audio(&segments, &cfg, AudioRequest::Recording(path))
            .expect_err("a 5xx is not an audio rejection — the run fails like today")
    });
    assert!(err.to_string().contains("HTTP 500"), "unexpected: {err}");
    assert_eq!(seen.lock().unwrap().len(), 1, "no retry on a 5xx");
}

#[test]
fn a_missing_or_unusable_recording_degrades_to_text_only_with_a_note() {
    let segments = make_segments();

    // (a) The toggle is on but the UI found no recording: the note passes
    // through, the request is today's text-only one.
    let (url, seen) = spawn_endpoint(1, |_index, request| {
        assert!(audio_part(&request_body(request)["messages"][1]["content"]).is_none());
        ("200 OK".to_string(), polished_reply(&[0, 1]))
    });
    let cfg = cfg_for(&url);
    let note = "the recording was not sent — keep audio was off; the text was polished on its own";
    let outcome = with_key(|| {
        OpenAiCompat::new()
            .expect("client")
            .polish_with_audio(
                &segments,
                &cfg,
                AudioRequest::Unavailable {
                    note: note.to_string(),
                },
            )
            .expect("polish")
    });
    assert_eq!(outcome.notes, vec![note.to_string()]);
    assert_eq!(outcome.segments[0].text, "Polished 0.");
    assert_eq!(seen.lock().unwrap().len(), 1);

    // (b) The path vanished between the check and the run: text-only + a note.
    let (url, seen) = spawn_endpoint(1, |_index, _request| {
        ("200 OK".to_string(), polished_reply(&[0, 1]))
    });
    let cfg = cfg_for(&url);
    let outcome = with_key(|| {
        OpenAiCompat::new()
            .expect("client")
            .polish_with_audio(
                &segments,
                &cfg,
                AudioRequest::Recording(std::path::PathBuf::from(
                    "/definitely/not/here/recording.wav",
                )),
            )
            .expect("polish")
    });
    assert_eq!(outcome.notes.len(), 1);
    assert!(
        outcome.notes[0].contains("could not read the recording"),
        "unexpected: {}",
        outcome.notes[0]
    );
    assert_eq!(seen.lock().unwrap().len(), 1, "text-only only");

    // (c) A file that is not decodable audio at all: text-only + a note.
    let (url, seen) = spawn_endpoint(1, |_index, _request| {
        ("200 OK".to_string(), polished_reply(&[0, 1]))
    });
    let cfg = cfg_for(&url);
    let (_dir, path) = temp_audio("garbage", b"this is not audio");
    let outcome = with_key(|| {
        OpenAiCompat::new()
            .expect("client")
            .polish_with_audio(&segments, &cfg, AudioRequest::Recording(path))
            .expect("polish")
    });
    assert_eq!(outcome.notes.len(), 1);
    assert!(
        outcome.notes[0].contains("could not decode the recording"),
        "unexpected: {}",
        outcome.notes[0]
    );
    assert_eq!(seen.lock().unwrap().len(), 1);

    // (d) A WAV with no samples: text-only + a note.
    let (url, seen) = spawn_endpoint(1, |_index, _request| {
        ("200 OK".to_string(), polished_reply(&[0, 1]))
    });
    let cfg = cfg_for(&url);
    let (_dir, path) = temp_audio("empty", &silent_wav(0));
    let outcome = with_key(|| {
        OpenAiCompat::new()
            .expect("client")
            .polish_with_audio(&segments, &cfg, AudioRequest::Recording(path))
            .expect("polish")
    });
    assert_eq!(outcome.notes.len(), 1);
    assert!(
        outcome.notes[0].contains("holds no audio"),
        "unexpected: {}",
        outcome.notes[0]
    );
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn an_imported_non_16k_recording_is_decoded_and_resampled_for_the_wire() {
    // The file on disk is a WAV but not the wire shape (48 kHz here), so the
    // client goes through the decode + resample branch — the same one an
    // imported mp3/mp4 would take.
    let source = wav_bytes(&vec![1_000i16; 48_000], 48_000, 1);
    let (_dir, path) = temp_audio("imported", &source);
    let segments = make_segments();
    let (url, seen) = spawn_endpoint(1, |_index, request| {
        let ids = payload_ids(&request_body(request));
        ("200 OK".to_string(), polished_reply(&ids))
    });
    let cfg = cfg_for(&url);
    let outcome = with_key(|| {
        OpenAiCompat::new()
            .expect("client")
            .polish_with_audio(&segments, &cfg, AudioRequest::Recording(path))
            .expect("polish")
    });
    assert!(outcome.notes.is_empty(), "no note: the audio was sent");

    let seen = seen.lock().unwrap();
    let body = request_body(&seen[0]);
    let (data, format) = audio_part(&body["messages"][1]["content"]).expect("audio part");
    assert_eq!(format, "wav");
    let chunk = Pcm16Wav::parse(
        &base64::engine::general_purpose::STANDARD
            .decode(data)
            .expect("base64"),
    )
    .expect("WAV chunk");
    // 1 s at 48 kHz → ≈1 s at 16 kHz mono PCM16. The length is the pipeline's
    // own resampler output (rubato's FFT resampler pads the final chunk, so
    // the tail is approximate — the same as any imported file's transcript).
    assert_eq!(chunk.sample_rate, 16_000);
    assert_eq!(chunk.channels, 1);
    let seconds = chunk.data.len() as f64 / 32_000.0;
    assert!(
        (0.95..=1.05).contains(&seconds),
        "≈1 s of audio, got {seconds} s ({} bytes)",
        chunk.data.len()
    );
}
