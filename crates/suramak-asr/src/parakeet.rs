//! Parakeet fast engine (M10A) — NVIDIA **Parakeet TDT 0.6B v3 int8** through
//! **sherpa-rs** (Rust-native ONNX Runtime; the Core ML execution provider on
//! macOS where available).
//!
//! Everything here is behind the `parakeet` cargo feature (**default OFF**).
//! Without the feature the module still exists with the same API — `compiled()`
//! is `false` and `load` refuses — so routing can never select the engine and
//! builds without the feature behave exactly as before (`docs/ENGINES.md`).
//!
//! Granularity (honest mapping): sherpa-onnx's offline transducer returns the
//! **whole clip in one decode** plus token-level timestamps. There are no
//! whisper-style segment boundaries; [`segments_from_tokens`] reconstructs
//! segments by splitting the token stream at pauses ≥ [`SEGMENT_GAP_S`]. When a
//! model returns no timestamps at all, the whole result maps to one segment
//! spanning the audio (documented in `docs/ENGINES.md`).
//!
//! sherpa-onnx itself is built with the execution providers its prebuilt
//! libraries carry: the CPU provider everywhere and Core ML on macOS. On macOS
//! [`default_provider`] returns `coreml`; sherpa-onnx logs and falls back to CPU
//! per session when a node cannot run there, and [`ParakeetRecognizer::load`]
//! retries on `cpu` if a Core ML session cannot be created at all.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use suramak_core::models;
use suramak_core::transcript::Segment;

use crate::whisper::AsrError;

/// Whether this build carries the `parakeet` feature.
pub const fn compiled() -> bool {
    cfg!(feature = "parakeet")
}

/// sherpa-onnx execution provider for the platform.
///
/// `coreml` on macOS (the prebuilt sherpa-onnx libraries are built with the
/// CoreML execution provider; sherpa-onnx logs "CoreML is for Apple only.
/// Fallback to cpu!" elsewhere and per-node falls back to CPU), `cpu` everywhere
/// else. Pure so both branches are pinned by tests on any platform.
pub const fn default_provider(macos: bool) -> &'static str {
    if macos {
        "coreml"
    } else {
        "cpu"
    }
}

/// The provider for the platform this binary runs on.
pub const fn provider() -> &'static str {
    default_provider(cfg!(target_os = "macos"))
}

/// Token pause (seconds) that starts a new reconstructed segment.
pub const SEGMENT_GAP_S: f32 = 0.5;

/// The model id recorded in transcripts produced by this engine.
pub const MODEL_ID: &str = models::PARAKEET.id;

/// One Parakeet decode: the full text plus token-level timestamps (seconds).
///
/// `tokens` and `timestamps_s` are parallel when the model reported timestamps;
/// both are empty when it did not (or when the clip held no speech).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParakeetResult {
    pub text: String,
    pub tokens: Vec<String>,
    pub timestamps_s: Vec<f32>,
}

impl ParakeetResult {
    /// Whether the model reported token timestamps for this result.
    pub fn has_timestamps(&self) -> bool {
        !self.tokens.is_empty() && self.tokens.len() == self.timestamps_s.len()
    }
}

