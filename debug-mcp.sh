#!/usr/bin/env bash
# debug-mcp.sh — Wrapper that runs psyxe-mcp with debug logging to a file.
# Point your MCP client at this script instead of the binary directly.
# Logs go to /tmp/psyxe-mcp-debug.log

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
export RUST_LOG=debug

exec "$SCRIPT_DIR/target/release/psyxe-mcp" "$@" 2>>/tmp/psyxe-mcp-debug.log
