# MODEL-PINS — sha256 + byte sizes for the model manifest

Verified 2026-10-13 against `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/` (repo name corrected per PLAN-NOTES §1; all six files verified present). Paste these values verbatim into `crates/suramak-core/src/models.rs`.

| id | hf file | bytes | sha256 | role |
|---|---|---|---|---|
| tiny | ggml-tiny.bin | 77691713 | be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21 | CI + tests |
| base | ggml-base.bin | 147951465 | 60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe | optional/legacy |
| small | ggml-small.bin | 487601967 | 1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b | fast |
| turbo-q5-0 | ggml-large-v3-turbo-q5_0.bin | 574041195 | 394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2 | desktop default |
| turbo-q8-0 | ggml-large-v3-turbo-q8_0.bin | 874188075 | 317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1 | high quality |
| turbo | ggml-large-v3-turbo.bin | 1624555275 | 1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69 | accuracy |

Notes:
- Dev-machine cache (not part of the repo): `/home/ubuntu/.cache/suramak/models/` already holds all six files + `SHA256SUMS` — use it to test the download/verify path without re-downloading (e.g. point the store at that dir or copy/hardlink into a test data dir).
- The mirror rule (PLAN §2): same paths on `https://hf-mirror.com` via `SURAMAK_HF_MIRROR`.

## M10A — Parakeet TDT 0.6B v3 int8 (the fast engine)

Verified 2026-10-15 against
`https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/`
(the k2-fsa author's mirror of the `asr-models` release artifacts; the same files
ship inside `sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2` on
`github.com/k2-fsa/sherpa-onnx/releases`). Downloaded and hashed on the VPS; the
sha256 values also match the repository's LFS etags. Paste verbatim into
`crates/suramak-core/src/models.rs` (`PARAKEET.files`).

| id | hf file | bytes | sha256 | role |
|---|---|---|---|---|
| parakeet | encoder.int8.onnx | 652184281 | acfc2b4456377e15d04f0243af540b7fe7c992f8d898d751cf134c3a55fd2247 | fast engine (encoder) |
| parakeet | decoder.int8.onnx | 11845275 | 179e50c43d1a9de79c8a24149a2f9bac6eb5981823f2a2ed88d655b24248db4e | fast engine (decoder) |
| parakeet | joiner.int8.onnx | 6355277 | 3164c13fc2821009440d20fcb5fdc78bff28b4db2f8d0f0b329101719c0948b3 | fast engine (joiner) |
| parakeet | tokens.txt | 93939 | d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d | fast engine (token table) |

Notes:
- The model is a **directory** (`sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/`), not
  one file; every file carries its own pin. Total 670 478 772 bytes (~670 MB).
- Layout check: all four files must be present (`PARAKEET_REQUIRED_FILES`).
- Coverage: 25 European languages (bg, hr, cs, da, nl, en, et, fi, fr, de, el, hu,
  it, lv, lt, mt, pl, pt, ro, sk, sl, es, sv, ru, uk) — **no Indonesian**.
- Mirror rule: `SURAMAK_HF_MIRROR` swaps in `https://hf-mirror.com/…` (same paths).
- `scripts/pin-model-hashes.sh` prints this section too (third block).

## M16F — sherpa-onnx macOS bundle dylibs (`Contents/Frameworks`)

These are the native libraries that ship **inside** the macOS app when it is
built with the `parakeet` feature. They come from sherpa-rs-sys 0.6.8's pinned
download (the same files on every machine); no download happens at package
time — `scripts/bundle-sherpa-dylibs.sh` verifies the local copies against
`packaging/sherpa-dylibs.tsv` and fails on any mismatch.

Source archive (universal2: x86_64 + arm64):

`https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.12.9/sherpa-onnx-v1.12.9-osx-universal2-shared.tar.bz2`
— archive sha256 `e3f3596b84c65eef8939a8a45ced3d7c4ed857455ba5ae1148ce7a1c93ff9a3c`
(the sherpa-rs cache directory is keyed by that hash).

| file | bytes | sha256 | role |
|---|---|---|---|
| libsherpa-onnx-c-api.dylib | 8907832 | dbee23f44d7c66d858242127b9a7fffc99ee989726fc89386b64afb8214e78cb | sherpa-onnx 1.12.9 C API (bundled) |
| libonnxruntime.1.17.1.dylib | 50396248 | 4979386d1e8b42f4177fa110dbc9600b10846662fe555eac2f338ffef3ab6bfa | ONNX Runtime 1.17.1 (bundled dependency) |

Notes:
- Only these two ship. `libsherpa-onnx-cxx-api.dylib` is part of the archive but
  sherpa-rs never links it, so it is deliberately not bundled.
- The pins cover the **pristine upstream** bytes. Inside the bundle each copy
  gains one `LC_RPATH` (`@executable_path/../Frameworks`) via
  `install_name_tool`, so the bundled file's hash is expected to differ; the
  gate asserts the rpath and resolution instead of a post-patch hash (which is
  not stable across Xcode toolchains).
- Licenses: sherpa-onnx Apache-2.0, ONNX Runtime MIT — full texts in
  `packaging/THIRD-PARTY-NOTICES.txt` (shipped in `Contents/Resources`).
- `scripts/pin-model-hashes.sh` prints this table's lines as its fourth block.
