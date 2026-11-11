//! M16.4-S1 — VAD-driven chunking for long recordings.
//!
//! **Feature `vad-chunking`, default OFF.** Nothing in the capture path calls
//! this module, no service or settings surface exists yet (slice 5 owns the
//! settings); the pure chunker plus the join rule are host-tested against
//! whisper in `tests/vad_join.rs`. Builds without the feature are unchanged
//! (the module is not compiled at all).
//!
//! # The three invariants this module exists to hold
//!
//! 1. **512 samples per VAD call, always.** `sherpa-onnx`'s VAD ORs every
//!    window inside one `AcceptWaveform` call before deciding "speech", so a
//!    pause shorter than the call is invisible to the segmentation. M16DF
//!    measured 1 s blocks merging 13 segments into 8 and shifting starts by up
//!    to 960 ms; 512 samples (32 ms at 16 kHz) is the model's native window and
//!    the only size that keeps the segment list exact. [`feed_schedule`] is the
//!    single place that decides block sizes, and its unit test pins every block
//!    to exactly [`FEED_SAMPLES`].
//! 2. **A cut never lands inside speech.** Chunks close only at a VAD segment
//!    end, i.e. inside a pause the VAD already resolved to ≥
//!    [`VadConfig::min_silence_duration`]. The cut is the VAD's own end, which
//!    M16DF measured ~116 ms past the last energy-gated voiced frame (the VAD
//!    keeps the decay tail) and ≥ 906 ms before the next voiced frame at the
//!    production pause floor. There is no extra constant added after the end.
//! 3. **Chunk boundaries are reproducible.** The VAD is deterministic at a
//!    fixed window size (M16DF: 3/3 runs identical to the millisecond) and
//!    [`chunks_from_segments`] is a pure function of the segment list, so the
//!    same PCM yields byte-identical chunk spans on every run.
//!
//! # Chunk policy
//!
//! Whisper pads every call to a 30 s window, so chunk cadence — not VAD cost —
//! decides the CPU bill (M16DF §4.5: 10 s chunks cost 1.93× a full pass, 30 s
//! chunks 1.15×). The policy is therefore: grow a chunk until it carries at
//! least [`VadConfig::min_chunk_ms`] of audio **and** the next pause is at least
//! [`VadConfig::pause_ms`]; close at the first segment end past
//! [`VadConfig::max_chunk_ms`] when no such pause appears; never split a
//! segment (a >14 s monologue becomes one long chunk rather than a cut through
//! speech). Length is measured on the chunk's own audio — the samples whisper
//! actually decodes — so a chunk that starts with several seconds of pause
//! still lands inside the target band. The soft maximum can overshoot by one
//! segment, because a cut may only land at a VAD segment end. Each chunk starts
//! [`VadConfig::pre_roll_ms`] before **its own first word**, so the pause
//! between the previous cut and that start is never decoded: a 10 s silent head
//! makes whisper under-transcribe the speech after it (measured — see
//! `docs/reports/M16d1-vad-join-gate.md`). A pause shorter than the pre-roll
//! makes the next chunk start before the previous cut; that overlap is
//! re-decoded and dropped by [`join_decoded`] under M7's rule.
//!
//! # Join rule (M7's skip-and-merge, reused verbatim)
//!
//! `live.rs` drops a segment whose end lies within [`MERGE_TOLERANCE_MS`] of
//! the covered frontier; here the same rule drops the re-decoded pre-roll of
//! the next chunk:
//!
//! ```text
//! for each decoded segment s of chunk k (timestamps shifted by chunk.start_ms):
//!     if s.text is empty or s.end_ms <= chunk.core_start_ms + MERGE_TOLERANCE_MS: drop
//!     else: append with idx = session.segments.len()
//! ```
//!
//! The difference from M7 is what the tolerance protects: M7 drops a
//! re-decoded window head, this drops the padded lead-in of the next chunk.
//! M7 is safe because the final pass replaces the provisional list; a
//! chunk-only pipeline has no such backstop, which is why `tests/vad_join.rs`
//! measures dropped/duplicated phrases at the joins before any capture work.
//!
//! # The model file
//!
//! `silero_vad.onnx` v5.1.2 (2 327 524 B, sha256
//! `2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f`, MIT) is
//! **not committed**. Host tests locate it through `SURAMAK_VAD_MODEL_PATH` and
//! skip when it is absent (the alignment test's model gate). The model manager
//! that pins, verifies and ships it is M16.4 slice 4.
//!
//! Running the gated tests on Linux needs the prebuilt sherpa-onnx shared
//! libraries on the loader path (they are copied next to the test binary, one
//! directory up from `deps/`):
//!
//! ```text
//! SURAMAK_VAD_MODEL_PATH=/tmp/silero_vad_v5.1.2.onnx \
//! LD_LIBRARY_PATH=target/debug \
//! cargo test -p suramak-asr --features vad-chunking --test vad_join -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};

