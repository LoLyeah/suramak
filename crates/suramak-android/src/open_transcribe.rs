//! M24C — `Open Transcribe`, the pure half of the contract.
//!
//! Another app asks Suramak to transcribe one file and receives the text back
//! through its own `PendingIntent`. This module owns the two decisions that
//! must be the same on every build and are therefore host-tested:
//!
//! * **the request** — an intent's action, `STREAM`, mime, language, sender and
//!   "is there a `RESULT_INTENT`" become one [`Request`], or a named
//!   [`Refusal`] that the caller can be told about;
//! * **the reply** — a stored transcript (or a failure reason) becomes the
//!   exact extra set the result broadcast carries.
//!
//! Kotlin does none of this reasoning: it reads the platform extras, asks the
//! job host to parse them (`op: open_transcribe_request`), and later asks for
//! the payload (`op: open_transcribe_result` / `open_transcribe_failure`).
//! Nothing here touches Android, a database or the queue — the same shape as
//! [`crate::voice_note`], which is why the rules can be asserted at all (an
//! emulator cannot read another app's received extras).
//!
//! ## The rules this file encodes
//!
//! * **One file, one request, one reply.** The contract carries a single
//!   `content://` URI; a batch is not part of it (the caller can send N
//!   requests, they queue behind the one inference slot).
//! * **A path is not a URI.** `file://` and bare paths are refused: the import
//!   reads a file descriptor, and a path that Suramak cannot open under its own
//!   storage sandbox would only fail later, after the caller stopped listening.
//! * **The mime check is the import's own** ([`crate::jobs::is_supported_mime`]) —
//!   the manifest's matrix, the share target and this contract cannot drift.
//! * **An unusable language becomes `auto`, never a guess.** The raw tag is
//!   kept in [`Request::ignored_language`] so the log can name what was
//!   dropped. Guessing "the app's configured language" would transcribe a
//!   French note as Indonesian, which is the M22B rule again.
//! * **A reply needs somewhere to go.** Without a `RESULT_INTENT` the request
//!   is refused *before* a job exists: a transcript nobody can receive is work
//!   the caller did not ask for.
//! * **An empty transcript is not a result.** It is refused as a payload and
//!   reported as a failure instead, so a caller never renders a blank message
//!   as if the file had been silent.

use std::collections::BTreeMap;

use serde::Serialize;
use suramak_core::diarization::Diarization;
use suramak_core::transcript::Transcript;

// ---------------------------------------------------------------------------
// The contract's names
// ---------------------------------------------------------------------------

/// The action a caller sends to ask for a transcription.
pub const ACTION: &str = "app.suramak.action.TRANSCRIBE";
/// The action of the broadcast Suramak sends back through the caller's
/// `PendingIntent`.
pub const ACTION_RESULT: &str = "app.suramak.action.TRANSCRIBE_RESULT";
/// `android.intent.extra.STREAM` — the one `content://` URI to transcribe.
pub const EXTRA_STREAM: &str = "android.intent.extra.STREAM";
/// `app.suramak.extra.LANGUAGE` — `auto`, or a two-letter code.
pub const EXTRA_LANGUAGE: &str = "app.suramak.extra.LANGUAGE";
/// `app.suramak.extra.SENDER` — a display name, shown and never looked up.
pub const EXTRA_SENDER: &str = "app.suramak.extra.SENDER";
/// `app.suramak.extra.RESULT_INTENT` — the caller's reply `PendingIntent`.
pub const EXTRA_RESULT_INTENT: &str = "app.suramak.extra.RESULT_INTENT";
/// The result's transcript id (a uuid string).
pub const EXTRA_TRANSCRIPT_ID: &str = "app.suramak.extra.TRANSCRIPT_ID";
/// The result's plain text, speaker-labelled when diarization ran.
pub const EXTRA_TRANSCRIPT_TEXT: &str = "app.suramak.extra.TRANSCRIPT_TEXT";
/// The result's failure reason — present only on failure.
pub const EXTRA_ERROR: &str = "app.suramak.extra.ERROR";

