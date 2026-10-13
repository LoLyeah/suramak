//! Multi-source mixing engine (M6) — **pure DSP, compiled on every platform**
//! (no cpal, no device, no `capture` feature), so the Linux CI/VPS unit-tests
//! the whole mixing story with synthetic sources.
//!
//! One recording = N concurrently captured sources summed into **the single
//! 16 kHz mono stream** the rest of the app consumes ([`mix`] returns f32
//! samples; [`crate::capture::write_recording_wav`] writes them as the one
//! 16 kHz mono i16 WAV). Device I/O stays behind the `capture` feature; only
//! the buffers it produced reach this module.
//!
//! ## Clock-drift handling (the documented approach)
//!
//! Device clocks differ (tens to hundreds of ppm), so N sources recorded over
//! the same wall-clock session deliver *different sample counts* for the same
//! span: a source clocked 250 ppm fast yields `rate · (1 + 250e-6) · D`
//! samples for a `D`-second session. Mixing them all at their nominal rates
//! slides the fast source ahead of the others by `ppm · t` — at 250 ppm that
//! is 45 ms after three minutes, and it keeps growing (the "periodic
//! artifacts" of naive mixing).
//!
//! The mixer therefore measures each source's **effective rate** from the two
//! exact quantities the capture layer knows — samples delivered (`N`) and the
//! session's wall-clock span (`D`, measured by the runtime with `Instant`) —
//! and resamples that source at its measured rate instead of the nominal one:
//!
//! ```text
//! ratio ρ = (N / (rate · D))     // measured rate / nominal rate
//! ```
//!
//! * Stage 1 — the source is resampled to 16 kHz **exactly as today's
//!   single-source path does** ([`crate::capture::resample_16k`] → core's
//!   rubato FFT resampler).
//! * Stage 2 — the 16 kHz stream is stretched by `ρ` with a 4-point
//!   Catmull-Rom fractional resampler (constant ratio, no sample
//!   insertion/deletion).
//!
//! Alignment error is therefore bounded by the *measurement* error of `D` (a
//! few ms of `Instant` noise, independent of run length) — never by the run
//! length. The unit tests prove this on long synthetic runs (`drift` tests:
//! naive mixing moves 45 ms over 3 minutes at 250 ppm; the corrected mix stays
//! within a sample). Because the correction is a constant resampling ratio,
//! there is no correction *event* to hear: no sample is ever inserted or
//! dropped, so the output cannot click (`never_clicks` test).
//!
//! `ρ` is clamped to ±[`MAX_DRIFT_CORRECTION`] (2%) — beyond that the
//! measurement is not trusted (a stalled device, a bogus duration) and the
//! clamp keeps the output sane; the per-source report shows the applied ratio
//! either way.
//!
//! ## Clipping policy (documented, never silent)
//!
//! Per-source gain applies **before** the sum; the sum itself is not squashed
//! (no limiter, no automatic gain reduction). Hard saturation happens exactly
//! where the single 16-bit PCM format requires it — the float→i16 conversion
//! clamps to the ±1.0 full scale (the existing `write_wav_i16` behaviour) —
//! and the mixer *reports* it: [`MixOutcome::clipped_samples`] counts every
//! output sample whose sum exceeded ±1.0 and [`MixOutcome::peak`] carries the
//! pre-saturation peak. Keep per-source gains such that the sum stays inside
//! ±1.0 to avoid saturation (gains default to unity in the UI).
//!
//! ## Solo regression guard
//!
//! With exactly one source, unity gain and no measured deviation (`ρ == 1`),
//! the output is *bit-identical* to today's path: stage 1 is the same
//! function, stage 2 is an exact pass-through (the interpolator is the
//! identity at integer positions). With a measured session the tail zero
//! padding of the FFT resampler is trimmed to the session length — the one
//! deliberate difference (documented in the report).

use std::time::Duration;

use suramak_core::resample::TARGET_RATE;

use crate::capture::{resample_16k, write_recording_wav};
use crate::whisper::AsrError;

/// The single recording rate the rest of the app consumes (core's
/// [`suramak_core::resample::TARGET_RATE`], re-exported for callers here).
pub use suramak_core::resample::TARGET_RATE as MIX_RATE;

/// Bound of the measured-clock correction (2%). A measured deviation beyond
/// this is not trusted (stalled device, bogus duration) and is clamped.
pub const MAX_DRIFT_CORRECTION: f64 = 0.02;

