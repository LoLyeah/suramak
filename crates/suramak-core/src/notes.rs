//! M24B: citation-verified notes — Minutes, Action items and a bilingual
//! EN↔ID brief.
//!
//! One mechanism, three presets: each is a system prompt plus an output
//! contract (Markdown, `[mm:ss]` / `[hh:mm:ss]` citations, no invented
//! speakers, no invented times, keep the source language). Generation reuses
//! the polish client's own `/chat/completions` path — the endpoint and model
//! are the ones the user configured, there is no second HTTP path.
//!
//! The honesty rule is [`check_notes`]: every bracketed time token the model
//! produced is parsed and matched to a real segment start within
//! [`CITATION_TOLERANCE_MS`]. A valid token is rewritten to the matched
//! segment's canonical time. An invalid one is **removed from the text** and
//! counted, with a breakdown naming why (`removed` for no matching audio,
//! `past_end`, `malformed`, `duplicate`). A citation that names audio which
//! does not exist is never shipped as if it were true.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::session::{session_incomplete_refusal, session_timeline, SessionPartInput};
use crate::settings::PolishSettings;
use crate::transcript::Segment;
use crate::SuramakError;

/// A cited time may be this far from a real segment start and still count as
/// that segment's citation (the work order's ±1.5 s).
pub const CITATION_TOLERANCE_MS: i64 = 1_500;

/// The largest segment-text payload one notes request carries (characters).
/// Longer transcripts are split at segment boundaries and merged in order; the
/// outcome records how many parts ran. This is the notes path's own budget —
/// the audio chunk cap ([`crate::polish::AUDIO_CHUNK_CAP_BYTES`]) measures
/// WAV bytes, not prompt text.
pub const NOTES_CHUNK_CHARS: usize = 24_000;

/// The three presets. One shared mechanism: a system prompt and an output
/// contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NotePreset {
    /// Meeting minutes: decisions, discussion, follow-ups.
    Minutes,
    /// Action items with owners as spoken.
    ActionItems,
    /// A bilingual EN↔ID brief (summary in both languages, quotes kept).
    BilingualBrief,
}

impl NotePreset {
    /// The canonical product order (also the store's read order and the
    /// sheet's row order).
    pub const ALL: [NotePreset; 3] = [
        NotePreset::Minutes,
        NotePreset::ActionItems,
        NotePreset::BilingualBrief,
    ];

    /// The stable key written to `notes.preset` and parsed back. Never renamed
    /// without a migration.
    pub fn key(self) -> &'static str {
        match self {
            NotePreset::Minutes => "minutes",
            NotePreset::ActionItems => "action_items",
            NotePreset::BilingualBrief => "bilingual_brief",
        }
    }

    /// The sheet's row label.
    pub fn label(self) -> &'static str {
        match self {
            NotePreset::Minutes => "Minutes",
            NotePreset::ActionItems => "Action items",
            NotePreset::BilingualBrief => "Bilingual brief (EN↔ID)",
        }
    }

    /// The document's H1 (the model is told not to emit one).
    pub fn heading(self) -> &'static str {
        match self {
            NotePreset::Minutes => "Minutes",
            NotePreset::ActionItems => "Action items",
            NotePreset::BilingualBrief => "Brief (EN↔ID)",
        }
    }

    /// Parses a stored key. Unknown text is `None`, never guessed at.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "minutes" => Some(NotePreset::Minutes),
            "action_items" => Some(NotePreset::ActionItems),
            "bilingual_brief" => Some(NotePreset::BilingualBrief),
            _ => None,
        }
    }

    /// The preset's system prompt: the shared citation/honesty contract plus
    /// the preset's own document shape.
    pub fn system_prompt(self) -> String {
        let specific = match self {
            NotePreset::Minutes => MINUTES_PROMPT,
            NotePreset::ActionItems => ACTION_ITEMS_PROMPT,
            NotePreset::BilingualBrief => BILINGUAL_PROMPT,
        };
        format!("{specific}\n\n{SHARED_CONTRACT}")
    }

    /// The path a run of `parts` took, in words the sheet can show.
    pub fn path_label(parts: usize) -> String {
        if parts <= 1 {
            "single request".to_string()
        } else {
            format!("{parts} chunks merged")
        }
    }
}