use suramak_core::transcript::Segment;

use crate::live::MERGE_TOLERANCE_MS;

/// Samples per VAD call: Silero v5's native window at 16 kHz (32 ms).
pub const FEED_SAMPLES: usize = 512;

/// Sample rate of the chunker's input — the pipeline's one recording rate.
pub const SAMPLE_RATE: u32 = 16_000;

/// A chunk must reach this length before a pause may close it. Below ~10 s
/// whisper's per-call overhead (a padded 30 s encode) dominates the CPU bill.
pub const MIN_CHUNK_MS: u32 = 10_000;

/// Soft upper bound: past this, close at the first segment end even if the
/// pause is shorter than [`CLOSE_PAUSE_MS`]. A single longer utterance is never
/// split — the VAD's `max_speech_duration` owns that case.
pub const MAX_CHUNK_MS: u32 = 14_000;

/// Pause length that closes a chunk once it is at least [`MIN_CHUNK_MS`] long.
pub const CLOSE_PAUSE_MS: u32 = 700;

/// Audio carried before a chunk's first word for decoder context. It matches
/// M7's window overlap: whisper loses or misreads the first word of a window
/// that starts at the speech boundary (measured — 500 ms still lost "the
/// quick" at a chunk head, 1 500 ms did not), and the extra audio costs one
/// padded encode, not a second call.
pub const PRE_ROLL_MS: u32 = 1_500;

/// One voiced span the VAD reported, in stream samples (half-open).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VadSegment {
    pub start: usize,
    pub end: usize,
}

impl VadSegment {
    /// Length in samples.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Whether the span is empty (no audio).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One chunk of audio to decode, in stream samples (half-open).
///
/// `start..end` is the audio handed to the decoder; `core_start..end` is the
/// part that carries new content. `core_start` is the previous chunk's cut
/// (0 for the first chunk), so `start..core_start` is the re-decoded pre-roll
/// that [`join_decoded`] drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VadChunk {
    pub start: usize,
    pub core_start: usize,
    pub end: usize,
}

impl VadChunk {
    /// Chunk start in session milliseconds.
    pub fn start_ms(&self) -> i64 {
        samples_to_ms(self.start)
    }

    /// First new-content sample in session milliseconds.
    pub fn core_start_ms(&self) -> i64 {
        samples_to_ms(self.core_start)
    }

    /// Cut point in session milliseconds.
    pub fn end_ms(&self) -> i64 {
        samples_to_ms(self.end)
    }

    /// Audio length in samples.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Whether the chunk carries no audio.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The chunk's audio, clamped to `pcm`.
    pub fn slice<'a>(&self, pcm: &'a [f32]) -> &'a [f32] {
        let end = self.end.min(pcm.len());
        let start = self.start.min(end);
        &pcm[start..end]
    }
}

fn samples_to_ms(samples: usize) -> i64 {
    (samples as i64) * 1000 / SAMPLE_RATE as i64
}

