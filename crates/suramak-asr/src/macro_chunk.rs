//! M26C — 15-minute silence-aligned macro-windows over one mapped `PcmSlice`.
//!
//! This module is the **pure** half of the slice: boundary selection, the prompt
//! firewall and the deterministic seam stitch. It never touches whisper; the
//! decode loop that applies these rules to one reused `WhisperState` lives in
//! [`crate::macro_pipeline`] and [`crate::whisper::WindowSession`].
//!
//! Design: `docs/reports/M26-memory-design.md` §C. The rules are fixed by the
//! work order (`docs/WORKORDER-M26C.md`) and are reproduced here as constants,
//! not as knobs:
//!
//! * window target 900 s, boundary search in `[cursor+840 s, cursor+930 s]`;
//! * RMS over 20 ms frames, silence threshold
//!   `max(1e−4, percentile15(E) × 1.8)`;
//! * **C1** cut at the midpoint of the longest silent run ≥ 400 ms, overlap
//!   1.5 s;
//! * **C2** (no silence) the minimum-energy 200 ms window in
//!   `[cursor+870 s, cursor+915 s]`, overlap 4.0 s, flagged `cut_without_pause`;
//! * the prompt carries at most 160 tokens, and only when the tail passes the
//!   three checks (unique byte-4-gram ratio > 0.45, mean token probability
//!   ≥ 0.45, no 1–6 word phrase repeated ≥ 3 times); otherwise zero tokens;
//! * the seam keeps words by midpoint (`cut − overlap/2`), then drops the
//!   longest common suffix-of-previous / prefix-of-next run of up to 12
//!   normalised words.
//!
//! Everything here is deterministic: ties in the cut search go to the earliest
//! frame, and the seam match takes the **longest** common run, so the same
//! audio always produces the same cut positions.

use suramak_core::transcript::WordTiming;

/// 16 kHz mono f32, the pipeline's one sample format.
pub const SAMPLE_RATE: usize = 16_000;
/// Samples in one millisecond.
pub const SAMPLES_PER_MS: usize = SAMPLE_RATE / 1_000;
/// One RMS frame: 20 ms.
pub const FRAME_SAMPLES: usize = 320;

/// The fixed cut/prompt/seam policy of §C.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MacroConfig {
    /// Window target: a slice longer than this is cut into windows.
    pub window_target_ms: i64,
    /// Boundary search band, relative to the window cursor.
    pub search_start_ms: i64,
    pub search_end_ms: i64,
    /// Adaptive silence floor.
    pub silence_floor: f32,
    pub silence_percentile: f32,
    pub silence_factor: f32,
    /// C1: the shortest silent run that is a cut candidate.
    pub min_silence_ms: i64,
    pub overlap_ms: i64,
    /// C2: the fallback band and window.
    pub fallback_start_ms: i64,
    pub fallback_end_ms: i64,
    pub fallback_window_ms: i64,
    pub fallback_overlap_ms: i64,
    /// A tail shorter than this is folded into the previous window instead of
    /// being decoded as its own tiny window.
    pub min_tail_ms: i64,
}

impl Default for MacroConfig {
    fn default() -> Self {
        Self {
            window_target_ms: 900_000,
            search_start_ms: 840_000,
            search_end_ms: 930_000,
            silence_floor: 1e-4,
            silence_percentile: 0.15,
            silence_factor: 1.8,
            min_silence_ms: 400,
            overlap_ms: 1_500,
            fallback_start_ms: 870_000,
            fallback_end_ms: 915_000,
            fallback_window_ms: 200,
            fallback_overlap_ms: 4_000,
            min_tail_ms: 60_000,
        }
    }
}

impl MacroConfig {
    fn ms_to_samples(ms: i64) -> usize {
        (ms.max(0) as usize).saturating_mul(SAMPLES_PER_MS)
    }

    /// The target in samples.
    pub fn target_samples(&self) -> usize {
        Self::ms_to_samples(self.window_target_ms)
    }

