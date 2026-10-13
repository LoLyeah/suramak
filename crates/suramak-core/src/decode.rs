use std::io::Cursor;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{CodecType, DecoderOptions};
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

/// Display name + conversion hint carried into the actionable decode errors.
///
/// The user sees the file name (not the full path) and an `ffmpeg` command that
/// produces a file this build can decode.
struct InputLabel {
    name: String,
    suggested: String,
}

impl InputLabel {
    fn new(name: &str) -> Self {
        let suggested = std::path::Path::new(name)
            .with_extension("mp4")
            .to_string_lossy()
            .into_owned();
        Self {
            name: name.to_string(),
            suggested,
        }
    }

    /// The file name of `path` (falling back to the full path for odd inputs).
    fn from_path(path: &std::path::Path) -> Self {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("input");
        Self::new(name)
    }

    /// The probe found no reader for this container (or the file ran out of
    /// bytes before a format could be recognized).
    fn unsupported_format(&self, detail: &str) -> SuramakError {
        SuramakError::UnsupportedFormat {
            name: self.name.clone(),
            detail: detail.to_string(),
            suggested: self.suggested.clone(),
        }
    }

    /// The container demuxed, but no decoder exists for its audio codec.
    fn unsupported_codec(&self, codec: CodecType) -> SuramakError {
        let detail = match codec_name(codec) {
            Some(codec) => format!("the audio codec ({codec}) is not supported"),
            None => "the audio codec is not supported".to_string(),
        };
        SuramakError::UnsupportedFormat {
            name: self.name.clone(),
            detail,
            suggested: self.suggested.clone(),
        }
    }
}

/// Human name for a codec that demuxes but has no decoder in this build
/// (Opus in WebM/MKV is the case users actually hit).
fn codec_name(codec: CodecType) -> Option<&'static str> {
    use symphonia::core::codecs as c;
    Some(match codec {
        c::CODEC_TYPE_OPUS => "Opus",
        c::CODEC_TYPE_ALAC => "ALAC",
        c::CODEC_TYPE_WMA => "WMA",
        c::CODEC_TYPE_SPEEX => "Speex",
        c::CODEC_TYPE_EAC3 => "E-AC-3",
        c::CODEC_TYPE_AC4 => "AC-4",
        c::CODEC_TYPE_DCA => "DTS",
        _ => return None,
    })
}

/// Decodes audio from a file path into interleaved f32 samples.
///
/// Container support (M5): wav, mp3, flac, ogg/vorbis, m4a, mp4, mov, m4v and
/// mkv — a video file's first audio track is decoded and its video streams are
/// ignored. Anything else fails with [`SuramakError::UnsupportedFormat`], whose
/// message names a working conversion.
pub fn decode_file(path: &std::path::Path) -> Result<AudioBuffer, SuramakError> {
    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    decode_stream(mss, hint, &InputLabel::from_path(path))
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
    let label = InputLabel::new(&match extension_hint {
        Some(ext) => format!("input.{ext}"),
        None => "input".to_string(),
    });
    decode_stream(mss, hint, &label)
}

fn decode_stream(
    mss: MediaSourceStream,
    hint: Hint,
    label: &InputLabel,
) -> Result<AudioBuffer, SuramakError> {
    let format_opts = FormatOptions::default();
    let meta_opts = MetadataOptions::default();
    let dec_opts = DecoderOptions::default();

    let probed = match symphonia::default::get_probe().format(&hint, mss, &format_opts, &meta_opts)
    {
        Ok(probed) => probed,
        // No reader for this container — a user-facing, actionable error.
        Err(symphonia::core::errors::Error::Unsupported(_)) => {
            return Err(label.unsupported_format("this file's format is not supported"));
        }
        // A marker scan (or a matched reader's header scan) that ran off the end
        // of the file — e.g. a real `.aiff`, which this build has no reader for.
        Err(symphonia::core::errors::Error::IoError(e))
            if e.kind() == std::io::ErrorKind::UnexpectedEof =>
        {
            return Err(label
                .unsupported_format("this file's format is not supported (empty or truncated)"));
        }
        Err(err) => return Err(SuramakError::Symphonia(err)),
    };
    let mut format = probed.format;

    // The first track with a real codec: for video files this skips the video
    // streams (demuxers map them to `CODEC_TYPE_NULL`) and picks the audio track.
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or(SuramakError::NoAudioData)?;

    let track_id = track.id;
    let mut decoder = match symphonia::default::get_codecs().make(&track.codec_params, &dec_opts) {
        Ok(decoder) => decoder,
        // The container demuxed, but there is no decoder for its audio codec
        // (e.g. Opus in WebM/MKV) — actionable, same as the probe case.
        Err(symphonia::core::errors::Error::Unsupported(_)) => {
            return Err(label.unsupported_codec(track.codec_params.codec));
        }
        Err(err) => return Err(SuramakError::Symphonia(err)),
    };

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

/// Writes f32 samples to a WAV file as **16-bit signed PCM** (additive sibling of
/// [`write_wav`], which stays 32-bit float for its existing callers).
///
/// M3 recordings use this: 16 kHz mono i16 is both the smallest sane intermediate
/// (~115 MB/hour vs ~230 MB/h f32) and the sample depth whisper's decode step
/// yields anyway. Samples are clamped to `i16` range and rounded to nearest.
pub fn write_wav_i16(
    path: &std::path::Path,
    samples: &[f32],
    rate: u32,
    channels: u16,
) -> Result<(), SuramakError> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| SuramakError::Io(std::io::Error::other(e.to_string())))?;
    for &s in samples {
        let scaled = (s * f32::from(i16::MAX)).round();
        let value = scaled.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
        writer
            .write_sample(value)
            .map_err(|e| SuramakError::Io(std::io::Error::other(e.to_string())))?;
    }
    writer
        .finalize()
        .map_err(|e| SuramakError::Io(std::io::Error::other(e.to_string())))?;
    Ok(())
}
