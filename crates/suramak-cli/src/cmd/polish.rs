//! `polish <id> [--base-url U] [--model M]`
//!
//! Sends the stored segments to an OpenAI-compatible `/chat/completions` endpoint,
//! validates the response and upserts the polished texts back into the library.
//! Requires `SURAMAK_OPENAI_API_KEY` on Linux (keychain on macOS/Windows).

use anyhow::{bail, Result};
use clap::Args;
use suramak_core::paths;
use suramak_core::polish::{OpenAiCompat, PolishBackend};
use suramak_core::settings::{PolishSettings, Settings};
use suramak_core::store::Store;

use crate::cmd::load_transcript;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_MODEL: &str = "gpt-4o-mini";

#[derive(Args)]
pub struct PolishArgs {
    /// Transcript id (uuid) from the library — see `suramak-cli library list`
    pub id: String,

    /// OpenAI-compatible base URL (default: settings.toml, else OpenAI)
    #[arg(long)]
    pub base_url: Option<String>,

    /// Model name (default: settings.toml, else gpt-4o-mini)
    #[arg(long)]
    pub model: Option<String>,
}

pub fn run(args: PolishArgs) -> Result<()> {
    let store = Store::open(&paths::db_path()?)?;
    let transcript = load_transcript(&store, &args.id)?;
    if transcript.segments.is_empty() {
        bail!("transcript {} has no segments to polish", transcript.id);
    }

    // Flags beat settings.toml, which beats the built-in defaults.
    let settings = Settings::load(&paths::settings_path()?)?;
    let base_url = args
        .base_url
        .or_else(|| settings.polish.as_ref().map(|p| p.base_url.clone()))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let model = args
        .model
        .or_else(|| settings.polish.as_ref().map(|p| p.model.clone()))
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let cfg = PolishSettings {
        base_url: base_url.clone(),
        model: model.clone(),
    };
    let backend = OpenAiCompat::new()?;

    println!(
        "polishing {} segment(s) with '{}' at {}",
        transcript.segments.len(),
        model,
        base_url
    );
    let polished = backend.polish(&transcript.segments, &cfg)?;
    store.upsert_segments(transcript.id, &polished)?;
    println!(
        "polished {} segment(s) and saved to the library",
        polished.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    //! `polish` against a hand-rolled OpenAI-compatible mock endpoint — the same
    //! `TcpListener` pattern as `suramak-core/tests/polish.rs` (no new deps).
    //! This drives the *real* command path (arg resolution → blocking client →
    //! validated reply → library upsert) end to end.
    //!
    //! The environment (`XDG_DATA_HOME` for the data dir, `SURAMAK_OPENAI_API_KEY`
    //! for the key) is process-global; this is the only test in the CLI test
    //! binary, and it restores both variables before returning.

    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use suramak_core::paths;
    use suramak_core::store::Store;
    use suramak_core::transcript::{Segment, Source, Transcript};

    use super::*;

    fn read_request(stream: &mut TcpStream) -> String {
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).unwrap_or(0);
        String::from_utf8_lossy(&buf[..n]).to_string()
    }

    fn respond(stream: &mut TcpStream, status: &str, body: &str) {
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(resp.as_bytes()).ok();
    }

    /// Spawns a mock endpoint that serves up to 4 connections, recording the
    /// requests it saw.
    fn spawn_mock(handler: fn(&mut TcpStream, &str)) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let addr = listener.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        thread::spawn(move || {
            for _ in 0..4 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let request = read_request(&mut stream);
                if let Ok(mut guard) = record.lock() {
                    guard.push(request.clone());
                }
                handler(&mut stream, &request);
            }
        });
        (format!("http://{addr}/v1"), seen)
    }

    fn success_handler(stream: &mut TcpStream, _request: &str) {
        let content =
            r#"{"segments":[{"id":0,"text":"Hello world."},{"id":1,"text":"This is a test."}]}"#;
        let body = format!(
            "{{\"choices\":[{{\"message\":{{\"role\":\"assistant\",\"content\":\"{}\"}}}}]}}",
            content.replace('\\', "\\\\").replace('"', "\\\"")
        );
        respond(stream, "200 OK", &body);
    }

    #[test]
    fn polish_against_a_mock_endpoint_updates_the_library() {
        let root = std::env::temp_dir().join(format!("suramak-cli-polish-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("temp root");
        let previous_xdg = std::env::var_os("XDG_DATA_HOME");
        let previous_key = std::env::var_os("SURAMAK_OPENAI_API_KEY");
        std::env::set_var("XDG_DATA_HOME", &root);
        std::env::set_var("SURAMAK_OPENAI_API_KEY", "test-key");

        let (base_url, seen) = spawn_mock(success_handler);
        let transcript = Transcript::new(
            "polish-cli.wav".to_string(),
            Source::File {
                name: "polish-cli.wav".to_string(),
            },
            "tiny".to_string(),
            Some("en".to_string()),
            None,
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
            ],
        );
        Store::open(&paths::db_path().expect("db path"))
            .expect("store")
            .insert_transcript(&transcript)
            .expect("insert");

        run(PolishArgs {
            id: transcript.id.to_string(),
            base_url: Some(base_url),
            model: Some("mock-model".to_string()),
        })
        .expect("the polish command must succeed against the mock");

        // The command hit the OpenAI-compatible route with the key (HTTP header
        // names arrive lowercase from hyper, so compare case-insensitively).
        let requests = seen.lock().expect("mock request log").clone();
        assert_eq!(requests.len(), 1, "exactly one request");
        let request = requests[0].to_lowercase();
        assert!(request.contains("post /v1/chat/completions"));
        assert!(
            request.contains("authorization: bearer"),
            "the bearer key must be sent: {}",
            requests[0]
        );

        // …and the polished texts are in the library (fresh connection).
        let stored = Store::open(&paths::db_path().expect("db path"))
            .expect("store")
            .get_transcript(transcript.id)
            .expect("stored transcript");
        assert_eq!(stored.segments[0].text, "Hello world.");
        assert_eq!(stored.segments[1].text, "This is a test.");

        match previous_xdg {
            Some(value) => std::env::set_var("XDG_DATA_HOME", value),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        match previous_key {
            Some(value) => std::env::set_var("SURAMAK_OPENAI_API_KEY", value),
            None => std::env::remove_var("SURAMAK_OPENAI_API_KEY"),
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
