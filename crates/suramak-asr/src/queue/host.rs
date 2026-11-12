//! M22A2 — the one way a host makes its queue durable.
//!
//! The CLI, the desktop app and the Android jobs host all need the same three
//! steps before their first job: open the app's library store, attach it to a
//! [`QueueDriver`] as the journal, and resume every row a previous process left
//! mid-job. Keeping that sequence in one function is what stops the hosts from
//! drifting — a host that forgot `resume_pending` would look wired but silently
//! start a crashed lecture from zero.
//!
//! The driver is the journal's single writer; callers must not open a second
//! `QueueDriver` over the same database while this one is alive.

use std::path::Path;

use suramak_core::store::Store;
use suramak_core::SuramakError;

use super::driver::{InferenceBackend, JobOptions, PendingResume, Prefetcher, QueueDriver};

/// A driver with the durable journal attached, plus what the launch-time
/// resume did with every interrupted row.
pub struct DurableQueue<B: InferenceBackend, P: Prefetcher> {
    pub driver: QueueDriver<B, P>,
    /// One entry per journal row `resume_pending` looked at, in journal order
    /// (oldest first). `Resumed`/`Fresh` rows are already in the driver's queue.
    pub resumed: Vec<PendingResume>,
}

impl<B: InferenceBackend, P: Prefetcher> DurableQueue<B, P> {
    /// Splits the host value into the two pieces its worker needs.
    pub fn into_parts(self) -> (QueueDriver<B, P>, Vec<PendingResume>) {
        (self.driver, self.resumed)
    }
}

/// Opens `db_path`, builds a queue driver whose journal is that store, and
/// resumes every interrupted job **before** the host enqueues new work.
///
/// This is deliberately the only constructor the hosts use: `Store::open` runs
/// the M22A torn-write reconciliation, and `resume_pending` is the only place
/// that turns a `decoding`/`transcribing` row into a runnable item (or a named
/// `resume_unavailable` failure).
pub fn open_durable_queue<B: InferenceBackend, P: Prefetcher>(
    backend: B,
    prefetch: P,
    options: JobOptions,
    db_path: &Path,
) -> Result<DurableQueue<B, P>, SuramakError> {
    let store = Store::open(db_path)?;
    let mut driver = QueueDriver::new(backend, prefetch, options).with_journal(store);
    let resumed = driver.resume_pending();
    Ok(DurableQueue { driver, resumed })
}
