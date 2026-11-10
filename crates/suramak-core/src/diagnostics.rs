//! Advisory subtitle diagnostics: characters per line (CPL) and characters per
//! second (CPS).
//!
//! These are guardrails, not gates: a segment over a threshold still plays and
//! still exports. The editor renders a quiet badge where the segment already
//! renders, and the same numbers can be computed by tests/exporters.
//!
//! ## Counting rules (the contract)
//!
//! - Text is split on hard line breaks (`\n`, `\r\n`, `\r`). Each line is
//!   trimmed of leading and trailing whitespace before counting; **interior
//!   spaces count** because they occupy subtitle width.
//! - Counts are **grapheme clusters**, never bytes and never UTF-16 units:
//!   `é` written as `e` + U+0301 is one character, an emoji ZWJ sequence
//!   (`👩🏽‍💻`) is one, a keycap (`1️⃣`) is one, a flag (two regional
//!   indicators) is one, and a variation selector (`🎙️`) does not add one.
//! - **CPL** is the longest line's grapheme count. A single 80-character line
//!   is an 80-CPL segment — there is no implicit wrapping; ASS/SRT only break
//!   where the text says so.
//! - **CPS** is the total grapheme count of all lines (newlines excluded)
//!   divided by the cue's duration in seconds. A cue whose `end_ms` is not
//!   after its `start_ms` has no meaningful rate: CPS is reported as `0.0` and
//!   `zero_duration` is set. That is a timing defect for the exporter's
//!   zero-duration policy, not a reading-speed finding.
//! - A value **strictly greater** than a threshold crosses it: 37 CPL is fine
//!   at the default, 38 is a warning, 42 is still a warning, 43 is an error.
//!   The same rule applies to 17/20 CPS.
//!
//! ## Grapheme coverage
//!
//! This is a bounded, dependency-free approximation of UAX #29 aimed at the
//! scripts Suramak transcribes (Latin/Indonesian, Arabic-script Indonesian,
//! common emoji): combining marks, variation selectors, emoji modifiers, tag
//! characters and keycap marks attach to the previous cluster; a zero-width
//! joiner attaches both sides; regional-indicator pairs form one cluster. It is
//! not a full Unicode grapheme-break table — an unlisted combining mark counts
//! as its own cluster, which can only make a diagnostic more conservative.

/// The advisory thresholds. Defaults: CPL 37/42, CPS 17/20.
///
/// `warn` must not exceed `error`; callers that configure these keep that
/// invariant (a swapped pair collapses to the strictest value, never panics).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubtitleLimits {
    /// CPL above this is a warning.
    pub cpl_warn: usize,
    /// CPL above this is an error.
    pub cpl_error: usize,
    /// CPS above this is a warning.
    pub cps_warn: f32,
    /// CPS above this is an error.
    pub cps_error: f32,
}

impl Default for SubtitleLimits {
    fn default() -> Self {
        Self {
            cpl_warn: 37,
            cpl_error: 42,
            cps_warn: 17.0,
            cps_error: 20.0,
        }
    }
}

/// How far one dimension is past its advisory thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warning,
    Error,
}

/// The computed diagnostics for one segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentDiagnostics {
    /// Grapheme clusters across all lines (newlines excluded).
    pub graphemes: usize,
    /// Hard line count (at least 1, even for empty text).
    pub lines: usize,
    /// The longest line's grapheme count.
    pub longest_line: usize,
    /// Graphemes per second; `0.0` when the cue has no positive duration.
    pub cps: f32,
    /// CPL severity against the limits.
    pub cpl_level: Level,
    /// CPS severity against the limits.
    pub cps_level: Level,
    /// True when `end_ms <= start_ms` (CPS is not judged).
    pub zero_duration: bool,
}

impl SegmentDiagnostics {
    /// The worse of the two dimensions.
    pub fn level(&self) -> Level {
        self.cpl_level.max(self.cps_level)
    }

