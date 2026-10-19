// M13a: Android-only platform seams (app-private data dir + log sink). Behind
// `cfg(target_os = "android")` — desktop builds are byte-identical.
#[cfg(target_os = "android")]
pub mod android;
pub mod decode;
pub mod download;
pub mod error;
pub mod export;
pub mod keychain;
pub mod models;
pub mod paths;
pub mod polish;
pub mod resample;
pub mod settings;
pub mod store;
pub mod time;
pub mod transcript;

pub use error::SuramakError;
