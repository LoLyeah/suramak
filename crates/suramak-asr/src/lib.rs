//! suramak-asr — whisper-rs pipeline, worker thread and (feature-gated) microphone capture.
//!
//! The crate is pure glue around `suramak-core`:
//! `decode` (core) → `resample` to 16 kHz mono (core) → whisper (this crate) → core `Transcript`.

pub mod capture;
pub mod pipeline;
pub mod whisper;
pub mod worker;

pub use pipeline::{model_id_from_path, parse_lang, transcribe_file};
pub use whisper::{AsrError, ContextCache};
pub use worker::{Job, Worker, WorkerEvent};
