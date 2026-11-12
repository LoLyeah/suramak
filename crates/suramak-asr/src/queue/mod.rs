//! M16.1 — the bounded multi-file transcription queue.
//!
//! One [`JobQueue`] holds every accepted file and its per-item state:
//!
//! ```text
//! queued → decoding → transcribing → done
//!                       ↘ failed(reason)
//!           ↘ cancelled
//! ```
//!
//! ## Concurrency contract
//!
//! * **Exactly one item may hold the inference slot.** [`JobQueue::next_inference`]
//!   returns `None` while another item is running; the slot is released only by a
//!   terminal event (`done` / `failed` / `cancelled`). The whisper context cache is
//!   serialized behind the same slot, so two engine runs can never share it.
//! * **Decode may run ahead, bounded by [`MAX_DECODE_AHEAD`] (2) items.**
//!   Decoding is CPU-only, allocates one 16 kHz mono `f32` buffer per item and
//!   never touches the engine context, so it is safe to overlap with
//!   transcription. At most two items are in the decode-ahead stage at once
//!   (decoding *or* decoded and waiting for the slot); the item the engine is
//!   transcribing is outside that count. At most three sample buffers exist at
//!   any instant.
//! * **Memory is bounded by [`MAX_PREFETCH_BYTES`] (96 MiB).** A 16 kHz mono
//!   `f32` buffer is [`BYTES_PER_AUDIO_SECOND`] = 64 KiB per audio second, so the
//!   prefetch pool holds ~26 minutes of decoded audio; an item whose
//!   decoded buffer would cross the cap is dropped from the pool and decoded
//!   inline by the worker instead (never a failure, never unbounded growth).
//!
//! The queue is deliberately **in memory only**: the store keeps its existing
//! per-transcript semantics (exactly as a single job behaves today — nothing is
//! written until the finished transcript is saved; there is no jobs table and no
//! placeholder row). Quitting drops queued items, and the UI makes that loss
//! visible before the window goes away.
//!
//! [`JobQueue`] is pure (no threads, no clocks): every rule above is pinned by
//! the unit tests below. [`driver::QueueDriver`] is the production pump — one
//! inference slot, bounded decode-ahead — used by the CLI.

pub mod driver;
pub mod host;
pub mod journal;

use std::collections::HashSet;
use std::path::PathBuf;

use crate::engine::EngineId;
use crate::worker::JobStage;

/// Decoded-ahead items allowed at once (the documented bound).
pub const MAX_DECODE_AHEAD: usize = 2;

/// Total decoded audio the prefetch pool may retain, in bytes (96 MiB).
///
/// `16_000 samples/s × 4 B = 64 KiB` per audio second, so this is ~24 minutes
/// of decoded audio across the (at most two) prefetched items.
pub const MAX_PREFETCH_BYTES: usize = 96 * 1024 * 1024;

/// Bytes of a 16 kHz mono `f32` buffer per audio second.
pub const BYTES_PER_AUDIO_SECOND: usize = 16_000 * std::mem::size_of::<f32>();

/// The per-item state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemState {
    /// Accepted, not started.
    Queued,
    /// Decoding (`ready: false`), or decoded and waiting for the inference slot
    /// (`ready: true`). A decoded item stays in this state until it runs — it is
    /// not transcribing yet, and the state never pretends otherwise.
    Decoding { ready: bool },
    /// Holds the single inference slot.
    Transcribing,
    /// Finished successfully (the transcript was delivered).
    Done,
    /// Failed with the reason the job card shows.
    Failed { reason: String },
    /// Cancelled while queued or while running.
    Cancelled,
}

impl ItemState {
    /// Whether the item will never change again.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ItemState::Done | ItemState::Failed { .. } | ItemState::Cancelled
        )
    }

    /// Whether the item still counts as work in the queue.
    pub fn is_live(&self) -> bool {
        !self.is_terminal()
    }

    /// The short word the card and the CLI use for the state.
    pub fn label(&self) -> &'static str {
        match self {
            ItemState::Queued => "queued",
            ItemState::Decoding { ready: false } => "decoding",
            ItemState::Decoding { ready: true } => "ready",
            ItemState::Transcribing => "transcribing",
            ItemState::Done => "done",
            ItemState::Failed { .. } => "failed",
            ItemState::Cancelled => "cancelled",
        }
    }

    /// The failure reason, when this state is `failed`.
    pub fn reason(&self) -> Option<&str> {
        match self {
            ItemState::Failed { reason } => Some(reason),
            _ => None,
        }
    }
}

