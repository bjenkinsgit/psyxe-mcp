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

echo "Installing psyXe shortcuts..."
for sc in "$SHORTCUTS_DIR"/*.shortcut; do
    name="$(basename "$sc" .shortcut)"
    # Prefer signed version if available
    signed="${sc%.shortcut}.signed.shortcut"
    if [[ -f "$signed" ]]; then
        sc="$signed"
        name="$(basename "$sc" .signed.shortcut)"
    fi

    echo "  Installing: $name"
    # Open the shortcut file — macOS will prompt to add it to Shortcuts.app
    open "$sc"
done

echo ""
echo "Done. Check Shortcuts.app to confirm the shortcuts were added."
echo "You may need to approve each one in the import dialog."
