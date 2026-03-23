#!/usr/bin/env bash
# debug-mcp.sh — Wrapper that runs psyxe-mcp with debug logging to a file.
# Point your MCP client at this script instead of the binary directly.
# Logs go to /tmp/psyxe-mcp-debug.log

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
export RUST_LOG=debug

# Dump environment for debugging
echo "=== ENV DUMP $(date) ===" >> /tmp/psyxe-mcp-debug.log
echo "HOME=$HOME" >> /tmp/psyxe-mcp-debug.log
echo "HF_HOME=${HF_HOME:-unset}" >> /tmp/psyxe-mcp-debug.log
echo "PWD=$PWD" >> /tmp/psyxe-mcp-debug.log
echo "PATH=$PATH" >> /tmp/psyxe-mcp-debug.log
ls -d ~/.cache/huggingface/hub/models--sentence-transformers--all-MiniLM-L6-v2/snapshots/*/config.json >> /tmp/psyxe-mcp-debug.log 2>&1
echo "=== END ENV ===" >> /tmp/psyxe-mcp-debug.log

exec "$SCRIPT_DIR/target/release/psyxe-mcp" "$@" 2>>/tmp/psyxe-mcp-debug.log
