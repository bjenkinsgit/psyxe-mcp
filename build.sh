#!/usr/bin/env bash
# build.sh — Build everything for psyXe MCP server in one step.
#
# Usage:
#   ./build.sh              # Full build (Rust + Swift + BERT model)
#   ./build.sh --no-memvid  # Build without semantic search (no FFmpeg/BERT needed)

set -euo pipefail

bold()  { printf "\n\033[1m▸ %s\033[0m\n" "$*"; }
green() { printf "\033[32m  ✓ %s\033[0m\n" "$*"; }
red()   { printf "\033[31m  ✗ %s\033[0m\n" "$*"; }
warn()  { printf "\033[33m  ⚠ %s\033[0m\n" "$*"; }

ROOT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT_DIR"

FEATURES="--release"
NO_MEMVID=false

if [[ "${1:-}" == "--no-memvid" ]]; then
    NO_MEMVID=true
    FEATURES="--release --no-default-features"
fi

START=$SECONDS

# ── Prerequisites ───────────────────────────────────────────────────────────

bold "Checking and installing prerequisites"

# Homebrew
if ! command -v brew &>/dev/null; then
    bold "Installing Homebrew"
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    # Add brew to PATH for this session (Apple Silicon default location)
    eval "$(/opt/homebrew/bin/brew shellenv 2>/dev/null || /usr/local/bin/brew shellenv 2>/dev/null)"
fi
green "Homebrew: available"

# Rust toolchain
if ! command -v cargo &>/dev/null; then
    bold "Installing Rust toolchain"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi
green "Rust: $(rustc --version | awk '{print $2}')"

# Xcode Command Line Tools (provides Swift + system headers)
if ! command -v swift &>/dev/null; then
    bold "Installing Xcode Command Line Tools"
    xcode-select --install 2>/dev/null || true
    echo "  Waiting for Xcode CLT install to complete..."
    echo "  If a dialog appeared, click Install and wait, then re-run ./build.sh"
    exit 1
fi
green "Swift: available"

# FFmpeg + pkg-config (for semantic search)
if [[ "$NO_MEMVID" == false ]]; then
    BREW_NEEDED=()
    command -v ffmpeg &>/dev/null || BREW_NEEDED+=(ffmpeg)
    command -v pkg-config &>/dev/null || BREW_NEEDED+=(pkg-config)

    if [[ ${#BREW_NEEDED[@]} -gt 0 ]]; then
        bold "Installing ${BREW_NEEDED[*]}"
        brew install "${BREW_NEEDED[@]}"
    fi
    green "FFmpeg: $(ffmpeg -version 2>&1 | head -1 | awk '{print $3}')"
    green "pkg-config: available"
fi

# ── Rust build ──────────────────────────────────────────────────────────────

bold "Building MCP server (Rust)"
cargo build $FEATURES
green "psyxe-mcp built"

# ── Swift helpers ───────────────────────────────────────────────────────────

bold "Building Swift helpers"

for helper in reminders-helper contacts-helper; do
    if [[ -d "swift/$helper" ]]; then
        (cd "swift/$helper" && swift build -c release 2>&1 | sed "s/^/  [$helper] /")
        # Copy next to the MCP server binary
        cp "swift/$helper/.build/release/$helper" target/release/
        green "$helper built and copied to target/release/"
    else
        warn "swift/$helper/ not found, skipping"
    fi
done

# Also copy pdf-helper if present
if [[ -d "swift/pdf-helper" ]]; then
    (cd swift/pdf-helper && swift build -c release 2>&1 | sed "s/^/  [pdf-helper] /")
    cp swift/pdf-helper/.build/release/pdf-helper target/release/
    green "pdf-helper built and copied to target/release/"
fi

# ── BERT model pre-download ────────────────────────────────────────────────

if [[ "$NO_MEMVID" == false ]]; then
    bold "Pre-downloading BERT model"
    echo "  Model: sentence-transformers/all-MiniLM-L6-v2 (~90MB)"
    echo "  This avoids a delay on the first semantic search."

    # The model is downloaded by HuggingFace Hub to ~/.cache/huggingface/
    # We trigger the download by running a quick index stats check.
    # If no notes index exists yet, it'll just say "no index" and exit.
    if target/release/psyxe-mcp access list &>/dev/null 2>&1; then
        # Server binary works — use it to trigger model download
        # Run notes_index_stats via MCP protocol (initialize + call)
        INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"build","version":"0.1"}}}'
        STATS='{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"notes_index_stats","arguments":{}}}'
        printf '%s\n%s\n' "$INIT" "$STATS" | timeout 120 target/release/psyxe-mcp 2>/dev/null | tail -1 | head -c 200 > /dev/null 2>&1 || true
        green "BERT model cached (or already present)"
    else
        warn "Could not pre-download BERT model (server binary not working)"
        echo "  The model will download on first semantic search (~90MB)"
    fi
fi

# ── Summary ─────────────────────────────────────────────────────────────────

ELAPSED=$((SECONDS - START))
MINS=$((ELAPSED / 60))
SECS=$((ELAPSED % 60))

echo ""
bold "Build complete in ${MINS}m ${SECS}s"
echo ""
echo "  Binary:  $ROOT_DIR/target/release/psyxe-mcp"
echo "  Helpers: $ROOT_DIR/target/release/reminders-helper"
echo "           $ROOT_DIR/target/release/contacts-helper"
echo ""

if [[ "$NO_MEMVID" == true ]]; then
    echo "  Built without semantic search (--no-memvid)."
    echo "  Notes tools use AppleScript text search."
else
    echo "  Semantic search enabled (BERT model cached)."
fi

echo ""
echo "Next steps:"
echo "  1. Install shortcuts (optional): ./install-shortcuts.sh"
echo "  2. Configure your MCP client — see README.md"
echo "  3. Set access controls: target/release/psyxe-mcp access discover reminders"
