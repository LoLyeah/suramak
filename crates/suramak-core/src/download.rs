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
        req = req.header("Range", format!("bytes={existing_bytes}-"));
    }

    let mut resp = req.send()?;
    let status = resp.status();
    // A server either honours the Range (206: body starts at `existing_bytes`)
    // or it doesn't. Both non-206 answers poison the append below: a 200 body
    // appended to the existing bytes corrupts the file (double data), and a
    // stale `.part` that is already complete-sized gets HTTP 416 on every
    // retry, locking the user out of the download forever. So: resume ONLY on a
    // real 206; on 416 drop the stale part (the next attempt starts clean);
    // on 200 restart from byte 0.
    let resuming = existing_bytes > 0 && status.as_u16() == 206;
    if existing_bytes > 0 && !resuming {
        if status.as_u16() == 416 {
            let _ = std::fs::remove_file(&part_path);
            warn!(
                "the server refused the resume range (HTTP 416) — removed the stale partial download; returning the error so the next attempt starts from scratch"
            );
            // RETURN — audit pass 2 (SUR2-NEW-01): without this, execution fell
            // through to opening .part and streaming the 416 error body (an HTML
            // page) into it, failing only later at the sha256 check with a
            // misleading message. The removal already happened above, so a fresh
            // start is guaranteed; this return just reports the real cause.
            return Err(SuramakError::Http(resp.error_for_status().unwrap_err()));
        } else if !status.is_success() {
            return Err(SuramakError::Http(resp.error_for_status().unwrap_err()));
        } else {
            // 200: the server ignored the Range header — the body is the full
            // file, so discard the partial bytes and write from zero.
            debug!(
                "the server ignored the Range header (HTTP 200) — restarting the download from byte 0"
            );
            on_progress(0);
        }
    } else if !status.is_success() && status.as_u16() != 206 {
        return Err(SuramakError::Http(resp.error_for_status().unwrap_err()));
    }

    if let Some(parent) = part_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = if resuming {
        debug!("resuming from byte {existing_bytes}");
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&part_path)?
    } else {
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&part_path)?
    };

    let base = if resuming { existing_bytes } else { 0 };
    let mut written = 0u64;
    let mut buf = [0u8; 65536];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        written += n as u64;
        on_progress(base + written);
    }
    drop(file);

    // Verify checksum over the full file. A mismatch means the bytes on disk
    // are corrupt (or the server misbehaved): keep nothing — a leftover .part
    // would either fail verification again or 416 the next attempt.
    if let Err(err) = verify_sha256(&part_path, expected_sha256) {
        let _ = std::fs::remove_file(&part_path);
        warn!("checksum mismatch — removed the partial download; the next attempt starts from scratch");
        return Err(err);
    }

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

