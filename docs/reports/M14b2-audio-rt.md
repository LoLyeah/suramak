# M14B2 Report — lock-free audio capture (SUR-HIGH-03)

**Date**: 2026-10-21
**Executor**: Hermes subagent (deepseek-v4.1-flash)
**Workorder**: `docs/WORKORDER-M14B2.md` (authoritative). Read first, as instructed: `docs/reports/AGY-AUDIT-2026-10-21.md` §SUR-HIGH-03, `crates/suramak-asr/src/capture.rs` (whole file), and the M7 chunk consumer `crates/suramak-ui/src/runtime.rs` (`live_step` → `MultiRecorder::take_live_chunk`).

**Delivered**: the cpal input callback is now lock-free, allocation-free and syscall-free. It mixes each delivery down through an allocation-free iterator and try-pushes into an `rtrb` SPSC ring; a per-session consumer thread owns the shared sample `Vec`, the rolling RMS and the `rms_bits` word, drains the ring, and is joined on stop after a final drain. A full ring drops the incoming samples and counts them in an atomic. Seven new host-runnable unit tests (sequence parity, overflow drop policy, non-blocking stress with a 0–50 ms consumer stall, shutdown drain, ring sizing, device-format conversions, mixdown parity).

**Untouched**: device enumeration and every public `capture` signature (`Recorder`, `MultiRecorder`, `start_recording*`, `take_live_chunk`, `stop`), resampling, the mixing engine, the whisper/live pipeline, `keep_audio` semantics, `suramak-taps`, `suramak-ui`. No `git commit`/`push`, no sudo, nothing outside `/home/ubuntu/suramak`, no new dependency besides `rtrb`.

---

## 1. The audit's three complaints, answered in code

| Audit complaint (SUR-HIGH-03) | Answer | Where |
|---|---|---|
| "Locking Standard Mutexes: `samples.lock()` … the UI thread also locks `samples` … priority inversion" | No `Mutex` is reachable from the RT path. `RtProducer` holds `rtrb::Producer<f32>` + `Arc<AtomicU64>`; the only shared mutations are the ring's head/tail atomics and the drop counter. The callback closure captures `RtProducer` + `usize` only. | `capture_rt.rs:190` (`write_block`), `capture.rs:661-703` (the three closures) |
| "Heap Allocation on the Audio Thread: `mono_mixdown` creates a new `Vec` … `buffer.push` triggers reallocations" | The mixdown is an iterator (`MonoFrames`) over the device slice; the ring is allocated once at session start; the shared `Vec` only grows on the consumer thread. The `i16`/`u16` arms no longer build an intermediate `Vec<f32>` either — `Interleaved` converts in place. | `capture_rt.rs:76-164`, `capture_rt.rs:67` (`ring_capacity`) |
| (implicit) "no policy for a full buffer — the old code simply blocked" | Drop-newest at the producer: once a push fails the rest of the block is skipped, the count goes into an `AtomicU64`; `Recorder::stop` logs a warning if it is ever non-zero. §5 documents why drop-oldest is not possible lock-free and why a full ring is already a fatal state. | `capture_rt.rs:190-210`, `capture.rs:477-486` |

The old callback, in full (this is what is gone):

```rust
fn accumulate(
    samples: &Mutex<Vec<f32>>,
    rms_bits: &AtomicU32,
    rolling: &mut RollingRms,
    data: &[f32],
    channels: usize,
) {
    if data.is_empty() { return; }
    let mut buffer = samples.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for mono in mono_mixdown(data, channels) {   // Vec<f32> allocation, every delivery
        buffer.push(mono);                       // reallocation under the lock
        rolling.push(mono);
    }
    rms_bits.store(rolling.value().to_bits(), Ordering::Relaxed);
}
```

…plus, in the `I16` and `U16` arms, `let floats: Vec<f32> = data.iter().map(…).collect();` before the same call.

## 2. Before / after callback path

### Before (one delivery, RT thread)

