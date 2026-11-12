//! M22B — the voice-note offer and the paged transcript notification.
//!
//! This module is **pure**: it takes a shared intent's facts, a transcript and
//! its diarization, and returns strings. No JNI, no Android types, no egui.
//! Kotlin paints the notification from [`TranscriptNotification`]; the
//! micro-sheet draws [`OfferPlan`] through the shared compact Deck. Everything
//! here is host-tested, which is the only way the paging and copy rules can be
//! asserted at all (the emulator cannot read a notification's body).
//!
//! ## The rules this file encodes
//!
//! * **The share intent is the only ingest.** There is no media-folder scan and
//!   no notification listener anywhere in this flow; the sender's name exists
//!   only when the intent carried one ([`sender_from_intent`]).
//! * **Honest progress.** The engine is not streaming, so the live
//!   notification says `Transcribing… 62%` and nothing else. Whole-transcript
//!   text appears only after the queue finished and the row was stored.
//! * **Paging is deterministic.** A page holds at most [`PAGE_WORDS`] words;
//!   `Copy` always carries the whole cleaned transcript, never the page.
//! * **Speaker prefixes only when diarization ran.** The labels come from the
//!   stored run (`Speaker 1`, or the owner's manual name).
//! * **Failure names the queue's reason.** There is no generic "something went
//!   wrong" when the queue gave a sentence.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;
use suramak_core::diarization::Diarization;
use suramak_core::textrules::{self, CleanOptions, TextRules};
use suramak_core::transcript::Transcript;
use suramak_ui::deck::quote_card::{self, QuoteCard};
use suramak_ui::deck::theme::AppTheme;

/// A notification page's word budget. One BigText body is roughly this much
/// before an OEM clamps it; the model keeps every page inside it so a long
/// transcript can be read page by page instead of truncated.
pub const PAGE_WORDS: usize = 90;

// ---------------------------------------------------------------------------
// The language pill
// ---------------------------------------------------------------------------

/// The two languages the share sheet offers — one definition, shared with the
/// compact micro-sheet (`suramak_ui::deck::compact::voice_note`). The
/// product's transcription is EN/ID-first; any other language stays on the
/// app's Auto setting, which is exactly what an unpicked pill means.
pub use suramak_ui::deck::compact::voice_note::Pill;

/// What the sheet should preselect and whether the job may start by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferPlan {
    /// The preselected pill. `None` means the sheet starts on Auto.
    pub language: Option<Pill>,
    /// True only when the language is unambiguous **and** a verified model is
    /// installed. The sheet then auto-dismisses in
    /// [`AUTO_DISMISS_SECONDS`] and starts the job.
    pub auto_start: bool,
    /// The honest line under the pill when the job cannot start by itself.
    pub reason: String,
}

/// The intent→pill rule.
///
/// An explicit tag on the intent wins when it is one we understand. An
/// explicit tag we do **not** understand leaves the offer on Auto — falling
/// back to the app's configured language would silently transcribe a French
/// note as Indonesian. With no tag, a fixed app language is unambiguous; the
/// app's `auto` is not.
pub fn offer_plan(
    explicit_language: Option<&str>,
    configured_language: &str,
    model: Option<&str>,
) -> OfferPlan {
    let explicit = explicit_language
        .map(str::trim)
        .filter(|tag| !tag.is_empty());
    let language = match explicit {
        Some(tag) => Pill::from_tag(tag),
        None => Pill::from_tag(configured_language),
    };
    let model_ok = model.is_some_and(|id| !id.trim().is_empty());
    let auto_start = language.is_some() && model_ok;
    let reason = if !model_ok {
        "No speech model is installed on this phone yet.".to_string()
    } else if language.is_none() {
        "Pick a language, or transcribe with auto-detect.".to_string()
    } else {
        String::new()
    };
    OfferPlan {
        language,
        auto_start,
        reason,
    }
}

/// The only sender source in the flow: the sharing intent's own title extra,
/// trimmed, empty rejected. Never a contact lookup, never a media filename.
pub fn sender_from_intent(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Progress and failure copy
// ---------------------------------------------------------------------------

/// The live notification's one line. Deliberately not a partial transcript:
/// whisper decodes in 30-second windows and any "growing text" would be a
/// retroactive rewrite pretending to be a stream.
pub fn progress_text(percent: u8) -> String {
    format!("Transcribing… {}%", percent.min(100))
}

/// A failure exactly as the queue reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailureNotice {
    pub title: String,
    pub body: String,
}

