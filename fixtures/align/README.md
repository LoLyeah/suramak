# fixtures/align — word-timing measurement fixtures (M15.6)

These two files exist for one purpose: a **ground-truth** timeline to measure
the word alignment that whisper's token timestamps produce. They are not
product audio and no test other than `crates/suramak-asr/tests/alignment.rs`
reads them.

## How they were made (once, 2026-11-10, ffmpeg 8.0.1 + libflite)

Each word was synthesized **as its own clip** with ffmpeg's `flite` source
(voice `slt`), trimmed to the first/last sample above −40 dBFS, and the clips
were joined with exactly 300 ms of silence (250 ms lead-in, 400 ms tail). The
word boundaries are therefore sample-accurate by construction — no human
annotation and no forced aligner was involved, so the reference cannot inherit
whisper's own bias.

The generator is a throwaway (it lives in the VPS scratch tree, not the repo);
the commands it ran per word were exactly:

```sh
ffmpeg -hide_banner -loglevel error -y \
  -f lavfi -i "flite=text='<word>':voice=slt" \
  -ar 16000 -ac 1 -c:a pcm_s16le <word>.wav
# then: trim to |sample| > 0.01 (-40 dBFS), concatenate s16le frames, write wav
```

| file | words | duration |
|---|---|---|
| `en-words.wav` | the quick brown fox jumps over the lazy dog (9) | 6.315 s |
| `id-words.wav` | selamat pagi nama saya suramak aplikasi lokal (7) | 6.254 s |

`*-words.tsv` is the construction timeline (`idx`, `word`, `start_ms`,
`end_ms`); it is the reference, not a transcription.

## Hashes

`SHA256SUMS` pins all four files; verify with `sha256sum -c SHA256SUMS`.

## What the measurement showed (tiny model, CPU, release build)

Run with:

```sh
SURAMAK_REQUIRE_MODEL=1 cargo test -p suramak-asr --release --test alignment -- --ignored --nocapture
```

| fixture | median abs start error | p90 | max | within 100 ms |
|---|---|---|---|---|
| en-words | 11 ms | 45 ms | 45 ms | 100 % |
| id-words | 112 ms | 361 ms | 361 ms | 43 % |

The numbers are an **upper bound on achievable accuracy**, not a typical case:
every word here is separated by a real silence, so whisper's forced timestamp
tokens land on those silences. Contiguous speech interpolates the gaps (see
`docs/reports/M15d-alignment.md`), which is why the product calls these
estimates and keeps segment seeking as the fallback.
