use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use crate::SuramakError;

/// Downloads a file from `url` into `dest`, with resume support and sha256 verification.
///
/// If `dest.part` already exists, resumes from its current size via `Range` header.
/// After the download completes, verifies sha256 against `expected_sha256` and renames
/// the `.part` file to `dest`.
pub fn download(url: &str, dest: &Path, expected_sha256: &str) -> Result<(), SuramakError> {
    let part_path = part_path(dest);

    if dest.exists() {
        info!("already exists: {}", dest.display());
        return Ok(());
    }

    let existing_bytes = if part_path.exists() {
        let meta = std::fs::metadata(&part_path)?;
        meta.len()
    } else {
        0
    };

    let client = reqwest::blocking::Client::builder()
        .https_only(false)
        .use_rustls_tls()
        .build()?;

    let mut req = client.get(url);
    if existing_bytes > 0 {
        debug!("resuming from byte {existing_bytes}");
        req = req.header("Range", format!("bytes={existing_bytes}-"));
    }

    let mut resp = req.send()?;
    if !resp.status().is_success() && resp.status().as_u16() != 206 {
        return Err(SuramakError::Http(resp.error_for_status().unwrap_err()));
    }

    if let Some(parent) = part_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&part_path)?;

    let mut buf = [0u8; 65536];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
    }
    drop(file);

    // Verify checksum over the full file.
    verify_sha256(&part_path, expected_sha256)?;

    std::fs::rename(&part_path, dest)?;
    info!("download complete: {}", dest.display());
    Ok(())
}

/// Verifies sha256 of a file against an expected hex string.
pub fn verify_sha256(path: &Path, expected: &str) -> Result<(), SuramakError> {
    let mut hasher = Sha256::new();
    let mut f = std::fs::File::open(path)?;
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(SuramakError::ChecksumMismatch {
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
}

fn part_path(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".part");
    PathBuf::from(p)
}