/// Reconstructs [`Segment`]s from a [`ParakeetResult`] (token timestamps in seconds).
///
/// - No tokens and no text → no segments (the pipeline treats that as "no speech").
/// - A pause of at least [`SEGMENT_GAP_S`] between consecutive tokens starts a
///   new segment; each segment ends where the next one starts (the last one ends
///   at the end of the audio — `audio_ms`).
/// - Timestamps missing or not parallel to the tokens → **one** segment
///   `[0, audio_ms]` carrying the whole text: the honest mapping when the model
///   gives no token times at all.
pub fn segments_from_result(result: &ParakeetResult, audio_ms: i64) -> Vec<Segment> {
    let audio_ms = audio_ms.max(0);
    if result.tokens.is_empty() {
        let text = result.text.trim().to_string();
        if text.is_empty() {
            return Vec::new();
        }
        return vec![Segment {
            idx: 0,
            start_ms: 0,
            end_ms: audio_ms,
            text,
        }];
    }
    let tokens = &result.tokens;
    let timestamps_s = &result.timestamps_s;
    if tokens.len() != timestamps_s.len() {
        let text = tokens.concat().trim().to_string();
        if text.is_empty() {
            return Vec::new();
        }
        return vec![Segment {
            idx: 0,
            start_ms: 0,
            end_ms: audio_ms,
            text,
        }];
    }

    // Group token indices at pauses ≥ SEGMENT_GAP_S.
    let mut groups: Vec<(usize, usize)> = Vec::new(); // inclusive index ranges
    let mut start = 0usize;
    for i in 1..tokens.len() {
        if timestamps_s[i] - timestamps_s[i - 1] >= SEGMENT_GAP_S {
            groups.push((start, i - 1));
            start = i;
        }
    }
    groups.push((start, tokens.len() - 1));

    // A leading punctuation-only token (".", ",", "!") belongs to the sentence it
    // terminates — the pause after "dog" is what starts the split, and Parakeet
    // emits the period as its own token *after* that pause. Fold such tokens into
    // the previous group so a segment never starts with stray punctuation.
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (mut first, last) in groups {
        if let Some((_, prev_last)) = merged.last_mut() {
            while first <= last && is_punctuation_only(&tokens[first]) {
                *prev_last = first;
                first += 1;
            }
        }
        if first <= last {
            merged.push((first, last));
        }
    }

    let mut segments = Vec::with_capacity(merged.len());
    for (n, (first, last)) in merged.iter().enumerate() {
        let text = tokens[*first..=*last].concat().trim().to_string();
        if text.is_empty() {
            continue;
        }
        let start_ms = ((timestamps_s[*first].max(0.0) * 1000.0).round() as i64).min(audio_ms);
        // The segment runs to the start of the next one (pauses belong to the
        // segment that precedes them); the last one runs to the audio end. Both
        // ends are clamped into the clip.
        let end_ms = match merged.get(n + 1) {
            Some((next_first, _)) => {
                ((timestamps_s[*next_first].max(0.0) * 1000.0).round() as i64).min(audio_ms)
            }
            None => audio_ms,
        }
        .max(start_ms);
        segments.push(Segment {
            idx: segments.len() as u32,
            start_ms,
            end_ms,
            text,
        });
    }
    segments
}

/// Whether a token holds no alphanumeric character at all (pure punctuation,
/// possibly with surrounding spaces).
fn is_punctuation_only(token: &str) -> bool {
    let trimmed = token.trim();
    !trimmed.is_empty() && !trimmed.chars().any(char::is_alphanumeric)
}

/// A loaded Parakeet recognizer (sherpa-onnx offline transducer).
///
/// Holds the recognizer's C handle; only the owning thread should use it (the
/// worker keeps it in [`ParakeetCache`]).
pub struct ParakeetRecognizer {
    #[cfg(feature = "parakeet")]
    inner: ffi::Inner,
}

impl std::fmt::Debug for ParakeetRecognizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The handle is a raw C pointer; nothing useful (or safe) to print.
        f.debug_struct("ParakeetRecognizer")
            .field("compiled", &compiled())
            .finish_non_exhaustive()
    }
}

impl ParakeetRecognizer {
    /// Loads the model from `model_dir` (the installed Parakeet directory).
    ///
    /// `provider` is a sherpa-onnx execution-provider name ([`provider`]).
    /// macOS: a failing `coreml` session is retried on `cpu` (logged, never a
    /// hard failure — the engine stays Parakeet either way).
    #[cfg(feature = "parakeet")]
    pub fn load(model_dir: &Path, num_threads: i32, provider: &str) -> Result<Self, AsrError> {
        if !dir_ready(model_dir) {
            return Err(AsrError::Parakeet(format!(
                "the Parakeet model is not installed at {} \
                 (run `suramak-cli models download parakeet`)",
                model_dir.display()
            )));
        }
        match ffi::Inner::create(model_dir, num_threads, provider) {
            Ok(inner) => Ok(Self { inner }),
            Err(err) if provider != "cpu" => {
                tracing::warn!("parakeet: {provider} session failed ({err}); retrying on cpu");
                ffi::Inner::create(model_dir, num_threads, "cpu").map(|inner| Self { inner })
            }
            Err(err) => Err(err),
        }
    }

