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
fn download_with_progress_reports_monotonic_bytes() {
    use suramak_core::download::download_with_progress;

    let body: Vec<u8> = (0u8..=255).collect();
    let expected_hash = sha256_of(&body);
    let addr = spawn_file_server(body.clone());

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("progress.bin");
    let url = format!("http://{addr}/progress.bin");

    let mut samples: Vec<u64> = Vec::new();
    download_with_progress(&url, &dest, &expected_hash, |bytes| samples.push(bytes)).unwrap();

    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert!(!samples.is_empty(), "progress must fire at least once");
    assert!(
        samples.windows(2).all(|w| w[0] <= w[1]),
        "progress must be monotonic: {samples:?}"
    );
    // The final report is the full size (fires per 64 KiB chunk → once here).
    assert_eq!(*samples.last().unwrap(), body.len() as u64);

    // A resumed download reports its starting offset first.
    let dir2 = tempfile::tempdir().unwrap();
    let dest2 = dir2.path().join("resume.bin");
    let addr2 = spawn_file_server(body.clone());
    std::fs::write(dir2.path().join("resume.bin.part"), &body[..100]).unwrap();
    let mut resumed: Vec<u64> = Vec::new();
    download_with_progress(
        &format!("http://{addr2}/resume.bin"),
        &dest2,
        &expected_hash,
        |bytes| resumed.push(bytes),
    )
    .unwrap();
    assert_eq!(resumed[0], 100, "first report is the pre-existing offset");
    assert_eq!(*resumed.last().unwrap(), body.len() as u64);
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
    // No temp directory is left behind (the name carries a per-call sequence
    // number since SUR-MED-01 — assert on the shape, not one exact name).
    let leftovers: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "staging dirs must be renamed away: {leftovers:?}"
    );
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

// ---------------------------------------------------------------------------
// M10A: multi-file pinned downloads (the Parakeet model directory)
// ---------------------------------------------------------------------------

/// Mock HTTP server serving several files by path (up to `max_requests`).
fn spawn_multi_file_server(
    files: Vec<(String, Vec<u8>)>,
    max_requests: usize,
) -> std::net::SocketAddr {
    use std::collections::HashMap;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let table: HashMap<String, Vec<u8>> = files.into_iter().collect();
    thread::spawn(move || {
        for _ in 0..max_requests {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let req = read_request(&mut stream);
            let path = req
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or_default()
                .to_string();
            match table.get(&path) {
                Some(body) => send_response(&mut stream, "200 OK", body),
                None => send_response(&mut stream, "404 Not Found", b"not found"),
            }
        }
    });
    addr
}

const PARAKEET_TEST_FILES: &[(&str, &str)] = &[
    ("encoder.int8.onnx", "encoder-bytes"),
    ("decoder.int8.onnx", "decoder-bytes"),
    ("joiner.int8.onnx", "joiner-bytes"),
    ("tokens.txt", "token-bytes"),
];

#[test]
fn download_files_fetches_every_pinned_file_and_is_idempotent() {
    let bodies: Vec<(String, Vec<u8>)> = PARAKEET_TEST_FILES
        .iter()
        .map(|(name, body)| (format!("/{name}"), body.as_bytes().to_vec()))
        .collect();
    let addr = spawn_multi_file_server(bodies, 8);

    let pins: Vec<(&str, u64, String)> = PARAKEET_TEST_FILES
        .iter()
        .map(|(name, body)| (*name, body.len() as u64, sha256_of(body.as_bytes())))
        .collect();
    let pins: Vec<(&str, u64, &str)> = pins
        .iter()
        .map(|(name, size, hash)| (*name, *size, hash.as_str()))
        .collect();
    let required: Vec<&str> = PARAKEET_TEST_FILES.iter().map(|(name, _)| *name).collect();

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("parakeet");
    suramak_core::download::download_files(
        &dest,
        &pins,
        |name| format!("http://{addr}/{name}"),
        &required,
    )
    .unwrap();

    for (name, body) in PARAKEET_TEST_FILES {
        assert_eq!(std::fs::read(dest.join(name)).unwrap(), body.as_bytes());
    }
    // No `.part` files are left behind.
    let leftovers: Vec<String> = std::fs::read_dir(&dest)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".part"))
        .collect();
    assert!(leftovers.is_empty(), "leftover part files: {leftovers:?}");

    // Idempotent: a second call verifies what is on disk and never contacts the
    // server (the closure would panic if it were called).
    suramak_core::download::download_files(
        &dest,
        &pins,
        |name| panic!("must not download {name} again"),
        &required,
    )
    .unwrap();
}

