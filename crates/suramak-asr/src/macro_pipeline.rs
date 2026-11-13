//! M26C — the macro-window decode loop over one mapped [`PcmSlice`].
//!
//! This module is the **driver**: it plans the windows with
//! [`crate::macro_chunk`], creates **one** [`WindowSession`] for the whole job,
//! decodes each window, applies the prompt firewall to the accepted tail,
//! stitches the seam, and reports progress by samples consumed.
//!
//! The single-call path is not touched: [`crate::pipeline::transcribe_file`]
//! only enters here when the mapped slice is longer than the 15-minute window
//! target **and** [`MACRO_WINDOWS_ENABLED`] is true, so a shorter file keeps
//! whisper's one call and its byte-identical transcript. The flag is `false`
//! because the M26C gate failed (see the constant and the report).
//!
//! Memory: the mapped PCM pages of a completed window are dropped with
//! `madvise(MADV_DONTNEED)` ([`PcmSlice::advise_dontneed`]) before the next
//! window starts. The mel spectrogram whisper allocates is bounded by the
//! window length (≤ 15 min), not by the file.

use std::path::Path;

use suramak_core::pcm::PcmSlice;
use suramak_core::settings::LangMode;
use suramak_core::transcript::{Segment, WordTiming};
use whisper_rs::WhisperContext;

use crate::cancel::CancelToken;
use crate::engine::{EngineDecision, EngineId};
use crate::macro_chunk::{
    plan_windows, prompt_firewall, stitch_seam, FirewallReject, MacroConfig, MacroWindow,
    PromptDecision, MAX_PROMPT_TOKENS, SAMPLES_PER_MS,
};
use crate::whisper::{AsrError, TextToken, WindowSession};

/// What happened at one window boundary — the report's cut table.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowReport {
    pub index: usize,
    pub start_ms: i64,
    pub end_ms: i64,
    pub cut_ms: Option<i64>,
    pub overlap_ms: i64,
    pub cut_without_pause: bool,
    /// Tokens the firewall passed to the next window (0 when it refused).
    pub carried_tokens: usize,
    /// Why the firewall refused the tail, if it did.
    pub prompt_reject: Option<FirewallReject>,
    /// Distinct token ids actually carried (for the report's provenance).
    pub first_carried_token: Option<i32>,
}

/// The macro path's result.
#[derive(Debug, Clone)]
pub struct MacroOutcome {
    pub segments: Vec<Segment>,
    pub words: Vec<WordTiming>,
    pub engine_used: EngineId,
    pub warnings: Vec<String>,
    pub windows: Vec<WindowReport>,
}

/// Whether the macro path is enabled in production.
///
/// **`false`: the M26C gate did not pass.** On the 64.8-minute natural corpus
/// the macro path moved WER by −0.62 pt (better, but the gate is symmetric
/// ±0.5), the segment count by −7.1 % (the gate allows 2 %), and the 90-minute
/// tiny peak was 553 MB (the gate allows 450 MB, although it was flat to
/// −0.6 MB against the 30-minute run). The repeated clip produced no loop.
/// `docs/reports/M26c-macro-windows.md` holds the numbers and the STOP
/// decision. The implementation stays in the tree, exercised by
/// `tests/m26c_macro_windows.rs`, so a later slice can retry without
/// rediscovering the mechanism — but production keeps the single call and the
/// M26A Phase-1 duration cap until a run passes all four gates.
pub const MACRO_WINDOWS_ENABLED: bool = false;

/// True when this slice must take the macro path.
pub fn applies(total_samples: usize) -> bool {
    MACRO_WINDOWS_ENABLED && MacroConfig::default().applies_to(total_samples)
}

/// The absolute mid-sample of the accepted tail for one window.
fn window_midpoint_ms(start_sample: usize, window: &MacroWindow) -> Option<i64> {
    let base_ms = (start_sample / SAMPLES_PER_MS) as i64;
    window.seam_midpoint_ms().map(|midpoint| base_ms + midpoint)
}

