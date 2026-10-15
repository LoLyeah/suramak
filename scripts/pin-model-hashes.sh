#!/usr/bin/env bash
# pin-model-hashes.sh — downloads models + Core ML encoder packages and prints sha256
# lines for models.rs.  Current values are already pasted into
# crates/suramak-core/src/models.rs (ggml from docs/MODEL-PINS.md, encoder packages from
# the real HF artifacts).  Run this script only when re-pinning.
set -euo pipefail

BASE_URL="${SURAMAK_HF_MIRROR:-https://huggingface.co/ggerganov/whisper.cpp/resolve/main}"
DEST_DIR="${1:-/tmp/suramak-model-pins}"
mkdir -p "$DEST_DIR"

declare -A FILES=(
    ["tiny"]="ggml-tiny.bin"
    ["base"]="ggml-base.bin"
    ["small"]="ggml-small.bin"
    ["turbo-q5-0"]="ggml-large-v3-turbo-q5_0.bin"
    ["turbo-q8-0"]="ggml-large-v3-turbo-q8_0.bin"
    ["turbo"]="ggml-large-v3-turbo.bin"
)

# Official Core ML encoder packages (`ggml-<model>-encoder.mlmodelc.zip`). All three
# turbo decoders share one package: the encoder is external to the ggml file.
declare -A ENCODERS=(
    ["tiny"]="ggml-tiny-encoder.mlmodelc.zip"
    ["base"]="ggml-base-encoder.mlmodelc.zip"
    ["small"]="ggml-small-encoder.mlmodelc.zip"
    ["turbo-q5-0"]="ggml-large-v3-turbo-encoder.mlmodelc.zip"
    ["turbo-q8-0"]="ggml-large-v3-turbo-encoder.mlmodelc.zip"
    ["turbo"]="ggml-large-v3-turbo-encoder.mlmodelc.zip"
)

fetch() { # fetch <file-name> -> prints the local path
    local file="$1"
    local dest="$DEST_DIR/$file"
    if [[ ! -f "$dest" ]]; then
        echo "Downloading $file ..." >&2
        curl -L --progress-bar -o "$dest" "$BASE_URL/$file"
    fi
    printf '%s' "$dest"
}

echo "# Paste these lines into crates/suramak-core/src/models.rs (ggml decoder files)"
for id in tiny base small turbo-q5-0 turbo-q8-0 turbo; do
    file="${FILES[$id]}"
    dest=$(fetch "$file")
    hash=$(sha256sum "$dest" | awk '{print $1}')
    size=$(stat -c%s "$dest")
    echo "id=$id  file=$file  bytes=$size  sha256=$hash"
done

echo
echo "# Encoder packages — ModelInfo.encoder { zip_file, size_bytes, sha256 }"
for id in tiny base small turbo-q5-0 turbo-q8-0 turbo; do
    file="${ENCODERS[$id]}"
    dest=$(fetch "$file")
    hash=$(sha256sum "$dest" | awk '{print $1}')
    size=$(stat -c%s "$dest")
    echo "id=$id  encoder_zip=$file  bytes=$size  sha256=$hash"
done

# ---------------------------------------------------------------------------
# M10A: the Parakeet fast engine — one pinned file set per model directory.
# ---------------------------------------------------------------------------
PARAKEET_BASE_URL="${SURAMAK_PARAKEET_MIRROR:-https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main}"
PARAKEET_FILES=(
    "encoder.int8.onnx"
    "decoder.int8.onnx"
    "joiner.int8.onnx"
    "tokens.txt"
)

echo
echo "# Parakeet TDT v3 int8 — PARAKEET.files { name, size_bytes, sha256 }"
echo "# (the same artifacts as sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2)"
for file in "${PARAKEET_FILES[@]}"; do
    dest="$DEST_DIR/parakeet-$file"
    if [[ ! -f "$dest" ]]; then
        echo "Downloading $file ..." >&2
        curl -L --progress-bar -o "$dest" "$PARAKEET_BASE_URL/$file"
    fi
    hash=$(sha256sum "$dest" | awk '{print $1}')
    size=$(stat -c%s "$dest")
    echo "id=parakeet  file=$file  bytes=$size  sha256=$hash"
done