    /// The advisory badge text, or `None` when both dimensions are fine.
    ///
    /// Only exceeded dimensions appear, so a segment that is merely dense does
    /// not also claim a line-length problem: `CPL 44 · CPS 21.3`.
    pub fn badge(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.cpl_level != Level::Ok {
            parts.push(format!("CPL {}", self.longest_line));
        }
        if self.cps_level != Level::Ok {
            parts.push(format!("CPS {:.1}", self.cps));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }
}

/// Counts grapheme clusters in `text` (newlines count as ordinary clusters —
/// callers measure lines, not the joined text).
pub fn grapheme_count(text: &str) -> usize {
    let mut count = 0usize;
    let mut join_next = false;
    let mut regional_run = 0u8;

    for ch in text.chars() {
        if ch == '\u{200D}' {
            // ZWJ extends the current cluster and joins the next character.
            join_next = true;
            if count == 0 {
                count = 1;
            }
            continue;
        }
        if is_regional_indicator(ch) {
            if regional_run == 1 {
                // The second half of a flag: same cluster.
                regional_run = 0;
                continue;
            }
            regional_run = 1;
            count += 1;
            continue;
        }
        regional_run = 0;
        if is_extend(ch) || join_next {
            join_next = false;
            if count == 0 {
                count = 1;
            }
            continue;
        }
        count += 1;
    }
    count
}

/// Whether `ch` attaches to the preceding cluster (combining mark, variation
/// selector, emoji modifier, tag character or keycap mark).
fn is_extend(ch: char) -> bool {
    matches!(ch as u32,
        // Combining Diacritical Marks (and supplements) — the `café` case.
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF
        // Hebrew and Arabic points/marks.
        | 0x0591..=0x05BD | 0x05BF | 0x05C1..=0x05C2 | 0x05C4..=0x05C5 | 0x05C7
        | 0x0610..=0x061A | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC
        | 0x06DF..=0x06E4 | 0x06E7..=0x06E8 | 0x06EA..=0x06ED
        // Devanagari signs (covers the Arabic/Devanagari-script Indonesian edge).
        | 0x0900..=0x0903 | 0x093A..=0x093C | 0x093E..=0x094F | 0x0951..=0x0957
        | 0x0962..=0x0963
        // Thai marks (stock recorder apps on Android produce Thai, not ID, but
        // the rule is script-independent).
        | 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E
        // General combining-mark blocks (0x20E3, the keycap mark, is in this range).
        | 0x20D0..=0x20F0 | 0xFE20..=0xFE2F
        // Variation selectors.
        | 0xFE00..=0xFE0F | 0xE0100..=0xE01EF
        // Emoji skin-tone modifiers and tag characters.
        | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F
    )
}

fn is_regional_indicator(ch: char) -> bool {
    matches!(ch as u32, 0x1F1E6..=0x1F1FF)
}

/// Computes [`SegmentDiagnostics`] for one segment's text and timing.
pub fn diagnose(
    text: &str,
    start_ms: i64,
    end_ms: i64,
    limits: &SubtitleLimits,
) -> SegmentDiagnostics {
    let mut graphemes = 0usize;
    let mut lines = 0usize;
    let mut longest_line = 0usize;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line).trim();
        let count = grapheme_count(line);
        lines += 1;
        graphemes += count;
        longest_line = longest_line.max(count);
    }

    let duration_ms = end_ms.saturating_sub(start_ms);
    let zero_duration = duration_ms <= 0;
    let cps = if zero_duration {
        0.0
    } else {
        graphemes as f32 / (duration_ms as f32 / 1000.0)
    };

    let cpl_level = level_above(
        longest_line as f64,
        limits.cpl_warn as f64,
        limits.cpl_error as f64,
    );
    let cps_level = if zero_duration {
        Level::Ok
    } else {
        level_above(cps as f64, limits.cps_warn as f64, limits.cps_error as f64)
    };

    SegmentDiagnostics {
        graphemes,
        lines: lines.max(1),
        longest_line,
        cps,
        cpl_level,
        cps_level,
        zero_duration,
    }
}

