//! The M26B PCM handoff: decoded audio becomes a byte file, then a `PcmSlice`.
//!
//! A job's 16 kHz mono f32 audio is written by
//! [`crate::decode::decode_to_pcm16k_mono`] to `{temp}/suramak_*.pcm32` and
//! handed to the engine through [`PcmSlice`]. On unix (Linux, macOS, Android)
//! the file is `mmap`ed read-only and `posix_madvise(MADV_SEQUENTIAL)` tells the
//! kernel the access pattern, so the PCM is clean, reclaimable file pages
//! instead of an anonymous `Vec<f32>` that LMKD cannot evict. Windows has no
//! `mmap` in `std` and this slice may not add a crate: [`PcmSlice::open_temp`]
//! there reads the same bytes through [`WindowPcmReader`] into one f32 buffer
//! (`File::read_exact_at`, 64 KiB windows) — the API is identical, the memory
//! win is unix-only, and that difference is documented rather than hidden.
//!
//! `PcmSlice` owns the temp file: `Drop` unmaps it and removes it on success,
//! cancellation and unwinding alike. [`sweep_orphaned_temp_pcm`] is the startup
//! net for a process that was killed before any destructor ran.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::SuramakError;

/// Prefix of every decode temp file. The startup sweep matches this prefix.
pub const TEMP_PCM_PREFIX: &str = "suramak_";
/// Extension of the raw 16 kHz mono f32le temp file.
pub const TEMP_PCM_EXTENSION: &str = "pcm32";
/// 16 kHz mono f32: 64 000 bytes per second of audio.
pub const PCM_BYTES_PER_SECOND: u64 = 64_000;
/// Free space the preflight must leave untouched beyond the decoded audio.
pub const TEMP_PCM_HEADROOM_BYTES: u64 = 100_000_000;
/// Bytes a `PcmSlice` windowed read holds at once (Windows fallback).
const WINDOW_BYTES: usize = 64 * 1024;

/// The directory decode temp files live in.
///
/// Desktop (CLI, macOS, Windows, Linux) uses the process temp directory, as the
/// work order and the design name it. Android cannot write `/data/local/tmp`
/// from an app sandbox, so once the shell has installed the app-private data
/// dir the slice uses its `tmp/` child instead — the same volume the job's
/// media already lives on, and a directory the sweep can read.
pub fn temp_pcm_dir() -> PathBuf {
    #[cfg(target_os = "android")]
    if let Some(dir) = crate::android::data_dir_override() {
        let dir = dir.join("tmp");
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }
    std::env::temp_dir()
}

/// A unique path for one job's decoded audio: `{temp}/suramak_{pid}_{n}_{t}.pcm32`.
///
/// The pid keeps two processes from colliding; the counter and timestamp keep
/// two jobs in one process apart. The `suramak_*.pcm32` shape is what the
/// startup sweep recognises.
pub fn temp_pcm_path() -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    temp_pcm_dir().join(format!(
        "{TEMP_PCM_PREFIX}{}_{sequence}_{nanos}.{TEMP_PCM_EXTENSION}",
        std::process::id()
    ))
}

