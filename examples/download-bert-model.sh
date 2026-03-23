#!/usr/bin/env bash
# download-bert-model.sh — Pre-download the BERT model used by semantic search.
#
# Downloads sentence-transformers/all-MiniLM-L6-v2 from HuggingFace Hub
# to the standard cache directory (~/.cache/huggingface/). This avoids
# a ~90MB download delay on the first semantic search.

set -euo pipefail

MODEL="sentence-transformers/all-MiniLM-L6-v2"
CACHE_DIR="${HF_HOME:-$HOME/.cache/huggingface}/hub"
MODEL_DIR="$CACHE_DIR/models--sentence-transformers--all-MiniLM-L6-v2"

if [[ -d "$MODEL_DIR" ]] && [[ -n "$(ls -A "$MODEL_DIR/snapshots/" 2>/dev/null)" ]]; then
    echo "BERT model already cached at $MODEL_DIR"
    exit 0
fi

echo "Downloading BERT model: $MODEL (~90MB)"
echo "Cache location: $CACHE_DIR"
echo ""

# Download the model files using curl against HuggingFace Hub API
# These are the files memvid-rs/candle needs for inference
BASE_URL="https://huggingface.co/$MODEL/resolve/main"
FILES=(
    "config.json"
    "tokenizer.json"
    "tokenizer_config.json"
    "model.safetensors"
)

mkdir -p "$MODEL_DIR/snapshots/main"

for file in "${FILES[@]}"; do
    dest="$MODEL_DIR/snapshots/main/$file"
    if [[ -f "$dest" ]]; then
        echo "  Already have: $file"
    else
        echo "  Downloading: $file"
        curl -sL "$BASE_URL/$file" -o "$dest"
    fi
done

echo ""
echo "BERT model cached successfully."
