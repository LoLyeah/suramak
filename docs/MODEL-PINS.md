# MODEL-PINS — sha256 + byte sizes for the model manifest

Verified 2026-10-13 against `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/` (repo name corrected per PLAN-NOTES §1; all six files verified present). Paste these values verbatim into `crates/suramak-core/src/models.rs`.

| id | hf file | bytes | sha256 | role |
|---|---|---|---|---|
| tiny | ggml-tiny.bin | 77691713 | be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21 | CI + tests |
| base | ggml-base.bin | 147951465 | 60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe | optional/legacy |
| small | ggml-small.bin | 487601967 | 1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b | fast |
| turbo-q5-0 | ggml-large-v3-turbo-q5_0.bin | 574041195 | 394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2 | default (macOS) |
| turbo-q8-0 | ggml-large-v3-turbo-q8_0.bin | 874188075 | 317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1 | high quality |
| turbo | ggml-large-v3-turbo.bin | 1624555275 | 1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69 | accuracy |

Notes:
- Dev-machine cache (not part of the repo): `/home/ubuntu/.cache/suramak/models/` already holds all six files + `SHA256SUMS` — use it to test the download/verify path without re-downloading (e.g. point the store at that dir or copy/hardlink into a test data dir).
- The mirror rule (PLAN §2): same paths on `https://hf-mirror.com` via `SURAMAK_HF_MIRROR`.