1. `samples.lock()` — OS mutex, contended with the UI (`take_live_chunk`), with `stop`, and with the other recorders of a multi-source session.
2. `mono_mixdown` → heap-allocate a `Vec<f32>` sized for the block.
3. `buffer.push` per sample → reallocation + memcpy when the recording buffer crosses a capacity step (which it does, repeatedly, for the first minutes of every recording).
4. `RollingRms::push` per sample (cheap, but on the RT thread).
5. `rms_bits.store`.
6. Unlock (may wake a waiting UI thread — a futex syscall on the contention path).

For `i16`/`u16` devices, another `Vec<f32>` allocation and a full conversion pass came first.

### After

```rust
// capture.rs — the whole callback, one arm of the format match
cpal::SampleFormat::F32 => {
    let mut sink = RtProducer::new(producer, Arc::clone(&dropped));
    let channels = config.channels as usize;
    device.build_input_stream(
        &config,
        move |data: &[f32], _| { sink.write_block(data, channels); },
        stream_error(Arc::clone(&teardown)),
        None,
    )?
}
```

```rust
// capture_rt.rs — write_block: the entire RT workload
pub(crate) fn write_block<B: Interleaved + ?Sized>(&mut self, block: &B, channels: usize) -> usize {
    let mut frames = MonoFrames::new(block, channels);   // no allocation: an iterator
    let total = frames.len();
    let mut dropped = 0usize;
    for (index, mono) in frames.by_ref().enumerate() {
        if self.ring.push(mono).is_err() {                // wait-free; Err == full
            dropped = total - index;                      // skip the rest of the block
            break;
        }
    }
    if dropped > 0 {
        self.dropped.fetch_add(dropped as u64, Ordering::Relaxed);
    }
    dropped
}
```

`MonoFrames::next` averages one frame by `(start..end).map(sample).sum::<f32>() / len` — the same operations, in the same order, as the old `frame.iter().sum::<f32>() / frame.len() as f32`, so the samples are bit-identical (test `ring_path_matches_the_legacy_accumulate_exactly` compares 22 634 samples, and the published RMS after every one of 64 blocks, bit-for-bit).

Non-RT side:

```rust
// capture_rt.rs — the consumer thread's loop
fn run(mut self, stop: &AtomicBool) {
    loop {
        let drained = self.drain();
        if stop.load(Ordering::Acquire) && drained == 0 { break; }  // final drain done
        if drained == 0 { std::thread::park_timeout(IDLE_POLL); }    // 5 ms
    }
}
```

`drain` takes the buffer mutex **once per 1024-sample chunk**, extends the `Vec`, pushes the chunk into `RollingRms`, and stores `rms_bits` once per drain.

### Stop / quit handoff

`Recorder::stop` (and the plain-`Drop` quit path, via field order) is: mark teardown → `drop(stream)` (stops the device callback, which drops the ring's producer) → `drop(tap)` → `consumer.shutdown()` (set the stop flag, join; the loop's last drain empties the ring) → clone the buffer. So the returned `AudioBuffer` contains every sample the callback ever pushed — exactly what the in-callback accumulation produced. The consumer field is declared last so a plain `Drop` gets the same ordering without any extra code.

## 3. Why `rtrb`, not `ringbuf`

`rtrb = "=0.3.1"` (pinned in `[workspace.dependencies]`, one lock entry, no transitive dependencies — verified in `Cargo.lock`: the `rtrb` package has no `dependencies` list).

