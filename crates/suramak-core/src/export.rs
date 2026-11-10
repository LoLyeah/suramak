use std::collections::HashMap;
use std::fmt::Write;

use crate::diagnostics::{diagnose, SegmentDiagnostics, SubtitleLimits};
use crate::time::{fmt_ms, fmt_plain, fmt_srt, fmt_vtt};
use crate::transcript::{Source, Transcript};
use crate::translate::TranslationRecord;
use crate::SuramakError;

/// Exports a transcript to Markdown.
pub fn to_markdown(t: &Transcript) -> Result<String, SuramakError> {
    let mut out = String::new();
    writeln!(out, "# {}", t.title).unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "- Date: {}",
        t.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    )
    .unwrap();
    writeln!(out, "- Source: {}", source_label(&t.source)).unwrap();
    writeln!(out, "- Model: {}", t.model).unwrap();
    writeln!(
        out,
        "- Language: {}",
        t.language.as_deref().unwrap_or("auto")
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(out, "## Transcript").unwrap();
    writeln!(out).unwrap();
    for seg in &t.segments {
        writeln!(
            out,
            "[{} → {}] {}",
            fmt_ms(seg.start_ms),
            fmt_ms(seg.end_ms),
            seg.text.trim()
        )
        .unwrap();
        writeln!(out).unwrap();
    }
    Ok(out)
}

/// Exports a transcript to plain text (one line per segment).
pub fn to_txt(t: &Transcript) -> Result<String, SuramakError> {
    let mut out = String::new();
    for seg in &t.segments {
        writeln!(out, "[{}] {}", fmt_plain(seg.start_ms), seg.text.trim()).unwrap();
    }
    Ok(out)
}

/// Exports a transcript to JSON (suramak schema v1).
pub fn to_json(t: &Transcript) -> Result<String, SuramakError> {
    let source_val = match &t.source {
        Source::File { name } => serde_json::json!({ "type": "file", "name": name }),
        Source::Recording {
            started_at,
            duration_ms,
        } => serde_json::json!({
            "type": "recording",
            "started_at": started_at.to_rfc3339(),
            "duration_ms": duration_ms,
        }),
    };

    let segments: Vec<serde_json::Value> = t
        .segments
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.idx,
                "start_ms": s.start_ms,
                "end_ms": s.end_ms,
                "start": fmt_ms(s.start_ms),
                "end": fmt_ms(s.end_ms),
                "text": s.text.trim(),
            })
        })
        .collect();

    let obj = serde_json::json!({
        "app": "suramak",
        "schema": 1,
        "id": t.id.to_string(),
        "title": t.title,
        "created_at": t.created_at.to_rfc3339(),
        "source": source_val,
        "model": t.model,
        "language": t.language,
        "segments": segments,
    });

    Ok(serde_json::to_string_pretty(&obj)?)
}

/// Exports a transcript to SRT subtitle format.
pub fn to_srt(t: &Transcript) -> Result<String, SuramakError> {
    let mut out = String::new();
    for seg in &t.segments {
        writeln!(out, "{}", seg.idx + 1).unwrap();
        writeln!(out, "{} --> {}", fmt_srt(seg.start_ms), fmt_srt(seg.end_ms)).unwrap();
        writeln!(out, "{}", seg.text.trim()).unwrap();
        writeln!(out).unwrap();
    }
    Ok(out)
}

/// Exports a transcript to tab-separated values — one row per segment, with a
/// header row. This is the spreadsheet/automation sibling of [`to_txt`]: the
/// same rows, the same order, the same text.
///
/// A TSV cell cannot carry a tab or a line break, and the format has no quoting
/// convention every reader honours. The text cell therefore folds every run of
/// whitespace to one space (the only lossy step, and the same normalization a
/// spreadsheet would apply on paste). `idx`, `start_ms` and `end_ms` are the
/// machine columns; a reader that wants `m:ss` uses [`to_txt`] or the JSON.
pub fn to_tsv(t: &Transcript) -> Result<String, SuramakError> {
    let mut out = String::from("idx\tstart_ms\tend_ms\ttext\n");
    for seg in &t.segments {
        let text = seg.text.split_whitespace().collect::<Vec<_>>().join(" ");
        writeln!(
            out,
            "{}\t{}\t{}\t{}",
            seg.idx, seg.start_ms, seg.end_ms, text
        )
        .unwrap();
    }
    Ok(out)
}