/// The shared contract every preset's prompt carries.
const SHARED_CONTRACT: &str = "\
You receive JSON: {\"title\":string,\"language\":string|null,\"segments\":[\
{\"id\":int,\"start\":\"mm:ss\" or \"hh:mm:ss\",\"start_ms\":int,\
\"speaker\":string|null,\"text\":string}]}. \
The segments are the transcript in order; `start` is the exact, real time of \
that segment. \
Return ONLY Markdown, no code fences and no surrounding commentary. \
Cite the audio for every decision, action, claim or quote with a bracketed \
timestamp copied from a segment's `start`: [mm:ss], or [hh:mm:ss] when the \
recording is an hour or longer. Never invent a timestamp, never round one, \
and never cite a time that is not a segment start. If a sentence has no \
supporting segment, leave it uncited. \
Never invent speaker names; use the `speaker` label exactly as given, and \
`Unattributed` when it is null. Never invent facts, owners, dates or due \
times that were not spoken. Keep the transcript's language; do not translate \
unless the preset asks for it. \
Start directly with a section heading (## …); never add a top-level `#` title.";

const MINUTES_PROMPT: &str = "You write meeting minutes from a transcript. \
Use these sections, omitting any that the transcript does not support: \
`## Decisions`, `## Discussion`, `## Follow-ups`. \
Under `## Discussion`, group by topic with a short bold topic lead. \
Attribute decisions to the speaker who made them when the transcript does. \
Every bullet that states a decision, number, date or commitment carries its \
citation. Be concise: minutes are a record, not a retelling.";

const ACTION_ITEMS_PROMPT: &str = "You extract action items from a transcript. \
Write one `- ` bullet per action, in the order they were agreed. Each bullet \
is `- **Owner** — action [citation]`, where Owner is the speaker label exactly \
as spoken, or `Unassigned` when no owner was named. Put any spoken due time in \
the action sentence (never invent one). \
End with `## Open questions` only when the transcript leaves a question \
unanswered; omit the section otherwise. Do not include actions nobody agreed \
to.";

const BILINGUAL_PROMPT: &str = "You write a bilingual brief for a meeting that \
may switch between English and Bahasa Indonesia. \
Start with `## Ringkasan` (if the meeting is mainly Indonesian) or \
`## Summary` (if mainly English): 3–6 bullets covering the decisions and the \
state of the work, each cited. \
Then add exactly one short paragraph in the OTHER language under `## Summary \
(English)` or `## Ringkasan (Bahasa Indonesia)` — a faithful one-paragraph \
brief, not a new set of claims. \
Then `## Quotes` with at most three short verbatim quotes, each in its \
original language and cited. Keep every quote exactly as spoken.";

/// One `notes` row, as stored and as the sheet reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredNote {
    pub preset: NotePreset,
    /// The stored Markdown, verbatim (already citation-checked).
    pub body_md: String,
    pub citations_checked: u32,
    pub citations_removed: u32,
    /// Why the citations were removed.
    pub breakdown: CitationBreakdown,
    /// The model that produced it (`settings.polish.model` at run time).
    pub model: String,
    /// RFC 3339, the regeneration time.
    pub created_at: String,
}

impl StoredNote {
    /// The one honest result line. `12 citations checked · 1 removed (no
    /// matching audio)`. With nothing removed the parenthetical is omitted;
    /// with nothing cited at all it says so rather than pretending.
    pub fn result_line(&self) -> String {
        if self.citations_checked == 0 {
            return "no citations returned".to_string();
        }
        if self.citations_removed == 0 {
            format!("{} citations checked · 0 removed", self.citations_checked)
        } else {
            format!(
                "{} citations checked · {} removed (no matching audio)",
                self.citations_checked, self.citations_removed
            )
        }
    }

    /// The breakdown's short form, shown under the result line only when
    /// something was removed.
    pub fn breakdown_line(&self) -> Option<String> {
        if self.citations_removed == 0 {
            return None;
        }
        let mut parts = Vec::new();
        if self.breakdown.removed > 0 {
            parts.push(format!("{} no match", self.breakdown.removed));
        }
        if self.breakdown.past_end > 0 {
            parts.push(format!("{} past end", self.breakdown.past_end));
        }
        if self.breakdown.malformed > 0 {
            parts.push(format!("{} malformed", self.breakdown.malformed));
        }
        if self.breakdown.duplicate > 0 {
            parts.push(format!("{} duplicate", self.breakdown.duplicate));
        }
        Some(parts.join(" · "))
    }
}

