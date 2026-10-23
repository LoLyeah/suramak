//! The probe's instance-state payload — Kotlin `Bundle` ⇄ Rust `SpikeState`.
//!
//! **Why this exists.** The spike's text lives in [`crate::ui::SpikeState`],
//! which is a process-wide `OnceLock` — so it already survives an Activity
//! recreation in the same process (rotation), because `android_main` entry #2
//! finds the same `SpikeState`. What it does *not* survive is the **process**
//! going away: the owner's v2.3 field run lost a whole session's text when the
//! process was killed at rotation, and any low-memory kill takes the state with
//! it.
//!
//! The platform's answer is the Activity's **instance state**:
//! `onSaveInstanceState` writes into a `Bundle` the system keeps for that
//! Activity record and hands back to `onCreate` — in the same process for a
//! configuration change, and in a **new** process after a kill. `MainActivity`
//! puts one string in that bundle and hands it back over JNI on the next entry
//! (see `platform::nativeRestoreProbe` / `nativeSaveProbe`).
//!
//! **What it carries.** Both text fields the gate's checks type into:
//!
//! * `probe` — check 2's marker (`lifecycle-123`), the one the gate says must be
//!   "still in the probe every time";
//! * `text` — check 1's typed sentence, which is the session's real content and
//!   the thing a native kill actually destroyed in v2.3.
//!
//! **The format.** One string, so a `Bundle` key carries it whole:
//!
//! ```text
//! suramak-probe/1
//! probe=<length of the probe, in UTF-8 bytes>
//! text=<length of the text, in UTF-8 bytes>
//! <probe bytes><text bytes>
//! ```
//!
//! Length prefixes are why there is no escaping: either field may contain
//! newlines, `=`, quotes, emoji — anything — and the decoder never guesses where
//! a field ends. Lengths count *bytes*, because that is what slicing a `str`
//! needs and what a UTF-16 `Bundle` string converts back to losslessly.
//!
//! This module has no dependencies and no JNI, which is what lets the shape be
//! unit-tested on the host; [`crate::platform`] only moves the bytes across the
//! boundary. It is deliberately **not** the product's persistence path (the real
//! app owns SQLite) — it is the spike's own probe, and the workorder says to
//! keep it minimal.

/// First line of every payload. Bumping it is how a future format change
/// refuses old payloads instead of misreading them.
pub const MAGIC: &str = "suramak-probe/1";

/// Refuse absurd payloads before allocating: an Activity's saved state is a few
/// KiB at most, and the spike's fields are a marker and a sentence.
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024;

/// A decoded probe payload: the two text fields, byte-for-byte.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProbePayload {
    /// Check 2's marker field (`SpikeState::probe`).
    pub probe: String,
    /// Check 1's typed sentence (`SpikeState::text`).
    pub text: String,
}

impl ProbePayload {
    pub fn new(probe: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            probe: probe.into(),
            text: text.into(),
        }
    }

    /// `(probe chars, text chars)` — what the log line quotes.
    pub fn char_counts(&self) -> (usize, usize) {
        (self.probe.chars().count(), self.text.chars().count())
    }

    pub fn is_empty(&self) -> bool {
        self.probe.is_empty() && self.text.is_empty()
    }
}

/// Why a payload was refused. Every variant is named in the log line the
/// restore path writes, so a rejected payload never looks like "no payload".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadError {
    /// Not our format at all (empty string, another app's key, a future magic).
    Magic,
    /// A `probe=`/`text=` line is missing or unparsable.
    Header,
    /// The declared lengths and the bytes actually present disagree.
    Truncated,
    /// A field's length lands in the middle of a UTF-8 character.
    Boundary,
    /// Longer than [`MAX_PAYLOAD_BYTES`] (counted on the whole payload).
    TooLarge,
}

impl PayloadError {
    /// The wording a log line uses. Kept here, next to the variants, so the
    /// test can pin the exact words the device log will carry.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Magic => "not a suramak-probe/1 payload",
            Self::Header => "a probe=/text= header line is missing or not a number",
            Self::Truncated => "the declared byte lengths do not match the payload",
            Self::Boundary => "a field length splits a UTF-8 character",
            Self::TooLarge => "the payload is larger than the 16 KiB ceiling",
        }
    }
}

impl std::fmt::Display for PayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Encode both fields as the one-string payload `MainActivity` puts in the
/// `Bundle`.
pub fn encode(payload: &ProbePayload) -> String {
    let mut out =
        String::with_capacity(MAGIC.len() + 48 + payload.probe.len() + payload.text.len());
    out.push_str(MAGIC);
    out.push('\n');
    out.push_str("probe=");
    out.push_str(&payload.probe.len().to_string());
    out.push('\n');
    out.push_str("text=");
    out.push_str(&payload.text.len().to_string());
    out.push('\n');
    out.push_str(&payload.probe);
    out.push_str(&payload.text);
    out
}

