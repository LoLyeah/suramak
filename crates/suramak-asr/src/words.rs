//! Token → word timing for whisper output (M15.6).
//!
//! whisper.cpp computes per-token spans only when `token_timestamps` is on: it
//! snaps the tokens that carry a forced timestamp token (`tid`) to
//! `t_beg + 2·(tid − token_beg)` centiseconds and fills the gaps between those
//! anchors proportionally to each token's *voice length*
//! (`whisper_exp_compute_token_level_timestamps`, vendored
//! `whisper-rs-sys-0.15.0/whisper.cpp/src/whisper.cpp:8420-8510`). whisper-rs
//! 0.16 exposes the resulting spans through
//! `WhisperToken::token_data()` → `whisper_token_data { t0, t1, .. }` (see
//! `whisper-rs-0.16.0/src/whisper_state/token.rs:42-50`). The spans are
//! **estimates at 10 ms granularity**, not forced alignment, and everything
//! here only *reads* them.
//!
//! This module maps those token spans onto whole words. It never invents a
//! time: a segment whose tokens do not spell exactly the text the library
//! stored gets **no** word timing (the caller then falls back to segment
//! seeking). That rule is the difference between an honest estimate and a
//! fabricated one.
//!
//! Boundaries are ASCII whitespace. That is the right rule for the languages
//! the workbench targets (English, Indonesian); scripts that do not separate
//! words with spaces (CJK, Thai) are out of scope and get one "word" per
//! space-delimited run at best — stated in the M15.6 report rather than
//! papered over with a segmenter this crate does not have.

use suramak_core::transcript::WordTiming;

/// One whisper token as the mapper needs it: the raw bytes it spells, its
/// estimated span (milliseconds), and its probability.
///
/// `bytes` is the token's raw byte string, not a `&str`: whisper can split a
/// multi-byte UTF-8 character across tokens (common for Indonesian), and a
/// lossy per-token conversion would corrupt the reconstruction.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenSpan {
    pub bytes: Vec<u8>,
    /// Token start in milliseconds (whisper reports centiseconds; the caller
    /// converts).
    pub start_ms: i64,
    /// Token end in milliseconds.
    pub end_ms: i64,
    /// Token probability (`whisper_token_data::p`), kept for provenance and
    /// for the report's low-confidence analysis.
    pub probability: f32,
}

impl TokenSpan {
    pub fn new(bytes: impl Into<Vec<u8>>, start_ms: i64, end_ms: i64, probability: f32) -> Self {
        Self {
            bytes: bytes.into(),
            start_ms,
            end_ms,
            probability,
        }
    }
}

