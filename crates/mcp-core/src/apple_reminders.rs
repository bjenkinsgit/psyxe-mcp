//! Apple Reminders Integration via EventKit (Swift helper) with AppleScript fallback
//!
//! Prefers a compiled Swift binary (`reminders-helper`) that uses EventKit for
//! direct Reminders API access. Falls back to AppleScript files when the binary
//! is not found. The Swift helper uses JSON stdin/stdout while AppleScript uses
//! the delimiter-based `RECORD_START`/`RECORD_END` protocol.
//!
//! Includes an allowed-lists Keychain store for restricting which Reminder lists
//! the agent can interact with.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};


use crate::access_store::AccessStore;
use crate::applescript_utils::{find_scripts_dir, run_script};

// ============================================================================
// Data Structures
// ============================================================================

/// A reminder record parsed from AppleScript search/list output
#[derive(Debug, Serialize)]
pub struct ReminderRecord {
    pub id: String,
    pub name: String,
    pub list: String,
    pub due_date: String,
    pub completed: bool,
    pub priority: u8,
    pub notes: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub location: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub start_date: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentInfo>,
}

/// Alarm information from a reminder
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AlarmInfo {
    #[serde(rename = "location")]
    Location {
        title: String,
        latitude: f64,
        longitude: f64,
        radius: f64,
        proximity: String,
    },
    #[serde(rename = "time")]
    Time { offset_minutes: i64 },
}

/// Attachment information from a reminder (fetched from Reminders CoreData store)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentInfo {
    /// Attachment type: "file", "url", "image", "audio", or "attachment"
    #[serde(rename = "type")]
    pub attachment_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uti: Option<String>,
    /// Local file path if the iCloud link resolves to a file on disk
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
}

/// Recurrence rule information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecurrenceInfo {
    pub frequency: String,
    pub interval: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurrence_count: Option<u32>,
}

/// Full reminder detail from get output
#[derive(Debug, Serialize)]
pub struct ReminderDetail {
    pub name: String,
    pub list: String,
    pub due_date: String,
    pub completed: bool,
    pub priority: u8,
    pub notes: String,
    pub created: String,
    pub modified: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub location: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub start_date: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub completion_date: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alarms: Vec<AlarmInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<RecurrenceInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentInfo>,
}

/// A reminder list returned by the Reminders.app AppleScript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReminderListInfo {
    pub name: String,
    pub id: String,
}

// ============================================================================
// Shortcuts URL helper
// ============================================================================

/// Extract a plain filesystem path from a URL or path string.
/// file:/// URLs are decoded to bare paths; bare paths pass through unchanged.
/// Non-file URLs (https://, etc.) are passed through as-is.
///
/// Also fixes a common LLM encoding mistake where the `/` in iCloud's
/// `Library/Mobile Documents` directory is percent-encoded as `%20`,
/// producing the invalid path `Library Mobile Documents`.
fn url_to_path(url: &str) -> String {
    if url.starts_with("file:///") {
        let raw = &url["file:///".len()..];
        let decoded = urlencoding::decode(raw)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| raw.to_string());
        let path = format!("/{}", decoded);

        // Fix common LLM error: "Library Mobile Documents" → "Library/Mobile Documents"
        // LLMs sometimes percent-encode the slash as %20, merging the two path components.
        if path.contains("Library Mobile Documents") {
            let fixed = path.replace("Library Mobile Documents", "Library/Mobile Documents");
            if std::path::Path::new(&fixed).parent().map_or(false, |p| p.exists()) {
                tracing::warn!(
                    original = %path,
                    fixed = %fixed,
                    "Fixed LLM URL encoding error: 'Library Mobile Documents' → 'Library/Mobile Documents'"
                );
                return fixed;
            }
        }

        path
    } else {
        url.to_string()
    }
}

/// Set a reminder's URL field via the macOS Shortcuts app.
/// This bypasses EventKit (which silently drops reminder URLs) by using the
/// Set a reminder's URL field via the "Set_Reminder_URL" Apple Shortcut.
/// EventKit's `reminder.url` doesn't reliably persist, so we use the Shortcut
/// which sets it through Shortcuts actions that do stick.
/// For local file paths, builds a `file:///` URL so the reminder opens the file
/// directly via macOS default app (the Shortcut must store this URL as-is).
fn set_reminder_url_via_helper(list_name: &str, reminder_title: &str, url: &str) -> Result<()> {
    let file_path = url_to_path(url);

    // Validate that the file exists before setting the URL — catch bad LLM paths early
    // instead of silently setting a broken URL that fails when clicked.
    if !file_path.starts_with("http") && !std::path::Path::new(&file_path).exists() {
        return Err(anyhow!(
            "File does not exist: '{}'. Check the path — iCloud paths use 'Library/Mobile Documents' (with a slash), \
             not 'Library Mobile Documents'. The correct file:// URL encoding keeps the slash literal: \
             file:///Users/.../Library/Mobile%20Documents/...",
            file_path
        ));
    }

    // Build a file:/// URL for local paths so the reminder opens directly
    // via macOS default app, not through a shortcuts:// wrapper.
    let url_value = if file_path.starts_with("http") {
        file_path.clone()
    } else {
        format!("file://{}", urlencoding::encode(&file_path)
            .replace("%2F", "/"))
    };

    let input = json!({
        "list_name": list_name,
        "reminder_title": reminder_title,
        "the_file": url_value,
    });
    let input_str = serde_json::to_string(&input)?;

    tracing::info!(
        list = list_name,
        reminder = reminder_title,
        url = %url_value,
        "Setting reminder URL via Shortcut"
    );

    let mut child = std::process::Command::new("shortcuts")
        .arg("run")
        .arg("Set_Reminder_URL")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn Set_Reminder_URL shortcut: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(input_str.as_bytes())
            .map_err(|e| anyhow!("Failed to write to Set_Reminder_URL stdin: {}", e))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| anyhow!("Failed to wait for Set_Reminder_URL shortcut: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(
            stderr = %stderr,
            "Set_Reminder_URL shortcut failed"
        );
        return Err(anyhow!("Set_Reminder_URL shortcut failed: {}", stderr));
    }

    tracing::info!("Set_Reminder_URL shortcut succeeded");
    Ok(())
}

// ============================================================================
// Swift Helper Binary
// ============================================================================