    /// [`ParakeetRecognizer::load`] without the `parakeet` feature: the honest
    /// refusal (routing never selects the engine in such a build).
    #[cfg(not(feature = "parakeet"))]
    pub fn load(model_dir: &Path, num_threads: i32, provider: &str) -> Result<Self, AsrError> {
        let _ = (model_dir, num_threads, provider);
        Err(AsrError::ParakeetUnsupported)
    }

    /// Runs the model on 16 kHz mono f32 `samples`.
    #[cfg(feature = "parakeet")]
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<ParakeetResult, AsrError> {
        ffi::run(&mut self.inner, samples, sample_rate)
    }

    /// [`ParakeetRecognizer::transcribe`] without the `parakeet` feature.
    #[cfg(not(feature = "parakeet"))]
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<ParakeetResult, AsrError> {
        let _ = (samples, sample_rate);
        Err(AsrError::ParakeetUnsupported)
    }
}

/// Whether `dir` contains every file an installed Parakeet directory needs.
pub fn dir_ready(dir: &Path) -> bool {
    suramak_core::download::dir_ready(dir, models::PARAKEET_REQUIRED_FILES)
}

/// Cache of loaded Parakeet recognizers, keyed by `(model dir, provider)`.
///
/// Loading the model costs seconds (652 MB of ONNX), so the worker keeps it
/// alive across jobs — the same shape as the whisper [`crate::whisper::ContextCache`].
#[derive(Default)]
pub struct ParakeetCache {
    recognizers: HashMap<(PathBuf, String), ParakeetRecognizer>,
}

impl ParakeetCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the cached recognizer for `model_dir`, loading it on first use.
    pub fn get_or_load(
        &mut self,
        model_dir: &Path,
        num_threads: i32,
        provider: &str,
    ) -> Result<&mut ParakeetRecognizer, AsrError> {
        let key = (model_dir.to_path_buf(), provider.to_string());
        if !self.recognizers.contains_key(&key) {
            let recognizer = ParakeetRecognizer::load(model_dir, num_threads, provider)?;
            self.recognizers.insert(key.clone(), recognizer);
        }
        Ok(self
            .recognizers
            .get_mut(&key)
            .expect("recognizer was just inserted"))
    }

    pub fn len(&self) -> usize {
        self.recognizers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recognizers.is_empty()
    }
}

// ---------------------------------------------------------------------------
// sherpa-onnx FFI (feature `parakeet`)
// ---------------------------------------------------------------------------

#[cfg(feature = "parakeet")]
mod ffi {
    //! Thin raw binding over sherpa-onnx's C API, mirroring `sherpa-rs`'s own
    //! `transducer` module (0.6.8) but reading the **token timestamps** from the
    //! result — the safe wrapper returns only the text.
    //!
    //! Pinned against the vendored `sherpa-rs-sys` 0.6.8 sources (sherpa-onnx
    //! 1.12.9): `SherpaOnnxOfflineRecognizerResult { text, timestamps, count,
    //! tokens, tokens_arr, json }` — `count` is the token count and `timestamps`
    //! is non-null only when the model reported one timestamp per token.

    use std::ffi::{CStr, CString};
    use std::path::Path;

    use sherpa_rs::sherpa_rs_sys as sys;

    use super::ParakeetResult;
    use crate::whisper::AsrError;

    fn cstr(s: &str) -> Result<CString, AsrError> {
        CString::new(s).map_err(|err| AsrError::Parakeet(format!("string with NUL byte: {err}")))
    }

    /// The loaded recognizer handle.
    pub struct Inner {
        recognizer: *const sys::SherpaOnnxOfflineRecognizer,
    }

    // The worker thread owns the recognizer; sherpa-onnx recognizers are used
    // from a single thread at a time (`SherpaOnnxDecodeOfflineStream`).
    unsafe impl Send for Inner {}