pub fn failure_notice(reason: &str) -> FailureNotice {
    let body = reason.trim();
    FailureNotice {
        title: "Transcription failed".to_string(),
        body: if body.is_empty() {
            "The file could not be transcribed.".to_string()
        } else {
            body.to_string()
        },
    }
}

// ---------------------------------------------------------------------------
// The paged transcript notification
// ---------------------------------------------------------------------------

/// The actions a notification carries. Kotlin maps each to a PendingIntent; a
/// name absent from the list is not rendered, so a failure cannot show `Copy`
/// over text that does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Copy,
    NextPage,
    OpenEditor,
    ShareQuote,
    Retry,
    Cancel,
}

/// Everything the completed notification paints. `text` is the current page;
/// `copy_text` is always the whole cleaned transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TranscriptNotification {
    /// `Transcript ready · 1,240 words` (or `Transcript ready` for no words).
    pub title: String,
    /// The sender's name — present only when the intent provided it.
    pub subtext: Option<String>,
    /// The page being shown, 0-based.
    pub page: usize,
    pub page_count: usize,
    /// The current page's text, with speaker prefixes when diarization ran.
    pub text: String,
    /// The whole cleaned transcript, with speaker prefixes — the Copy payload.
    pub copy_text: String,
    /// Body words, excluding the speaker prefixes.
    pub word_count: usize,
    /// Whether any speaker label was applied.
    pub speakers: bool,
    pub actions: Vec<Action>,
}

impl TranscriptNotification {
    /// The summary line the notification's BigText style shows above the body:
    /// `Page 3 of 56`, or `Copied · Page 3 of 56` after the Copy action.
    pub fn summary(&self, copied: bool) -> String {
        let page = format!("Page {} of {}", self.page + 1, self.page_count);
        if copied {
            format!("Copied · {page}")
        } else {
            page
        }
    }
}

/// One line of the cleaned transcript: the speaker's label (when known) and
/// the segment's text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    speaker: Option<String>,
    text: String,
}

impl Line {
    fn body_words(&self) -> usize {
        self.text.split_whitespace().count()
    }

    fn rendered(&self) -> String {
        match &self.speaker {
            Some(label) => format!("{label}: {}", self.text),
            None => self.text.clone(),
        }
    }
}

/// Joins the stored segments, applies the typographic clean (whitespace only —
/// the stored text and its dictionary are untouched) and prefixes the speaker
/// label when the row has one.
fn cleaned_lines(transcript: &Transcript, diarization: Option<&Diarization>) -> Vec<Line> {
    let preview = textrules::preview(
        transcript,
        &TextRules::default(),
        &CleanOptions {
            collapse_whitespace: true,
            ..CleanOptions::default()
        },
    );
    let mut cleaned: BTreeMap<u32, String> = BTreeMap::new();
    for row in &preview.rows {
        cleaned.insert(row.idx, row.after.clone());
    }
    let labels = diarization
        .map(|run| run.segment_speakers(&transcript.segments))
        .unwrap_or_default();
    let mut lines = Vec::new();
    for (position, segment) in transcript.segments.iter().enumerate() {
        let text = cleaned
            .get(&segment.idx)
            .cloned()
            .unwrap_or_else(|| segment.text.clone());
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let speaker = labels
            .get(position)
            .copied()
            .flatten()
            .and_then(|speaker| diarization.and_then(|run| run.label(speaker.primary)))
            .map(str::to_string);
        lines.push(Line {
            speaker,
            text: text.to_string(),
        });
    }
    lines
}

/// Deterministic pagination: each page holds at most [`PAGE_WORDS`] body
/// words; a single long segment is split at a word boundary and its speaker
/// prefix stays on the first chunk only. Empty input still yields one (empty)
/// page so the notification always has a body.
fn paginate(lines: &[Line]) -> Vec<String> {
    let mut pages: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut words_on_page = 0usize;

    let flush = |pages: &mut Vec<String>, current: &mut String, words: &mut usize| {
        pages.push(std::mem::take(current));
        *words = 0;
    };

    for line in lines {
        let body: Vec<&str> = line.text.split_whitespace().collect();
        if body.is_empty() {
            continue;
        }
        let mut start = 0usize;
        while start < body.len() {
            if words_on_page >= PAGE_WORDS {
                flush(&mut pages, &mut current, &mut words_on_page);
            }
            let room = PAGE_WORDS - words_on_page;
            let take = room.min(body.len() - start);
            let chunk = body[start..start + take].join(" ");
            let rendered = if start == 0 {
                match &line.speaker {
                    Some(label) => format!("{label}: {chunk}"),
                    None => chunk,
                }
            } else {
                chunk
            };
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(&rendered);
            words_on_page += take;
            start += take;
            if words_on_page >= PAGE_WORDS {
                flush(&mut pages, &mut current, &mut words_on_page);
            }
        }
    }
    if !current.is_empty() {
        pages.push(current);
    }
    if pages.is_empty() {
        pages.push(String::new());
    }
    pages
}