/// Cached path to the Swift reminders-helper binary (None = not yet found).
/// Uses Mutex instead of OnceLock so a failed lookup can be retried
/// (e.g. when SCRIPTS_DIR_OVERRIDE is set after the first call).
static HELPER_PATH: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Search for the compiled Swift helper binary in known locations.
fn find_helper_binary() -> Option<PathBuf> {
    // 1. SCRIPTS_DIR_OVERRIDE (used by Tauri bundled resources)
    if let Some(override_dir) = std::env::var_os("SCRIPTS_DIR_OVERRIDE") {
        let p = PathBuf::from(override_dir).join("reminders-helper");
        if p.is_file() {
            return Some(p);
        }
    }

    // 2. Dev paths relative to CWD
    let dev_paths = [
        "target/swift/reminders-helper",
        "swift/reminders-helper/.build/release/reminders-helper",
        "swift/reminders-helper/.build/debug/reminders-helper",
        "../../swift/reminders-helper/.build/release/reminders-helper",
        "../../swift/reminders-helper/.build/debug/reminders-helper",
    ];
    for rel in &dev_paths {
        let p = PathBuf::from(rel);
        if p.is_file() {
            return Some(p);
        }
    }

    // 3. Relative to the running executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            // Same directory as the binary (MCP server standalone install)
            let p = exe_dir.join("reminders-helper");
            if p.is_file() {
                return Some(p);
            }
            // Bundled macOS app: Contents/MacOS/../Helpers/reminders-helper (preferred)
            let p = exe_dir.join("../Helpers/reminders-helper");
            if p.is_file() {
                return Some(p);
            }
            // Bundled macOS app: Contents/MacOS/../Resources/reminders-helper (legacy)
            let p = exe_dir.join("../Resources/reminders-helper");
            if p.is_file() {
                return Some(p);
            }
            // Dev build: target/release/ → target/swift/
            let p = exe_dir.join("../swift/reminders-helper");
            if p.is_file() {
                return Some(p);
            }
        }
    }

    // 4. CARGO_MANIFEST_DIR-relative (for cargo test/run)
    if let Some(manifest_dir) = std::env::var_os("CARGO_MANIFEST_DIR") {
        let base = PathBuf::from(manifest_dir);
        for suffix in &[
            "../../target/swift/reminders-helper",
            "../../swift/reminders-helper/.build/release/reminders-helper",
            "../../swift/reminders-helper/.build/debug/reminders-helper",
        ] {
            let p = base.join(suffix);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    None
}

/// Get the cached helper binary path.
/// Retries lookup if not yet found (SCRIPTS_DIR_OVERRIDE may be set late).
fn helper_binary() -> Option<PathBuf> {
    let guard = HELPER_PATH.lock().unwrap();
    if let Some(ref path) = *guard {
        return Some(path.clone());
    }
    drop(guard);

    // Not cached yet — try to find it
    if let Some(path) = find_helper_binary() {
        tracing::info!(helper_path = %path.display(), "Resolved reminders-helper binary");
        let mut guard = HELPER_PATH.lock().unwrap();
        *guard = Some(path.clone());
        Some(path)
    } else {
        None
    }
}

/// Run the Swift helper binary with the given subcommand and JSON input.
/// Returns the stdout output on success.
fn run_helper(subcommand: &str, args: &Value) -> Result<String> {
    let binary = helper_binary()
        .ok_or_else(|| anyhow!("Swift helper binary not found"))?;

    let mut child = Command::new(binary)
        .arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn reminders-helper: {}", e))?;

    // Write JSON to stdin
    if let Some(mut stdin) = child.stdin.take() {
        let json_bytes = serde_json::to_vec(args)?;
        stdin.write_all(&json_bytes)?;
        // stdin is dropped here, closing the pipe
    }

    // Wait with timeout (EventKit auth prompt can block)
    let output = child
        .wait_with_output()
        .map_err(|e| anyhow!("Failed to wait for reminders-helper: {}", e))?;

    // Log helper diagnostics from stderr (even on success)
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        for line in stderr.lines() {
            if line.starts_with("[reminders-helper]") {
                tracing::info!("{}", line);
            }
        }
    }

    if output.status.success() {
        let result = String::from_utf8_lossy(&output.stdout).to_string();
        tracing::debug!(subcommand, result_len = result.len(), "helper stdout (first 500): {}", &result[..result.len().min(500)]);
        Ok(result)
    } else {
        // Try to extract error message from stderr JSON
        if let Ok(err_json) = serde_json::from_str::<Value>(&stderr) {
            if let Some(msg) = err_json.get("message").and_then(|v| v.as_str()) {
                return Err(anyhow!("{}", msg));
            }
        }
        let code = output.status.code().unwrap_or(-1);
        if code == 2 {
            Err(anyhow!("Reminders access not granted"))
        } else {
            Err(anyhow!(
                "reminders-helper exited with code {}: {}",
                code,
                stderr.trim()
            ))
        }
    }
}

// ============================================================================
// Allowed-Lists Store (thin facade over unified AccessStore)
// ============================================================================

/// Public type returned by store methods for Tauri commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowedReminderList {
    pub name: String,
    pub enabled: bool,
    pub allowed_at: String,
    pub guidance: Option<String>,
}

/// Facade for managing which Reminder lists the agent can access.
///
/// Delegates to [`AccessStore`] which holds all access-control data in a
/// single Keychain entry with a process-level cache.
///
/// When no entries exist, all lists are accessible (no restriction).
/// When entries exist, only lists marked as allowed+enabled are accessible
/// for targeted operations (create, complete, delete).
pub struct AllowedRemindersStore;

impl AllowedRemindersStore {
    /// Load all allowed-list entries.
    pub fn list_all() -> Result<Vec<AllowedReminderList>, String> {
        AccessStore::list_reminder_lists()
    }

    /// Check if a list name is directly allowed for targeted operations.
    pub fn is_allowed(list_name: &str) -> bool {
        AccessStore::is_reminder_list_allowed(list_name)
    }

    /// Get the list of enabled group/list names.
    pub fn enabled_names() -> Vec<String> {
        AccessStore::reminder_list_enabled_names()
    }

    /// Returns true if the allowed store has any entries.
    pub fn has_restrictions() -> bool {
        AccessStore::has_reminder_restrictions()
    }

    /// Add a list to the allowed set.
    pub fn allow_list(list_name: &str) -> Result<(), String> {
        AccessStore::allow_reminder_list(list_name)
    }

    /// Remove a list from the allowed set.
    pub fn disallow_list(list_name: &str) -> Result<(), String> {
        AccessStore::disallow_reminder_list(list_name)
    }

    /// Toggle the enabled state for an allowed list.
    pub fn set_enabled(list_name: &str, enabled: bool) -> Result<(), String> {
        AccessStore::set_reminder_list_enabled(list_name, enabled)
    }

    /// Set or clear the guidance prompt for an allowed list.
    pub fn set_guidance(list_name: &str, guidance: Option<String>) -> Result<(), String> {
        AccessStore::set_reminder_list_guidance(list_name, guidance)
    }
}

/// Format a `## Reminder Lists` guidance section for the agent system prompt.
///
/// Returns guidance lines for all allowed+enabled lists that have a guidance prompt.
/// Returns an empty string if no guidance is configured.
pub fn format_reminders_guidance_section() -> String {
    AccessStore::format_reminders_guidance_section()
}

/// Format a short suffix for reminders tool descriptions listing allowed lists.
/// Returns empty string when no restrictions are configured.
pub fn format_allowed_lists_suffix() -> String {
    if !AccessStore::has_reminder_restrictions() {
        return String::new();
    }
    let names = AccessStore::reminder_list_enabled_names();
    if names.is_empty() {
        return String::new();
    }
    format!(" [Allowed lists: {}]", names.join(", "))
}

// ============================================================================
// Availability Check
// ============================================================================

/// Check if Apple Reminders integration is available (macOS only).
/// Returns true if the Swift helper binary OR AppleScript files are found.
pub fn is_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        if helper_binary().is_some() {
            return true;
        }
        let scripts_dir = find_scripts_dir();
        scripts_dir.join("reminders_search.applescript").exists()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

// ============================================================================
// Output Parsing
// ============================================================================

/// Parse delimiter-based output into ReminderRecords
fn parse_records(output: &str) -> Result<Vec<ReminderRecord>> {
    let mut records = Vec::new();
    let mut current: Option<ReminderRecord> = None;

    for line in output.lines() {
        let line = line.trim();

        if line == "RECORD_START" {
            current = Some(ReminderRecord {
                id: String::new(),
                name: String::new(),
                list: String::new(),
                due_date: String::new(),
                completed: false,
                priority: 0,
                notes: String::new(),
                snippet: String::new(),
                url: String::new(),
                location: String::new(),
                start_date: String::new(),
                attachments: Vec::new(),
            });
        } else if line == "RECORD_END" {
            if let Some(record) = current.take() {
                records.push(record);
            }
        } else if line.starts_with("ERROR:") {
            return Err(anyhow!("{}", line));
        } else if let Some(ref mut record) = current {
            if let Some((key, value)) = line.split_once(": ") {
                match key {
                    "id" => record.id = value.to_string(),
                    "name" => record.name = value.to_string(),
                    "list" => record.list = value.to_string(),
                    "due_date" => record.due_date = value.to_string(),
                    "completed" => record.completed = value == "true",
                    "priority" => record.priority = value.parse().unwrap_or(0),
                    "notes" => record.notes = value.to_string(),
                    "snippet" => record.snippet = value.to_string(),
                    _ => {}
                }
            }
        }
    }

    Ok(records)
}

/// Parse reminder detail from get output (key-value, not RECORD delimited)
fn parse_reminder_detail(output: &str) -> Result<ReminderDetail> {
    let mut detail = ReminderDetail {
        name: String::new(),
        list: String::new(),
        due_date: String::new(),
        completed: false,
        priority: 0,
        notes: String::new(),
        created: String::new(),
        modified: String::new(),
        url: String::new(),
        location: String::new(),
        start_date: String::new(),
        completion_date: String::new(),
        alarms: Vec::new(),
        recurrence: None,
        attachments: Vec::new(),
    };

    for line in output.lines() {
        let line = line.trim();

        if line.starts_with("ERROR:") {
            return Err(anyhow!("{}", line));
        }

        if let Some((key, value)) = line.split_once(": ") {
            match key {
                "name" => detail.name = value.to_string(),
                "list" => detail.list = value.to_string(),
                "due_date" => detail.due_date = value.to_string(),
                "completed" => detail.completed = value == "true",
                "priority" => detail.priority = value.parse().unwrap_or(0),
                "notes" => detail.notes = value.to_string(),
                "created" => detail.created = value.to_string(),
                "modified" => detail.modified = value.to_string(),
                _ => {}
            }
        }
    }

    if detail.name.is_empty() {
        return Err(anyhow!("Failed to parse reminder detail"));
    }

    Ok(detail)
}

