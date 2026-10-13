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

// ---------------------------------------------------------------------------
// `.zip` artifacts (Core ML encoder packages)
// ---------------------------------------------------------------------------

/// Builds an in-memory zip with the given entries (name, bytes). Names ending in `/`
/// become directory entries.
fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    let mut buf = Vec::new();
    {
        let mut writer = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, data) in entries {
            if name.ends_with('/') {
                writer.add_directory(*name, options).unwrap();
            } else {
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
        }
        writer.finish().unwrap();
    }
    buf
}

/// A synthetic encoder package shaped like the real ones: everything under one
/// top-level directory, plus a macOS `__MACOSX/` junk entry.
fn synthetic_encoder_zip() -> Vec<u8> {
    build_zip(&[
        ("ggml-tiny-encoder.mlmodelc/", b""),
        ("ggml-tiny-encoder.mlmodelc/coremldata.bin", b"coreml"),
        ("ggml-tiny-encoder.mlmodelc/metadata.json", b"{}"),
        ("ggml-tiny-encoder.mlmodelc/model.mil", b"mil"),
        ("ggml-tiny-encoder.mlmodelc/weights/", b""),
        ("ggml-tiny-encoder.mlmodelc/weights/weight.bin", b"weights"),
        (
            "__MACOSX/ggml-tiny-encoder.mlmodelc/._coremldata.bin",
            b"junk",
        ),
    ])
}

const REQUIRED: &[&str] = &[
    "coremldata.bin",
    "metadata.json",
    "model.mil",
    "weights/weight.bin",
];

#[test]
fn extract_zip_flattens_top_level_and_skips_junk() {
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("pkg.zip");
    std::fs::write(&zip_path, synthetic_encoder_zip()).unwrap();

    let dest = dir.path().join("ggml-tiny-encoder.mlmodelc");
    suramak_core::download::extract_zip(&zip_path, &dest).unwrap();

    assert!(suramak_core::download::dir_ready(&dest, REQUIRED));
    assert_eq!(
        std::fs::read(dest.join("weights/weight.bin")).unwrap(),
        b"weights"
    );
    // The archive's own top-level directory must not be nested inside `dest`.
    assert!(!dest.join("ggml-tiny-encoder.mlmodelc").exists());
    // `__MACOSX/` junk is skipped, not written next to the package.
    assert!(!dir.path().join("__MACOSX").exists());
    assert!(!dest.join("._coremldata.bin").exists());
    // No temp directory is left behind.
    assert!(!dir
        .path()
        .join(format!(
            "ggml-tiny-encoder.mlmodelc.{}.tmp",
            std::process::id()
        ))
        .exists());
}

#[test]
fn extract_zip_replaces_an_existing_directory() {
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("pkg.zip");
    std::fs::write(&zip_path, synthetic_encoder_zip()).unwrap();

    let dest = dir.path().join("ggml-tiny-encoder.mlmodelc");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("stale.bin"), b"stale").unwrap();

    suramak_core::download::extract_zip(&zip_path, &dest).unwrap();
    assert!(suramak_core::download::dir_ready(&dest, REQUIRED));
    assert!(!dest.join("stale.bin").exists(), "stale files must be gone");
}

#[test]
fn extract_zip_rejects_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("evil.zip");
    std::fs::write(
        &zip_path,
        build_zip(&[
            ("pkg/", b""),
            ("pkg/../escape.txt", b"escaped"),
            ("pkg/ok.txt", b"ok"),
        ]),
    )
    .unwrap();

    let dest = dir.path().join("pkg");
    let result = suramak_core::download::extract_zip(&zip_path, &dest);
    assert!(result.is_err(), "traversal entry must be rejected");
    assert!(!dest.exists(), "nothing may be extracted on rejection");
    assert!(!dir.path().join("escape.txt").exists());
}

#[test]
fn download_zip_mock_server_verifies_and_extracts() {
    let zip_bytes = synthetic_encoder_zip();
    let expected_hash = sha256_of(&zip_bytes);
    let addr = spawn_file_server(zip_bytes.clone());

    let dir = tempfile::tempdir().unwrap();
    let url = format!("http://{addr}/ggml-tiny-encoder.mlmodelc.zip");

    let dest = suramak_core::download::download_zip(
        &url,
        dir.path(),
        "ggml-tiny-encoder.mlmodelc.zip",
        "ggml-tiny-encoder.mlmodelc",
        &expected_hash,
        REQUIRED,
    )
    .unwrap();

    assert_eq!(dest, dir.path().join("ggml-tiny-encoder.mlmodelc"));
    assert!(suramak_core::download::dir_ready(&dest, REQUIRED));
    // The archive is removed after a successful extraction.
    assert!(!dir.path().join("ggml-tiny-encoder.mlmodelc.zip").exists());

    // Idempotent: a second call short-circuits without contacting the server.
    let again = suramak_core::download::download_zip(
        "http://127.0.0.1:1/unreachable.zip",
        dir.path(),
        "ggml-tiny-encoder.mlmodelc.zip",
        "ggml-tiny-encoder.mlmodelc",
        &expected_hash,
        REQUIRED,
    )
    .unwrap();
    assert_eq!(again, dest);
}

#[test]
fn download_zip_checksum_mismatch_leaves_no_directory() {
    let zip_bytes = synthetic_encoder_zip();
    let addr = spawn_file_server(zip_bytes);

    let dir = tempfile::tempdir().unwrap();
    let url = format!("http://{addr}/ggml-tiny-encoder.mlmodelc.zip");

    let result = suramak_core::download::download_zip(
        &url,
        dir.path(),
        "ggml-tiny-encoder.mlmodelc.zip",
        "ggml-tiny-encoder.mlmodelc",
        "0000000000000000000000000000000000000000000000000000000000000000",
        REQUIRED,
    );
    assert!(result.is_err());
    assert!(!dir.path().join("ggml-tiny-encoder.mlmodelc").exists());
}

#[test]
fn download_zip_requires_the_pinned_files() {
    // A zip that extracts fine but misses a required file must be rejected.
    let zip_bytes = build_zip(&[
        ("ggml-tiny-encoder.mlmodelc/", b""),
        ("ggml-tiny-encoder.mlmodelc/coremldata.bin", b"coreml"),
    ]);
    let expected_hash = sha256_of(&zip_bytes);
    let addr = spawn_file_server(zip_bytes);

    let dir = tempfile::tempdir().unwrap();
    let url = format!("http://{addr}/ggml-tiny-encoder.mlmodelc.zip");

    let result = suramak_core::download::download_zip(
        &url,
        dir.path(),
        "ggml-tiny-encoder.mlmodelc.zip",
        "ggml-tiny-encoder.mlmodelc",
        &expected_hash,
        REQUIRED,
    );
    let err = result.expect_err("incomplete package must fail");
    assert!(
        err.to_string().contains("required files are missing"),
        "unexpected error: {err}"
    );
}