/// Why citations were removed, counted separately.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CitationBreakdown {
    /// Valid-looking time, but no segment start within the tolerance.
    pub removed: u32,
    /// A time past the end of the recording.
    pub past_end: u32,
    /// A bracket that reads as a time (digits and colons) but does not parse.
    pub malformed: u32,
    /// A second citation of a segment already cited in this document.
    pub duplicate: u32,
}

impl CitationBreakdown {
    pub fn total(&self) -> u32 {
        self.removed + self.past_end + self.malformed + self.duplicate
    }
}

/// The validator's result: the rewritten Markdown plus the counts.
#[derive(Debug, Clone, PartialEq)]
pub struct NotesCheck {
    pub body_md: String,
    pub citations_checked: u32,
    pub citations_removed: u32,
    pub breakdown: CitationBreakdown,
}

/// The outcome of one notes generation (possibly chunked then merged).
#[derive(Debug, Clone, PartialEq)]
pub struct NotesOutcome {
    pub preset: NotePreset,
    pub body_md: String,
    pub citations_checked: u32,
    pub citations_removed: u32,
    pub breakdown: CitationBreakdown,
    /// How many requests ran (1 unless the segments exceeded the budget).
    pub parts: usize,
    pub model: String,
}

impl NotesOutcome {
    pub fn is_chunked(&self) -> bool {
        self.parts > 1
    }

    /// The stored-row shape (the runtime adds the timestamp).
    pub fn stored(&self, created_at: String) -> StoredNote {
        StoredNote {
            preset: self.preset,
            body_md: self.body_md.clone(),
            citations_checked: self.citations_checked,
            citations_removed: self.citations_removed,
            breakdown: self.breakdown,
            model: self.model.clone(),
            created_at,
        }
    }
}

/// A time-like bracket token.
enum TimeToken {
    Valid(i64),
    Malformed,
}

/// Parses one bracket's content. `None` means "not a time token at all" — a
/// `[banana]` is left exactly where it is and never counted. A token that
/// starts with a digit and contains only digits/colons is time-like; if its
/// shape or ranges are wrong it is [`TimeToken::Malformed`] (removed).
fn parse_time_token(raw: &str) -> Option<TimeToken> {
    let token = raw.trim();
    let mut chars = token.chars();
    match chars.next() {
        Some(first) if first.is_ascii_digit() => {}
        _ => return None,
    }
    if !token.chars().all(|c| c.is_ascii_digit() || c == ':') {
        return None;
    }
    if !token.contains(':') {
        // `[12]` is a bracket, not a time token — untouched.
        return None;
    }
    let parts: Vec<&str> = token.split(':').collect();
    if parts.len() != 2 && parts.len() != 3 {
        return Some(TimeToken::Malformed);
    }
    if parts.iter().any(|part| part.is_empty() || part.len() > 3) {
        return Some(TimeToken::Malformed);
    }
    let values: Option<Vec<u32>> = parts.iter().map(|part| part.parse::<u32>().ok()).collect();
    let Some(values) = values else {
        return Some(TimeToken::Malformed);
    };
    let (hours, minutes, seconds) = match values.as_slice() {
        [minutes, seconds] => (0, *minutes, *seconds),
        [hours, minutes, seconds] => (*hours, *minutes, *seconds),
        _ => return Some(TimeToken::Malformed),
    };
    if minutes >= 60 || seconds >= 60 {
        return Some(TimeToken::Malformed);
    }
    Some(TimeToken::Valid(
        (i64::from(hours) * 3600 + i64::from(minutes) * 60 + i64::from(seconds)) * 1000,
    ))
}