/// Hard ceiling on the total bytes one archive may expand to (SUR-MED-01).
///
/// The official whisper.cpp Core ML encoder packages are on the order of 1–2 GB
/// expanded, so 5 GiB leaves >2× headroom for a legitimate (or re-packed) encoder
/// archive while capping how much disk a hostile one can fill. Enforced across the
/// whole extraction, not per entry: a zip bomb is many modest-looking entries
/// whose *expanded* total is enormous.
pub const MAX_EXTRACTED_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Extracts `zip_path` into `dest_dir` (created/replaced).
///
/// The official encoder packages wrap their contents in a single top-level directory;
/// that directory is stripped, so the caller controls the destination name. Entries
/// outside the top-level directory (e.g. macOS `__MACOSX/` resource-fork junk) are
/// skipped. Paths are sanitized — absolute paths and `..` components are rejected, so a
/// hostile archive cannot escape `dest_dir`.
///
/// Extraction is atomic: files land in a sibling `<dest_dir>.<pid>-<seq>.tmp`
/// directory which replaces `dest_dir` only after every entry was written, and the
/// expanded total is capped at [`MAX_EXTRACTED_BYTES`].
pub fn extract_zip(zip_path: &Path, dest_dir: &Path) -> Result<(), SuramakError> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| SuramakError::Zip(e.to_string()))?;

    let prefix = archive_prefix(&mut archive)?;

    let tmp = temp_sibling(dest_dir);
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp)?;
    }
    std::fs::create_dir_all(&tmp)?;

    match extract_entries(&mut archive, &prefix, &tmp, MAX_EXTRACTED_BYTES) {
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

/// The archive's single top-level directory (the wrapper the extractor strips).
///
/// Every *content* entry must share one first path component. Metadata entries do
/// not count: `__MACOSX/…` resource forks and root-level files (a bare
/// `.DS_Store` or README, i.e. no `/`) say nothing about the layout. The old
/// "first entry wins" rule picked `__MACOSX` for an archive that listed its junk
/// first and then extracted nothing at all (SUR-MED-01).
///
/// An archive whose content genuinely spans several roots is rejected instead of
/// half-extracted: `download_zip`'s destination layout (the package files at the
/// root of `dest_dir`) cannot represent two top-level directories, so keeping one
/// of them would silently misroute the install.
fn archive_prefix<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<String, SuramakError> {
    let mut roots: Vec<String> = Vec::new();
    for name in archive.file_names() {
        if name.starts_with("__MACOSX/") {
            continue; // macOS resource-fork junk, never package content
        }
        let Some((root, _)) = name.split_once('/') else {
            continue; // a root-level file: not part of a package directory
        };
        if root.is_empty() {
            continue;
        }
        if !roots.iter().any(|seen| seen == root) {
            roots.push(root.to_string());
        }
    }
    match roots.len() {
        0 => Err(SuramakError::Zip(
            "archive has no directory entries".to_string(),
        )),
        1 => Ok(roots.remove(0)),
        _ => Err(SuramakError::Zip(format!(
            "archive spans {} top-level directories ({}); expected one model directory",
            roots.len(),
            roots.join(", ")
        ))),
    }
}

/// Writes every entry under `prefix/` into `tmp`, returning `(files_written, entries_skipped)`.
///
/// `budget_bytes` caps the **expanded** total across the whole archive
/// (SUR-MED-01): each entry is read through `Read::take` — the archive's declared
/// sizes are not trusted — and the extraction fails as soon as one entry would
/// push the total past the budget.
fn extract_entries<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    prefix: &str,
    tmp: &Path,
    budget_bytes: u64,
) -> Result<(usize, usize), SuramakError> {
    let mut files = 0usize;
    let mut skipped = 0usize;
    let mut remaining = budget_bytes;
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
        // `take(remaining + 1)`: one byte past the budget is enough to prove the
        // entry wants more than the archive may have, without reading the rest.
        let mut limited = (&mut entry).take(remaining.saturating_add(1));
        let written = std::io::copy(&mut limited, &mut sink)?;
        if written > remaining {
            return Err(SuramakError::Zip(format!(
                "archive expands past the {budget_bytes}-byte extraction limit (at '{name}')"
            )));
        }
        remaining -= written;
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

/// Sibling temp directory used for atomic extraction
/// (`<dest_dir>.<pid>-<seq>.tmp`).
///
/// Unique per call, not per process (SUR-MED-01): two concurrent extractions into
/// the same parent would otherwise share `<name>.<pid>.tmp` and each delete the
/// other's work in progress. The pid keeps different processes apart; the counter
/// keeps threads of this process apart (the same argument as the encoder-hide
/// path's sequence).
fn temp_sibling(dest_dir: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = dest_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("extract");
    let parent = dest_dir.parent().unwrap_or(Path::new("."));
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    parent.join(format!("{name}.{}-{seq}.tmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha256_hex(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Serves ONE hand-crafted response on an ephemeral port and returns its URL.
    fn serve_once(response: Vec<u8>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            // Drain the request head; a single GET with headers is well under
            // one buffer, but read to the terminator to be safe.
            let mut seen = Vec::new();
            let mut buf = [0u8; 1024];
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.extend_from_slice(&buf[..n]);
                if seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = std::io::Write::write_all(&mut stream, &response);
        });
        format!("http://{addr}/model.bin")
    }

    fn http_response(status_line: &str, body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "{status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn ok_200(body: &[u8]) -> Vec<u8> {
        http_response("HTTP/1.1 200 OK", body)
    }

    fn partial_and_dest(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("suramak-dl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (dir.join("model.bin"), dir)
    }

    #[test]
    fn a_server_that_ignores_the_range_header_restarts_from_zero() {
        const FULL: &[u8] = b"0123456789";
        let (dest, dir) = partial_and_dest("restart-200");
        // Half the file already on disk — appending a full 200 body would corrupt it.
        std::fs::write(part_path(&dest), &FULL[..5]).unwrap();

        let url = serve_once(ok_200(FULL));
        download(&url, &dest, &sha256_hex(FULL)).unwrap();

        let got = std::fs::read(&dest).unwrap();
        assert_eq!(
            got, FULL,
            "the file must contain exactly one copy of the body"
        );
        assert!(!part_path(&dest).exists(), "the .part must be renamed away");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_complete_part_is_removed_on_416_not_locked_out_forever() {
        const FULL: &[u8] = b"0123456789";
        let (dest, dir) = partial_and_dest("stale-416");
        std::fs::write(part_path(&dest), FULL).unwrap(); // complete-sized → 416

        let url = serve_once(http_response(
            "HTTP/1.1 416 Requested Range Not Satisfiable",
            b"",
        ));
        let err = download(&url, &dest, &sha256_hex(FULL)).unwrap_err();
        // The error must name the 416 itself — the fall-through defect
        // (SUR2-NEW-01) surfaced later as a *checksum* message instead.
        assert!(
            format!("{err}").contains("416"),
            "the surfaced error should name the refused range, got: {err}"
        );
        assert!(
            !part_path(&dest).exists(),
            "a 416 must not leave the stale .part in place: every later attempt would fail again"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_206_resume_appends_only_the_second_half() {
        const FULL: &[u8] = b"0123456789";
        let (dest, dir) = partial_and_dest("resume-206");
        std::fs::write(part_path(&dest), &FULL[..5]).unwrap();

        let url = serve_once(http_response("HTTP/1.1 206 Partial Content", &FULL[5..]));
        download(&url, &dest, &sha256_hex(FULL)).unwrap();

        let got = std::fs::read(&dest).unwrap();
        assert_eq!(got, FULL, "resume must rebuild the exact original bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- SUR-MED-01: archive extraction limits + prefix detection ------------

    /// An in-memory zip built from `(name, bytes)`; a name ending in `/` is a
    /// directory entry (no data written).
    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write as _;
        use zip::write::SimpleFileOptions;
        use zip::{CompressionMethod, ZipWriter};
        let mut buf = Vec::new();
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
        buf
    }

    fn scratch_zip_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("suramak-zip-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The archive we actually extract is an encoder package whose first listed
    /// entry can be `__MACOSX/` junk. The old "first entry wins" rule picked that
    /// junk as the prefix and extracted nothing; the common-root rule must find
    /// the real package.
    #[test]
    fn a_macos_junk_first_entry_does_not_become_the_extraction_prefix() {
        let dir = scratch_zip_dir("prefix-junk");
        let zip_path = dir.join("pkg.zip");
        std::fs::write(
            &zip_path,
            zip_bytes(&[
                ("__MACOSX/", b""),
                ("__MACOSX/pkg/._model.bin", b"junk"),
                ("pkg/", b""),
                ("pkg/model.bin", b"weights"),
            ]),
        )
        .unwrap();

        let dest = dir.join("pkg");
        extract_zip(&zip_path, &dest).expect("the real package must be found");
        assert_eq!(std::fs::read(dest.join("model.bin")).unwrap(), b"weights");
        assert!(!dest.join("._model.bin").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two content roots cannot be represented by `download_zip`'s layout, so the
    /// extraction is refused instead of silently keeping one of them.
    #[test]
    fn an_archive_with_two_top_level_directories_is_rejected() {
        let dir = scratch_zip_dir("prefix-two");
        let zip_path = dir.join("pkg.zip");
        std::fs::write(
            &zip_path,
            zip_bytes(&[
                ("pkg-a/", b""),
                ("pkg-a/a.bin", b"a"),
                ("pkg-b/b.bin", b"b"),
            ]),
        )
        .unwrap();

        let dest = dir.join("pkg");
        let err = extract_zip(&zip_path, &dest).expect_err("two roots must not half-extract");
        let message = format!("{err}");
        assert!(
            message.contains("top-level directories"),
            "unexpected error: {message}"
        );
        assert!(!dest.exists(), "nothing may be extracted on rejection");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A hostile-shaped archive: 200 directories deep, then one entry that
    /// expands past the budget. A tiny deflated payload (64 KiB of zeros ≈ 80
    /// bytes) proves the cap works without a real bomb.
    #[test]
    fn deep_nesting_extracts_but_an_oversized_expansion_is_refused() {
        let dir = scratch_zip_dir("cap");
        let deep = format!("pkg/{}", (0..200).map(|_| "d/").collect::<String>());
        let deep_name = format!("{deep}leaf.bin");
        let zip_path = dir.join("pkg.zip");
        std::fs::write(
            &zip_path,
            zip_bytes(&[
                ("pkg/", b""),
                (deep_name.as_str(), b"deep"),
                ("pkg/big.bin", &[0u8; 65_536]),
            ]),
        )
        .unwrap();

        // Within a 1 MiB budget the deep path extracts fine…
        let ok_tmp = dir.join("ok");
        std::fs::create_dir_all(&ok_tmp).unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let prefix = archive_prefix(&mut archive).unwrap();
        assert_eq!(prefix, "pkg");
        let (files, skipped) = extract_entries(&mut archive, &prefix, &ok_tmp, 1024 * 1024)
            .expect("deep nesting is legal");
        // The two files land; the wrapper's own `pkg/` entry is the one skip.
        assert_eq!((files, skipped), (2, 1));
        assert!(ok_tmp.join(&deep_name["pkg/".len()..]).is_file());

        // …and the same archive is refused under a 32 KiB budget: the 65 536-byte
        // entry cannot fit, and the extraction stops at the first byte past it.
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let capped_tmp = dir.join("capped");
        std::fs::create_dir_all(&capped_tmp).unwrap();
        let err = extract_entries(&mut archive, &prefix, &capped_tmp, 32 * 1024)
            .expect_err("an entry past the budget must fail the extraction");
        assert!(
            format!("{err}").contains("extraction limit"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Unique temp siblings: two extractions into the same parent must not pick
    /// the same staging directory (the PID-only name collided across threads).
    #[test]
    fn temp_siblings_are_unique_within_one_process() {
        let dest = std::env::temp_dir().join("suramak-tmp-sibling").join("pkg");
        let first = temp_sibling(&dest);
        let second = temp_sibling(&dest);
        assert_ne!(first, second, "two staging dirs must never collide");
        assert_eq!(first.parent(), dest.parent());
        assert_eq!(second.parent(), dest.parent());
        assert!(first
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("pkg."));
    }
}
