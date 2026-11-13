# M26B — stream the audio to disk, then map it

Slice 2 of `docs/reports/M26-memory-design.md` §B/§E. Before this slice every
job held the recording twice: `decode_file` built an interleaved `Vec<f32>` at
the source rate, `to_16khz_mono` built a second mono/resampled `Vec<f32>`, and
only then did whisper run. On a 30-minute 48 kHz stereo import that is 659 MiB
(decoded) + 330 MiB (mono copy) + 115 MiB (resampled) resident at the resample
peak. After this slice the decode+resample stage writes 16 kHz mono f32le to
`{temp}/suramak_*.pcm32` in bounded chunks, the engine reads a `mmap` of that
file, and the two full-file buffers are gone. One path serves every input:
desktop files, imports, Android shares and microphone recordings.

## 1. One streaming decode path

`suramak_core::decode::decode_to_pcm16k_mono(input, out_path)` (and its
cancellable form) is the only decode a job uses. It drives the same symphonia
demuxer, codec registry, truncation checks and panic boundary as `decode_file`;
the packet sink grew a second implementation instead of a second pipeline.

Byte-exactness is structural, not approximate:

* mono mixdown normalizes each channel and sums in channel order, exactly like
  `resample::mixdown_mono`;
* rubato is fed the same 1024-sample chunks (`STREAM_CHUNK_SAMPLES`), with the
  same last-chunk zero padding;
* the tail is cut to `round(frames × 16_000 / rate)`, the same rule as
  `to_16khz_mono`, including the "a short clip may end a few frames early"
  direction (the stream truncates, never pads);
* samples are written as little-endian f32 through a 64 KiB staging buffer.

Resident sample buffers are one decoded packet (symphonia's own buffer; the
codecs in this build top out at a few thousand frames), at most one
1024-sample chunk plus one packet's mono frames in `pending`, and rubato's FFT
scratch — tens of KiB, not a function of duration. The 8 MB sample budget is
two orders of magnitude away; there is no whole-file `Vec<f32>` and no
whole-file resample buffer anywhere on this path.

A failed, cancelled or panicking decode removes the partial file through
`TempPcmGuard`; only a successful `PcmSlice::open_temp` disarms it.

## 2. The fast path for our own recordings

`PcmSlice::from_recording_wav` parses the RIFF chunk table (no new crate) and
accepts exactly the shape `write_wav_i16` writes: PCM, one channel, 16 000 Hz,
16-bit. Its `data` chunk is mapped (unix) and converted to f32 with
`normalize_i16` — the same divide-by-32768-and-clamp rule as the symphonia S16
branch, moved into one shared function so the two cannot drift. No demuxer, no
decoder, no resampler. Anything else (44.1/48 kHz, stereo, float WAV, not a
WAV) returns `None` and falls through to the streaming decoder, which reports
the real error.

The conversion still needs one `Vec<f32>` because whisper.cpp's C API takes a
contiguous `const float *`. That is one allocation where the old path had two
(the decoded buffer is not built at all), and both the decoder and the FFT
resampler are skipped. The peak for a 16 kHz mono recording is therefore
`4 bytes/sample` instead of `8 bytes/sample` at the resample peak.

## 3. `PcmSlice`

| target | storage | notes |
|---|---|---|
| Linux / macOS / Android | `libc::mmap(PROT_READ, MAP_PRIVATE)` + `posix_madvise(POSIX_MADV_SEQUENTIAL)` | `as_slice()` is a `&[f32]` over the file pages; reclaimable by the OS. libc 0.2 does not bind `posix_madvise` for Android, so Bionic's symbol is declared in `pcm.rs` (API 21+, minSdk 29). |
| Windows / other | `WindowPcmReader`: `File::read_exact_at` into a reusable 64 KiB window, converted into one `Vec<f32>` | Pure `std::fs`, no new crate. The API is identical; the memory win is unix-only, and that is the documented difference, not a hidden fallback. |

`PcmSlice` owns its temp file: `Drop` unmaps (unix) and removes the path. A
`.pcm32` that is empty or not a whole number of f32 samples is a named error
rather than a slice with a bogus length. `unsafe impl Send/Sync` is justified by
the mapping being read-only and exclusively owned.

