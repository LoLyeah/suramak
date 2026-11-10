use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::LazyLock;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{CodecRegistry, CodecType, DecoderOptions};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia_adapter_libopus::OpusDecoder;

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
/// (ALAC in an m4a is the case users actually hit; Opus decodes natively since
/// M10C, so it no longer reaches this map in the default build — kept because
/// this is a name table, not a support list).
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

/// The decode path's codec registry: symphonia's default set plus the libopus
/// adapter's Opus decoder (M10C).
///
/// symphonia 0.5 has no first-party Opus decoder, so webm/opus, mkv-opus and
/// raw Ogg-Opus (`.opus` voice notes) used to demux and then fail with the
/// conversion recipe. `OpusDecoder` registers for `CODEC_TYPE_OPUS`; the
/// adapter bundles libopus (compiled from source via `cc` — no runtime
/// library, nothing shipped separately). Built once per process; the registry
/// is read-only afterwards.
fn codec_registry() -> &'static CodecRegistry {
    static REGISTRY: LazyLock<CodecRegistry> = LazyLock::new(|| {
        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        registry.register_all::<OpusDecoder>();
        registry
    });
    &REGISTRY
}

/// Decodes audio from a file path into interleaved f32 samples.
///
/// Container support (M5): wav, mp3, flac, ogg/vorbis, m4a, mp4, mov, m4v and
/// mkv — a video file's first audio track is decoded and its video streams are
/// ignored. Opus decodes natively since M10C (webm, mkv, raw Ogg-Opus `.opus`
/// voice notes). Anything else fails with [`SuramakError::UnsupportedFormat`],
/// whose message names a working conversion.
///
/// **Shared-decode contract (M15.2):** this function — symphonia + the bundled
/// libopus adapter, plus the `Truncated` check — is *the* import decoder. The
/// Android M13-C import path must call it (or [`decode_file_cancellable`]) on
/// the copied local file, never a platform `MediaCodec`/`JNI` decoder, so the
/// same four container families and the same errors behave identically on
/// desktop and device.
pub fn decode_file(path: &std::path::Path) -> Result<AudioBuffer, SuramakError> {
    decode_file_cancellable(path, &|| false)
}

/// [`decode_file`] with a cooperative cancellation check.
///
/// `cancelled` is consulted before the probe and once per demuxed packet; the
/// first `true` returns [`SuramakError::Cancelled`] and drops every sample
/// collected so far — no partial buffer ever escapes. This is the same
/// cooperative shape as the ASR pipeline's stage checks: pass
/// `|| token.is_cancelled()` and the decode stops mid-file instead of after it.
pub fn decode_file_cancellable(
    path: &std::path::Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<AudioBuffer, SuramakError> {
    let label = InputLabel::from_path(path);
    let mut file = std::fs::File::open(path)?;
    // Structural truncation, before the demuxers touch the bytes:
    // - Ogg-Opus (the WhatsApp/Discord voice-note shape) hides its total length
    //   from a cut file; the tail page's EOS flag is the only signal.
    // - Matroska/WebM declares its Segment size up front, which a cut file
    //   cannot satisfy (and symphonia's EBML reader panics trying).
    if let Some(detail) = file_truncation_detail(&mut file)? {
        return Err(SuramakError::Truncated {
            name: label.name.clone(),
            detail,
        });
    }
    file.seek(SeekFrom::Start(0))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    decode_stream(mss, hint, &label, cancelled)
}

/// Decodes audio from raw bytes (useful for tests).
pub fn decode_bytes(
    data: Vec<u8>,
    extension_hint: Option<&str>,
) -> Result<AudioBuffer, SuramakError> {
    decode_bytes_cancellable(data, extension_hint, &|| false)
}

/// [`decode_bytes`] with a cooperative cancellation check.
pub fn decode_bytes_cancellable(
    data: Vec<u8>,
    extension_hint: Option<&str>,
    cancelled: &dyn Fn() -> bool,
) -> Result<AudioBuffer, SuramakError> {
    let label = InputLabel::new(&match extension_hint {
        Some(ext) => format!("input.{ext}"),
        None => "input".to_string(),
    });
    if let Some(detail) = bytes_truncation_detail(&data) {
        return Err(SuramakError::Truncated {
            name: label.name.clone(),
            detail,
        });
    }
    let cursor = Cursor::new(data);
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = extension_hint {
        hint.with_extension(ext);
    }
    decode_stream(mss, hint, &label, cancelled)
}

fn decode_stream(
    mss: MediaSourceStream,
    hint: Hint,
    label: &InputLabel,
    cancelled: &dyn Fn() -> bool,
) -> Result<AudioBuffer, SuramakError> {
    // The probe and the demuxer are foreign code. A malformed container can
    // panic in either (symphonia-format-mkv's EBML reader panics on a truncated
    // header); the boundary turns that into a named error instead of a crash.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decode_stream_inner(mss, hint, label, cancelled)
    }));
    match outcome {
        Ok(result) => result,
        Err(payload) => Err(SuramakError::DecoderPanic {
            name: label.name.clone(),
            detail: panic_detail(payload.as_ref()),
        }),
    }
}

