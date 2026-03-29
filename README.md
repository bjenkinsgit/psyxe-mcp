# psyXe MCP Server

An [MCP server](https://modelcontextprotocol.io/) that gives AI assistants access to your Apple Notes, Reminders, and Contacts — with optional BERT-powered semantic search.

All tools run locally via macOS-native APIs (AppleScript, EventKit, Contacts framework). No data leaves your Mac. No API keys required.

**New to MCP?** Check out the [FAQ](FAQ.md) for answers to common questions about what this is, whether it works with your setup, and how your data stays private.

## Video Tutorials

| | |
|---|---|
| [![Notes & Semantic Search](https://img.youtube.com/vi/Ha-O8jwoh9E/mqdefault.jpg)](https://youtu.be/Ha-O8jwoh9E) | [**Apple Notes & Semantic Search**](https://youtu.be/Ha-O8jwoh9E) — Connect your AI to Apple Notes with BERT-powered semantic search |
| [![Apple Contacts](https://img.youtube.com/vi/POjUyFA7wDI/mqdefault.jpg)](https://youtu.be/POjUyFA7wDI) | [**Apple Contacts**](https://youtu.be/POjUyFA7wDI) — Search, create, and manage contacts from your AI assistant |
| [![Apple Reminders](https://img.youtube.com/vi/AgcxHeTji1k/mqdefault.jpg)](https://youtu.be/AgcxHeTji1k) | [**Apple Reminders**](https://youtu.be/AgcxHeTji1k) — Full CRUD for reminders and lists via any MCP client |
| [![Access Control](https://img.youtube.com/vi/ApIAIc4MQUI/mqdefault.jpg)](https://youtu.be/ApIAIc4MQUI) | [**Access Control**](https://youtu.be/ApIAIc4MQUI) — Configure exactly which data your AI can access |

## What Can It Do?

| Category | Tools | Description |
|----------|-------|-------------|
| **Notes** | `search_notes`, `list_notes`, `get_note`, `open_note`, `notes_tags`, `notes_search_by_tag`, `notes_index` | Search, browse, and read Apple Notes |
| **Notes (Semantic)** | `notes_semantic_search`, `notes_smart_search`, `notes_rebuild_index`, `notes_index_stats` | BERT-powered semantic search across all your notes |
| **Reminders** | `list_reminder_lists`, `search_reminders`, `list_reminders`, `get_reminder`, `create_reminder`, `create_reminders_batch`, `complete_reminder`, `delete_reminder`, `edit_reminder`, `edit_reminders_batch`, `open_reminders`, `create_reminder_list`, `delete_reminder_list` | Full CRUD for Apple Reminders |
| **Contacts** | `list_contact_groups`, `search_contacts`, `list_contacts`, `get_contact`, `create_contact`, `edit_contact`, `delete_contact` | Search and manage Apple Contacts |
| **Files** | `file_search`, `read_file`, `write_file` | Search and read/write files in granted folders |

## Install

### Homebrew (recommended)

```bash
brew tap bjenkinsgit/tap
brew install psyxe-mcp
```

This installs everything — binary, Swift helpers, FFmpeg, and the BERT model. No compilation required.

### Build from Source

```bash
git clone https://github.com/bjenkinsgit/psyxe-mcp.git
cd psyxe-mcp
./build.sh
```

The build script handles everything automatically:
- Installs Homebrew, Rust, FFmpeg, and pkg-config if missing
- Builds the MCP server binary (Rust)
- Builds Swift helpers for Reminders and Contacts
- Copies helpers next to the binary
- Pre-downloads the BERT model (~90MB) so first search is instant

Build without semantic search (skips FFmpeg and BERT):

```bash
./build.sh --no-memvid
```

**Requirements:** macOS 12+ (Monterey or later). Xcode Command Line Tools will be prompted if not installed.

The binary and helpers are in `target/release/`. Use the full path when configuring your MCP client.

### Install Apple Shortcuts (optional)

Two shortcuts enable linking Reminders to file artifacts:

```bash
./install-shortcuts.sh
```

This opens each shortcut in Shortcuts.app for you to approve.

## Configure Your MCP Client

If you installed via Homebrew, the command is just `psyxe-mcp` (it's in your PATH). If you built from source, use the full path: `/Users/yourname/src/psyxe-mcp/target/release/psyxe-mcp`.

### Claude Code (CLI)

```bash
claude mcp add psyxe -- psyxe-mcp
```

Or edit `~/.claude/claude_mcp_config.json`:

```json
{
  "mcpServers": {
    "psyxe": {
      "command": "psyxe-mcp"
    }
  }
}
```

### Claude Desktop

Edit `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "psyxe": {
      "command": "psyxe-mcp"
    }
  }
}
```

### Cursor

Open Settings → MCP Servers → Add new server:

```json
{
  "psyxe": {
    "command": "psyxe-mcp"
  }
}
```

### Windsurf

Edit `~/.codeium/windsurf/mcp_config.json`:

```json
{
  "mcpServers": {
    "psyxe": {
      "command": "psyxe-mcp"
    }
  }
}
```

### OpenAI Codex CLI

Edit `~/.codex/config.toml`:

```toml
[mcp_servers.psyxe]
command = "psyxe-mcp"
```

> **Note:** If you built from source instead of using Homebrew, replace `psyxe-mcp` with the full path to the binary (e.g., `/Users/yourname/src/psyxe-mcp/target/release/psyxe-mcp`).

## Access Control

By default, the MCP server has full access to all your Notes, Reminders, Contacts, and files. To restrict what your AI can see, use the built-in access control CLI to create and manage `~/.psyxe/access.toml`.

No manual file editing is needed — the CLI creates the file with secure permissions (owner-only read/write) on first use.

### Quick Start

```bash
# 1. See what's available
psyxe-mcp access discover reminders
psyxe-mcp access discover notes

# 2. Grant access to only what the AI should see
psyxe-mcp access grant reminders "Work"
psyxe-mcp access grant notes "Projects"

# 3. Verify your restrictions
psyxe-mcp access list
```

Once any rule is set for a category, only explicitly granted resources are accessible — everything else in that category is denied.

### Discover What's Available

```bash
# See your reminder lists
psyxe-mcp access discover reminders

# See your contact groups
psyxe-mcp access discover contacts

# See your note folders
psyxe-mcp access discover notes

# See common file locations
psyxe-mcp access discover files
```

### Grant / Revoke Access

```bash
# Only allow access to specific reminder lists
psyxe-mcp access grant reminders "Work"
psyxe-mcp access grant reminders "Shopping" --rw    # read-write

# Only allow access to a specific contact group
psyxe-mcp access grant contacts "iCloud"

# Only allow access to specific note folders
psyxe-mcp access grant notes "Projects"

# Grant file access to a folder
psyxe-mcp access grant files "/Users/you/Documents" --rw

# Revoke access
psyxe-mcp access revoke reminders "Shopping"

# See current restrictions
psyxe-mcp access list

# Remove all restrictions (restore full access)
psyxe-mcp access reset
```

Access rules are stored in `~/.psyxe/access.toml` with owner-only permissions (`chmod 600`). The server refuses to load the config if it is group- or world-readable, preventing other processes from tampering with access rights.

## Semantic Search

When built with the `memvid` feature (enabled by default), the server includes BERT-powered semantic search for Apple Notes. This uses [memvid-rs](https://github.com/bjenkinsgit/memvid-rs) to encode your notes into a searchable vector index.

### First Use

The first time you (or your AI assistant) run a semantic search, the server will build an index of all your notes. This takes a few minutes depending on how many notes you have. Subsequent searches are instant.

```bash
# Or ask your AI assistant: "search my notes for machine learning concepts"
# It will automatically build the index on first use.
```

### How It Works

1. All notes are fetched from Notes.app
2. Each note is chunked and encoded with a BERT model (384-dimensional embeddings)
3. Chunks are stored as QR codes in a ProRes video file (compact, durable archive)
4. A vector index (HNSW) enables instant semantic similarity search
5. The index auto-detects when notes change and prompts for rebuild

### Try It Out

We've included [sample notes](examples/sample-notes.md) designed to showcase semantic search:

```bash
# Load 10 sample notes into Apple Notes
./examples/load-sample-notes.sh

# Then ask your AI assistant to rebuild the index and try queries like:
#   "retirement savings"  → finds Tax Strategy (never mentions "retirement")
#   "Italian cooking"     → finds Carbonara recipe (never says "Italian")
```

See [examples/sample-notes.md](examples/sample-notes.md) for the full list of demo queries.

### Choosing a Different BERT Model

The default model (`sentence-transformers/all-MiniLM-L6-v2`, 384 dimensions) balances speed and quality. You can swap in any HuggingFace BERT-family sentence-transformer model.

**Via environment variable:**
```bash
MEMVID_MODEL_NAME=BAAI/bge-small-en-v1.5 target/release/psyxe-mcp warmup
```

**Via config file** — create `memvid_config.toml` in the repo root or next to the binary:
```toml
[ml]
model_name = "BAAI/bge-small-en-v1.5"
```

After changing models, rebuild the index (ask your AI assistant or run warmup again).

**Popular alternatives:**

| Model | Dimensions | Trade-off |
|-------|-----------|-----------|
| `sentence-transformers/all-MiniLM-L6-v2` | 384 | Default. Fast, good quality |
| `BAAI/bge-small-en-v1.5` | 384 | Retrieval-optimized, slightly better for search |
| `sentence-transformers/all-mpnet-base-v2` | 768 | Higher quality, ~2x slower |
| `BAAI/bge-base-en-v1.5` | 768 | Best retrieval quality, needs query prefix |

For instruction-tuned models (like BGE), add query/document prefixes:
```toml
[ml]
model_name = "BAAI/bge-small-en-v1.5"
embedding_query_prefix = "Represent this sentence for searching relevant passages: "
embedding_document_prefix = ""
```

### Remote Embedding API

Use any OpenAI-compatible embedding endpoint instead of local BERT:

```toml
[ml]
embedding_provider = "remote"
```

Then set the endpoint via environment variables:

```bash
export EMBEDDING_API_URL="http://localhost:11434/v1/embeddings"  # Ollama
export EMBEDDING_API_MODEL="nomic-embed-text"
```

Works with OpenAI, Ollama, vLLM, LM Studio, or any OpenAI-compatible endpoint.

### Without Semantic Search

Build without memvid to skip the FFmpeg/BERT dependency entirely (no `brew install` needed):

```bash
cargo build --release --no-default-features
```

Notes tools still work — they fall back to AppleScript-based text search. All other tools (Reminders, Contacts, Files) are unaffected.

## macOS Permissions

On first use, macOS will prompt you to grant permission for:

- **Notes** — "osascript" wants to access Notes
- **Reminders** — "reminders-helper" wants to access Reminders
- **Contacts** — "contacts-helper" wants to access Contacts

Approve these in the dialog that appears. You can review/revoke them later in System Settings → Privacy & Security.

## Architecture

```
┌─────────────────┐     stdio (JSON-RPC)     ┌──────────────┐
│  Claude Code /   │ ◄─────────────────────► │  psyxe-mcp   │
│  Claude Desktop  │                          │  (MCP server) │
└─────────────────┘                          └──────┬───────┘
                                                     │
                                    ┌────────────────┼────────────────┐
                                    ▼                ▼                ▼
                             ┌────────────┐  ┌─────────────┐  ┌───────────┐
                             │ AppleScript │  │ Swift Helper│  │  memvid   │
                             │ (Notes)     │  │ (EventKit,  │  │ (BERT +   │
                             │             │  │  Contacts)  │  │  ProRes)  │
                             └──────┬──────┘  └──────┬──────┘  └─────┬─────┘
                                    ▼                ▼               ▼
                             ┌────────────┐  ┌─────────────┐  ┌───────────┐
                             │  Notes.app  │  │   EventKit  │  │ NoteStore │
                             │             │  │   Contacts  │  │  SQLite   │
                             └────────────┘  └─────────────┘  └───────────┘
```

The MCP server is a thin stdio bridge. All the real work happens in `psyxe-mcp-core`, the open-source library that provides direct access to macOS-native APIs.

## Configuration

### Semantic Search (memvid)

Place a `memvid_config.toml` in the working directory or next to the binary:

```toml
[chunking]
chunk_size = 700
overlap = 100

[ml]
device = "metal"    # auto | cpu | cuda | metal

[qr]
error_correction = "low"
version = 40

[video]
codec = "prores_ks"
prores_profile = "proxy"
library_log_level = "error"
```

See [memvid-rs](https://github.com/bjenkinsgit/memvid-rs) for all configuration options.

### Environment Variables

| Variable | Purpose |
|----------|---------|
| `RUST_LOG` | Log level (default: `info`). Logs go to stderr. |
| `TOOLS_JSON` | Path to custom tools.json (overrides embedded) |

## Troubleshooting

**"osascript is not allowed to send keystrokes"**
Grant Accessibility permission: System Settings → Privacy & Security → Accessibility

**"reminders-helper" wants to access your Reminders**
Click Allow. If you previously denied, re-enable in System Settings → Privacy & Security → Reminders.

**Semantic search is slow on first run**
The BERT model downloads on first use (~90MB). Subsequent runs use the cached model. Index building speed depends on note count — Metal GPU acceleration helps significantly on Apple Silicon.

**Notes search returns stale results**
The server monitors for changes and will prompt your AI assistant to rebuild the index. You can also force it: ask your assistant to "rebuild the notes index".

## License

Apache 2.0 — see [LICENSE](LICENSE).

This project uses FFmpeg at runtime for ProRes video encoding only (LGPL codec). No GPL-licensed codecs (x264, x265, etc.) are used.

## Credits

Built on [psyxe-mcp-core](crates/mcp-core/), powered by [memvid-rs](https://github.com/bjenkinsgit/memvid-rs) for semantic search.