/// Deletes every `suramak_*.pcm32` in `dir`; returns how many were removed.
///
/// Called once at process start, before any job can create a file. A kill
/// (`SIGKILL`, Android LMKD, a power cut) leaves the temp file behind because
/// no destructor runs; this is the only cleanup that can reach those files.
/// Failures are ignored: a file another process still holds is not this
/// process's to fail over.
pub fn sweep_orphaned_temp_pcm(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(TEMP_PCM_PREFIX)
            && name.ends_with(&format!(".{TEMP_PCM_EXTENSION}"))
            && std::fs::remove_file(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

/// Bytes the disk preflight requires for `duration_ms` of decoded audio:
/// `duration_seconds × 64 000 + 100 MB`, with an unknown duration charged the
/// headroom only (the caller cannot invent a length).
pub fn temp_space_needed(duration_ms: Option<u64>) -> u64 {
    duration_ms
        .unwrap_or(0)
        .saturating_mul(PCM_BYTES_PER_SECOND / 1000)
        .saturating_add(TEMP_PCM_HEADROOM_BYTES)
}

/// The named refusal when the temp volume cannot hold the decoded audio.
///
/// `available` is `None` when the platform cannot answer; an unknown free-space
/// figure does not block a job (the same rule as the model download preflight).
pub fn check_temp_space(
    dir: &Path,
    needed: u64,
    available: Option<u64>,
) -> Result<(), SuramakError> {
    let Some(available) = available else {
        return Ok(());
    };
    if available >= needed {
        return Ok(());
    }
    Err(SuramakError::TempDiskFull {
        dir: dir.to_path_buf(),
        needed,
        available,
    })
}

/// One job's 16 kHz mono f32 audio, ready for the engine.
///
/// `as_slice()` is valid for the lifetime of the value. On unix the storage is
/// a read-only private mapping of the temp file; on other targets it is one
/// `Vec<f32>` read through [`WindowPcmReader`]. The temp file (when this value
/// owns one) is removed by `Drop`, after the mapping is gone.
pub struct PcmSlice {
    storage: Storage,
    /// `Some` only when this value owns a temp file to delete.
    owned_path: Option<PathBuf>,
    samples: usize,
}

enum Storage {
    #[cfg(unix)]
    Mapped(Mapping),
    Buffered(Vec<f32>),
}

impl PcmSlice {
    /// Maps `path` (a `suramak_*.pcm32` written by the streaming decoder) and
    /// takes ownership of it: the file is deleted when this value drops.
    ///
    /// The file must be a whole number of little-endian f32 samples.
    pub fn open_temp(path: &Path) -> Result<Self, SuramakError> {
        let bytes = std::fs::metadata(path)?.len();
        if bytes == 0 {
            return Err(SuramakError::NoAudioData);
        }
        if bytes % 4 != 0 {
            return Err(SuramakError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{path:?} is not a whole number of f32 samples ({bytes} bytes)"),
            )));
        }
        let samples = bytes as usize / 4;
        #[cfg(unix)]
        let storage = Storage::Mapped(Mapping::open(path, bytes as usize)?);
        #[cfg(not(unix))]
        let storage = Storage::Buffered(WindowPcmReader::open(path)?.into_samples());
        Ok(Self {
            storage,
            owned_path: Some(path.to_path_buf()),
            samples,
        })
    }

    /// Wraps an in-memory buffer (the live-microphone path and the acceptance
    /// test's pre-M26B reference). No file is owned.
    #[doc(hidden)]
    pub fn from_samples(samples: Vec<f32>) -> Self {
        Self {
            samples: samples.len(),
            storage: Storage::Buffered(samples),
            owned_path: None,
        }
    }

    /// The fast path for a recording this app wrote: a 16 kHz mono PCM16 WAV.
    ///
    /// The `data` chunk is mapped (unix) and its i16 samples are converted with
    /// the exact same `sample / 32768` rule as the symphonia decode path — no
    /// container demux, no resampler. `None` means "not that shape; decode it
    /// normally", and every failure to read falls back the same way so the
    /// normal decoder can report the real error.
    ///
    /// The conversion needs one `Vec<f32>` because whisper's C API takes a
    /// contiguous float slice. That is still half the peak of the old
    /// decode-into-`Vec<f32>` + resample-into-a-second-`Vec<f32>` path, and it
    /// skips both the decoder and the FFT resampler.
    pub fn from_recording_wav(path: &Path) -> Option<Self> {
        let span = wav_pcm16_mono_16k_span(path)?;
        let samples = read_pcm16_span_as_f32(path, &span).ok()?;
        Some(Self::from_samples(samples))
    }

    /// The samples. 16 kHz mono f32 for every constructor.
    pub fn as_slice(&self) -> &[f32] {
        match &self.storage {
            #[cfg(unix)]
            Storage::Mapped(mapping) => mapping.as_f32(),
            Storage::Buffered(samples) => samples,
        }
    }

    pub fn len(&self) -> usize {
        self.samples
    }

    pub fn is_empty(&self) -> bool {
        self.samples == 0
    }

    /// Bytes the engine will read — the queue's decode-ahead budget speaks in
    /// bytes and must keep counting the same quantity it counted before.
    pub fn byte_len(&self) -> usize {
        self.samples.saturating_mul(std::mem::size_of::<f32>())
    }

    /// `"mmap"` on unix, `"read_exact_at"` on the windowed fallback, `"memory"`
    /// for a buffer that owns no file. The cfg tests pin the platform choice.
    pub fn backend(&self) -> &'static str {
        match &self.storage {
            #[cfg(unix)]
            Storage::Mapped(_) => "mmap",
            Storage::Buffered(_) if self.owned_path.is_some() => "read_exact_at",
            Storage::Buffered(_) => "memory",
        }
    }
}