    impl Drop for Inner {
        fn drop(&mut self) {
            // SAFETY: the pointer came from `SherpaOnnxCreateOfflineRecognizer`
            // and is destroyed exactly once, here.
            unsafe { sys::SherpaOnnxDestroyOfflineRecognizer(self.recognizer) };
        }
    }

    impl Inner {
        /// Creates the offline transducer recognizer for the Parakeet model.
        pub fn create(
            model_dir: &Path,
            num_threads: i32,
            provider: &str,
        ) -> Result<Self, AsrError> {
            let encoder = cstr(&model_dir.join("encoder.int8.onnx").to_string_lossy())?;
            let decoder = cstr(&model_dir.join("decoder.int8.onnx").to_string_lossy())?;
            let joiner = cstr(&model_dir.join("joiner.int8.onnx").to_string_lossy())?;
            let tokens = cstr(&model_dir.join("tokens.txt").to_string_lossy())?;
            let provider = cstr(provider)?;
            let model_type = cstr("nemo_transducer")?;
            let decoding_method = cstr("greedy_search")?;
            let empty = cstr("")?;

            // SAFETY: every pointer below is borrowed from a `CString` that lives
            // until the end of this block; `SherpaOnnxCreateOfflineRecognizer`
            // copies what it needs (the C API documents this) and returns a
            // handle or null.
            let recognizer = unsafe {
                let model_config = sys::SherpaOnnxOfflineModelConfig {
                    transducer: sys::SherpaOnnxOfflineTransducerModelConfig {
                        encoder: encoder.as_ptr(),
                        decoder: decoder.as_ptr(),
                        joiner: joiner.as_ptr(),
                    },
                    tokens: tokens.as_ptr(),
                    num_threads,
                    debug: 0,
                    provider: provider.as_ptr(),
                    model_type: model_type.as_ptr(),
                    modeling_unit: empty.as_ptr(),
                    bpe_vocab: empty.as_ptr(),
                    // The remaining model slots stay NULL (only the transducer
                    // branch is configured).
                    telespeech_ctc: std::mem::zeroed(),
                    paraformer: std::mem::zeroed(),
                    tdnn: std::mem::zeroed(),
                    nemo_ctc: std::mem::zeroed(),
                    whisper: std::mem::zeroed(),
                    sense_voice: std::mem::zeroed(),
                    moonshine: std::mem::zeroed(),
                    fire_red_asr: std::mem::zeroed(),
                    dolphin: std::mem::zeroed(),
                    zipformer_ctc: std::mem::zeroed(),
                    canary: std::mem::zeroed(),
                };
                let config = sys::SherpaOnnxOfflineRecognizerConfig {
                    model_config,
                    feat_config: sys::SherpaOnnxFeatureConfig {
                        sample_rate: 16_000,
                        feature_dim: 80,
                    },
                    lm_config: std::mem::zeroed(),
                    decoding_method: decoding_method.as_ptr(),
                    max_active_paths: 4,
                    hotwords_file: empty.as_ptr(),
                    hotwords_score: 0.0,
                    rule_fsts: empty.as_ptr(),
                    rule_fars: empty.as_ptr(),
                    blank_penalty: 0.0,
                    hr: std::mem::zeroed(),
                };
                sys::SherpaOnnxCreateOfflineRecognizer(&config)
            };
            if recognizer.is_null() {
                return Err(AsrError::Parakeet(format!(
                    "SherpaOnnxCreateOfflineRecognizer failed for {} (provider {})",
                    model_dir.display(),
                    provider.to_string_lossy()
                )));
            }
            Ok(Self { recognizer })
        }
    }