    /// Whether a slice of `total_samples` takes the macro path at all.
    pub fn applies_to(&self, total_samples: usize) -> bool {
        total_samples > self.target_samples()
    }
}

/// Why a boundary was placed where it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutKind {
    /// C1: the midpoint of a silent run ≥ 400 ms.
    Silence,
    /// C2: the minimum-energy 200 ms window — non-stop speech.
    MinimumEnergy,
}

/// One decoded window. `start..end` is the sample range handed to whisper;
/// `cut_sample` is the boundary this window ends at (`None` for the last one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacroWindow {
    pub start_sample: usize,
    pub end_sample: usize,
    pub cut_sample: Option<usize>,
    pub overlap_samples: usize,
    /// C2's flag: the cut fell in speech, not in a pause.
    pub cut_without_pause: bool,
}

impl MacroWindow {
    pub fn start_ms(&self) -> i64 {
        (self.start_sample / SAMPLES_PER_MS) as i64
    }

    pub fn end_ms(&self) -> i64 {
        (self.end_sample / SAMPLES_PER_MS) as i64
    }

    pub fn cut_ms(&self) -> Option<i64> {
        self.cut_sample.map(|cut| (cut / SAMPLES_PER_MS) as i64)
    }

    /// The seam midpoint: `cut − overlap/2`. Words at or before it belong to
    /// this window, words after it to the next one.
    pub fn seam_midpoint_ms(&self) -> Option<i64> {
        let cut = self.cut_ms()?;
        Some(cut - (self.overlap_samples / SAMPLES_PER_MS) as i64 / 2)
    }
}

/// The cut chosen for one cursor, before it becomes a [`MacroWindow`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutChoice {
    pub cut_sample: usize,
    pub overlap_samples: usize,
    pub kind: CutKind,
}

/// One 20 ms frame's mean square (the RMS is its square root; ranking by mean
/// square and by RMS is the same order, so both rules can share one pass).
fn frame_mean_square(samples: &[f32], start: usize) -> f64 {
    let mut sum = 0.0f64;
    for &value in &samples[start..start + FRAME_SAMPLES] {
        sum += f64::from(value) * f64::from(value);
    }
    sum / FRAME_SAMPLES as f64
}

/// `percentile15(E)` — nearest-rank on the ascending frame energies.
fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

/// The frames of the search band, as `(first_frame_index, mean_squares)`.
///
/// Frame `i` covers `cursor + i × 20 ms`; every window is cut on the same 20 ms
/// grid regardless of where the previous cut landed. The first index is
/// returned with the energies because the band starts mid-window.
fn band_frames(
    samples: &[f32],
    cursor: usize,
    total: usize,
    band_lo: usize,
    band_hi: usize,
) -> (usize, Vec<f64>) {
    let first = cursor + (band_lo.saturating_sub(cursor) / FRAME_SAMPLES) * FRAME_SAMPLES;
    let mut energies = Vec::new();
    let mut start = first;
    while start + FRAME_SAMPLES <= total {
        if start >= band_lo && start + FRAME_SAMPLES <= band_hi {
            energies.push(frame_mean_square(samples, start));
        }
        start += FRAME_SAMPLES;
    }
    ((first - cursor) / FRAME_SAMPLES, energies)
}

