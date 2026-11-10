use chrono::DateTime;
use suramak_core::diagnostics::SubtitleLimits;
use suramak_core::export::{
    bilingual_advisories, bilingual_cues, to_ass, to_ass_bilingual, to_ass_with, to_json,
    to_markdown, to_srt, to_srt_bilingual, to_txt, to_vtt, to_vtt_bilingual, AssOptions,
    ZeroDuration, ASS_ONE_FRAME_MS, BILINGUAL_LINE_BUDGET_CPL,
};
use suramak_core::transcript::{Segment, Source, Transcript};
use suramak_core::translate::TranslationRecord;
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

// ---- M15.8: stacked bilingual outputs ------------------------------------

/// A translation record for `idx` (the store shape the exporters read).
fn translation(idx: u32, text: &str) -> (u32, TranslationRecord) {
    (
        idx,
        TranslationRecord::new(text, "mock-model", "2026-11-10T10:00:00+00:00", vec![idx]),
    )
}

#[test]
fn bilingual_srt_stacks_the_translation_under_the_source_line() {
    let t = fixed_transcript();
    let translations = vec![
        translation(0, "Halo dunia."),
        translation(1, "Ini adalah ujian."),
    ];
    let out = to_srt_bilingual(&t, &translations).unwrap();
    let expected = "\
1
00:00:00,000 --> 00:00:02,400
Hello world.
Halo dunia.

2
00:00:02,400 --> 00:00:05,000
This is a test.
Ini adalah ujian.

3
00:00:05,000 --> 00:00:08,500
Goodbye.

";
    assert_eq!(out, expected, "the untranslated cue stays single-line");
    // No blank line ever sits inside a cue payload: that would terminate it.
    assert!(
        !out.contains("Hello world.\n\n") && !out.contains("Halo dunia.\nHalo"),
        "{out}"
    );
}

#[test]
fn bilingual_cues_align_by_segment_id_not_by_list_order() {
    let t = fixed_transcript();
    // Out of order, with one id that no longer exists (a stale row after an
    // edit) — the transcript's order and timing win.
    let translations = vec![
        translation(2, "Selamat tinggal."),
        translation(9, "orphan"),
        translation(0, "Halo dunia."),
    ];
    let cues = bilingual_cues(&t, &translations);
    assert_eq!(cues.len(), 3);
    assert_eq!(
        (cues[0].idx, cues[0].translation.as_str()),
        (0, "Halo dunia.")
    );
    assert_eq!(cues[1].translation, "");
    assert_eq!(
        (cues[2].idx, cues[2].translation.as_str()),
        (2, "Selamat tinggal.")
    );
    assert!(cues.iter().all(|cue| cue.idx != 9), "orphans are ignored");
}

#[test]
fn bilingual_vtt_escapes_entities_and_keeps_both_lines() {
    let t = transcript_with(&[(0, 2_000, "Use <i>this</i> & that")]);
    let translations = vec![translation(0, "Pakai <i>ini</i> & itu > semua")];
    let out = to_vtt_bilingual(&t, &translations).unwrap();
    assert!(out.starts_with("WEBVTT\n\n"), "{out}");
    assert!(
        out.contains("Use &lt;i&gt;this&lt;/i&gt; &amp; that"),
        "the source line is escaped too: {out}"
    );
    assert!(
        out.contains("Pakai &lt;i&gt;ini&lt;/i&gt; &amp; itu &gt; semua"),
        "the translated line is escaped: {out}"
    );
    // The escaping is text-only: the cue timing is untouched.
    assert!(out.contains("00:00:00.000 --> 00:00:02.000"));
}