* **The guarantee we need is its documented contract.** rtrb's crate docs: "Reading from and writing into the ring buffer is *lock-free* and *wait-free*. All reading and writing functions return immediately." `push` is a slot store plus a `Release` store of the tail — no CAS, no retry loop, no spin.
* **Exact capacity, allocated once.** `RingBuffer::new(capacity)` allocates `Vec::with_capacity(capacity)` and uses the number verbatim (no power-of-two rounding in 0.3.1), so "1 s at the session rate" is literally what you get. The overflow test pins this: a 64-slot ring accepts exactly 64 samples and drops 2 496 of the 2 560 offered.
* **Zero dependencies.** `ringbuf 0.4` is a fine SPSC ring too (`HeapRb::new(..).split()`, `try_push`), but it pulls `crossbeam-utils` (crates.io metadata for `ringbuf 0.4.0`: normal dependency `crossbeam-utils ^0.8`). For a crate whose only job is to hold the RT line, one dependency-free, `no_std`-capable implementation with `read_chunk`/`as_slices` batch reads was the smaller surface.
* **Batch reads with commit semantics.** `Consumer::read_chunk(n)` + `as_slices()` + `commit_all()` let the consumer copy with `extend_from_slice` instead of per-sample `pop()`.

Both crates would have satisfied the workorder; the deciding factors were "no dependencies" and "wait-free is the documented API contract, not an implementation detail". `rtrb` is **not** optional in `suramak-asr`: it is tiny and dependency-free, and keeping it unconditional is what lets the RT-path tests run on every build (including the Linux CI job that has no `capture` feature). The module is `pub(crate)`, so the public API is unchanged.

## 4. Ring sizing

```rust
pub(crate) fn ring_capacity(sample_rate: u32) -> usize {
    (sample_rate as usize).saturating_mul(RING_SECONDS).max(1)   // RING_SECONDS = 1
}
```

| Session rate | Slots (`f32`) | Bytes | Slack |
|---|---|---|---|
| 48 000 Hz | 48 000 | 192 000 (187.5 KiB) | 1.000 s |
| 44 100 Hz | 44 100 | 176 400 (172.3 KiB) | 1.000 s |
| 16 000 Hz | 16 000 | 64 000 (62.5 KiB) | 1.000 s |

Mono samples, so the channel count does not change the size: a 48 kHz stereo device produces 48 000 mono frames per second and therefore fills the ring in exactly 1 s if the consumer is dead. A multi-source session allocates one ring (and one consumer thread) per `Recorder` — 2 sources worst case = 384 KiB at 48 kHz.

The allocation happens in `start_one` **before** the stream is built (`capture.rs:644`), on the caller's thread; `rtrb` never allocates again.

For calibration: the stress test's consumer stalls for up to 50 ms at a time, i.e. it uses at most 5 % of the ring; a healthy consumer drains in microseconds.

## 5. Drop policy

**Drop-newest, counted, per sample, at the producer.** When `push` reports full, the remaining samples of that block are skipped (one failed push per block, not one per sample) and added to `dropped_samples` (`AtomicU64`, `Relaxed` — the value is only read at stop and in tests).

* **Why not drop-oldest**: it cannot be done from the producer side of an SPSC ring without the consumer's index — it would need a second shared cursor and a CAS loop, i.e. exactly the unbounded retry the RT thread must not have. The consumer *could* drop-oldest on its side, but that buys nothing: a full ring already means the consumer has not run for a whole second.
* **When it can happen**: only if the consumer thread stopped (panic, killed) or the machine starved it for ≥ 1 s while audio kept arriving. Both are already "the recording is lost" states — the counter does not change the outcome, it makes it diagnosable.
* **What the user sees**: nothing in the normal case. At stop, if the counter is non-zero, one line: `audio capture dropped samples — the capture consumer thread could not keep up` (`tracing::warn!`, with `device` and `dropped`). No UI change, no error path.
* **Cost of an overflowing block**: one failed `push` plus one `fetch_add` — measured 9–26 µs per 256-sample block into a 64-slot ring on this 2-vCPU VPS, harness overhead included. In the steady state a 192-frame stereo block costs ~27 µs end to end (10.2 ms for 375 blocks), i.e. 0.7 % of its 4 ms audio period.

## 6. Observable behavior — what is identical and what moved

