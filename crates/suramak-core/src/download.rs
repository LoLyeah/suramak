use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::SuramakError;

/// Downloads a file from `url` into `dest`, with resume support and sha256 verification.
///
/// If `dest.part` already exists, resumes from its current size via `Range` header.
/// After the download completes, verifies sha256 against `expected_sha256` and renames
/// the `.part` file to `dest`.
pub fn download(url: &str, dest: &Path, expected_sha256: &str) -> Result<(), SuramakError> {
    download_with_progress(url, dest, expected_sha256, |_| {})
}

/// [`download`] with a byte-progress callback (M3: the in-app model manager shows a
/// progress bar). `on_progress` receives the number of bytes on disk so far —
/// `existing .part` size + bytes written this run — and fires once per read chunk
/// plus once up front (so a resumed download reports its starting offset).
pub fn download_with_progress(
    url: &str,
    dest: &Path,
    expected_sha256: &str,
    mut on_progress: impl FnMut(u64),
) -> Result<(), SuramakError> {
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
    on_progress(existing_bytes);

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

    let mut written = 0u64;
    let mut buf = [0u8; 65536];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        written += n as u64;
        on_progress(existing_bytes + written);
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

/// Downloads a `.zip` artifact and extracts it into `models_dir/<dest_dir_name>`.
///
/// 1. short-circuits when the destination already contains every file in `required_files`;
/// 2. downloads the archive to `models_dir/<zip_file_name>` (resume + sha256 verify);
/// 3. extracts it atomically (see [`extract_zip`]);
/// 4. re-checks the extracted layout, then removes the archive.
///
/// Returns the extracted directory. The destination name is caller-controlled because
/// whisper.cpp's Core ML lookup is name-sensitive: it must be exactly
/// `<model file stem without -qX_X>-encoder.mlmodelc` (see `models::encoder_dir_name`).
pub fn download_zip(
    url: &str,
    models_dir: &Path,
    zip_file_name: &str,
    dest_dir_name: &str,
    expected_sha256: &str,
    required_files: &[&str],
) -> Result<PathBuf, SuramakError> {
    let dest = models_dir.join(dest_dir_name);
    if dir_ready(&dest, required_files) {
        info!("already extracted: {}", dest.display());
        return Ok(dest);
    }

    let zip_path = models_dir.join(zip_file_name);
    download(url, &zip_path, expected_sha256)?;
    extract_zip(&zip_path, &dest)?;

    if !dir_ready(&dest, required_files) {
        return Err(SuramakError::Zip(format!(
            "extracted {} but required files are missing ({})",
            dest.display(),
            required_files.join(", ")
        )));
    }
    // The archive is not needed once extracted; keep the models dir lean (the turbo
    // encoder alone is >1 GB).
    if let Err(err) = std::fs::remove_file(&zip_path) {
        warn!(
            "could not remove the downloaded archive {}: {err}",
            zip_path.display()
        );
    }
    Ok(dest)
}

/// Whether `dir` is a directory containing every file in `required`.
pub fn dir_ready(dir: &Path, required: &[&str]) -> bool {
    dir.is_dir() && required.iter().all(|f| dir.join(f).is_file())
}

/// Downloads a set of pinned files into `dest_dir` (M10A: the Parakeet model is a
/// directory of artifacts, not one archive).
///
/// 1. short-circuits when `dest_dir` already contains every file in `required`
///    (each present file is size- and sha256-verified — a corrupted install fails
///    loudly instead of being trusted);
/// 2. downloads each missing file with resume + sha256 verification
///    ([`download`]) from `url_for(name)`;
/// 3. re-checks the layout ([`dir_ready`]) before claiming success.
pub fn download_files(
    dest_dir: &Path,
    files: &[(&str, u64, &str)],
    url_for: impl Fn(&str) -> String,
    required: &[&str],
) -> Result<(), SuramakError> {
    if dir_ready(dest_dir, required) {
        for (name, size, sha256) in files {
            let path = dest_dir.join(name);
            check_size(&path, *size)?;
            verify_sha256(&path, sha256)?;
        }
        info!("already installed: {}", dest_dir.display());
        return Ok(());
    }
    std::fs::create_dir_all(dest_dir)?;
    for (name, size, sha256) in files {
        let dest = dest_dir.join(name);
        if dest.exists() {
            // A file from an earlier interrupted run: verify before trusting it.
            check_size(&dest, *size)?;
            verify_sha256(&dest, sha256)?;
            continue;
        }
        let url = url_for(name);
        debug!("downloading {name} from {url}");
        download(&url, &dest, sha256)?;
        // Re-verify the final file (post-rename) so the claim is airtight.
        check_size(&dest, *size)?;
        verify_sha256(&dest, sha256)?;
    }
    if !dir_ready(dest_dir, required) {
        let missing: Vec<&str> = required
            .iter()
            .filter(|f| !dest_dir.join(f).is_file())
            .copied()
            .collect();
        return Err(SuramakError::Download(format!(
            "downloaded into {} but required files are missing ({})",
            dest_dir.display(),
            missing.join(", ")
        )));
    }
    Ok(())
}

/// The pinned size check — a cheap pre-filter ahead of the sha256 (and the clear
/// error for a truncated file).
fn check_size(path: &Path, expected: u64) -> Result<(), SuramakError> {
    let actual = std::fs::metadata(path)?.len();
    if actual != expected {
        return Err(SuramakError::Download(format!(
            "{}: expected {expected} bytes, found {actual} — delete it and retry",
            path.display()
        )));
    }
    Ok(())
}

/// Extracts `zip_path` into `dest_dir` (created/replaced).
///
/// The official encoder packages wrap their contents in a single top-level directory;
/// that directory is stripped, so the caller controls the destination name. Entries
/// outside the top-level directory (e.g. macOS `__MACOSX/` resource-fork junk) are
/// skipped. Paths are sanitized — absolute paths and `..` components are rejected, so a
/// hostile archive cannot escape `dest_dir`.
///
/// Extraction is atomic: files land in a sibling `<dest_dir>.<pid>.tmp` directory which
/// replaces `dest_dir` only after every entry was written.
pub fn extract_zip(zip_path: &Path, dest_dir: &Path) -> Result<(), SuramakError> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| SuramakError::Zip(e.to_string()))?;

    // Top-level directory = first path component of the first entry.
    let prefix = archive
        .file_names()
        .next()
        .map(|n| n.split('/').next().unwrap_or("").to_string())
        .unwrap_or_default();
    if prefix.is_empty() {
        return Err(SuramakError::Zip(format!(
            "{}: archive has no entries",
            zip_path.display()
        )));
    }

    let tmp = temp_sibling(dest_dir);
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp)?;
    }
    std::fs::create_dir_all(&tmp)?;

    match extract_entries(&mut archive, &prefix, &tmp) {
        Ok((files, skipped)) => {
            debug!("extracted {files} files (skipped {skipped} foreign entries)");
            if dest_dir.exists() {
                std::fs::remove_dir_all(dest_dir)?;
            }
            std::fs::rename(&tmp, dest_dir)?;
            Ok(())
        }
        Err(err) => {
            // Never leave a partial extraction behind.
            let _ = std::fs::remove_dir_all(&tmp);
            Err(err)
        }
    }
}