/// Chunker configuration. Defaults are the M16DF production pins.
#[derive(Debug, Clone, PartialEq)]
pub struct VadConfig {
    /// VAD threshold (M16DF: 0.4–0.6 is the stable band, 0.5 the pin).
    pub threshold: f32,
    /// Silence that ends a VAD segment. 0.30 s is the probe's pin.
    pub min_silence_duration: f32,
    /// Shortest voiced span the VAD reports. **0.10 s**: at 0.25 s the
    /// fixture's deliberate 0.300 s cue is dropped entirely (a real
    /// dropped-text mode for interjections, M16DF §4.2).
    pub min_speech_duration: f32,
    /// Longest single VAD segment; the VAD force-splits past this.
    pub max_speech_duration: f32,
    /// VAD ring buffer, in seconds. `max_speech_duration` + 1 s holds the
    /// longest expected utterance at ~2 MB instead of the probe's 3.84 MB.
    pub buffer_seconds: f32,
    /// VAD worker threads (one is enough at 121 µs/block).
    pub num_threads: i32,
    /// Minimum chunk length before a pause may close it.
    pub min_chunk_ms: u32,
    /// Soft maximum; close at the next segment end past it.
    pub max_chunk_ms: u32,
    /// Pause length that closes a chunk at or past `min_chunk_ms`.
    pub pause_ms: u32,
    /// Audio carried before `core_start`.
    pub pre_roll_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: 0.50,
            min_silence_duration: 0.30,
            min_speech_duration: 0.10,
            max_speech_duration: 30.0,
            buffer_seconds: 31.0,
            num_threads: 1,
            min_chunk_ms: MIN_CHUNK_MS,
            max_chunk_ms: MAX_CHUNK_MS,
            pause_ms: CLOSE_PAUSE_MS,
            pre_roll_ms: PRE_ROLL_MS,
        }
    }
}

impl VadConfig {
    fn samples(&self, ms: u32) -> usize {
        (ms as usize) * SAMPLE_RATE as usize / 1000
    }
}

/// Why the VAD could not run.
#[derive(Debug, thiserror::Error)]
pub enum VadError {
    /// The model file named by the caller does not exist. Checked before the
    /// C API is called: sherpa-onnx returns a null detector for a missing file
    /// and every later call on it would be undefined behaviour.
    #[error("VAD model not found at {0}")]
    ModelMissing(PathBuf),
    /// sherpa-onnx refused to construct the detector (bad model, ORT failure).
    #[error("VAD model could not be loaded: {0}")]
    Model(String),
}

/// The exact 512-sample feed schedule for `total_samples`.
///
/// Returns `(start, len)` pairs where `len == FEED_SAMPLES` for **every** block
/// — the final block is zero-padded past the end of the audio, because a short
/// final call would be a different (smaller) window and the VAD's per-call
/// "any window voiced" OR would change. Pairs are contiguous and start at
/// multiples of [`FEED_SAMPLES`].
pub fn feed_schedule(total_samples: usize) -> Vec<(usize, usize)> {
    let blocks = total_samples.div_ceil(FEED_SAMPLES);
    (0..blocks)
        .map(|i| (i * FEED_SAMPLES, FEED_SAMPLES))
        .collect()
}

/// One feed block's audio: the real samples plus zero padding for the tail.
fn feed_block(samples: &[f32], start: usize, len: usize) -> Vec<f32> {
    let mut block = vec![0.0f32; len];
    if start < samples.len() {
        let end = (start + len).min(samples.len());
        block[..end - start].copy_from_slice(&samples[start..end]);
    }
    block
}

/// Drains every finished segment from the detector into `out`.
fn drain(vad: &mut sherpa_rs::silero_vad::SileroVad, out: &mut Vec<VadSegment>) {
    while !vad.is_empty() {
        let segment = vad.front();
        vad.pop();
        let start = segment.start.max(0) as usize;
        let end = start + segment.samples.len();
        if end > start {
            out.push(VadSegment { start, end });
        }
    }
}

