use chrono::DateTime;
use suramak_core::export::{
    to_ass, to_ass_with, to_json, to_markdown, to_srt, to_txt, to_vtt, AssOptions, ZeroDuration,
    ASS_ONE_FRAME_MS,
};
use suramak_core::transcript::{Segment, Source, Transcript};
use uuid::Uuid;

/// Returns a fixed 3-segment transcript for golden-string tests.
fn fixed_transcript() -> Transcript {
    let id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let created_at = DateTime::parse_from_rfc3339("2024-01-15T10:00:00Z")
        .unwrap()
        .to_utc();
    Transcript {
        id,
        title: "Test Transcript".to_string(),
        created_at,
        updated_at: created_at,
        source: Source::File {
            name: "intro.mp3".to_string(),
        },
        model: "turbo-q5-0".to_string(),
        language: Some("id".to_string()),
        audio_path: None,
        segments: vec![
            Segment {
                idx: 0,
                start_ms: 0,
                end_ms: 2400,
                text: "Hello world.".to_string(),
            },
            Segment {
                idx: 1,
                start_ms: 2400,
                end_ms: 5000,
                text: "This is a test.".to_string(),
            },
            Segment {
                idx: 2,
                start_ms: 5000,
                end_ms: 8500,
                text: "Goodbye.".to_string(),
            },
        ],
    }
}

#[test]
fn export_txt_golden() {
    let t = fixed_transcript();
    let out = to_txt(&t).unwrap();
    let expected = "\
[00:00:00] Hello world.\n\
[00:00:02] This is a test.\n\
[00:00:05] Goodbye.\n";
    assert_eq!(out, expected);
}

#[test]
fn export_srt_golden() {
    let t = fixed_transcript();
    let out = to_srt(&t).unwrap();
    let expected = "\
1\n\
00:00:00,000 --> 00:00:02,400\n\
Hello world.\n\
\n\
2\n\
00:00:02,400 --> 00:00:05,000\n\
This is a test.\n\
\n\
3\n\
00:00:05,000 --> 00:00:08,500\n\
Goodbye.\n\
\n";
    assert_eq!(out, expected);
}

#[test]
fn export_vtt_golden() {
    let t = fixed_transcript();
    let out = to_vtt(&t).unwrap();
    let expected = "\
WEBVTT\n\
\n\
00:00:00.000 --> 00:00:02.400\n\
Hello world.\n\
\n\
00:00:02.400 --> 00:00:05.000\n\
This is a test.\n\
\n\
00:00:05.000 --> 00:00:08.500\n\
Goodbye.\n\
\n";
    assert_eq!(out, expected);
}

#[test]
fn export_markdown_golden() {
    let t = fixed_transcript();
    let out = to_markdown(&t).unwrap();
    // Check structural elements; exact date formatting is stable.
    assert!(out.starts_with("# Test Transcript\n"), "title line");
    assert!(out.contains("## Transcript"), "transcript heading");
    assert!(
        out.contains("[00:00:00.000 → 00:00:02.400] Hello world."),
        "seg 0"
    );
    assert!(
        out.contains("[00:00:02.400 → 00:00:05.000] This is a test."),
        "seg 1"
    );
    assert!(
        out.contains("[00:00:05.000 → 00:00:08.500] Goodbye."),
        "seg 2"
    );
    assert!(out.contains("- Source: intro.mp3"), "source");
    assert!(out.contains("- Model: turbo-q5-0"), "model");
    assert!(out.contains("- Language: id"), "language");
}

#[test]
fn export_json_golden() {
    let t = fixed_transcript();
    let out = to_json(&t).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["app"], "suramak");
    assert_eq!(v["schema"], 1);
    assert_eq!(v["id"], "00000000-0000-0000-0000-000000000001");
    assert_eq!(v["title"], "Test Transcript");
    assert_eq!(v["model"], "turbo-q5-0");
    assert_eq!(v["language"], "id");
    let segs = v["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 3);
    assert_eq!(segs[0]["id"], 0);
    assert_eq!(segs[0]["start_ms"], 0);
    assert_eq!(segs[0]["end_ms"], 2400);
    assert_eq!(segs[0]["start"], "00:00:00.000");
    assert_eq!(segs[0]["end"], "00:00:02.400");
    assert_eq!(segs[0]["text"], "Hello world.");
    assert_eq!(segs[2]["text"], "Goodbye.");
}

// ---- M15.4: ASS ----------------------------------------------------------

/// A transcript with the given (start, end, text) cues and a fixed identity.
fn transcript_with(cues: &[(i64, i64, &str)]) -> Transcript {
    let id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let created_at = DateTime::parse_from_rfc3339("2024-01-15T10:00:00Z")
        .unwrap()
        .to_utc();
    Transcript {
        id,
        title: "Test Transcript".to_string(),
        created_at,
        updated_at: created_at,
        source: Source::File {
            name: "intro.mp3".to_string(),
        },
        model: "turbo-q5-0".to_string(),
        language: Some("id".to_string()),
        audio_path: None,
        segments: cues
            .iter()
            .enumerate()
            .map(|(idx, (start, end, text))| Segment {
                idx: idx as u32,
                start_ms: *start,
                end_ms: *end,
                text: (*text).to_string(),
            })
            .collect(),
    }
}

