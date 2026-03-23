#!/usr/bin/env bash
# release.sh — Build a distributable tarball for Homebrew and direct download.
#
# Usage:
#   ./release.sh              # Build for current architecture
#   ./release.sh 0.1.0        # Build with specific version tag
#
# Output: dist/psyxe-mcp-<version>-<arch>.tar.gz
#
# The tarball contains:
#   bin/psyxe-mcp              — MCP server binary
#   bin/reminders-helper       — Swift EventKit helper
#   bin/contacts-helper        — Swift Contacts helper
#   share/psyxe-mcp/scripts/  — AppleScript files
#   share/psyxe-mcp/shortcuts/ — Apple Shortcut files
#   share/psyxe-mcp/tools.json — Tool definitions (embedded at compile time, but useful for reference)

set -euo pipefail

bold()  { printf "\033[1m%s\033[0m\n" "$*"; }
green() { printf "\033[32m  ✓ %s\033[0m\n" "$*"; }

ROOT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT_DIR"

VERSION="${1:-0.1.0}"
ARCH="$(uname -m)"  # arm64 or x86_64
PLATFORM="apple-darwin"
TARBALL_NAME="psyxe-mcp-${VERSION}-${ARCH}-${PLATFORM}"
DIST_DIR="$ROOT_DIR/dist"
STAGING="$DIST_DIR/$TARBALL_NAME"

bold "Building psyxe-mcp $VERSION for $ARCH-$PLATFORM"
echo ""

# ── Clean staging ───────────────────────────────────────────────────────────

rm -rf "$STAGING"
mkdir -p "$STAGING/bin" "$STAGING/share/psyxe-mcp"

# ── Build Rust binary ───────────────────────────────────────────────────────

bold "Building MCP server (release)"
cargo build --release -p psyxe-mcp
cp target/release/psyxe-mcp "$STAGING/bin/"
green "psyxe-mcp binary"

# ── Build Swift helpers ─────────────────────────────────────────────────────

bold "Building Swift helpers"
for helper in reminders-helper contacts-helper; do
    if [[ -d "swift/$helper" ]]; then
        (cd "swift/$helper" && swift build -c release 2>&1 | tail -1)
        cp "swift/$helper/.build/release/$helper" "$STAGING/bin/"
        green "$helper"
    fi
done

# ── Bundle supporting files ─────────────────────────────────────────────────

bold "Bundling supporting files"

# AppleScript files
cp -R scripts "$STAGING/share/psyxe-mcp/scripts"
green "scripts/ ($(ls scripts/*.applescript | wc -l | tr -d ' ') AppleScript files)"

# Shortcuts
if [[ -d shortcuts ]] && ls shortcuts/*.shortcut 1>/dev/null 2>&1; then
    cp -R shortcuts "$STAGING/share/psyxe-mcp/shortcuts"
    green "shortcuts/"
fi

# tools.json (reference copy — also embedded in binary at compile time)
cp tools.json "$STAGING/share/psyxe-mcp/"
green "tools.json"

# install-shortcuts.sh
if [[ -f install-shortcuts.sh ]]; then
    cp install-shortcuts.sh "$STAGING/share/psyxe-mcp/"
    chmod +x "$STAGING/share/psyxe-mcp/install-shortcuts.sh"
    green "install-shortcuts.sh"
fi

# ── Ad-hoc sign binaries ───────────────────────────────────────────────────

bold "Signing binaries"
for bin in "$STAGING/bin/"*; do
    codesign --force -s - "$bin" 2>/dev/null && green "$(basename "$bin")" || true
done

# ── Create tarball ──────────────────────────────────────────────────────────

bold "Creating tarball"
cd "$DIST_DIR"
tar czf "${TARBALL_NAME}.tar.gz" "$TARBALL_NAME"
SHA256=$(shasum -a 256 "${TARBALL_NAME}.tar.gz" | awk '{print $1}')

green "${TARBALL_NAME}.tar.gz"
echo ""

# ── Summary ─────────────────────────────────────────────────────────────────

TARBALL_PATH="$DIST_DIR/${TARBALL_NAME}.tar.gz"
TARBALL_SIZE=$(du -h "$TARBALL_PATH" | awk '{print $1}')

bold "Release build complete"
echo ""
echo "  Tarball: $TARBALL_PATH"
echo "  Size:    $TARBALL_SIZE"
echo "  SHA256:  $SHA256"
echo ""
echo "Next steps:"
echo "  1. Create a GitHub release:"
echo "     gh release create v${VERSION} '$TARBALL_PATH' --title 'v${VERSION}' --notes 'Initial release'"
echo ""
echo "  2. Update the Homebrew formula with:"
echo "     url \"https://github.com/bjenkinsgit/psyxe-mcp/releases/download/v${VERSION}/${TARBALL_NAME}.tar.gz\""
echo "     sha256 \"${SHA256}\""