/// Exports a transcript to WebVTT subtitle format.
pub fn to_vtt(t: &Transcript) -> Result<String, SuramakError> {
    let mut out = String::new();
    writeln!(out, "WEBVTT").unwrap();
    writeln!(out).unwrap();
    for seg in &t.segments {
        writeln!(out, "{} --> {}", fmt_vtt(seg.start_ms), fmt_vtt(seg.end_ms)).unwrap();
        writeln!(out, "{}", seg.text.trim()).unwrap();
        writeln!(out).unwrap();
    }
    Ok(out)
}

/// How an ASS export treats a cue whose `end_ms` is not after its `start_ms`.
///
/// ASS itself has no notion of a zero-length dialogue that reads well: a
/// zero-duration event is either invisible or flickers for one frame depending
/// on the renderer. The serializer therefore never writes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ZeroDuration {
    /// Drop the cue (its text is still in the transcript; only this export
    /// omits it). Explicit, lossy, opt-in.
    Skip,
    /// Give the cue one frame at 25 fps (40 ms) — the default: the line stays
    /// readable and the timing defect stays visible in the file.
    #[default]
    OneFrame,
}

/// Options for [`to_ass_with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AssOptions {
    /// What to do with cues whose end is not after their start.
    pub zero_duration: ZeroDuration,
}

/// The one frame a zero-duration cue is widened to (25 fps, the frame rate the
/// style's `PlayRes` assumes for deterministic rendering).
pub const ASS_ONE_FRAME_MS: i64 = 40;

/// Exports a transcript to Advanced SubStation Alpha (ASS v4.00+) with the
/// default options (zero-duration cues widened to one frame).
///
/// One `Dialogue` per segment, using the same text the SRT/VTT paths read.
/// Multiline segments become `\N` hard breaks; `[Script Info]` fixes
/// `WrapStyle: 2`, so the renderer never re-wraps what the transcript says.
pub fn to_ass(t: &Transcript) -> Result<String, SuramakError> {
    to_ass_with(t, &AssOptions::default())
}

/// Exports a transcript to ASS v4.00+ with explicit options.
///
/// Escaping is deliberately minimal (only what the format requires):
/// - `{` and `}` are written as `\{`/`\}` — otherwise they open/close an
///   override block and the text inside disappears (libass 0.17 verified).
/// - A real line break becomes `\N`; `\r\n` and `\r` normalize to one break.
/// - A literal backslash is preserved as-is, except immediately before
///   `N`, `n` or `h` (which the format would read as line/space escapes): there
///   a zero-width space is inserted after the backslash, the only visually
///   lossless way to keep the pair (ASS has no backslash escape).
/// - Commas need no escaping: `Text` is the last field in a `Dialogue` line.
pub fn to_ass_with(t: &Transcript, opts: &AssOptions) -> Result<String, SuramakError> {
    let mut out = String::new();
    write_ass_preamble(&mut out, &t.title, &[ASS_PRIMARY_STYLE]);

    for seg in &t.segments {
        let start = seg.start_ms.max(0);
        let end = if seg.end_ms > start {
            seg.end_ms
        } else {
            match opts.zero_duration {
                ZeroDuration::Skip => continue,
                ZeroDuration::OneFrame => start + ASS_ONE_FRAME_MS,
            }
        };
        writeln!(
            out,
            "Dialogue: 0,{},{},Default,,0,0,0,,{}",
            fmt_ass(start),
            fmt_ass(end),
            escape_ass_text(&seg.text)
        )
        .unwrap();
    }
    Ok(out)
}

/// The bottom-centre style `to_ass` has always written.
const ASS_PRIMARY_STYLE: &str =
    "Style: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,\
     0,0,0,0,100,100,0,0,1,2,1,2,48,48,64,1";

