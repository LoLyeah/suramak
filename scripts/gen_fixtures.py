#!/usr/bin/env python3
"""Generate known-text speech fixtures (en + id) as 16k mono wav via edge-tts.

Fixtures are committed to the repo so transcription tests have known ground truth.
run: <hermes venv python> scripts/gen_fixtures.py   (needs edge-tts + ffmpeg)
"""
import asyncio
import pathlib
import subprocess

OUT = pathlib.Path("/home/ubuntu/suramak/fixtures/audio")
OUT.mkdir(parents=True, exist_ok=True)

SAMPLES = [
    (
        "en",
        "en-US-AriaNeural",
        "The quick brown fox jumps over the lazy dog. Suramak keeps every "
        "recording on this machine and never sends audio to the cloud.",
    ),
    (
        "id",
        "id-ID-GadisNeural",
        "Selamat pagi! Nama saya Suramak, aplikasi transkripsi lokal yang "
        "mendukung bahasa Indonesia dengan baik. Terima kasih sudah mencoba.",
    ),
]


async def main() -> None:
    import edge_tts

    for name, voice, text in SAMPLES:
        mp3 = OUT / f"{name}.mp3"
        wav = OUT / f"{name}.wav"
        await edge_tts.Communicate(text, voice).save(str(mp3))
        subprocess.run(
            ["ffmpeg", "-y", "-loglevel", "error", "-i", str(mp3),
             "-ar", "16000", "-ac", "1", str(wav)],
            check=True,
        )
        print(f"{name}: {wav} ({wav.stat().st_size} bytes, voice={voice})")

    manifest = OUT / "manifest.tsv"
    manifest.write_text("\n".join(f"{n}\t{t}" for n, _, t in SAMPLES) + "\n")
    print(f"manifest: {manifest}")


if __name__ == "__main__":
    asyncio.run(main())