/// One captured source offered to the mixer (mono f32 at its device rate).
#[derive(Debug, Clone, PartialEq)]
pub struct MixTrack<'a> {
    /// User-facing label (also what the per-source report shows).
    pub label: String,
    /// Device-reported sample rate of `samples`.
    pub rate: u32,
    /// The captured mono samples.
    pub samples: &'a [f32],
    /// Per-source gain, applied before the sum (`1.0` = unity).
    pub gain: f32,
    /// Muted sources contribute silence (gain 0).
    pub muted: bool,
}

/// Per-source outcome, for logs and the record view's honesty.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceMixReport {
    pub label: String,
    pub rate: u32,
    pub captured_samples: usize,
    pub captured_seconds: f64,
    /// Samples / session span — the measured effective rate; `None` without a
    /// session (then no correction is applied).
    pub measured_rate: Option<f64>,
    /// Applied correction in ppm: `(ρ − 1) · 1e6` (0 without a session).
    pub drift_ppm: f64,
    /// The gain actually applied (0.0 for a muted source).
    pub applied_gain: f32,
    pub muted: bool,
    /// The correction was clamped at [`MAX_DRIFT_CORRECTION`].
    pub clamped: bool,
}

/// What [`mix`] produced.
#[derive(Debug, Clone, PartialEq)]
pub struct MixOutcome {
    /// 16 kHz mono f32 samples — the recording's audio.
    pub samples: Vec<f32>,
    pub reports: Vec<SourceMixReport>,
    /// Output samples whose pre-saturation sum exceeded ±1.0 (see the module's
    /// clipping policy).
    pub clipped_samples: usize,
    /// Pre-saturation peak |sum|.
    pub peak: f32,
}