/// The bilingual pair's bottom line (the transcript's own language).
const ASS_BILINGUAL_PRIMARY_STYLE: &str =
    "Style: Primary,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,\
     0,0,0,0,100,100,0,0,1,2,1,2,48,48,64,1";

/// The bilingual pair's top line (the translation). Same face and outline,
/// slightly smaller, top-centre (`Alignment: 8`) with a wider top margin so the
/// two lines never collide at 1080p; the pair is the "new style pair" M15-A's
/// serializer reserved room for.
const ASS_BILINGUAL_SECONDARY_STYLE: &str =
    "Style: Secondary,Arial,42,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,\
     0,0,0,0,100,100,0,0,1,2,1,8,48,48,96,1";

/// Writes `[Script Info]`, the style table (one `Style:` line per entry) and
/// the `[Events]` header. `Title:` is sanitized to one line.
fn write_ass_preamble(out: &mut String, title: &str, styles: &[&str]) {
    writeln!(out, "[Script Info]").unwrap();
    writeln!(out, "; Exported by Suramak").unwrap();
    writeln!(out, "Title: {}", script_title(title)).unwrap();
    writeln!(out, "ScriptType: v4.00+").unwrap();
    writeln!(out, "WrapStyle: 2").unwrap();
    writeln!(out, "PlayResX: 1920").unwrap();
    writeln!(out, "PlayResY: 1080").unwrap();
    writeln!(out, "ScaledBorderAndShadow: yes").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "[V4+ Styles]").unwrap();
    writeln!(
        out,
        "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, \
         BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, \
         BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding"
    )
    .unwrap();
    for style in styles {
        writeln!(out, "{style}").unwrap();
    }
    writeln!(out).unwrap();
    writeln!(out, "[Events]").unwrap();
    writeln!(
        out,
        "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text"
    )
    .unwrap();
}

/// ASS timestamps: `H:MM:SS.cc` (centiseconds, nearest).
fn fmt_ass(ms: i64) -> String {
    let centis = (ms.max(0) + 5) / 10;
    let total_s = centis / 100;
    let cs = centis % 100;
    format!(
        "{}:{:02}:{:02}.{:02}",
        total_s / 3600,
        (total_s % 3600) / 60,
        total_s % 60,
        cs
    )
}

/// The `Title:` value: one line, no control characters that would end the field.
fn script_title(title: &str) -> String {
    title.replace(['\r', '\n'], " ").trim().to_string()
}

