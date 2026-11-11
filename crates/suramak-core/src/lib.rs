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
pub mod models;
pub mod paths;
pub mod polish;
pub mod resample;
pub mod search;
pub mod segment_edit;
pub mod settings;
pub mod store;
pub mod textrules;
pub mod time;
pub mod transcript;
pub mod translate;

pub use error::SuramakError;
// M16I: the id type the store, effects and the diarization lane all name — one
// re-export of the crate the workspace already pins, so a caller never needs a
// second `uuid` declaration to speak about a transcript id.
pub use uuid::Uuid;
