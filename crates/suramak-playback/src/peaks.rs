//! Real waveform peaks (M9b): the min/max envelope of a decoded source.
//!
//! Computed **once per source load** in a single pass over the decoded samples
//! (trivial memory: `bucket_count` pairs), then drawn by the editor's deck strip
//! in the frozen M9a geometry. The envelope is the raw channel extremes of the
//! decoded buffer — for stereo material the interleaved stream's min/max (the
//! standard peak-meter view; no extra mixdown buffer is allocated).
//!
//! Two deliberate choices, both documented for the report:
//!
//! * **Clamping.** Samples are clamped to `[-1, 1]` when the envelope is built:
//!   float sources may legally exceed full scale, and the strip must never draw
//!   past its band. The `peak` the normalizer uses is the clamped maximum, so it
//!   is always in `0..=1`.
//! * **Normalization.** [`Peaks::normalized`] / [`Peaks::range`] scale by the
//!   file's own maximum (`1 / peak`) so a quiet recording and a loud one both
//!   read at a usable height — the shape is preserved, only the vertical scale
//!   is per-file. A silent file (peak ≈ 0) is left untouched: all-zero buckets,
//!   an honest flat baseline, never invented data. A percentile divisor was the
//!   considered alternative (robust against a single loud click); the max was
//!   chosen for the slice because it is exact and testable, and the choice is
//!   noted for a later refinement.

use std::path::Path;

use suramak_core::decode;

/// The default bucket count: ~1/1024 of the file per bucket — far more than the
/// strip's ~110 columns, so each drawn bar aggregates a real span of audio.
pub const DEFAULT_BUCKETS: usize = 1024;

/// The envelope below which a file counts as silent (no scaling is applied).
const SILENCE: f32 = 1e-6;

/// A source's min/max envelope, raw (clamped to `[-1, 1]`) plus the file peak
/// that normalization uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Peaks {
    buckets: Vec<(f32, f32)>,
    peak: f32,
}

impl Peaks {
    /// Builds the envelope of `samples` in one pass, `bucket_count` buckets
    /// spanning the whole buffer. Buckets that receive no samples (fewer samples
    /// than buckets) stay `(0.0, 0.0)`.
    pub fn from_samples(samples: &[f32], bucket_count: usize) -> Peaks {
        if bucket_count == 0 {
            return Peaks {
                buckets: Vec::new(),
                peak: 0.0,
            };
        }
        let mut buckets = vec![(0.0f32, 0.0f32); bucket_count];
        let len = samples.len();
        if len > 0 {
            for (index, bucket) in buckets.iter_mut().enumerate() {
                let start = index * len / bucket_count;
                let end = (index + 1) * len / bucket_count;
                if start >= end {
                    continue;
                }
                let mut min = f32::INFINITY;
                let mut max = f32::NEG_INFINITY;
                for &sample in &samples[start..end] {
                    let sample = sample.clamp(-1.0, 1.0);
                    min = min.min(sample);
                    max = max.max(sample);
                }
                *bucket = (min, max);
            }
        }
        let peak = buckets
            .iter()
            .fold(0.0f32, |acc, (min, max)| acc.max(max.abs()).max(min.abs()));
        Peaks { buckets, peak }
    }

    /// The raw (clamped) envelope, one `(min, max)` pair per bucket.
    pub fn buckets(&self) -> &[(f32, f32)] {
        &self.buckets
    }

    /// The file's maximum clamped magnitude (`0.0` for silence).
    pub fn peak(&self) -> f32 {
        self.peak
    }

    /// The envelope scaled to the file's own peak: quiet and loud sources both
    /// read at full height; silent files are returned untouched (all zeros).
    pub fn normalized(&self) -> Vec<(f32, f32)> {
        if self.peak <= SILENCE {
            return self.buckets.clone();
        }
        let scale = 1.0 / self.peak;
        self.buckets
            .iter()
            .map(|(min, max)| (min * scale, max * scale))
            .collect()
    }

