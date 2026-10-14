//! Cooperative cancellation for in-flight transcription jobs (M9e, P2-2).
//!
//! One token per job: the UI/runtime owns the handle and sets it; the worker
//! and the pipeline observe it at **stage boundaries** (before the model load,
//! between decode → resample → whisper, and — inside whisper — between the
//! decoding chunks via whisper.cpp's own abort callback, the engine's designed
//! abort path). Nothing is preempted mid-call: the current stage finishes its
//! chunk, the pipeline returns [`crate::whisper::AsrError::Cancelled`], partial
//! results are dropped, and the worker moves on to the next job — the whisper
//! context cache (the engine) is never touched by a cancellation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A cooperative cancel flag shared between the UI, the worker and the
/// pipeline.
///
/// `Clone` shares the same flag (every clone sees a `cancel` immediately);
/// `Default` is a fresh, uncancelled token.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh, uncancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Idempotent; observed at the next check point.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_token_is_uncancelled_and_cancel_is_visible_to_every_clone() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        // Cancelling one handle cancels the shared flag (the worker holds a
        // clone; the UI holds the original).
        clone.cancel();
        assert!(token.is_cancelled());
        assert!(clone.is_cancelled());
        // Idempotent.
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn the_default_token_is_uncancelled() {
        let token = CancelToken::default();
        assert!(!token.is_cancelled());
    }
}
