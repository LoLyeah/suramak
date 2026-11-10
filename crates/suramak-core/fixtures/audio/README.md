# M15.2 audio fixtures — Opus container parity (EN + Indonesian)

Committed, small, **generated once** with the VPS's system ffmpeg 8.0.1. ffmpeg is
fixture tooling here — it is **not** a product dependency and never runs in
Suramak. Every file below is data; the decode path under test is the shipped
`symphonia` + `symphonia-adapter-libopus` (bundled libopus) code in
`crates/suramak-core/src/decode.rs`.

## Source speech

Both tracks are 4 s trims of the repo's existing TTS samples (16 kHz mono):

| language | source | text |
|---|---|---|
| EN | `fixtures/audio/en.wav` | "The quick brown fox jumps over the lazy dog. Suramak keeps every recording on this machine and never sends audio to the cloud." |
| ID | `fixtures/audio/id.wav` | "Selamat pagi! Nama saya Suramak, aplikasi transkripsi lokal yang mendukung bahasa Indonesia dengan baik. Terima kasih sudah mencoba." |

## Generator commands

Run from the repository root. The Opus packets are encoded **once** per language
and remuxed with `-c:a copy` into WebM and Matroska: the three containers carry
byte-identical Opus packets, so the parity test can demand bit-identical PCM
instead of "sounds the same".

```sh
F=crates/suramak-core/fixtures/audio

# 1. Encode once per language (Ogg-Opus, 48 kHz mono, 24 kbit/s, voice profile).
ffmpeg -y -i fixtures/audio/en.wav -t 4 -af aresample=48000 -ac 1 \
  -c:a libopus -b:a 24k -application voip -map_metadata -1 "$F/speech-en.opus"
ffmpeg -y -i fixtures/audio/id.wav -t 4 -af aresample=48000 -ac 1 \
  -c:a libopus -b:a 24k -application voip -map_metadata -1 "$F/speech-id.opus"

# 2. Remux the SAME packets into the two other containers (no re-encode).
for L in en id; do
  ffmpeg -y -i "$F/speech-$L.opus" -c:a copy -map_metadata -1 "$F/speech-$L.webm"
  ffmpeg -y -i "$F/speech-$L.opus" -c:a copy -map_metadata -1 "$F/speech-$L.mkv"
done

# 3. Truncated variants: the first 60 % of each complete file's bytes.
for L in en id; do
  for E in opus webm mkv; do
    head -c $(( $(stat -c%s "$F/speech-$L.$E") * 6 / 10 )) \
      "$F/speech-$L.$E" > "$F/truncated-$L.$E"
  done
done

# 4. A malformed container: text in a .webm name (probe must refuse it cleanly).
printf 'This is not a media container; it is a malformed fixture.\n' > "$F/malformed.webm"
```

The truncation cut is deliberately inside the packet stream (60 %), not at a
container boundary: a complete file must still decode, while a prefix must fail
with `SuramakError::Truncated` rather than import ~2.4 s of a 4 s voice note.

## sha256 pins (these committed bytes)

Verify with `cd crates/suramak-core/fixtures/audio && sha256sum -c SHA256SUMS`
(pins repeated here for review):

```
c227cb93d53e932121c77fd1cff3b968bc6734840668f4673ac51698904eca4b  speech-en.opus
d432184a819090be153fd1cc1c1f880bc6290ed8c77a3bfac7e29bd2ed2b4a44  speech-en.webm
2fcd02d664f672eaf77eebfc5dfb1171a5b72c5e9cb542e7e0e970c595eba0a6  speech-en.mkv
2ddfd6fae56fc8fe0eb091e986b031032327b998df27899628e57aee56342c73  speech-id.opus
bd9a626e174a0272a7329945e52cecf6f752e71f127cc64b9424b83b2c601530  speech-id.webm
7f58b4e2f0a1ca50e21b18f24a099b94502758945f9e10954426e781834e883e  speech-id.mkv
d4452f3c5f18a81ed8c730764f35ae0671b58b77e4aee4f097a0e59ab7c0ef1b  truncated-en.opus
ce0b46cd626ffc9a3a1e0a7173fa440fd1ef36987365ce77168392d3356f9f0f  truncated-en.webm
6688f0336e6210733975f8dc6beebd28e6a13a75741b13c375fed2951e90692f  truncated-en.mkv
d5713d45d00d4d93752cc909d0ed8056db86cfa5a9de3b06dd6098d1dbe5bde0  truncated-id.opus
b390f57a106e6755f46c7f76e8b13722e7205b891098e2be455e6f86b9c1d67f  truncated-id.webm
fa1c9b92d627e0b0b88bcf3c807ea44f6876ee30f7fa86195c81c28e02740847  truncated-id.mkv
01488a0ddb825b10323b16d030156c3b92b1f46db199e15adb5a228e7edcb9f0  malformed.webm
```

## Why these shapes

- **Opus always decodes at 48 kHz.** The Ogg-Opus mapping, WebM and Matroska all
  declare 48 kHz here; "varying input rates" cannot be exercised through Opus
  (libopus resamples any encode input to 48 kHz by definition). Rate conversion
  is covered by the WAV/MP3/AAC fixtures and the resampler tests elsewhere.
- **Same packets, three containers** isolates container parsing from codec
  behaviour: any PCM difference is a demuxer bug, not encoder noise.
- **60 % prefixes** keep the header (so the container probes successfully and
  declares ~4 s) while removing most packets — exactly the half-copied-file case
  users hit with interrupted downloads and share-sheet copies.