/// Formats a segment time in the canonical citation form: `mm:ss` below an
/// hour, `hh:mm:ss` from an hour on.
pub fn citation_clock(ms: i64) -> String {
    let total_s = ms.max(0) / 1000;
    let hours = total_s / 3600;
    let minutes = (total_s % 3600) / 60;
    let seconds = total_s % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// Rewrites every real citation to its segment's canonical time, removes every
/// citation that names audio which does not exist, and counts what happened.
///
/// `[banana]` is not a time token and is untouched. A time-like token that does
/// not parse is malformed and removed. A time past the recording's end is
/// `past_end`. A second citation of an already-cited segment is `duplicate`.
/// Everything else that matches no segment start within
/// [`CITATION_TOLERANCE_MS`] is `removed`.
pub fn check_notes(body: &str, segments: &[Segment]) -> NotesCheck {
    let end_ms = segments
        .iter()
        .map(|segment| segment.end_ms)
        .max()
        .unwrap_or(0);
    let mut output = String::with_capacity(body.len());
    let mut checked = 0u32;
    let mut breakdown = CitationBreakdown::default();
    let mut cited: BTreeSet<u32> = BTreeSet::new();

    let mut rest = body;
    while let Some(open) = rest.find('[') {
        // Keep everything before the '['; the bracket itself is only written
        // when the token survives (a removed citation leaves no stray '[').
        output.push_str(&rest[..open]);
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find(']') else {
            // An unterminated bracket: keep the remainder verbatim.
            output.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let token = &after_open[..close];
        let mut kept = false;
        let mut dropped_citation = false;
        if let Some(parsed) = parse_time_token(token) {
            checked += 1;
            match parsed {
                TimeToken::Malformed => {
                    breakdown.malformed += 1;
                    dropped_citation = true;
                }
                TimeToken::Valid(ms) => {
                    if segments.is_empty() || ms > end_ms.saturating_add(CITATION_TOLERANCE_MS) {
                        breakdown.past_end += 1;
                        dropped_citation = true;
                    } else if let Some(segment) = nearest_segment(ms, segments) {
                        if cited.insert(segment.idx) {
                            output.push('[');
                            output.push_str(&citation_clock(segment.start_ms));
                            kept = true;
                        } else {
                            breakdown.duplicate += 1;
                            dropped_citation = true;
                        }
                    } else {
                        breakdown.removed += 1;
                        dropped_citation = true;
                    }
                }
            }
        } else {
            // Not a time token: echo it exactly.
            output.push('[');
            output.push_str(token);
            kept = true;
        }
        if kept {
            output.push(']');
        } else if dropped_citation {
            // A removed citation must not leave its own separator behind:
            // "approved [13:00]." reads "approved." afterwards, not
            // "approved ." — the space belonged to the citation.
            if output.ends_with(' ') {
                output.pop();
            }
        }
        rest = &after_open[close + 1..];
    }
    output.push_str(rest);

    NotesCheck {
        body_md: output,
        citations_checked: checked,
        citations_removed: breakdown.total(),
        breakdown,
    }
}

/// The nearest segment whose start is within the tolerance; ties break to the
/// earlier row so the rewrite is deterministic.
fn nearest_segment(ms: i64, segments: &[Segment]) -> Option<&Segment> {
    segments
        .iter()
        .filter_map(|segment| {
            let distance = (segment.start_ms - ms).abs();
            (distance <= CITATION_TOLERANCE_MS).then_some((distance, segment))
        })
        .min_by_key(|(distance, segment)| (*distance, segment.idx))
        .map(|(_, segment)| segment)
}

/// One segment as the request carries it.
#[derive(Debug, Serialize)]
struct NotesSegment<'a> {
    id: u32,
    start: String,
    start_ms: i64,
    speaker: Option<&'a str>,
    text: &'a str,
}

#[derive(Debug, Serialize)]
struct NotesPayload<'a> {
    title: &'a str,
    language: Option<&'a str>,
    part: Option<usize>,
    parts: Option<usize>,
    segments: Vec<NotesSegment<'a>>,
}

/// Builds one notes request's user content (public so tests can pin that the
/// prompt carries the segments, their real starts and their speaker labels).
pub fn build_notes_payload(
    title: &str,
    language: Option<&str>,
    segments: &[Segment],
    speaker_labels: &[(u32, String)],
    part: Option<(usize, usize)>,
) -> String {
    let payload = NotesPayload {
        title,
        language,
        part: part.map(|(index, _)| index),
        parts: part.map(|(_, parts)| parts),
        segments: segments
            .iter()
            .map(|segment| NotesSegment {
                id: segment.idx,
                start: citation_clock(segment.start_ms),
                start_ms: segment.start_ms,
                speaker: speaker_labels
                    .iter()
                    .find(|(idx, _)| *idx == segment.idx)
                    .map(|(_, label)| label.as_str()),
                text: &segment.text,
            })
            .collect(),
    };
    serde_json::to_string(&payload).expect("a notes payload of plain fields cannot fail")
}