/// Rebuilds one window's segments and words after the seam/gate rules.
///
/// * `word_start` words at the front of `words` are dropped (the stitch's
///   duplicated prefix); every segment they covered is dropped with them.
/// * Remaining words must pass `keep_word`; a segment whose words are all
///   dropped disappears, and a segment that keeps only some words is rebuilt
///   from them (joined text, first/last word times).
/// * A segment with **no** word timing (whisper could not map its tokens) has
///   no word-level evidence: it is kept when `keep_segment` passes and it lies
///   after the first kept word's segment.
fn finalise_window(
    segments: &[Segment],
    words: &[WordTiming],
    word_start: usize,
    base_idx: u32,
    keep_word: &dyn Fn(&WordTiming) -> bool,
    keep_segment: &dyn Fn(&Segment) -> bool,
) -> (Vec<Segment>, Vec<WordTiming>) {
    let word_start = word_start.min(words.len());
    let first_segment = if word_start == 0 {
        0
    } else {
        words
            .get(word_start)
            .map(|word| word.segment_idx)
            .unwrap_or(u32::MAX)
    };
    let mut out_segments: Vec<Segment> = Vec::new();
    let mut out_words: Vec<WordTiming> = Vec::new();

    for segment in segments {
        if segment.idx < first_segment {
            continue; // entirely inside the dropped duplicate prefix
        }
        let segment_words: Vec<&WordTiming> = words[word_start..]
            .iter()
            .filter(|word| word.segment_idx == segment.idx)
            .collect();
        if segment_words.is_empty() {
            if keep_segment(segment) {
                out_segments.push(Segment {
                    idx: base_idx + out_segments.len() as u32,
                    ..segment.clone()
                });
            }
            continue;
        }
        let kept: Vec<&WordTiming> = segment_words
            .into_iter()
            .filter(|word| keep_word(word))
            .collect();
        if kept.is_empty() {
            continue;
        }
        let idx = base_idx + out_segments.len() as u32;
        let total_words = words
            .iter()
            .filter(|word| word.segment_idx == segment.idx)
            .count();
        let (text, start_ms, end_ms) = if kept.len() == total_words {
            (segment.text.clone(), segment.start_ms, segment.end_ms)
        } else {
            (
                kept.iter()
                    .map(|word| word.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
                kept.first()
                    .map(|word| word.start_ms)
                    .unwrap_or(segment.start_ms),
                kept.last()
                    .map(|word| word.end_ms)
                    .unwrap_or(segment.end_ms),
            )
        };
        out_segments.push(Segment {
            idx,
            start_ms,
            end_ms,
            text,
        });
        for (word_idx, word) in kept.into_iter().enumerate() {
            out_words.push(WordTiming {
                segment_idx: idx,
                word_idx: word_idx as u32,
                ..word.clone()
            });
        }
    }
    (out_segments, out_words)
}

/// The tokens of one window whose midpoint is at or before `midpoint_ms`.
fn accepted_tail_tokens(tokens: &[TextToken], midpoint_ms: i64) -> Vec<&TextToken> {
    tokens
        .iter()
        .filter(|token| token.start_ms >= 0 && (token.start_ms + token.end_ms) / 2 <= midpoint_ms)
        .collect()
}

/// Builds the next window's prompt: the last ≤160 accepted tokens, gated by
/// the firewall. The text is the exact bytes those tokens spell.
fn prompt_for_next(tail: &[&TextToken]) -> (Vec<i32>, Option<FirewallReject>) {
    let start = tail.len().saturating_sub(MAX_PROMPT_TOKENS);
    let tail = &tail[start..];
    let ids: Vec<i32> = tail.iter().map(|token| token.id).collect();
    let mut bytes = Vec::new();
    for token in tail {
        bytes.extend_from_slice(&token.bytes);
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let mean_probability = if tail.is_empty() {
        0.0
    } else {
        tail.iter().map(|token| token.probability).sum::<f32>() / tail.len() as f32
    };
    match prompt_firewall(&ids, &text, mean_probability) {
        PromptDecision::Carry(tokens) => (tokens, None),
        PromptDecision::Drop(reason) => (Vec::new(), Some(reason)),
    }
}

/// Decodes a whole slice as 15-minute macro-windows.
///
/// `from_ms` starts the first window mid-file (a resumed job); the window plan
/// is then relative to that cursor. Progress is by samples consumed across
/// windows, and cancellation is checked at every window boundary in addition
/// to whisper's own abort callback inside a window.
#[allow(clippy::too_many_arguments)]
pub fn transcribe_macro(
    ctx: &WhisperContext,
    model_path: &Path,
    samples: &PcmSlice,
    lang: &LangMode,
    initial_prompt: Option<&str>,
    n_threads: i32,
    plan: &EngineDecision,
    cancel: &CancelToken,
    from_ms: i64,
    next_idx: u32,
    on_progress: &mut dyn FnMut(u8),
    on_segment: &mut dyn FnMut(Segment),
) -> Result<MacroOutcome, AsrError> {
    let all = samples.as_slice();
    let start_sample = usize::try_from(from_ms.max(0))
        .unwrap_or(usize::MAX)
        .saturating_mul(SAMPLES_PER_MS)
        .min(all.len());
    let view = &all[start_sample..];
    let config = MacroConfig::default();
    let windows = plan_windows(view, view.len(), &config);
    if windows.is_empty() {
        return Err(AsrError::Inference("no macro windows planned".to_string()));
    }

    let mut session = WindowSession::new(ctx, model_path, plan)?;
    let mut segments: Vec<Segment> = Vec::new();
    let mut words: Vec<WordTiming> = Vec::new();
    let mut reports: Vec<WindowReport> = Vec::with_capacity(windows.len());
    let mut previous_words: Vec<WordTiming> = Vec::new();
    let mut previous_cut_ms = 0i64;
    let mut previous_overlap_ms = 0i64;
    let total = all.len();
    let mut last_pct = 0u8;

    let mut prompt_tokens: Vec<i32> = if start_sample == 0 {
        match initial_prompt {
            Some(text) => {
                let tokens = ctx
                    .tokenize(text, MAX_PROMPT_TOKENS)
                    .map_err(|e| AsrError::Inference(e.to_string()))?;
                let start = tokens.len().saturating_sub(MAX_PROMPT_TOKENS);
                tokens[start..].to_vec()
            }
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };

    for (index, window) in windows.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(AsrError::Cancelled);
        }
        let window_start = start_sample + window.start_sample;
        let window_end = start_sample + window.end_sample;
        let offset_ms = (window_start / SAMPLES_PER_MS) as i64;
        let midpoint = window_midpoint_ms(start_sample, window);

        let mut on_window_progress = |pct: u8| {
            let span = window_end.saturating_sub(window_start) as u64;
            let done = window_start as u64 + span * u64::from(pct.min(100)) / 100;
            let global = ((done * 100) / total.max(1) as u64).min(100) as u8;
            let value = global.max(last_pct);
            last_pct = value;
            on_progress(value);
        };
        let mut discard = |_: Segment| {};
        let outcome = session.transcribe(
            ctx,
            &all[window_start..window_end],
            lang,
            &prompt_tokens,
            n_threads,
            cancel,
            offset_ms,
            next_idx + segments.len() as u32,
            &mut on_window_progress,
            &mut discard,
        )?;
        // The PCM of a completed window is not needed again: drop its resident
        // pages before the next window's mel is allocated.
        samples.advise_dontneed(window_start, window_end);

        // 1. The stitch: the previous window's accepted tail already said these
        //    words, so the right window's duplicated prefix goes.
        let mut word_start = 0usize;
        if index > 0 {
            let seam = stitch_seam(
                &previous_words,
                &outcome.words,
                previous_cut_ms,
                previous_overlap_ms,
            );
            word_start = seam.next_drop.min(outcome.words.len());
        }

        // 2. The midpoint gate: words at or before `cut − overlap/2` belong to
        //    this window, later ones to the next.
        let (window_segments, window_words) = match midpoint {
            Some(midpoint) => finalise_window(
                &outcome.segments,
                &outcome.words,
                word_start,
                next_idx + segments.len() as u32,
                &|word| (word.start_ms + word.end_ms) / 2 <= midpoint,
                &|segment| (segment.start_ms + segment.end_ms) / 2 <= midpoint,
            ),
            None => finalise_window(
                &outcome.segments,
                &outcome.words,
                word_start,
                next_idx + segments.len() as u32,
                &|_| true,
                &|_| true,
            ),
        };

        // 3. The prompt for the next window: only the accepted, non-overlapped
        //    tail, and only when the firewall passes it.
        let (next_prompt, prompt_reject, carried, first_carried) = match midpoint {
            Some(midpoint) => {
                let tail = accepted_tail_tokens(&outcome.tokens, midpoint);
                let (tokens, reject) = prompt_for_next(&tail);
                (
                    tokens.clone(),
                    reject,
                    tokens.len(),
                    tokens.first().copied(),
                )
            }
            None => (Vec::new(), None, 0, None),
        };

        for segment in &window_segments {
            on_segment(segment.clone());
        }
        previous_words = window_words.clone();
        segments.extend(window_segments);
        words.extend(window_words);
        previous_cut_ms = window
            .cut_ms()
            .map(|cut| (start_sample / SAMPLES_PER_MS) as i64 + cut)
            .unwrap_or(0);
        previous_overlap_ms = (window.overlap_samples / SAMPLES_PER_MS) as i64;
        prompt_tokens = next_prompt;
        reports.push(WindowReport {
            index,
            start_ms: (window_start / SAMPLES_PER_MS) as i64,
            end_ms: (window_end / SAMPLES_PER_MS) as i64,
            cut_ms: window
                .cut_ms()
                .map(|cut| (start_sample / SAMPLES_PER_MS) as i64 + cut),
            overlap_ms: previous_overlap_ms,
            cut_without_pause: window.cut_without_pause,
            carried_tokens: carried,
            prompt_reject,
            first_carried_token: first_carried,
        });
    }

    on_progress(100);
    Ok(MacroOutcome {
        segments,
        words,
        engine_used: session.engine_used(),
        warnings: session.warnings().to_vec(),
        windows: reports,
    })
}