fn decode_stream_inner(
    mss: MediaSourceStream,
    hint: Hint,
    label: &InputLabel,
    cancelled: &dyn Fn() -> bool,
) -> Result<AudioBuffer, SuramakError> {
    if cancelled() {
        return Err(SuramakError::Cancelled);
    }
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
    let mut decoder = match codec_registry().make(&track.codec_params, &dec_opts) {
        Ok(decoder) => decoder,
        // The container demuxed, but there is no decoder for its audio codec
        // (e.g. ALAC in an m4a; Opus decoded natively since M10C) —
        // actionable, same as the probe case.
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
    // A container that knows its length (Ogg granule, WAV frames, MKV Duration)
    // lets a cut-off file fail below instead of looking like a short recording.
    let declared_frames = track.codec_params.n_frames;

    let mut samples: Vec<f32> = Vec::new();

    // Third-party demuxers can panic on malformed input (symphonia-format-mkv's
    // EBML reader does on a truncated Matroska cluster). The packet loop runs
    // behind `catch_unwind`: a panic becomes a named error, never a process
    // abort. Panics from our own code are caught too — the alternative is a
    // crashed import for a file the user just shared.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decode_packets(
            format.as_mut(),
            decoder.as_mut(),
            track_id,
            &mut samples,
            cancelled,
        )
    }));
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(err)) => return Err(err),
        Err(payload) => {
            // If the container declared a length and the panic happened with
            // most of it missing, "truncated" is the honest diagnosis; the
            // panic is just how this demuxer reports the missing bytes.
            if let Some(err) =
                truncation_error(label, declared_frames, samples.len(), channels, rate)
            {
                return Err(err);
            }
            return Err(SuramakError::DecoderPanic {
                name: label.name.clone(),
                detail: panic_detail(payload.as_ref()),
            });
        }
    }

    if samples.is_empty() {
        return Err(SuramakError::NoAudioData);
    }

    // Truncation: a declared length minus a 100 ms codec-delay/padding tolerance
    // is the floor a complete file must reach. Short files fail with a named,
    // actionable error instead of silently importing half a voice note.
    if let Some(err) = truncation_error(label, declared_frames, samples.len(), channels, rate) {
        return Err(err);
    }

    Ok(AudioBuffer {
        rate,
        channels,
        samples,
    })
}

/// The packet loop, factored out so [`decode_stream`] can wrap it in
/// `catch_unwind` (the decoder is foreign code).
fn decode_packets(
    format: &mut dyn symphonia::core::formats::FormatReader,
    decoder: &mut dyn symphonia::core::codecs::Decoder,
    track_id: u32,
    samples: &mut Vec<f32>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), SuramakError> {
    loop {
        // Cooperative cancellation: one check per demuxed packet (M15.2).
        if cancelled() {
            return Err(SuramakError::Cancelled);
        }
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
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

        append_samples(samples, decoder.decode(&packet)?);
    }
    Ok(())
}

