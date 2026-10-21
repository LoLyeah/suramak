//! whisper.cpp integration: context cache, engine engagement, parameter construction,
//! segment extraction.
//!
//! API facts re-verified against the whisper-rs 0.16.0 / whisper-rs-sys 0.15.0 sources
//! (docs/PLAN-NOTES.md §3 discipline):
//! - `WhisperContext::new_with_params(path, WhisperContextParameters::default())`,
//!   `ctx.create_state()`, `state.full(params, &[f32])`.
//! - `WhisperContextParameters { use_gpu, flash_attn, gpu_device, dtw_parameters }` —
//!   there is **no Rust-side Core ML knob**. whisper-rs-sys' `coreml` feature compiles
//!   whisper.cpp with `WHISPER_COREML=ON` and `WHISPER_COREML_ALLOW_FALLBACK=1`
//!   (build.rs), and whisper.cpp then uses `<model-stem>-encoder.mlmodelc` next to the
//!   model file **if and only if** that directory exists and loads
//!   (`whisper_get_coreml_path_encoder` / `whisper_encode_external` in src/whisper.cpp).
//!   Core ML engagement is therefore decided at **state** init, and the only lever Rust
//!   has is the presence of the encoder directory — see [`with_encoder_hidden`].
//! - whisper.cpp reports what it did on the whisper log channel ("Core ML model loaded"
//!   / "failed to load Core ML model from …"), which [`create_state_probed`] records so
//!   the pipeline can report the engine that *actually* ran.
//! - `WhisperSegment::start_timestamp()/end_timestamp()` are **centiseconds**
//!   (10 ms units) → multiply by 10 for milliseconds.

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::path::{Path, PathBuf};
use std::sync::{Once, OnceLock};

use suramak_core::models;
use suramak_core::settings::LangMode;
use suramak_core::transcript::Segment;
use suramak_core::SuramakError;
use tracing::warn;
use whisper_rs::whisper_rs_sys::{ggml_log_level, whisper_context, whisper_state};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::cancel::CancelToken;
use crate::engine::{EngineAvailability, EngineDecision, EngineId};

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

    /// M9e (P2-2): the job's [`crate::CancelToken`] was set and the pipeline
    /// stopped at a stage boundary. Partial results were discarded; the
    /// whisper context is untouched.
    #[error("transcription cancelled")]
    Cancelled,

    /// M10A: the Parakeet engine failed (model load, sherpa-onnx session or
    /// decode). The whisper path is unaffected.
    #[error("parakeet engine error: {0}")]
    Parakeet(String),

    /// M10A: this build has no `parakeet` feature, so the fast engine cannot run
    /// (routing never selects it — this is the honest refusal).
    #[error(
        "the parakeet engine is not available in this build (compile with feature `parakeet`)"
    )]
    ParakeetUnsupported,

    /// M10B: the Apple SpeechTranscriber engine failed (locale resolution, the
    /// authorization gate, the OS asset install, or the analyzer run itself).
    /// The whisper path is unaffected — the worker degrades to it with this
    /// message as the warning.
    #[error("speech-transcriber engine error: {0}")]
    SpeechTranscriber(String),

    /// M10B: this build has no macOS SpeechTranscriber (feature
    /// `speech-transcriber` off, or not a macOS target — the `speech` crate is a
    /// Swift bridge). Routing never selects it; this is the honest refusal.
    #[error(
        "the speech-transcriber engine is not available in this build \
         (compile with feature `speech-transcriber` on macOS)"
    )]
    SpeechTranscriberUnsupported,
}

/// Decode thread count suggestion: physical parallelism, clamped to 2..=8.
pub fn default_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(2)
        .clamp(2, 8)
}

