#!/usr/bin/env bash
# pin-model-hashes.sh — downloads models and prints sha256 lines for models.rs
# Current values are already pasted into crates/suramak-core/src/models.rs
# from docs/MODEL-PINS.md.  Run this script only when re-pinning.
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

echo "# Paste these lines into crates/suramak-core/src/models.rs"
for id in tiny base small turbo-q5-0 turbo-q8-0 turbo; do
    file="${FILES[$id]}"
    dest="$DEST_DIR/$file"
    if [[ ! -f "$dest" ]]; then
        echo "Downloading $file ..." >&2
        curl -L --progress-bar -o "$dest" "$BASE_URL/$file"
    fi
    hash=$(sha256sum "$dest" | awk '{print $1}')
    size=$(stat -c%s "$dest")
    echo "id=$id  file=$file  bytes=$size  sha256=$hash"
done