#[test]
fn bilingual_ass_writes_the_top_bottom_style_pair_and_two_events_per_cue() {
    let t = transcript_with(&[(0, 2_000, "Hello world."), (2_000, 4_000, "Goodbye.")]);
    let translations = vec![
        translation(0, "Halo dunia."),
        translation(1, "Selamat tinggal."),
    ];
    let out = to_ass_bilingual(&t, &translations, &AssOptions::default()).unwrap();

    // The style pair: bottom-centre primary, top-centre secondary.
    assert!(
        out.contains("Style: Primary,Arial,48,&H00FFFFFF"),
        "primary style missing: {out}"
    );
    assert!(
        out.contains("Style: Secondary,Arial,42,&H00FFFFFF"),
        "secondary style missing: {out}"
    );
    let secondary = out
        .lines()
        .find(|line| line.starts_with("Style: Secondary"))
        .unwrap();
    let fields: Vec<&str> = secondary.split(',').collect();
    assert_eq!(fields[18], "8", "Secondary is top-centre (Alignment 8)");
    let primary = out
        .lines()
        .find(|line| line.starts_with("Style: Primary"))
        .unwrap();
    let fields: Vec<&str> = primary.split(',').collect();
    assert_eq!(fields[18], "2", "Primary is bottom-centre (Alignment 2)");

    // Two events per cue, same timing, one per style.
    let dialogues: Vec<&str> = out
        .lines()
        .filter(|line| line.starts_with("Dialogue: "))
        .collect();
    assert_eq!(dialogues.len(), 4);
    assert!(dialogues[0].contains(",Primary,"));
    assert!(dialogues[0].ends_with("Hello world."));
    assert!(dialogues[1].contains(",Secondary,"));
    assert!(dialogues[1].ends_with("Halo dunia."));
    assert!(dialogues[0].contains("0:00:00.00,0:00:02.00"));
    assert!(dialogues[1].contains("0:00:00.00,0:00:02.00"));
}

#[test]
fn bilingual_ass_escapes_both_lines_and_honours_the_zero_duration_policy() {
    let t = transcript_with(&[
        (0, 2_000, "A {tag} & a comma, here"),
        (3_000, 3_000, "instant"),
        (4_000, 5_000, "normal"),
    ]);
    let translations = vec![
        translation(0, "Sebuah {tag} & koma, di sini"),
        translation(1, "seketika"),
        translation(2, "biasa"),
    ];

    let out = to_ass_bilingual(&t, &translations, &AssOptions::default()).unwrap();
    let events = parse_ass(&out);
    assert_eq!(events.len(), 6, "three cues × two languages");
    assert_eq!(events[0].2, "A {tag} & a comma, here");
    assert_eq!(events[1].2, "Sebuah {tag} & koma, di sini");
    assert_eq!(
        events[2],
        (3_000, 3_000 + ASS_ONE_FRAME_MS, "instant".to_string())
    );
    assert_eq!(
        events[3],
        (3_000, 3_000 + ASS_ONE_FRAME_MS, "seketika".to_string())
    );

    // Skip drops the pair as a unit, never half of it.
    let skipped = to_ass_bilingual(
        &t,
        &translations,
        &AssOptions {
            zero_duration: ZeroDuration::Skip,
        },
    )
    .unwrap();
    let events = parse_ass(&skipped);
    assert_eq!(events.len(), 4);
    assert!(events
        .iter()
        .all(|(_, _, text)| text != "instant" && text != "seketika"));
}

#[test]
fn bilingual_advisories_measure_both_lines_against_the_same_limits() {
    let t = transcript_with(&[(0, 1_000, "short source")]);
    let translations = vec![translation(
        0,
        "terjemahan yang jauh lebih panjang daripada baris sumbernya dan melewati batas",
    )];
    let advisories = bilingual_advisories(&t, &translations, &SubtitleLimits::default());
    assert_eq!(advisories.len(), 1);
    assert_eq!(
        advisories[0].primary_badge(),
        None,
        "the source line is fine"
    );
    let badge = advisories[0]
        .translation_badge()
        .expect("the translation is long");
    assert!(badge.starts_with("CPL "), "{badge}");
    // The documented budget is the error threshold of the same ladder.
    assert_eq!(BILINGUAL_LINE_BUDGET_CPL, 42);

    // An untranslated cue measures as empty: never a badge.
    let empty = bilingual_advisories(&t, &[], &SubtitleLimits::default());
    assert_eq!(empty[0].translation_badge(), None);
}

#[test]
fn bilingual_exporters_never_drop_a_zero_duration_cue_from_srt_or_vtt() {
    let t = transcript_with(&[(1_000, 1_000, "instant")]);
    let translations = vec![translation(0, "seketika")];
    let srt = to_srt_bilingual(&t, &translations).unwrap();
    assert!(srt.contains("instant\nseketika"), "{srt}");
    assert!(srt.contains("00:00:01,000 --> 00:00:01,000"));
    let vtt = to_vtt_bilingual(&t, &translations).unwrap();
    assert!(vtt.contains("instant\nseketika"), "{vtt}");
}