/// whisper.cpp's own system-info line, e.g.
/// `WHISPER : COREML = 1 | OPENVINO = 0 | Metal : EMBED_LIBRARY = 1 | CPU : …`.
///
/// This is the linked library reporting its own build features; `doctor` prints it.
///
/// # Thread safety
///
/// `whisper_print_system_info()` hands back the buffer of a function-static
/// `std::string` that it **rebuilds on every call** — and `whisper_init_state()`
/// calls it internally too. Two threads (e.g. our own read and another test's
/// context init) therefore interleave writes into that one buffer and a read can
/// come back torn (observed: `… AVX2CPU : SSE3 = 1 …` — two headers spliced).
///
/// The value cannot change during the process lifetime, so read it once, and if
/// the read is torn retry briefly until a complete line appears. `OnceLock`
/// additionally serialises concurrent callers of this function.
pub fn system_info() -> String {
    static INFO: OnceLock<String> = OnceLock::new();
    INFO.get_or_init(|| {
        let mut last = String::new();
        for _ in 0..100 {
            let candidate = read_system_info();
            // A complete line carries exactly one WHISPER section and one CPU
            // section; a torn read splices a second header into the middle
            // (observed: `… AVX2CPU : SSE3 = 1 …`).
            if candidate.contains("WHISPER :") && candidate.matches("CPU :").count() == 1 {
                return candidate;
            }
            last = candidate;
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        last
    })
    .clone()
}

/// One raw read of whisper.cpp's system-info buffer.
fn read_system_info() -> String {
    // SAFETY: `whisper_print_system_info` returns a pointer to a static string owned by
    // whisper.cpp (valid for the process lifetime); it takes no arguments and never
    // requires an initialised context.
    let ptr = unsafe { whisper_rs::whisper_rs_sys::whisper_print_system_info() };
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: non-null, NUL-terminated, static.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// Whether the linked whisper.cpp was built with Core ML support.
pub fn coreml_compiled() -> bool {
    system_info().contains("COREML = 1")
}

/// Whether the linked whisper.cpp was built with the Metal backend.
pub fn metal_compiled() -> bool {
    system_info().contains("Metal :")
}

// ---------------------------------------------------------------------------
// Core ML engagement probe
// ---------------------------------------------------------------------------

/// whisper.cpp's Core ML success line (src/whisper.cpp, `whisper_init_state`).
const COREML_LOADED_MARKER: &str = "Core ML model loaded";
/// whisper.cpp's Core ML failure line (same function).
const COREML_FAILED_MARKER: &str = "failed to load Core ML model";

/// What whisper.cpp reported about Core ML while a state was initialised.
#[derive(Debug, Default, Clone)]
struct CoremlProbe {
    /// whisper.cpp logged "Core ML model loaded".
    loaded: bool,
    /// whisper.cpp logged "failed to load Core ML model from …" and (because
    /// whisper-rs-sys builds with `WHISPER_COREML_ALLOW_FALLBACK=1`) continued on the
    /// ggml encoder.
    failure: Option<String>,
}

thread_local! {
    /// Pointer to the probe of the state init running on this thread (null when idle).
    static COREML_PROBE: Cell<*mut CoremlProbe> = const { Cell::new(std::ptr::null_mut()) };
}

static LOG_HOOK: Once = Once::new();

/// Installs the whisper.cpp log callback that watches Core ML engagement (once).
///
/// whisper-rs' own hooks go in first — they are what silences the C-level chatter (M0b).
/// Our callback replaces the whisper channel afterwards: like the default build it drops
/// the messages, and additionally records whisper.cpp's own Core ML report for the state
/// being initialised on this thread. The `Once` in whisper-rs means a later
/// `install_logging_hooks()` call cannot replace it again.
fn install_log_hooks() {
    LOG_HOOK.call_once(|| {
        whisper_rs::install_logging_hooks();
        // SAFETY: `whisper_log_set` only stores the callback; the trampoline reads the
        // thread-local probe pointer, which is null unless a guard installed a live one.
        unsafe {
            whisper_rs::whisper_rs_sys::whisper_log_set(
                Some(whisper_log_trampoline),
                std::ptr::null_mut(),
            );
        }
    });
}

unsafe extern "C" fn whisper_log_trampoline(
    _level: ggml_log_level,
    text: *const c_char,
    _user_data: *mut c_void,
) {
    if text.is_null() {
        return;
    }
    // SAFETY: whisper.cpp passes a NUL-terminated string.
    let message = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    COREML_PROBE.with(|cell| {
        let probe = cell.get();
        if probe.is_null() {
            return;
        }
        // SAFETY: the guard installs a pointer to a live stack local of this thread and
        // clears it before the local goes away; whisper.cpp logs synchronously.
        let probe = unsafe { &mut *probe };
        if message.contains(COREML_LOADED_MARKER) {
            probe.loaded = true;
        } else if message.contains(COREML_FAILED_MARKER) {
            probe.failure = Some(message.trim().to_string());
        }
    });
}

/// RAII: makes `probe` this thread's Core ML probe for the guarded scope.
struct CoremlProbeGuard;

impl CoremlProbeGuard {
    fn install(probe: &mut CoremlProbe) -> Self {
        COREML_PROBE.with(|cell| cell.set(probe as *mut CoremlProbe));
        Self
    }
}

impl Drop for CoremlProbeGuard {
    fn drop(&mut self) {
        COREML_PROBE.with(|cell| cell.set(std::ptr::null_mut()));
    }
}

/// Creates a whisper state, recording whisper.cpp's Core ML load report.
fn create_state_probed(ctx: &WhisperContext) -> Result<(WhisperState, CoremlProbe), AsrError> {
    install_log_hooks();
    let mut probe = CoremlProbe::default();
    let guard = CoremlProbeGuard::install(&mut probe);
    let state = ctx
        .create_state()
        .map_err(|e| AsrError::State(e.to_string()));
    drop(guard);
    state.map(|state| (state, probe))
}

/// Serialises filesystem work on Core ML encoder directories, in this process.
///
/// [`hidden_encoder_path`]'s sequence counter makes concurrent *hides* disjoint,
/// but the directory they hide is one shared resource. Without this lock two
/// threads on the same model file interleave badly (audit pass 2, SUR2-NEW-03):
/// with the package hidden by thread A, thread B's Core ML state init finds no
/// encoder and silently degrades to Metal/CPU for the rest of the session; and a
/// hide started inside that window stashes nothing, so A's restore lands while B
/// is still inside its critical section.
///
/// Holding it across hide → `f()` → restore, and across the Core ML state init
/// that needs the package present, makes the two operations mutually exclusive.
/// Poisoning is ignored: the guarded work is `rename` plus an RAII restore, so a
/// panicked holder leaves nothing for the next one to distrust.
fn encoder_fs_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs `f` with the model's Core ML encoder directory moved out of the way.
///
/// whisper.cpp has no runtime Core ML switch: it uses the encoder package whenever the
/// directory exists next to the model file. Hiding it for the duration of state init is
/// therefore how `metal`/`cpu` requests (and the Core ML retry) are honoured; the
/// directory is restored afterwards — also when `f` fails.
fn with_encoder_hidden<T>(model_path: &Path, f: impl FnOnce() -> T) -> T {
    // The hide, `f()` and the restore are one critical section (SUR2-NEW-03): a
    // concurrent hide/state-init on the same model must not observe the gap.
    // Declared first, so the guard outlives the `StashedEncoder` restore below
    // (drops run in reverse declaration order).
    let _lock = encoder_fs_lock();
    let dir = models::encoder_dir_for(model_path);
    recover_stranded_encoder(model_path);
    let hidden = hidden_encoder_path(&dir);
    let mut stashed = false;
    if dir.is_dir() {
        match std::fs::rename(&dir, &hidden) {
            Ok(()) => stashed = true,
            Err(err) => warn!(
                "could not hide the Core ML encoder directory {}: {err}",
                dir.display()
            ),
        }
    }
    // RAII: the restore must survive a panic inside `f` (e.g. a C++ assertion in
    // whisper init). Without Drop, unwinding strands the encoder directory and
    // the model silently loses Core ML acceleration until manual repair.
    let _guard = StashedEncoder {
        dir: dir.clone(),
        hidden,
        stashed,
    };
    f()
}

/// Restores an encoder directory a previous process left hidden (hard exit
/// while stashed). Only directories parked by a *different* pid are touched —
/// a same-pid one belongs to a concurrent hide in this process.
fn recover_stranded_encoder(model_path: &Path) {
    let dir = models::encoder_dir_for(model_path);
    if dir.is_dir() {
        return; // the encoder is where it belongs; nothing stranded
    }
    let Some(parent) = dir.parent() else { return };
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let candidate = entry.path();
        let Some(fname) = candidate.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(pid) = fname
            .strip_prefix(&format!(".{name}.hidden-"))
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() {
            continue; // ours — another thread is mid-hide/restore
        }
        match std::fs::rename(&candidate, &dir) {
            Ok(()) => tracing::info!(
                "recovered a Core ML encoder directory left hidden by a previous exit (pid {pid})"
            ),
            Err(err) => warn!(
                "could not recover the stranded Core ML encoder directory {}: {err}",
                candidate.display()
            ),
        }
        return; // one restore is enough; the dir now exists
    }
}

/// RAII restore for [`with_encoder_hidden`].
struct StashedEncoder {
    dir: PathBuf,
    hidden: PathBuf,
    stashed: bool,
}

impl Drop for StashedEncoder {
    fn drop(&mut self) {
        if !self.stashed {
            return;
        }
        if let Err(err) = std::fs::rename(&self.hidden, &self.dir) {
            warn!(
                "could not restore the Core ML encoder directory {} (left at {}): {err}",
                self.dir.display(),
                self.hidden.display()
            );
        }
    }
}

/// Sibling path the encoder directory is parked at while hidden.
///
/// Unique per call, not per process: the live decoder thread and the
/// transcription worker can hide the *same* model's encoder concurrently, and a
/// PID-only suffix makes the second hide collide with the first restore. A
/// process-wide call counter keeps concurrent hides disjoint (the pid keeps it
/// disjoint across processes, which [`recover_stranded_encoder`] relies on).
fn hidden_encoder_path(dir: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("encoder");
    let parent = dir.parent().unwrap_or(Path::new("."));
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{name}.hidden-{}-{seq}", std::process::id()))
}

/// Creates the decode state for `plan`, returning the state and the engine that will
/// actually run for it.
///
/// - Core ML plan: whisper.cpp loads the encoder package. If it reports a load failure,
///   or state init fails outright, the call degrades to Metal/CPU with a warning —
///   never a hard failure (docs/ENGINES.md, auto-fallback).
/// - Metal/CPU plan: the encoder directory is hidden during state init, so the ANE
///   encoder cannot sneak in (whisper.cpp would otherwise prefer it).
fn create_state_for_plan(
    ctx: &WhisperContext,
    model_path: &Path,
    plan: &EngineDecision,
    warnings: &mut Vec<String>,
) -> Result<(WhisperState, EngineId), AsrError> {
    // The fallback engine only depends on the platform/build (macos + metal),
    // not on any language — the SpeechTranscriber probe is irrelevant here.
    let fallback = EngineAvailability::current(model_path, None).best_without_coreml();

    if plan.engine == EngineId::CoreML {
        // The package must exist and stay put for the whole state init: a
        // concurrent `with_encoder_hidden` (live decoder vs. worker) would hide
        // it mid-load and this thread would degrade to Metal/CPU for the rest of
        // the session (SUR2-NEW-03). Taken in its own scope, so the retry below
        // can take it again through `with_encoder_hidden`.
        let attempt = {
            let _lock = encoder_fs_lock();
            create_state_probed(ctx)
        };
        return match attempt {
            // `loaded` is whisper.cpp's explicit confirmation; absence of a *failure*
            // report means the encoder was in use by construction (it existed and
            // whisper.cpp only skips Core ML when `whisper_coreml_init` returned null).
            Ok((state, probe)) if probe.loaded || probe.failure.is_none() => {
                Ok((state, EngineId::CoreML))
            }
            Ok((state, probe)) => {
                warnings.push(format!(
                    "Core ML encoder did not load ({}); using {} instead",
                    probe
                        .failure
                        .as_deref()
                        .unwrap_or("whisper.cpp reported no Core ML"),
                    fallback.as_str()
                ));
                Ok((state, fallback))
            }
            Err(err) => {
                warnings.push(format!(
                    "Core ML state init failed ({err}); retrying without the Core ML encoder ({})",
                    fallback.as_str()
                ));
                let (state, _probe) = with_encoder_hidden(model_path, || create_state_probed(ctx))?;
                Ok((state, fallback))
            }
        };
    }

    let (state, probe) = with_encoder_hidden(model_path, || create_state_probed(ctx))?;
    if probe.loaded {
        // Hiding failed (e.g. permissions): whisper.cpp used Core ML anyway. Report the
        // truth instead of the request.
        warnings.push(
            "the Core ML encoder directory could not be hidden; whisper.cpp used it anyway"
                .to_string(),
        );
        return Ok((state, EngineId::CoreML));
    }
    Ok((state, plan.engine))
}

// ---------------------------------------------------------------------------
// Context cache
// ---------------------------------------------------------------------------

/// Cache of loaded whisper contexts, keyed by `(model file path, use_gpu)`.
///
/// One context per model and GPU setting; loading is expensive so callers (the worker
/// thread) keep the cache alive across jobs. Only the owning thread should touch the
/// contexts.
#[derive(Default)]
pub struct ContextCache {
    contexts: HashMap<(PathBuf, bool), WhisperContext>,
}

impl ContextCache {
    /// Creates an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the cached context for `model_path`, loading it on first use.
    ///
    /// `use_gpu` comes from the resolved engine ([`EngineId::use_gpu`]); the Core ML
    /// encoder itself is not a context parameter (see the module docs).
    pub fn get_or_load(
        &mut self,
        model_path: &Path,
        use_gpu: bool,
    ) -> Result<&mut WhisperContext, AsrError> {
        if !model_path.exists() {
            return Err(AsrError::ModelFileMissing(model_path.to_path_buf()));
        }
        install_log_hooks();
        let key = (model_path.to_path_buf(), use_gpu);
        if !self.contexts.contains_key(&key) {
            let mut params = WhisperContextParameters::default();
            params.use_gpu(use_gpu);
            let ctx = WhisperContext::new_with_params(model_path, params).map_err(|e| {
                AsrError::ModelLoad {
                    path: model_path.to_path_buf(),
                    message: e.to_string(),
                }
            })?;
            self.contexts.insert(key.clone(), ctx);
        }
        Ok(self
            .contexts
            .get_mut(&key)
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

// ---------------------------------------------------------------------------
// Transcription
// ---------------------------------------------------------------------------

/// Result of one transcription: the segments plus the engine that actually ran.
#[derive(Debug, Clone)]
pub struct TranscribeOutcome {
    pub segments: Vec<Segment>,
    /// Engine that actually ran. Core ML only when whisper.cpp loaded the encoder.
    pub engine_used: EngineId,
    /// Non-fatal notes: engine downgrades, Core ML fallback, ignored requests.
    pub warnings: Vec<String>,
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

/// C trampoline for `FullParams::set_abort_callback` — the M9e cancel token's
/// check point inside whisper's decoding loop.
///
/// whisper.cpp calls it between decoding chunks and aborts the run when it
/// returns `true` (the engine's own designed abort path — the same mechanism
/// whisper.cpp's server uses for client disconnects; the context is not torn).
///
/// # Safety
///
/// `user_data` must point to a live [`CancelToken`] for the whole duration of
/// the `full()` call. whisper.cpp invokes the callback synchronously on the
/// decoding thread, so a stack local of [`transcribe`] satisfies this.
unsafe extern "C" fn abort_trampoline(user_data: *mut c_void) -> bool {
    if user_data.is_null() {
        return false;
    }
    // SAFETY: caller guarantees `user_data` points to a live CancelToken.
    let token = &*(user_data as *const CancelToken);
    token.is_cancelled()
}

/// Runs whisper on 16 kHz mono f32 `samples` and returns timed segments plus the engine
/// that ran.
///
/// `on_progress` receives whisper.cpp's progress percentage (0–100) as decoding advances.
/// `cancel` is checked by whisper.cpp itself between decoding chunks (the abort
/// callback, M9e P2-2) and once after the run: a cancelled call returns
/// [`AsrError::Cancelled`] and **no** segments — partial results are discarded
/// and the whisper context (the cached engine) is never touched.
/// Params exactly per docs/PLAN-NOTES.md §3: `Greedy { best_of: 1 }`, `set_n_threads`,
/// all `set_print_*` off, `set_token_timestamps(true)`, language from the [`LangMode`],
/// initial prompt only when set.
// The argument list mirrors the pipeline inputs (audio, language, prompt, threads,
// engine plan) and is called from exactly one place; a request struct would only move
// the same fields elsewhere.
#[allow(clippy::too_many_arguments)]
pub fn transcribe(
    ctx: &WhisperContext,
    model_path: &Path,
    samples: &[f32],
    lang: &LangMode,
    initial_prompt: Option<&str>,
    n_threads: i32,
    plan: &EngineDecision,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(u8),
) -> Result<TranscribeOutcome, AsrError> {
    let mut warnings = Vec::new();
    if let Some(warning) = &plan.warning {
        warnings.push(warning.clone());
    }
    let (mut state, engine_used) = create_state_for_plan(ctx, model_path, plan, &mut warnings)?;

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

    // M9e (P2-2): the cooperative cancel token rides whisper.cpp's own abort
    // callback — checked inside the decoding loop, between chunks. `cancel`
    // lives on the caller's stack for the whole call (the same lifetime rule as
    // `progress_ctx` above); the callback only reads the atomic flag.
    // SAFETY: `cancel` outlives `state.full()` (a borrow of the caller's
    // parameter); the trampoline reads one atomic and never touches the state.
    unsafe {
        params.set_abort_callback(Some(abort_trampoline));
        params.set_abort_callback_user_data(cancel as *const CancelToken as *mut c_void);
    }

    let full = state.full(params, samples);
    // A cancelled run surfaces as a whisper error (the engine aborted the
    // compute graph); the honest answer is `Cancelled`, checked before the
    // error is interpreted. No segments are read either way — the partial
    // results are discarded here.
    if cancel.is_cancelled() {
        return Err(AsrError::Cancelled);
    }
    full.map_err(|e| AsrError::Inference(e.to_string()))?;

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
    Ok(TranscribeOutcome {
        segments,
        engine_used,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::fs;

    /// Unique scratch dir under the system temp dir (no dev-dependencies needed).
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("suramak-asr-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn system_info_is_reported_by_the_linked_library() {
        let info = system_info();
        assert!(info.contains("WHISPER :"), "unexpected system info: {info}");
        // The probe markers must match what whisper.cpp logs (pinned to the vendored
        // whisper.cpp of whisper-rs-sys 0.15.0, src/whisper.cpp `whisper_init_state`).
        assert_eq!(COREML_LOADED_MARKER, "Core ML model loaded");
        assert_eq!(COREML_FAILED_MARKER, "failed to load Core ML model");
    }

    #[test]
    fn encoder_hiding_stashes_and_restores_the_directory() {
        let root = scratch_dir("hidden");
        let model = root.join("ggml-tiny.bin");
        fs::write(&model, b"model").unwrap();
        let encoder = models::encoder_dir_for(&model);
        fs::create_dir_all(encoder.join("weights")).unwrap();
        fs::write(encoder.join("coremldata.bin"), b"x").unwrap();

        let mut hidden_during_call = false;
        let model_visible = with_encoder_hidden(&model, || {
            hidden_during_call = !encoder.exists();
            model.exists()
        });
        assert!(hidden_during_call, "the encoder dir must be hidden inside");
        assert!(model_visible, "the model file must be untouched");
        assert!(
            encoder.join("coremldata.bin").is_file(),
            "the encoder dir must be restored"
        );

        // …also when the closure returns early / fails.
        let result: Result<(), &str> = with_encoder_hidden(&model, || Err("boom"));
        assert!(result.is_err());
        assert!(encoder.join("coremldata.bin").is_file());

        let leftovers: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "leftover hidden dirs: {leftovers:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn encoder_hiding_restores_even_when_the_closure_panics() {
        let root = scratch_dir("hidden-panic");
        let model = root.join("ggml-tiny.bin");
        fs::write(&model, b"model").unwrap();
        let encoder = models::encoder_dir_for(&model);
        fs::create_dir_all(encoder.join("weights")).unwrap();
        fs::write(encoder.join("coremldata.bin"), b"x").unwrap();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_encoder_hidden(&model, || panic!("whisper init blew up"));
        }));
        assert!(result.is_err(), "the panic must propagate");
        assert!(
            encoder.join("coremldata.bin").is_file(),
            "the RAII guard must restore the encoder dir during unwinding"
        );
        let leftovers: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "leftover hidden dirs: {leftovers:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn concurrent_hides_of_the_same_encoder_are_mutually_exclusive() {
        // SUR2-NEW-03: the hide/restore pair is one critical section per model,
        // so two threads can never be "inside" at once — and a thread that
        // starts while another holds the encoder must find it hidden for its own
        // whole closure (never restored underneath it).
        let root = scratch_dir("hidden-concurrent");
        let model = root.join("ggml-tiny.bin");
        fs::write(&model, b"model").unwrap();
        let encoder = models::encoder_dir_for(&model);
        fs::create_dir_all(encoder.join("weights")).unwrap();
        fs::write(encoder.join("coremldata.bin"), b"x").unwrap();

        let inside = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (tx, rx) = std::sync::mpsc::channel::<()>();

        let first = {
            let (inside, peak, encoder, model) =
                (inside.clone(), peak.clone(), encoder.clone(), model.clone());
            std::thread::spawn(move || {
                with_encoder_hidden(&model, || {
                    let now = inside.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    peak.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                    // Tell the second thread the hide has happened, then stay
                    // inside long enough that an unlocked second hide would see
                    // the restore land mid-closure.
                    tx.send(()).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(120));
                    assert!(
                        !encoder.exists(),
                        "the encoder must stay hidden for the whole closure"
                    );
                    inside.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                });
            })
        };

        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("the first hide must start");
        let second = {
            let (inside, peak, encoder, model) =
                (inside.clone(), peak.clone(), encoder.clone(), model.clone());
            std::thread::spawn(move || {
                with_encoder_hidden(&model, || {
                    let now = inside.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    peak.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    assert!(
                        !encoder.exists(),
                        "a hide queued behind another must stash the encoder itself"
                    );
                    inside.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                });
            })
        };

        first.join().unwrap();
        second.join().unwrap();
        assert_eq!(
            peak.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "two threads were inside the encoder critical section at once"
        );
        assert!(
            encoder.join("coremldata.bin").is_file(),
            "the encoder must be back in place after both hides"
        );
        let leftovers: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "leftover hidden dirs: {leftovers:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn recovery_restores_an_encoder_hidden_by_a_dead_process() {
        let root = scratch_dir("hidden-recover");
        let model = root.join("ggml-tiny.bin");
        fs::write(&model, b"model").unwrap();
        let encoder = models::encoder_dir_for(&model);
        let encoder_name = encoder.file_name().unwrap().to_string_lossy().into_owned();

        // A dir parked by a dead pid (1 is never us) must be restored…
        let stranded = root.join(format!(".{encoder_name}.hidden-1-7"));
        fs::create_dir_all(stranded.join("weights")).unwrap();
        fs::write(stranded.join("coremldata.bin"), b"x").unwrap();
        recover_stranded_encoder(&model);
        assert!(
            encoder.join("coremldata.bin").is_file(),
            "a foreign-pid stranded dir must be restored"
        );

        // …but a dir parked by OUR pid belongs to a concurrent hide: leave it.
        let _ = fs::remove_dir_all(&encoder);
        let ours = root.join(format!(".{encoder_name}.hidden-{}-9", std::process::id()));
        fs::create_dir_all(&ours).unwrap();
        recover_stranded_encoder(&model);
        assert!(
            !encoder.is_dir() && ours.is_dir(),
            "a same-pid dir must be left alone (another thread owns it)"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn encoder_hiding_without_an_encoder_is_a_noop() {
        let root = scratch_dir("no-encoder");
        let model = root.join("ggml-tiny.bin");
        fs::write(&model, b"model").unwrap();
        assert_eq!(with_encoder_hidden(&model, || 42), 42);
        assert!(model.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn hidden_encoder_path_is_a_sibling_of_the_encoder_dir() {
        let dir = Path::new("/models/ggml-tiny-encoder.mlmodelc");
        let hidden = hidden_encoder_path(dir);
        assert_eq!(hidden.parent(), dir.parent());
        let name = hidden.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(".ggml-tiny-encoder.mlmodelc.hidden-"));
    }

    #[test]
    fn coreml_probe_records_both_reports() {
        // The probe is driven by whisper.cpp's log callback; exercise it directly.
        let mut probe = CoremlProbe::default();
        let guard = CoremlProbeGuard::install(&mut probe);
        let loaded = CString::new("whisper_init_state: Core ML model loaded\n").unwrap();
        unsafe {
            whisper_log_trampoline(
                whisper_rs::whisper_rs_sys::ggml_log_level_GGML_LOG_LEVEL_INFO,
                loaded.as_ptr(),
                std::ptr::null_mut(),
            )
        };
        assert!(probe.loaded);
        assert!(probe.failure.is_none());
        drop(guard);

        let mut probe = CoremlProbe::default();
        let guard = CoremlProbeGuard::install(&mut probe);
        let failed = CString::new(
            "whisper_init_state: failed to load Core ML model from '/x/ggml-tiny-encoder.mlmodelc'\n",
        )
        .unwrap();
        unsafe {
            whisper_log_trampoline(
                whisper_rs::whisper_rs_sys::ggml_log_level_GGML_LOG_LEVEL_ERROR,
                failed.as_ptr(),
                std::ptr::null_mut(),
            )
        };
        assert!(!probe.loaded);
        assert!(probe
            .failure
            .as_deref()
            .unwrap_or_default()
            .contains("failed to load Core ML model"));
        drop(guard);

        // Outside a guard the trampoline must not touch anything (null probe).
        let ignored = CString::new("Core ML model loaded\n").unwrap();
        unsafe {
            whisper_log_trampoline(
                whisper_rs::whisper_rs_sys::ggml_log_level_GGML_LOG_LEVEL_INFO,
                ignored.as_ptr(),
                std::ptr::null_mut(),
            )
        };
    }
}