/// Splits segments at row boundaries so each part's text stays within
/// [`NOTES_CHUNK_CHARS`]. One oversized segment is kept whole (a transcript row
/// is never split mid-word).
fn chunk_segments(segments: &[Segment]) -> Vec<&[Segment]> {
    if segments.is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut used = 0usize;
    for (index, segment) in segments.iter().enumerate() {
        let size = segment.text.chars().count() + 64;
        if index > start && used + size > NOTES_CHUNK_CHARS {
            parts.push(&segments[start..index]);
            start = index;
            used = 0;
        }
        used += size;
    }
    parts.push(&segments[start..]);
    parts
}

/// Generates one preset's notes through the polish client's own
/// `/chat/completions` path, validates every citation against `segments`, and
/// returns the checked Markdown.
///
/// One request per preset unless the segment text exceeds
/// [`NOTES_CHUNK_CHARS`], in which case the parts are generated in order and
/// merged; `parts` says which path ran. The endpoint and model are exactly the
/// user's polish settings.
pub fn generate_notes(
    title: &str,
    language: Option<&str>,
    segments: &[Segment],
    speaker_labels: &[(u32, String)],
    preset: NotePreset,
    cfg: &PolishSettings,
) -> Result<NotesOutcome, SuramakError> {
    if segments.is_empty() {
        return Err(SuramakError::Notes(
            "the transcript has no segments — there is nothing to summarize".to_string(),
        ));
    }
    let client = crate::polish::OpenAiCompat::new()?;
    let parts = chunk_segments(segments);
    let total = parts.len();
    let mut bodies = Vec::with_capacity(total);
    for (index, part) in parts.iter().enumerate() {
        let user = build_notes_payload(
            title,
            language,
            part,
            speaker_labels,
            (total > 1).then_some((index + 1, total)),
        );
        let system = if total > 1 {
            format!(
                "{}\n\nThis transcript is split into {total} parts. You are writing \
                 part {} of {total}; write only this part's sections, starting directly \
                 with a section heading, and do not repeat the document title.",
                preset.system_prompt(),
                index + 1
            )
        } else {
            preset.system_prompt().to_string()
        };
        let body = client.complete_markdown(&system, &user, cfg)?;
        if body.trim().is_empty() {
            return Err(SuramakError::BadResponse(
                "the endpoint returned an empty note".to_string(),
            ));
        }
        bodies.push(body.trim().to_string());
    }

    let merged = bodies.join("\n\n");
    let check = check_notes(&merged, segments);
    let body_md = format!("# {}\n\n{}", preset.heading(), check.body_md.trim());
    Ok(NotesOutcome {
        preset,
        body_md,
        citations_checked: check.citations_checked,
        citations_removed: check.citations_removed,
        breakdown: check.breakdown,
        parts: total,
        model: cfg.model.clone(),
    })
}