/// Parse RECORD_START/RECORD_END delimited list output into ReminderListInfo.
fn parse_list_records(output: &str) -> Result<Vec<ReminderListInfo>> {
    let mut lists = Vec::new();
    let mut name = String::new();
    let mut id = String::new();
    let mut in_record = false;

    for line in output.lines() {
        let line = line.trim();
        if line == "RECORD_START" {
            name.clear();
            id.clear();
            in_record = true;
        } else if line == "RECORD_END" {
            if in_record && !name.is_empty() {
                lists.push(ReminderListInfo {
                    name: name.clone(),
                    id: id.clone(),
                });
            }
            in_record = false;
        } else if line.starts_with("ERROR:") {
            return Err(anyhow!("{}", line));
        } else if in_record {
            if let Some((key, value)) = line.split_once(": ") {
                match key {
                    "name" => name = value.to_string(),
                    "id" => id = value.to_string(),
                    _ => {}
                }
            }
        }
    }

    Ok(lists)
}

// ============================================================================
// Access Validation
// ============================================================================

/// Validate that a list name is allowed for targeted operations.
/// Uses the process-level cache (no Keychain reads after first load).
fn validate_list_access(list_name: &str) -> Result<()> {
    if AllowedRemindersStore::is_allowed(list_name) {
        return Ok(());
    }

    Err(anyhow!(
        "Access denied: list '{}' is not in the allowed set. \
         Configure allowed lists in the Reminders settings.",
        list_name
    ))
}

/// Filter a `list_lists` JSON result to only include allowed lists.
/// When no restrictions are configured, returns the result unchanged.
fn filter_list_lists_result(result: String) -> String {
    if !AllowedRemindersStore::has_restrictions() {
        return result;
    }
    let allowed = AllowedRemindersStore::enabled_names();
    let Ok(mut parsed) = serde_json::from_str::<Value>(&result) else {
        return result;
    };
    if let Some(lists) = parsed.get_mut("lists").and_then(|v| v.as_array_mut()) {
        lists.retain(|item| {
            item.get("name")
                .and_then(|n| n.as_str())
                .map(|name| allowed.iter().any(|a| a.eq_ignore_ascii_case(name)))
                .unwrap_or(false)
        });
        parsed["count"] = json!(lists.len());
    }
    serde_json::to_string(&parsed).unwrap_or(result)
}

/// Centralized access control gate — called at the top of execute_apple_reminders()
/// BEFORE any execution path (FFI, helper, AppleScript).
fn validate_action_access(action: &str, args: &Value) -> Result<()> {
    // Discovery and UI actions are always allowed
    if matches!(action, "list_lists" | "open") {
        return Ok(());
    }

    if !AllowedRemindersStore::has_restrictions() {
        return Ok(()); // No restrictions configured = all access allowed
    }

    let list = args.get("list").and_then(|v| v.as_str()).filter(|s| !s.is_empty());

    match action {
        // List management — validate the target list name
        "create_list" | "delete_list" => {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if action == "delete_list" && !name.is_empty() {
                validate_list_access(name)?;
            }
            // create_list: new list won't be in allowed set yet — allow creation,
            // but it won't be usable until added to the allowed set
        }
        // Batch operations — validate batch-level and per-item lists
        "create_batch" | "edit_batch" => {
            if let Some(batch_list) = list {
                validate_list_access(batch_list)?;
            }
            if let Some(items) = args.get("items").and_then(|v| v.as_array()) {
                for item in items {
                    let item_list = item.get("list").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
                    if let Some(il) = item_list {
                        validate_list_access(il)?;
                    } else if list.is_none() {
                        return Err(anyhow!(
                            "Access restrictions are active. Specify a list from the allowed set: {:?}",
                            AllowedRemindersStore::enabled_names()
                        ));
                    }
                }
            }
        }
        // Search and get — auto-scope to allowed lists when no list specified
        "search" | "get" => {
            if let Some(list_name) = list {
                validate_list_access(list_name)?;
            }
            // No list specified is OK for search/get — they'll be scoped
            // to allowed lists downstream (Swift helper retries across allowed lists)
        }
        // All other actions (list, create, complete, delete, edit)
        _ => {
            if let Some(list_name) = list {
                validate_list_access(list_name)?;
            } else {
                // No list specified but restrictions are active — reject
                return Err(anyhow!(
                    "Access restrictions are active. Specify a 'list' parameter from the allowed set: {:?}",
                    AllowedRemindersStore::enabled_names()
                ));
            }
        }
    }

    Ok(())
}

// ============================================================================
// Priority Mapping
// ============================================================================

/// Map priority strings to Apple Reminders priority values.
/// Apple uses: 0=none, 1=high, 5=medium, 9=low
pub fn map_priority(priority: &str) -> u8 {
    match priority.to_lowercase().as_str() {
        "high" | "1" => 1,
        "medium" | "med" | "5" => 5,
        "low" | "9" => 9,
        "none" | "0" | "" => 0,
        _ => priority.parse().unwrap_or(0),
    }
}

// ============================================================================
// Internal Operations
// ============================================================================

fn list_reminder_lists() -> Result<String> {
    let output = run_script("reminders_list_lists.applescript", &[])?;
    let mut lists = parse_list_records(&output)?;
    lists.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(serde_json::to_string(&json!({
        "count": lists.len(),
        "lists": lists
    }))?)
}

fn search_reminders(args: &Value) -> Result<String> {
    let query = args["query"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required 'query' argument"))?;
    let list = args.get("list").and_then(|v| v.as_str());

    let script_args: Vec<&str> = match list {
        Some(l) => vec![query, l],
        None => vec![query],
    };

    let output = run_script("reminders_search.applescript", &script_args)?;
    let records = parse_records(&output)?;

    Ok(serde_json::to_string(&json!({
        "count": records.len(),
        "results": records
    }))?)
}

fn list_reminders(args: &Value) -> Result<String> {
    let list = args["list"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required 'list' argument"))?;

    validate_list_access(list)?;

    let show_completed = args
        .get("show_completed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let show_str = if show_completed { "true" } else { "false" };
    let output = run_script("reminders_list.applescript", &[list, show_str])?;
    let records = parse_records(&output)?;

    Ok(serde_json::to_string(&json!({
        "list": list,
        "count": records.len(),
        "reminders": records
    }))?)
}

fn get_reminder(args: &Value) -> Result<String> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required 'name' argument"))?;
    let list = args.get("list").and_then(|v| v.as_str());

    let script_args: Vec<&str> = match list {
        Some(l) => vec![name, l],
        None => vec![name],
    };

    let output = run_script("reminders_get.applescript", &script_args)?;
    let detail = parse_reminder_detail(&output)?;

    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "reminder": detail
    }))?)
}

fn create_reminder(args: &Value) -> Result<String> {
    let title = args["title"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required 'title' argument"))?;
    let list = args.get("list").and_then(|v| v.as_str()).unwrap_or("");
    let due_date = args.get("due_date").and_then(|v| v.as_str()).unwrap_or("");
    let notes = args.get("notes").and_then(|v| v.as_str()).unwrap_or("");
    let priority = args
        .get("priority")
        .and_then(|v| v.as_str())
        .map(map_priority)
        .unwrap_or(0);

    // Validate list access if a specific list is requested
    if !list.is_empty() {
        validate_list_access(list)?;
    }

    let priority_str = priority.to_string();
    let output = run_script(
        "reminders_create.applescript",
        &[title, list, due_date, notes, &priority_str],
    )?;

    let output = output.trim();
    if output.starts_with("ERROR:") {
        return Err(anyhow!("{}", output));
    }

    Ok(serde_json::to_string(&json!({
        "success": true,
        "message": output
    }))?)
}

fn complete_reminder(args: &Value) -> Result<String> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required 'name' argument"))?;
    let list = args.get("list").and_then(|v| v.as_str()).unwrap_or("");
    let completed = args
        .get("completed")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let completed_str = if completed { "true" } else { "false" };
    let output = run_script(
        "reminders_complete.applescript",
        &[name, list, completed_str],
    )?;

    let output = output.trim();
    if output.starts_with("ERROR:") {
        return Err(anyhow!("{}", output));
    }

    Ok(serde_json::to_string(&json!({
        "success": true,
        "message": output
    }))?)
}