/// One accepted file and its current state.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueItem {
    /// Queue-local id, unique for the life of the process.
    pub id: u64,
    /// 1-based enqueue position. Stable across reorders and cancellations: the
    /// CLI's `--cancel` and its per-file lines refer to this, not to the current
    /// on-screen order.
    pub seq: usize,
    pub input: PathBuf,
    pub state: ItemState,
    /// 0–100, while this item holds the inference slot.
    pub pct: u8,
    /// Engine that actually ran (set at `done`).
    pub engine: Option<EngineId>,
    /// Non-fatal engine notes (downgrades, Core ML fallback).
    pub warnings: Vec<String>,
    /// The cancel was requested for this (running) item; the worker answers at
    /// its next stage boundary. Mirrors the card's `cancelling…`.
    pub cancelling: bool,
    /// Bytes this item contributed to the prefetch pool while decoded-ahead.
    pub decoded_bytes: usize,
    /// M22A2: this item continues a job the journal left interrupted — the
    /// app's existing job surface says so instead of presenting it as a fresh
    /// import.
    pub resumed: bool,
}

impl QueueItem {
    /// The file's display name (the full path when it has no file name).
    pub fn name(&self) -> String {
        self.input
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.input.display().to_string())
    }

    /// Whether this item is waiting (queued or decoding, including decoded and
    /// waiting for the slot).
    pub fn is_waiting(&self) -> bool {
        matches!(self.state, ItemState::Queued | ItemState::Decoding { .. })
    }

    /// Whether the item still counts as work in the queue.
    pub fn is_live(&self) -> bool {
        self.state.is_live()
    }
}

/// Counts by state, for the CLI's summary line and the card's compact header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueueCounts {
    pub queued: usize,
    pub decoding: usize,
    pub transcribing: usize,
    pub done: usize,
    pub failed: usize,
    pub cancelled: usize,
}

impl QueueCounts {
    pub fn total(&self) -> usize {
        self.queued + self.decoding + self.transcribing + self.done + self.failed + self.cancelled
    }

    /// Items that have not started (everything before the inference slot).
    pub fn waiting(&self) -> usize {
        self.queued + self.decoding
    }
}

/// A decode-ahead request handed to the prefetch pool.
#[derive(Debug, Clone)]
pub struct DecodeTicket {
    pub id: u64,
    pub seq: usize,
    pub input: PathBuf,
}

/// What the queue decided about one decoded buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeAdmission {
    /// Accepted: the caller keeps the samples; they count against the budget.
    Kept,
    /// The buffer would cross [`MAX_PREFETCH_BYTES`]: the item goes back to
    /// `queued` (marked "do not prefetch") and will decode inline instead.
    OverBudget,
    /// The item was cancelled (or already left the queue) while its decode was
    /// in flight — drop the buffer.
    Stale,
}

/// An item that may take the inference slot.
#[derive(Debug, Clone)]
pub struct InferenceTicket {
    pub id: u64,
    pub seq: usize,
    pub input: PathBuf,
    /// Whether the caller holds prefetched samples for this item. When `true`
    /// the item is already `transcribing`; when `false` it starts in `decoding`
    /// because the worker will decode it inline.
    pub prefetched: bool,
    /// M22A2: the item continues a journal row (the host marks its surface).
    pub resumed: bool,
}

/// The answer to a cancellation request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelDecision {
    /// A waiting item was removed (the queue list loses a row).
    Removed,
    /// The running item's cooperative token must be set; it stays running until
    /// the worker answers `Cancelled`.
    Running,
    /// The running item was already asked to cancel — idempotent.
    AlreadyRequested,
    /// Nothing to cancel (terminal or unknown item).
    Inert,
}

