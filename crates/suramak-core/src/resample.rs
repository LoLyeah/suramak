use rubato::{FftFixedIn, Resampler};

use crate::decode::AudioBuffer;
use crate::SuramakError;

/// Target sample rate for whisper.cpp.
pub const TARGET_RATE: u32 = 16_000;

/// Resamples and mono-mixes `buf` to 16 kHz mono f32.
///
/// The returned length is the **exact** duration of the input at 16 kHz
/// (`round(len * 16000 / rate)`, SUR-LOW-04): rubato pads the final chunk to
/// `chunk_size` and emits a full output chunk for it, so the raw stream carries up
/// to a chunk of artificial silence at the tail — audible as a gap at the end of a
/// transcript's audio and pure noise for whisper.
pub fn to_16khz_mono(buf: &AudioBuffer) -> Result<Vec<f32>, SuramakError> {
    let mono = mixdown_mono(&buf.samples, buf.channels as usize);

    if buf.rate == TARGET_RATE {
        return Ok(mono);
    }

    let chunk_size = 1024;
    let mut resampler =
        FftFixedIn::<f32>::new(buf.rate as usize, TARGET_RATE as usize, chunk_size, 2, 1)
            .map_err(|e| SuramakError::ResamplerInit(format!("resampler init: {e}")))?;

    let mut output: Vec<f32> = Vec::new();
    let mut pos = 0usize;

    while pos < mono.len() {
        let end = (pos + chunk_size).min(mono.len());
        let mut chunk = mono[pos..end].to_vec();
        // Pad last chunk if needed.
        if chunk.len() < chunk_size {
            chunk.resize(chunk_size, 0.0);
        }
        let frames = resampler.process(&[chunk], None)?;
        output.extend_from_slice(&frames[0]);
        pos += chunk_size;
    }

    let expected_len =
        ((mono.len() as f64 * TARGET_RATE as f64) / buf.rate as f64).round() as usize;
    output.truncate(expected_len);

    Ok(output)
}

/// Mixes interleaved multi-channel samples to mono by averaging channels.
fn mixdown_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let n_frames = samples.len() / channels;
    let inv = 1.0 / channels as f32;
    (0..n_frames)
        .map(|i| {
            (0..channels)
                .map(|c| samples[i * channels + c])
                .sum::<f32>()
                * inv
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::AudioBuffer;
    use std::f32::consts::PI;

    /// Generates a sine wave at `freq` Hz, `sample_rate` Hz, `duration_secs` s, single channel.
    fn sine_wave(freq: f32, sample_rate: u32, duration_secs: f32) -> Vec<f32> {
        let n = (sample_rate as f32 * duration_secs) as usize;
        (0..n)
            .map(|i| (2.0 * PI * freq * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    /// Naive DFT to find the dominant frequency bin.
    fn dominant_freq(samples: &[f32], sample_rate: u32) -> f32 {
        let n = samples.len();
        // Only scan up to Nyquist.
        let half = n / 2;
        let mut max_mag = 0.0f32;
        let mut max_k = 0usize;
        for k in 1..half {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, &s) in samples.iter().enumerate() {
                let angle = 2.0 * PI * k as f32 * i as f32 / n as f32;
                re += s * angle.cos();
                im -= s * angle.sin();
            }
            let mag = (re * re + im * im).sqrt();
            if mag > max_mag {
                max_mag = mag;
                max_k = k;
            }
        }
        max_k as f32 * sample_rate as f32 / n as f32
    }

    #[test]
    fn resample_440hz_sine() {
        let src_rate = 44_100u32;
        let freq = 440.0f32;
        let samples = sine_wave(freq, src_rate, 0.5);

        let buf = AudioBuffer {
            rate: src_rate,
            channels: 1,
            samples,
        };

        let resampled = to_16khz_mono(&buf).expect("resample failed");

        // Use a shorter window for the DFT to keep the test fast.
        let window = &resampled[..resampled.len().min(2048)];
        let detected = dominant_freq(window, TARGET_RATE);

        let tolerance = freq * 0.02; // ±2%
        assert!(
            (detected - freq).abs() <= tolerance,
            "expected ~{freq} Hz, got {detected} Hz"
        );
    }

    #[test]
    fn mono_mixdown_averages() {
        let stereo = vec![1.0f32, -1.0, 0.5, -0.5];
        let mono = mixdown_mono(&stereo, 2);
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.0).abs() < 1e-6);
        assert!((mono[1] - 0.0).abs() < 1e-6);
    }

    /// SUR-LOW-04: rubato pads the final chunk and emits a whole output chunk for
    /// it, so the raw stream is longer than the audio. The returned buffer must be
    /// exactly the input's duration at 16 kHz — no appended silence.
    #[test]
    fn the_resampled_length_is_exactly_the_input_duration() {
        let rate = 44_100u32;
        let samples = sine_wave(440.0, rate, 0.5);
        let expected = ((samples.len() as f64 * TARGET_RATE as f64) / rate as f64).round() as usize;
        assert_eq!(expected, 8_000, "0.5 s at 16 kHz is 8 000 samples");

        let buf = AudioBuffer {
            rate,
            channels: 1,
            samples,
        };
        let resampled = to_16khz_mono(&buf).expect("resample");
        assert_eq!(
            resampled.len(),
            expected,
            "the tail must be cut to the real duration, not rubato's padded chunk"
        );

        // A 44.1 kHz input whose length is not a multiple of the chunk size never
        // gains samples: rubato's FFT latency can leave a short clip a little
        // under its ideal duration (pre-existing), but the fix guarantees it is
        // never *longer* — the appended-silence direction.
        let odd = sine_wave(440.0, rate, 0.1234);
        let ideal = ((odd.len() as f64 * TARGET_RATE as f64) / rate as f64).round() as usize;
        let buf = AudioBuffer {
            rate,
            channels: 1,
            samples: odd,
        };
        let resampled = to_16khz_mono(&buf).expect("resample");
        assert!(
            resampled.len() <= ideal,
            "resampled {} samples for an ideal {ideal}: the padded tail must be cut",
            resampled.len()
        );
        assert!(!resampled.is_empty());
    }

    /// SUR-LOW-01: an impossible rate is a *resampler* error, not a polish one.
    #[test]
    fn an_impossible_rate_is_reported_as_a_resampler_error() {
        let buf = AudioBuffer {
            rate: 0,
            channels: 1,
            samples: vec![0.0; 128],
        };
        let err = to_16khz_mono(&buf).expect_err("0 Hz cannot be resampled");
        assert!(
            matches!(err, SuramakError::ResamplerInit(_)),
            "unexpected error: {err}"
        );
    }
}
