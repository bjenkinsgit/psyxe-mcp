//! CLI subcommands for managing MCP access control.
//!
//! These commands run interactively in the terminal — they are never
//! exposed via the MCP protocol.

use crate::access_config::{
    AccessConfig, ContactAccess, FileAccess, NoteAccess, ReminderAccess,
};
use anyhow::{bail, Context, Result};
use psyxe_mcp_core::tool_dispatch;
use serde_json::{json, Value};

// ── Discovery ───────────────────────────────────────────────────────────────

/// Discover available reminder lists by querying the system.
fn discover_reminder_lists() -> Result<Vec<String>> {
    let (ok, output) = tool_dispatch::execute_tool("list_reminder_lists", &json!({}), None);
    if !ok {
        bail!("Failed to list reminder lists: {}", output);
    }
    let parsed: Value = serde_json::from_str(&output)?;
    let mut lists = Vec::new();
    if let Some(arr) = parsed.get("lists").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                let count = item.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
                lists.push(format!("{} ({})", name, count));
            }
        }
    }
    Ok(lists)
}

/// Discover available contact groups/containers by querying the system.
fn discover_contact_groups() -> Result<Vec<String>> {
    let (ok, output) = tool_dispatch::execute_tool("list_contact_groups", &json!({}), None);
    if !ok {
        bail!("Failed to list contact groups: {}", output);
    }
    let parsed: Value = serde_json::from_str(&output)?;
    let mut groups = Vec::new();
    if let Some(arr) = parsed.get("sources").and_then(|v| v.as_array()) {
        for item in arr {
            let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("?");
            let kind = item.get("type").and_then(|t| t.as_str()).unwrap_or("?");
            let count = item.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
            groups.push(format!("{} [{}] ({})", name, kind, count));
        }
    }
    Ok(groups)
}