impl std::fmt::Debug for PcmSlice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PcmSlice")
            .field("samples", &self.samples)
            .field("backend", &self.backend())
            .field("owned", &self.owned_path.is_some())
            .finish()
    }
}

impl Drop for PcmSlice {
    fn drop(&mut self) {
        // Unmap before removing the path (the unix storage owns the mapping;
        // the buffered fallback owns no handle). Then the temp file goes.
        #[cfg(unix)]
        {
            let storage = std::mem::replace(&mut self.storage, Storage::Buffered(Vec::new()));
            drop(storage);
        }
        if let Some(path) = self.owned_path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

// SAFETY: the mapped bytes are read-only (`PROT_READ`, `MAP_PRIVATE`) and owned
// exclusively by this value; no code writes through the pointer. Sending the
// handle to the worker thread is the whole point (the engine runs there).
unsafe impl Send for PcmSlice {}
// SAFETY: `as_slice` is the only access and returns an immutable slice.
unsafe impl Sync for PcmSlice {}

#[cfg(unix)]
struct Mapping {
    ptr: *mut libc::c_void,
    len: usize,
}

/// `posix_madvise(MADV_SEQUENTIAL)`: the kernel may read ahead and drop pages
/// behind the audio cursor.
///
/// libc 0.2 binds `posix_madvise` on the desktop unix targets but not on
/// Android, where Bionic still exports the POSIX call (API 21+; minSdk is 29).
/// Declaring the one symbol keeps the Android path the documented `madvise`
/// call rather than silently using the Linux-only spelling.
#[cfg(all(unix, not(target_os = "android")))]
unsafe fn advise_sequential(ptr: *mut libc::c_void, len: usize) {
    libc::posix_madvise(ptr, len, libc::POSIX_MADV_SEQUENTIAL);
}

#[cfg(target_os = "android")]
unsafe fn advise_sequential(ptr: *mut libc::c_void, len: usize) {
    // POSIX_MADV_SEQUENTIAL is 2 in Bionic's <sys/mman.h>.
    const POSIX_MADV_SEQUENTIAL: libc::c_int = 2;
    extern "C" {
        fn posix_madvise(
            address: *mut libc::c_void,
            length: libc::size_t,
            advice: libc::c_int,
        ) -> libc::c_int;
    }
    posix_madvise(ptr, len, POSIX_MADV_SEQUENTIAL);
}

#[cfg(unix)]
impl Mapping {
    fn open(path: &Path, len: usize) -> Result<Self, SuramakError> {
        let file = File::open(path)?;
        use std::os::unix::io::AsRawFd;
        // SAFETY: a fresh anonymous-private read-only mapping of `len` bytes of
        // a live file descriptor. The pointer is checked against MAP_FAILED;
        // the file may be closed immediately after (the mapping keeps its own
        // reference), but keeping `file` alive until the call returns is the
        // conservative order.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(SuramakError::Io(io::Error::last_os_error()));
        }
        // Sequential advice is a hint: a kernel that refuses it must not fail
        // the job. The whole slice is read front to back by whisper's mel pass.
        // SAFETY: `ptr`/`len` describe the live mapping above.
        unsafe {
            advise_sequential(ptr, len);
        }
        Ok(Self { ptr, len })
    }

    fn as_f32(&self) -> &[f32] {
        // SAFETY: the mapping is page-aligned (f32-aligned), `len` is a multiple
        // of 4, and it is readable for the value's lifetime. No mutable alias
        // exists.
        unsafe { std::slice::from_raw_parts(self.ptr as *const f32, self.len / 4) }
    }
}

#[cfg(unix)]
impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: `ptr`/`len` are the values `mmap` returned and never changed.
        unsafe {
            libc::munmap(self.ptr, self.len);
        }
    }
}

