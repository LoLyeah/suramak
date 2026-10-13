//! suramak-asr — whisper-rs pipeline, engine selection, worker thread and
//! (feature-gated) microphone capture.
//!
//! The crate is pure glue around `suramak-core`:
//! `decode` (core) → `resample` to 16 kHz mono (core) → whisper (this crate) → core `Transcript`.
//!
//! Engine tiers (docs/ENGINES.md): CPU, Metal and — on macOS builds — Core ML with the
//! official encoder package on the ANE. `engine` resolves the tier from
//! `(setting, language, availability)`; `whisper` reports which engine actually ran.

pub mod capture;
pub mod engine;
pub mod mix;
pub mod pipeline;
pub mod whisper;
pub mod worker;

pub use engine::{resolve_engine, resolve_for, EngineAvailability, EngineDecision, EngineId};
pub use pipeline::{model_id_from_path, parse_lang, transcribe_file, PipelineOutcome};
pub use whisper::{coreml_compiled, metal_compiled, system_info, AsrError, ContextCache};
pub use worker::{Job, Worker, WorkerEvent};