/// `value > error` → Error, `value > warn` → Warning, else Ok. Errors win when
/// a caller passes a warn/error pair out of order.
fn level_above(value: f64, warn: f64, error: f64) -> Level {
    if value > error || value > warn && error < warn {
        Level::Error
    } else if value > warn {
        Level::Warning
    } else {
        Level::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grapheme_counting_handles_latin_combining_and_emoji_sequences() {
        assert_eq!(grapheme_count("hello"), 5);
        assert_eq!(grapheme_count(""), 0);
        // Plain precomposed é is one scalar; decomposed e + U+0301 is one cluster.
        assert_eq!(grapheme_count("café"), 4);
        assert_eq!(grapheme_count("cafe\u{301}"), 4);
        assert_eq!(
            grapheme_count("\u{301}"),
            1,
            "a lone mark is still a cluster"
        );
        // Emoji: variation selector, skin tone, ZWJ sequence, keycap, flag.
        assert_eq!(grapheme_count("🎙️"), 1);
        assert_eq!(grapheme_count("👩🏽‍💻"), 1);
        assert_eq!(grapheme_count("1️⃣"), 1);
        assert_eq!(grapheme_count("🇮🇩"), 1);
        assert_eq!(grapheme_count("🇮🇩🇬🇧"), 2);
        // Indonesian words are plain scalars.
        assert_eq!(grapheme_count("pertanggungjawaban"), 18);
    }

    #[test]
    fn cpl_uses_the_longest_hard_line_and_counts_interior_spaces() {
        let limits = SubtitleLimits::default();
        // 37 graphemes: exactly at the warning line, still OK.
        let at = "a".repeat(37);
        assert_eq!(diagnose(&at, 0, 4_000, &limits).cpl_level, Level::Ok);
        // 38: warning. 42: still warning. 43: error.
        assert_eq!(
            diagnose(&"a".repeat(38), 0, 4_000, &limits).cpl_level,
            Level::Warning
        );
        assert_eq!(
            diagnose(&"a".repeat(42), 0, 4_000, &limits).cpl_level,
            Level::Warning
        );
        assert_eq!(
            diagnose(&"a".repeat(43), 0, 4_000, &limits).cpl_level,
            Level::Error
        );
        // Multiline: the longest line decides, the total excludes newlines.
        let d = diagnose("short\npertanggungjawaban yang panjang", 0, 4_000, &limits);
        assert_eq!(d.lines, 2);
        assert_eq!(d.longest_line, 31);
        assert_eq!(d.graphemes, 5 + 31);
        // CRLF is one line break, and surrounding whitespace is not counted.
        let d = diagnose("  first line  \r\nsecond", 0, 4_000, &limits);
        assert_eq!(d.lines, 2);
        assert_eq!(d.longest_line, 10);
        assert_eq!(d.graphemes, 10 + 6);
    }

    #[test]
    fn cps_is_strictly_greater_than_the_threshold_and_reports_one_decimal() {
        let limits = SubtitleLimits::default();
        // `total` graphemes split over `per_line` lines: CPL stays low, so the
        // test isolates the CPS dimension.
        let shaped = |total: usize, per_line: usize| {
            let mut lines = Vec::new();
            let mut left = total;
            while left > 0 {
                let take = left.min(per_line);
                lines.push("a".repeat(take));
                left -= take;
            }
            lines.join("\n")
        };

        // 68 graphemes over 4.0 s = 17.0 CPS: at the warning threshold, OK.
        let at = diagnose(&shaped(68, 17), 0, 4_000, &limits);
        assert!((at.cps - 17.0).abs() < 1e-4);
        assert_eq!(at.cpl_level, Level::Ok);
        assert_eq!(at.cps_level, Level::Ok);
        assert_eq!(at.badge(), None);

        // 17.5 CPS: warning, and only CPS is listed (the lines are short).
        let over = diagnose(&shaped(70, 18), 0, 4_000, &limits);
        assert_eq!(over.cpl_level, Level::Ok);
        assert_eq!(over.cps_level, Level::Warning);
        assert_eq!(over.badge().as_deref(), Some("CPS 17.5"));

        // 20.0 is still a warning; 20.5 is an error.
        let at_error = diagnose(&shaped(80, 20), 0, 4_000, &limits);
        assert_eq!(at_error.cps_level, Level::Warning);
        let error = diagnose(&shaped(82, 21), 0, 4_000, &limits);
        assert_eq!(error.cps_level, Level::Error);
        assert_eq!(error.level(), Level::Error);
        assert_eq!(error.badge().as_deref(), Some("CPS 20.5"));

        // A dense short cue is judged by rate, not by line length (short line).
        let d = diagnose("halo dunia", 0, 400, &limits);
        assert_eq!(d.cpl_level, Level::Ok);
        assert_eq!(d.cps_level, Level::Error, "10 graphemes / 0.4 s = 25 CPS");
    }

    #[test]
    fn a_zero_or_negative_duration_has_no_cps_and_is_flagged() {
        let limits = SubtitleLimits::default();
        for (start, end) in [(1_000, 1_000), (2_000, 1_000)] {
            let d = diagnose("Selamat pagi", start, end, &limits);
            assert!(d.zero_duration);
            assert_eq!(d.cps, 0.0);
            assert_eq!(d.cps_level, Level::Ok);
            assert_eq!(d.cpl_level, Level::Ok);
            assert!(d.badge().is_none());
        }
    }

    #[test]
    fn thresholds_are_configurable_and_a_swapped_pair_stays_conservative() {
        let strict = SubtitleLimits {
            cpl_warn: 10,
            cpl_error: 12,
            cps_warn: 5.0,
            cps_error: 6.0,
        };
        let d = diagnose("dua belas chr", 0, 2_400, &strict);
        assert_eq!(d.longest_line, 13);
        assert_eq!(d.cpl_level, Level::Error);
        assert_eq!(d.cps_level, Level::Warning, "13 / 2.4 s = 5.4 CPS");

        // warn > error: anything past the lower bound is an error (never a
        // silent pass and never a panic).
        let swapped = SubtitleLimits {
            cpl_warn: 20,
            cpl_error: 10,
            cps_warn: 20.0,
            cps_error: 10.0,
        };
        assert_eq!(
            diagnose(&"a".repeat(11), 0, 4_000, &swapped).cpl_level,
            Level::Error
        );
    }

    #[test]
    fn badge_only_lists_the_dimension_that_is_over() {
        let limits = SubtitleLimits::default();
        // Long but slow: only CPL.
        let long_slow = diagnose(&"a".repeat(45), 0, 10_000, &limits);
        assert_eq!(long_slow.badge().as_deref(), Some("CPL 45"));
        // Short but fast: only CPS.
        let short_fast = diagnose("halo dunia", 0, 400, &limits);
        assert_eq!(short_fast.badge().as_deref(), Some("CPS 25.0"));
    }
}