/// Maps a segment's token spans onto the words of `text`.
///
/// `text` is the segment text as stored (whisper-rs's `to_str()`, trimmed).
/// The function succeeds only when the tokens, concatenated and trimmed, are
/// **byte-identical** to `text`; otherwise it returns an empty list — a
/// segment whose stored text was edited, truncated, or lossily decoded has no
/// defensible word timing, and guessing one would misplace the karaoke.
///
/// Every returned word lies inside `[start_ms, end_ms]` (the segment's own
/// span), starts are non-decreasing, and every word has `end_ms >= start_ms`.
/// A word's start is the start of the first token it touches; its end is the
/// end of the last token it touches — no averaging, no resampling.
pub fn words_for_segment(
    segment_idx: u32,
    text: &str,
    start_ms: i64,
    end_ms: i64,
    tokens: &[TokenSpan],
) -> Vec<WordTiming> {
    if text.trim().is_empty() || tokens.is_empty() {
        return Vec::new();
    }
    let (segment_start, segment_end) = (start_ms.min(end_ms), start_ms.max(end_ms));

    // Token byte ranges inside the concatenation.
    let mut joined: Vec<u8> = Vec::new();
    let mut spans: Vec<(usize, usize, i64, i64)> = Vec::with_capacity(tokens.len());
    for token in tokens {
        let begin = joined.len();
        joined.extend_from_slice(&token.bytes);
        spans.push((begin, joined.len(), token.start_ms, token.end_ms));
    }

    // The tokens must spell exactly the stored text (after the same trim the
    // engine applies). Anything else is a different string and gets no timing.
    let (lo, hi) = trim_ascii_whitespace(&joined);
    if &joined[lo..hi] != text.as_bytes() {
        return Vec::new();
    }

    let mut words = Vec::new();
    let mut previous_start = segment_start;
    for (begin, end) in word_ranges(&joined, lo, hi) {
        // First and last token touching this word.
        let mut first: Option<(i64, i64)> = None;
        let mut last: Option<(i64, i64)> = None;
        for &(token_begin, token_end, t0, t1) in &spans {
            if token_end <= begin || token_begin >= end {
                continue;
            }
            if first.is_none() {
                first = Some((t0, t1));
            }
            last = Some((t0, t1));
        }
        let (Some((t0, _)), Some((_, t1))) = (first, last) else {
            // A word made only of bytes no token covers cannot be timed.
            return Vec::new();
        };
        if t0 < 0 || t1 < 0 {
            // Unknown token time (whisper leaves -1 when it could not fill a
            // gap). Refuse the segment instead of substituting the boundary.
            return Vec::new();
        }
        let word_start = t0.clamp(segment_start, segment_end).max(previous_start);
        let word_end = t1.clamp(word_start, segment_end);
        let word_text = String::from_utf8_lossy(&joined[begin..end]).into_owned();
        words.push(WordTiming {
            segment_idx,
            word_idx: words.len() as u32,
            start_ms: word_start,
            end_ms: word_end,
            text: word_text,
        });
        previous_start = word_start;
    }
    words
}

/// The byte range of `bytes` without leading/trailing ASCII whitespace.
fn trim_ascii_whitespace(bytes: &[u8]) -> (usize, usize) {
    let mut lo = 0;
    while lo < bytes.len() && bytes[lo].is_ascii_whitespace() {
        lo += 1;
    }
    let mut hi = bytes.len();
    while hi > lo && bytes[hi - 1].is_ascii_whitespace() {
        hi -= 1;
    }
    (lo, hi)
}

