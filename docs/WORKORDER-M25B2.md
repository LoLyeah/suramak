# WORKORDER — M25B2: the phone rotates its recording too

M25B shipped the cap and pause-aligned parts, but its own report names the
gap: **Android's streaming capture still writes one WAV**, so a take longer
than 45 minutes is refused by name at the queue instead of being split. The
phone is the meeting device. This slice closes that.

## The mechanism (deliberately simple)

Rotate **after** resampling, on the 16 kHz mono stream — never inside the
resampler. Each part is then a slice of one continuous resampled stream, so
concatenation is sample-exact **by construction** and no resampler state has
to survive a cut.

1. A `PartRotator` sits between the resampler output and the WAV writer:
   `write(&[f32])` either appends to the open part or — when a rotation is
   pending and the current sample run has been silent for ≥ 600 ms — closes
   the part and opens the next one, then continues with the same slice.
2. "Rotation pending" starts when the part reaches `SESSION_CAP_MS`; a grace
   of ≤ 120 s applies exactly as on desktop, and if no silence arrives the cut
   happens at the least-energetic 50 ms window seen in the last 5 s, with the
   part flagged `cut_without_pause`.
3. Silence detection is an RMS gate over 50 ms windows on the 16 kHz stream
   (threshold: the recording's own noise floor × 3, floor-clamped, computed
   over the first 2 s; the value used is logged once per part).
4. Stop flushes the rotator; the parts land in the library as
   `Part N of M` on the same rules M25B already set. The foreground
   notification names the current part (`Recording · part 2`).
5. The 45-minute cap now **never refuses a phone recording** — it rotates.
   The `session_too_long` refusal stays for imports that cannot be split
   (e.g. a file with no decodable audio) and for a recording that would need
   more than 12 parts (one honest message: "this take is longer than 9 hours").

## Verification

- Host tests over a synthetic stream: a 47-minute take rotates exactly once at
  the first silence after 45 min; concatenated parts equal the input samples
  bit-for-bit; a take with no silence rotates at the least-energetic window and
  is flagged; a 3-part take names parts correctly; the rotator never emits a
  zero-length part; the 12-part ceiling refuses with its message.
- A test that a **very long silence before the cap** does not rotate early.
- The Android host path is NDK-gated (`cargo ndk … check` + clippy); the
  rotator itself is pure Rust and host-tested.
- fmt/clippy -D warnings; one full workspace run (baseline 1461).
- Report: append §"phone rotation" to `docs/reports/M25b-long-sessions.md` with
  the real part lengths from a synthetic 47-minute take and the peak RSS.

## Do NOT

- No chunked decoding; no change to the resampler; no new crates; no queue
  journal/resume changes; no CI edits; no git commit/push; no sudo.