/// Discover available Notes folders via a lightweight AppleScript call.
fn discover_note_folders() -> Result<Vec<String>> {
    let output = std::process::Command::new("osascript")
        .args(["-e", r#"tell application "Notes" to get name of every folder"#])
        .output()
        .context("Failed to run osascript for Notes folders")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to list Notes folders: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut folders: Vec<String> = stdout
        .trim()
        .split(", ")
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    folders.sort();
    Ok(folders)
}

// ── CLI Command Handlers ────────────────────────────────────────────────────

/// Show current access configuration.
pub fn cmd_list() -> Result<()> {
    let config = AccessConfig::load()?;
    let path = crate::access_config::config_path();

    if !path.exists() {
        println!("No access restrictions configured.");
        println!("All contacts, reminders, notes, and files are accessible.");
        println!();
        println!("Run 'psyxe-mcp access discover' to see available resources.");
        return Ok(());
    }

    println!("Access config: {}", path.display());
    println!();

    // Reminders
    match &config.reminders {
        None => println!("Reminders:  unrestricted (full access)"),
        Some(r) if r.allowed_lists.is_empty() => println!("Reminders:  DENIED (no lists allowed)"),
        Some(r) => {
            println!("Reminders:");
            for list in &r.allowed_lists {
                let rw = if r.writable_lists.iter().any(|w| w.eq_ignore_ascii_case(list)) {
                    "read-write"
                } else {
                    "read-only"
                };
                println!("  {} [{}]", list, rw);
            }
        }
    }

    // Contacts
    match &config.contacts {
        None => println!("Contacts:   unrestricted (full access)"),
        Some(c) if c.allowed_groups.is_empty() => println!("Contacts:   DENIED (no groups allowed)"),
        Some(c) => {
            println!("Contacts:");
            for group in &c.allowed_groups {
                let rw = if c.writable_groups.iter().any(|w| w.eq_ignore_ascii_case(group)) {
                    "read-write"
                } else {
                    "read-only"
                };
                println!("  {} [{}]", group, rw);
            }
        }
    }

    // Notes
    match &config.notes {
        None => println!("Notes:      unrestricted (full access)"),
        Some(n) if n.allowed_folders.is_empty() => println!("Notes:      DENIED (no folders allowed)"),
        Some(n) => {
            println!("Notes:");
            for folder in &n.allowed_folders {
                let rw = if n.writable_folders.iter().any(|w| w.eq_ignore_ascii_case(folder)) {
                    "read-write"
                } else {
                    "read-only"
                };
                println!("  {} [{}]", folder, rw);
            }
        }
    }

    // Files
    match &config.files {
        None => println!("Files:      unrestricted (full access)"),
        Some(f) if f.allowed_folders.is_empty() => println!("Files:      DENIED (no folders allowed)"),
        Some(f) => {
            println!("Files:");
            for folder in &f.allowed_folders {
                let rw = if f.writable_folders.iter().any(|w| w == folder) {
                    "read-write"
                } else {
                    "read-only"
                };
                println!("  {} [{}]", folder, rw);
            }
        }
    }

    Ok(())
}

/// Discover available resources on this system.
pub fn cmd_discover(category: &str) -> Result<()> {
    println!("Note: macOS may show a permission dialog on first access.");
    println!();

    match category {
        "reminders" => {
            println!("Discovering reminder lists...");
            let lists = discover_reminder_lists()?;
            if lists.is_empty() {
                println!("  (no reminder lists found)");
            } else {
                for list in &lists {
                    println!("  {}", list);
                }
            }
            println!();
            println!("Grant access with: psyxe-mcp access grant reminders \"<list name>\"");
        }
        "contacts" => {
            println!("Discovering contact groups...");
            let groups = discover_contact_groups()?;
            if groups.is_empty() {
                println!("  (no contact groups found)");
            } else {
                for group in &groups {
                    println!("  {}", group);
                }
            }
            println!();
            println!("Grant access with: psyxe-mcp access grant contacts \"<group name>\"");
        }
        "notes" => {
            println!("Discovering note folders...");
            let folders = discover_note_folders()?;
            if folders.is_empty() {
                println!("  (no notes found)");
            } else {
                for folder in &folders {
                    println!("  {}", folder);
                }
            }
            println!();
            println!("Grant access with: psyxe-mcp access grant notes \"<folder name>\"");
        }
        "files" => {
            println!("Common file locations:");
            let home = dirs::home_dir().unwrap_or_default();
            let candidates = [
                ("Desktop", home.join("Desktop")),
                ("Documents", home.join("Documents")),
                ("Downloads", home.join("Downloads")),
                ("iCloud Drive", home.join("Library/Mobile Documents/com~apple~CloudDocs")),
            ];
            for (label, path) in &candidates {
                if path.exists() {
                    println!("  {} → {}", label, path.display());
                }
            }
            println!();
            println!("Grant access with: psyxe-mcp access grant files \"/absolute/path/to/folder\"");
            println!("  Add --rw for write access.");
        }
        other => bail!(
            "Unknown category: {}. Use: reminders, contacts, notes, files",
            other
        ),
    }
    Ok(())
}

/// Grant access to a resource.
pub fn cmd_grant(category: &str, name: &str, writable: bool) -> Result<()> {
    let mut config = AccessConfig::load()?;
    let mode = if writable { "read-write" } else { "read-only" };

    match category {
        "reminders" => {
            let access = config.reminders.get_or_insert_with(|| ReminderAccess {
                allowed_lists: Vec::new(),
                writable_lists: Vec::new(),
            });
            if !access.allowed_lists.iter().any(|l| l.eq_ignore_ascii_case(name)) {
                access.allowed_lists.push(name.to_string());
            }
            if writable && !access.writable_lists.iter().any(|l| l.eq_ignore_ascii_case(name)) {
                access.writable_lists.push(name.to_string());
            }
            if !writable {
                access.writable_lists.retain(|l| !l.eq_ignore_ascii_case(name));
            }
        }
        "contacts" => {
            let access = config.contacts.get_or_insert_with(|| ContactAccess {
                allowed_groups: Vec::new(),
                writable_groups: Vec::new(),
            });
            if !access.allowed_groups.iter().any(|g| g.eq_ignore_ascii_case(name)) {
                access.allowed_groups.push(name.to_string());
            }
            if writable && !access.writable_groups.iter().any(|g| g.eq_ignore_ascii_case(name)) {
                access.writable_groups.push(name.to_string());
            }
            if !writable {
                access.writable_groups.retain(|g| !g.eq_ignore_ascii_case(name));
            }
        }
        "notes" => {
            let access = config.notes.get_or_insert_with(|| NoteAccess {
                allowed_folders: Vec::new(),
                writable_folders: Vec::new(),
            });
            if !access.allowed_folders.iter().any(|f| f.eq_ignore_ascii_case(name)) {
                access.allowed_folders.push(name.to_string());
            }
            if writable && !access.writable_folders.iter().any(|f| f.eq_ignore_ascii_case(name)) {
                access.writable_folders.push(name.to_string());
            }
            if !writable {
                access.writable_folders.retain(|f| !f.eq_ignore_ascii_case(name));
            }
        }
        "files" => {
            // Validate that the path looks absolute
            if !name.starts_with('/') {
                bail!("File folder path must be absolute (start with /). Got: {}", name);
            }
            let access = config.files.get_or_insert_with(|| FileAccess {
                allowed_folders: Vec::new(),
                writable_folders: Vec::new(),
            });
            // File paths are case-sensitive (unlike app resource names)
            if !access.allowed_folders.iter().any(|f| f == name) {
                access.allowed_folders.push(name.to_string());
            }
            if writable && !access.writable_folders.iter().any(|f| f == name) {
                access.writable_folders.push(name.to_string());
            }
            if !writable {
                access.writable_folders.retain(|f| f != name);
            }
        }
        other => bail!(
            "Unknown category: {}. Use: reminders, contacts, notes, files",
            other
        ),
    }

    config.save()?;
    println!("Granted {} access to {} \"{}\"", mode, category, name);
    Ok(())
}

/// Revoke access to a resource.
pub fn cmd_revoke(category: &str, name: &str) -> Result<()> {
    let mut config = AccessConfig::load()?;

    match category {
        "reminders" => {
            if let Some(access) = &mut config.reminders {
                access.allowed_lists.retain(|l| !l.eq_ignore_ascii_case(name));
                access.writable_lists.retain(|l| !l.eq_ignore_ascii_case(name));
            }
        }
        "contacts" => {
            if let Some(access) = &mut config.contacts {
                access.allowed_groups.retain(|g| !g.eq_ignore_ascii_case(name));
                access.writable_groups.retain(|g| !g.eq_ignore_ascii_case(name));
            }
        }
        "notes" => {
            if let Some(access) = &mut config.notes {
                access.allowed_folders.retain(|f| !f.eq_ignore_ascii_case(name));
                access.writable_folders.retain(|f| !f.eq_ignore_ascii_case(name));
            }
        }
        "files" => {
            if let Some(access) = &mut config.files {
                access.allowed_folders.retain(|f| f != name);
                access.writable_folders.retain(|f| f != name);
            }
        }
        other => bail!(
            "Unknown category: {}. Use: reminders, contacts, notes, files",
            other
        ),
    }

    config.save()?;
    println!("Revoked access to {} \"{}\"", category, name);
    Ok(())
}

/// Reset all access controls (return to unrestricted).
pub fn cmd_reset() -> Result<()> {
    let path = crate::access_config::config_path();
    if path.exists() {
        std::fs::remove_file(&path)?;
        println!("Access restrictions removed. Full access restored.");
    } else {
        println!("No access restrictions were configured.");
    }
    Ok(())
}