/// The named truncation error when fewer frames than the container declared were
/// decoded (100 ms tolerance for codec delay/padding), else `None`.
fn truncation_error(
    label: &InputLabel,
    declared_frames: Option<u64>,
    sample_count: usize,
    channels: u16,
    rate: u32,
) -> Option<SuramakError> {
    let expected = declared_frames.filter(|frames| *frames > 0)?;
    let decoded = (sample_count / usize::from(channels.max(1))) as u64;
    let tolerance = u64::from(rate) / 10;
    if decoded.saturating_add(tolerance) >= expected {
        return None;
    }
    let got_s = decoded as f64 / f64::from(rate);
    let want_s = expected as f64 / f64::from(rate);
    Some(SuramakError::Truncated {
        name: label.name.clone(),
        detail: format!("decoded {got_s:.2} s of a declared {want_s:.2} s"),
    })
}

/// A panic payload as text (most panics carry `&str` or `String`).
fn panic_detail(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown decoder panic".to_string()
    }
}

/// Ogg capture pattern (`OggS`), the only container that hides its total length
/// from a truncated file.
const OGG_CAPTURE: &[u8] = b"OggS";
/// The last Ogg page is at most 65,307 bytes; 128 KiB of tail always contains it.
const OGG_TAIL_BYTES: u64 = 128 * 1024;

/// Structural truncation check for an on-disk file (see [`bytes_truncation_detail`]).
fn file_truncation_detail(file: &mut std::fs::File) -> std::io::Result<Option<String>> {
    let len = file.metadata()?.len();
    let mut head = [0u8; 512];
    let read = file.read(&mut head)?;
    let head = &head[..read];
    if head.starts_with(OGG_CAPTURE) {
        let tail_len = len.min(OGG_TAIL_BYTES) as usize;
        let mut tail = vec![0u8; tail_len];
        file.seek(SeekFrom::Start(len - tail_len as u64))?;
        file.read_exact(&mut tail)?;
        return Ok(ogg_tail_is_truncated(&tail)
            .then(|| "the Ogg stream ends before its end-of-stream page".to_string()));
    }
    Ok(mkv_truncation_detail(head, len))
}

/// The same check over in-memory bytes (the `decode_bytes` path).
fn bytes_truncation_detail(data: &[u8]) -> Option<String> {
    let len = data.len() as u64;
    if data.starts_with(OGG_CAPTURE) {
        let tail_start = data.len().saturating_sub(OGG_TAIL_BYTES as usize);
        return ogg_tail_is_truncated(&data[tail_start..])
            .then(|| "the Ogg stream ends before its end-of-stream page".to_string());
    }
    mkv_truncation_detail(&data[..data.len().min(512)], len)
}

/// Matroska/WebM structural completeness: the Segment element's declared size
/// must fit inside the file. `None` for a non-Matroska file or a live/unknown
/// segment size (which cannot be judged this way).
fn mkv_truncation_detail(head: &[u8], file_len: u64) -> Option<String> {
    const EBML_HEADER: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];
    const SEGMENT_ID: [u8; 4] = [0x18, 0x53, 0x80, 0x67];
    if !head.starts_with(&EBML_HEADER) {
        return None;
    }
    let (ebml_size, ebml_len, ebml_unknown) = ebml_vint(head, 4)?;
    if ebml_unknown {
        return None;
    }
    let segment_at = 4usize
        .checked_add(ebml_len)?
        .checked_add(ebml_size as usize)?;
    if head.get(segment_at..segment_at + 4)? != SEGMENT_ID {
        return None;
    }
    let (segment_size, segment_len, segment_unknown) = ebml_vint(head, segment_at + 4)?;
    if segment_unknown {
        return None;
    }
    let declared_end = (segment_at as u64)
        .checked_add(4)?
        .checked_add(segment_len as u64)?
        .checked_add(segment_size)?;
    (declared_end > file_len).then(|| {
        format!("the Matroska segment declares {declared_end} bytes, the file has only {file_len}")
    })
}