/// The job kind every Open Transcribe request is queued as. It travels through
/// the existing import path (`JobRecord.kind`), which is what makes the reply
/// possible without a second queue or a second transcription path.
pub const KIND: &str = "open_transcribe";

// ---------------------------------------------------------------------------
// The request
// ---------------------------------------------------------------------------

/// The platform facts of one incoming intent, as Kotlin read them.
///
/// `None` means "the extra was absent"; an empty string is treated the same
/// way (Android hands back `""` for a missing string extra on some paths).
#[derive(Debug, Clone, Copy, Default)]
pub struct IntentFacts<'a> {
    pub action: Option<&'a str>,
    pub stream: Option<&'a str>,
    pub mime: Option<&'a str>,
    pub language: Option<&'a str>,
    pub sender: Option<&'a str>,
    /// `intent.getParcelableExtra(RESULT_INTENT) != null`. The object itself
    /// never crosses into Rust.
    pub has_result_intent: bool,
}

/// A validated request — everything the job needs and nothing it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The `content://` URI, trimmed, exactly as the caller sent it.
    pub uri: String,
    /// The mime type the import will be validated against again.
    pub mime: String,
    /// `auto` or a lower-case two-letter code.
    pub language: String,
    /// The intent's sender, trimmed; `None` when absent or blank.
    pub sender: Option<String>,
    /// The raw language tag an unusable value was replaced for (`auto` was
    /// used). Kept for the log; never shown as if it had been honoured.
    pub ignored_language: Option<String>,
}

/// Why a request was refused. Every variant is a sentence the caller (or the
/// app's log) can be given verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The intent is not this contract's action at all.
    ForeignAction(String),
    /// No `android.intent.extra.STREAM`.
    MissingStream,
    /// A URI that is not `content://`.
    NotAContentUri(String),
    /// A mime type the import path would refuse anyway.
    UnsupportedMime(String),
    /// No `app.suramak.extra.RESULT_INTENT`: there is nowhere to reply.
    MissingResultIntent,
}

impl Refusal {
    /// A stable machine name (the log and the docs use it; it is not localised).
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::ForeignAction(_) => "not_open_transcribe",
            Refusal::MissingStream => "missing_stream",
            Refusal::NotAContentUri(_) => "not_a_content_uri",
            Refusal::UnsupportedMime(_) => "unsupported_mime",
            Refusal::MissingResultIntent => "missing_result_intent",
        }
    }

    /// The one honest sentence: what was wrong, in the caller's terms.
    pub fn sentence(&self) -> String {
        match self {
            Refusal::ForeignAction(action) => {
                format!("the intent action '{action}' is not {ACTION}")
            }
            Refusal::MissingStream => {
                format!("the request carries no {EXTRA_STREAM} content:// uri")
            }
            Refusal::NotAContentUri(uri) => format!(
                "'{uri}' is not a content:// uri — Suramak opens a file descriptor, \
                 so a bare path or file:// uri cannot be read"
            ),
            Refusal::UnsupportedMime(mime) => {
                format!("'{mime}' is not an audio or video type this build imports")
            }
            Refusal::MissingResultIntent => format!(
                "the request carries no {EXTRA_RESULT_INTENT}, so there is nowhere \
                 to send the transcript"
            ),
        }
    }
}

/// The language rule, in one place.
///
/// `auto` (or nothing) stays `auto`. A tag whose primary subtag is two ASCII
/// letters is honoured as that lower-case code, and a region suffix is dropped
/// (`id-ID` → `id`, `EN` → `en`) — whisper's selector is the two-letter code.
/// Anything else (a display name, a three-letter code, a sentence) becomes
/// `auto` and the raw tag is returned so the caller can say what was ignored:
/// a tag we do not understand must never silently become the app's configured
/// language.
pub fn normalise_language(raw: Option<&str>) -> (String, Option<String>) {
    let Some(raw) = raw else {
        return ("auto".to_string(), None);
    };
    let tag = raw.trim();
    if tag.is_empty() || tag.eq_ignore_ascii_case("auto") {
        return ("auto".to_string(), None);
    }
    let primary = tag.split(['-', '_']).next().unwrap_or("").trim();
    if primary.len() == 2 && primary.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return (primary.to_ascii_lowercase(), None);
    }
    ("auto".to_string(), Some(tag.to_string()))
}