#[test]
fn download_files_rejects_a_checksum_mismatch() {
    let bodies: Vec<(String, Vec<u8>)> = PARAKEET_TEST_FILES
        .iter()
        .map(|(name, body)| (format!("/{name}"), body.as_bytes().to_vec()))
        .collect();
    let addr = spawn_multi_file_server(bodies, 8);

    let good: Vec<(&str, u64, String)> = PARAKEET_TEST_FILES
        .iter()
        .map(|(name, body)| (*name, body.len() as u64, sha256_of(body.as_bytes())))
        .collect();
    // The first file's pin is wrong on purpose.
    let mut pins: Vec<(&str, u64, &str)> = good
        .iter()
        .map(|(name, size, hash)| (*name, *size, hash.as_str()))
        .collect();
    pins[0].2 = "0000000000000000000000000000000000000000000000000000000000000000";
    let required: Vec<&str> = PARAKEET_TEST_FILES.iter().map(|(name, _)| *name).collect();

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("parakeet");
    let err = suramak_core::download::download_files(
        &dest,
        &pins,
        |name| format!("http://{addr}/{name}"),
        &required,
    )
    .expect_err("a checksum mismatch must fail the install");
    assert!(
        err.to_string().contains("Checksum mismatch"),
        "unexpected error: {err}"
    );
    // The mismatching file never lands under its final name.
    assert!(!dest.join("encoder.int8.onnx").exists());
}

#[test]
fn download_files_verifies_a_corrupted_existing_install() {
    // A complete-looking directory whose bytes do not match the pins must be
    // rejected (a corrupted install is never trusted).
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("parakeet");
    std::fs::create_dir_all(&dest).unwrap();
    let mut pins: Vec<(&str, u64, String)> = Vec::new();
    for (name, body) in PARAKEET_TEST_FILES {
        // Same size, wrong bytes → the sha256 (not the size) is what catches it.
        std::fs::write(dest.join(name), vec![b'x'; body.len()]).unwrap();
        pins.push((name, body.len() as u64, sha256_of(body.as_bytes())));
    }
    let pins: Vec<(&str, u64, &str)> = pins
        .iter()
        .map(|(name, size, hash)| (*name, *size, hash.as_str()))
        .collect();
    let required: Vec<&str> = PARAKEET_TEST_FILES.iter().map(|(name, _)| *name).collect();

    let err = suramak_core::download::download_files(
        &dest,
        &pins,
        |name| panic!("must not download {name} — the directory looks complete"),
        &required,
    )
    .expect_err("corrupted bytes must fail verification");
    assert!(
        err.to_string().contains("Checksum mismatch"),
        "unexpected error: {err}"
    );
}

#[test]
fn download_files_rejects_a_truncated_existing_file_by_size() {
    // A truncated file (wrong size) fails the cheap size pre-check with a clear
    // message, before any hashing.
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("parakeet");
    std::fs::create_dir_all(&dest).unwrap();
    let mut pins: Vec<(&str, u64, String)> = Vec::new();
    for (i, (name, body)) in PARAKEET_TEST_FILES.iter().enumerate() {
        // The last file is truncated; the rest are fine (so the layout is ready
        // and the idempotent path runs).
        let bytes = if i + 1 == PARAKEET_TEST_FILES.len() {
            &body.as_bytes()[..body.len() - 1]
        } else {
            body.as_bytes()
        };
        std::fs::write(dest.join(name), bytes).unwrap();
        pins.push((name, body.len() as u64, sha256_of(body.as_bytes())));
    }
    let pins: Vec<(&str, u64, &str)> = pins
        .iter()
        .map(|(name, size, hash)| (*name, *size, hash.as_str()))
        .collect();
    let required: Vec<&str> = PARAKEET_TEST_FILES.iter().map(|(name, _)| *name).collect();

    let err = suramak_core::download::download_files(
        &dest,
        &pins,
        |name| panic!("must not download {name}"),
        &required,
    )
    .expect_err("a truncated file must fail the size check");
    assert!(
        err.to_string().contains("expected") && err.to_string().contains("bytes"),
        "unexpected error: {err}"
    );
}

#[test]
fn download_files_requires_the_whole_layout() {
    // Only one file is pinned; the layout check demands two → the download
    // succeeds but the install is refused with the missing name.
    let body = b"only-one".to_vec();
    let addr = spawn_multi_file_server(vec![("/one.bin".to_string(), body.clone())], 4);
    let hash = sha256_of(&body);

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("parakeet");
    let err = suramak_core::download::download_files(
        &dest,
        &[("one.bin", body.len() as u64, hash.as_str())],
        |name| format!("http://{addr}/{name}"),
        &["one.bin", "two.bin"],
    )
    .expect_err("an incomplete layout must fail");
    assert!(
        err.to_string().contains("required files are missing")
            && err.to_string().contains("two.bin"),
        "unexpected error: {err}"
    );
}