    /// Decodes `samples` and returns text + token timestamps.
    pub fn run(
        inner: &mut Inner,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<ParakeetResult, AsrError> {
        // Validate the length before any FFI call (SUR-MED-03), so an oversized
        // buffer is an error with nothing to clean up — the FFI's sample count is
        // an `int32`, and a longer buffer would wrap to a negative length and let
        // the C++ side walk past the pointer.
        let sample_count = super::sample_count_i32(samples.len())?;
        // SAFETY: the recognizer handle is live (owned by `inner`); the stream is
        // created and destroyed within this call; every pointer read below points
        // into the result that is destroyed after it was copied out.
        unsafe {
            let stream = sys::SherpaOnnxCreateOfflineStream(inner.recognizer);
            if stream.is_null() {
                return Err(AsrError::Parakeet(
                    "SherpaOnnxCreateOfflineStream failed".to_string(),
                ));
            }
            sys::SherpaOnnxAcceptWaveformOffline(
                stream,
                sample_rate as i32,
                samples.as_ptr(),
                sample_count,
            );
            sys::SherpaOnnxDecodeOfflineStream(inner.recognizer, stream);
            let result_ptr = sys::SherpaOnnxGetOfflineStreamResult(stream);
            if result_ptr.is_null() {
                sys::SherpaOnnxDestroyOfflineStream(stream);
                return Err(AsrError::Parakeet(
                    "SherpaOnnxGetOfflineStreamResult returned null".to_string(),
                ));
            }
            let raw = result_ptr.read();
            let text = if raw.text.is_null() {
                String::new()
            } else {
                CStr::from_ptr(raw.text).to_string_lossy().into_owned()
            };
            let count = raw.count.max(0) as usize;
            let mut tokens = Vec::with_capacity(count);
            if !raw.tokens_arr.is_null() {
                for i in 0..count {
                    let ptr = *raw.tokens_arr.add(i);
                    if ptr.is_null() {
                        tokens.push(String::new());
                    } else {
                        tokens.push(CStr::from_ptr(ptr).to_string_lossy().into_owned());
                    }
                }
            }
            let timestamps_s = if raw.timestamps.is_null() || count == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(raw.timestamps, count).to_vec()
            };
            sys::SherpaOnnxDestroyOfflineRecognizerResult(result_ptr);
            sys::SherpaOnnxDestroyOfflineStream(stream);
            Ok(ParakeetResult {
                text,
                tokens,
                timestamps_s,
            })
        }
    }
}