/// Runs the Silero VAD over 16 kHz mono PCM and returns its voiced spans.
///
/// Every `accept_waveform` call carries exactly [`FEED_SAMPLES`] samples (see
/// [`feed_schedule`]); segments are drained after each call and once after
/// `flush()` (which emits the tail of an utterance still open at the end of the
/// audio). The result is ordered, disjoint and clamped to the audio length.
pub fn segments_with_model(
    model_path: &Path,
    samples: &[f32],
    config: &VadConfig,
) -> Result<Vec<VadSegment>, VadError> {
    if !model_path.is_file() {
        return Err(VadError::ModelMissing(model_path.to_path_buf()));
    }
    let mut vad = sherpa_rs::silero_vad::SileroVad::new(
        sherpa_rs::silero_vad::SileroVadConfig {
            model: model_path.to_string_lossy().into_owned(),
            min_silence_duration: config.min_silence_duration,
            min_speech_duration: config.min_speech_duration,
            max_speech_duration: config.max_speech_duration,
            threshold: config.threshold,
            sample_rate: SAMPLE_RATE,
            // Never configurable: the model is only valid at its 512-sample window.
            window_size: FEED_SAMPLES as i32,
            provider: None,
            num_threads: Some(config.num_threads),
            debug: false,
        },
        config.buffer_seconds,
    )
    .map_err(|error| VadError::Model(error.to_string()))?;

    let mut segments = Vec::new();
    for (start, len) in feed_schedule(samples.len()) {
        vad.accept_waveform(feed_block(samples, start, len));
        drain(&mut vad, &mut segments);
    }
    vad.flush();
    drain(&mut vad, &mut segments);

    Ok(normalize_segments(segments, samples.len()))
}

/// Orders, de-overlaps and clamps the VAD's spans.
///
/// sherpa-onnx emits segments in stream order and disjoint, but the chunker's
/// policy is only meaningful on a disjoint list; this makes the precondition
/// true rather than assumed.
fn normalize_segments(mut segments: Vec<VadSegment>, total: usize) -> Vec<VadSegment> {
    segments.sort_by_key(|segment| segment.start);
    let mut out: Vec<VadSegment> = Vec::with_capacity(segments.len());
    for segment in segments {
        let start = segment.start.min(total);
        let end = segment.end.min(total);
        if end <= start {
            continue;
        }
        match out.last_mut() {
            // An overlap means the detector re-reported audio; keep the union.
            Some(previous) if start < previous.end => previous.end = previous.end.max(end),
            _ => out.push(VadSegment { start, end }),
        }
    }
    out
}

