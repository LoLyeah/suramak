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

fn source_label(source: &Source) -> String {
    match source {
        Source::File { name } => name.clone(),
        Source::Recording { started_at, .. } => {
            format!("Recording {}", started_at.format("%Y-%m-%d %H:%M UTC"))
        }
    }
}
