# psyXe MCP Server

An [MCP server](https://modelcontextprotocol.io/) that gives AI assistants access to your Apple Notes, Reminders, and Contacts — with optional BERT-powered semantic search.

All tools run locally via macOS-native APIs (AppleScript, EventKit, Contacts framework). No data leaves your Mac. No API keys required.

## What Can It Do?

| Category | Tools | Description |
|----------|-------|-------------|
| **Notes** | `search_notes`, `list_notes`, `get_note`, `open_note`, `notes_tags`, `notes_search_by_tag`, `notes_index` | Search, browse, and read Apple Notes |
| **Notes (Semantic)** | `notes_semantic_search`, `notes_smart_search`, `notes_rebuild_index`, `notes_index_stats` | BERT-powered semantic search across all your notes |
| **Reminders** | `list_reminder_lists`, `search_reminders`, `list_reminders`, `get_reminder`, `create_reminder`, `create_reminders_batch`, `complete_reminder`, `delete_reminder`, `edit_reminder`, `edit_reminders_batch`, `open_reminders`, `create_reminder_list`, `delete_reminder_list` | Full CRUD for Apple Reminders |
| **Contacts** | `list_contact_groups`, `search_contacts`, `list_contacts`, `get_contact`, `create_contact`, `edit_contact`, `delete_contact` | Search and manage Apple Contacts |
| **Files** | `file_search`, `read_file`, `write_file` | Search and read/write files in granted folders |

## Install

### Prerequisites

- macOS 12+ (Monterey or later)
- [Rust toolchain](https://rustup.rs/) (1.85+)
- FFmpeg and pkg-config (`brew install ffmpeg pkg-config`) — required for semantic search

### Build from Source

```bash
# Install dependencies (if not already present)
brew install ffmpeg pkg-config

git clone https://github.com/bjenkinsgit/psyxe-mcp.git
cd psyxe-mcp

# Build the MCP server binary
cargo build --release

# Build Swift helpers (faster Reminders + Contacts access)
cd swift/reminders-helper && swift build -c release && cd ../..
cd swift/contacts-helper && swift build -c release && cd ../..
```

The binary is at `target/release/psyxe-mcp`.

### Install Apple Shortcuts (optional)

Two shortcuts enable linking Reminders to file artifacts:

```bash
./install-shortcuts.sh
```

This opens each shortcut in Shortcuts.app for you to approve.

## Configure Your MCP Client

### Claude Code (CLI)

```bash
claude mcp add psyxe -- /absolute/path/to/psyxe-mcp
```

Or edit `~/.claude/claude_mcp_config.json`:

```json
{
  "mcpServers": {
    "psyxe": {
      "command": "/absolute/path/to/psyxe-mcp"
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
      "command": "/absolute/path/to/psyxe-mcp"
    }
  }
}
```

### Cursor

Open Settings → MCP Servers → Add new server:

```json
{
  "psyxe": {
    "command": "/absolute/path/to/psyxe-mcp"
  }
}
```

### Windsurf

Edit `~/.codeium/windsurf/mcp_config.json`:

```json
{
  "mcpServers": {
    "psyxe": {
      "command": "/absolute/path/to/psyxe-mcp"
    }
  }
}
```

### OpenAI Codex CLI

```bash
codex mcp add psyxe --command /absolute/path/to/psyxe-mcp
```

Or edit `~/.codex/config.toml`:

```toml
[mcp_servers.psyxe]
command = "/absolute/path/to/psyxe-mcp"
```

Replace `/absolute/path/to/psyxe-mcp` with the actual path to the binary, e.g., `/Users/yourname/src/psyxe-mcp/target/release/psyxe-mcp`.

## Access Control

By default, the MCP server has full access to all your Notes, Reminders, Contacts, and files. You can restrict this with the built-in access control CLI.

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

Access rules are stored in `~/.psyxe/access.toml`. Once any rule is set for a category, only explicitly granted resources are accessible — everything else is denied.

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

## Credits

Built on [psyxe-mcp-core](crates/mcp-core/), powered by [memvid-rs](https://github.com/bjenkinsgit/memvid-rs) for semantic search.