/// Normalizes newlines and maps them to ASS `\N`, escaping each line's text.
fn escape_ass_text(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<&str> = normalized.split('\n').map(str::trim).collect();
    while lines.first().is_some_and(|line| line.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
        .iter()
        .map(|line| escape_ass_line(line))
        .collect::<Vec<_>>()
        .join("\\N")
}

/// Escapes one line for the `Text` field (see [`to_ass_with`] for the rules).
fn escape_ass_line(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    for (index, ch) in chars.iter().enumerate() {
        match ch {
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\\' => {
                out.push('\\');
                if matches!(chars.get(index + 1), Some('N' | 'n' | 'h')) {
                    out.push('\u{200B}');
                }
            }
            _ => out.push(*ch),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// M15.8: stacked bilingual outputs
// ---------------------------------------------------------------------------

/// The honest per-line budget for a stacked bilingual cue (M15.8).
///
/// A bilingual row doubles the text in the same time window, so each of the two
/// lines gets the same advisory ceiling the single-language rows use — the
/// error threshold of the documented CPL ladder (`42`). Nothing is shortened or
/// wrapped by the exporter: the number is what the advisory badges and the
/// report measure against (see [`bilingual_advisories`]), exactly like the
/// editor's M15.4 badges. A translation longer than this is shown in full, with
/// a badge, because cutting a machine translation mid-sentence would be worse
/// than an over-long line.
pub const BILINGUAL_LINE_BUDGET_CPL: usize = 42;

/// One cue of a bilingual export: the transcript's own line and its opt-in
/// translation, sharing the cue's timing (cue alignment is by segment id, never
/// by array position).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BilingualCue {
    pub idx: u32,
    pub start_ms: i64,
    pub end_ms: i64,
    /// The transcript's text for this cue (the language the user transcribed).
    pub primary: String,
    /// The translated line; empty when this cue has no translation (a partial
    /// run still exports, with the untranslated cues carrying one line).
    pub translation: String,
}

/// Pairs each segment with its stored translation, in transcript order.
///
/// A translation whose idx has no segment (a stale row after a structural edit)
/// is ignored — the transcript is the authority for cue order and timing.
pub fn bilingual_cues(
    t: &Transcript,
    translations: &[(u32, TranslationRecord)],
) -> Vec<BilingualCue> {
    let by_idx: HashMap<u32, &TranslationRecord> = translations
        .iter()
        .map(|(idx, record)| (*idx, record))
        .collect();
    t.segments
        .iter()
        .map(|segment| BilingualCue {
            idx: segment.idx,
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
            primary: segment.text.trim().to_string(),
            translation: by_idx
                .get(&segment.idx)
                .map(|record| record.text.trim().to_string())
                .unwrap_or_default(),
        })
        .collect()
}

/// Normalizes a cue's text into the lines a subtitle block may carry: CRLF/CR
/// become `\n`, each line is trimmed, blank lines are dropped (a blank line
/// terminates an SRT/VTT cue — it can never be part of the payload).
fn subtitle_lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// The payload of one stacked SRT/VTT cue: primary line(s) then translation
/// line(s), with no blank separator (see [`to_srt_bilingual`]).
fn stacked_payload(cue: &BilingualCue) -> Vec<String> {
    let mut lines = subtitle_lines(&cue.primary);
    lines.extend(subtitle_lines(&cue.translation));
    lines
}

/// Exports a transcript as a **stacked bilingual SRT**: one cue per segment,
/// the transcript's line first and the translation directly beneath it.
///
/// Format note (documented in the report): the workorder's "EN line / blank /
/// ID line" schematic is rendered as two adjacent lines here. An empty line
/// inside an SRT cue is the cue terminator in the format grammar, so writing
/// one would split every cue into two and break the timing alignment; the
/// "blank" is the vertical separation the ASS style pair provides, and it is a
/// deliberate omission in SRT/VTT rather than an oversight.
///
/// Escaping: SRT has no markup of its own, so text is written literally after
/// newline normalization. A cue with no translation remains a one-line cue.
pub fn to_srt_bilingual(
    t: &Transcript,
    translations: &[(u32, TranslationRecord)],
) -> Result<String, SuramakError> {
    let mut out = String::new();
    for cue in bilingual_cues(t, translations) {
        writeln!(out, "{}", cue.idx + 1).unwrap();
        writeln!(out, "{} --> {}", fmt_srt(cue.start_ms), fmt_srt(cue.end_ms)).unwrap();
        for line in stacked_payload(&cue) {
            writeln!(out, "{line}").unwrap();
        }
        writeln!(out).unwrap();
    }
    Ok(out)
}

/// Exports a transcript as a **stacked bilingual WebVTT**: same layout as
/// [`to_srt_bilingual`], with the text escaped for cue payloads (`&`, `<`, `>`
/// become entities — WebVTT would otherwise read `<…>` as a tag and a bare `&`
/// as the start of an entity reference).
pub fn to_vtt_bilingual(
    t: &Transcript,
    translations: &[(u32, TranslationRecord)],
) -> Result<String, SuramakError> {
    let mut out = String::new();
    writeln!(out, "WEBVTT").unwrap();
    writeln!(out).unwrap();
    for cue in bilingual_cues(t, translations) {
        writeln!(out, "{} --> {}", fmt_vtt(cue.start_ms), fmt_vtt(cue.end_ms)).unwrap();
        for line in stacked_payload(&cue) {
            writeln!(out, "{}", escape_vtt_text(&line)).unwrap();
        }
        writeln!(out).unwrap();
    }
    Ok(out)
}

/// Escapes one WebVTT cue-payload line (the three characters with syntactic
/// meaning in cue text).
fn escape_vtt_text(line: &str) -> String {
    line.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Exports a transcript as a **bilingual ASS** with the M15-A serializer's new
/// style pair: `Primary` (the transcript's line, bottom-centre) and `Secondary`
/// (the translation, top-centre). Each cue is two `Dialogue` events sharing one
/// timing, so the pair cannot drift out of sync and the renderer does the
/// vertical separation (no blank text line is needed).
///
/// The zero-duration policy applies to the pair as a unit: `Skip` drops both
/// events, `OneFrame` widens both to the same 25 fps frame. Text escaping is
/// the same as the single-language ASS path (braces, hard breaks, literal
/// `\N`/`\n`/`\h` sequences).
pub fn to_ass_bilingual(
    t: &Transcript,
    translations: &[(u32, TranslationRecord)],
    opts: &AssOptions,
) -> Result<String, SuramakError> {
    let mut out = String::new();
    write_ass_preamble(
        &mut out,
        &t.title,
        &[ASS_BILINGUAL_PRIMARY_STYLE, ASS_BILINGUAL_SECONDARY_STYLE],
    );

    for cue in bilingual_cues(t, translations) {
        let start = cue.start_ms.max(0);
        let end = if cue.end_ms > start {
            cue.end_ms
        } else {
            match opts.zero_duration {
                ZeroDuration::Skip => continue,
                ZeroDuration::OneFrame => start + ASS_ONE_FRAME_MS,
            }
        };
        let primary = sub_ass_text(&cue.primary);
        if !primary.is_empty() {
            writeln!(
                out,
                "Dialogue: 0,{},{},Primary,,0,0,0,,{}",
                fmt_ass(start),
                fmt_ass(end),
                primary
            )
            .unwrap();
        }
        let translation = sub_ass_text(&cue.translation);
        if !translation.is_empty() {
            writeln!(
                out,
                "Dialogue: 0,{},{},Secondary,,0,0,0,,{}",
                fmt_ass(start),
                fmt_ass(end),
                translation
            )
            .unwrap();
        }
    }
    Ok(out)
}

/// Escapes a bilingual cue line for ASS; an empty line is dropped (the other
/// half of the pair still renders).
fn sub_ass_text(text: &str) -> String {
    escape_ass_text(text)
}

/// One cue's advisory diagnostics for **both** lines (M15.8).
///
/// The badges the editor paints come from here: each line is measured on its
/// own against the same [`SubtitleLimits`] and the same cue timing, so a
/// translation that is too long or too fast is visible even when the source
/// line is comfortable.
#[derive(Debug, Clone, PartialEq)]
pub struct BilingualAdvice {
    pub idx: u32,
    pub primary: SegmentDiagnostics,
    pub translation: SegmentDiagnostics,
}

impl BilingualAdvice {
    /// The primary line's badge (`None` when comfortable).
    pub fn primary_badge(&self) -> Option<String> {
        self.primary.badge()
    }

    /// The translated line's badge (`None` when comfortable or absent).
    pub fn translation_badge(&self) -> Option<String> {
        self.translation.badge()
    }
}

/// Measures both lines of every bilingual cue. `translation` diagnostics on an
/// untranslated cue are the empty-text diagnostics (no badge), so callers can
/// render the pair unconditionally.
pub fn bilingual_advisories(
    t: &Transcript,
    translations: &[(u32, TranslationRecord)],
    limits: &SubtitleLimits,
) -> Vec<BilingualAdvice> {
    bilingual_cues(t, translations)
        .into_iter()
        .map(|cue| BilingualAdvice {
            idx: cue.idx,
            primary: diagnose(&cue.primary, cue.start_ms, cue.end_ms, limits),
            translation: diagnose(&cue.translation, cue.start_ms, cue.end_ms, limits),
        })
        .collect()
}

fn source_label(source: &Source) -> String {
    match source {
        Source::File { name } => name.clone(),
        Source::Recording { started_at, .. } => {
            format!("Recording {}", started_at.format("%Y-%m-%d %H:%M UTC"))
        }
    }
}