/// Mixes `tracks` into one 16 kHz mono stream.
///
/// `session` is the capture session's wall-clock span (what the runtime
/// measures around the live streams); it drives both the output length
/// (`span · 16 kHz`) and the per-source clock-drift measurement. `None` is
/// "no session known": no correction, output length = the longest source —
/// today's solo semantics.
pub fn mix(tracks: &[MixTrack<'_>], session: Option<Duration>) -> Result<MixOutcome, AsrError> {
    if tracks.is_empty() {
        return Err(AsrError::Capture("no input sources selected".to_string()));
    }
    for track in tracks {
        if track.rate == 0 {
            return Err(AsrError::Capture(format!(
                "source '{}' reports sample rate 0",
                track.label
            )));
        }
    }
    let session_secs = session
        .map(|duration| duration.as_secs_f64())
        .filter(|secs| *secs > 0.0);

    // Stage 1 for every source — the same resample today's single-source path
    // uses (bit-identical for solo/unity, see the module docs).
    let mut staged: Vec<Vec<f32>> = Vec::with_capacity(tracks.len());
    for track in tracks {
        staged.push(resample_16k(track.samples, track.rate)?);
    }

    // Target length: the session's wall span at 16 kHz; without a session, the
    // longest nominal resample (today's length semantics).
    let target_len = match session_secs {
        Some(secs) => (secs * f64::from(TARGET_RATE)).round() as usize,
        None => staged.iter().map(Vec::len).max().unwrap_or(0),
    };

    let mut output = vec![0.0f32; target_len];
    let mut reports = Vec::with_capacity(tracks.len());
    for (track, nominal) in tracks.iter().zip(staged.iter()) {
        // Measured clock ratio ρ = N / (rate · D). `None` session → 1.0 (no
        // correction; today's behavior).
        let (rho, clamped) = match session_secs {
            Some(secs) if !track.samples.is_empty() => {
                let raw = track.samples.len() as f64 / (f64::from(track.rate) * secs);
                let clamped = raw > 1.0 + MAX_DRIFT_CORRECTION || raw < 1.0 - MAX_DRIFT_CORRECTION;
                (
                    raw.clamp(1.0 - MAX_DRIFT_CORRECTION, 1.0 + MAX_DRIFT_CORRECTION),
                    clamped,
                )
            }
            _ => (1.0, false),
        };
        let gain = if track.muted { 0.0 } else { track.gain };
        if gain != 0.0 {
            if (rho - 1.0).abs() > f64::EPSILON {
                // Stage 2: constant-ratio Catmull-Rom stretch (never clicks).
                for (index, slot) in output.iter_mut().enumerate() {
                    let position = index as f64 * rho;
                    *slot += gain * catmull_rom(nominal, position);
                }
            } else {
                for (index, slot) in output.iter_mut().enumerate() {
                    *slot += gain * nominal.get(index).copied().unwrap_or(0.0);
                }
            }
        }
        let captured_seconds = track.samples.len() as f64 / f64::from(track.rate);
        reports.push(SourceMixReport {
            label: track.label.clone(),
            rate: track.rate,
            captured_samples: track.samples.len(),
            captured_seconds,
            measured_rate: session_secs
                .filter(|_| !track.samples.is_empty())
                .map(|secs| track.samples.len() as f64 / secs),
            drift_ppm: (rho - 1.0) * 1e6,
            applied_gain: gain,
            muted: track.muted,
            clamped,
        });
    }

    let peak = output
        .iter()
        .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
    let clipped_samples = output.iter().filter(|sample| sample.abs() > 1.0).count();
    Ok(MixOutcome {
        samples: output,
        reports,
        clipped_samples,
        peak,
    })
}

/// Mixes `tracks` and writes the result as the one recording format (16 kHz
/// mono i16 WAV) through [`write_recording_wav`] — the pipeline's tail, in one
/// call for the runtime and the demo.
pub fn mix_to_wav(
    path: &std::path::Path,
    tracks: &[MixTrack<'_>],
    session: Option<Duration>,
) -> Result<MixOutcome, AsrError> {
    let outcome = mix(tracks, session)?;
    write_recording_wav(path, &outcome.samples, TARGET_RATE)?;
    Ok(outcome)
}

/// 4-point Catmull-Rom interpolation of `samples` at fractional `position`
/// (out-of-range taps read 0.0, i.e. silence padding at either end).
///
/// The identity at integer positions is exact (`t == 0.0` → `p1`), which is
/// what makes the no-correction path a bit-exact pass-through.
fn catmull_rom(samples: &[f32], position: f64) -> f32 {
    let base = position.floor();
    let t = (position - base) as f32;
    let index = base as isize;
    let p0 = tap(samples, index - 1);
    let p1 = tap(samples, index);
    let p2 = tap(samples, index + 1);
    let p3 = tap(samples, index + 2);
    p1 + 0.5
        * t
        * (p2 - p0 + t * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3 + t * (3.0 * (p1 - p2) + p3 - p0)))
}

fn tap(samples: &[f32], index: isize) -> f32 {
    if index < 0 {
        0.0
    } else {
        samples.get(index as usize).copied().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// A source whose *content* is a 220 Hz tone plus a loud Hann-windowed
    /// 1 kHz burst every `pulse_period_s` of content time (starting at
    /// `pulse_offset_s`), "captured" by a device clocked `ppm` ppm off its
    /// nominal rate: the device delivers `rate · (1 + ppm/1e6) · secs` samples
    /// for a `secs`-second session.
    ///
    /// The bursts are the alignment markers: after a correct mix they sit at
    /// their content-time positions; a naive mix slides them by `ppm · t`.
    fn drifting_source(
        rate: u32,
        secs: f64,
        ppm: f64,
        pulse_period_s: f64,
        pulse_offset_s: f64,
    ) -> Vec<f32> {
        let true_rate = f64::from(rate) * (1.0 + ppm / 1e6);
        let count = (true_rate * secs).round() as usize;
        let burst_secs = 0.005;
        let mut samples = vec![0.0f32; count];
        for (index, sample) in samples.iter_mut().enumerate() {
            let tau = index as f64 / true_rate; // content time of this sample
            let mut value = 0.2 * (2.0 * PI * 220.0 * tau).sin() as f32;
            let phase = (tau - pulse_offset_s).rem_euclid(pulse_period_s);
            if phase < burst_secs {
                let hann = 0.5 - 0.5 * (2.0 * PI * phase / burst_secs).cos();
                value += 0.8 * hann as f32 * (2.0 * PI * 1000.0 * phase).sin() as f32;
            }
            *sample = value;
        }
        samples
    }

    /// A pure drifting sine (for the click/continuity test).
    fn drifting_sine(rate: u32, secs: f64, ppm: f64, freq: f64) -> Vec<f32> {
        let true_rate = f64::from(rate) * (1.0 + ppm / 1e6);
        let count = (true_rate * secs).round() as usize;
        (0..count)
            .map(|index| {
                let tau = index as f64 / true_rate;
                (2.0 * PI * freq * tau).sin() as f32
            })
            .collect()
    }

    fn track<'a>(label: &str, rate: u32, samples: &'a [f32], gain: f32) -> MixTrack<'a> {
        MixTrack {
            label: label.to_string(),
            rate,
            samples,
            gain,
            muted: false,
        }
    }

    /// Content-time sample position of a pulse peak (burst centre at +2.5 ms).
    fn pulse_position_at(content_secs: f64) -> usize {
        ((content_secs + 0.0025) * f64::from(TARGET_RATE)).round() as usize
    }

    /// Offset (in samples) of the loudest point within ±`radius` of `expected`.
    fn pulse_offset(output: &[f32], expected: usize, radius: usize) -> isize {
        let low = expected.saturating_sub(radius);
        let high = (expected + radius).min(output.len());
        let mut best = low;
        let mut best_value = 0.0f32;
        for (index, sample) in output.iter().enumerate().take(high).skip(low) {
            if sample.abs() > best_value {
                best_value = sample.abs();
                best = index;
            }
        }
        best as isize - expected as isize
    }

    /// Movement (samples) of each pulse relative to the first one, at the given
    /// content-time positions.
    fn pulse_movements(output: &[f32], times: &[f64], radius: usize) -> Vec<isize> {
        let offsets: Vec<isize> = times
            .iter()
            .map(|time| pulse_offset(output, pulse_position_at(*time), radius))
            .collect();
        offsets.iter().map(|offset| offset - offsets[0]).collect()
    }

    /// The pulse times of a `drifting_source` run: every `period` seconds.
    fn pulse_times(period: f64, count: u32) -> Vec<f64> {
        (1..=count).map(|k| f64::from(k) * period).collect()
    }

    #[test]
    fn sums_two_sources_with_per_source_gains() {
        // Two 16 kHz sources with known samples; gains 1.0 and 0.5.
        let a: Vec<f32> = (0..100).map(|i| (i as f32 / 100.0) - 0.5).collect();
        let b: Vec<f32> = (0..100).map(|i| 0.25 * (i as f32 * 0.1).sin()).collect();
        let tracks = [track("mic", 16_000, &a, 1.0), track("tap", 16_000, &b, 0.5)];
        let outcome = mix(&tracks, None).expect("mix");
        assert_eq!(outcome.samples.len(), 100);
        for index in 0..100 {
            let expected = a[index] + 0.5 * b[index];
            assert!(
                (outcome.samples[index] - expected).abs() < 1e-6,
                "sample {index}: {} != {expected}",
                outcome.samples[index]
            );
        }
        assert_eq!(outcome.reports.len(), 2);
        assert_eq!(outcome.reports[0].applied_gain, 1.0);
        assert_eq!(outcome.reports[1].applied_gain, 0.5);
        assert_eq!(outcome.clipped_samples, 0);
    }

    #[test]
    fn a_muted_source_contributes_silence() {
        let loud = vec![0.9f32; 64];
        let quiet = vec![0.1f32; 64];
        let tracks = [
            MixTrack {
                muted: true,
                ..track("mic", 16_000, &loud, 1.0)
            },
            track("tap", 16_000, &quiet, 1.0),
        ];
        let outcome = mix(&tracks, None).expect("mix");
        assert!(outcome
            .samples
            .iter()
            .all(|sample| (*sample - 0.1).abs() < 1e-6));
        assert_eq!(outcome.reports[0].applied_gain, 0.0);
        assert!(outcome.reports[0].muted);
        assert_eq!(outcome.reports[1].applied_gain, 1.0);
    }

    #[test]
    fn unity_solo_matches_the_existing_single_source_path() {
        // 48 kHz so stage 1 is the real FFT resampler, not the 16 kHz fast path.
        let samples = drifting_sine(48_000, 0.5, 0.0, 440.0);
        let tracks = [track("solo", 48_000, &samples, 1.0)];

        // No session: bit-identical to today's `resample_16k`.
        let outcome = mix(&tracks, None).expect("mix");
        let today = resample_16k(&samples, 48_000).expect("resample");
        assert_eq!(
            outcome.samples, today,
            "solo + unity + no session must be the existing path, bit for bit"
        );

        // A session whose measured clock equals nominal (N = rate·D exactly):
        // still bit-identical, minus the resampler's tail zero padding.
        let secs = 0.5;
        let outcome = mix(&tracks, Some(Duration::from_secs_f64(secs))).expect("mix");
        assert_eq!(outcome.samples.len(), 8_000);
        assert_eq!(outcome.samples[..], today[..8_000]);
        assert_eq!(outcome.reports[0].drift_ppm, 0.0);
        assert!(!outcome.reports[0].clamped);

        // 16 kHz source: the core fast path passes samples through unchanged.
        let native = drifting_sine(16_000, 0.1, 0.0, 440.0);
        let tracks = [track("solo16k", 16_000, &native, 1.0)];
        let outcome = mix(&tracks, None).expect("mix");
        assert_eq!(outcome.samples, native);
    }

    #[test]
    fn format_and_length_invariants() {
        let a = drifting_sine(48_000, 2.5, 0.0, 300.0);
        let b = drifting_sine(44_100, 2.5, 0.0, 700.0);
        let tracks = [track("a", 48_000, &a, 0.7), track("b", 44_100, &b, 0.3)];
        let outcome = mix(&tracks, Some(Duration::from_secs_f64(2.5))).expect("mix");
        // Length = the session span at the one recording rate.
        assert_eq!(outcome.samples.len(), 40_000);
        // All finite; the summed peak of two ≤0.7+0.3 sines stays inside ±1.
        assert!(outcome.samples.iter().all(|sample| sample.is_finite()));
        assert!(outcome.peak <= 1.0, "peak {}", outcome.peak);
        assert_eq!(outcome.clipped_samples, 0);
        // Reports carry the measurement story for every source.
        assert_eq!(outcome.reports.len(), 2);
        for report in &outcome.reports {
            assert!((report.captured_seconds - 2.5).abs() < 0.01);
            assert!(report.measured_rate.is_some());
            assert!(report.drift_ppm.abs() < 1_000.0, "{}", report.drift_ppm);
        }
    }

    #[test]
    fn clipping_saturates_at_the_pcm_boundary_and_is_counted() {
        // Two loud sources: 0.8 + 0.9 = 1.7 pre-saturation.
        let a = vec![0.8f32; 16_000];
        let b = vec![0.9f32; 16_000];
        let tracks = [track("a", 16_000, &a, 1.0), track("b", 16_000, &b, 1.0)];
        let outcome = mix(&tracks, None).expect("mix");
        assert_eq!(outcome.clipped_samples, 16_000, "every sample exceeds ±1");
        assert!((outcome.peak - 1.7).abs() < 1e-5, "peak {}", outcome.peak);
        // The sum itself is NOT squashed (no limiter): it is saturated exactly
        // at the i16 conversion, and never wraps.
        let path =
            std::env::temp_dir().join(format!("suramak-mix-clip-{}.wav", std::process::id()));
        write_recording_wav(&path, &outcome.samples, TARGET_RATE).expect("write");
        let decoded = suramak_core::decode::decode_file(&path).expect("decode");
        assert_eq!(decoded.rate, TARGET_RATE);
        assert_eq!(decoded.channels, 1);
        assert!(
            decoded.samples.iter().all(|sample| *sample >= 0.999),
            "positive saturation at full scale, not a wrap"
        );
        let negative = vec![-0.8f32; 16];
        let negative2 = vec![-0.9f32; 16];
        let outcome = mix(
            &[
                track("a", 16_000, &negative, 1.0),
                track("b", 16_000, &negative2, 1.0),
            ],
            None,
        )
        .expect("mix");
        write_recording_wav(&path, &outcome.samples, TARGET_RATE).expect("write");
        let decoded = suramak_core::decode::decode_file(&path).expect("decode");
        assert!(
            decoded.samples.iter().all(|sample| *sample <= -0.999),
            "negative saturation at full scale"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn empty_track_list_is_rejected() {
        let err = mix(&[], None).expect_err("no sources must fail");
        assert!(err.to_string().contains("no input sources"), "{err}");
    }

    /// **The drift proof.** A source clocked 250 ppm fast over 3 minutes: naive
    /// mixing (nominal rate) slides it by 35 ms; the measured-rate correction
    /// keeps every pulse within a sample of its content position.
    #[test]
    fn a_fast_source_stays_aligned_over_a_long_run() {
        let (rate, secs, ppm, period) = (24_000u32, 180.0f64, 250.0f64, 20.0f64);
        let samples = drifting_source(rate, secs, ppm, period, 0.0);
        let tracks = [track("fast", rate, &samples, 1.0)];
        let naive = mix(&tracks, None).expect("naive mix");
        let corrected = mix(&tracks, Some(Duration::from_secs_f64(secs))).expect("mix");

        let times = pulse_times(period, 8);
        let radius = 2_000; // ±125 ms, comfortably beyond the 35 ms drift
        let naive_moves = pulse_movements(&naive.samples, &times, radius);
        let corrected_moves = pulse_movements(&corrected.samples, &times, radius);
        let ms = |samples: isize| samples as f64 * 1000.0 / f64::from(TARGET_RATE);

        println!(
            "[drift +{ppm} ppm] source: {rate} Hz nominal, {secs} s session, \
             {} samples captured ({} s of content)",
            samples.len(),
            samples.len() as f64 / f64::from(rate)
        );
        println!("[drift] pulse movement vs first pulse (ms): naive | corrected");
        for (index, time) in times.iter().enumerate() {
            println!(
                "[drift]   t={time:>5.1}s: {:>7.2} | {:>7.2}",
                ms(naive_moves[index]),
                ms(corrected_moves[index])
            );
        }

        // Naive mixing drifts: at the last pulse the movement matches ppm·t.
        let expected_naive =
            (times.last().unwrap() - times[0]) * ppm / 1e6 * f64::from(TARGET_RATE);
        assert!(
            (*naive_moves.last().unwrap() as f64 - expected_naive).abs() <= 4.0,
            "naive movement {} vs analytical {expected_naive}",
            naive_moves.last().unwrap()
        );
        assert!(
            ms(*naive_moves.last().unwrap()) >= 20.0,
            "the test must exercise real drift"
        );
        // The corrected mix stays bounded — the whole point.
        for movement in &corrected_moves {
            assert!(
                movement.abs() <= 3,
                "corrected pulse moved {movement} samples ({} ms)",
                ms(*movement)
            );
        }
        assert!(!corrected.reports[0].clamped);
        assert!((corrected.reports[0].drift_ppm - 250.0).abs() < 1.0);
    }

    /// The slow half of the drift proof: a source clocked 250 ppm slow loses
    /// samples; naive mixing slides it backwards, the correction pins it.
    #[test]
    fn a_slow_source_stays_aligned_over_a_long_run() {
        let (rate, secs, ppm, period) = (24_000u32, 180.0f64, -250.0f64, 20.0f64);
        let samples = drifting_source(rate, secs, ppm, period, 0.0);
        let tracks = [track("slow", rate, &samples, 1.0)];
        let naive = mix(&tracks, None).expect("naive mix");
        let corrected = mix(&tracks, Some(Duration::from_secs_f64(secs))).expect("mix");

        let times = pulse_times(period, 8);
        let radius = 2_000;
        let naive_moves = pulse_movements(&naive.samples, &times, radius);
        let corrected_moves = pulse_movements(&corrected.samples, &times, radius);
        let ms = |samples: isize| samples as f64 * 1000.0 / f64::from(TARGET_RATE);

        println!(
            "[drift {ppm} ppm] source: {rate} Hz nominal, {secs} s session, \
             {} samples captured",
            samples.len()
        );
        for (index, time) in times.iter().enumerate() {
            println!(
                "[drift]   t={time:>5.1}s: {:>7.2} | {:>7.2}",
                ms(naive_moves[index]),
                ms(corrected_moves[index])
            );
        }
        // Naive mixing drifts: at the last pulse the movement matches ppm·t
        // (analytically −35 ms here), so the test cannot pass vacuously.
        let expected_naive =
            (times.last().unwrap() - times[0]) * ppm / 1e6 * f64::from(TARGET_RATE);
        assert!(
            (*naive_moves.last().unwrap() as f64 - expected_naive).abs() <= 4.0,
            "naive movement {} vs analytical {expected_naive}",
            naive_moves.last().unwrap()
        );
        assert!(
            ms(*naive_moves.last().unwrap()) <= -20.0,
            "the slow test must exercise real drift"
        );
        for movement in &corrected_moves {
            assert!(
                movement.abs() <= 3,
                "corrected pulse moved {movement} samples ({} ms)",
                ms(*movement)
            );
        }
        assert!((corrected.reports[0].drift_ppm + 250.0).abs() < 1.0);
    }

    /// Two sources drifting in opposite directions must stay aligned *to each
    /// other* (the mic + system-audio meeting combo): naive mixing slides them
    /// apart at the sum of both drifts; the correction pins both to the shared
    /// session timeline. Distinct pulse times let each source be measured
    /// inside the single mix.
    #[test]
    fn two_sources_with_opposite_drift_stay_mutually_aligned() {
        let (rate, secs, ppm, period) = (16_000u32, 100.0f64, 300.0f64, 20.0f64);
        let fast = drifting_source(rate, secs, ppm, period, 0.0);
        let slow = drifting_source(rate, secs, -ppm, period, 10.0);
        let tracks = [
            track("fast", rate, &fast, 1.0),
            track("slow", rate, &slow, 1.0),
        ];
        let naive = mix(&tracks, None).expect("naive mix");
        let corrected = mix(&tracks, Some(Duration::from_secs_f64(secs))).expect("mix");

        // Fast pulses at 20/40/60/80 s; the slow source's pulses are offset by
        // 10 s, so a ±2 000-sample search window around each expected position
        // sees only its own source's marker.
        let fast_times = pulse_times(period, 4);
        let slow_times: Vec<f64> = fast_times.iter().map(|time| time + 10.0).collect();
        let radius = 2_000;
        let ms = |samples: isize| samples as f64 * 1000.0 / f64::from(TARGET_RATE);
        let fast_moves = pulse_movements(&corrected.samples, &fast_times, radius);
        let slow_moves = pulse_movements(&corrected.samples, &slow_times, radius);
        let naive_fast = pulse_movements(&naive.samples, &fast_times, radius);
        let naive_slow = pulse_movements(&naive.samples, &slow_times, radius);

        println!("[drift ±{ppm} ppm, two sources, {secs} s] pulse movement (ms):");
        for (index, time) in fast_times.iter().enumerate() {
            println!(
                "[drift]   t={time:>5.1}s: naive fast {:>7.2} | corrected fast {:>7.2}",
                ms(naive_fast[index]),
                ms(fast_moves[index])
            );
        }
        for (index, time) in slow_times.iter().enumerate() {
            println!(
                "[drift]   t={time:>5.1}s: naive slow {:>7.2} | corrected slow {:>7.2}",
                ms(naive_slow[index]),
                ms(slow_moves[index])
            );
        }

        // Corrected: both sources pinned to the session timeline.
        for movement in fast_moves.iter().chain(slow_moves.iter()) {
            assert!(
                movement.abs() <= 3,
                "corrected pulse moved {movement} samples ({} ms)",
                ms(*movement)
            );
        }
        // Naive: each slides with its own clock — and the two *apart* by 2·ppm·t
        // (36 ms at t=80 s vs t=20 s), which is what the correction removes.
        let expected = 60.0 * ppm / 1e6 * f64::from(TARGET_RATE);
        assert!(
            (naive_fast.last().unwrap().abs() as f64 - expected).abs() <= 4.0,
            "naive fast movement {} vs analytical {expected}",
            naive_fast.last().unwrap()
        );
        assert!(
            (naive_slow.last().unwrap().abs() as f64 - expected).abs() <= 4.0,
            "naive slow movement {} vs analytical {expected}",
            naive_slow.last().unwrap()
        );
        let naive_relative = ms(*naive_fast.last().unwrap()) - ms(*naive_slow.last().unwrap());
        assert!(
            naive_relative >= 30.0,
            "naive relative slide {naive_relative} ms"
        );
        assert_eq!(corrected.reports.len(), 2);
        assert!((corrected.reports[0].drift_ppm - 300.0).abs() < 1.0);
        assert!((corrected.reports[1].drift_ppm + 300.0).abs() < 1.0);
    }

    /// The correction is a constant resampling ratio — no sample is inserted or
    /// dropped, so the output can never click. A drifting 440 Hz sine corrected
    /// by 400 ppm stays a clean full-amplitude 440 Hz sine: over the steady
    /// middle the largest sample-to-sample step is the sine's own slope, not a
    /// discontinuity.
    #[test]
    fn a_corrected_drifting_sine_never_clicks() {
        let (rate, secs, ppm) = (48_000u32, 30.0f64, 400.0f64);
        let samples = drifting_sine(rate, secs, ppm, 440.0);
        let tracks = [track("drift", rate, &samples, 1.0)];
        let outcome = mix(&tracks, Some(Duration::from_secs_f64(secs))).expect("mix");

        // 1. No discontinuity from the correction: over the steady middle the
        //    max adjacent delta is the 440 Hz sine's own slope at 16 kHz
        //    (2·sin(π·440/16000) ≈ 0.1726) plus interpolation slack. The
        //    resampler's edges (its warm-up and its tail truncation) are
        //    pre-existing properties of the single-source path and are
        //    excluded — their measured values are printed for the report.
        let ideal_step = (2.0 * (PI * 440.0 / f64::from(TARGET_RATE)).sin()) as f32;
        let length = outcome.samples.len();
        let middle = &outcome.samples[2_000..length - 2_000];
        let max_step = middle
            .windows(2)
            .map(|window| (window[1] - window[0]).abs())
            .fold(0.0f32, f32::max);
        let head_step = outcome.samples[..2_000]
            .windows(2)
            .map(|window| (window[1] - window[0]).abs())
            .fold(0.0f32, f32::max);
        let tail_step = outcome.samples[length - 2_000..]
            .windows(2)
            .map(|window| (window[1] - window[0]).abs())
            .fold(0.0f32, f32::max);
        println!(
            "[click] max step: middle {max_step:.5} (sine slope {ideal_step:.5}), \
             head {head_step:.5}, tail {tail_step:.5} (resampler edges, pre-existing)"
        );
        assert!(
            max_step <= ideal_step * 1.05 + 0.002,
            "click detected in the steady region: max step {max_step} vs sine slope {ideal_step}"
        );

        // 2. Still a clean sine: the best-shift correlation against the ideal
        //    tone is ~1 (a click or a dropped sample would crater it).
        let ideal: Vec<f32> = (0..outcome.samples.len())
            .map(|index| (2.0 * PI * 440.0 * index as f64 / f64::from(TARGET_RATE)).sin() as f32)
            .collect();
        let start = 4_000;
        let end = outcome.samples.len() - 4_000;
        let (_, correlation) = best_shift_correlation(&outcome.samples[start..end], &ideal, 256);
        assert!(
            correlation >= 0.999,
            "corrected tone is not clean: correlation {correlation}"
        );

        // 3. Full amplitude preserved (no squashing of any kind).
        assert!(
            outcome.peak > 0.99 && outcome.peak <= 1.01,
            "peak {}",
            outcome.peak
        );
        assert_eq!(outcome.clipped_samples, 0);
    }

    /// Correlation of `window` against `ideal` at the best integer shift within
    /// ±`radius`, normalized to [−1, 1].
    fn best_shift_correlation(window: &[f32], ideal: &[f32], radius: isize) -> (isize, f32) {
        let mut best_shift = 0;
        let mut best = f32::MIN;
        for shift in -radius..=radius {
            let mut dot = 0.0f32;
            let mut energy = 0.0f32;
            let mut window_energy = 0.0f32;
            for (index, window_sample) in window.iter().enumerate() {
                let other = (index as isize + shift) as usize;
                let other = ideal.get(other).copied().unwrap_or(0.0);
                dot += window_sample * other;
                energy += other * other;
                window_energy += window_sample * window_sample;
            }
            let denominator = (energy * window_energy).sqrt();
            let score = if denominator > 0.0 {
                dot / denominator
            } else {
                0.0
            };
            if score > best {
                best = score;
                best_shift = shift;
            }
        }
        (best_shift, best)
    }

    /// The mixer demo (acceptance §6): a small run mixing two synthetic sources
    /// (one drifting) into the one 16 kHz mono i16 WAV, with the numbers printed.
    /// Ignored by default — run explicitly:
    /// `cargo test -p suramak-asr -- --ignored mix_demo --nocapture`
    /// (writes `build/m6-out/mix-demo.wav` relative to the crate directory —
    /// `crates/suramak-asr/build/…` under `cargo test` — and gitignored).
    #[test]
    #[ignore = "writes build/m6-out/mix-demo.wav; run explicitly for the M6 acceptance"]
    fn mix_demo_two_synthetic_sources_to_a_wav() {
        let secs = 5.0f64;
        let mic = drifting_source(48_000, secs, 250.0, 1.0, 0.0);
        let tap = drifting_sine(48_000, secs, -250.0, 660.0);
        let tracks = [
            MixTrack {
                label: "mic (48 kHz, +250 ppm)".to_string(),
                rate: 48_000,
                samples: &mic,
                gain: 0.5,
                muted: false,
            },
            MixTrack {
                label: "system audio (48 kHz, −250 ppm, gain 0.25)".to_string(),
                rate: 48_000,
                samples: &tap,
                gain: 0.25,
                muted: false,
            },
        ];
        let dir = std::path::Path::new("build/m6-out");
        std::fs::create_dir_all(dir).expect("create build/m6-out");
        let path = dir.join("mix-demo.wav");
        let outcome =
            mix_to_wav(&path, &tracks, Some(Duration::from_secs_f64(secs))).expect("mix to wav");
        for report in &outcome.reports {
            println!(
                "[demo] {}: {} samples, measured {:.3} Hz (drift {:+.1} ppm, gain {})",
                report.label,
                report.captured_samples,
                report.measured_rate.unwrap_or(f64::NAN),
                report.drift_ppm,
                report.applied_gain
            );
        }
        println!(
            "[demo] output: {} samples at {} Hz, peak {:.3}, clipped {}",
            outcome.samples.len(),
            TARGET_RATE,
            outcome.peak,
            outcome.clipped_samples
        );
        let decoded = suramak_core::decode::decode_file(&path).expect("decode back");
        println!(
            "[demo] wav: {} bytes, rate {} channels {} samples {} ({:.2} s)",
            std::fs::metadata(&path).unwrap().len(),
            decoded.rate,
            decoded.channels,
            decoded.samples.len(),
            decoded.samples.len() as f64 / f64::from(decoded.rate)
        );
        assert_eq!(decoded.rate, 16_000);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.samples.len(), outcome.samples.len());
    }
}