    /// The **normalized** min/max over every bucket the time span `from_t..to_t`
    /// (fractions of the file, clamped to `0..=1`) touches — what one strip bar
    /// covering that span must draw. A span with no buckets is `(0.0, 0.0)`.
    pub fn range(&self, from_t: f32, to_t: f32) -> (f32, f32) {
        let count = self.buckets.len();
        if count == 0 {
            return (0.0, 0.0);
        }
        let from = from_t.clamp(0.0, 1.0) * count as f32;
        let to = to_t.clamp(0.0, 1.0) * count as f32;
        let start = (from.floor() as usize).min(count - 1);
        let end = ((to.ceil() as usize).max(start + 1)).min(count);
        let scale = if self.peak > SILENCE {
            1.0 / self.peak
        } else {
            1.0
        };
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for (bucket_min, bucket_max) in &self.buckets[start..end] {
            min = min.min(*bucket_min);
            max = max.max(*bucket_max);
        }
        (min * scale, max * scale)
    }

    /// Decodes `path` and builds its envelope — the "once, at source load" step
    /// the UI's peaks job runs off the main thread.
    pub fn from_file(path: &Path, bucket_count: usize) -> Result<Peaks, String> {
        let buffer = decode::decode_file(path).map_err(|err| err.to_string())?;
        Ok(Peaks::from_samples(&buffer.samples, bucket_count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_all_zeros() {
        let samples = vec![0.0f32; 4_096];
        let peaks = Peaks::from_samples(&samples, 64);
        assert_eq!(peaks.peak(), 0.0);
        assert!(peaks
            .buckets()
            .iter()
            .all(|(min, max)| *min == 0.0 && *max == 0.0));
        // Normalization never invents height for silence.
        assert_eq!(peaks.normalized(), peaks.buckets().to_vec());
        assert_eq!(peaks.range(0.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn a_constant_signal_is_flat_in_every_bucket() {
        let samples = vec![0.5f32; 1_024];
        let peaks = Peaks::from_samples(&samples, 16);
        assert!((peaks.peak() - 0.5).abs() < 1e-6);
        for (min, max) in peaks.buckets() {
            assert_eq!(*min, 0.5, "DC reads flat: min == the constant");
            assert_eq!(*max, 0.5);
        }
        // Normalized to the file peak, a DC signal is a full-height flat bar.
        assert!(peaks
            .normalized()
            .iter()
            .all(|(min, max)| (*min - 1.0).abs() < 1e-6 && (*max - 1.0).abs() < 1e-6));
    }

    #[test]
    fn out_of_range_samples_clamp_to_full_scale() {
        let samples = vec![2.0f32, -3.0, 0.25, 1.5];
        let peaks = Peaks::from_samples(&samples, 2);
        assert_eq!(
            peaks.buckets()[0],
            (-1.0, 1.0),
            "clamped to ±1 before the extremes are taken"
        );
        assert_eq!(peaks.buckets()[1], (0.25, 1.0));
        assert!(
            peaks.peak() <= 1.0,
            "the normalizer never exceeds full scale"
        );
        let normalized = peaks.normalized();
        assert!(normalized
            .iter()
            .all(|(min, max)| (-1.0..=1.0).contains(min) && (-1.0..=1.0).contains(max)));
    }

    #[test]
    fn bucket_counts_hold_at_the_edges() {
        // Zero buckets: nothing to store.
        assert!(Peaks::from_samples(&[0.5; 32], 0).buckets().is_empty());
        // Empty samples: a full grid of zeros (the honest baseline).
        let empty = Peaks::from_samples(&[], 8);
        assert_eq!(empty.buckets().len(), 8);
        assert!(empty.buckets().iter().all(|b| *b == (0.0, 0.0)));
        // Fewer samples than buckets: bucket `i` covers the span
        // `[i·len/n, (i+1)·len/n)` of the buffer, so 4 samples over 8 buckets
        // land in buckets 1, 3, 5, 7 — the empty buckets stay zero, no samples
        // are invented.
        let peaks = Peaks::from_samples(&[1.0, -1.0, 0.5, -0.5], 8);
        assert_eq!(peaks.buckets().len(), 8);
        assert_eq!(peaks.buckets()[1], (1.0, 1.0));
        assert_eq!(peaks.buckets()[3], (-1.0, -1.0));
        assert_eq!(peaks.buckets()[5], (0.5, 0.5));
        assert_eq!(peaks.buckets()[7], (-0.5, -0.5));
        assert_eq!(peaks.buckets()[0], (0.0, 0.0));
        assert_eq!(peaks.buckets()[2], (0.0, 0.0));
        // Exact division: every bucket sees exactly its slice.
        let samples: Vec<f32> = (0..8).map(|i| i as f32 / 8.0).collect();
        let peaks = Peaks::from_samples(&samples, 4);
        assert_eq!(peaks.buckets()[0], (0.0, 0.125));
        assert_eq!(peaks.buckets()[1], (0.25, 0.375));
        assert_eq!(peaks.buckets()[2], (0.5, 0.625));
        assert_eq!(peaks.buckets()[3], (0.75, 0.875));
    }

    #[test]
    fn normalization_scales_quiet_files_and_keeps_loud_ones() {
        // A quiet recording (peak 0.1): the loudest half normalizes to full
        // height, and the shape survives proportionally.
        let quiet = Peaks::from_samples(&[0.1, -0.05, 0.05, -0.025], 2);
        assert!((quiet.peak() - 0.1).abs() < 1e-6);
        assert_eq!(quiet.buckets()[0], (-0.05, 0.1));
        assert_eq!(quiet.buckets()[1], (-0.025, 0.05));
        let normalized = quiet.normalized();
        assert!(
            (normalized[0].1 - 1.0).abs() < 1e-6,
            "the file peak → full height"
        );
        assert!((normalized[0].0 + 0.5).abs() < 1e-6, "-0.05 → -0.5");
        assert!((normalized[1].1 - 0.5).abs() < 1e-6, "0.05 → 0.5");
        assert!((normalized[1].0 + 0.25).abs() < 1e-6, "-0.025 → -0.25");
        // A loud file at full scale is untouched.
        let loud = Peaks::from_samples(&[1.0, -1.0, 1.0, -1.0], 2);
        assert_eq!(loud.normalized(), loud.buckets().to_vec());
        // The ratio between two buckets is the same before and after scaling.
        let ratio_before = quiet.buckets()[1].1 / quiet.buckets()[0].1;
        let ratio_after = normalized[1].1 / normalized[0].1;
        assert!((ratio_before - ratio_after).abs() < 1e-6);
    }

    #[test]
    fn range_aggregates_the_buckets_a_span_touches() {
        // Buckets: [0.2], [0.5, 0.6], [0.4], [-0.9, -0.3]; peak = 0.9.
        let samples = vec![0.2, 0.5, 0.6, 0.4, -0.9, -0.3];
        let peaks = Peaks::from_samples(&samples, 4);
        assert!((peaks.peak() - 0.9).abs() < 1e-6);
        // The full span is the file's own normalized extremes.
        let (min, max) = peaks.range(0.0, 1.0);
        assert!((max - 0.6 / 0.9).abs() < 1e-6, "the loudest bucket, scaled");
        assert!((min + 1.0).abs() < 1e-6, "the -0.9 bucket normalizes to -1");
        // The first quarter covers bucket 0 only.
        let (min, max) = peaks.range(0.0, 0.25);
        assert!((min - 0.2 / 0.9).abs() < 1e-6 && (max - 0.2 / 0.9).abs() < 1e-6);
        // A span crossing buckets aggregates their extremes.
        let (min, max) = peaks.range(0.4, 0.9);
        assert!((max - 0.6 / 0.9).abs() < 1e-6, "0.5/0.6 buckets included");
        assert!((min + 1.0).abs() < 1e-6, "the -0.9 bucket included");
        // Clamped outside 0..=1; a degenerate span still reads one bucket.
        assert_eq!(peaks.range(2.0, 3.0), peaks.range(0.75, 1.0));
        let (min, max) = peaks.range(0.5, 0.5);
        assert!(min <= max);
        // An empty envelope has no range at all.
        assert_eq!(Peaks::from_samples(&[], 0).range(0.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn a_real_wav_file_decodes_into_a_usable_envelope() {
        // The repo's fixture: 9 s, 16 kHz mono. If it is absent (a fresh clone
        // without fixtures) the test is a no-op, never a failure.
        let path = std::path::Path::new("../../fixtures/audio/en.wav");
        if !path.is_file() {
            return;
        }
        let peaks = Peaks::from_file(path, DEFAULT_BUCKETS).expect("decode the fixture");
        assert_eq!(peaks.buckets().len(), DEFAULT_BUCKETS);
        assert!(peaks.peak() > 0.01, "the fixture carries speech");
        let normalized = peaks.normalized();
        assert!(
            normalized.iter().any(|(min, max)| max - min > 0.5),
            "some part of the file reads as real signal"
        );
        assert!(normalized
            .iter()
            .all(|(min, max)| *min >= -1.0 && *max <= 1.0));
    }
}