/// `1,240`-style grouping, without a formatting crate.
fn thousands(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Builds the completed notification. `page` is normalised into the transcript
/// (`page % page_count`), so the Next-page action can wrap without a bounds
/// check on the Kotlin side.
pub fn build_transcript_notification(
    transcript: &Transcript,
    diarization: Option<&Diarization>,
    sender: Option<&str>,
    page: usize,
) -> TranscriptNotification {
    let lines = cleaned_lines(transcript, diarization);
    let word_count: usize = lines.iter().map(Line::body_words).sum();
    let copy_text = lines
        .iter()
        .map(Line::rendered)
        .collect::<Vec<_>>()
        .join("\n");
    let pages = paginate(&lines);
    let page_count = pages.len();
    let page = page % page_count;
    let mut actions = vec![Action::Copy];
    if page_count > 1 {
        actions.push(Action::NextPage);
    }
    actions.push(Action::OpenEditor);
    actions.push(Action::ShareQuote);
    TranscriptNotification {
        title: if word_count == 0 {
            "Transcript ready".to_string()
        } else if word_count == 1 {
            "Transcript ready · 1 word".to_string()
        } else {
            format!("Transcript ready · {} words", thousands(word_count))
        },
        subtext: sender
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string),
        page,
        page_count,
        text: pages[page].clone(),
        copy_text,
        word_count,
        speakers: diarization.is_some() && lines.iter().any(|line| line.speaker.is_some()),
        actions,
    }
}

// ---------------------------------------------------------------------------
// The quote card
// ---------------------------------------------------------------------------

/// A rendered quote card staged under `files/exports/quotes/`. Kotlin hands
/// the one file to `ACTION_SEND` through the existing FileProvider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StagedQuote {
    pub path: String,
    pub name: String,
    pub bytes: u64,
    pub checksum: String,
    pub mime: String,
    pub page: usize,
    pub page_count: usize,
}

/// Renders the page a notification is showing into a PNG quote card.
///
/// The card is the M18.2 shape: the transcript title, the sender (only when it
/// came from the intent), the page's text in Newsreader, and the app footer —
/// all from the Deck's own tokens, rasterised off-screen by
/// [`suramak_ui::deck::snapshot`], never a screenshot of the window.
pub fn stage_quote_card(
    root: &Path,
    transcript: &Transcript,
    diarization: Option<&Diarization>,
    sender: Option<&str>,
    page: usize,
) -> Result<StagedQuote, String> {
    let model = build_transcript_notification(transcript, diarization, sender, page);
    let card = QuoteCard {
        title: &transcript.title,
        attribution: model.subtext.as_deref(),
        body: &model.text,
        theme: AppTheme::Dark,
    };
    let shot = quote_card::render(&card, 540.0);
    let dir = root.join("exports").join("quotes");
    fs::create_dir_all(&dir).map_err(|err| format!("the quote staging folder: {err}"))?;
    let name = format!("{}-page-{}.png", transcript.id, model.page + 1);
    let path = dir.join(&name);
    let partial = path.with_extension("png.part");
    quote_card::write_png(&partial, &shot)
        .map_err(|err| format!("the quote card could not be written: {err}"))?;
    fs::rename(&partial, &path)
        .map_err(|err| format!("the quote card could not be placed: {err}"))?;
    let bytes =
        fs::read(&path).map_err(|err| format!("the quote card could not be read back: {err}"))?;
    Ok(StagedQuote {
        path: path.to_string_lossy().into_owned(),
        name,
        bytes: bytes.len() as u64,
        checksum: crate::export_job::checksum_hex(crate::export_job::fnv1a64(&bytes)),
        mime: "image/png".to_string(),
        page: model.page,
        page_count: model.page_count,
    })
}