fn delete_reminder(args: &Value) -> Result<String> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required 'name' argument"))?;
    let list = args.get("list").and_then(|v| v.as_str()).unwrap_or("");

    let output = run_script("reminders_delete.applescript", &[name, list])?;

    let output = output.trim();
    if output.starts_with("ERROR:") {
        return Err(anyhow!("{}", output));
    }

    Ok(serde_json::to_string(&json!({
        "success": true,
        "message": output
    }))?)
}

fn create_reminders_batch(args: &Value) -> Result<String> {
    let list = args["list"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required 'list' argument"))?;
    let items = args["items"]
        .as_array()
        .ok_or_else(|| anyhow!("Missing required 'items' array argument"))?;

    validate_list_access(list)?;

    // Encode items as "title:::notes|||title:::notes|||..."
    let mut encoded_parts: Vec<String> = Vec::new();
    for item in items {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let notes = item.get("notes").and_then(|v| v.as_str()).unwrap_or("");
        if notes.is_empty() {
            encoded_parts.push(title.to_string());
        } else {
            encoded_parts.push(format!("{}:::{}", title, notes));
        }
    }

    if encoded_parts.is_empty() {
        return Ok(serde_json::to_string(&json!({
            "success": true,
            "message": "No items to create"
        }))?);
    }

    let encoded = encoded_parts.join("|||");
    let output = run_script("reminders_create_batch.applescript", &[list, &encoded])?;

    let output = output.trim();
    if output.starts_with("ERROR:") {
        return Err(anyhow!("{}", output));
    }

    Ok(serde_json::to_string(&json!({
        "success": true,
        "message": output
    }))?)
}

fn edit_reminder(args: &Value) -> Result<String> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required 'name' argument"))?;
    let list = args.get("list").and_then(|v| v.as_str()).unwrap_or("");
    let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let due_date = args.get("due_date").and_then(|v| v.as_str()).unwrap_or("");
    let notes = args.get("notes").and_then(|v| v.as_str()).unwrap_or("");
    let priority = args
        .get("priority")
        .and_then(|v| v.as_str())
        .map(map_priority)
        .map(|p| p.to_string())
        .unwrap_or_default();

    let output = run_script(
        "reminders_edit.applescript",
        &[name, list, title, due_date, notes, &priority],
    )?;

    let output = output.trim();
    if output.starts_with("ERROR:") {
        return Err(anyhow!("{}", output));
    }

    Ok(serde_json::to_string(&json!({
        "success": true,
        "message": output
    }))?)
}

fn edit_reminders_batch(args: &Value) -> Result<String> {
    let list = args["list"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required 'list' argument"))?;
    let items = args["items"]
        .as_array()
        .ok_or_else(|| anyhow!("Missing required 'items' array argument"))?;

    validate_list_access(list)?;

    // Encode items as "name:::title:::due_date:::notes:::priority" joined by "|||"
    let mut encoded_parts: Vec<String> = Vec::new();
    for item in items {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let due_date = item.get("due_date").and_then(|v| v.as_str()).unwrap_or("");
        let notes = item.get("notes").and_then(|v| v.as_str()).unwrap_or("");
        let priority = item
            .get("priority")
            .and_then(|v| v.as_str())
            .map(map_priority)
            .map(|p| p.to_string())
            .unwrap_or_default();

        encoded_parts.push(format!(
            "{}:::{}:::{}:::{}:::{}",
            name, title, due_date, notes, priority
        ));
    }

    if encoded_parts.is_empty() {
        return Ok(serde_json::to_string(&json!({
            "success": true,
            "message": "No items to edit"
        }))?);
    }

    let encoded = encoded_parts.join("|||");
    let output = run_script("reminders_edit_batch.applescript", &[list, &encoded])?;

    let output = output.trim();
    if output.starts_with("ERROR:") {
        return Err(anyhow!("{}", output));
    }

    Ok(serde_json::to_string(&json!({
        "success": true,
        "message": output
    }))?)
}

fn open_reminders(args: &Value) -> Result<String> {
    let list = args.get("list").and_then(|v| v.as_str()).unwrap_or("");

    let output = run_script("reminders_open.applescript", &[list])?;

    let output = output.trim();
    if output.starts_with("ERROR:") {
        return Err(anyhow!("{}", output));
    }

    Ok(serde_json::to_string(&json!({
        "success": true,
        "message": output
    }))?)
}

fn create_reminder_list(args: &Value) -> Result<String> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required 'name' argument"))?;

    let output = run_script("reminders_create_list.applescript", &[name])?;

    let output = output.trim();
    if output.starts_with("ERROR:") {
        return Err(anyhow!("{}", output));
    }

    Ok(serde_json::to_string(&json!({
        "success": true,
        "message": output
    }))?)
}

fn delete_reminder_list(args: &Value) -> Result<String> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required 'name' argument"))?;

    let output = run_script("reminders_delete_list.applescript", &[name])?;

    let output = output.trim();
    if output.starts_with("ERROR:") {
        return Err(anyhow!("{}", output));
    }

    Ok(serde_json::to_string(&json!({
        "success": true,
        "message": output
    }))?)
}

// ============================================================================
// Public API
// ============================================================================

/// Fetch all reminder lists (for Tauri picker UI).
/// Tries the Swift helper first, falls back to AppleScript.
/// This bypasses access restrictions since it's needed for discovery.
/// Results are sorted alphabetically by name (case-insensitive).
pub fn fetch_all_reminder_lists() -> Result<Vec<ReminderListInfo>, String> {
    // 1. Try in-process EventKit FFI (works in sandboxed release builds)
    #[cfg(target_os = "macos")]
    {
        match crate::eventkit_ffi::list_all_calendars() {
            Ok(cals) if !cals.is_empty() => {
                tracing::info!(count = cals.len(), "fetch_all_reminder_lists: EventKit FFI succeeded");
                let mut lists: Vec<ReminderListInfo> = cals
                    .into_iter()
                    .map(|(name, id)| ReminderListInfo { name, id })
                    .collect();
                lists.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                return Ok(lists);
            }
            Ok(_) => {
                tracing::debug!("EventKit FFI returned 0 lists, trying helper");
            }
            Err(e) => {
                tracing::debug!("EventKit FFI failed: {}, trying helper", e);
            }
        }
    }

    // 2. Try Swift helper
    let has_helper = helper_binary().is_some();
    tracing::info!(helper_found = has_helper, "fetch_all_reminder_lists: checking helper");
    if has_helper {
        match run_helper("list-lists", &json!({})) {
            Ok(output) => {
                tracing::info!(output_len = output.len(), "fetch_all_reminder_lists: helper returned output");
                if let Ok(parsed) = serde_json::from_str::<Value>(&output) {
                    if let Some(lists_arr) = parsed.get("lists").and_then(|v| v.as_array()) {
                        let lists: Vec<ReminderListInfo> = lists_arr
                            .iter()
                            .filter_map(|v| {
                                Some(ReminderListInfo {
                                    name: v.get("name")?.as_str()?.to_string(),
                                    id: v.get("id")?.as_str()?.to_string(),
                                })
                            })
                            .collect();
                        tracing::info!(count = lists.len(), "fetch_all_reminder_lists: returning lists");
                        if !lists.is_empty() {
                            return Ok(lists);
                        }
                        // Helper returned 0 lists (sandbox may block EventKit XPC) — try AppleScript
                        tracing::info!("Helper returned 0 lists, falling back to AppleScript");
                    } else {
                        tracing::warn!(json = %parsed, "fetch_all_reminder_lists: no 'lists' array in response");
                    }
                } else {
                    tracing::warn!(raw = %output, "fetch_all_reminder_lists: failed to parse JSON");
                }
            }
            Err(e) => {
                tracing::warn!("Swift helper failed for list-lists, falling back to AppleScript: {}", e);
            }
        }
    }

    // Fallback to AppleScript
    let output = run_script("reminders_list_lists.applescript", &[])
        .map_err(|e| e.to_string())?;
    let mut lists = parse_list_records(&output).map_err(|e| e.to_string())?;
    lists.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(lists)
}

