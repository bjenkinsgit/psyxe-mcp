#!/usr/bin/env bash
# install-shortcuts.sh — Import psyXe Apple Shortcuts into Shortcuts.app.
#
# These shortcuts enable the MCP server to link Reminders to artifacts
# via shortcuts:// URIs.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SHORTCUTS_DIR="$SCRIPT_DIR/shortcuts"

if ! command -v shortcuts &>/dev/null; then
    echo "Error: 'shortcuts' CLI not found. Requires macOS 12+."
    exit 1
fi

if [[ ! -d "$SHORTCUTS_DIR" ]] || ! ls "$SHORTCUTS_DIR"/*.shortcut 1>/dev/null 2>&1; then
    echo "No shortcut files found in $SHORTCUTS_DIR"
    exit 1
fi

TMPDIR_CLEAN="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_CLEAN"' EXIT

echo "Installing psyXe shortcuts..."
for sc in "$SHORTCUTS_DIR"/*.shortcut; do
    # Skip signed variants — we handle them below
    [[ "$sc" == *.signed.shortcut ]] && continue

    name="$(basename "$sc" .shortcut)"

    # Prefer signed version if available (avoids "untrusted shortcut" warning)
    signed="${sc%.shortcut}.signed.shortcut"
    if [[ -f "$signed" ]]; then
        sc="$signed"
    fi

    # Copy to temp dir with the clean name so macOS imports it correctly
    clean_file="$TMPDIR_CLEAN/${name}.shortcut"
    cp "$sc" "$clean_file"

    echo "  Installing: $name"
    open "$clean_file"
    # Brief pause so Shortcuts.app can process each import dialog
    sleep 1
done

echo ""
echo "Done. Check Shortcuts.app to confirm the shortcuts were added."
echo "You may need to approve each one in the import dialog."