/// Parses one intent into a request, or refuses it with a named reason.
///
/// The order is the order a caller can fix things in: the action, the URI, the
/// URI's scheme, the type, then the reply route.
pub fn parse(facts: &IntentFacts<'_>) -> Result<Request, Refusal> {
    let action = facts.action.map(str::trim).unwrap_or_default();
    if action != ACTION {
        return Err(Refusal::ForeignAction(action.to_string()));
    }
    let uri = facts.stream.map(str::trim).unwrap_or_default();
    if uri.is_empty() {
        return Err(Refusal::MissingStream);
    }
    if !uri
        .get(.."content://".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("content://"))
    {
        return Err(Refusal::NotAContentUri(uri.to_string()));
    }
    let mime = facts.mime.map(str::trim).unwrap_or_default().to_string();
    if !crate::jobs::is_supported_mime(&mime) {
        return Err(Refusal::UnsupportedMime(mime));
    }
    if !facts.has_result_intent {
        return Err(Refusal::MissingResultIntent);
    }
    let (language, ignored_language) = normalise_language(facts.language);
    Ok(Request {
        uri: uri.to_string(),
        mime,
        language,
        sender: crate::voice_note::sender_from_intent(facts.sender),
        ignored_language,
    })
}

// ---------------------------------------------------------------------------
// The reply
// ---------------------------------------------------------------------------

/// Why a payload could not be built. Every variant means "send a failure
/// reply instead", never "send a half-empty success".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadError {
    /// A success payload without a transcript id names nothing.
    MissingTranscriptId,
    /// An empty transcript is a failure, not a result.
    EmptyText,
    /// A failure payload without a reason is not allowed to invent one here.
    EmptyReason,
}

impl PayloadError {
    pub fn sentence(self) -> &'static str {
        match self {
            PayloadError::MissingTranscriptId => "a success reply needs a transcript id",
            PayloadError::EmptyText => "the transcript is empty — there is no text to reply with",
            PayloadError::EmptyReason => "a failure reply needs a reason",
        }
    }
}

/// The result broadcast's action and extras, ready for Kotlin to put on an
/// `Intent` and hand to `PendingIntent.send`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResultPayload {
    pub action: &'static str,
    /// The extra names are the contract's, not Rust field names: Kotlin copies
    /// every entry verbatim, so a rename here is a contract change.
    pub extras: BTreeMap<&'static str, String>,
}

impl ResultPayload {
    /// A finished transcription. `text` is the cleaned transcript with speaker
    /// prefixes when the run had them ([`text_for`] builds exactly that).
    pub fn success(transcript_id: &str, text: &str) -> Result<Self, PayloadError> {
        let id = transcript_id.trim();
        if id.is_empty() {
            return Err(PayloadError::MissingTranscriptId);
        }
        let text = text.trim();
        if text.is_empty() {
            return Err(PayloadError::EmptyText);
        }
        let mut extras = BTreeMap::new();
        extras.insert(EXTRA_TRANSCRIPT_ID, id.to_string());
        extras.insert(EXTRA_TRANSCRIPT_TEXT, text.to_string());
        Ok(Self {
            action: ACTION_RESULT,
            extras,
        })
    }

    /// A named failure — the queue's own reason, never a generic sentence.
    /// No transcript id: on failure there is no library row to name.
    pub fn failure(reason: &str) -> Result<Self, PayloadError> {
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(PayloadError::EmptyReason);
        }
        let mut extras = BTreeMap::new();
        extras.insert(EXTRA_ERROR, reason.to_string());
        Ok(Self {
            action: ACTION_RESULT,
            extras,
        })
    }

    /// The whole text the reply carries (for the log line, not for Kotlin).
    pub fn transcript_text(&self) -> Option<&str> {
        self.extras.get(EXTRA_TRANSCRIPT_TEXT).map(String::as_str)
    }

    /// The failure sentence, when this is a failure payload.
    pub fn error(&self) -> Option<&str> {
        self.extras.get(EXTRA_ERROR).map(String::as_str)
    }
}

