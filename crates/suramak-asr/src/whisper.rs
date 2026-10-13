//! whisper.cpp integration: context cache, parameter construction, segment extraction.
//!
//! API facts verified against the whisper-rs 0.16.0 sources (see also docs/PLAN-NOTES.md §3):
//! - `WhisperContext::new_with_params(path, WhisperContextParameters::default())`
//! - `ctx.create_state()` / `state.full(params, &[f32])`
//! - `WhisperSegment::start_timestamp()/end_timestamp()` are **centiseconds**
//!   (10 ms units) → multiply by 10 for milliseconds.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};

use suramak_core::settings::LangMode;
use suramak_core::transcript::Segment;
use suramak_core::SuramakError;
use whisper_rs::whisper_rs_sys::{whisper_context, whisper_state};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Errors raised by the ASR layer.
///
/// Core pipeline failures stay [`SuramakError`] and convert via `From`; whisper-side
/// failures use the variants below.
#[derive(Debug, thiserror::Error)]
pub enum AsrError {
    #[error(transparent)]
    Core(#[from] SuramakError),

    #[error("failed to load whisper model {path}: {message}")]
    ModelLoad { path: PathBuf, message: String },

    #[error("failed to create whisper state: {0}")]
    State(String),

    #[error("whisper inference failed: {0}")]
    Inference(String),

    #[error("whisper segment text was not valid UTF-8: {0}")]
    SegmentText(String),

    #[error("model file not found: {0}")]
    ModelFileMissing(PathBuf),

    #[error("audio capture is not available in this build (compile with feature `capture`)")]
    CaptureUnsupported,

    #[error("audio capture error: {0}")]
    Capture(String),

    #[error("suramak worker is no longer running")]
    WorkerStopped,
}

/// Decode thread count suggestion: physical parallelism, clamped to 2..=8.
pub fn default_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(2)
        .clamp(2, 8)
}

/// Cache of loaded whisper contexts, keyed by model file path.
///
/// One context per model; loading is expensive so callers (the worker thread) keep
/// the cache alive across jobs. Only the owning thread should touch the contexts.
#[derive(Default)]
pub struct ContextCache {
    contexts: HashMap<PathBuf, WhisperContext>,
}

impl ContextCache {
    /// Creates an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the cached context for `model_path`, loading it on first use.
    pub fn get_or_load(&mut self, model_path: &Path) -> Result<&mut WhisperContext, AsrError> {
        if !model_path.exists() {
            return Err(AsrError::ModelFileMissing(model_path.to_path_buf()));
        }
        if !self.contexts.contains_key(model_path) {
            let ctx =
                WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
                    .map_err(|e| AsrError::ModelLoad {
                        path: model_path.to_path_buf(),
                        message: e.to_string(),
                    })?;
            self.contexts.insert(model_path.to_path_buf(), ctx);
        }
        Ok(self
            .contexts
            .get_mut(model_path)
            .expect("context was just inserted"))
    }

    /// Number of loaded contexts.
    pub fn len(&self) -> usize {
        self.contexts.len()
    }

    /// Whether no context is loaded.
    pub fn is_empty(&self) -> bool {
        self.contexts.is_empty()
    }
}

/// User-data payload for the whisper.cpp progress callback.
struct ProgressContext<'a> {
    on_progress: &'a mut dyn FnMut(u8),
}

/// C trampoline for `FullParams::set_progress_callback`.
///
/// # Safety
///
/// `user_data` must point to a live [`ProgressContext`] for the whole duration of the
/// `full()` call. whisper.cpp invokes the callback synchronously on the decoding thread,
/// so a stack local of [`transcribe`] satisfies this.
unsafe extern "C" fn progress_trampoline(
    _ctx: *mut whisper_context,
    _state: *mut whisper_state,
    progress: std::ffi::c_int,
    user_data: *mut c_void,
) {
    if user_data.is_null() {
        return;
    }
    // SAFETY: caller guarantees `user_data` points to a live ProgressContext.
    let progress_ctx = &mut *(user_data as *mut ProgressContext<'_>);
    let pct = progress.clamp(0, 100) as u8;
    (progress_ctx.on_progress)(pct);
}

/// Runs whisper on 16 kHz mono f32 `samples` and returns timed segments.
///
/// `on_progress` receives whisper.cpp's progress percentage (0–100) as decoding advances.
/// Params exactly per docs/PLAN-NOTES.md §3: `Greedy { best_of: 1 }`, `set_n_threads`,
/// all `set_print_*` off, `set_token_timestamps(true)`, language from the [`LangMode`],
/// initial prompt only when set.
pub fn transcribe(
    ctx: &WhisperContext,
    samples: &[f32],
    lang: &LangMode,
    initial_prompt: Option<&str>,
    n_threads: i32,
    mut on_progress: impl FnMut(u8),
) -> Result<Vec<Segment>, AsrError> {
    let mut state = ctx
        .create_state()
        .map_err(|e| AsrError::State(e.to_string()))?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(n_threads);
    params.set_language(match lang {
        LangMode::Auto => None,
        LangMode::Fixed(l) => Some(l.as_str()),
    });
    if let Some(prompt) = initial_prompt {
        params.set_initial_prompt(prompt);
    }
    // The worker/CLI owns all presentation; silence whisper.cpp's console output.
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_token_timestamps(true);

    // Progress forwarding via the raw C callback + user-data pointer (PLAN §2).
    // `progress_ctx` lives on this stack frame for the whole `full()` call.
    let mut progress_ctx = ProgressContext {
        on_progress: &mut on_progress,
    };
    // SAFETY: both pointers are valid for the duration of `state.full()` below; the
    // callback only reads the context and forwards the percentage.
    unsafe {
        params.set_progress_callback(Some(progress_trampoline));
        params.set_progress_callback_user_data(
            &mut progress_ctx as *mut ProgressContext<'_> as *mut c_void,
        );
    }

    state
        .full(params, samples)
        .map_err(|e| AsrError::Inference(e.to_string()))?;

    let n_segments = state.full_n_segments();
    if n_segments < 0 {
        return Err(AsrError::Inference(format!(
            "whisper returned a negative segment count ({n_segments})"
        )));
    }

    let mut segments = Vec::with_capacity(n_segments as usize);
    for (i, seg) in state.as_iter().enumerate() {
        let text = match seg.to_str() {
            Ok(s) => s.trim().to_string(),
            // whisper.cpp can emit non-UTF-8 token bytes for some languages; keep the
            // transcript usable with a lossy conversion instead of failing the job.
            Err(_) => seg
                .to_str_lossy()
                .map_err(|e| AsrError::SegmentText(e.to_string()))?
                .trim()
                .to_string(),
        };
        segments.push(Segment {
            idx: i as u32,
            // Centiseconds → milliseconds.
            start_ms: seg.start_timestamp() * 10,
            end_ms: seg.end_timestamp() * 10,
            text,
        });
    }
    Ok(segments)
}