/// Picks the boundary for the window starting at `cursor`.
///
/// Returns `None` when the remaining audio is too short to be worth a cut
/// (`total − cursor` is at or below the target, or the only cut would leave a
/// tail below [`MacroConfig::min_tail_ms`]).
pub fn select_cut(
    samples: &[f32],
    cursor: usize,
    total: usize,
    config: &MacroConfig,
) -> Option<CutChoice> {
    if cursor >= total {
        return None;
    }
    let target = config.target_samples();
    if total - cursor <= target {
        return None;
    }
    let band_lo = cursor + MacroConfig::ms_to_samples(config.search_start_ms);
    let band_hi = (cursor + MacroConfig::ms_to_samples(config.search_end_ms)).min(total);
    if band_lo >= band_hi {
        return None;
    }
    let (first_frame, energies) = band_frames(samples, cursor, total, band_lo, band_hi);

    // C1 — the longest silent run at or above the minimum duration.
    if !energies.is_empty() {
        let mut sorted = energies.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        // The percentile is taken over mean-square energies; the comparison is
        // on RMS, so the percentile is square-rooted before the factor applies.
        let threshold = (percentile(&sorted, f64::from(config.silence_percentile)).sqrt()
            * f64::from(config.silence_factor))
        .max(f64::from(config.silence_floor));
        let min_frames = (config.min_silence_ms / 20).max(1) as usize;

        let mut best: Option<(usize, usize)> = None; // (start_frame, len)
        let mut run_start: Option<usize> = None;
        for (index, energy) in energies.iter().map(|e| e.sqrt()).enumerate() {
            if energy < threshold {
                run_start.get_or_insert(index);
            } else if let Some(start) = run_start.take() {
                let len = index - start;
                if len >= min_frames && best.is_none_or(|(_, best_len)| len > best_len) {
                    best = Some((start, len));
                }
            }
        }
        if let Some(start) = run_start {
            let len = energies.len() - start;
            if len >= min_frames && best.is_none_or(|(_, best_len)| len > best_len) {
                best = Some((start, len));
            }
        }
        if let Some((start_frame, len)) = best {
            let run_lo = cursor + (first_frame + start_frame) * FRAME_SAMPLES;
            let run_hi = run_lo + len * FRAME_SAMPLES;
            let cut = run_lo + (run_hi - run_lo) / 2;
            return Some(CutChoice {
                cut_sample: cut,
                overlap_samples: MacroConfig::ms_to_samples(config.overlap_ms),
                kind: CutKind::Silence,
            });
        }
    }

    // C2 — no pause long enough: the quietest 200 ms in the narrowed band.
    let window_frames = (config.fallback_window_ms / 20).max(1) as usize;
    let window_samples = FRAME_SAMPLES * window_frames;
    let c2_lo = cursor + MacroConfig::ms_to_samples(config.fallback_start_ms);
    let c2_hi = (cursor + MacroConfig::ms_to_samples(config.fallback_end_ms)).min(total);
    let mut best: Option<(usize, f64)> = None; // (window start sample, mean square)
    let mut start = cursor;
    while start + window_samples <= total {
        if start >= c2_lo && start + window_samples <= c2_hi {
            let mean = (0..window_frames)
                .map(|frame| frame_mean_square(samples, start + frame * FRAME_SAMPLES))
                .sum::<f64>()
                / window_frames as f64;
            if best.is_none_or(|(_, best_mean)| mean < best_mean) {
                best = Some((start, mean));
            }
        }
        start += FRAME_SAMPLES;
    }
    let window_lo = best.map(|(start, _)| start).unwrap_or(c2_lo);
    let cut = window_lo + window_samples / 2;
    if cut <= cursor || total.saturating_sub(cut) < MacroConfig::ms_to_samples(config.min_tail_ms) {
        return None;
    }
    Some(CutChoice {
        cut_sample: cut.min(total),
        overlap_samples: MacroConfig::ms_to_samples(config.fallback_overlap_ms),
        kind: CutKind::MinimumEnergy,
    })
}