/// Parses our own ASS output: one `(start_ms, end_ms, text)` per Dialogue.
/// This is the round-trip reader; the external libass check is documented in
/// the report (ffmpeg is not a test dependency).
fn parse_ass(ass: &str) -> Vec<(i64, i64, String)> {
    fn ms(stamp: &str) -> i64 {
        let (hms, cs) = stamp.split_once('.').expect("centiseconds");
        let parts: Vec<i64> = hms.split(':').map(|p| p.parse().unwrap()).collect();
        let centis: i64 = cs.parse().unwrap();
        ((parts[0] * 3600 + parts[1] * 60 + parts[2]) * 100 + centis) * 10
    }
    ass.lines()
        .filter(|line| line.starts_with("Dialogue: "))
        .map(|line| {
            let fields: Vec<&str> = line["Dialogue: ".len()..].splitn(10, ',').collect();
            assert_eq!(fields.len(), 10, "a Dialogue has 10 comma fields: {line}");
            let text = fields[9]
                .replace("\\N", "\n")
                .replace("\\{", "{")
                .replace("\\}", "}")
                .replace('\u{200B}', "");
            (ms(fields[1]), ms(fields[2]), text)
        })
        .collect()
}

#[test]
fn export_ass_golden_header_and_events() {
    let t = fixed_transcript();
    let out = to_ass(&t).unwrap();
    let expected = "\
[Script Info]
; Exported by Suramak
Title: Test Transcript
ScriptType: v4.00+
WrapStyle: 2
PlayResX: 1920
PlayResY: 1080
ScaledBorderAndShadow: yes

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,2,1,2,48,48,64,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:00.00,0:00:02.40,Default,,0,0,0,,Hello world.
Dialogue: 0,0:00:02.40,0:00:05.00,Default,,0,0,0,,This is a test.
Dialogue: 0,0:00:05.00,0:00:08.50,Default,,0,0,0,,Goodbye.
";
    assert_eq!(out, expected);
}

#[test]
fn ass_escapes_only_what_the_format_requires() {
    let t = transcript_with(&[
        (0, 2_000, "A {tag} here, with a comma"),
        (2_000, 4_000, "first line\r\nsecond line"),
        (4_000, 6_000, r"a literal \N sequence"),
        (6_000, 8_000, r"path\to\file"),
    ]);
    let out = to_ass(&t).unwrap();
    let events = parse_ass(&out);
    assert_eq!(events[0].2, "A {tag} here, with a comma");
    assert_eq!(events[1].2, "first line\nsecond line");
    // The literal `\N` survives (with the zero-width separator ASS needs).
    assert_eq!(events[2].2, r"a literal \N sequence");
    assert_eq!(events[3].2, r"path\to\file");

    // The raw file has no unescaped brace that could open an override block.
    for line in out.lines().filter(|line| line.starts_with("Dialogue: ")) {
        let text = line.splitn(10, ',').nth(9).unwrap();
        let mut backslashes = 0usize;
        for ch in text.chars() {
            if ch == '\\' {
                backslashes += 1;
                continue;
            }
            if (ch == '{' || ch == '}') && backslashes % 2 == 0 {
                panic!("unescaped brace in {text:?}");
            }
            backslashes = 0;
        }
    }
}

#[test]
fn ass_zero_duration_cues_are_widened_or_skipped_never_emitted_empty() {
    let t = transcript_with(&[
        (1_000, 1_000, "instant"),
        (2_000, 2_000, "also instant"),
        (3_000, 4_000, "normal"),
    ]);

    // Default: one 25 fps frame at the cue's start.
    let events = parse_ass(&to_ass(&t).unwrap());
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0],
        (1_000, 1_000 + ASS_ONE_FRAME_MS, "instant".to_string())
    );
    assert_eq!(events[1].1 - events[1].0, ASS_ONE_FRAME_MS);

    // Skip: dropped from this export only.
    let skipped = to_ass_with(
        &t,
        &AssOptions {
            zero_duration: ZeroDuration::Skip,
        },
    )
    .unwrap();
    let events = parse_ass(&skipped);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].2, "normal");

    // Every emitted cue has positive duration and increasing/valid timestamps.
    for (start, end, _) in parse_ass(&to_ass(&t).unwrap()) {
        assert!(end > start, "cue {start}..{end} must not be zero-length");
    }
}

#[test]
fn ass_clamps_negative_starts_and_sanitizes_the_title() {
    let mut t = transcript_with(&[(-500, 1_500, "negative start")]);
    t.title = "line one\nline two".to_string();
    let out = to_ass(&t).unwrap();
    assert!(out.contains("Title: line one line two"), "{out}");
    let events = parse_ass(&out);
    assert_eq!(events[0].0, 0, "ASS has no negative timecodes");
    assert_eq!(events[0].1, 1_500);
}

#[test]
fn ass_and_srt_carry_the_same_words() {
    let t = transcript_with(&[
        (0, 1_500, "Selamat pagi, dunia."),
        (1_500, 3_000, "Kita mulai dengan anggaran."),
    ]);
    let ass = parse_ass(&to_ass(&t).unwrap());
    let srt = to_srt(&t).unwrap();
    let srt_text: String = srt
        .lines()
        .filter(|line| {
            !line.is_empty() && !line.contains("-->") && !line.chars().all(|c| c.is_ascii_digit())
        })
        .collect::<Vec<_>>()
        .join("\n");
    for (_, _, text) in &ass {
        assert!(
            srt_text.contains(text.as_str()),
            "{text:?} missing from SRT"
        );
    }
    let ass_text: String = ass
        .iter()
        .map(|(_, _, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        ass_text, srt_text,
        "both subtitle paths read the same segments"
    );
}

#[test]
fn ass_timestamps_round_to_centiseconds_and_cover_hours() {
    let t = transcript_with(&[(3_723_456, 3_723_999, "one hour in")]);
    let out = to_ass(&t).unwrap();
    assert!(out.contains("1:02:03.46,1:02:04.00"), "{out}");
    let events = parse_ass(&out);
    assert_eq!(events[0].0, 3_723_460);
    assert_eq!(events[0].1, 3_724_000);
}