/// Returns prefetched bytes to the pool when an item stops holding them.
fn release_prefetch(item: &mut QueueItem, pool: &mut usize) {
    *pool = pool.saturating_sub(item.decoded_bytes);
    item.decoded_bytes = 0;
}

/// The bounded multi-file queue. Pure: no threads, no clocks.
#[derive(Debug, Clone, Default)]
pub struct JobQueue {
    items: Vec<QueueItem>,
    running: Option<u64>,
    next_id: u64,
    next_seq: usize,
    /// Items the prefetch pool must not try again (over budget, decode failure
    /// already reported): they decode inline when their turn comes.
    no_prefetch: HashSet<u64>,
    /// Bytes currently held by decoded-ahead items.
    prefetched_bytes: usize,
}

impl JobQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accepts a file at the end of the queue.
    pub fn enqueue(&mut self, input: PathBuf) -> u64 {
        self.enqueue_marked(input, false)
    }

    /// M22A2: accepts a file that continues a journal row left by an earlier
    /// process. The queue rules are identical; only the row carries the
    /// `resumed` marker the host surfaces.
    pub fn enqueue_resumed(&mut self, input: PathBuf) -> u64 {
        self.enqueue_marked(input, true)
    }

    fn enqueue_marked(&mut self, input: PathBuf, resumed: bool) -> u64 {
        self.next_id += 1;
        self.next_seq += 1;
        let id = self.next_id;
        self.items.push(QueueItem {
            id,
            seq: self.next_seq,
            input,
            state: ItemState::Queued,
            pct: 0,
            engine: None,
            warnings: Vec::new(),
            cancelling: false,
            decoded_bytes: 0,
            resumed,
        });
        id
    }

    /// No items at all (live or terminal).
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    pub fn item(&self, id: u64) -> Option<&QueueItem> {
        self.items.iter().find(|item| item.id == id)
    }

    pub fn running(&self) -> Option<&QueueItem> {
        self.running.and_then(|id| self.item(id))
    }

    pub fn running_id(&self) -> Option<u64> {
        self.running
    }

    /// Items that have not started, in queue order (the card's list).
    ///
    /// The running item is never a waiting row, even while it is still in its
    /// inline decode stage — it is the card's subject, not "up next".
    pub fn waiting_items(&self) -> Vec<&QueueItem> {
        self.items
            .iter()
            .filter(|item| self.is_waiting_row(item))
            .collect()
    }

    pub fn waiting_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| self.is_waiting_row(item))
            .count()
    }

    /// Items in the decode-ahead stage — decoding now *or* decoded and waiting
    /// for the slot. This is the item bound ([`MAX_DECODE_AHEAD`]); the byte
    /// bound ([`MAX_PREFETCH_BYTES`]) governs the buffers themselves.
    pub fn decoding_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| {
                Some(item.id) != self.running && matches!(item.state, ItemState::Decoding { .. })
            })
            .count()
    }

    /// A row is "waiting" when it has not started and is not the running item.
    fn is_waiting_row(&self, item: &QueueItem) -> bool {
        Some(item.id) != self.running && item.is_waiting()
    }

    /// Bytes the prefetch pool is holding for decoded-ahead items.
    pub fn prefetched_bytes(&self) -> usize {
        self.prefetched_bytes
    }

    pub fn counts(&self) -> QueueCounts {
        let mut counts = QueueCounts::default();
        for item in &self.items {
            match item.state {
                ItemState::Queued => counts.queued += 1,
                ItemState::Decoding { .. } => counts.decoding += 1,
                ItemState::Transcribing => counts.transcribing += 1,
                ItemState::Done => counts.done += 1,
                ItemState::Failed { .. } => counts.failed += 1,
                ItemState::Cancelled => counts.cancelled += 1,
            }
        }
        counts
    }

    /// Nothing waiting and nothing running.
    pub fn is_idle(&self) -> bool {
        self.running.is_none() && self.waiting_count() == 0
    }

    /// The next decode-ahead request, if the bound allows one.
    ///
    /// In queue order (so the item the engine will need first is decoded first),
    /// never more than [`MAX_DECODE_AHEAD`] at once, and never an item the pool
    /// already skipped for budget reasons.
    pub fn next_decode(&mut self) -> Option<DecodeTicket> {
        if self.decoding_count() >= MAX_DECODE_AHEAD || self.prefetched_bytes >= MAX_PREFETCH_BYTES
        {
            return None;
        }
        let index = self.items.iter().position(|item| {
            item.state == ItemState::Queued && !self.no_prefetch.contains(&item.id)
        })?;
        let item = &mut self.items[index];
        item.state = ItemState::Decoding { ready: false };
        Some(DecodeTicket {
            id: item.id,
            seq: item.seq,
            input: item.input.clone(),
        })
    }

    /// Admits one decoded buffer against the memory budget.
    pub fn mark_decoded(&mut self, id: u64, bytes: usize) -> DecodeAdmission {
        let Some(index) = self.items.iter().position(|item| item.id == id) else {
            return DecodeAdmission::Stale;
        };
        if !matches!(
            self.items[index].state,
            ItemState::Decoding { ready: false }
        ) {
            return DecodeAdmission::Stale;
        }
        if bytes > MAX_PREFETCH_BYTES.saturating_sub(self.prefetched_bytes) {
            // Too large for the pool: decode inline when this item's turn comes.
            self.no_prefetch.insert(id);
            self.items[index].state = ItemState::Queued;
            return DecodeAdmission::OverBudget;
        }
        self.prefetched_bytes += bytes;
        self.items[index].state = ItemState::Decoding { ready: true };
        self.items[index].decoded_bytes = bytes;
        DecodeAdmission::Kept
    }

    /// The prefetch pool could not decode this item (bad container, I/O). It is
    /// a named failure — the queue continues with the next item. Returns whether
    /// the item was still live (a cancelled item is left alone).
    pub fn decode_failed(&mut self, id: u64, reason: String) -> bool {
        self.no_prefetch.insert(id);
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            if item.is_live() {
                release_prefetch(item, &mut self.prefetched_bytes);
                item.state = ItemState::Failed { reason };
                return true;
            }
        }
        false
    }

    /// The next item for the single inference slot, if it is free.
    ///
    /// Queue order is strict: if the head of the queue is still decoding, the
    /// slot waits for it rather than letting a later (already decoded) item jump
    /// ahead.
    pub fn next_inference(&mut self) -> Option<InferenceTicket> {
        if self.running.is_some() {
            return None;
        }
        let index = self.items.iter().position(|item| item.is_live())?;
        let prefetched = match self.items[index].state {
            ItemState::Decoding { ready: true } => true,
            ItemState::Queued => false,
            // Decoding in flight: wait. Transcribing can only be the running
            // item, handled by the early return above.
            _ => return None,
        };
        let id = self.items[index].id;
        self.running = Some(id);
        let item = &mut self.items[index];
        item.pct = 0;
        item.state = if prefetched {
            ItemState::Transcribing
        } else {
            ItemState::Decoding { ready: false }
        };
        if prefetched {
            release_prefetch(item, &mut self.prefetched_bytes);
        }
        Some(InferenceTicket {
            id,
            seq: item.seq,
            input: item.input.clone(),
            prefetched,
            resumed: item.resumed,
        })
    }

    /// The worker reported a pipeline stage for the running item.
    pub fn on_stage(&mut self, id: u64, stage: JobStage) {
        if self.running != Some(id) {
            return;
        }
        if stage == JobStage::Transcribing {
            if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                if matches!(item.state, ItemState::Decoding { .. }) {
                    item.state = ItemState::Transcribing;
                }
            }
        }
    }

    /// Worker progress, 0–100, for the running item only.
    pub fn on_progress(&mut self, id: u64, pct: u8) {
        if self.running != Some(id) {
            return;
        }
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.pct = pct.min(100);
        }
    }

    /// The running item finished; its transcript is in the caller's hands.
    pub fn on_finished(&mut self, id: u64, engine: EngineId, warnings: Vec<String>) {
        if self.running == Some(id) {
            self.running = None;
        }
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.engine = Some(engine);
            item.warnings = warnings;
            item.pct = 100;
            item.state = ItemState::Done;
        }
    }

    /// The running (or decoding) item failed with a named reason.
    pub fn on_failed(&mut self, id: u64, reason: String) {
        if self.running == Some(id) {
            self.running = None;
        }
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            if item.is_live() {
                release_prefetch(item, &mut self.prefetched_bytes);
                item.state = ItemState::Failed { reason };
            }
        }
    }

    /// The worker confirmed the running item's cancellation.
    pub fn on_cancelled(&mut self, id: u64) {
        if self.running == Some(id) {
            self.running = None;
        }
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            if item.is_live() {
                release_prefetch(item, &mut self.prefetched_bytes);
                item.state = ItemState::Cancelled;
            }
        }
    }

    /// Cancels one item.
    ///
    /// A waiting item is removed outright. The running item's token is the
    /// caller's to set (the existing seam); the queue only records the request,
    /// keeps the slot occupied and waits for the terminal `cancelled` event, so
    /// a second job can never start on top of a half-stopped one.
    pub fn cancel(&mut self, id: u64) -> CancelDecision {
        if self.running == Some(id) {
            if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                if item.cancelling {
                    return CancelDecision::AlreadyRequested;
                }
                item.cancelling = true;
            }
            return CancelDecision::Running;
        }
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            if item.is_waiting() {
                release_prefetch(item, &mut self.prefetched_bytes);
                item.state = ItemState::Cancelled;
                return CancelDecision::Removed;
            }
        }
        CancelDecision::Inert
    }

    /// Moves a waiting item one place earlier (the card's Up control).
    pub fn raise(&mut self, id: u64) -> bool {
        self.shift_waiting(id, -1)
    }

    /// Moves a waiting item one place later (the card's Down control).
    pub fn lower(&mut self, id: u64) -> bool {
        self.shift_waiting(id, 1)
    }

    fn shift_waiting(&mut self, id: u64, delta: isize) -> bool {
        let Some(index) = self.items.iter().position(|item| item.id == id) else {
            return false;
        };
        if !self.is_waiting_row(&self.items[index]) {
            return false;
        }
        let mut cursor = index as isize + delta;
        while cursor >= 0 && (cursor as usize) < self.items.len() {
            let candidate = &self.items[cursor as usize];
            if self.is_waiting_row(candidate) {
                self.items.swap(index, cursor as usize);
                return true;
            }
            cursor += delta;
        }
        false
    }

    /// Drops every item that has not started, returning the dropped rows.
    ///
    /// The quit path calls this after the user acknowledged the loss; the
    /// running item (if any) keeps today's behavior — it is not interrupted.
    pub fn drop_waiting(&mut self) -> Vec<QueueItem> {
        let running = self.running;
        let mut dropped = Vec::new();
        for item in self.items.iter_mut() {
            if Some(item.id) == running || !item.is_waiting() {
                continue;
            }
            release_prefetch(item, &mut self.prefetched_bytes);
            item.state = ItemState::Cancelled;
            dropped.push(item.clone());
        }
        dropped
    }

    /// Removes terminal rows (done / failed / cancelled) from the list. The UI
    /// calls this when a batch starts so stale outcomes never linger.
    pub fn retain_live(&mut self) {
        self.items.retain(|item| item.is_live());
        let live: HashSet<u64> = self.items.iter().map(|item| item.id).collect();
        self.no_prefetch.retain(|id| live.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        PathBuf::from("/tmp/suramak-queue-tests").join(name)
    }

    fn queue_with(names: &[&str]) -> (JobQueue, Vec<u64>) {
        let mut queue = JobQueue::new();
        let ids = names.iter().map(|name| queue.enqueue(path(name))).collect();
        (queue, ids)
    }

    /// The documented walk: queued → decoding → transcribing → done, with the
    /// decode-ahead pool feeding the slot.
    #[test]
    fn the_state_machine_walks_queued_decoding_transcribing_done() {
        let (mut queue, ids) = queue_with(&["a.wav"]);
        assert_eq!(queue.item(ids[0]).unwrap().state, ItemState::Queued);

        let ticket = queue.next_decode().expect("decode ahead");
        assert_eq!(ticket.id, ids[0]);
        assert!(matches!(
            queue.item(ids[0]).unwrap().state,
            ItemState::Decoding { ready: false }
        ));

        assert_eq!(queue.mark_decoded(ids[0], 1_000), DecodeAdmission::Kept);
        assert!(matches!(
            queue.item(ids[0]).unwrap().state,
            ItemState::Decoding { ready: true }
        ));

        let run = queue.next_inference().expect("the slot is free");
        assert!(run.prefetched);
        assert_eq!(queue.item(ids[0]).unwrap().state, ItemState::Transcribing);

        queue.on_progress(ids[0], 42);
        assert_eq!(queue.item(ids[0]).unwrap().pct, 42);
        queue.on_finished(ids[0], EngineId::Cpu, vec!["note".to_string()]);
        assert_eq!(queue.item(ids[0]).unwrap().state, ItemState::Done);
        assert_eq!(queue.item(ids[0]).unwrap().engine, Some(EngineId::Cpu));
        assert!(queue.is_idle());
    }

    /// Exactly one item can ever hold the inference slot.
    #[test]
    fn only_one_item_holds_the_inference_slot() {
        let (mut queue, _) = queue_with(&["a.wav", "b.wav", "c.wav"]);
        assert!(queue.next_inference().is_some());
        assert!(
            queue.next_inference().is_none(),
            "a second item must never take the slot"
        );
        assert_eq!(queue.running_id(), Some(1));
        // An inline item starts in its decode stage; the stage event is what
        // makes it `transcribing`.
        assert_eq!(queue.counts().transcribing, 0);
        queue.on_stage(1, JobStage::Transcribing);
        assert_eq!(queue.counts().transcribing, 1);
        // Only a terminal event releases it.
        queue.on_failed(1, "boom".to_string());
        assert!(queue.next_inference().is_some());
    }

    /// At most [`MAX_DECODE_AHEAD`] decodes are in flight; completing one lets
    /// the next start.
    #[test]
    fn decode_ahead_is_bounded_to_two_items() {
        let (mut queue, _) = queue_with(&["a.wav", "b.wav", "c.wav", "d.wav"]);
        assert!(queue.next_decode().is_some());
        assert!(queue.next_decode().is_some());
        assert!(
            queue.next_decode().is_none(),
            "a third decode would exceed the bound"
        );
        assert_eq!(queue.decoding_count(), MAX_DECODE_AHEAD);

        // A decoded item still occupies its decode-ahead slot — no third item
        // may be decoded while two are waiting.
        queue.mark_decoded(1, 1_000);
        assert!(queue.next_decode().is_none(), "both slots are still held");
        // Taking one into the inference slot frees exactly one.
        queue.next_inference().expect("the decoded head runs");
        assert!(queue.next_decode().is_some());
        assert_eq!(queue.decoding_count(), MAX_DECODE_AHEAD);
        assert!(queue.next_decode().is_none());
    }

    /// A buffer that would cross the memory cap is refused, the item returns to
    /// `queued` and is decoded inline — bounded memory without a stuck queue.
    #[test]
    fn a_decoded_buffer_over_the_memory_budget_is_decoded_inline() {
        let (mut queue, ids) = queue_with(&["huge.wav", "small.wav"]);
        queue.next_decode().expect("decode ahead");
        assert_eq!(
            queue.mark_decoded(ids[0], MAX_PREFETCH_BYTES + 1),
            DecodeAdmission::OverBudget
        );
        assert_eq!(queue.prefetched_bytes(), 0);
        assert_eq!(queue.item(ids[0]).unwrap().state, ItemState::Queued);
        // The oversized item is not retried; the next queued item is.
        let ticket = queue.next_decode().expect("the next item");
        assert_eq!(ticket.id, ids[1]);
        // The slot still takes the oversized head first, inline.
        let run = queue.next_inference().expect("the slot is free");
        assert_eq!(run.id, ids[0]);
        assert!(!run.prefetched);
    }

    /// Cancelling a queued item removes it; the running item is untouched.
    #[test]
    fn cancel_removes_a_queued_item() {
        let (mut queue, ids) = queue_with(&["a.wav", "b.wav"]);
        queue.next_inference().expect("a runs");
        assert_eq!(queue.cancel(ids[1]), CancelDecision::Removed);
        assert_eq!(queue.item(ids[1]).unwrap().state, ItemState::Cancelled);
        assert_eq!(queue.running_id(), Some(ids[0]));
        assert_eq!(queue.waiting_count(), 0);
    }

    /// A cancel that lands while the item is decoding drops the buffer and the
    /// queue walks on to the next item.
    #[test]
    fn cancel_mid_decode_is_stale_and_the_queue_continues() {
        let (mut queue, ids) = queue_with(&["a.wav", "b.wav"]);
        queue.next_decode().expect("a decodes");
        assert_eq!(queue.cancel(ids[0]), CancelDecision::Removed);
        assert_eq!(
            queue.mark_decoded(ids[0], 1_000),
            DecodeAdmission::Stale,
            "a late buffer for a cancelled item must be dropped"
        );
        assert_eq!(queue.prefetched_bytes(), 0);
        let run = queue.next_inference().expect("b takes the slot");
        assert_eq!(run.id, ids[1]);
    }

    /// A cancel that lands mid-transcription keeps the slot occupied until the
    /// worker confirms — no second job starts on top of a half-stopped one.
    #[test]
    fn cancel_mid_transcribe_waits_for_the_worker_and_frees_the_slot() {
        let (mut queue, ids) = queue_with(&["a.wav", "b.wav"]);
        queue.next_inference().expect("a runs");
        assert_eq!(queue.cancel(ids[0]), CancelDecision::Running);
        assert!(queue.item(ids[0]).unwrap().cancelling);
        assert_eq!(queue.cancel(ids[0]), CancelDecision::AlreadyRequested);
        assert!(
            queue.next_inference().is_none(),
            "the slot is still a's until the worker answers"
        );
        queue.on_cancelled(ids[0]);
        assert_eq!(queue.item(ids[0]).unwrap().state, ItemState::Cancelled);
        assert_eq!(queue.next_inference().map(|ticket| ticket.id), Some(ids[1]));
    }

    /// One failed item never blocks the next one; the reason is named.
    #[test]
    fn a_failure_does_not_cascade() {
        let (mut queue, ids) = queue_with(&["bad.wav", "good.wav", "next.wav"]);
        queue.next_inference().expect("bad runs");
        queue.on_failed(ids[0], "decode: unsupported codec".to_string());
        assert_eq!(
            queue.item(ids[0]).unwrap().state.reason(),
            Some("decode: unsupported codec")
        );
        let run = queue.next_inference().expect("good runs");
        assert_eq!(run.id, ids[1]);
        queue.on_finished(ids[1], EngineId::Cpu, Vec::new());
        assert_eq!(queue.next_inference().map(|t| t.id), Some(ids[2]));
        assert_eq!(queue.counts().failed, 1);
        assert_eq!(queue.counts().done, 1);
    }

    /// A decode-ahead failure is named and the queue continues.
    #[test]
    fn a_decode_failure_is_named_and_the_queue_continues() {
        let (mut queue, ids) = queue_with(&["bad.wav", "good.wav"]);
        queue.next_decode().expect("bad decodes");
        queue.decode_failed(ids[0], "decode: truncated ogg".to_string());
        assert_eq!(
            queue.item(ids[0]).unwrap().state,
            ItemState::Failed {
                reason: "decode: truncated ogg".to_string()
            }
        );
        let run = queue.next_inference().expect("good takes the slot");
        assert_eq!(run.id, ids[1]);
    }

    /// Shutdown drops the waiting items (named) and leaves the running one alone
    /// — today's Stop & Save behavior for started work.
    #[test]
    fn shutdown_drops_waiting_items_and_names_them() {
        let (mut queue, ids) = queue_with(&["running.wav", "queued.wav", "decoded.wav"]);
        queue.next_inference().expect("the first item runs");
        queue.next_decode().expect("the third decodes");
        queue.mark_decoded(ids[2], 2_000);
        let dropped = queue.drop_waiting();
        let names: Vec<String> = dropped.iter().map(QueueItem::name).collect();
        assert_eq!(names, vec!["queued.wav", "decoded.wav"]);
        assert_eq!(queue.item(ids[1]).unwrap().state, ItemState::Cancelled);
        assert_eq!(queue.item(ids[2]).unwrap().state, ItemState::Cancelled);
        assert_eq!(queue.running_id(), Some(ids[0]));
        assert_eq!(queue.prefetched_bytes(), 0);
    }

    /// Up/down reorder waiting rows; the running row and terminal rows are not
    /// movable.
    #[test]
    fn raise_and_lower_reorder_waiting_items_only() {
        let (mut queue, ids) = queue_with(&["running.wav", "a.wav", "b.wav", "c.wav"]);
        queue.next_inference().expect("the first item runs");

        assert!(queue.raise(ids[2]), "b moves above a");
        let order: Vec<u64> = queue.items().iter().map(|item| item.id).collect();
        assert_eq!(order, vec![ids[0], ids[2], ids[1], ids[3]]);

        assert!(queue.lower(ids[2]));
        let order: Vec<u64> = queue.items().iter().map(|item| item.id).collect();
        assert_eq!(order, vec![ids[0], ids[1], ids[2], ids[3]]);

        assert!(!queue.raise(ids[0]), "the running row is pinned");
        assert!(!queue.lower(ids[3]), "c is already last among waiting rows");
        assert!(queue.raise(ids[3]), "the last row can still move up");
        queue.on_cancelled(ids[0]);
        assert!(
            !queue.raise(ids[0]),
            "a terminal row is not part of the queue anymore"
        );
    }

    /// Queue order wins over decode order: a later item decoded first must not
    /// jump ahead of an undecoded head.
    #[test]
    fn queue_order_wins_when_a_later_item_decodes_first() {
        let (mut queue, ids) = queue_with(&["head.wav", "later.wav"]);
        // Decode the head, then the later item (the head stays in flight).
        let head = queue.next_decode().expect("head decode");
        let later = queue.next_decode().expect("later decode");
        assert_eq!(head.id, ids[0]);
        assert_eq!(later.id, ids[1]);
        assert_eq!(queue.mark_decoded(later.id, 1_000), DecodeAdmission::Kept);
        assert!(
            queue.next_inference().is_none(),
            "the head is still decoding — the slot waits"
        );
        assert_eq!(queue.mark_decoded(head.id, 1_000), DecodeAdmission::Kept);
        assert_eq!(queue.next_inference().map(|t| t.id), Some(ids[0]));
    }

    /// Once the slot takes a prefetched item, its bytes leave the pool (the job
    /// owns the buffer from then on).
    #[test]
    fn prefetched_bytes_are_released_when_the_slot_takes_the_item() {
        let (mut queue, ids) = queue_with(&["a.wav", "b.wav"]);
        queue.next_decode().expect("a");
        queue.mark_decoded(ids[0], 4_000);
        assert_eq!(queue.prefetched_bytes(), 4_000);
        queue.next_inference().expect("a runs");
        assert_eq!(queue.prefetched_bytes(), 0);
    }

    /// The card's "+n more" arithmetic: waiting rows are bounded for display.
    #[test]
    fn waiting_rows_can_be_bounded_for_the_card() {
        let mut queue = JobQueue::new();
        for index in 0..6 {
            queue.enqueue(path(&format!("clip-{index}.wav")));
        }
        queue.next_inference().expect("one runs");
        let waiting = queue.waiting_items();
        assert_eq!(waiting.len(), 5);
        let visible: Vec<&QueueItem> = waiting.iter().copied().take(3).collect();
        assert_eq!(visible.len(), 3);
        assert_eq!(waiting.len() - visible.len(), 2, "the '+2 more' line");
        // The counts are raw state counts (the running item's inline decode is
        // a `decoding` row there); the card's list uses `waiting_items`.
        assert_eq!(queue.counts().total(), 6);
    }
}