/// Map an action name to the Swift helper subcommand.
fn action_to_subcommand(action: &str) -> Option<&'static str> {
    match action {
        "list_lists" => Some("list-lists"),
        "search" => Some("search"),
        "list" => Some("list"),
        "get" => Some("get"),
        "create" => Some("create"),
        "create_batch" => Some("create-batch"),
        "complete" => Some("complete"),
        "delete" => Some("delete"),
        "edit" => Some("edit"),
        "edit_batch" => Some("edit-batch"),
        "open" => Some("open"),
        "create_list" => Some("create-list"),
        "delete_list" => Some("delete-list"),
        _ => None,
    }
}

/// After a successful edit, set the URL field via the Swift EventKit helper
/// to ensure it persists (the edit command's URL field is sometimes dropped).
/// `resolved_list` is extracted from the Swift helper response when the LLM
/// doesn't provide a list name in the tool args.
/// Returns an error string if URL setting failed (e.g. file doesn't exist),
/// so the caller can append it to the tool result for the LLM to see.
fn set_urls_via_shortcut(action: &str, args: &Value, resolved_list: Option<&str>) -> Option<String> {
    if action == "edit" {
        if let (Some(url), Some(name)) = (
            args.get("url").and_then(|v| v.as_str()),
            args.get("name").and_then(|v| v.as_str()),
        ) {
            let list = args
                .get("list")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .or(resolved_list)
                .unwrap_or("");
            if list.is_empty() {
                tracing::warn!(
                    reminder = name,
                    "Skipping set-url helper: list name is required but was not provided"
                );
                return Some("URL not set: list name is required".to_string());
            }
            if let Err(e) = set_reminder_url_via_helper(list, name, url) {
                let err_msg = format!("FAILED to set URL: {}. The file must exist before you can attach it. Create the file first, then set the URL.", e);
                tracing::warn!(error = %e, "Failed to set URL via helper");
                return Some(err_msg);
            }
        }
    } else if action == "edit_batch" {
        let list = args
            .get("list")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or(resolved_list)
            .unwrap_or("");
        if list.is_empty() {
            tracing::warn!("Skipping set-url helper for batch: list name is required but was not provided");
            return Some("URL not set: list name is required for batch".to_string());
        }
        if let Some(items) = args.get("items").and_then(|v| v.as_array()) {
            let mut errors = Vec::new();
            for item in items {
                if let (Some(url), Some(name)) = (
                    item.get("url").and_then(|v| v.as_str()),
                    item.get("name").and_then(|v| v.as_str()),
                ) {
                    if let Err(e) = set_reminder_url_via_helper(list, name, url) {
                        tracing::warn!(
                            reminder = name,
                            error = %e,
                            "Failed to set URL via helper"
                        );
                        errors.push(format!("{}: {}", name, e));
                    }
                }
            }
            if !errors.is_empty() {
                return Some(format!("FAILED to set URL for: {}", errors.join("; ")));
            }
        }
    }
    None
}

/// Actions where "not found in list X" should trigger a cross-list retry.
const RETRIABLE_ACTIONS: &[&str] = &["edit", "complete", "delete", "get"];

/// Main entry point for agent tool execution.
/// Tries the Swift EventKit helper first, falls back to AppleScript.
/// Sanitize edit args to prevent LLM echo-back issues.
///
/// LLMs tend to echo all fields from `get_reminder` back into `edit_reminder`,
/// sending empty strings and zero-valued defaults for fields they don't intend
/// to change. Without sanitization this causes:
/// - `recurrence` + empty `due_date` → Swift validation error
/// - `notes: ""` → wipes task instructions from the notes field
/// - `title: ""` → clears the reminder title
///
/// We strip fields that carry empty/default values since the LLM wasn't trying
/// to change them. The `name` and `list` fields (identifiers) are never stripped.
fn sanitize_edit_args(args: &Value) -> Value {
    let mut args = args.clone();
    if let Some(obj) = args.as_object_mut() {
        // URL-only update: when the LLM is just setting a URL, strip everything
        // else to prevent accidental modifications to dates, recurrence, notes, etc.
        // The prompt says "only send name, list, and url" but LLMs echo back all fields.
        let has_url = obj.get("url").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
        if has_url {
            let keep_keys: &[&str] = &["name", "list", "url"];
            let keys_to_remove: Vec<String> = obj.keys()
                .filter(|k| !keep_keys.contains(&k.as_str()))
                .cloned()
                .collect();
            if !keys_to_remove.is_empty() {
                tracing::debug!(
                    stripped = ?keys_to_remove,
                    "URL-only edit: stripping non-essential fields to prevent unintended modifications"
                );
                for key in &keys_to_remove {
                    obj.remove(key);
                }
            }
            return args;
        }

        let due_date_empty = obj.get("due_date")
            .and_then(|v| v.as_str())
            .map(|s| s.is_empty())
            .unwrap_or(true);

        // Strip recurrence if due_date is empty — can't set recurrence without a due date,
        // and the LLM is just echoing the existing value back
        if due_date_empty {
            if obj.contains_key("recurrence") {
                tracing::debug!("Stripping echoed-back recurrence (due_date is empty)");
                obj.remove("recurrence");
            }
        }

        // Strip string fields that are empty — the LLM is echoing back defaults,
        // not intentionally clearing them. Identifiers (name, list) are excluded.
        const STRIPPABLE_STRINGS: &[&str] = &[
            "title", "notes", "new_list", "location", "start_date", "priority",
        ];
        for field in STRIPPABLE_STRINGS {
            if let Some(val) = obj.get(*field) {
                let is_empty = val.as_str().map(|s| s.is_empty()).unwrap_or(false);
                // Also treat "none" priority as a no-op echo-back
                let is_noop = *field == "priority"
                    && val.as_str().map(|s| s == "none").unwrap_or(false);
                if is_empty || is_noop {
                    obj.remove(*field);
                }
            }
        }

        // Strip complex fields that are all-zeros/empty (LLM echo-back defaults)
        if let Some(loc) = obj.get("location_alarm") {
            let is_default = loc.get("latitude").and_then(|v| v.as_f64()) == Some(0.0)
                && loc.get("longitude").and_then(|v| v.as_f64()) == Some(0.0);
            if is_default {
                tracing::debug!("Stripping default location_alarm (0,0)");
                obj.remove("location_alarm");
            }
        }
        if let Some(alarm) = obj.get("time_alarm") {
            let is_default = alarm.get("offset_minutes").and_then(|v| v.as_i64()) == Some(0);
            if is_default {
                tracing::debug!("Stripping default time_alarm (offset 0)");
                obj.remove("time_alarm");
            }
        }
    }
    args
}

/// Resolve iCloud Drive share links in attachment data to local file paths using mdfind.
/// Scoped to granted folders when access restrictions are active.
fn resolve_icloud_attachment_paths(result: String) -> String {
    let Ok(mut parsed) = serde_json::from_str::<Value>(&result) else {
        return result;
    };

    let mut changed = false;

    // Collect attachment arrays from various result shapes
    let attachment_paths: Vec<Vec<String>> = vec![];
    let _ = attachment_paths; // suppress warning

    // Process a single reminder's attachments
    fn process_attachments(attachments: &mut Vec<Value>, changed: &mut bool) {
        let granted = crate::access_store::AccessStore::enabled_folders();
        for att in attachments.iter_mut() {
            let Some(url) = att.get("url").and_then(|v| v.as_str()) else { continue };
            if !url.contains("icloud.com/iclouddrive") { continue; }
            if att.get("local_path").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()) {
                continue; // already resolved
            }

            // Extract filename from URL fragment
            let Some(fragment) = url.split('#').nth(1).filter(|f| !f.is_empty()) else { continue };
            let base_name = fragment.replace('_', " ");

            // Use mdfind scoped to granted folders (or common locations if no restrictions)
            if let Some(local_path) = mdfind_file(&base_name, &granted) {
                att.as_object_mut().unwrap().insert(
                    "local_path".to_string(),
                    Value::String(local_path),
                );
                *changed = true;
            }
        }
    }

    // Shape 1: {"reminder": {"attachments": [...]}}
    if let Some(reminder) = parsed.get_mut("reminder") {
        if let Some(atts) = reminder.get_mut("attachments").and_then(|v| v.as_array_mut()) {
            process_attachments(atts, &mut changed);
        }
    }

    // Shape 2: {"results"|"reminders": [{"attachments": [...]}, ...]}
    for key in &["results", "reminders"] {
        if let Some(items) = parsed.get_mut(*key).and_then(|v| v.as_array_mut()) {
            for item in items.iter_mut() {
                if let Some(atts) = item.get_mut("attachments").and_then(|v| v.as_array_mut()) {
                    process_attachments(atts, &mut changed);
                }
            }
        }
    }

    if changed {
        serde_json::to_string(&parsed).unwrap_or(result)
    } else {
        result
    }
}