/// Plans the window list for a whole slice.
///
/// The first window starts at sample 0; each next one starts `overlap` samples
/// before the previous cut (whisper sees the seam twice, and the stitch picks
/// one copy). The last window always ends at `total_samples`.
pub fn plan_windows(
    samples: &[f32],
    total_samples: usize,
    config: &MacroConfig,
) -> Vec<MacroWindow> {
    let mut windows = Vec::new();
    let mut cursor = 0usize;
    let total = total_samples.min(samples.len());
    loop {
        if cursor >= total {
            break;
        }
        let Some(choice) = select_cut(samples, cursor, total, config) else {
            windows.push(MacroWindow {
                start_sample: cursor,
                end_sample: total,
                cut_sample: None,
                overlap_samples: 0,
                cut_without_pause: false,
            });
            break;
        };
        let next_cursor = choice.cut_sample.saturating_sub(choice.overlap_samples);
        let tail = total.saturating_sub(next_cursor);
        if next_cursor <= cursor || tail < MacroConfig::ms_to_samples(config.min_tail_ms) {
            // Fold the remaining audio into this window: a 20 s tail decode is
            // pure overhead and its cold start is a quality risk.
            windows.push(MacroWindow {
                start_sample: cursor,
                end_sample: total,
                cut_sample: None,
                overlap_samples: 0,
                cut_without_pause: false,
            });
            break;
        }
        windows.push(MacroWindow {
            start_sample: cursor,
            end_sample: choice.cut_sample,
            cut_sample: Some(choice.cut_sample),
            overlap_samples: choice.overlap_samples,
            cut_without_pause: choice.kind == CutKind::MinimumEnergy,
        });
        cursor = next_cursor;
    }
    windows
}

// ---------------------------------------------------------------------------
// Prompt firewall
// ---------------------------------------------------------------------------

/// Why a prompt tail was rejected — carried into logs and the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallReject {
    /// The tail had no text tokens.
    Empty,
    /// unique byte-4-grams / total byte-4-grams was at or below 0.45.
    LowDiversity,
    /// The mean token probability was below 0.45.
    LowProbability,
    /// A 1–6 word phrase appeared three times in a row.
    Repetition,
}

/// The firewall's verdict for one carried tail.
#[derive(Debug, Clone, PartialEq)]
pub enum PromptDecision {
    Carry(Vec<i32>),
    Drop(FirewallReject),
}

/// The most tokens a macro window may carry into the next one.
pub const MAX_PROMPT_TOKENS: usize = 160;
/// The unique byte-4-gram ratio a tail must beat.
pub const MIN_UNIQUE_4GRAM_RATIO: f64 = 0.45;
/// The mean token probability a tail must reach.
pub const MIN_MEAN_PROBABILITY: f32 = 0.45;
/// How many consecutive emissions of one phrase count as a loop.
pub const MAX_PHRASE_REPEATS: usize = 3;

/// `unique byte 4-grams / total byte 4-grams` over the tail text.
///
/// Short tails (< 4 bytes of text) are trivially diverse: there is nothing to
/// repeat, and the token-count check already bounds them.
pub fn unique_byte_4gram_ratio(text: &str) -> f64 {
    let bytes = text.as_bytes();
    if bytes.len() < 4 {
        return 1.0;
    }
    let mut grams: Vec<[u8; 4]> = Vec::with_capacity(bytes.len() - 3);
    for window in bytes.windows(4) {
        grams.push([window[0], window[1], window[2], window[3]]);
    }
    grams.sort_unstable();
    let unique = {
        let mut count = 0;
        let mut previous: Option<[u8; 4]> = None;
        for gram in &grams {
            if previous.as_ref() != Some(gram) {
                count += 1;
                previous = Some(*gram);
            }
        }
        count
    };
    unique as f64 / grams.len() as f64
}

/// Normalises one word for the firewall and the seam match: lowercase, and
/// only letters/digits survive (`"Hello,"` → `"hello"`).
pub fn normalise_word(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    for character in word.chars() {
        if character.is_alphanumeric() {
            for lower in character.to_lowercase() {
                out.push(lower);
            }
        }
    }
    out
}

fn normalised_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(normalise_word)
        .filter(|word| !word.is_empty())
        .collect()
}