/// Turns the VAD's voiced spans into decode chunks — pure, no model needed.
///
/// The policy is documented on the module: grow until `min_chunk_ms` **and** a
/// pause of at least `pause_ms`, close at the first segment end past
/// `max_chunk_ms` otherwise, never cut inside a segment, always cut at a
/// segment end. `total_samples` bounds the final cut.
///
/// Preconditions (established by [`segments_with_model`]): `segments` is ordered
/// and disjoint. Defensive: inverted or empty spans are skipped and every span
/// is clamped to `total_samples`.
pub fn chunks_from_segments(
    segments: &[VadSegment],
    total_samples: usize,
    config: &VadConfig,
) -> Vec<VadChunk> {
    let min_chunk = config.samples(config.min_chunk_ms);
    let max_chunk = config.samples(config.max_chunk_ms);
    let pause = config.samples(config.pause_ms);
    let pre_roll = config.samples(config.pre_roll_ms);

    let spans: Vec<VadSegment> = segments
        .iter()
        .filter_map(|segment| {
            let start = segment.start.min(total_samples);
            let end = segment.end.min(total_samples);
            (end > start).then_some(VadSegment { start, end })
        })
        .collect();
    if spans.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut first = 0;
    // End of the previous chunk's audio (the cut); 0 for the first chunk.
    let mut covered = 0usize;
    while first < spans.len() {
        // Every chunk starts `pre_roll` before **its own first word**. The
        // pause between the previous cut and this start is silence the VAD
        // already resolved, so it is never decoded: a chunk that begins with
        // 10 s of silence makes whisper under-transcribe the speech that
        // follows (measured on `m16df-60s-enonly`, see the M16D1 report).
        // When the pause is shorter than the pre-roll the start lands before
        // the previous cut and the overlap is re-decoded and dropped at the
        // join — M7's exact rule.
        let start = spans[first].start.saturating_sub(pre_roll);
        let mut last = first;
        while last + 1 < spans.len() {
            let span = spans[last].end.saturating_sub(start);
            let gap = spans[last + 1].start.saturating_sub(spans[last].end);
            if (span >= min_chunk && gap >= pause) || span >= max_chunk {
                break;
            }
            last += 1;
        }
        let cut = spans[last].end.min(total_samples);
        chunks.push(VadChunk {
            start,
            core_start: covered,
            end: cut,
        });
        covered = cut;
        first = last + 1;
    }
    chunks
}

/// Runs the VAD and returns the decode chunks for `samples` (16 kHz mono f32).
pub fn chunk_pcm(
    model_path: &Path,
    samples: &[f32],
    config: &VadConfig,
) -> Result<Vec<VadChunk>, VadError> {
    let segments = segments_with_model(model_path, samples, config)?;
    Ok(chunks_from_segments(&segments, samples.len(), config))
}

/// One chunk's decoded segments (chunk-relative milliseconds, as whisper
/// reports them).
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkDecode {
    pub chunk: VadChunk,
    pub segments: Vec<Segment>,
}

/// Joins per-chunk decodes into one session-timeline segment list — M7's rule.
///
/// * timestamps are translated by `chunk.start_ms` (the chunk's audio offset,
///   pre-roll included);
/// * a segment that is empty, degenerate, or ends within
///   [`MERGE_TOLERANCE_MS`] of the chunk's `core_start` is dropped — it is the
///   re-decode of audio the previous chunk already covered;
/// * everything else is appended with `idx = out.len()`, so the list is
///   append-only and ordered.
pub fn join_decoded(decoded: &[ChunkDecode]) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    for decode in decoded {
        let offset = decode.chunk.start_ms();
        let core_start = decode.chunk.core_start_ms();
        for segment in &decode.segments {
            let text = segment.text.trim();
            if text.is_empty() {
                continue;
            }
            let start_ms = offset + segment.start_ms;
            let end_ms = offset + segment.end_ms;
            if end_ms <= start_ms {
                continue;
            }
            if end_ms <= core_start + MERGE_TOLERANCE_MS {
                continue;
            }
            out.push(Segment {
                idx: out.len() as u32,
                start_ms,
                end_ms,
                text: text.to_string(),
            });
        }
    }
    out
}