/// Use mdfind (Spotlight) to locate a file by name, optionally scoped to granted folders.
fn mdfind_file(base_name: &str, granted_folders: &[crate::file_search::GrantedFolder]) -> Option<String> {
    use std::process::Command;

    let mut cmd = Command::new("mdfind");
    // kMDItemFSName matches the file system name
    cmd.arg(format!("kMDItemFSName == '{}*'", base_name.replace('\'', "\\'")));

    if !granted_folders.is_empty() {
        // Scope to granted folders only
        for folder in granted_folders {
            if folder.enabled {
                cmd.arg("-onlyin").arg(&folder.path);
            }
        }
    } else {
        // No folder restrictions — search common iCloud Drive locations
        let home = std::env::var("HOME").unwrap_or_default();
        for dir in &["Desktop", "Documents", "Downloads"] {
            cmd.arg("-onlyin").arg(format!("{}/{}", home, dir));
        }
        let icloud = format!("{}/Library/Mobile Documents/com~apple~CloudDocs", home);
        if std::path::Path::new(&icloud).exists() {
            cmd.arg("-onlyin").arg(&icloud);
        }
    }

    let output = cmd.output().ok()?;
    if !output.status.success() { return None; }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Return the first match
    stdout.lines().next().map(|s| s.to_string())
}

/// Post-filter search/get results to only include reminders from allowed lists.
/// Called when restrictions are active and no specific list was requested.
fn filter_results_by_allowed_lists(action: &str, args: &Value, result: String) -> String {
    // Only filter search results when restrictions are active and no list was specified
    if action != "search"
        || !AllowedRemindersStore::has_restrictions()
        || args.get("list").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).is_some()
    {
        return result;
    }

    let allowed = AllowedRemindersStore::enabled_names();
    if allowed.is_empty() {
        return result;
    }

    // Parse and filter the results array
    if let Ok(mut parsed) = serde_json::from_str::<Value>(&result) {
        let retain_fn = |item: &Value| -> bool {
            item.get("list")
                .and_then(|v| v.as_str())
                .map(|list| allowed.iter().any(|a| a.eq_ignore_ascii_case(list)))
                .unwrap_or(false)
        };

        // Filter results/reminders array and update count
        for key in &["results", "reminders"] {
            if let Some(arr) = parsed.get_mut(*key).and_then(|v| v.as_array_mut()) {
                arr.retain(retain_fn);
                let new_count = arr.len();
                if let Some(obj) = parsed.as_object_mut() {
                    obj.insert("count".to_string(), Value::Number(new_count.into()));
                }
                break;
            }
        }
        serde_json::to_string(&parsed).unwrap_or(result)
    } else {
        result
    }
}