| Observable | Before | After |
|---|---|---|
| Sample sequence / final `AudioBuffer` | pushed by the callback | identical samples, in order, drained by the consumer and joined before `stop` returns — bit-for-bit equal in the parity test |
| `rms_bits` (level meter) | stored after each device block | same value sequence when drained per block (parity test compares `to_bits()` after all 64 blocks); in the app it is published once per drain, so it can lag by ≤ 5 ms (the idle poll) — invisible against the 100 ms RMS window |
| `take_live_chunk` (M7) | read from the `Vec` | unchanged code, same `Vec`, at most ~5 ms later. The UI calls it from its frame loop (`runtime.rs:751`, from `main.rs:559`), so the lag never accumulates |
| `keep_audio`, resampling, mix, whisper | — | untouched |
| Threads | 1 device thread | 1 device thread + 1 consumer thread per source, named `suramak-capture`, parked 5 ms at a time |

The live-chunk cursor logic and the buffer lock are unchanged; only the writer moved off the RT thread.

## 7. Tests

All host-runnable, no device, no feature gate (they run in `cargo test --workspace` and again with `--features capture,capture-test`):

| Test | What it pins |
|---|---|
| `mono_frames_match_the_allocating_mixdown` | the iterator and the public `mono_mixdown` agree for stereo, mono, trailing partial frame, `channels > len`, `channels == 0`, empty |
| `ring_path_matches_the_legacy_accumulate_exactly` | **workorder test 1**: 64 produced blocks (varied 1–700 frames, mono and stereo, partial trailing frames) → identical 22 634-sample sequence and identical `rms_bits` after every block vs a verbatim copy of the old `accumulate` |
| `a_full_ring_drops_the_incoming_samples_and_counts_them` | **workorder test 2**: 64-slot ring, consumer paused → exactly 2 496 samples dropped, counter and per-call return agree, nothing reached the buffer, every call < 50 ms deadline |
| `a_delayed_consumer_never_blocks_the_producer_and_loses_nothing` | **workorder stress**: 1.5 s of 48 kHz stereo in 192-frame blocks at the audio clock, consumer sleeping a deterministic-random 0–50 ms between drains → zero drops, exact 72 000-sample sequence, final RMS equal to a fresh meter over it, worst `write_block` well under the deadline |
| `consumer_shutdown_drains_the_ring_before_returning` | the stop handoff: a `shutdown` issued immediately after the push still delivers every sample and the final RMS |
| `ring_capacity_covers_one_second_at_the_session_rate` | 48 000 / 44 100 / 16 000 / 0 → never zero |
| `i16_and_u16_blocks_convert_like_the_old_callback` | the `Interleaved` conversions are the exact expressions the old closures used |

### Measured numbers (this VPS, 2 vCPU, `cargo test … -- --nocapture`)

```
parity: 64 blocks, 22634 samples compared bit-for-bit, final rms 0.496414
overflow: 10 blocks x 256 samples into a 64-slot ring, 2496 dropped, worst write_block 11.732µs
stress: 375 blocks x 192 frames (1.50 s of audio), worst write_block 42.47µs at block 163, total inside write_block 10.211953ms
```

Four earlier stress runs (the worst-block index moves run to run, i.e. the outliers are the OS descheduling the test thread, not the first delivery or the ring):

```
worst write_block 68.128µs at block 264, total inside write_block 10.627ms
worst write_block 213.401µs at block 112, total inside write_block 11.548ms
worst write_block 57.368µs at block 289, total inside write_block 10.359ms
worst write_block 48.952µs at block 312, total inside write_block 10.579ms
```

So: **≈ 0.7 % of the audio period spent inside the callback** (10.2–11.5 ms of CPU for 1.50 s of audio), worst single block 42–213 µs (the 213 µs outlier is a scheduler stall; the audio period is 4 ms), zero drops with a consumer that stalls up to 50 ms. The deadline assertion is 50 ms — ~230× the observed worst case, because it is a regression tripwire for "someone put a wait back in", not a benchmark.

## 8. Verification