/// M25C: one preset's notes over a whole multi-part session.
///
/// The parts are put on a session-absolute timeline first (part 2 starts at
/// part 1's duration), and the **same** absolute segments are both sent to the
/// model and passed to [`check_notes`]. A citation that lives in part 3 is
/// therefore checked against the timeline the model saw, not against part 1.
///
/// An incomplete session is refused before any request is made, with the named
/// `session_incomplete` reason and the honest `2 of 3 parts transcribed` line.
pub fn generate_session_notes(
    title: &str,
    language: Option<&str>,
    parts: &[SessionPartInput],
    speaker_labels: &[(u32, String)],
    preset: NotePreset,
    cfg: &PolishSettings,
) -> Result<NotesOutcome, SuramakError> {
    let total = parts
        .first()
        .map(|part| part.part_count as usize)
        .unwrap_or(0);
    if let Some(reason) = session_incomplete_refusal(parts.len(), total) {
        return Err(SuramakError::Notes(reason));
    }
    let timeline = session_timeline(parts);
    if timeline.segments.is_empty() {
        return Err(SuramakError::Notes(
            "the session has no segments — there is nothing to summarize".to_string(),
        ));
    }
    generate_notes(
        title,
        language,
        &timeline.segments,
        speaker_labels,
        preset,
        cfg,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(idx: u32, start_ms: i64, end_ms: i64, text: &str) -> Segment {
        Segment {
            idx,
            start_ms,
            end_ms,
            text: text.to_string(),
        }
    }

    /// 0:00, 0:12, 1:05, 1:41 — the fixture the validator matrix uses.
    fn roster() -> Vec<Segment> {
        vec![
            segment(0, 0, 8_000, "Good morning everyone."),
            segment(1, 12_000, 20_000, "We decided to ship on Friday."),
            segment(2, 65_000, 78_000, "Budi will own the migration."),
            segment(3, 101_000, 120_000, "That is all."),
        ]
    }

    #[test]
    fn an_exact_citation_is_kept_and_canonical() {
        let check = check_notes("- ship Friday [00:12]\n", &roster());
        assert_eq!(check.body_md, "- ship Friday [00:12]\n");
        assert_eq!(check.citations_checked, 1);
        assert_eq!(check.citations_removed, 0);
    }

    #[test]
    fn a_citation_within_the_tolerance_is_rewritten_to_the_segment_time() {
        // 11.4 s is 0.6 s before segment 1's 12 s start.
        let check = check_notes("ship Friday [00:11]\n", &roster());
        assert_eq!(check.body_md, "ship Friday [00:12]\n");
        assert_eq!(check.citations_removed, 0);

        // 1.4 s before: still inside ±1.5 s.
        let inside = check_notes("owner [01:04]\n", &roster());
        assert_eq!(inside.body_md, "owner [01:05]\n");
        assert_eq!(inside.citations_removed, 0);

        // 2 s off: no segment start is close enough.
        let outside = check_notes("owner [01:03]\n", &roster());
        assert_eq!(outside.body_md, "owner\n");
        assert_eq!(outside.citations_removed, 1);
        assert_eq!(outside.breakdown.removed, 1);
    }

    #[test]
    fn the_hh_mm_ss_form_is_accepted_and_canonicalised() {
        let check = check_notes("decision [00:01:05]\n", &roster());
        assert_eq!(check.body_md, "decision [01:05]\n");
        assert_eq!(check.citations_checked, 1);
        assert_eq!(check.citations_removed, 0);
    }

    #[test]
    fn several_tokens_on_one_line_are_each_checked() {
        let check = check_notes(
            "- Ship Friday [00:12]; migrate [01:05]; done [02:00]\n",
            &roster(),
        );
        assert_eq!(
            check.body_md,
            "- Ship Friday [00:12]; migrate [01:05]; done\n"
        );
        assert_eq!(check.citations_checked, 3);
        assert_eq!(check.citations_removed, 1);
        assert_eq!(check.breakdown.removed, 1);
    }

    #[test]
    fn a_non_time_bracket_is_untouched_and_not_counted() {
        let check = check_notes("a [banana] and a [note] stay\n", &roster());
        assert_eq!(check.body_md, "a [banana] and a [note] stay\n");
        assert_eq!(check.citations_checked, 0);
        assert_eq!(check.citations_removed, 0);
    }

    #[test]
    fn a_malformed_time_token_is_removed_and_counted() {
        let check = check_notes("bad [12:60] and [1:2:3:4]\n", &roster());
        assert_eq!(check.body_md, "bad and\n");
        assert_eq!(check.citations_checked, 2);
        assert_eq!(check.citations_removed, 2);
        assert_eq!(check.breakdown.malformed, 2);
    }

    #[test]
    fn a_token_past_the_end_is_counted_separately() {
        // The last segment ends at 2:00.
        let check = check_notes("later [45:00]\n", &roster());
        assert_eq!(check.body_md, "later\n");
        assert_eq!(check.citations_checked, 1);
        assert_eq!(check.citations_removed, 1);
        assert_eq!(check.breakdown.past_end, 1);
        assert_eq!(check.breakdown.removed, 0);
    }

    #[test]
    fn a_duplicate_citation_of_one_segment_is_removed_and_counted() {
        let check = check_notes("first [00:12] then again [12] and [00:12]\n", &roster());
        assert_eq!(check.body_md, "first [00:12] then again [12] and\n");
        // `[12]` is not time-like (no colon) — untouched, uncounted.
        assert_eq!(check.citations_checked, 2);
        assert_eq!(check.citations_removed, 1);
        assert_eq!(check.breakdown.duplicate, 1);
    }

    #[test]
    fn the_removed_total_is_the_sum_of_the_breakdown() {
        let check = check_notes("[00:30] [12:60] [45:00] [00:12] [00:12]\n", &roster());
        assert_eq!(check.body_md, " [00:12]\n");
        assert_eq!(check.citations_checked, 5);
        assert_eq!(check.citations_removed, 4);
        assert_eq!(check.breakdown.removed, 1);
        assert_eq!(check.breakdown.malformed, 1);
        assert_eq!(check.breakdown.past_end, 1);
        assert_eq!(check.breakdown.duplicate, 1);
        assert_eq!(check.citations_removed, check.breakdown.total());
    }

    #[test]
    fn a_token_before_the_first_segment_is_removed() {
        // The first segment starts at 0; 30 s is not within 1.5 s of 12 s.
        let check = check_notes("[00:30]\n", &roster());
        assert_eq!(check.body_md, "\n");
        assert_eq!(check.breakdown.removed, 1);
    }

    #[test]
    fn canonical_clock_switches_at_an_hour() {
        assert_eq!(citation_clock(65_000), "01:05");
        assert_eq!(citation_clock(3_600_000), "01:00:00");
        assert_eq!(citation_clock(3_723_000), "01:02:03");
    }

    #[test]
    fn the_payload_carries_real_starts_and_speaker_labels() {
        let labels = vec![(2u32, "Budi".to_string())];
        let payload = build_notes_payload("Weekly sync", Some("en"), &roster(), &labels, None);
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["title"], "Weekly sync");
        assert_eq!(value["segments"][1]["start"], "00:12");
        assert_eq!(value["segments"][1]["start_ms"], 12_000);
        assert_eq!(value["segments"][2]["speaker"], "Budi");
        assert!(value["segments"][0]["speaker"].is_null());
    }

    #[test]
    fn every_preset_prompt_carries_the_citation_contract() {
        for preset in NotePreset::ALL {
            let prompt = preset.system_prompt();
            assert!(prompt.contains("Never invent a timestamp"), "{preset:?}");
            assert!(prompt.contains("[mm:ss]"), "{preset:?}");
            assert!(
                prompt.contains("never add a top-level `#` title"),
                "{preset:?}"
            );
        }
        assert!(MINUTES_PROMPT.contains("## Decisions"));
        assert!(ACTION_ITEMS_PROMPT.contains("Unassigned"));
        assert!(BILINGUAL_PROMPT.contains("Bahasa Indonesia"));
    }

    #[test]
    fn preset_keys_round_trip_and_unknown_keys_are_none() {
        for preset in NotePreset::ALL {
            assert_eq!(NotePreset::parse(preset.key()), Some(preset));
        }
        assert_eq!(NotePreset::parse("summary"), None);
    }

    #[test]
    fn the_result_line_is_honest_about_zero_and_removals() {
        let note = StoredNote {
            preset: NotePreset::Minutes,
            body_md: String::new(),
            citations_checked: 12,
            citations_removed: 1,
            breakdown: CitationBreakdown {
                removed: 1,
                ..Default::default()
            },
            model: "m".to_string(),
            created_at: String::new(),
        };
        assert_eq!(
            note.result_line(),
            "12 citations checked · 1 removed (no matching audio)"
        );
        assert_eq!(note.breakdown_line().as_deref(), Some("1 no match"));

        let clean = StoredNote {
            citations_checked: 12,
            citations_removed: 0,
            breakdown: CitationBreakdown::default(),
            ..note.clone()
        };
        assert_eq!(clean.result_line(), "12 citations checked · 0 removed");
        assert_eq!(clean.breakdown_line(), None);

        let none = StoredNote {
            citations_checked: 0,
            ..clean
        };
        assert_eq!(none.result_line(), "no citations returned");
    }

    #[test]
    fn chunking_splits_at_row_boundaries_and_keeps_order() {
        let tall: Vec<Segment> = (0..10)
            .map(|idx| {
                segment(
                    idx,
                    i64::from(idx) * 1000,
                    i64::from(idx) * 1000 + 900,
                    &"x".repeat(4_000),
                )
            })
            .collect();
        let parts = chunk_segments(&tall);
        assert!(parts.len() > 1, "10 × 4k chars exceeds the 24k budget");
        let mut seen = Vec::new();
        for part in &parts {
            assert!(
                part.iter().map(|s| s.text.len() + 64).sum::<usize>() <= NOTES_CHUNK_CHARS,
                "a part stays inside the budget unless one row alone exceeds it"
            );
            seen.extend(part.iter().map(|s| s.idx));
        }
        assert_eq!(seen, (0..10).collect::<Vec<_>>(), "order and coverage");
    }
}
