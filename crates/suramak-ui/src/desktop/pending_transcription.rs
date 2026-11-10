//! M9e (P1-1): the pending-transcription entry.
//!
//! Quit-mid-recording **Stop & Save** finalizes the recording's wav into the
//! recordings dir and writes this small entry into the data dir; the next
//! launch reads it and auto-runs the transcription (`Runtime::pending_transcription`
//! → `AppState::begin_pending_resume`). The entry is cleared when the resumed
//! transcript is saved (or when the entry's audio turns out to be missing —
//! the resume never fabricates work).
//!
//! Format: one plain-text record (no new dependency). Backslash escapes keep
//! the key/value lines unambiguous for any path; the header pins the version.
//!
//! ```text
//! suramak-pending-transcription v1
//! title=Recording 2026-10-14 09:41:02
//! wav=/home/u/.local/share/suramak/recordings/Recording 2026-10-14 09-41-02.wav
//! created=1770000000
//! ```

use std::path::{Path, PathBuf};

use suramak_core::paths;

/// File name inside the data dir.
pub const PENDING_FILE: &str = "pending-transcription.txt";

/// The format's first line; anything else is not ours.
const HEADER: &str = "suramak-pending-transcription v1";

/// One pending transcription: the recording's title, where its wav was saved
/// and when the entry was written (unix seconds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTranscription {
    pub title: String,
    pub wav: PathBuf,
    pub created: u64,
}

impl PendingTranscription {
    /// A fresh entry for `title` + `wav`, stamped now.
    pub fn new(title: impl Into<String>, wav: PathBuf) -> Self {
        Self {
            title: title.into(),
            wav,
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }
}

/// The entry's path in the data dir.
pub fn path() -> Result<PathBuf, String> {
    paths::data_dir()
        .map(|dir| dir.join(PENDING_FILE))
        .map_err(|err| err.to_string())
}

/// Writes the entry (create the data dir, write, rename into place — a torn
/// write can never be read as a valid entry).
pub fn write(entry: &PendingTranscription) -> Result<(), String> {
    let path = path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, encode(entry)).map_err(|err| err.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|err| err.to_string())
}

/// Reads the stored entry; `None` when there is none (or it is unreadable —
/// logged, never fatal: a corrupt entry must not block startup).
pub fn read() -> Option<PendingTranscription> {
    let path = match path() {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!("pending transcription: {err}");
            return None;
        }
    };
    read_from(&path)
}

/// Reads + decodes `path` (the testable core of [`read`]).
pub fn read_from(path: &Path) -> Option<PendingTranscription> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::warn!(
                "pending transcription: cannot read {}: {err}",
                path.display()
            );
            return None;
        }
    };
    match decode(&text) {
        Some(entry) => Some(entry),
        None => {
            tracing::warn!(
                "pending transcription: {} is not a readable entry — ignoring it",
                path.display()
            );
            None
        }
    }
}

/// Removes the entry (best effort; logged).
pub fn clear() {
    let path = match path() {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!("pending transcription: {err}");
            return;
        }
    };
    match std::fs::remove_file(&path) {
        Ok(()) => tracing::info!("pending transcription: entry cleared"),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => tracing::warn!(
            "pending transcription: cannot remove {}: {err}",
            path.display()
        ),
    }
}

/// The entry's text form (pure; round-trip tested).
pub fn encode(entry: &PendingTranscription) -> String {
    format!(
        "{HEADER}\ntitle={}\nwav={}\ncreated={}\n",
        escape(&entry.title),
        escape(&entry.wav.to_string_lossy()),
        entry.created
    )
}

/// The reverse of [`encode`]; `None` for anything that is not a well-formed
/// entry of this version.
pub fn decode(text: &str) -> Option<PendingTranscription> {
    let mut lines = text.lines();
    if lines.next()? != HEADER {
        return None;
    }
    let mut title = None;
    let mut wav = None;
    let mut created = None;
    for line in lines {
        let (key, value) = line.split_once('=')?;
        match key {
            "title" => title = Some(unescape(value)?),
            "wav" => wav = Some(PathBuf::from(unescape(value)?)),
            "created" => created = value.parse::<u64>().ok(),
            _ => {} // forward-compatible: unknown keys are ignored
        }
    }
    Some(PendingTranscription {
        title: title?,
        wav: wav?,
        created: created?,
    })
}

/// `\` → `\\`, newline → `\n` — enough for any path or title.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

fn unescape(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next()? {
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_round_trips_through_encode_and_decode() {
        let entry = PendingTranscription {
            title: "Recording 2026-10-14 09:41:02".to_string(),
            wav: PathBuf::from("/home/u/.local/share/suramak/recordings/Recording 09-41-02.wav"),
            created: 1_770_000_000,
        };
        let text = encode(&entry);
        assert!(text.starts_with(HEADER));
        assert_eq!(decode(&text), Some(entry));
    }

    #[test]
    fn escapes_keep_awkward_paths_intact() {
        let entry = PendingTranscription {
            title: "line\nbreak \\ back".to_string(),
            wav: PathBuf::from("/tmp/a=b\\c\n.wav"),
            created: 7,
        };
        let text = encode(&entry);
        assert_eq!(
            text.lines().count(),
            4,
            "no raw newlines leak into the file"
        );
        assert_eq!(decode(&text), Some(entry));
        // A bare `=` in a value is fine (split_once takes the first).
        assert_eq!(unescape("a=b"), Some("a=b".to_string()));
    }

    #[test]
    fn malformed_text_is_rejected_not_guessed() {
        assert_eq!(decode(""), None);
        assert_eq!(decode("not ours\n"), None);
        // Missing fields, bad created, dangling escape — all None.
        assert_eq!(decode(&format!("{HEADER}\ntitle=x\n")), None);
        assert_eq!(decode(&format!("{HEADER}\nwav=/x\ncreated=zz\n")), None);
        assert_eq!(
            decode(&format!("{HEADER}\ntitle=x\nwav=/x\ncreated=1\nweird=1\n")),
            Some(PendingTranscription {
                title: "x".to_string(),
                wav: PathBuf::from("/x"),
                created: 1,
            }),
            "unknown keys are ignored (forward-compatible)"
        );
        assert_eq!(
            decode(&format!("{HEADER}\ntitle=a\\q\nwav=/x\ncreated=1\n")),
            None
        );
    }

    #[test]
    fn a_written_entry_reads_back_and_clears() {
        // The real data dir is process-global; this test only exercises the
        // pure read path against a scratch file.
        let dir = std::env::temp_dir().join(format!("suramak-pending-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(PENDING_FILE);
        let entry = PendingTranscription::new("Recording X", PathBuf::from("/tmp/x.wav"));
        std::fs::write(&path, encode(&entry)).expect("write");
        assert_eq!(read_from(&path), Some(entry));
        std::fs::remove_file(&path).expect("remove");
        assert_eq!(read_from(&path), None, "a missing entry reads as None");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