```
$ cargo fmt --all --check                        # clean (no output, exit 0)
$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.91s     # exit 0, 0 warnings
$ cargo clippy -p suramak-asr --features capture,capture-test --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.41s     # exit 0 — the capture-gated code is clean too
$ cargo test --workspace
    … test result: ok. 497 passed; 0 failed …                               # 490 baseline + 7 new
$ cargo test -p suramak-asr --features capture,capture-test
    … test result: ok. 105 passed; 0 failed; 1 ignored …                     # lib (96 baseline + 7 new + 2 capture-only)
    … test result: ok. 6 passed; 0 failed; 7 ignored …                       # tests/pipeline.rs
```

Per-suite counts in the workspace run: `suramak-asr` lib 103 (96 + 7 new) is the only crate whose count changed; 497 total, 0 failed, 0 new warnings.

The `capture`-feature run includes `enumeration_returns_a_list` and `stop_time_stream_errors_are_quiet_but_live_ones_stay_loud` unchanged, and the pre-existing `mono_mixdown_averages_stereo_frames` still passes against the rewritten helper (which now delegates to the iterator).

Only one dependency was added: `rtrb 0.3.1` (`Cargo.toml`, `crates/suramak-asr/Cargo.toml`, `Cargo.lock` +7 lines, checksum `f3f94e84…`).

## 9. Honest boundary

* **Unit-tested here; not device-tested.** Every test drives the RT path directly (`RtProducer::write_block`) and the consumer (`drain`, `ConsumerThread`) with no cpal stream, because this VPS has no microphone. No real CoreAudio/ALSA/WASAPI callback ran, so the claim proven here is "the callback body cannot block, allocate or syscall, and its output is identical", not "this recording sounded clean".
* **The real check is the owner lane on macOS**: record ~10 minutes with a mic and with system audio, watch for dropouts/clicks, confirm the wav duration matches the wall-clock span, and confirm the new `dropped samples` warning never appears (if it does, the consumer thread is not being scheduled and the ring is genuinely too small for that machine). The M14 smoke script is the natural place.
* **The timing assertions are tripwires, not proofs.** The structural guarantee is the type-level one — `RtProducer` contains no lock, `write_block` calls no allocating API. The deadline test only catches a regression that introduces ≥ 50 ms of waiting.
* **Atomics on 32-bit.** `AtomicU64::fetch_add` is lock-free on x86_64/aarch64 (the shipped targets); on a 32-bit target it would fall back to a library lock. The workspace does not ship 32-bit targets.
* **One extra thread per source**, parked 5 ms at a time. Its CPU use is not measured here (it was ~0 in the stress test's wall-clock behaviour, but that is not a profiled number).
* **First-touch page faults** of the 192 KiB ring happen on the RT thread's first `push` (the allocation is at session start, but the pages are faulted in on first write). The harness did not show it (the worst block was never block 0 in any run); a real session pays it once, at the first callback, before the recording is interesting. If it ever shows up as a dropout, pre-faulting the ring in `start_one` is the fix.

## 10. Files changed

| File | Change |
|---|---|
| `crates/suramak-asr/src/capture_rt.rs` (new, 715 lines) | `Interleaved` + `MonoFrames` (allocation-free mixdown), `RtProducer::write_block`, `CaptureConsumer::drain`, `ConsumerThread`, `ring_capacity`, 7 unit tests |
| `crates/suramak-asr/src/capture.rs` | `mono_mixdown` delegates to `MonoFrames`; `Recorder` gains `consumer` + `dropped`; `stop` joins the consumer after the stream; `start_one` builds the ring and spawns the consumer; the three format arms call `RtProducer::write_block`; `accumulate` deleted; module docs |
| `crates/suramak-asr/src/lib.rs` | `pub(crate) mod capture_rt;` (public surface unchanged) |
| `crates/suramak-asr/Cargo.toml`, `Cargo.toml`, `Cargo.lock` | `rtrb = "=0.3.1"` |
| `docs/reports/M14b2-audio-rt.md` | this report |