/// The staging directory a previous process may have left behind. Pruned at
/// host creation like the export staging: a quote survives only for the share
/// chooser that asked for it.
pub fn prune_quote_cards(root: &Path) -> usize {
    let dir = root.join("exports").join("quotes");
    let Ok(entries) = fs::read_dir(&dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if entry.path().is_file() && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_core::diarization::{Diarization, DiarizationRun, Speaker, SpeakerTurn};
    use suramak_core::transcript::{Segment, Source, Transcript};

    fn transcript(segments: Vec<(u32, &str)>) -> Transcript {
        Transcript::new(
            "Voice note".to_string(),
            Source::File {
                name: "Ptt.opus".to_string(),
            },
            "tiny".to_string(),
            Some("en".to_string()),
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

    fn five_thousand_words() -> Transcript {
        // 5,000 words in 50 segments of 100 words each.
        let segments = (0..50)
            .map(|idx| {
                let text = (0..100)
                    .map(|word| format!("word{word}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                (idx, text)
            })
            .collect::<Vec<_>>();
        let segments: Vec<Segment> = segments
            .into_iter()
            .map(|(idx, text)| Segment {
                idx,
                start_ms: i64::from(idx) * 60_000,
                end_ms: i64::from(idx) * 60_000 + 59_000,
                text,
            })
            .collect();
        Transcript::new(
            "Long note".to_string(),
            Source::File {
                name: "long.opus".to_string(),
            },
            "tiny".to_string(),
            Some("en".to_string()),
            None,
            segments,
        )
    }

    fn diarization(transcript: &Transcript) -> Diarization {
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
        let speakers = vec![Speaker::new(0, "Rina", 0), Speaker::new(1, "Speaker 2", 1)];
        let turns = vec![
            SpeakerTurn::new(0, 0, 2_000, 0),
            SpeakerTurn::new(1, 2_000, 4_000, 1),
        ];
        Diarization {
            run,
            speakers,
            turns,
        }
    }

    #[test]
    fn the_language_pill_maps_intent_tags_and_never_guesses() {
        assert_eq!(Pill::from_tag("id"), Some(Pill::Id));
        assert_eq!(Pill::from_tag("id-ID"), Some(Pill::Id));
        assert_eq!(Pill::from_tag("in"), Some(Pill::Id));
        assert_eq!(Pill::from_tag("EN"), Some(Pill::En));
        assert_eq!(Pill::from_tag("en_US"), Some(Pill::En));
        assert_eq!(Pill::from_tag("fr"), None);
        assert_eq!(Pill::from_tag(""), None);
    }

    #[test]
    fn the_offer_auto_starts_only_with_a_language_and_a_verified_model() {
        let plan = offer_plan(Some("id"), "auto", Some("tiny"));
        assert_eq!(plan.language, Some(Pill::Id));
        assert!(plan.auto_start);
        assert!(plan.reason.is_empty());

        let plan = offer_plan(None, "en", Some("base"));
        assert_eq!(plan.language, Some(Pill::En));
        assert!(plan.auto_start);

        let plan = offer_plan(None, "auto", Some("tiny"));
        assert_eq!(plan.language, None);
        assert!(!plan.auto_start);
        assert!(plan.reason.contains("Pick a language"));

        let plan = offer_plan(Some("id"), "auto", None);
        assert_eq!(plan.language, Some(Pill::Id));
        assert!(!plan.auto_start);
        assert!(plan.reason.contains("No speech model"));

        // An explicit tag we do not understand must not fall back to the
        // configured language: a French note is not an Indonesian one.
        let plan = offer_plan(Some("fr"), "id", Some("tiny"));
        assert_eq!(plan.language, None);
        assert!(!plan.auto_start);
    }

    #[test]
    fn the_sender_exists_only_when_the_intent_provided_one() {
        assert_eq!(sender_from_intent(None), None);
        assert_eq!(sender_from_intent(Some("   ")), None);
        assert_eq!(sender_from_intent(Some(" Rina ")), Some("Rina".to_string()));
        let bare = transcript(vec![(0, "hello")]);
        let model = build_transcript_notification(&bare, None, None, 0);
        assert_eq!(model.subtext, None, "never a guessed sender");
        let named = build_transcript_notification(&bare, None, Some("Rina"), 0);
        assert_eq!(named.subtext.as_deref(), Some("Rina"));
    }

    #[test]
    fn progress_is_the_queue_percent_and_never_a_partial_transcript() {
        assert_eq!(progress_text(62), "Transcribing… 62%");
        assert_eq!(progress_text(0), "Transcribing… 0%");
        assert_eq!(progress_text(140), "Transcribing… 100%");
    }

    #[test]
    fn a_five_thousand_word_transcript_pages_and_the_copy_payload_is_the_whole_text() {
        let long = five_thousand_words();
        let model = build_transcript_notification(&long, None, None, 0);
        assert_eq!(model.word_count, 5_000);
        assert_eq!(
            model.page_count,
            5_000_usize.div_ceil(PAGE_WORDS),
            "page count follows the 90-word budget"
        );
        assert!(model.page_count > 1);
        assert_eq!(model.title, "Transcript ready · 5,000 words");
        assert_eq!(model.actions[0], Action::Copy);
        assert!(model.actions.contains(&Action::NextPage));
        assert_eq!(
            model.copy_text.split_whitespace().count(),
            5_000,
            "Copy carries every word, not the page"
        );
        assert_eq!(model.text.split_whitespace().count(), PAGE_WORDS);

        // Every page is within the budget, and the pages together are the
        // same words in the same order — nothing dropped at a boundary.
        let mut seen = Vec::new();
        for page in 0..model.page_count {
            let one = build_transcript_notification(&long, None, None, page);
            assert_eq!(one.page, page);
            assert!(one.text.split_whitespace().count() <= PAGE_WORDS);
            seen.extend(one.text.split_whitespace().map(str::to_string));
        }
        assert_eq!(seen.len(), 5_000);
        assert_eq!(seen, model.copy_text.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn pages_wrap_and_normalise_the_requested_index() {
        let long = five_thousand_words();
        let first = build_transcript_notification(&long, None, None, 0);
        let wrapped = build_transcript_notification(&long, None, None, first.page_count);
        assert_eq!(wrapped.page, 0);
        assert_eq!(wrapped.text, first.text);
    }

    #[test]
    fn speaker_prefixes_appear_only_when_diarization_ran() {
        let note = transcript(vec![(0, "Halo, apa kabar?"), (1, "Baik, terima kasih.")]);
        let diarized = diarization(&note);
        let model = build_transcript_notification(&note, Some(&diarized), None, 0);
        assert!(model.speakers);
        assert!(model.text.starts_with("Rina: Halo"));
        assert!(model.text.contains("Speaker 2: Baik"));
        assert!(model.copy_text.contains("Rina: Halo, apa kabar?"));

        let plain = build_transcript_notification(&note, None, None, 0);
        assert!(!plain.speakers);
        assert_eq!(plain.text, "Halo, apa kabar?\nBaik, terima kasih.");
    }

    #[test]
    fn the_clean_pass_collapses_whitespace_but_never_rewrites_words() {
        let note = transcript(vec![(0, "  halo   dunia  "), (1, "kedua")]);
        let model = build_transcript_notification(&note, None, None, 0);
        assert_eq!(model.text, "halo dunia\nkedua");
        assert_eq!(model.copy_text, "halo dunia\nkedua");
    }

    #[test]
    fn failure_copy_names_the_queue_reason_and_never_invents_one() {
        let notice = failure_notice("the audio track is empty");
        assert_eq!(notice.title, "Transcription failed");
        assert_eq!(notice.body, "the audio track is empty");
        let fallback = failure_notice("   ");
        assert_eq!(fallback.body, "The file could not be transcribed.");
    }

    #[test]
    fn an_empty_transcript_still_has_one_page_and_no_word_count() {
        let empty = transcript(vec![]);
        let model = build_transcript_notification(&empty, None, None, 3);
        assert_eq!(model.page, 0);
        assert_eq!(model.page_count, 1);
        assert_eq!(model.text, "");
        assert_eq!(model.title, "Transcript ready");
        assert!(!model.actions.contains(&Action::NextPage));
        assert_eq!(model.summary(false), "Page 1 of 1");
        assert_eq!(model.summary(true), "Copied · Page 1 of 1");
    }

    #[test]
    fn a_single_word_is_not_called_words() {
        let one = transcript(vec![(0, "Halo")]);
        let model = build_transcript_notification(&one, None, None, 0);
        assert_eq!(model.title, "Transcript ready · 1 word");
        assert_eq!(model.word_count, 1);
    }

    #[test]
    fn thousands_grouping_is_plain_and_deterministic() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_240), "1,240");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn the_quote_card_stages_a_real_png_and_cleans_up() {
        let root = std::env::temp_dir().join(format!("suramak-quote-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let note = transcript(vec![(0, "Halo, apa kabar? Ini catatan suara.")]);
        let staged = stage_quote_card(&root, &note, None, Some("Rina"), 0).expect("a card");
        assert_eq!(staged.mime, "image/png");
        assert!(staged.bytes > 1_000, "the PNG has real pixels");
        let bytes = fs::read(&staged.path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(staged.page, 0);
        assert_eq!(staged.page_count, 1);
        assert_eq!(prune_quote_cards(&root), 1);
        assert!(!Path::new(&staged.path).exists());
        let _ = fs::remove_dir_all(&root);
    }
}
