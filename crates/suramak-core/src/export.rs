use std::fmt::Write;

use crate::time::{fmt_ms, fmt_plain, fmt_srt, fmt_vtt};
use crate::transcript::{Source, Transcript};
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
    writeln!(out, "[Script Info]").unwrap();
    writeln!(out, "; Exported by Suramak").unwrap();
    writeln!(out, "Title: {}", script_title(&t.title)).unwrap();
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
    // White fill, black outline, bottom-centre, generous side margins at 1080p.
    // Arial is a family name: a renderer without it falls back to its sans face.
    writeln!(
        out,
        "Style: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,\
         0,0,0,0,100,100,0,0,1,2,1,2,48,48,64,1"
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(out, "[Events]").unwrap();
    writeln!(
        out,
        "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text"
    )
    .unwrap();

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

fn source_label(source: &Source) -> String {
    match source {
        Source::File { name } => name.clone(),
        Source::Recording { started_at, .. } => {
            format!("Recording {}", started_at.format("%Y-%m-%d %H:%M UTC"))
        }
    }
}