/// Parses an EBML variable-length integer: `(value, byte length, unknown-size)`.
///
/// The marker bit in the first byte gives the byte length; `value` has the
/// marker removed. An all-ones value is EBML's "unknown size" (live streams).
fn ebml_vint(data: &[u8], at: usize) -> Option<(u64, usize, bool)> {
    let first = *data.get(at)?;
    if first == 0 {
        return None;
    }
    let length = first.leading_zeros() as usize + 1;
    if length > 8 || data.len() < at + length {
        return None;
    }
    let mask = 0x80u8 >> (length - 1);
    let mut value = u64::from(first & (mask - 1));
    for &byte in &data[at + 1..at + length] {
        value = (value << 8) | u64::from(byte);
    }
    let unknown = value == (1u64 << (7 * length)) - 1;
    Some((value, length, unknown))
}

/// `tail` is the last (up to) 128 KiB of an Ogg stream: true when its last
/// **valid** page (CRC checked) is incomplete or lacks the end-of-stream flag.
fn ogg_tail_is_truncated(tail: &[u8]) -> bool {
    let mut end = tail.len();
    while let Some(pos) = find_last(tail, b"OggS", end) {
        // Fixed header (27 bytes) + segment table must both be present.
        if tail.len() >= pos + 27 {
            let version = tail[pos + 4];
            let flags = tail[pos + 5];
            let n_segments = usize::from(tail[pos + 26]);
            if version == 0 && tail.len() >= pos + 27 + n_segments {
                let table = &tail[pos + 27..pos + 27 + n_segments];
                let body_len: usize = table.iter().map(|&b| usize::from(b)).sum();
                let page_len = 27 + n_segments + body_len;
                if pos + page_len <= tail.len()
                    && ogg_crc32(&tail[pos..pos + page_len])
                        == u32::from_le_bytes([
                            tail[pos + 22],
                            tail[pos + 23],
                            tail[pos + 24],
                            tail[pos + 25],
                        ])
                {
                    // A real page. If the bytes end before it does, the cut is
                    // inside it; if it is not the EOS page, the stream stopped
                    // early.
                    return flags & 0x04 == 0;
                }
            }
        }
        if pos == 0 {
            break;
        }
        end = pos;
    }
    // Not even one verifiable page in the tail: the stream cannot be complete.
    true
}

/// The last occurrence of `needle` in `haystack[..end]`.
fn find_last(haystack: &[u8], needle: &[u8], end: usize) -> Option<usize> {
    if needle.is_empty() || end < needle.len() {
        return None;
    }
    (0..=end - needle.len())
        .rev()
        .find(|&index| haystack[index..].starts_with(needle))
}

