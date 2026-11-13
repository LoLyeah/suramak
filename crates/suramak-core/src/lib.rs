// M13a: Android-only platform seams (app-private data dir + log sink). Behind
// `cfg(target_os = "android")` — desktop builds are byte-identical.
#[cfg(target_os = "android")]
pub mod android;
pub mod decode;
pub mod device;
pub mod diagnostics;
pub mod diarization;
pub mod download;
pub mod error;
pub mod export;
pub mod keychain;
// M26A: device-derived duration caps and the model-fit gate.
pub mod limits;
pub mod models;
// M24B: citation-verified notes (Minutes / Action items / EN↔ID brief).
pub mod notes;
pub mod paths;
pub mod polish;
pub mod resample;
pub mod search;
pub mod segment_edit;
pub mod session;
pub mod settings;
pub mod store;
pub mod textrules;
pub mod time;
pub mod transcript;
pub mod translate;
// M24A: the watched inbox (polling scanner, settle gate, sha256 dedupe).
pub mod watch;

pub use error::SuramakError;
// M22A: the journal stores absolute timestamps; the same re-export rule as the
// id below — a caller of the store speaks in core's types, not a second
// dependency edge.
pub use chrono::{DateTime, Utc};
// M16I: the id type the store, effects and the diarization lane all name — one
// re-export of the crate the workspace already pins, so a caller never needs a
// second `uuid` declaration to speak about a transcript id.
pub use uuid::Uuid;