/// The `int32` sample count the sherpa-onnx FFI takes.
///
/// Split out so the boundary is unit-tested: `samples.len() as i32` used to wrap
/// silently for a buffer longer than `i32::MAX` samples, handing the C++ side a
/// negative length (SUR-MED-03).
// Only the `parakeet` FFI path calls this; the boundary test exercises it in
// every build, hence the scoped allow rather than a cfg on the function.
#[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
fn sample_count_i32(len: usize) -> Result<i32, AsrError> {
    i32::try_from(len).map_err(|_| {
        AsrError::Parakeet(format!(
            "audio buffer of {len} samples exceeds the i32 sample count the recognizer accepts"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(list: &[&str]) -> Vec<String> {
        list.iter().map(|t| t.to_string()).collect()
    }

    fn result(tokens: &[&str], timestamps_s: Vec<f32>, audio_ms: i64) -> Vec<Segment> {
        let result = ParakeetResult {
            text: tokens.concat(),
            tokens: tokens.iter().map(|t| t.to_string()).collect(),
            timestamps_s,
        };
        segments_from_result(&result, audio_ms)
    }

    #[test]
    fn the_ffi_sample_count_is_bounds_checked_not_wrapped() {
        // SUR-MED-03: the boundary is exact at i32::MAX and everything above it
        // is an error — a wrapping cast would go negative (-1 for 2^32-1) and the
        // C++ decoder would read out of bounds.
        assert_eq!(sample_count_i32(0).expect("empty buffer"), 0);
        assert_eq!(sample_count_i32(16_000).expect("one second"), 16_000);
        assert_eq!(
            sample_count_i32(i32::MAX as usize).expect("at the limit"),
            i32::MAX
        );

        let err = sample_count_i32(i32::MAX as usize + 1)
            .expect_err("one sample past i32::MAX must be refused");
        assert!(
            matches!(err, AsrError::Parakeet(_)),
            "unexpected error: {err}"
        );

        // The 32-bit wrap value specifically (2^32 - 1 → -1 as i32).
        assert!(sample_count_i32(u32::MAX as usize).is_err());
    }

    #[test]
    fn provider_is_coreml_on_macos_and_cpu_elsewhere() {
        assert_eq!(default_provider(true), "coreml");
        assert_eq!(default_provider(false), "cpu");
        // The running binary reports its platform's choice.
        assert_eq!(provider(), default_provider(cfg!(target_os = "macos")));
        assert_eq!(
            provider(),
            if cfg!(target_os = "macos") {
                "coreml"
            } else {
                "cpu"
            }
        );
    }

    #[test]
    fn segments_split_at_pauses_and_span_the_audio() {
        // Two sentences with a 0.5 s pause between them (the real shape of a
        // Parakeet v3 result on the en.wav fixture).
        let tokens = [
            " The", " quick", " brown", " fox", " jumps", " over", " the", " lazy", " dog", ".",
            " Sur", "amak", " keeps", " every", " rec", "ording", ".",
        ];
        let times = vec![
            0.0, 0.24, 0.40, 0.64, 0.88, 1.04, 1.28, 1.44, 1.52, 1.68, // "…dog."
            2.20, 2.40, 2.64, 2.88, 3.20, 3.52, 3.76, // "Suramak keeps every recording."
        ];
        let segments = result(&tokens, times, 9_000);
        assert_eq!(segments.len(), 2, "a 0.52 s pause must split: {segments:?}");
        assert_eq!(segments[0].idx, 0);
        assert_eq!(
            segments[0].text,
            "The quick brown fox jumps over the lazy dog."
        );
        assert_eq!(segments[0].start_ms, 0);
        // The first segment runs to the start of the second one.
        assert_eq!(segments[0].end_ms, 2_200);
        assert_eq!(segments[1].idx, 1);
        assert_eq!(segments[1].text, "Suramak keeps every recording.");
        assert_eq!(segments[1].start_ms, 2_200);
        assert_eq!(
            segments[1].end_ms, 9_000,
            "the last segment runs to the audio end"
        );
    }

    #[test]
    fn leading_punctuation_folds_into_the_sentence_it_ends() {
        // The real Parakeet v3 shape on the fixture: the pause lands *before* the
        // period token, so the split point sits between "dog" and "." — the fold
        // keeps "." with the sentence it terminates.
        let tokens = [
            " The", " quick", " brown", " fox", " jumps", " over", " the", " lazy", " dog", ".",
            " Sur", "imat", " keeps", " every", " rec", "ording", ".",
        ];
        let times = vec![
            0.0, 0.24, 0.40, 0.64, 0.88, 1.04, 1.28, 1.44, 1.52, 1.68, // "…dog"
            2.24, 2.40, 2.64, 2.88, 3.20, 3.52, 3.76, // "." then "Surimat …"
        ];
        let segments = result(&tokens, times, 9_000);
        assert_eq!(segments.len(), 2, "{segments:?}");
        assert_eq!(
            segments[0].text,
            "The quick brown fox jumps over the lazy dog."
        );
        assert_eq!(segments[0].end_ms, 2_240);
        assert_eq!(segments[1].text, "Surimat keeps every recording.");
        assert_eq!(segments[1].start_ms, 2_240);
        // A punctuation token that started its own group is absorbed into the
        // previous one (the sentence keeps its full stop).
        let segments = result(&[" word", ".", " next"], vec![0.0, 0.6, 0.9], 1_000);
        assert_eq!(segments.len(), 2, "{segments:?}");
        assert_eq!(segments[0].text, "word.");
        assert_eq!(segments[0].end_ms, 900);
        assert_eq!(segments[1].text, "next");
        assert_eq!(segments[1].start_ms, 900);
    }

    #[test]
    fn segments_do_not_split_below_the_pause_threshold() {
        let segments = result(&[" a", " b", " c"], vec![0.0, 0.4, 0.8], 1_000);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "a b c");
        assert_eq!(segments[0].end_ms, 1_000);
    }

    #[test]
    fn missing_timestamps_map_to_one_whole_audio_segment() {
        // Not parallel → honest single-segment mapping.
        let segments = result(&[" hello", " world"], vec![], 2_500);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start_ms, 0);
        assert_eq!(segments[0].end_ms, 2_500);
        assert_eq!(segments[0].text, "hello world");
    }

    #[test]
    fn empty_results_map_to_no_segments() {
        assert!(result(&[], vec![], 1_000).is_empty());
        // Whitespace-only tokens are dropped, not emitted as an empty segment.
        assert!(result(&[" ", "  "], vec![0.0, 0.1], 1_000).is_empty());
        // Text without tokens still maps to one segment (never dropped).
        let no_tokens = ParakeetResult {
            text: " hello".to_string(),
            tokens: Vec::new(),
            timestamps_s: Vec::new(),
        };
        let segments = segments_from_result(&no_tokens, 1_000);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "hello");
    }

    #[test]
    fn segment_times_are_clamped_to_the_audio() {
        // A model that reports times past the audio end (padding) cannot produce
        // segments outside the clip.
        let segments = result(&[" a", " b"], vec![0.0, 2.0], 1_000);
        assert!(!segments.is_empty());
        for segment in &segments {
            assert!(
                segment.start_ms >= 0 && segment.start_ms <= segment.end_ms,
                "{segment:?}"
            );
            assert!(segment.end_ms <= 1_000, "{segment:?}");
        }
        // Without a pause the single segment still ends at the audio end.
        let segments = result(&[" a", " b"], vec![0.9, 1.3], 1_000);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start_ms, 900);
        assert_eq!(segments[0].end_ms, 1_000);
    }

    #[test]
    fn result_reports_timestamp_availability() {
        let with = ParakeetResult {
            text: "x".to_string(),
            tokens: tokens(&[" x"]),
            timestamps_s: vec![0.0],
        };
        assert!(with.has_timestamps());
        let without = ParakeetResult {
            text: "x".to_string(),
            tokens: tokens(&[" x"]),
            timestamps_s: Vec::new(),
        };
        assert!(!without.has_timestamps());
        assert!(!ParakeetResult::default().has_timestamps());
    }

    #[test]
    fn dir_ready_matches_the_manifest_layout() {
        let dir = std::env::temp_dir().join(format!("suramak-parakeet-asr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!dir_ready(&dir));
        for file in models::PARAKEET.files {
            std::fs::write(dir.join(file.name), b"x").unwrap();
        }
        assert!(dir_ready(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn model_id_is_the_manifest_id() {
        assert_eq!(MODEL_ID, "parakeet");
    }

    /// Without the feature every entry point refuses honestly; with it, loading a
    /// missing model directory still refuses (the layout check runs first).
    #[test]
    fn load_refuses_without_the_model() {
        let dir =
            std::env::temp_dir().join(format!("suramak-parakeet-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let err = ParakeetRecognizer::load(&dir, 2, provider())
            .expect_err("a missing model must not load");
        if compiled() {
            match err {
                AsrError::Parakeet(message) => {
                    assert!(message.contains("not installed"), "unexpected: {message}")
                }
                other => panic!("expected AsrError::Parakeet, got {other:?}"),
            }
        } else {
            assert!(matches!(err, AsrError::ParakeetUnsupported));
        }
        let mut cache = ParakeetCache::new();
        assert!(cache.is_empty());
        assert!(cache.get_or_load(&dir, 2, provider()).is_err());
        assert!(cache.is_empty(), "failed loads must not populate the cache");
    }

    #[cfg(not(feature = "parakeet"))]
    #[test]
    fn without_the_feature_the_engine_is_not_compiled() {
        assert!(!compiled());
        assert!(matches!(
            ParakeetRecognizer::load(Path::new("/nonexistent"), 2, "cpu"),
            Err(AsrError::ParakeetUnsupported)
        ));
    }

    #[cfg(feature = "parakeet")]
    #[test]
    fn with_the_feature_the_engine_is_compiled() {
        assert!(compiled());
    }
}