/// The reply's text: the same cleaned, speaker-prefixed body the transcript
/// notification's Copy action carries ([`crate::voice_note`]), so the caller
/// and the user read the same words. One rendering rule, not two.
pub fn text_for(transcript: &Transcript, diarization: Option<&Diarization>) -> String {
    crate::voice_note::build_transcript_notification(transcript, diarization, None, 0).copy_text
}

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_core::diarization::{DiarizationRun, Speaker, SpeakerTurn};
    use suramak_core::transcript::{Segment, Source};

    fn facts<'a>(stream: Option<&'a str>, mime: Option<&'a str>) -> IntentFacts<'a> {
        IntentFacts {
            action: Some(ACTION),
            stream,
            mime,
            language: None,
            sender: None,
            has_result_intent: true,
        }
    }

    fn note(segments: Vec<(u32, &str)>) -> Transcript {
        Transcript::new(
            "Interview".to_string(),
            Source::File {
                name: "clip.ogg".to_string(),
            },
            "tiny".to_string(),
            Some("id".to_string()),
            None,
            segments
                .into_iter()
                .map(|(idx, text)| Segment {
                    idx,
                    start_ms: i64::from(idx) * 2_000,
                    end_ms: i64::from(idx) * 2_000 + 1_900,
                    text: text.to_string(),
                })
                .collect(),
        )
    }

    fn two_speakers(transcript: &Transcript) -> Diarization {
        let run = DiarizationRun::new(
            transcript.id,
            1,
            "pyannote-segmentation-3.0.onnx",
            "seg",
            "campplus.onnx",
            "emb",
            2,
            0.5,
            10_000,
        );
        Diarization {
            run,
            speakers: vec![Speaker::new(0, "Rina", 0), Speaker::new(1, "Speaker 2", 1)],
            turns: vec![
                SpeakerTurn::new(0, 0, 2_000, 0),
                SpeakerTurn::new(1, 2_000, 4_000, 1),
            ],
        }
    }

    #[test]
    fn a_well_formed_request_becomes_the_contract_request() {
        let parsed = parse(&IntentFacts {
            action: Some(ACTION),
            stream: Some(" content://com.example.provider/audio/7 "),
            mime: Some("audio/ogg; codecs=opus"),
            language: Some("EN"),
            sender: Some("  Rina  "),
            has_result_intent: true,
        })
        .expect("a valid request");
        assert_eq!(parsed.uri, "content://com.example.provider/audio/7");
        assert_eq!(parsed.mime, "audio/ogg; codecs=opus");
        assert_eq!(parsed.language, "en");
        assert_eq!(parsed.sender.as_deref(), Some("Rina"));
        assert_eq!(parsed.ignored_language, None);
    }

    #[test]
    fn a_request_without_a_stream_is_refused_by_name() {
        let refusal = parse(&facts(None, Some("audio/ogg"))).expect_err("no uri");
        assert_eq!(refusal.code(), "missing_stream");
        assert!(refusal.sentence().contains(EXTRA_STREAM));

        let refusal = parse(&facts(Some("   "), Some("audio/ogg"))).expect_err("blank uri");
        assert_eq!(refusal.code(), "missing_stream");
    }

    #[test]
    fn a_file_path_is_not_a_content_uri() {
        let refusal = parse(&facts(Some("file:///sdcard/note.opus"), Some("audio/ogg")))
            .expect_err("file uri");
        assert_eq!(refusal.code(), "not_a_content_uri");
        assert!(refusal.sentence().contains("file:///sdcard/note.opus"));

        let refusal =
            parse(&facts(Some("/sdcard/note.opus"), Some("audio/ogg"))).expect_err("bare path");
        assert_eq!(refusal.code(), "not_a_content_uri");
    }

    #[test]
    fn an_unsupported_mime_is_refused_with_the_type_the_caller_sent() {
        for mime in ["text/plain", "image/png", "application/octet-stream", ""] {
            let refusal = parse(&facts(Some("content://x/1"), Some(mime))).expect_err(mime);
            assert_eq!(refusal.code(), "unsupported_mime", "{mime}");
        }
        let refusal = parse(&facts(Some("content://x/1"), None)).expect_err("no type");
        assert_eq!(refusal.code(), "unsupported_mime");
        assert!(
            refusal.sentence().contains("''"),
            "the absent type is named empty"
        );
    }

    #[test]
    fn the_share_matrix_is_the_contract_matrix() {
        for mime in [
            "audio/ogg",
            "audio/mpeg",
            "audio/mp4",
            "audio/webm",
            "video/mp4",
            "video/quicktime",
            "application/ogg",
        ] {
            let parsed = parse(&facts(Some("content://x/1"), Some(mime))).expect(mime);
            assert_eq!(parsed.mime, mime);
        }
    }

    #[test]
    fn a_request_without_a_result_intent_is_refused_before_a_job_exists() {
        let mut incoming = facts(Some("content://x/1"), Some("audio/ogg"));
        incoming.has_result_intent = false;
        let refusal = parse(&incoming).expect_err("no reply route");
        assert_eq!(refusal.code(), "missing_result_intent");
        assert!(refusal.sentence().contains(EXTRA_RESULT_INTENT));
    }

    #[test]
    fn a_foreign_action_is_refused() {
        let mut incoming = facts(Some("content://x/1"), Some("audio/ogg"));
        incoming.action = Some("android.intent.action.SEND");
        let refusal = parse(&incoming).expect_err("not ours");
        assert_eq!(refusal.code(), "not_open_transcribe");
        assert!(refusal.sentence().contains("android.intent.action.SEND"));
    }

    #[test]
    fn an_unknown_language_falls_back_to_auto_and_is_named() {
        assert_eq!(normalise_language(None), ("auto".to_string(), None));
        assert_eq!(normalise_language(Some("")), ("auto".to_string(), None));
        assert_eq!(
            normalise_language(Some("  AUTO ")),
            ("auto".to_string(), None)
        );
        assert_eq!(normalise_language(Some("id")), ("id".to_string(), None));
        assert_eq!(normalise_language(Some("EN")), ("en".to_string(), None));
        assert_eq!(normalise_language(Some("id-ID")), ("id".to_string(), None));
        assert_eq!(normalise_language(Some("en_US")), ("en".to_string(), None));
        assert_eq!(
            normalise_language(Some("English")),
            ("auto".to_string(), Some("English".to_string()))
        );
        assert_eq!(
            normalise_language(Some("fil")),
            ("auto".to_string(), Some("fil".to_string())),
            "a three-letter code is not whisper's selector"
        );
        assert_eq!(
            normalise_language(Some("banana")),
            ("auto".to_string(), Some("banana".to_string()))
        );

        let parsed = parse(&IntentFacts {
            language: Some("banana"),
            ..facts(Some("content://x/1"), Some("audio/ogg"))
        })
        .expect("still a valid request");
        assert_eq!(parsed.language, "auto");
        assert_eq!(parsed.ignored_language.as_deref(), Some("banana"));
    }

    #[test]
    fn a_blank_sender_is_absent_and_never_a_guess() {
        let parsed = parse(&IntentFacts {
            sender: Some("   "),
            ..facts(Some("content://x/1"), Some("audio/ogg"))
        })
        .expect("a request");
        assert_eq!(parsed.sender, None);
    }

    #[test]
    fn a_success_payload_carries_the_id_and_the_text() {
        let payload = ResultPayload::success("8d1f3c1e-0000-4000-8000-00000000abcd", "Halo dunia")
            .expect("a payload");
        assert_eq!(payload.action, ACTION_RESULT);
        assert_eq!(
            payload.extras.get(EXTRA_TRANSCRIPT_ID).map(String::as_str),
            Some("8d1f3c1e-0000-4000-8000-00000000abcd")
        );
        assert_eq!(payload.transcript_text(), Some("Halo dunia"));
        assert_eq!(payload.error(), None);
    }

    #[test]
    fn the_payload_carries_speaker_prefixes_only_when_diarization_ran() {
        let plain = note(vec![(0, "Halo, apa kabar?"), (1, "Baik, terima kasih.")]);
        let text = text_for(&plain, None);
        assert_eq!(text, "Halo, apa kabar?\nBaik, terima kasih.");
        assert!(!text.contains("Rina"));

        let diarized = two_speakers(&plain);
        let text = text_for(&plain, Some(&diarized));
        assert!(text.contains("Rina: Halo, apa kabar?"), "{text}");
        assert!(text.contains("Speaker 2: Baik, terima kasih."), "{text}");

        let payload = ResultPayload::success("8d1f3c1e-0000-4000-8000-00000000abcd", &text)
            .expect("a payload");
        assert!(payload.transcript_text().unwrap().starts_with("Rina: "));
    }

    #[test]
    fn an_empty_transcript_is_refused_not_sent_as_a_blank_result() {
        let empty = note(vec![]);
        assert_eq!(text_for(&empty, None), "");
        let error = ResultPayload::success("8d1f3c1e-0000-4000-8000-00000000abcd", "")
            .expect_err("no text");
        assert_eq!(error, PayloadError::EmptyText);
        let error = ResultPayload::success("8d1f3c1e-0000-4000-8000-00000000abcd", "   \n ")
            .expect_err("whitespace is empty");
        assert_eq!(error, PayloadError::EmptyText);
    }

    #[test]
    fn a_success_payload_needs_an_id() {
        assert_eq!(
            ResultPayload::success("  ", "Halo").expect_err("no id"),
            PayloadError::MissingTranscriptId
        );
    }

    #[test]
    fn a_failure_payload_carries_the_reason_and_never_text() {
        let payload = ResultPayload::failure(" the audio track is empty ").expect("a payload");
        assert_eq!(payload.action, ACTION_RESULT);
        assert_eq!(payload.error(), Some("the audio track is empty"));
        assert_eq!(payload.transcript_text(), None);
        assert!(!payload.extras.contains_key(EXTRA_TRANSCRIPT_ID));
        assert_eq!(
            ResultPayload::failure("   ").expect_err("no reason"),
            PayloadError::EmptyReason
        );
    }

    #[test]
    fn the_extras_serialise_under_the_contract_names() {
        let payload = ResultPayload::success("id-1", "Halo").expect("a payload");
        let json = serde_json::to_value(&payload).expect("serialisable");
        assert_eq!(json["action"], ACTION_RESULT);
        assert_eq!(json["extras"][EXTRA_TRANSCRIPT_TEXT], "Halo");
        assert_eq!(json["extras"][EXTRA_TRANSCRIPT_ID], "id-1");
    }

    #[test]
    fn the_contract_names_are_the_documented_ones() {
        assert_eq!(ACTION, "app.suramak.action.TRANSCRIBE");
        assert_eq!(ACTION_RESULT, "app.suramak.action.TRANSCRIBE_RESULT");
        assert_eq!(EXTRA_STREAM, "android.intent.extra.STREAM");
        assert_eq!(EXTRA_LANGUAGE, "app.suramak.extra.LANGUAGE");
        assert_eq!(EXTRA_SENDER, "app.suramak.extra.SENDER");
        assert_eq!(EXTRA_RESULT_INTENT, "app.suramak.extra.RESULT_INTENT");
        assert_eq!(EXTRA_TRANSCRIPT_ID, "app.suramak.extra.TRANSCRIPT_ID");
        assert_eq!(EXTRA_TRANSCRIPT_TEXT, "app.suramak.extra.TRANSCRIPT_TEXT");
        assert_eq!(EXTRA_ERROR, "app.suramak.extra.ERROR");
        assert_eq!(KIND, "open_transcribe");
    }
}