/// Writes every entry under `prefix/` into `tmp`, returning `(files_written, entries_skipped)`.
fn extract_entries<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    prefix: &str,
    tmp: &Path,
) -> Result<(usize, usize), SuramakError> {
    let mut files = 0usize;
    let mut skipped = 0usize;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| SuramakError::Zip(e.to_string()))?;
        let name = entry.name().to_string();
        // Strip the archive's own top-level directory; skip anything outside it.
        let Some(rel) = name
            .strip_prefix(prefix)
            .and_then(|r| r.strip_prefix('/'))
            .filter(|r| !r.is_empty())
        else {
            skipped += 1;
            continue;
        };
        let Some(rel) = sanitized_relative(rel) else {
            return Err(SuramakError::Zip(format!("unsafe path in archive: {name}")));
        };
        let out = tmp.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut sink = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut sink)?;
        files += 1;
    }
    if files == 0 {
        return Err(SuramakError::Zip(format!(
            "archive contains no files under '{prefix}/'"
        )));
    }
    Ok((files, skipped))
}

/// Rejects absolute paths and `..` components; returns the safe relative path.
fn sanitized_relative(rel: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for comp in Path::new(rel).components() {
        match comp {
            std::path::Component::Normal(c) => out.push(c),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Sibling temp directory used for atomic extraction (`<dest_dir>.<pid>.tmp`).
fn temp_sibling(dest_dir: &Path) -> PathBuf {
    let name = dest_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("extract");
    let parent = dest_dir.parent().unwrap_or(Path::new("."));
    parent.join(format!("{name}.{}.tmp", std::process::id()))
}