/// True when the same 1–6 word phrase is emitted [`MAX_PHRASE_REPEATS`] times
/// in a row.
///
/// "Repeated" is read as *consecutive* repetition: that is what a decoder loop
/// looks like (`"Thank you. Thank you. Thank you."`), and counting
/// non-consecutive occurrences would reject every natural English tail — the
/// word `"the"` appears more than three times in 160 tokens. The report states
/// this operational definition.
pub fn has_phrase_loop(words: &[String]) -> bool {
    for phrase_len in 1..=6usize {
        if words.len() < phrase_len * MAX_PHRASE_REPEATS {
            break;
        }
        let mut index = 0usize;
        while index + phrase_len <= words.len() {
            let phrase = &words[index..index + phrase_len];
            let mut repeats = 1usize;
            let mut next = index + phrase_len;
            while next + phrase_len <= words.len() && &words[next..next + phrase_len] == phrase {
                repeats += 1;
                next += phrase_len;
            }
            if repeats >= MAX_PHRASE_REPEATS {
                return true;
            }
            index += 1;
        }
    }
    false
}

/// Applies the three checks to one candidate tail.
///
/// `tokens` are the text-token ids of the accepted, non-overlapped tail (in
/// reading order); `text` is what those tokens spell; `mean_probability` is the
/// mean `whisper_full_get_token_p` over the same tokens. The caller truncates
/// to [`MAX_PROMPT_TOKENS`] (and the function re-checks it).
pub fn prompt_firewall(tokens: &[i32], text: &str, mean_probability: f32) -> PromptDecision {
    if tokens.is_empty() {
        return PromptDecision::Drop(FirewallReject::Empty);
    }
    if tokens.len() > MAX_PROMPT_TOKENS {
        return PromptDecision::Drop(FirewallReject::Empty);
    }
    if unique_byte_4gram_ratio(text) <= MIN_UNIQUE_4GRAM_RATIO {
        return PromptDecision::Drop(FirewallReject::LowDiversity);
    }
    if mean_probability.is_nan() || mean_probability < MIN_MEAN_PROBABILITY {
        return PromptDecision::Drop(FirewallReject::LowProbability);
    }
    if has_phrase_loop(&normalised_words(text)) {
        return PromptDecision::Drop(FirewallReject::Repetition);
    }
    PromptDecision::Carry(tokens.to_vec())
}

// ---------------------------------------------------------------------------
// Seam stitch
// ---------------------------------------------------------------------------

/// How many words the seam match may span.
pub const MAX_SEAM_WORDS: usize = 12;
/// The minimum length (in characters) for the one-edit fuzzy match.
pub const FUZZY_MIN_CHARS: usize = 5;
/// A single-word match must land within this gap (milliseconds).
pub const SINGLE_WORD_MAX_GAP_MS: i64 = 800;

/// Levenshtein distance at most one: equal, one substitution, one insert or
/// one delete. Words shorter than [`FUZZY_MIN_CHARS`] must match exactly.
pub fn words_match(left: &str, right: &str) -> bool {
    let left = normalise_word(left);
    let right = normalise_word(right);
    if left == right {
        return true;
    }
    if left.chars().count() < FUZZY_MIN_CHARS || right.chars().count() < FUZZY_MIN_CHARS {
        return false;
    }
    let a: Vec<char> = left.chars().collect();
    let b: Vec<char> = right.chars().collect();
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    // One substitution: same length, exactly one differing position.
    if a.len() == b.len() {
        return a.iter().zip(&b).filter(|(x, y)| x != y).count() <= 1;
    }
    // One insertion/deletion: walk the longer string once.
    let (long, short) = if a.len() > b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    let mut i = 0usize;
    let mut j = 0usize;
    let mut edits = 0usize;
    while i < long.len() && j < short.len() {
        if long[i] == short[j] {
            i += 1;
            j += 1;
        } else {
            edits += 1;
            if edits > 1 {
                return false;
            }
            i += 1;
        }
    }
    edits + (long.len() - i) + (short.len() - j) <= 1
}

/// The seam decision: `prev_keep` words of the left window stay, and the first
/// `next_drop` words of the right window are removed (they were already said
/// by the left one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeamMatch {
    pub prev_keep: usize,
    pub next_drop: usize,
    /// How many words the common run covered (0 = only the timestamp gate).
    pub matched_words: usize,
}