/// Total audio a chunk list covers, for diagnostics (`end - start` each).
pub fn chunk_audio_ms(chunks: &[VadChunk]) -> i64 {
    chunks
        .iter()
        .map(|chunk| chunk.end_ms() - chunk.start_ms())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start_s: f32, end_s: f32) -> VadSegment {
        VadSegment {
            start: (start_s * SAMPLE_RATE as f32) as usize,
            end: (end_s * SAMPLE_RATE as f32) as usize,
        }
    }

    #[test]
    fn the_feed_is_exactly_512_samples_per_call() {
        for total in [0usize, 1, 511, 512, 513, 16_000, 96_000, 960_000] {
            let schedule = feed_schedule(total);
            assert_eq!(schedule.len(), total.div_ceil(FEED_SAMPLES));
            for (index, (start, len)) in schedule.iter().enumerate() {
                assert_eq!(
                    *len, FEED_SAMPLES,
                    "block {index} of {total} is not 512 wide"
                );
                assert_eq!(
                    *start,
                    index * FEED_SAMPLES,
                    "block {index} is not contiguous"
                );
            }
            if total > 0 {
                let (last_start, last_len) = *schedule.last().expect("non-empty");
                assert!(last_start < total, "last block starts past the audio");
                assert!(
                    last_start + last_len >= total,
                    "the padded tail must cover the last sample"
                );
            }
        }
    }

    #[test]
    fn the_padded_tail_is_zero_filled_and_keeps_the_audio_prefix() {
        let samples: Vec<f32> = (0..600).map(|i| i as f32 / 600.0).collect();
        let block = feed_block(&samples, 512, FEED_SAMPLES);
        assert_eq!(block.len(), FEED_SAMPLES);
        assert_eq!(block[..88], samples[512..600]);
        assert!(
            block[88..].iter().all(|s| *s == 0.0),
            "tail must be silence"
        );
    }

    #[test]
    fn default_config_pins_the_feasibility_values() {
        let config = VadConfig::default();
        assert_eq!(config.threshold, 0.50);
        assert_eq!(config.min_silence_duration, 0.30);
        // 0.10 recovers the fixture's 0.300 s cue (dropped at 0.25).
        assert_eq!(config.min_speech_duration, 0.10);
        assert_eq!(config.max_speech_duration, 30.0);
        assert_eq!(config.min_chunk_ms, 10_000);
        assert_eq!(config.max_chunk_ms, 14_000);
        assert_eq!(config.pause_ms, 700);
        assert_eq!(config.pre_roll_ms, 1_500);
        assert_eq!(FEED_SAMPLES, 512);
        assert_eq!(SAMPLE_RATE, 16_000);
    }

    #[test]
    fn a_pause_before_the_minimum_is_crossed_rather_than_cut() {
        // Three utterances with 0.4 s pauses between them and a 0.8 s pause
        // after: the 0.4 s pauses are below the 700 ms floor, so the chunk
        // spans all three and closes only at the 0.8 s pause (10.4 s).
        let segments = [
            seg(0.0, 3.2),
            seg(3.6, 6.8),
            seg(7.2, 10.4),
            seg(11.2, 14.2),
        ];
        let chunks = chunks_from_segments(&segments, 16_000 * 20, &VadConfig::default());
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].end_ms(), 10_400);
        assert_eq!(chunks[1].core_start_ms(), 10_400);
        assert_eq!(chunks[1].end_ms(), 14_200);
    }

    #[test]
    fn a_chunk_closes_at_the_first_pause_after_ten_seconds() {
        // 3.0 s speech + 0.8 s pause × 4 → the fourth segment ends at 14.4 s,
        // but the pause after the third (at 11.4 s) is the first one at or past
        // the 10 s floor, so the chunk closes there.
        let segments = [
            seg(0.0, 3.0),
            seg(3.8, 6.8),
            seg(7.6, 10.6),
            seg(11.4, 14.4),
            seg(15.2, 18.2),
        ];
        let chunks = chunks_from_segments(&segments, 16_000 * 25, &VadConfig::default());
        assert_eq!(chunks.len(), 2, "10.6 s then the rest");
        assert_eq!(chunks[0].end_ms(), 10_600);
        assert_eq!(chunks[1].end_ms(), 18_200);
    }

    #[test]
    fn a_short_cue_is_kept_in_a_chunk_and_never_dropped() {
        // The M16DF 0.300 s interjection plus neighbours: every span must be
        // inside exactly one chunk's core, including the short one.
        let segments = [
            seg(0.0, 4.0),
            seg(4.7, 8.7),
            seg(9.4, 9.7),
            seg(10.4, 14.4),
            seg(15.2, 19.2),
        ];
        let chunks = chunks_from_segments(&segments, 16_000 * 21, &VadConfig::default());
        for span in segments {
            let owners: Vec<_> = chunks
                .iter()
                .filter(|chunk| span.start >= chunk.core_start && span.end <= chunk.end)
                .collect();
            assert_eq!(owners.len(), 1, "span {span:?} must have exactly one owner");
        }
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].end_ms(), 14_400);
    }

    /// The configured pre-roll in milliseconds (assertions below stay valid if
    /// the default is tuned).
    fn pre() -> i64 {
        VadConfig::default().pre_roll_ms as i64
    }

    #[test]
    fn every_chunk_starts_at_its_own_first_word_and_the_cut_is_the_frontier() {
        let segments = [seg(0.0, 11.0), seg(11.8, 22.8), seg(23.6, 34.6)];
        let chunks = chunks_from_segments(&segments, 16_000 * 40, &VadConfig::default());
        assert_eq!(chunks.len(), 3);
        // The pre-roll before each chunk's first word, never before 0.
        assert_eq!(chunks[0].start_ms(), 0);
        assert_eq!(chunks[1].start_ms(), 11_800 - pre());
        assert_eq!(chunks[2].start_ms(), 23_600 - pre());
        for pair in chunks.windows(2) {
            let previous = &pair[0];
            let next = &pair[1];
            assert_eq!(
                next.core_start, previous.end,
                "the cut is the new core start"
            );
            // An 800 ms pause is shorter than the 1 500 ms pre-roll: at most
            // one pre-roll of audio is re-decoded, never more.
            assert!(next.start >= previous.end - 24_000);
        }
        // The silence between the cut and the next pre-roll is never decoded.
        assert_eq!(chunks[0].end_ms(), 11_000);
        assert_eq!(chunks[1].core_start_ms(), 11_000);
    }

    #[test]
    fn a_pause_longer_than_the_pre_roll_leaves_no_overlap() {
        // 2 s pause vs 1.5 s pre-roll: the next chunk starts after the cut, so
        // the pause's tail is skipped rather than re-decoded.
        let segments = [seg(0.0, 14.5), seg(16.5, 25.0)];
        let chunks = chunks_from_segments(&segments, 16_000 * 30, &VadConfig::default());
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].end_ms(), 14_500);
        assert_eq!(chunks[1].core_start_ms(), 14_500);
        assert_eq!(chunks[1].start_ms(), 16_500 - pre());
        assert!(chunks[1].start > chunks[0].end, "no overlap to drop");
    }

    #[test]
    fn a_long_monologue_is_one_chunk_not_a_cut_through_speech() {
        // 20 s of continuous speech: past the 14 s soft max, but the policy
        // must not cut inside a segment. The VAD's own max_speech splits at 30 s.
        let segments = [seg(0.0, 20.0), seg(21.0, 24.0)];
        let chunks = chunks_from_segments(&segments, 16_000 * 26, &VadConfig::default());
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].end_ms(), 20_000);
        assert_eq!(
            chunks[1].start_ms(),
            21_000 - pre(),
            "the pre-roll into the long chunk, after the pause"
        );
    }

    #[test]
    fn the_last_chunk_ends_at_the_last_voiced_frame() {
        let segments = [seg(2.0, 12.0)];
        let chunks = chunks_from_segments(&segments, 16_000 * 60, &VadConfig::default());
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].start_ms(),
            2_000 - pre(),
            "pre-roll before speech"
        );
        assert_eq!(
            chunks[0].end_ms(),
            12_000,
            "trailing silence is not decoded"
        );
    }

    #[test]
    fn silence_yields_no_chunks() {
        assert!(chunks_from_segments(&[], 16_000 * 5, &VadConfig::default()).is_empty());
        let degenerate = [VadSegment {
            start: 800,
            end: 800,
        }];
        assert!(chunks_from_segments(&degenerate, 16_000 * 5, &VadConfig::default()).is_empty());
    }

    #[test]
    fn chunking_is_deterministic_across_three_runs() {
        let segments = [
            seg(1.0, 12.5),
            seg(13.2, 24.2),
            seg(25.0, 36.0),
            seg(36.7, 47.7),
        ];
        let config = VadConfig::default();
        let first = chunks_from_segments(&segments, 16_000 * 60, &config);
        for _ in 0..2 {
            assert_eq!(
                chunks_from_segments(&segments, 16_000 * 60, &config),
                first,
                "the same segment list must yield identical chunks"
            );
        }
        assert!(chunk_audio_ms(&first) > 0);
    }

    #[test]
    fn every_chunk_holds_at_least_its_core_and_never_overlaps_its_own_core() {
        let segments = [
            seg(0.0, 2.0),
            seg(2.7, 5.2),
            seg(6.0, 12.4),
            seg(13.1, 20.0),
            seg(20.8, 31.0),
        ];
        let chunks = chunks_from_segments(&segments, 16_000 * 35, &VadConfig::default());
        for chunk in &chunks {
            // The start may sit after core_start: the pause between the previous
            // cut and this chunk's pre-roll is silence and is not decoded.
            assert!(chunk.start < chunk.end);
            assert!(chunk.core_start < chunk.end);
        }
        for pair in chunks.windows(2) {
            assert!(pair[0].end <= pair[1].end, "cuts advance monotonically");
        }
    }

    fn decoded(chunk: VadChunk, text: &str, start_ms: i64, end_ms: i64) -> ChunkDecode {
        ChunkDecode {
            chunk,
            segments: vec![Segment {
                idx: 0,
                start_ms,
                end_ms,
                text: text.to_string(),
            }],
        }
    }

    #[test]
    fn the_join_drops_the_redecoded_pre_roll_and_keeps_new_text() {
        let chunk0 = VadChunk {
            start: 0,
            core_start: 0,
            end: 160_000,
        };
        let chunk1 = VadChunk {
            start: 152_000,
            core_start: 160_000,
            end: 320_000,
        };
        // Chunk 1 starts 500 ms before core_start, so stream-relative 0 is
        // stream 9 500 ms and its first 500 ms are the previous chunk's tail.
        let joined = join_decoded(&[
            decoded(chunk0, "first phrase.", 1_000, 10_000),
            // Absolute end 10 000 ms == core_start: the overlap re-decode →
            // dropped by the 50 ms tolerance (end <= core_start + 50).
            decoded(chunk1, "first phrase.", 0, 500),
            // Straddles core_start: new content → kept.
            decoded(chunk1, "second phrase.", 600, 4_500),
        ]);
        assert_eq!(joined.len(), 2);
        assert_eq!(joined[0].text, "first phrase.");
        assert_eq!(joined[0].start_ms, 1_000);
        assert_eq!(joined[0].idx, 0);
        assert_eq!(joined[1].text, "second phrase.");
        assert_eq!(joined[1].start_ms, 10_100, "shifted by the chunk's start");
        assert_eq!(joined[1].idx, 1);
    }

    #[test]
    fn the_join_drops_empty_and_degenerate_segments() {
        let chunk = VadChunk {
            start: 0,
            core_start: 0,
            end: 160_000,
        };
        let joined = join_decoded(&[
            decoded(chunk, "   ", 0, 500),
            decoded(chunk, "zero length", 500, 500),
            decoded(chunk, " kept ", 600, 900),
        ]);
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].text, "kept", "text is trimmed");
    }

    #[test]
    fn a_long_chunk_selects_a_short_audio_slice() {
        let chunk = VadChunk {
            start: 1_000,
            core_start: 9_000,
            end: 11_000,
        };
        let pcm = vec![0.5f32; 12_000];
        assert_eq!(chunk.slice(&pcm).len(), 10_000);
        assert_eq!(
            chunk.slice(&pcm[..10_500]).len(),
            9_500,
            "clamped to the PCM"
        );
        assert!(chunk.slice(&pcm[..500]).is_empty());
    }
}
