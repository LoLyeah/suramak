//! suramak-asr — whisper-rs pipeline, engine selection, worker thread and
//! (feature-gated) microphone capture.
//!
//! The crate is pure glue around `suramak-core`:
//! `decode` (core) → `resample` to 16 kHz mono (core) → whisper (this crate) → core `Transcript`.
//!
//! Engine tiers (docs/ENGINES.md): CPU, Metal and — on macOS builds — Core ML with the
//! official encoder package on the ANE. `engine` resolves the tier from
//! `(setting, fast engine, language, availability)`; `whisper` reports which engine
//! actually ran. M10A adds the opt-in **Parakeet** fast engine (`parakeet`, feature
//! `parakeet`, default OFF) — see `docs/WORKORDER-M10A.md`; M10B adds the opt-in
//! **Apple SpeechTranscriber** macOS fast path (feature `speech-transcriber`,
//! default OFF, macOS 26+ only) — see `docs/WORKORDER-M10B.md`.
//!
//! M7: `live` is the windowed live-transcription machinery (pure scheduler +
//! bounded queue + provisional merging on every platform; the decode thread
//! rides the whisper machinery) — the final pass stays exactly the worker's.

pub mod cancel;
pub mod capture;
pub mod engine;
pub mod live;
pub mod mix;
pub mod parakeet;
pub mod pipeline;
pub mod speech_transcriber;
pub mod whisper;
pub mod worker;

pub use cancel::CancelToken;
pub use engine::{resolve_engine, resolve_for, EngineAvailability, EngineDecision, EngineId};
pub use live::{LiveConfig, LiveDecodeSettings, LiveDecoder, LiveResult, LiveSession, LiveWindow};
pub use parakeet::{ParakeetCache, ParakeetRecognizer, ParakeetResult};
pub use pipeline::{model_id_from_path, parse_lang, transcribe_file, PipelineOutcome};
pub use speech_transcriber::{
    SpeechTranscriberEngine, SttAssetState, SttAuthorization, SttResult, SttSpan,
};
pub use whisper::{coreml_compiled, metal_compiled, system_info, AsrError, ContextCache};
pub use worker::{Job, Worker, WorkerEvent};
