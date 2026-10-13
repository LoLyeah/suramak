use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use suramak_core::download::{download, verify_sha256};

/// Minimal HTTP/1.1 response writer.
fn send_response(stream: &mut TcpStream, status: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).ok();
    stream.write_all(body).ok();
}

/// Reads the full HTTP request headers from the stream.
fn read_request(stream: &mut TcpStream) -> String {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    String::from_utf8_lossy(&buf[..n]).to_string()
}

/// Spawns a mock HTTP server that serves `data` and supports `Range` requests.
/// Returns the bound address.
fn spawn_file_server(data: Vec<u8>) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        // Accept exactly two connections: one fresh, one resume (or just one).
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let req = read_request(&mut stream);

            // Parse Range header.
            let range_start = req
                .lines()
                .find(|l| l.to_lowercase().starts_with("range:"))
                .and_then(|l| l.split_once('='))
                .and_then(|(_, r)| r.trim_end_matches('-').trim().parse::<usize>().ok());

            match range_start {
                Some(start) if start < data.len() => {
                    let body = &data[start..];
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\n\
                         Content-Range: bytes {start}-{}/{}\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n",
                        data.len() - 1,
                        data.len(),
                        body.len()
                    );
                    stream.write_all(header.as_bytes()).ok();
                    stream.write_all(body).ok();
                }
                _ => {
                    send_response(&mut stream, "200 OK", &data);
                }
            }
        }
    });
    addr
}

/// Sha256 of a slice.
fn sha256_of(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(data))
}

#[test]
fn download_fresh() {
    let body: Vec<u8> = (0u8..128).collect();
    let expected_hash = sha256_of(&body);
    let addr = spawn_file_server(body.clone());

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    let url = format!("http://{addr}/model.bin");

    download(&url, &dest, &expected_hash).unwrap();
    assert!(dest.exists());

    let got = std::fs::read(&dest).unwrap();
    assert_eq!(got, body);
}

#[test]
fn download_resume_from_part() {
    let body: Vec<u8> = (0u8..200).collect();
    let expected_hash = sha256_of(&body);
    let addr = spawn_file_server(body.clone());

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    let part = dir.path().join("model.bin.part");

    // Write the first half as a pre-existing .part file.
    std::fs::write(&part, &body[..100]).unwrap();

    let url = format!("http://{addr}/model.bin");
    download(&url, &dest, &expected_hash).unwrap();

    let got = std::fs::read(&dest).unwrap();
    assert_eq!(got, body);
}

#[test]
fn download_checksum_mismatch_returns_error() {
    let body: Vec<u8> = vec![1, 2, 3];
    let addr = spawn_file_server(body);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("bad.bin");
    let url = format!("http://{addr}/bad.bin");

    let result = download(
        &url,
        &dest,
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    assert!(result.is_err());
    // dest must not exist (rename did not happen)
    assert!(!dest.exists());
}

#[test]
fn download_skips_if_dest_exists() {
    // No server needed — file already there.
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("exists.bin");
    std::fs::write(&dest, b"data").unwrap();

    // Arbitrary URL that would fail if actually contacted.
    download("http://127.0.0.1:1/nope", &dest, "any").unwrap();
    // Should not error because dest already exists.
}

#[test]
fn verify_sha256_ok_and_fail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.bin");
    std::fs::write(&path, b"hello").unwrap();

    use sha2::{Digest, Sha256};
    let hash = format!("{:x}", Sha256::digest(b"hello"));
    verify_sha256(&path, &hash).unwrap();

    let bad = verify_sha256(
        &path,
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    assert!(bad.is_err());
}