/// Ogg's page CRC: CRC-32, polynomial `0x04C11DB7`, init 0, no reflection, no
/// final xor, computed with the four checksum bytes treated as zero.
fn ogg_crc32(page: &[u8]) -> u32 {
    let mut crc: u32 = 0;
    for (index, &byte) in page.iter().enumerate() {
        let byte = if (22..26).contains(&index) { 0 } else { byte };
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Maps a signed integer PCM sample onto `[-1.0, 1.0]`.
///
/// `full_scale` is the magnitude of the format's **negative** extreme (128,
/// 32_768, 2^23, 2^31), not `MAX` (SUR-LOW-05): dividing by `MAX` maps `MIN` to
/// −1.00003…, outside the normalized interval the rest of the pipeline assumes.
/// The clamp makes that invariant explicit even if a sample overshoots.
fn normalize_pcm(sample: f32, full_scale: f32) -> f32 {
    (sample / full_scale).clamp(-1.0, 1.0)
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
                    out.push(normalize_pcm(b.chan(ch)[i] as f32, 128.0));
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
                    out.push(normalize_pcm(b.chan(ch)[i] as f32, 32_768.0));
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
                    out.push(normalize_pcm(b.chan(ch)[i].0 as f32, 8_388_608.0));
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
                    out.push(normalize_pcm(b.chan(ch)[i] as f32, 2_147_483_648.0));
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

/// The audio duration of a file, read from the container's headers — **no
/// samples are decoded** (a 2-hour file costs one seek, not two hours of CPU).
///
/// M14e: the transcribe card interpolates its progress bar between whisper's
/// (sparse) progress events using the wall clock against this duration, so the
/// bar moves on short files instead of sitting at 0 %.
///
/// `None` when the container does not carry a frame count (a headerless MP3, a
/// stream) — callers must stay honest about the missing size ("estimating")
/// rather than invent one. The value is the *nominal* duration: a container
/// with a wrong header yields a wrong estimate, which is exactly what the 97 %
/// clamp is for.
pub fn probe_duration_ms(path: &std::path::Path) -> Option<u64> {
    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)?;
    let params = &track.codec_params;
    let frames = params.n_frames?;
    if frames == 0 {
        return None;
    }
    // Frame count over the sample rate is the normal answer (wav/flac/mp4).
    if let Some(rate) = params.sample_rate.filter(|rate| *rate > 0) {
        return Some(frames.saturating_mul(1000) / u64::from(rate));
    }
    // A container that only knows its timebase still answers honestly.
    let time = params.time_base?.calc_time(frames);
    Some(
        time.seconds
            .saturating_mul(1000)
            .saturating_add((time.frac * 1000.0).round() as u64),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    /// SUR-LOW-05: every signed integer width must map `MIN` to exactly −1.0 and
    /// `MAX` to just under +1.0 — the asymmetry of `/MAX` pushed the negative
    /// peak past the normalized range (and into whisper's input).
    #[test]
    fn signed_pcm_normalization_is_symmetric_at_full_scale() {
        // 8-bit: -128 → -1.0, 127 → 0.9921875
        assert_eq!(normalize_pcm(i8::MIN as f32, 128.0), -1.0);
        assert!((normalize_pcm(i8::MAX as f32, 128.0) - 127.0 / 128.0).abs() < 1e-6);
        // 16-bit: the width the audit cited (-32768 → -1.0000305 before the fix).
        assert_eq!(normalize_pcm(i16::MIN as f32, 32_768.0), -1.0);
        assert!((normalize_pcm(i16::MAX as f32, 32_768.0) - 32_767.0 / 32_768.0).abs() < 1e-6);
        // 24-bit and 32-bit use the same rule (symphonia's i24/i32 sources).
        assert_eq!(normalize_pcm(-8_388_608.0, 8_388_608.0), -1.0);
        assert_eq!(normalize_pcm(-2_147_483_648.0, 2_147_483_648.0), -1.0);
        // Digital silence stays zero, and no value escapes the interval.
        assert_eq!(normalize_pcm(0.0, 32_768.0), 0.0);
        assert_eq!(normalize_pcm(40_000.0, 32_768.0), 1.0);
        assert_eq!(normalize_pcm(-40_000.0, 32_768.0), -1.0);
    }

    /// M14e: the duration probe reads the header and never decodes samples —
    /// a 2 s WAV answers ~2000 ms, a file that is not audio answers `None`
    /// (the card then says "estimating" instead of inventing a size).
    #[test]
    fn the_duration_probe_reads_headers_and_refuses_what_it_cannot_know() {
        let dir = std::env::temp_dir().join(format!("suramak-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let wav = dir.join("two-seconds.wav");
        let samples = vec![0.0f32; 32_000];
        write_wav(&wav, &samples, 16_000, 1).expect("write the fixture");
        let ms = probe_duration_ms(&wav).expect("a WAV header carries its frame count");
        assert!(
            (1_990..=2_010).contains(&ms),
            "2 s at 16 kHz must probe as ~2000 ms, got {ms}"
        );

        // A file that is not audio (and a missing file) answer `None`.
        let junk = dir.join("not-audio.txt");
        std::fs::write(&junk, b"hello").unwrap();
        assert_eq!(probe_duration_ms(&junk), None);
        assert_eq!(probe_duration_ms(&dir.join("missing.wav")), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A synthetic Ogg page (one lacing segment) with a valid CRC.
    fn ogg_page(flags: u8, sequence: u32, body: &[u8]) -> Vec<u8> {
        assert!(body.len() < 255);
        let mut page = Vec::new();
        page.extend_from_slice(b"OggS");
        page.push(0); // version
        page.push(flags); // 0x04 = end of stream
        page.extend_from_slice(&0u64.to_le_bytes()); // granule
        page.extend_from_slice(&1u32.to_le_bytes()); // serial
        page.extend_from_slice(&sequence.to_le_bytes());
        page.extend_from_slice(&[0, 0, 0, 0]); // checksum placeholder
        page.push(1); // one segment
        page.push(body.len() as u8);
        page.extend_from_slice(body);
        let crc = ogg_crc32(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        page
    }

    #[test]
    fn the_ogg_tail_check_requires_a_complete_end_of_stream_page() {
        // One complete EOS page: not truncated.
        let complete = ogg_page(0x04, 0, b"last packet");
        assert!(!ogg_tail_is_truncated(&complete));

        // A page without the EOS flag: the stream stopped early.
        let no_eos = ogg_page(0x00, 0, b"more to come");
        assert!(ogg_tail_is_truncated(&no_eos));

        // A body cut short: the last verifiable page is the earlier, non-EOS one.
        let mut cut = ogg_page(0x00, 0, b"first page");
        cut.extend_from_slice(&ogg_page(0x04, 1, b"final page body"));
        cut.truncate(cut.len() - 5);
        assert!(ogg_tail_is_truncated(&cut));

        // Complete multi-page stream: the last page carries EOS.
        let mut two = ogg_page(0x00, 0, b"first page");
        two.extend_from_slice(&ogg_page(0x04, 1, b"final page"));
        assert!(!ogg_tail_is_truncated(&two));

        // Garbage without any page: cannot be a complete stream.
        assert!(ogg_tail_is_truncated(b"not an ogg page"));
    }

    #[test]
    fn the_matroska_segment_size_is_checked_against_the_file_length() {
        // EBML header (4-byte id, 1-byte size = 12, 12 zero bytes) then a
        // Segment (4-byte id, 1-byte size) with a 7-byte body.
        let mut head = vec![0x1A, 0x45, 0xDF, 0xA3, 0x8C];
        head.extend_from_slice(&[0u8; 12]);
        head.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0x87]);
        head.extend_from_slice(&[0u8; 7]);
        let total = head.len() as u64;
        assert_eq!(mkv_truncation_detail(&head, total), None);
        let detail = mkv_truncation_detail(&head, total - 5).expect("short file");
        assert!(detail.contains("declares"), "{detail}");

        // Unknown segment size (all value bits set) cannot be judged.
        let mut live = vec![0x1A, 0x45, 0xDF, 0xA3, 0x8C];
        live.extend_from_slice(&[0u8; 12]);
        live.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0xFF]);
        assert_eq!(mkv_truncation_detail(&live, 10), None);
        // A non-Matroska file is not our business.
        assert_eq!(mkv_truncation_detail(b"OggSxxxx", 8), None);
    }

    #[test]
    fn ebml_vints_decode_their_marker_length_and_unknown_size() {
        assert_eq!(ebml_vint(&[0x80], 0), Some((0, 1, false)));
        assert_eq!(ebml_vint(&[0x81], 0), Some((1, 1, false)));
        assert_eq!(ebml_vint(&[0x40, 0x01], 0), Some((1, 2, false)));
        assert_eq!(ebml_vint(&[0xFF], 0), Some((127, 1, true)));
        assert_eq!(ebml_vint(&[0x00], 0), None);
    }
}