/// The pure-`std::fs` window reader behind the Windows fallback.
///
/// It reads a `.pcm32` through one reusable [`WINDOW_BYTES`] buffer with
/// `read_exact_at` (positional, no seek state), converting each little-endian
/// f32 window into one output buffer. On unix this type is not the production
/// reader, but it is compiled and tested there: the byte stream it produces is
/// asserted identical to the `mmap` slice, so the Windows branch is exercised
/// by the host suite rather than trusted.
pub struct WindowPcmReader {
    samples: Vec<f32>,
}

impl WindowPcmReader {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let bytes = file.metadata()?.len();
        if bytes % 4 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a .pcm32 file must be a whole number of f32 samples",
            ));
        }
        let mut samples = Vec::with_capacity(bytes as usize / 4);
        let mut window = vec![0u8; WINDOW_BYTES];
        let mut offset = 0u64;
        while offset < bytes {
            let take = (bytes - offset).min(WINDOW_BYTES as u64) as usize;
            read_exact_at(&file, &mut window[..take], offset)?;
            for word in window[..take].chunks_exact(4) {
                samples.push(f32::from_le_bytes([word[0], word[1], word[2], word[3]]));
            }
            offset += take as u64;
        }
        Ok(Self { samples })
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.samples
    }

    #[cfg(not(unix))]
    fn into_samples(self) -> Vec<f32> {
        self.samples
    }
}

/// Positional read. Windows uses `File::read_exact_at`; every other target
/// (including the Linux host that tests this fallback) seeks a clone and reads,
/// which is the same byte-for-byte contract without a platform crate.
#[cfg(windows)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    file.read_exact_at(buf, offset)
}

#[cfg(not(windows))]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf)
}

/// The `data` chunk of a 16 kHz mono PCM16 WAV.
struct WavPcm16Span {
    offset: u64,
    len: u64,
}

/// Parses just the RIFF chunk table: `Some` only for PCM16, one channel,
/// 16 000 Hz — the exact shape [`crate::decode::write_wav_i16`] produces.
fn wav_pcm16_mono_16k_span(path: &Path) -> Option<WavPcm16Span> {
    let mut file = File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    let mut riff = [0u8; 12];
    file.read_exact(&mut riff).ok()?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return None;
    }
    let mut fmt_ok = false;
    let mut position = 12u64;
    while position.checked_add(8)? <= file_len {
        file.seek(SeekFrom::Start(position)).ok()?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header).ok()?;
        let id = &header[0..4];
        let size = u64::from(u32::from_le_bytes([
            header[4], header[5], header[6], header[7],
        ]));
        let body = position + 8;
        if body.checked_add(size)? > file_len {
            return None;
        }
        match id {
            b"fmt " if size >= 16 => {
                let mut fmt = [0u8; 16];
                file.read_exact(&mut fmt).ok()?;
                let format = u16::from_le_bytes([fmt[0], fmt[1]]);
                let channels = u16::from_le_bytes([fmt[2], fmt[3]]);
                let rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
                let block_align = u16::from_le_bytes([fmt[12], fmt[13]]);
                let bits = u16::from_le_bytes([fmt[14], fmt[15]]);
                fmt_ok = format == 1
                    && channels == 1
                    && rate == 16_000
                    && bits == 16
                    && block_align == 2;
            }
            b"data" if fmt_ok => {
                if size == 0 || size % 2 != 0 {
                    return None;
                }
                return Some(WavPcm16Span {
                    offset: body,
                    len: size,
                });
            }
            _ => {}
        }
        // RIFF chunks are word-aligned: an odd-sized chunk carries one pad byte.
        position = body + size + (size & 1);
    }
    None
}