/// Applies the two seam rules to one pair of word lists.
///
/// 1. **Midpoint gate**: the left keeps words whose midpoint is at or before
///    `cut − overlap/2`; the right keeps words whose midpoint is after it.
/// 2. **Boundary stitch**: among the last 12 kept left words and the first 12
///    kept right words, take the longest common suffix/prefix run (1–12 words,
///    one edit allowed for words of 5+ characters). A one-word match must be
///    exact and within [`SINGLE_WORD_MAX_GAP_MS`] of the left word's end.
///
/// The returned counts are indexes into the two input lists: `prev_keep` is the
/// length of the kept left prefix, `next_drop` the number of right words to
/// skip (the midpoint-gated ones plus the matched prefix).
pub fn stitch_seam(
    prev: &[WordTiming],
    next: &[WordTiming],
    cut_ms: i64,
    overlap_ms: i64,
) -> SeamMatch {
    let midpoint = cut_ms - overlap_ms / 2;
    let prev_keep = prev
        .iter()
        .take_while(|word| (word.start_ms + word.end_ms) / 2 <= midpoint)
        .count();
    let next_start = next
        .iter()
        .take_while(|word| (word.start_ms + word.end_ms) / 2 <= midpoint)
        .count();

    let left = &prev[prev_keep.saturating_sub(MAX_SEAM_WORDS)..prev_keep];
    let right = &next[next_start..next.len().min(next_start + MAX_SEAM_WORDS)];

    let mut matched = 0usize;
    for len in (1..=left.len().min(right.len())).rev() {
        let left_run = &left[left.len() - len..];
        let right_run = &right[..len];
        let all_match = left_run
            .iter()
            .zip(right_run)
            .all(|(a, b)| words_match(&a.text, &b.text));
        if !all_match {
            continue;
        }
        if len == 1 {
            // A one-word match is too weak for the fuzzy rule: require the same
            // word, close in time.
            if normalise_word(&left_run[0].text) != normalise_word(&right_run[0].text) {
                continue;
            }
            let gap = right_run[0].start_ms - left_run[0].end_ms;
            if !(gap.abs() < SINGLE_WORD_MAX_GAP_MS) {
                continue;
            }
        }
        matched = len;
        break;
    }

    SeamMatch {
        prev_keep,
        next_drop: next_start + matched,
        matched_words: matched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: f32, amplitude: f32) -> Vec<f32> {
        let count = (seconds * SAMPLE_RATE as f32) as usize;
        (0..count)
            .map(|index| {
                amplitude
                    * (2.0 * std::f32::consts::PI * 180.0 * index as f32 / SAMPLE_RATE as f32).sin()
            })
            .collect()
    }

    fn silence(seconds: f32) -> Vec<f32> {
        vec![0.0; (seconds * SAMPLE_RATE as f32) as usize]
    }

    fn cat(parts: &[Vec<f32>]) -> Vec<f32> {
        let mut out = Vec::new();
        for part in parts {
            out.extend_from_slice(part);
        }
        out
    }

    fn short_config() -> MacroConfig {
        MacroConfig {
            window_target_ms: 10_000,
            search_start_ms: 8_000,
            search_end_ms: 10_000,
            fallback_start_ms: 8_500,
            fallback_end_ms: 9_500,
            min_tail_ms: 2_000,
            ..MacroConfig::default()
        }
    }

    /// A burst pattern: 300 ms of loud tone, 100 ms at half amplitude. 25 % of
    /// the frames are quiet (so `percentile15` lands on them) but no quiet run
    /// reaches 400 ms, so C1 cannot fire and C2 must.
    fn bursty(seconds: f32) -> Vec<f32> {
        let mut out = Vec::new();
        let mut elapsed = 0.0f32;
        while elapsed < seconds {
            out.extend(tone(0.3, 0.3));
            out.extend(tone(0.1, 0.15));
            elapsed += 0.4;
        }
        out
    }

    #[test]
    fn c1_cuts_at_the_midpoint_of_the_longest_silent_run() {
        // 9 s tone, a 600 ms silence inside the 8–10 s band, then 2 s tone.
        let samples = cat(&[tone(8.6, 0.3), silence(0.6), tone(2.0, 0.3)]);
        let config = short_config();
        let cut = select_cut(&samples, 0, samples.len(), &config).expect("a cut");
        assert_eq!(cut.kind, CutKind::Silence);
        let cut_ms = (cut.cut_sample / SAMPLES_PER_MS) as i64;
        assert_eq!(cut_ms, 8_900, "C1 must cut at the run's midpoint");
        assert_eq!(cut.overlap_samples, 1_500 * SAMPLES_PER_MS);
    }

    #[test]
    fn c2_takes_the_quietest_200ms_and_flags_it() {
        // No 400 ms pause anywhere: C1 cannot fire.
        let samples = bursty(12.0);
        let config = short_config();
        let cut = select_cut(&samples, 0, samples.len(), &config).expect("a cut");
        assert_eq!(cut.kind, CutKind::MinimumEnergy);
        assert_eq!(cut.overlap_samples, 4_000 * SAMPLES_PER_MS);
        let cut_ms = (cut.cut_sample / SAMPLES_PER_MS) as i64;
        assert!(
            (8_700..=9_500).contains(&cut_ms),
            "C2 cut at {cut_ms} ms is outside the fallback band"
        );
    }

    #[test]
    fn planning_stops_at_the_final_window_and_overlaps_by_the_rule() {
        let samples = cat(&[tone(8.6, 0.3), silence(0.6), tone(7.8, 0.3)]);
        let config = short_config();
        let windows = plan_windows(&samples, samples.len(), &config);
        assert_eq!(windows.len(), 2, "one cut, two windows");
        let first = windows[0];
        let second = windows[1];
        assert_eq!(first.start_sample, 0);
        assert_eq!(
            second.start_sample,
            first.cut_sample.unwrap() - first.overlap_samples
        );
        assert_eq!(second.end_sample, samples.len());
        assert!(second.cut_sample.is_none());
        assert!(!first.cut_without_pause);
    }

    #[test]
    fn a_slice_at_or_below_the_target_is_one_window() {
        let samples = tone(10.0, 0.2);
        let config = short_config();
        let windows = plan_windows(&samples, samples.len(), &config);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].start_sample, 0);
        assert_eq!(windows[0].end_sample, samples.len());
        assert!(windows[0].cut_sample.is_none());
        assert!(!config.applies_to(samples.len()));
    }

    #[test]
    fn the_firewall_carries_a_clean_tail_and_rejects_a_loop() {
        let clean = "the committee approved the budget after a long debate about the \
                     northern rail extension and its cost";
        let tokens: Vec<i32> = (1..=20).collect();
        assert_eq!(
            prompt_firewall(&tokens, clean, 0.8),
            PromptDecision::Carry(tokens.clone())
        );

        let looped = "thank you thank you thank you thank you";
        assert!(matches!(
            prompt_firewall(&tokens, looped, 0.9),
            PromptDecision::Drop(FirewallReject::Repetition)
                | PromptDecision::Drop(FirewallReject::LowDiversity)
        ));

        assert_eq!(
            prompt_firewall(&tokens, clean, 0.40),
            PromptDecision::Drop(FirewallReject::LowProbability)
        );

        let repetitive = "aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa";
        assert!(matches!(
            prompt_firewall(&tokens, repetitive, 0.9),
            PromptDecision::Drop(FirewallReject::LowDiversity)
                | PromptDecision::Drop(FirewallReject::Repetition)
        ));

        assert_eq!(
            prompt_firewall(&[], clean, 0.9),
            PromptDecision::Drop(FirewallReject::Empty)
        );
        let too_many: Vec<i32> = (0..161).collect();
        assert_eq!(
            prompt_firewall(&too_many, clean, 0.9),
            PromptDecision::Drop(FirewallReject::Empty)
        );
    }

    #[test]
    fn the_unique_4gram_ratio_is_the_designs_measure() {
        assert!((unique_byte_4gram_ratio("abcd") - 1.0).abs() < 1e-9);
        // "aaaa" repeated: one unique 4-gram out of five.
        let ratio = unique_byte_4gram_ratio("aaaaaaaa");
        assert!((ratio - 1.0 / 5.0).abs() < 1e-9, "{ratio}");
        assert!((unique_byte_4gram_ratio("abc") - 1.0).abs() < 1e-9);
    }

    fn word(text: &str, start_ms: i64, end_ms: i64, segment: u32) -> WordTiming {
        WordTiming {
            segment_idx: segment,
            word_idx: 0,
            start_ms,
            end_ms,
            text: text.to_string(),
        }
    }

    #[test]
    fn the_midpoint_gate_splits_the_seam_zone() {
        // cut 100 s, overlap 1.5 s → midpoint 99.25 s.
        let prev = vec![
            word("alpha", 98_000, 98_400, 0),
            word("bravo", 99_000, 99_500, 0), // midpoint 99.25 → kept (<=)
            word("charlie", 99_400, 99_900, 0), // midpoint 99.65 → dropped
        ];
        // The right window re-hears "bravo" slightly late, so it slips past the
        // midpoint gate and the stitch has to remove it.
        let next = vec![
            word("bravo", 99_400, 99_800, 0), // midpoint 99.60 → kept by the gate
            word("charlie", 99_800, 100_200, 0),
            word("delta", 100_200, 100_600, 0),
        ];
        let seam = stitch_seam(&prev, &next, 100_000, 1_500);
        assert_eq!(seam.prev_keep, 2);
        assert_eq!(seam.next_drop, 1, "the late duplicate is dropped");
        assert_eq!(seam.matched_words, 1);
    }

    #[test]
    fn the_stitch_drops_a_duplicated_suffix_prefix_run() {
        let prev = vec![
            word("we", 97_800, 98_100, 0),
            word("reviewed", 98_100, 98_700, 0),
            word("the", 98_700, 99_200, 0),
        ];
        // A systematic timestamp shift puts all three re-heard words past the
        // midpoint; the gate cannot see them, the 3-word run does.
        let next = vec![
            word("we", 99_300, 99_600, 0),
            word("reviewed", 99_600, 100_200, 0),
            word("the", 100_200, 100_700, 0),
            word("budget", 100_900, 101_400, 0),
        ];
        let seam = stitch_seam(&prev, &next, 100_000, 1_500);
        assert_eq!(seam.prev_keep, 3);
        assert_eq!(seam.next_drop, 3);
        assert_eq!(seam.matched_words, 3);
    }

    #[test]
    fn a_single_word_match_needs_to_be_exact_and_close() {
        let prev = vec![word("Revenue", 98_000, 98_500, 0)];
        let close = vec![word("revenue", 99_260, 99_700, 0)];
        assert_eq!(stitch_seam(&prev, &close, 100_000, 1_500).matched_words, 1);

        let far = vec![word("revenue", 99_400, 99_800, 0)];
        assert_eq!(
            stitch_seam(&prev, &far, 100_000, 1_500).matched_words,
            0,
            "a 0.9 s gap is not a duplicate"
        );

        let fuzzy = vec![word("revenus", 99_260, 99_700, 0)];
        assert_eq!(
            stitch_seam(&prev, &fuzzy, 100_000, 1_500).matched_words,
            0,
            "one-word matches must be exact"
        );
    }

    #[test]
    fn fuzzy_matching_allows_one_edit_only_for_long_words() {
        assert!(words_match("committee", "commitee"));
        assert!(words_match("committee", "committees"));
        assert!(!words_match("committee", "comity"));
        assert!(!words_match("cat", "cut"), "short words must be exact");
        assert!(words_match("Cat,", "cat."), "punctuation is stripped");
    }
}