`pcm::sweep_orphaned_temp_pcm(temp_dir)` deletes `suramak_*.pcm32` at process
start. It is called from `suramak-cli`'s `main`, the desktop `run()` (after the
single-instance lock, so a second launch cannot sweep a live job's mapping) and
`suramak_android::android_main`.

The temp name is `{temp}/suramak_{pid}_{sequence}_{nanos}.pcm32` — a unique job
token instead of the queue's numeric id, because the pipeline does not carry
that id; the sweep shape and the single-owner rule are unchanged. On desktop
`{temp}` is the process temp directory; on Android, where an app cannot write
`/data/local/tmp`, `pcm::temp_pcm_dir()` is the shell's app-private data
directory plus `tmp/` (created on demand, on the same volume as the job's
media).

## 4. Whisper takes the slice

`whisper::transcribe` now takes `&PcmSlice` and calls
`state.full(params, samples.as_slice())`. `FullParams` is untouched: same
greedy `best_of: 1`, threads, token timestamps, language, prompt, progress and
abort callbacks, segment callback. The resume tail (`from_ms`) is now a
borrowed view inside the same mapping rather than a pre-sliced `&[f32]`; the
sample values and the timestamp offset are identical.

The queue's decode-ahead pool changed buffer type, not semantics: admission is
still counted in bytes (`PcmSlice::byte_len()`), the one-inference-slot and
`MAX_DECODE_AHEAD` bounds are unchanged, a cancelled item's late buffer is
still dropped, and `Job.samples` is still the optional decoded-ahead handoff.
The prefetch thread now writes `.pcm32` instead of growing a `Vec<f32>`.

## 5. Disk preflight

Before the temp file is created:

```text
needed = duration_seconds × 64 000 + 100 000 000      (f32 × 16 kHz + 100 MB)
refuse unless free_bytes >= needed
```

The duration comes from the container's declared frame count, or
`probe_duration_ms` (headers, no decode) when there is none; an unknowable
duration is charged the 100 MB headroom only. `statvfs` (unix/Android) or
`GetDiskFreeSpaceExW` (Windows) supplies the free bytes; an unavailable answer
does not block. The refusal is the named `temp_disk_full:` variant
(`SuramakError::TempDiskFull`), names the directory and both byte counts, and
the preflight runs before `create_new`, so no partial file exists.

## 6. Verification

### 6.1 Sample-level parity (the mechanism)

`cargo test -p suramak-core --test m26b_pcm` — 5 passed. Every fixture is
decoded both ways and compared with `f32::to_bits`:

| input | shape | result |
|---|---|---|
| `fixtures/audio/en.wav` | 16 kHz mono PCM16 | identical |
| `fixtures/audio/id.wav` | 16 kHz mono PCM16 | identical |
| `fixtures/audio/en.mp3` | 24 kHz mono MP3 | identical (resampler + tail truncation) |
| generated `stereo-44k.wav` | 44.1 kHz stereo | identical (mixdown + resampler) |

The fast path is compared to both the streaming and the in-memory decode of
`en.wav`; all three are bit-identical. `crates/suramak-android/tests/
decode_parity.rs` (4 passed) independently asserts that `FileDecoder`'s
streaming output is bit-identical to `to_16khz_mono(decode_file(...))` for the
Ogg/WebM/Matroska Opus fixtures.

### 6.2 Byte-identical transcripts (the acceptance)

`SURAMAK_REQUIRE_MODEL=1 cargo test -p suramak-asr --release --test
m26b_streaming -- --ignored --exact three_fixtures_produce_byte_identical_transcripts
--nocapture` — 1 passed in 288.29 s, tiny model, release profile. The old path
is `decode_16khz_mono_in_memory_for_tests` (`#[doc(hidden)]`, never called by
production) + `PcmSlice::from_samples`, run through the same pipeline with the
same params; the new path lets `prepare_samples` choose the fast/streaming
branch.

| fixture | segments | segment-text/timestamp digest |
|---|---|---|
| short en (`en.wav`, fast path) | 2 | `5dd0db6c399bbeb9` both |
| id (`id.wav`, fast path) | 3 | `63362fd00ba178c4` both |
| 20-minute synthesised 44.1 kHz WAV (streaming resample) | 80 | `8db922dedfc0db02` both |

