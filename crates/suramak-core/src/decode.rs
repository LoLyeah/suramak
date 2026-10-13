use std::io::Cursor;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::SuramakError;

/// Decoded audio ready for resampling.
#[derive(Debug)]
pub struct AudioBuffer {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

/// Decodes audio from a file path into interleaved f32 samples.
pub fn decode_file(path: &std::path::Path) -> Result<AudioBuffer, SuramakError> {
    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    decode_stream(mss, hint)
}

/// Decodes audio from raw bytes (useful for tests).
pub fn decode_bytes(
    data: Vec<u8>,
    extension_hint: Option<&str>,
) -> Result<AudioBuffer, SuramakError> {
    let cursor = Cursor::new(data);
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = extension_hint {
        hint.with_extension(ext);
    }
    decode_stream(mss, hint)
}

fn decode_stream(mss: MediaSourceStream, hint: Hint) -> Result<AudioBuffer, SuramakError> {
    let format_opts = FormatOptions::default();
    let meta_opts = MetadataOptions::default();
    let dec_opts = DecoderOptions::default();

    let probed = symphonia::default::get_probe().format(&hint, mss, &format_opts, &meta_opts)?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or(SuramakError::NoAudioData)?;

    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &dec_opts)?;

    let rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(1);

    let mut samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(symphonia::core::errors::Error::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(e) => return Err(SuramakError::Symphonia(e)),
        };

        if packet.track_id() != track_id {
            continue;
        }

        append_samples(&mut samples, decoder.decode(&packet)?);
    }

    if samples.is_empty() {
        return Err(SuramakError::NoAudioData);
    }

    Ok(AudioBuffer {
        rate,
        channels,
        samples,
    })
}

/// Appends interleaved f32 samples from any `AudioBufferRef` variant.
fn append_samples(out: &mut Vec<f32>, buf: AudioBufferRef<'_>) {
    match buf {
        AudioBufferRef::F32(b) => {
            let n_frames = b.frames();
            let n_ch = b.spec().channels.count();
            for i in 0..n_frames {
                for ch in 0..n_ch {
                    out.push(b.chan(ch)[i]);
                }
            }
        }
        AudioBufferRef::F64(b) => {
            let n_frames = b.frames();
            let n_ch = b.spec().channels.count();
            for i in 0..n_frames {
                for ch in 0..n_ch {
                    out.push(b.chan(ch)[i] as f32);
                }
            }
        }
        AudioBufferRef::S8(b) => {
            let n_frames = b.frames();
            let n_ch = b.spec().channels.count();
            for i in 0..n_frames {
                for ch in 0..n_ch {
                    out.push(b.chan(ch)[i] as f32 / i8::MAX as f32);
                }
            }
        }
        AudioBufferRef::U8(b) => {
            let n_frames = b.frames();
            let n_ch = b.spec().channels.count();
            for i in 0..n_frames {
                for ch in 0..n_ch {
                    out.push((b.chan(ch)[i] as f32 - 128.0) / 128.0);
                }
            }
        }
        AudioBufferRef::S16(b) => {
            let n_frames = b.frames();
            let n_ch = b.spec().channels.count();
            for i in 0..n_frames {
                for ch in 0..n_ch {
                    out.push(b.chan(ch)[i] as f32 / i16::MAX as f32);
                }
            }
        }
        AudioBufferRef::U16(b) => {
            let n_frames = b.frames();
            let n_ch = b.spec().channels.count();
            for i in 0..n_frames {
                for ch in 0..n_ch {
                    out.push((b.chan(ch)[i] as f32 - 32768.0) / 32768.0);
                }
            }
        }
        AudioBufferRef::S24(b) => {
            let n_frames = b.frames();
            let n_ch = b.spec().channels.count();
            for i in 0..n_frames {
                for ch in 0..n_ch {
                    out.push(b.chan(ch)[i].0 as f32 / 8_388_607.0);
                }
            }
        }
        AudioBufferRef::U24(b) => {
            let n_frames = b.frames();
            let n_ch = b.spec().channels.count();
            for i in 0..n_frames {
                for ch in 0..n_ch {
                    out.push((b.chan(ch)[i].0 as f32 - 8_388_608.0) / 8_388_608.0);
                }
            }
        }
        AudioBufferRef::S32(b) => {
            let n_frames = b.frames();
            let n_ch = b.spec().channels.count();
            for i in 0..n_frames {
                for ch in 0..n_ch {
                    out.push(b.chan(ch)[i] as f32 / i32::MAX as f32);
                }
            }
        }
        AudioBufferRef::U32(b) => {
            let n_frames = b.frames();
            let n_ch = b.spec().channels.count();
            for i in 0..n_frames {
                for ch in 0..n_ch {
                    out.push((b.chan(ch)[i] as f32 - 2_147_483_648.0) / 2_147_483_648.0);
                }
            }
        }
    }
}

/// Writes f32 samples to a WAV file using hound.
pub fn write_wav(
    path: &std::path::Path,
    samples: &[f32],
    rate: u32,
    channels: u16,
) -> Result<(), SuramakError> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| SuramakError::Io(std::io::Error::other(e.to_string())))?;
    for &s in samples {
        writer
            .write_sample(s)
            .map_err(|e| SuramakError::Io(std::io::Error::other(e.to_string())))?;
    }
    writer
        .finalize()
        .map_err(|e| SuramakError::Io(std::io::Error::other(e.to_string())))?;
    Ok(())
}
