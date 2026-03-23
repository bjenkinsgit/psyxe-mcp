//! psyXe MCP Server
//!
//! Exposes Apple ecosystem tools (Notes, Reminders, Contacts) as MCP tools
//! over stdio transport. Claude Code launches this as a subprocess.
//!
//! All tools use macOS-native APIs (AppleScript, EventKit, Contacts framework)
//! and require no API keys or credentials.
//!
//! # Access Control
//!
//! Run `psyxe-mcp access` subcommands to manage which contacts, reminders,
//! and notes the MCP server can access. Config stored at `~/.psyxe/access.toml`.

mod access_cli;
mod access_config;
mod access_filter;
mod server;
mod tool_filter;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use psyxe_mcp_core::tools::ToolsConfig;
use rmcp::ServiceExt;

/// tools.json embedded at compile time — no external file needed for distribution.
const EMBEDDED_TOOLS_JSON: &str = include_str!("../../../tools.json");

// ── CLI Definition ──────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "psyxe-mcp",
    about = "Apple ecosystem MCP server for Claude Code",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage access control for contacts, reminders, and notes
    Access {
        #[command(subcommand)]
        action: AccessAction,
    },
    /// Download and cache the BERT model for semantic search
    Warmup,
}

#[derive(Subcommand)]
enum AccessAction {
    /// Show current access restrictions
    List,
    /// Discover available resources on this system
    Discover {
        /// Category: reminders, contacts, or notes
        category: String,
    },
    /// Grant access to a resource
    Grant {
        /// Category: reminders, contacts, or notes
        category: String,
        /// Resource name (list, group, or folder)
        name: String,
        /// Grant read-write access (default is read-only)
        #[arg(long = "rw")]
        writable: bool,
    },
    /// Revoke access to a resource
    Revoke {
        /// Category: reminders, contacts, or notes
        category: String,
        /// Resource name (list, group, or folder)
        name: String,
    },
    /// Remove all access restrictions (restore full access)
    Reset,
}

// ── Tools Config Loading ────────────────────────────────────────────────────

/// Load tools config, preferring external file (for development) then falling back
/// to the compile-time embedded version (for distribution).
fn load_tools_config() -> Result<ToolsConfig> {
    // 1. Explicit env var override (development / testing)
    if let Ok(path) = std::env::var("TOOLS_JSON") {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read TOOLS_JSON from {}", path))?;
        let config: ToolsConfig = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse TOOLS_JSON from {}", path))?;
        tracing::info!(path = %path, "Loaded tools.json from TOOLS_JSON env");
        return Ok(config);
    }

    // 2. Sibling to the binary (for bundled installs with custom tools.json)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("tools.json");
            if candidate.exists() {
                if let Ok(content) = std::fs::read_to_string(&candidate) {
                    if let Ok(config) = serde_json::from_str::<ToolsConfig>(&content) {
                        tracing::info!(path = %candidate.display(), "Loaded tools.json from disk");
                        return Ok(config);
                    }
                }
            }
        }
    }

    // 3. Embedded (compile-time) — the default for distributed binaries
    let config: ToolsConfig = serde_json::from_str(EMBEDDED_TOOLS_JSON)
        .context("Failed to parse embedded tools.json (this is a build error)")?;
    tracing::info!("Using embedded tools.json");
    Ok(config)
}

// ── Main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle subcommands (interactive CLI, not MCP mode)
    match cli.command {
        Some(Commands::Access { action }) => {
            return match action {
                AccessAction::List => access_cli::cmd_list(),
                AccessAction::Discover { category } => access_cli::cmd_discover(&category),
                AccessAction::Grant {
                    category,
                    name,
                    writable,
                } => access_cli::cmd_grant(&category, &name, writable),
                AccessAction::Revoke { category, name } => access_cli::cmd_revoke(&category, &name),
                AccessAction::Reset => access_cli::cmd_reset(),
            };
        }
        #[cfg(feature = "memvid")]
        Some(Commands::Warmup) => {
            println!("Downloading and caching BERT model for semantic search...");
            println!("Model: sentence-transformers/all-MiniLM-L6-v2");
            // Trigger the embedding pipeline which downloads the model on first use
            let result = tokio::task::spawn_blocking(|| {
                psyxe_mcp_core::memvid_notes::warmup_model()
            }).await;
            match result {
                Ok(Ok(())) => {
                    println!("BERT model cached successfully.");
                    return Ok(());
                }
                Ok(Err(e)) => {
                    eprintln!("Failed to download BERT model: {}", e);
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Warmup task failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        #[cfg(not(feature = "memvid"))]
        Some(Commands::Warmup) => {
            println!("Semantic search is not enabled (built without memvid feature).");
            return Ok(());
        }
        None => {} // Fall through to MCP server mode
    }

    // ── MCP Server Mode ─────────────────────────────────────────────────────

    // NOTE: No .env loading — this MCP server is credential-free by design.
    // All exposed tools use macOS-native APIs that need no API keys.
    // The SecretsStore (biometric) is intentionally not used here because
    // Touch ID would block the subprocess indefinitely if the user doesn't
    // see the prompt.

    // CRITICAL: All logging must go to stderr. stdout is the MCP JSON-RPC channel.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    // Load access control config
    let access_config = access_config::AccessConfig::load()?;
    let has_restrictions = access_config.reminders.is_some()
        || access_config.contacts.is_some()
        || access_config.notes.is_some();
    if has_restrictions {
        tracing::info!("Access restrictions active from ~/.psyxe/access.toml");
    }

    // Load tools config (embedded at compile time, with disk override for development)
    let config = load_tools_config()?;

    // Convert filtered tool definitions to MCP format (Notes, Reminders, Contacts only)
    let mcp_tools = tool_filter::build_mcp_tools(&config.tools);
    tracing::info!(count = mcp_tools.len(), "Registered MCP tools");

    // Create server and start stdio transport
    let server = server::McpServer::new(mcp_tools, None, access_config);
    let service = server
        .serve(rmcp::transport::stdio())
        .await
        .context("Failed to start MCP stdio transport")?;

    tracing::info!("MCP server running on stdio");

    // Spawn background staleness monitor for the Notes semantic index.
    #[cfg(feature = "memvid")]
    {
        let peer = service.peer().clone();
        tokio::spawn(server::staleness_monitor(peer));
    }

    service.waiting().await?;

    Ok(())
}