The assertion is stronger than the digest: every segment's `idx`, `start_ms`,
`end_ms` and `text` is compared, and the segment counts must match.

### 6.3 Peak RSS (30 minutes, tiny, release, Linux)

The fixture is 30 minutes of 48 kHz **stereo** PCM16 (330 MB on disk; 10 s of
speech every 30 s, natural pitch). `SURAMAK_REQUIRE_MODEL=1 cargo test -p
suramak-asr --release --test m26b_streaming -- --ignored --exact
rss_30_minutes_streaming_is_200mb_lower --nocapture` — 1 passed in 418.37 s.
Each mode runs in its own child process of the test binary; the child prints
`VmHWM` from `/proc/self/status`.

| path | peak VmHWM |
|---|---|
| old in-memory (`decode_file` + `to_16khz_mono`) | **1 105 MiB** |
| new streaming + `mmap` | **578 MiB** |
| difference | **526.8 MiB** (acceptance: ≥ 200 MiB) |

The old peak is the arithmetic made visible: 659 MiB decoded interleaved +
330 MiB mono copy + 115 MiB resampled output. The new peak is whisper's own
base plus the 115 MiB of file-backed PCM pages — the same text, with one copy
instead of three.

The saving scales with the source shape. On a 44.1 kHz **mono** trial the same
box measured 722 MiB (old) vs 539 MiB (new), 183 MiB: the in-memory path's two
copies are 303 MiB each, and whisper's ~460 MiB base dominates. At 16 kHz mono
(our own recordings, which also take the WAV fast path) the ceiling is the one
115 MiB copy. The 48 kHz stereo import is the shape this slice targets and the
one the design's `2 × 3.84 MB/min/channel` arithmetic describes.

### 6.4 Temp hygiene

`cargo test -p suramak-asr --release --test m26b_streaming -- --ignored
--exact temp_hygiene_on_success_cancel_and_panic` — 1 passed. It covers:

* success through the streaming path (MP3, no fast path): zero
  `suramak_{pid}_*.pcm32` remain;
* cancellation mid-decode (token trips after the writer opened the file): the
  partial file is removed;
* a simulated panic while the mapped slice is alive: unwinding runs `Drop`,
  zero files remain;
* a planted `suramak_{pid}_orphan.pcm32` is removed by
  `sweep_orphaned_temp_pcm`; non-`suramak_*` and non-`.pcm32` files survive
  (`crates/suramak-core/tests/m26b_sweep.rs`, 1 passed).

### 6.5 Build gates

* `cargo fmt --all -- --check` — clean.
* `cargo clippy --workspace --all-targets -- -D warnings` — clean.
* Android, NDK r27d (`ANDROID_NDK`/`_HOME`/`_ROOT`, `CMAKE_GENERATOR=Ninja`,
  the repo's `ndk-toolchain.cmake`), target `arm64-v8a`:
  `cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-android --all-targets
  -- -D warnings` and the same for `-p suramak-android-spike` — both clean.
* Full workspace suite: **1522 passed, 0 failed, 17 ignored** (47 test
  binaries; baseline 1509 + the 13 new non-ignored tests in this slice — 7
  `pcm.rs` unit tests, 5 `m26b_pcm.rs`, 1 `m26b_sweep.rs`). The four whisper
  e2e tests are `#[ignore]`d, as the existing e2e suite is.

## 7. Limits and deliberate deviations

* **Windows** keeps the same API but reads one in-process `Vec<f32>` through
  `read_exact_at`; it has no memory win. `WindowPcmReader` is compiled and
  byte-compared to the `mmap` paths on Linux, and the cfg tests assert
  `backend() == "mmap"` on unix and `"read_exact_at"` on the fallback.
* **The WAV fast path allocates one `Vec<f32>`** (whisper's f32-only API). It
  is still one allocation instead of two and skips decode+resample; a file
  that is not exactly 16 kHz mono PCM16 never takes it.
* **The sweep assumes one instance per temp directory.** Desktop runs it after
  acquiring the single-instance lock; CLI/Android are single-process at that
  point. A file another process is still writing is only a risk for two
  simultaneous CLI batches sharing a temp dir, which this app does not start.
* **Queue semantics are unchanged** — same admission accounting, decode-ahead
  bound, cancellation and event order. Only the buffer type on the wire between
  the prefetch pool and the worker changed.