/// Word byte ranges (`[begin, end)`) inside `bytes[lo..hi]`, split on runs of
/// ASCII whitespace. ASCII space bytes never occur inside a multi-byte UTF-8
/// sequence, so this cannot split a character.
fn word_ranges(bytes: &[u8], lo: usize, hi: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut cursor = lo;
    while cursor < hi {
        while cursor < hi && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let begin = cursor;
        while cursor < hi && !bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor > begin {
            ranges.push((begin, cursor));
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(text: &str, t0: i64, t1: i64) -> TokenSpan {
        TokenSpan::new(text.as_bytes().to_vec(), t0, t1, 0.9)
    }

    fn texts(words: &[WordTiming]) -> Vec<&str> {
        words.iter().map(|word| word.text.as_str()).collect()
    }

    #[test]
    fn a_one_token_word_keeps_the_token_span() {
        let words = words_for_segment(0, "hello", 0, 1_000, &[token(" hello", 100, 420)]);
        assert_eq!(texts(&words), vec!["hello"]);
        assert_eq!((words[0].start_ms, words[0].end_ms), (100, 420));
        assert_eq!((words[0].segment_idx, words[0].word_idx), (0, 0));
    }

    #[test]
    fn a_word_split_across_tokens_spans_them_all() {
        // "Suramak" as three sub-word tokens, as whisper's BPE often emits it.
        let tokens = [
            token(" Sur", 200, 340),
            token("am", 340, 430),
            token("ak", 430, 610),
        ];
        let words = words_for_segment(0, "Suramak", 0, 2_000, &tokens);
        assert_eq!(texts(&words), vec!["Suramak"]);
        assert_eq!((words[0].start_ms, words[0].end_ms), (200, 610));
    }

    #[test]
    fn several_words_in_one_token_share_its_span() {
        // Whisper sometimes emits " a b" as one token; each word inherits it.
        let words = words_for_segment(0, "a b", 0, 1_000, &[token(" a b", 120, 480)]);
        assert_eq!(texts(&words), vec!["a", "b"]);
        assert_eq!((words[0].start_ms, words[0].end_ms), (120, 480));
        assert_eq!((words[1].start_ms, words[1].end_ms), (120, 480));
    }

    #[test]
    fn punctuation_stays_with_its_word() {
        let tokens = [
            token(" Hello", 0, 300),
            token(",", 300, 320),
            token(" world.", 400, 900),
        ];
        let words = words_for_segment(0, "Hello, world.", 0, 1_000, &tokens);
        assert_eq!(texts(&words), vec!["Hello,", "world."]);
    }

    #[test]
    fn non_ascii_word_boundaries_are_not_split_mid_character() {
        // Indonesian with accents and a multi-byte word; the space is the only
        // boundary and the UTF-8 stays intact.
        let tokens = [token(" café", 0, 300), token(" sudah", 300, 700)];
        let words = words_for_segment(0, "café sudah", 0, 1_000, &tokens);
        assert_eq!(texts(&words), vec!["café", "sudah"]);
    }

    #[test]
    fn a_token_split_inside_a_utf8_character_still_maps() {
        // 'é' = 0xC3 0xA9, split across two tokens. Concatenation restores it;
        // the word is one byte range and inherits both tokens' spans.
        let mut first = "caf".as_bytes().to_vec();
        first.push(0xC3);
        let mut second = vec![0xA9];
        second.extend_from_slice(b" sudah");
        let tokens = [
            TokenSpan::new(first, 0, 300, 0.8),
            TokenSpan::new(second, 300, 750, 0.8),
        ];
        let words = words_for_segment(0, "café sudah", 0, 1_000, &tokens);
        assert_eq!(texts(&words), vec!["café", "sudah"]);
        assert_eq!((words[0].start_ms, words[0].end_ms), (0, 750));
    }

    #[test]
    fn a_text_that_does_not_match_the_tokens_gets_no_timing() {
        // The user edited the segment: the tokens no longer spell it.
        let words = words_for_segment(0, "hello there", 0, 1_000, &[token(" hello", 100, 400)]);
        assert!(words.is_empty());
        // …and a whitespace-only / empty text has nothing to map.
        assert!(words_for_segment(0, "   ", 0, 1_000, &[token(" x", 0, 10)]).is_empty());
        assert!(words_for_segment(0, "x", 0, 1_000, &[]).is_empty());
    }

    #[test]
    fn an_unknown_token_time_refuses_the_whole_segment() {
        // whisper leaves -1 when it cannot fill a gap; substituting anything
        // would be an invented boundary.
        let tokens = [token(" one", 0, 300), token(" two", -1, -1)];
        assert!(words_for_segment(0, "one two", 0, 1_000, &tokens).is_empty());
    }

    #[test]
    fn times_are_clamped_to_the_segment_and_stay_monotonic() {
        // A token span crossing the segment ends is clamped into it, and the
        // starts never go backwards.
        let tokens = [token(" one", 0, 300), token(" two", 700, 1_400)];
        let words = words_for_segment(0, "one two", 200, 1_000, &tokens);
        assert_eq!(words[0].start_ms, 200, "clamped up to the segment start");
        assert_eq!(words[1].end_ms, 1_000, "clamped down to the segment end");
        assert!(words[1].start_ms >= words[0].start_ms);
    }

    #[test]
    fn word_indices_are_dense_and_ordered() {
        let tokens = [
            token(" a", 0, 100),
            token(" bb", 100, 200),
            token(" ccc", 200, 300),
        ];
        let words = words_for_segment(4, "a bb ccc", 0, 400, &tokens);
        assert_eq!(
            words.iter().map(|w| w.word_idx).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(words.iter().all(|w| w.segment_idx == 4));
    }

    #[test]
    fn tabs_and_newlines_are_boundaries_too() {
        let words = words_for_segment(0, "a\tb\nc", 0, 900, &[token(" a\tb\nc", 10, 800)]);
        assert_eq!(texts(&words), vec!["a", "b", "c"]);
    }
}
