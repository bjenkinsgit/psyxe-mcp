# Frequently Asked Questions

## General

### What is psyXe MCP?

psyXe MCP is a free, open-source program that lets your AI assistant (like Claude, Cursor, or Windsurf) read and work with your Apple Notes, Reminders, and Contacts. Instead of copy-pasting information into your AI chat, the AI can search and access your data directly.

### What does MCP stand for?

MCP stands for [Model Context Protocol](https://modelcontextprotocol.io/). It's a standard way for AI applications to connect to external tools and data sources. Think of it like a USB port — it's a universal connector that lets different AI apps plug into the same tools.

### Is this an AI model?

No. psyXe MCP doesn't include an AI model. It's a bridge between your AI app and your Apple data. You bring your own AI — whether that's Claude, ChatGPT through Cursor, a local model, or anything else that supports MCP.

### Do I need to be a programmer to use this?

Not if you install via Homebrew (a popular Mac package manager). The install is two commands in Terminal, and configuring your AI app is a small copy-paste into a settings file. The [video tutorials](README.md#video-tutorials) walk through the entire process step by step.

### Is it really free?

Yes. psyXe MCP is open-source under the Apache 2.0 license. No trials, no feature limits, no account required.

---

## Privacy & Security

### Does my data leave my Mac?

No. psyXe MCP runs entirely on your Mac using macOS-native APIs (the same ones Apple's own apps use). Your Notes, Reminders, and Contacts are accessed locally. Nothing is sent to any external server by psyXe MCP.

Your AI app may send data to its own cloud service (e.g., Claude Desktop sends your conversation to Anthropic's servers), but that's between you and your AI provider — psyXe MCP itself never phones home.

### Can the AI delete or modify my data?

For Notes, the AI can only read and search — it cannot create, edit, or delete notes.

For Reminders and Contacts, the AI can create, edit, and delete items. If that concerns you, use the built-in [access control](#can-i-limit-what-the-ai-can-access) to restrict which reminder lists, contact groups, or folders the AI can see.

### Can I limit what the AI can access?

Yes. psyXe MCP includes a built-in access control system. You can restrict access to specific reminder lists, contact groups, note folders, or file folders. Once you set a restriction for a category, only the items you've explicitly granted are visible — everything else is hidden from the AI.

See the [Access Control video tutorial](https://youtu.be/ApIAIc4MQUI) or the [README](README.md#access-control) for details.

---

## Compatibility

### What Mac do I need?

Any Mac running macOS 12 (Monterey) or later. Both Intel and Apple Silicon (M1/M2/M3/M4) Macs are supported. Semantic search runs faster on Apple Silicon thanks to Metal GPU acceleration, but it works on Intel too.

### Which AI apps work with this?

Any AI application that supports MCP. Popular ones include:

- **Claude Code** (Anthropic's CLI)
- **Claude Desktop** (Anthropic's desktop app)
- **Cursor** (AI code editor)
- **Windsurf** (AI code editor)
- **OpenAI Codex CLI**

The list keeps growing as more apps adopt the MCP standard. If your AI app has an "MCP servers" section in its settings, it should work.

### Does it work with ChatGPT?

Not directly — the ChatGPT app and website don't support MCP yet. However, if you use ChatGPT's models through an MCP-compatible app like Cursor, it works.

### Does it work with local/offline AI models?

Yes. If you run a local model (via Ollama, LM Studio, etc.) through an MCP-compatible client, psyXe MCP works with it. Since both the AI and psyXe MCP run on your Mac, your data never leaves your machine at all.

### Does it work on Windows or Linux?

No. psyXe MCP uses macOS-native APIs to access Apple Notes, Reminders, and Contacts. These apps and APIs only exist on macOS.

---

## Installation

### How do I install it?

The easiest way is via Homebrew:

```bash
brew tap bjenkinsgit/tap
brew install psyxe-mcp
```

That's it. No compilation, no dependencies to manage. See the [README](README.md#install) for other options.

### I don't have Homebrew. Is that a problem?

The install script can set up Homebrew for you automatically if you choose to build from source. Alternatively, you can [install Homebrew](https://brew.sh/) first (it's a one-line command) and then install psyXe MCP.

### macOS is asking me to allow access to Notes/Reminders/Contacts. Is that normal?

Yes. The first time psyXe MCP accesses each app, macOS will show a permission dialog. You need to click "Allow" for each one. This is standard macOS security — the same thing happens when any app tries to access your personal data. You can review or revoke these permissions later in System Settings > Privacy & Security.

### The build failed. What should I do?

If building from source, make sure you have Xcode Command Line Tools installed (`xcode-select --install`). The build script will prompt you if they're missing, but installing them beforehand avoids interruptions.

If you're still stuck, [open an issue](https://github.com/bjenkinsgit/psyxe-mcp/issues) with the error output and we'll help.

---

## Semantic Search

### What is semantic search?

Regular search finds notes that contain your exact words. Semantic search finds notes that are *about* what you mean, even if they use different words. For example, searching for "retirement savings" can find a note about tax-advantaged investment strategies — even if the note never mentions "retirement."

### How does it work?

psyXe MCP uses a BERT model (a type of AI that understands language) to convert your notes and your search query into numerical representations called embeddings. Notes whose embeddings are close to your query's embedding are returned as results. This all happens locally on your Mac.

### Do I need an internet connection for semantic search?

Only the first time, to download the BERT model (~90MB). After that, everything runs offline.

### How long does the initial index build take?

It depends on how many notes you have. A few hundred notes typically takes 1-2 minutes. The index is rebuilt automatically when the server detects your notes have changed.

### Can I skip semantic search and just use regular search?

Yes. If you install with `--no-memvid` or build with `--no-default-features`, semantic search is excluded entirely. All other features (Notes text search, Reminders, Contacts, Files) work normally.

---

## Usage

### How do I actually use it?

Once installed and configured, you just talk to your AI assistant naturally. For example:

- "Search my notes for ideas about home renovation"
- "Show me my reminders that are due this week"
- "Find the phone number for Dr. Smith in my contacts"
- "Create a reminder to call the dentist tomorrow at 9am"

The AI will automatically use psyXe MCP's tools when it needs to access your Apple data.

### Do I need to learn any special commands?

No. You interact with your AI assistant the same way you always do. The AI decides when to use psyXe MCP's tools based on what you ask.

### Can I use it while my AI app is running?

psyXe MCP starts automatically when your AI app needs it and stays running in the background. You don't need to launch or manage it separately.

### I added new notes/reminders but the AI can't find them. Why?

For Notes with semantic search: the index may need to be rebuilt. Ask your AI assistant to "rebuild the notes index." For text-based search, new notes appear immediately.

For Reminders and Contacts: changes should appear immediately since these are queried live.
