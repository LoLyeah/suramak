//! M22A2 — the desktop host's durable queue creation.
//!
//! The desktop binary's job bridge owns a real [`QueueDriver`] (one inference
//! slot, bounded decode-ahead). This module is the one place it opens the
//! library, attaches the store journal and calls `resume_pending`, so the
//! startup path and the Android jobs host cannot drift from the CLI.
//!
//! It lives in the library (not the binary) so the host wiring is testable
//! without a window: `crates/suramak-ui/tests/durable_resume.rs` builds this
//! exact host with a deterministic backend and drives a seeded interrupted job
//! to completion.

use std::path::Path;

use suramak_asr::{
    open_durable_queue, InferenceBackend, JobOptions, PendingResume, Prefetcher, QueueDriver,
};
use suramak_core::SuramakError;

/// A driver whose journal is the app's library, plus the resume report.
pub struct DurableQueue<B: InferenceBackend, P: Prefetcher> {
    pub driver: QueueDriver<B, P>,
    /// One entry per journal row the launch looked at (oldest first). Rows with
    /// a `Resumed`/`Fresh` outcome are already enqueued in `driver`.
    pub resumed: Vec<PendingResume>,
}

impl<B: InferenceBackend, P: Prefetcher> DurableQueue<B, P> {
    /// Opens `db_path` and builds the driver exactly as the app will run it.
    pub fn open(
        backend: B,
        prefetch: P,
        options: JobOptions,
        db_path: &Path,
    ) -> Result<Self, SuramakError> {
        let queue = open_durable_queue(backend, prefetch, options, db_path)?;
        let (driver, resumed) = queue.into_parts();
        Ok(Self { driver, resumed })
    }

    /// Whether the driver actually owns a journal (the store opened).
    pub fn journaled(&self) -> bool {
        self.driver.journal().is_some()
    }

    /// Splits the host value into the bridge thread's pieces.
    pub fn into_parts(self) -> (QueueDriver<B, P>, Vec<PendingResume>) {
        (self.driver, self.resumed)
    }
}