pub fn execute_apple_reminders(action: &str, args: &Value) -> Result<String> {
    // Enforce access control BEFORE any execution path (FFI, helper, AppleScript)
    validate_action_access(action, args)?;

    // Sanitize edit args to strip LLM echo-back fields that cause validation errors
    let sanitized;
    let args = if matches!(action, "edit" | "create") {
        sanitized = sanitize_edit_args(args);
        &sanitized
    } else if matches!(action, "edit_batch" | "create_batch") {
        let mut batch_args = args.clone();
        if let Some(items) = batch_args.get_mut("items").and_then(|v| v.as_array_mut()) {
            for item in items.iter_mut() {
                *item = sanitize_edit_args(item);
            }
        }
        sanitized = batch_args;
        &sanitized
    } else {
        args
    };

    // 1. Try in-process EventKit FFI (works in sandboxed release builds)
    // Skip FFI for read operations — the Swift helper adds attachment data from CoreData SQLite
    // that EventKit doesn't expose via its public API.
    #[cfg(target_os = "macos")]
    if action != "open" && !matches!(action, "get" | "search" | "list") {
        match crate::eventkit_ffi::execute(action, args) {
            Ok(result) => {
                tracing::debug!(action, "EventKit FFI succeeded");
                // After a successful edit, set URLs via Shortcuts (EventKit drops them)
                let mut url_error = None;
                if matches!(action, "edit" | "edit_batch") {
                    let resolved_list = serde_json::from_str::<Value>(&result)
                        .ok()
                        .and_then(|v| v.get("list")?.as_str().map(String::from));
                    url_error = set_urls_via_shortcut(action, args, resolved_list.as_deref());
                }
                let result = if action == "list_lists" { filter_list_lists_result(result) } else { result };
                let result = filter_results_by_allowed_lists(action, args, result);
                let result = resolve_icloud_attachment_paths(result);
                // Append URL error to result so the LLM sees it
                let result = if let Some(err) = url_error {
                    format!("{}\n\nWARNING: {}", result, err)
                } else {
                    result
                };
                return Ok(result);
            }
            Err(e) => {
                tracing::debug!(action, error = %e, "EventKit FFI failed, trying helper");
            }
        }
    }

    // 2. Try Swift helper binary
    let helper = helper_binary();
    tracing::debug!(action, helper_found = helper.is_some(), helper_path = ?helper, "execute_apple_reminders");
    if helper.is_some() {
        if let Some(subcommand) = action_to_subcommand(action) {
            match run_helper(subcommand, args) {
                Ok(result) => {
                    // If helper returned 0 lists/reminders, the sandbox may be blocking
                    // EventKit XPC — fall through to AppleScript instead of returning empty.
                    if action == "list_lists" {
                        if let Ok(parsed) = serde_json::from_str::<Value>(&result) {
                            if parsed.get("count").and_then(|v| v.as_u64()) == Some(0) {
                                tracing::info!("Swift helper returned 0 lists for '{}', falling back to AppleScript", action);
                                // Fall through to AppleScript below
                            } else {
                                return Ok(filter_list_lists_result(result));
                            }
                        } else {
                            return Ok(filter_list_lists_result(result));
                        }
                    } else {
                        // After a successful edit, set URLs via Shortcuts (EventKit drops them)
                        let mut url_error = None;
                        if matches!(action, "edit" | "edit_batch") {
                            let resolved_list = serde_json::from_str::<Value>(&result)
                                .ok()
                                .and_then(|v| v.get("list")?.as_str().map(String::from));
                            url_error = set_urls_via_shortcut(action, args, resolved_list.as_deref());
                        }
                        let result = filter_results_by_allowed_lists(action, args, result);
                        let result = resolve_icloud_attachment_paths(result);
                        let result = if let Some(err) = url_error {
                            format!("{}\n\nWARNING: {}", result, err)
                        } else {
                            result
                        };
                        return Ok(result);
                    }
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    // If the reminder wasn't found in the specified list, retry across all lists.
                    // The LLM sometimes passes the wrong list name; the Swift helper supports
                    // searching all lists when the list field is omitted.
                    let bad_list = args.get("list")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    if bad_list.is_some()
                        && err_msg.contains("not found")
                        && RETRIABLE_ACTIONS.contains(&action)
                    {
                        // Retry across other allowed lists (respecting access control)
                        let allowed = AllowedRemindersStore::enabled_names();
                        let tried = bad_list.unwrap_or("");
                        tracing::warn!(
                            action, error = %err_msg, tried_list = tried,
                            "Reminder not found in specified list — trying other allowed lists"
                        );
                        for alt_list in &allowed {
                            if alt_list == tried { continue; }
                            let mut retry_args = args.clone();
                            retry_args["list"] = serde_json::Value::String(alt_list.clone());
                            match run_helper(subcommand, &retry_args) {
                                Ok(result) => {
                                    tracing::info!(action, list = %alt_list,
                                        "Cross-list retry succeeded");
                                    let mut url_error = None;
                                    if matches!(action, "edit" | "edit_batch") {
                                        let resolved_list = serde_json::from_str::<Value>(&result)
                                            .ok()
                                            .and_then(|v| v.get("list")?.as_str().map(String::from));
                                        url_error = set_urls_via_shortcut(action, args, resolved_list.as_deref());
                                    }
                                    let result = if let Some(err) = url_error {
                                        format!("{}\n\nWARNING: {}", result, err)
                                    } else {
                                        result
                                    };
                                    return Ok(result);
                                }
                                Err(_) => continue,
                            }
                        }
                        tracing::error!(
                                "TOOL FAILED: Swift helper '{}' — not found in any allowed list, falling back to AppleScript",
                                action
                        );
                    } else {
                        tracing::error!(
                            "TOOL FAILED: Swift helper '{}' — falling back to AppleScript: {}",
                            action, e
                        );
                    }
                }
            }
        }
    }

    // Fallback to AppleScript
    let result = match action {
        "list_lists" => list_reminder_lists(),
        "search" => search_reminders(args),
        "list" => list_reminders(args),
        "get" => get_reminder(args),
        "create" => create_reminder(args),
        "create_batch" => create_reminders_batch(args),
        "complete" => complete_reminder(args),
        "delete" => delete_reminder(args),
        "edit" => edit_reminder(args),
        "edit_batch" => edit_reminders_batch(args),
        "open" => open_reminders(args),
        "create_list" => create_reminder_list(args),
        "delete_list" => delete_reminder_list(args),
        _ => Err(anyhow!("Unknown Apple Reminders action: {}", action)),
    };

    // After a successful AppleScript edit, also set URLs via Shortcuts
    // (AppleScript path doesn't return list name — rely on args only)
    let url_error = if result.is_ok() && matches!(action, "edit" | "edit_batch") {
        set_urls_via_shortcut(action, args, None)
    } else {
        None
    };

    // Filter list_lists results to only show allowed lists
    if action == "list_lists" {
        return result.map(filter_list_lists_result);
    }

    result.map(|r| {
        let r = filter_results_by_allowed_lists(action, args, r);
        let r = resolve_icloud_attachment_paths(r);
        if let Some(err) = &url_error {
            format!("{}\n\nWARNING: {}", r, err)
        } else {
            r
        }
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_reminder_records() {
        let output = r#"RECORD_START
id: x-apple-reminder://ABC123
name: Buy groceries
list: Shopping
due_date: 2026-02-20T09:00:00Z
completed: false
priority: 0
snippet: Milk, eggs, bread
RECORD_END
RECORD_START
id: x-apple-reminder://DEF456
name: Call dentist
list: Personal
due_date: 2026-02-18T14:00:00Z
completed: true
priority: 1
snippet: Schedule annual checkup
RECORD_END"#;

        let records = parse_records(output).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "Buy groceries");
        assert_eq!(records[0].list, "Shopping");
        assert!(!records[0].completed);
        assert_eq!(records[0].priority, 0);
        assert_eq!(records[1].name, "Call dentist");
        assert!(records[1].completed);
        assert_eq!(records[1].priority, 1);
    }

    #[test]
    fn test_parse_reminder_records_empty() {
        let output = "";
        let records = parse_records(output).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn test_parse_reminder_records_error() {
        let output = "ERROR: Reminders application not available";
        let result = parse_records(output);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ERROR:"));
    }

    #[test]
    fn test_parse_reminder_detail() {
        let output = r#"name: Buy groceries
list: Shopping
due_date: 2026-02-20T09:00:00Z
completed: false
priority: 5
notes: Milk, eggs, bread, butter
created: 2026-02-15T10:00:00Z
modified: 2026-02-16T08:30:00Z"#;

        let detail = parse_reminder_detail(output).unwrap();
        assert_eq!(detail.name, "Buy groceries");
        assert_eq!(detail.list, "Shopping");
        assert_eq!(detail.due_date, "2026-02-20T09:00:00Z");
        assert!(!detail.completed);
        assert_eq!(detail.priority, 5);
        assert_eq!(detail.notes, "Milk, eggs, bread, butter");
        assert_eq!(detail.created, "2026-02-15T10:00:00Z");
    }

    #[test]
    fn test_parse_list_records() {
        let output = r#"RECORD_START
name: Shopping
id: x-apple-reminderlist://ABC
RECORD_END
RECORD_START
name: Work
id: x-apple-reminderlist://DEF
RECORD_END"#;

        let lists = parse_list_records(output).unwrap();
        assert_eq!(lists.len(), 2);
        assert_eq!(lists[0].name, "Shopping");
        assert_eq!(lists[0].id, "x-apple-reminderlist://ABC");
        assert_eq!(lists[1].name, "Work");
    }

    #[test]
    fn test_is_available_without_scripts() {
        // This test just ensures the function runs without panicking
        let _ = is_available();
    }

    #[test]
    fn test_allowed_store_behavior() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();
        AccessStore::inject_empty_cache();

        // Empty store → no restrictions → all allowed
        assert!(!AllowedRemindersStore::has_restrictions());
        assert!(AllowedRemindersStore::is_allowed(
            "nonexistent_test_list_xyz_98765"
        ));

        AccessStore::clear_cache();
    }

    #[test]
    fn test_allowed_store_keychain_roundtrip() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();
        // Keychain is bypassed in tests (returns empty/Ok by default).
        // Start with a clean empty cache.
        AccessStore::inject_empty_cache();

        let list = "test_roundtrip_list_abc";

        // Allow
        AllowedRemindersStore::allow_list(list).expect("allow should succeed");

        // Verify in list
        let all = AllowedRemindersStore::list_all().expect("list_all should succeed");
        assert!(all.iter().any(|l| l.name == list && l.enabled));

        // is_allowed
        assert!(AllowedRemindersStore::is_allowed(list));

        // Disallow
        AllowedRemindersStore::disallow_list(list).expect("disallow should succeed");

        // Verify gone
        let all = AllowedRemindersStore::list_all().expect("list_all should succeed");
        assert!(!all.iter().any(|l| l.name == list));

        AccessStore::clear_cache();
    }

    #[test]
    fn test_allowed_store_enabled_toggle() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();
        AccessStore::inject_empty_cache();

        let list = "test_toggle_list_def";

        AllowedRemindersStore::allow_list(list).expect("allow should succeed");

        // Disable
        AllowedRemindersStore::set_enabled(list, false).expect("set_enabled should succeed");
        let all = AllowedRemindersStore::list_all().expect("list_all should succeed");
        let entry = all.iter().find(|l| l.name == list).expect("should exist");
        assert!(!entry.enabled);

        // Re-enable
        AllowedRemindersStore::set_enabled(list, true).expect("set_enabled should succeed");
        let all = AllowedRemindersStore::list_all().expect("list_all should succeed");
        let entry = all.iter().find(|l| l.name == list).expect("should exist");
        assert!(entry.enabled);

        // Cleanup
        AllowedRemindersStore::disallow_list(list).expect("disallow should succeed");
        AccessStore::clear_cache();
    }

    #[test]
    fn test_validate_list_access() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();
        AccessStore::inject_empty_cache();

        // Empty store → no restrictions → access allowed
        let result = validate_list_access("nonexistent_test_list_xyz_98765");
        assert!(result.is_ok());

        AccessStore::clear_cache();
    }

    #[test]
    fn test_execute_unknown_action() {
        let result = execute_apple_reminders("nonexistent_action", &json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown"));
    }

    #[test]
    fn test_action_to_subcommand() {
        assert_eq!(action_to_subcommand("list_lists"), Some("list-lists"));
        assert_eq!(action_to_subcommand("search"), Some("search"));
        assert_eq!(action_to_subcommand("list"), Some("list"));
        assert_eq!(action_to_subcommand("get"), Some("get"));
        assert_eq!(action_to_subcommand("create"), Some("create"));
        assert_eq!(action_to_subcommand("create_batch"), Some("create-batch"));
        assert_eq!(action_to_subcommand("complete"), Some("complete"));
        assert_eq!(action_to_subcommand("delete"), Some("delete"));
        assert_eq!(action_to_subcommand("open"), Some("open"));
        assert_eq!(action_to_subcommand("create_list"), Some("create-list"));
        assert_eq!(action_to_subcommand("edit"), Some("edit"));
        assert_eq!(action_to_subcommand("edit_batch"), Some("edit-batch"));
        assert_eq!(action_to_subcommand("delete_list"), Some("delete-list"));
        assert_eq!(action_to_subcommand("bogus"), None);
    }

    #[test]
    fn test_helper_binary_lookup() {
        // Just ensure it doesn't panic — result depends on whether binary exists
        let _ = helper_binary();
    }

    #[test]
    fn test_url_to_path_bare_path() {
        let path = url_to_path("/Users/bwj/Library/Mobile Documents/report.pdf");
        assert_eq!(path, "/Users/bwj/Library/Mobile Documents/report.pdf");
    }

    #[test]
    fn test_url_to_path_file_scheme_encoded() {
        let path = url_to_path("file:///Users/bwj/Library/Mobile%20Documents/report.pdf");
        assert_eq!(path, "/Users/bwj/Library/Mobile Documents/report.pdf");
    }

    #[test]
    fn test_url_to_path_file_scheme_unencoded() {
        let path = url_to_path("file:///Users/bwj/Library/Mobile Documents/report.pdf");
        assert_eq!(path, "/Users/bwj/Library/Mobile Documents/report.pdf");
    }

    #[test]
    fn test_url_to_path_https_passthrough() {
        let path = url_to_path("https://example.com/page?q=hello world");
        assert_eq!(path, "https://example.com/page?q=hello world");
    }

    #[test]
    fn test_url_to_path_fixes_library_mobile_documents() {
        // LLM sometimes encodes the "/" in "Library/Mobile Documents" as %20,
        // producing "Library%20Mobile%20Documents" → "Library Mobile Documents".
        // url_to_path should fix this when the corrected parent directory exists.
        let bad_url = "file:///Users/bwj/Library%20Mobile%20Documents/com~apple~CloudDocs/test.pdf";
        let path = url_to_path(bad_url);
        // On a real Mac with iCloud, "Library/Mobile Documents" exists, so the fix applies.
        // If the directory doesn't exist (CI), it falls through to the unfixed path.
        let icloud_dir = std::path::Path::new("/Users/bwj/Library/Mobile Documents");
        if icloud_dir.exists() {
            assert_eq!(path, "/Users/bwj/Library/Mobile Documents/com~apple~CloudDocs/test.pdf");
        } else {
            // On systems without iCloud, the parent check fails so no fix is applied
            assert_eq!(path, "/Users/bwj/Library Mobile Documents/com~apple~CloudDocs/test.pdf");
        }
    }

    #[test]
    #[ignore] // Requires macOS Shortcuts app + "Set_Reminder_URL" shortcut + real reminder
    fn test_set_reminder_url_via_helper() {
        let result = set_reminder_url_via_helper(
            "TauriTasks",
            "Daily weather report",
            "file:///Users/bwj/Library/Mobile%20Documents/com~apple~CloudDocs/TauriTasksWorkspace/Weather%20report%20for%202026-02-23.pdf",
        );
        assert!(result.is_ok(), "set_reminder_url_via_helper failed: {:?}", result.err());
    }

    #[test]
    fn test_format_reminders_guidance_section() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();
        AccessStore::inject_empty_cache();

        // Empty store → no guidance → empty string
        let section = format_reminders_guidance_section();
        assert!(section.is_empty());

        AccessStore::clear_cache();
    }

    #[test]
    fn test_guidance_roundtrip() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();
        AccessStore::inject_empty_cache();

        let list = "test_guidance_list_ghi";

        // Create list, set guidance, verify, clear, verify, cleanup
        AllowedRemindersStore::allow_list(list).expect("allow should succeed");

        AllowedRemindersStore::set_guidance(list, Some("Shopping items and groceries".into()))
            .expect("set_guidance should succeed");

        let all = AllowedRemindersStore::list_all().expect("list_all should succeed");
        let entry = all.iter().find(|l| l.name == list).expect("should exist");
        assert_eq!(entry.guidance.as_deref(), Some("Shopping items and groceries"));

        // Clear guidance
        AllowedRemindersStore::set_guidance(list, None)
            .expect("clear guidance should succeed");

        let all = AllowedRemindersStore::list_all().expect("list_all should succeed");
        let entry = all.iter().find(|l| l.name == list).expect("should exist");
        assert!(entry.guidance.is_none());

        // Cleanup
        AllowedRemindersStore::disallow_list(list).expect("disallow should succeed");
        AccessStore::clear_cache();
    }

    #[test]
    fn test_priority_mapping() {
        assert_eq!(map_priority("high"), 1);
        assert_eq!(map_priority("HIGH"), 1);
        assert_eq!(map_priority("medium"), 5);
        assert_eq!(map_priority("med"), 5);
        assert_eq!(map_priority("low"), 9);
        assert_eq!(map_priority("none"), 0);
        assert_eq!(map_priority(""), 0);
        assert_eq!(map_priority("1"), 1);
        assert_eq!(map_priority("5"), 5);
        assert_eq!(map_priority("9"), 9);
        assert_eq!(map_priority("0"), 0);
    }

    #[test]
    fn test_validate_action_access_no_restrictions() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();
        AccessStore::inject_empty_cache();

        // No restrictions => all actions allowed, even without list
        assert!(validate_action_access("search", &json!({"query": "test"})).is_ok());
        assert!(validate_action_access("get", &json!({"name": "test"})).is_ok());
        assert!(validate_action_access("delete", &json!({"name": "test"})).is_ok());

        AccessStore::clear_cache();
    }

    #[test]
    fn test_validate_action_access_discovery_always_allowed() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();
        AccessStore::inject_empty_cache();
        AllowedRemindersStore::allow_list("Shopping").unwrap();

        // list_lists and open are always allowed (discovery)
        assert!(validate_action_access("list_lists", &json!({})).is_ok());
        assert!(validate_action_access("open", &json!({})).is_ok());

        AccessStore::clear_cache();
    }

    #[test]
    fn test_validate_action_access_rejects_unscoped_when_restricted() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();
        AccessStore::inject_empty_cache();
        AllowedRemindersStore::allow_list("Shopping").unwrap();

        // No list specified => rejected for all targeted actions
        assert!(validate_action_access("search", &json!({"query": "test"})).is_err());
        assert!(validate_action_access("get", &json!({"name": "test"})).is_err());
        assert!(validate_action_access("complete", &json!({"name": "test"})).is_err());
        assert!(validate_action_access("delete", &json!({"name": "test"})).is_err());
        assert!(validate_action_access("edit", &json!({"name": "test"})).is_err());
        assert!(validate_action_access("create", &json!({"name": "test"})).is_err());

        // With allowed list => passes
        assert!(validate_action_access("search", &json!({"query": "test", "list": "Shopping"})).is_ok());
        assert!(validate_action_access("get", &json!({"name": "test", "list": "Shopping"})).is_ok());

        // With disallowed list => rejected
        assert!(validate_action_access("search", &json!({"query": "test", "list": "Work"})).is_err());
        assert!(validate_action_access("get", &json!({"name": "test", "list": "Work"})).is_err());

        AccessStore::clear_cache();
    }

    #[test]
    fn test_validate_action_access_batch_enforcement() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();
        AccessStore::inject_empty_cache();
        AllowedRemindersStore::allow_list("Shopping").unwrap();

        // Batch with allowed list => ok
        let args = json!({"list": "Shopping", "items": [{"name": "Milk"}, {"name": "Eggs"}]});
        assert!(validate_action_access("create_batch", &args).is_ok());

        // Batch with disallowed list => rejected
        let args = json!({"list": "Work", "items": [{"name": "Task 1"}]});
        assert!(validate_action_access("create_batch", &args).is_err());

        // Per-item list overrides — disallowed item list rejected even if batch list is ok
        let args = json!({"list": "Shopping", "items": [{"name": "Task 1", "list": "Work"}]});
        assert!(validate_action_access("create_batch", &args).is_err());

        // No batch list and no item list => rejected
        let args = json!({"items": [{"name": "Task 1"}]});
        assert!(validate_action_access("create_batch", &args).is_err());

        AccessStore::clear_cache();
    }
}
