use rubato::{FftFixedIn, Resampler};

use crate::decode::AudioBuffer;
use crate::SuramakError;

/// Target sample rate for whisper.cpp.
pub const TARGET_RATE: u32 = 16_000;

/// Resamples and mono-mixes `buf` to 16 kHz mono f32.
pub fn to_16khz_mono(buf: &AudioBuffer) -> Result<Vec<f32>, SuramakError> {
    let mono = mixdown_mono(&buf.samples, buf.channels as usize);

    if buf.rate == TARGET_RATE {
        return Ok(mono);
    }

    let chunk_size = 1024;
    let mut resampler =
        FftFixedIn::<f32>::new(buf.rate as usize, TARGET_RATE as usize, chunk_size, 2, 1)
            .map_err(|e| SuramakError::Polish(format!("resampler init: {e}")))?;

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
}