/// Decode what `MainActivity` handed back. Strict on purpose: a payload that
/// does not match the format byte-for-byte is refused, not "best effort"
/// repaired — a half-applied restore would look like the save path working.
pub fn decode(payload: &str) -> Result<ProbePayload, PayloadError> {
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(PayloadError::TooLarge);
    }
    let rest = payload
        .strip_prefix(MAGIC)
        .and_then(|rest| rest.strip_prefix('\n'))
        .ok_or(PayloadError::Magic)?;
    let (probe_line, rest) = rest.split_once('\n').ok_or(PayloadError::Header)?;
    let (text_line, body) = rest.split_once('\n').ok_or(PayloadError::Header)?;
    let probe_len = field_len(probe_line, "probe=")?;
    let text_len = field_len(text_line, "text=")?;
    let total = probe_len
        .checked_add(text_len)
        .ok_or(PayloadError::Truncated)?;
    if body.len() != total {
        return Err(PayloadError::Truncated);
    }
    if !body.is_char_boundary(probe_len) {
        return Err(PayloadError::Boundary);
    }
    Ok(ProbePayload {
        probe: body[..probe_len].to_owned(),
        text: body[probe_len..].to_owned(),
    })
}

fn field_len(line: &str, key: &str) -> Result<usize, PayloadError> {
    line.strip_prefix(key)
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or(PayloadError::Header)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shapes the two fields really take on the phone: a short marker, a
    /// typed Indonesian sentence with autocorrect, emoji, quotes, newlines.
    fn fixtures() -> Vec<ProbePayload> {
        vec![
            ProbePayload::default(),
            ProbePayload::new("lifecycle-123", ""),
            ProbePayload::new(
                "",
                "Saya sedang menguji papan ketik Android dengan koreksi otomatis.",
            ),
            ProbePayload::new("lifecycle-123", "line one\nline two\r\nline three\n"),
            ProbePayload::new(
                "marker=1\nprobe=x",
                "bytes=5\ntext=99\nemoji 🎹 quotes \" ' = and a trailing newline\n",
            ),
        ]
    }

    #[test]
    fn a_round_trip_keeps_every_byte_of_both_fields() {
        for payload in fixtures() {
            let encoded = encode(&payload);
            let decoded = decode(&encoded).unwrap_or_else(|err| panic!("{payload:?}: {err}"));
            assert_eq!(decoded, payload, "round trip changed the payload");
        }
    }

    #[test]
    fn the_header_counts_bytes_so_multibyte_text_still_decodes() {
        let payload = ProbePayload::new("🎹", "🎹🎹");
        assert_eq!(payload.char_counts(), (1, 2));
        let encoded = encode(&payload);
        assert!(encoded.contains("probe=4"), "{encoded}");
        assert!(encoded.contains("text=8"), "{encoded}");
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.probe, "🎹");
        assert_eq!(decoded.text, "🎹🎹");
    }

    #[test]
    fn an_empty_payload_is_valid_and_says_so() {
        let encoded = encode(&ProbePayload::default());
        assert_eq!(encoded, "suramak-probe/1\nprobe=0\ntext=0\n");
        let decoded = decode(&encoded).unwrap();
        assert!(decoded.is_empty());
        assert_eq!(decoded.char_counts(), (0, 0));
    }

    #[test]
    fn a_foreign_or_future_payload_is_refused_by_name() {
        assert_eq!(decode("").unwrap_err(), PayloadError::Magic);
        assert_eq!(decode("lifecycle-123").unwrap_err(), PayloadError::Magic);
        assert_eq!(
            decode("suramak-probe/2\nprobe=0\ntext=0\n").unwrap_err(),
            PayloadError::Magic
        );
        // Our magic but no header at all.
        assert_eq!(
            decode("suramak-probe/1\nprobe=0").unwrap_err(),
            PayloadError::Header
        );
        assert_eq!(
            decode("suramak-probe/1\nnonsense\ntext=0\n").unwrap_err(),
            PayloadError::Header
        );
        assert_eq!(
            decode("suramak-probe/1\nprobe=0\nnonsense\n").unwrap_err(),
            PayloadError::Header
        );
        // Declared lengths that do not match the bytes present.
        assert_eq!(
            decode("suramak-probe/1\nprobe=9\ntext=0\nshort").unwrap_err(),
            PayloadError::Truncated
        );
        // A length that lands inside a 4-byte emoji (1 + 3 = the 4 bytes present).
        assert_eq!(
            decode("suramak-probe/1\nprobe=1\ntext=3\n🎹").unwrap_err(),
            PayloadError::Boundary
        );
    }

    #[test]
    fn an_oversized_payload_is_refused_before_it_is_parsed() {
        let huge = "x".repeat(MAX_PAYLOAD_BYTES + 1);
        assert_eq!(decode(&huge).unwrap_err(), PayloadError::TooLarge);
        // Exactly at the ceiling is still parsed, and fails on its own merits.
        let mut at_ceiling = encode(&ProbePayload::new("ok", ""));
        at_ceiling.push_str(&" ".repeat(MAX_PAYLOAD_BYTES - at_ceiling.len()));
        assert_eq!(decode(&at_ceiling).unwrap_err(), PayloadError::Truncated);
    }

    #[test]
    fn every_error_has_a_sentence_for_the_log() {
        for error in [
            PayloadError::Magic,
            PayloadError::Header,
            PayloadError::Truncated,
            PayloadError::Boundary,
            PayloadError::TooLarge,
        ] {
            assert!(!error.as_str().is_empty());
            assert_eq!(error.to_string(), error.as_str());
        }
    }
}