fn read_pcm16_span_as_f32(path: &Path, span: &WavPcm16Span) -> io::Result<Vec<f32>> {
    #[cfg(unix)]
    {
        let file = File::open(path)?;
        use std::os::unix::io::AsRawFd;
        // The data chunk starts at byte 44 for hound's header, which is not a
        // page boundary; map the whole (small, one recording) file and slice.
        let len = file.metadata()?.len() as usize;
        // SAFETY: read-only private mapping of a live file, checked below.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
        let outcome = pcm16_bytes_to_f32(
            bytes
                .get(span.offset as usize..(span.offset + span.len) as usize)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "short WAV data"))?,
        );
        // SAFETY: the values are the ones `mmap` returned.
        unsafe {
            libc::munmap(ptr, len);
        }
        outcome
    }
    #[cfg(not(unix))]
    {
        let file = File::open(path)?;
        let mut bytes = vec![0u8; span.len as usize];
        read_exact_at(&file, &mut bytes, span.offset)?;
        pcm16_bytes_to_f32(&bytes)
    }
}

/// The one i16→f32 rule shared with the symphonia S16 branch: divide by the
/// negative full scale and clamp (SUR-LOW-05), so the fast path's samples are
/// bit-identical to the decode path's.
fn pcm16_bytes_to_f32(bytes: &[u8]) -> io::Result<Vec<f32>> {
    if bytes.len() % 2 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "odd PCM16 data length",
        ));
    }
    let mut samples = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let value = i16::from_le_bytes([pair[0], pair[1]]);
        samples.push(crate::decode::normalize_i16(value));
    }
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{TEMP_PCM_PREFIX}{}_{name}.{TEMP_PCM_EXTENSION}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn write_samples(path: &Path, samples: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, &bytes).expect("write pcm32");
        bytes
    }

    /// The window fallback (the Windows reader) must reproduce the exact byte
    /// stream the unix mapping exposes — including a value with a negative zero
    /// and one with an exponent that a lossy conversion would round.
    #[test]
    fn the_window_reader_matches_the_mapped_bytes() {
        let path = temp_file("window");
        let samples = [
            0.0f32,
            -0.0,
            1.0,
            -1.0,
            f32::from_bits(0x3f80_0001),
            f32::from_bits(0x8000_0001),
            f32::MIN_POSITIVE,
        ];
        write_samples(&path, &samples);

        let windowed = WindowPcmReader::open(&path).expect("window read");
        assert_eq!(windowed.as_slice().len(), samples.len());
        for (index, (read, expected)) in windowed.as_slice().iter().zip(&samples).enumerate() {
            assert_eq!(read.to_bits(), expected.to_bits(), "sample {index}");
        }

        let _ = std::fs::remove_file(&path);
    }

    /// The platform choice is a compile-time fact, asserted where it is
    /// compiled: unix maps, the fallback reads windows.
    #[cfg(unix)]
    #[test]
    fn unix_selects_the_mmap_backend() {
        let path = temp_file("backend-unix");
        write_samples(&path, &[0.0; 4]);
        let slice = PcmSlice::open_temp(&path).expect("open temp");
        assert_eq!(slice.backend(), "mmap");
    }

    #[cfg(not(unix))]
    #[test]
    fn the_fallback_selects_the_windowed_backend() {
        let path = temp_file("backend-window");
        write_samples(&path, &[0.0; 4]);
        let slice = PcmSlice::open_temp(&path).expect("open temp");
        assert_eq!(slice.backend(), "read_exact_at");
    }

    /// `open_temp` maps on unix and must expose the same bits; dropping it
    /// deletes the temp file (the RAII contract).
    #[test]
    fn open_temp_maps_the_file_and_drop_deletes_it() {
        let path = temp_file("map");
        let samples = vec![0.25f32, -0.5, 0.0, 1.0 / 3.0];
        write_samples(&path, &samples);

        let slice = PcmSlice::open_temp(&path).expect("open temp");
        assert_eq!(slice.len(), samples.len());
        assert_eq!(slice.byte_len(), samples.len() * 4);
        #[cfg(unix)]
        assert_eq!(slice.backend(), "mmap");
        #[cfg(not(unix))]
        assert_eq!(slice.backend(), "read_exact_at");
        for (index, (read, expected)) in slice.as_slice().iter().zip(&samples).enumerate() {
            assert_eq!(read.to_bits(), expected.to_bits(), "sample {index}");
        }
        drop(slice);
        assert!(!path.exists(), "Drop must remove the temp file");
    }

    /// An empty or truncated `.pcm32` is a named error, never a zero-length
    /// slice the engine would read past.
    #[test]
    fn an_empty_or_unaligned_pcm32_is_refused() {
        let empty = temp_file("empty");
        std::fs::write(&empty, b"").unwrap();
        assert!(matches!(
            PcmSlice::open_temp(&empty),
            Err(SuramakError::NoAudioData)
        ));
        let _ = std::fs::remove_file(&empty);

        let short = temp_file("short");
        std::fs::write(&short, [0u8; 7]).unwrap();
        assert!(matches!(
            PcmSlice::open_temp(&short),
            Err(SuramakError::Io(_))
        ));
        let _ = std::fs::remove_file(&short);
    }

    /// A panic while the slice is alive still runs `Drop` — the simulated
    /// mid-job panic leaves no `.pcm32` behind.
    #[test]
    fn unwinding_deletes_the_temp_file() {
        let path = temp_file("panic");
        write_samples(&path, &[0.0; 16]);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _slice = PcmSlice::open_temp(&path).expect("open temp");
            panic!("simulated job panic");
        }));
        assert!(outcome.is_err());
        assert!(!path.exists(), "a panic must not leak the temp file");
    }

    /// The preflight arithmetic is exactly `duration × 64 000 + 100 MB`, and
    /// the refusal is named `temp_disk_full`.
    #[test]
    fn the_disk_preflight_refuses_with_a_named_reason() {
        assert_eq!(temp_space_needed(Some(0)), TEMP_PCM_HEADROOM_BYTES);
        assert_eq!(temp_space_needed(Some(1000)), 100_000_000 + 64_000);
        assert_eq!(
            temp_space_needed(Some(30 * 60 * 1000)),
            100_000_000 + 30 * 60 * 64_000
        );
        // Unknown duration: the fixed headroom only.
        assert_eq!(temp_space_needed(None), TEMP_PCM_HEADROOM_BYTES);

        let dir = Path::new("/tmp");
        check_temp_space(dir, 1_000, Some(2_000)).expect("enough room");
        check_temp_space(dir, 1_000, None).expect("unknown free space does not block");
        let error = check_temp_space(dir, 2_000, Some(1_999)).expect_err("must refuse");
        let message = error.to_string();
        assert!(message.starts_with("temp_disk_full:"), "{message}");
        assert!(message.contains("2000"), "{message}");
        assert!(message.contains("1999"), "{message}");
    }

    /// A 16 kHz mono PCM16 WAV is the fast-path shape; anything else is
    /// `None` so the normal decoder and its actionable errors stay in charge.
    #[test]
    fn only_our_recording_shape_takes_the_fast_path() {
        let dir = std::env::temp_dir().join(format!("suramak-m26b-wav-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let ours = dir.join("ours.wav");
        crate::decode::write_wav_i16(&ours, &[0.0, 0.5, -0.5, 1.0], 16_000, 1).unwrap();
        let slice = PcmSlice::from_recording_wav(&ours).expect("fast path");
        assert_eq!(slice.len(), 4);

        let wrong_rate = dir.join("rate.wav");
        crate::decode::write_wav_i16(&wrong_rate, &[0.0; 8], 44_100, 1).unwrap();
        assert!(PcmSlice::from_recording_wav(&wrong_rate).is_none());

        let stereo = dir.join("stereo.wav");
        crate::decode::write_wav_i16(&stereo, &[0.0; 8], 16_000, 2).unwrap();
        assert!(PcmSlice::from_recording_wav(&stereo).is_none());

        let float = dir.join("float.wav");
        crate::decode::write_wav(&float, &[0.0; 8], 16_000, 1).unwrap();
        assert!(PcmSlice::from_recording_wav(&float).is_none());

        let not_wav = dir.join("not.wav");
        std::fs::write(&not_wav, b"not a riff file").unwrap();
        assert!(PcmSlice::from_recording_wav(&not_wav).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
