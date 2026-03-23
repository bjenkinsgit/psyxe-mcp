//! Apple Contacts Integration via Contacts framework (Swift helper)
//!
//! Uses a compiled Swift binary (`contacts-helper`) that uses the Contacts
//! framework for direct access to the macOS address book. No AppleScript
//! fallback — this is a new integration, Swift helper only.
//!
//! Includes access-control via the unified AccessStore for restricting which
//! contact containers and groups the agent can access.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};


use crate::access_store::AccessStore;

// ============================================================================
// Data Structures
// ============================================================================

/// A contact record from search/list output
#[derive(Debug, Serialize, Deserialize)]
pub struct ContactRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub organization: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub email: String,
}

/// Full contact detail
#[derive(Debug, Serialize, Deserialize)]
pub struct ContactDetail {
    pub id: String,
    pub given_name: String,
    #[serde(default)]
    pub middle_name: String,
    pub family_name: String,
    #[serde(default)]
    pub nickname: String,
    pub name: String,
    #[serde(default)]
    pub organization: String,
    #[serde(default)]
    pub job_title: String,
    #[serde(default)]
    pub department: String,
    #[serde(default)]
    pub phones: Vec<LabeledValue>,
    #[serde(default)]
    pub emails: Vec<LabeledValue>,
    #[serde(default)]
    pub addresses: Vec<LabeledValue>,
    #[serde(default)]
    pub urls: Vec<LabeledValue>,
    #[serde(default)]
    pub social_profiles: Vec<SocialProfile>,
    #[serde(default)]
    pub instant_messages: Vec<InstantMessage>,
    #[serde(default)]
    pub has_image: bool,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub birthday: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InstantMessage {
    pub service: String,
    pub username: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LabeledValue {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SocialProfile {
    pub service: String,
    pub username: String,
    #[serde(default)]
    pub url: String,
}

/// A contact container or group from list-groups output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactSourceInfo {
    pub name: String,
    pub id: String,
    /// "container" or "group"
    #[serde(rename = "type")]
    pub source_type: String,
    pub count: usize,
    #[serde(default)]
    pub container_name: Option<String>,
}

// Re-export from access_store
pub use crate::access_store::AllowedContactSource;

// ============================================================================
// Swift Helper Binary
// ============================================================================

/// Cached path to the Swift contacts-helper binary (None = not yet found).
/// Uses Mutex instead of OnceLock so a failed lookup can be retried
/// (e.g. when SCRIPTS_DIR_OVERRIDE is set after the first call).
static HELPER_PATH: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Search for the compiled Swift helper binary in known locations.
fn find_helper_binary() -> Option<PathBuf> {
    // 1. SCRIPTS_DIR_OVERRIDE (used by Tauri bundled resources)
    if let Some(override_dir) = std::env::var_os("SCRIPTS_DIR_OVERRIDE") {
        let p = PathBuf::from(override_dir).join("contacts-helper");
        if p.is_file() {
            return Some(p);
        }
    }

    // 2. Dev paths relative to CWD
    let dev_paths = [
        "target/swift/contacts-helper",
        "swift/contacts-helper/.build/release/contacts-helper",
        "swift/contacts-helper/.build/debug/contacts-helper",
        "../../swift/contacts-helper/.build/release/contacts-helper",
        "../../swift/contacts-helper/.build/debug/contacts-helper",
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
            let p = exe_dir.join("contacts-helper");
            if p.is_file() {
                return Some(p);
            }
            // Bundled macOS app: Contents/MacOS/../Helpers/contacts-helper (preferred)
            let p = exe_dir.join("../Helpers/contacts-helper");
            if p.is_file() {
                return Some(p);
            }
            // Bundled macOS app: Contents/MacOS/../Resources/contacts-helper (legacy)
            let p = exe_dir.join("../Resources/contacts-helper");
            if p.is_file() {
                return Some(p);
            }
            // Dev build: target/release/ → target/swift/
            let p = exe_dir.join("../swift/contacts-helper");
            if p.is_file() {
                return Some(p);
            }
        }
    }

    // 4. CARGO_MANIFEST_DIR-relative (for cargo test/run)
    if let Some(manifest_dir) = std::env::var_os("CARGO_MANIFEST_DIR") {
        let base = PathBuf::from(manifest_dir);
        for suffix in &[
            "../../target/swift/contacts-helper",
            "../../swift/contacts-helper/.build/release/contacts-helper",
            "../../swift/contacts-helper/.build/debug/contacts-helper",
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

    if let Some(path) = find_helper_binary() {
        tracing::debug!(path = %path.display(), "Resolved contacts-helper binary");
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
        .ok_or_else(|| anyhow!("Swift contacts-helper binary not found"))?;

    let mut child = Command::new(binary)
        .arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn contacts-helper: {}", e))?;

    // Write JSON to stdin
    if let Some(mut stdin) = child.stdin.take() {
        let json_bytes = serde_json::to_vec(args)?;
        tracing::debug!(
            subcommand = subcommand,
            input = %String::from_utf8_lossy(&json_bytes),
            "contacts-helper: sending input"
        );
        stdin.write_all(&json_bytes)?;
        // stdin is dropped here, closing the pipe
    }

    let output = child
        .wait_with_output()
        .map_err(|e| anyhow!("Failed to wait for contacts-helper: {}", e))?;

    // Log helper diagnostics from stderr (even on success)
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        for line in stderr.lines() {
            if line.starts_with("[contacts-helper]") {
                tracing::info!("{}", line);
            }
        }
    }

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        // Extract error message from stderr. The stderr may contain diagnostic lines
        // (prefixed with [contacts-helper]) followed by an error JSON object.
        // Try each line as JSON to find the error message.
        let mut error_msg: Option<String> = None;
        for line in stderr.lines() {
            if line.starts_with('{') {
                if let Ok(err_json) = serde_json::from_str::<Value>(line) {
                    if let Some(msg) = err_json.get("message").and_then(|v| v.as_str()) {
                        error_msg = Some(msg.to_string());
                        break;
                    }
                }
            }
        }
        let code = output.status.code().unwrap_or(-1);
        if code == 2 {
            Err(anyhow!("Contacts access not granted"))
        } else if let Some(msg) = error_msg {
            Err(anyhow!("{}", msg))
        } else {
            Err(anyhow!(
                "contacts-helper exited with code {}: {}",
                code,
                stderr.trim()
            ))
        }
    }
}

// ============================================================================
// Allowed-Sources Store (thin facade over unified AccessStore)
// ============================================================================

/// Facade for managing which contact containers/groups the agent can access.
///
/// Delegates to [`AccessStore`] which holds all access-control data in a
/// single Keychain entry with a process-level cache.
///
/// When no entries exist, all sources are accessible (no restriction).
/// When entries exist, only sources marked as allowed+enabled are accessible.
/// Write operations additionally require the writable flag.
pub struct AllowedContactsStore;

impl AllowedContactsStore {
    /// Load all allowed contact source entries.
    pub fn list_all() -> Result<Vec<AllowedContactSource>, String> {
        AccessStore::list_contact_sources()
    }

    /// Check if a source name is allowed for read operations.
    pub fn is_allowed(name: &str) -> bool {
        AccessStore::is_contact_source_allowed(name)
    }

    /// Get the list of enabled source names.
    pub fn enabled_names() -> Vec<String> {
        AccessStore::contact_source_enabled_names()
    }

    /// Returns true if the allowed store has any entries.
    pub fn has_restrictions() -> bool {
        AccessStore::has_contact_restrictions()
    }

    /// Check if a source is writable.
    pub fn is_writable(name: &str) -> bool {
        AccessStore::is_contact_source_writable(name)
    }

    /// Add a source to the allowed set.
    pub fn allow_source(
        name: &str,
        source_type: &str,
        container_name: Option<&str>,
    ) -> Result<(), String> {
        AccessStore::allow_contact_source(name, source_type, container_name)
    }

    /// Remove a source from the allowed set.
    pub fn disallow_source(name: &str) -> Result<(), String> {
        AccessStore::disallow_contact_source(name)
    }

    /// Toggle the enabled state for a source.
    pub fn set_enabled(name: &str, enabled: bool) -> Result<(), String> {
        AccessStore::set_contact_source_enabled(name, enabled)
    }

    /// Set or clear the guidance prompt for a source.
    pub fn set_guidance(name: &str, guidance: Option<String>) -> Result<(), String> {
        AccessStore::set_contact_source_guidance(name, guidance)
    }

    /// Set the writable flag for a source.
    pub fn set_writable(name: &str, writable: bool) -> Result<(), String> {
        AccessStore::set_contact_source_writable(name, writable)
    }
}

// ============================================================================
// Result Filtering
// ============================================================================

/// Filter a `list-groups` JSON result to only include allowed sources.
/// When no restrictions are configured, returns the result unchanged.
fn filter_list_groups_result(result: String) -> String {
    if !AllowedContactsStore::has_restrictions() {
        return result;
    }
    let allowed = AllowedContactsStore::enabled_names();
    let Ok(mut parsed) = serde_json::from_str::<Value>(&result) else {
        return result;
    };
    if let Some(sources) = parsed.get_mut("sources").and_then(|v| v.as_array_mut()) {
        sources.retain(|item| {
            item.get("name")
                .and_then(|n| n.as_str())
                .map(|name| allowed.iter().any(|a| a.eq_ignore_ascii_case(name)))
                .unwrap_or(false)
        });
        parsed["count"] = serde_json::json!(sources.len());
    }
    serde_json::to_string(&parsed).unwrap_or(result)
}

// ============================================================================
// Access Validation
// ============================================================================

/// Validate that a source (container or group) is allowed for read access.
/// If no restrictions are configured, all access is allowed.
fn validate_source_access(args: &Value) -> Result<()> {
    if !AllowedContactsStore::has_restrictions() {
        return Ok(());
    }

    let container = args.get("container").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
    let group = args.get("group").and_then(|v| v.as_str()).filter(|s| !s.is_empty());

    // When restrictions are active, a scope (container or group) MUST be specified.
    // Without this, the LLM can bypass restrictions by omitting the scope parameter.
    if container.is_none() && group.is_none() {
        return Err(anyhow!(
            "Access restrictions are active. Specify a 'container' or 'group' from the allowed set: {:?}",
            AllowedContactsStore::enabled_names()
        ));
    }

    // When an allowed group is specified, skip the container check — the group's
    // parent container is implicitly authorized (the Swift helper resolves the
    // container from the group). This prevents the common case where the LLM
    // specifies both group="people_w_phones" and container="iCloud", but only
    // the group is in the allowed list.
    let group_is_allowed = group.map(|g| AllowedContactsStore::is_allowed(g)).unwrap_or(false);

    if let Some(container) = container {
        if !group_is_allowed && !AllowedContactsStore::is_allowed(container) {
            return Err(anyhow!(
                "Access denied: container '{}' is not in the allowed contact sources",
                container
            ));
        }
    }
    if let Some(group) = group {
        if !AllowedContactsStore::is_allowed(group) {
            return Err(anyhow!(
                "Access denied: group '{}' is not in the allowed contact sources",
                group
            ));
        }
    }

    Ok(())
}

/// Validate that a source is allowed for write access (requires writable flag).
fn validate_source_writable(args: &Value) -> Result<()> {
    if !AllowedContactsStore::has_restrictions() {
        return Ok(());
    }

    // When a writable group is specified, skip the container write check —
    // the group's parent container is implicitly authorized for writes.
    let group_is_writable = args.get("group").and_then(|v| v.as_str())
        .is_some_and(|g| !g.is_empty() && AllowedContactsStore::is_writable(g));

    if let Some(container) = args.get("container").and_then(|v| v.as_str()) {
        if !container.is_empty() && !group_is_writable && !AllowedContactsStore::is_writable(container) {
            return Err(anyhow!(
                "Write access denied: container '{}' is read-only",
                container
            ));
        }
    }
    if let Some(group) = args.get("group").and_then(|v| v.as_str()) {
        if !group.is_empty() && !AllowedContactsStore::is_writable(group) {
            return Err(anyhow!(
                "Write access denied: group '{}' is read-only",
                group
            ));
        }
    }

    // For write ops without a specific source, check if ANY source is writable
    let has_scope = args.get("container").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty())
        || args.get("group").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty());
    if !has_scope {
        let enabled = AllowedContactsStore::enabled_names();
        let any_writable = enabled.iter().any(|name| AllowedContactsStore::is_writable(name));
        if !any_writable {
            return Err(anyhow!(
                "Write access denied: no writable contact sources configured"
            ));
        }
    }

    Ok(())
}

// ============================================================================
// Availability Check
// ============================================================================

/// Check if Apple Contacts integration is available (macOS only).
/// Returns true if the Swift helper binary is found.
pub fn is_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        helper_binary().is_some()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Fetch all contact containers and groups (for the source picker).
/// This bypasses access restrictions — discovery must work for the picker.
pub fn fetch_all_contact_sources() -> Result<Vec<ContactSourceInfo>, String> {
    let result = run_helper("list-groups", &json!({}))
        .map_err(|e| e.to_string())?;
    let parsed: Value = serde_json::from_str(&result)
        .map_err(|e| format!("Failed to parse list-groups output: {}", e))?;
    let sources: Vec<ContactSourceInfo> = parsed
        .get("sources")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    Ok(sources)
}

/// Execute an Apple Contacts action via the Swift helper.
///
/// This is the main dispatch function called by the agent loop.
/// Handles access control validation before delegating to the helper.
pub fn execute_apple_contacts(action: &str, args: &Value) -> Result<String> {
    let helper = helper_binary();
    tracing::info!(action, ?args, helper_found = helper.is_some(), "execute_apple_contacts");

    if helper.is_none() {
        return Err(anyhow!("Apple Contacts integration not available: contacts-helper binary not found"));
    }

    // For create/edit: if the LLM sent only `container` without `group`, but a writable
    // group exists, inject the group automatically. This prevents Cocoa error 134092
    // (cross-container membership failure) and ensures the contact lands in the right group.
    // This MUST run before access validation so the injected group satisfies the access check.
    let mut patched_args = args.clone();
    if matches!(action, "create" | "edit") && AllowedContactsStore::has_restrictions() {
        let has_group = args.get("group").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty());
        if !has_group {
            if let Some(writable_group) = find_first_writable_group() {
                tracing::info!(
                    "Auto-injecting group='{}' for {} (LLM omitted group parameter)",
                    writable_group, action
                );
                patched_args["group"] = json!(writable_group);
                // Remove container to avoid confusion — group branch resolves it
                patched_args.as_object_mut().map(|m| m.remove("container"));
            }
        }
    }

    if patched_args != *args {
        tracing::info!(action, patched_args = %patched_args, "contacts args patched before helper call");
    }

    // Access control validation (runs on patched args so auto-injected group is checked)
    match action {
        // list-groups is always allowed (discovery for the picker)
        "list-groups" => {}
        // Read operations: validate source access
        "search" | "list" | "get" => {
            validate_source_access(&patched_args)?;
        }
        // Write operations: validate both access and writable
        "create" | "edit" | "delete" => {
            validate_source_access(&patched_args)?;
            validate_source_writable(&patched_args)?;
        }
        _ => {}
    }

    let result = run_helper(action, &patched_args);

    // Filter list-groups to only show allowed sources in agent/chat path
    if action == "list-groups" {
        return result.map(filter_list_groups_result);
    }

    result
}

/// Find the first writable group in the allowed contact sources.
fn find_first_writable_group() -> Option<String> {
    let sources = AllowedContactsStore::list_all().ok()?;
    sources.iter()
        .find(|s| s.enabled && s.writable && s.source_type == "group")
        .map(|s| s.name.clone())
}

/// Format a `## Allowed Contact Sources` section for the agent system prompt.
pub fn format_contacts_prompt_section() -> String {
    AccessStore::format_contacts_guidance_section()
}

/// Format a short suffix for contacts tool descriptions listing allowed sources.
/// Returns empty string when no restrictions are configured.
pub fn format_allowed_sources_suffix() -> String {
    if !AllowedContactsStore::has_restrictions() {
        return String::new();
    }
    let sources = AllowedContactsStore::list_all().unwrap_or_default();
    let entries: Vec<String> = sources.iter()
        .filter(|s| s.enabled)
        .map(|s| {
            let access = if s.writable { "rw" } else { "ro" };
            let kind = if s.source_type == "group" { "group" } else { "container" };
            format!("{}({},{})", s.name, kind, access)
        })
        .collect();
    if entries.is_empty() {
        return String::new();
    }
    format!(" [Allowed sources: {}. Use `group` parameter for groups, not `container`.]", entries.join(", "))
}

/// Check whether an email address is in the designated email allowlist group.
///
/// Returns `Ok(true)` if:
/// - No allowlist group is configured (open access)
/// - The contacts-helper binary is not available (fail open)
/// - The email matches a contact in the allowlist group
///
/// Returns `Ok(false)` if the email is not found in the allowlist group.
pub fn is_email_allowed(email: &str) -> Result<bool> {
    let group = match AccessStore::get_email_allowlist_group() {
        Some(g) => g,
        None => return Ok(true), // No allowlist = open access
    };

    if !is_available() {
        tracing::warn!("contacts-helper not available, allowing email through (fail open)");
        return Ok(true);
    }

    let result = run_helper("search", &json!({
        "query": email,
        "group": group,
    }))?;

    let parsed: Value = serde_json::from_str(&result)
        .map_err(|e| anyhow!("Failed to parse search result: {}", e))?;

    let count = parsed
        .get("count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    if count > 0 {
        tracing::debug!(email = %email, group = %group, count = count, "Email found in allowlist group");
        Ok(true)
    } else {
        tracing::info!(email = %email, group = %group, "Email NOT found in allowlist group");
        Ok(false)
    }
}

/// Check whether a phone number is in the designated email allowlist group.
///
/// Uses the same allowlist group as `is_email_allowed` — the group contains
/// people (contacts), not just emails. The contacts-helper `search` subcommand
/// matches phone numbers as well.
///
/// Returns `Ok(true)` if:
/// - No allowlist group is configured (open access)
/// - The contacts-helper binary is not available (fail open)
/// - The phone matches a contact in the allowlist group
///
/// Returns `Ok(false)` if the phone is not found in the allowlist group.
pub fn is_phone_allowed(phone: &str) -> Result<bool> {
    let group = match AccessStore::get_email_allowlist_group() {
        Some(g) => g,
        None => return Ok(true), // No allowlist = open access
    };

    if !is_available() {
        tracing::warn!("contacts-helper not available, allowing phone through (fail open)");
        return Ok(true);
    }

    // Strip phone to digits only for search — the contacts-helper normalizes
    // stored numbers too, but passing digits avoids format mismatch issues.
    let digits: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();
    let search_query = if digits.len() >= 7 { &digits } else { phone };

    let result = run_helper("search", &json!({
        "query": search_query,
        "group": group,
    }))?;

    tracing::debug!(
        phone = %phone,
        group = %group,
        result_preview = %&result[..result.len().min(200)],
        "Phone allowlist search raw result"
    );

    let parsed: Value = serde_json::from_str(&result)
        .map_err(|e| anyhow!("Failed to parse search result: {}", e))?;

    let count = parsed
        .get("count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    if count > 0 {
        tracing::info!(phone = %phone, group = %group, count = count, "Phone found in allowlist group");
        Ok(true)
    } else {
        tracing::info!(phone = %phone, group = %group, "Phone NOT found in allowlist group");
        Ok(false)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contact_record_deserialize() {
        let json = r#"{"id":"abc123","name":"John Doe","organization":"Acme","phone":"+1234567890","email":"john@example.com"}"#;
        let record: ContactRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.id, "abc123");
        assert_eq!(record.name, "John Doe");
        assert_eq!(record.organization, "Acme");
        assert_eq!(record.phone, "+1234567890");
        assert_eq!(record.email, "john@example.com");
    }

    #[test]
    fn test_contact_detail_deserialize() {
        let json = r#"{
            "id": "abc123",
            "given_name": "John",
            "family_name": "Doe",
            "name": "John Doe",
            "organization": "Acme Corp",
            "job_title": "Engineer",
            "department": "R&D",
            "phones": [{"label": "mobile", "value": "+1234567890"}],
            "emails": [{"label": "work", "value": "john@acme.com"}],
            "addresses": [],
            "urls": [],
            "social_profiles": [],
            "has_image": false,
            "note": "Test note",
            "birthday": "1990-06-15"
        }"#;
        let detail: ContactDetail = serde_json::from_str(json).unwrap();
        assert_eq!(detail.given_name, "John");
        assert_eq!(detail.family_name, "Doe");
        assert_eq!(detail.job_title, "Engineer");
        assert_eq!(detail.phones.len(), 1);
        assert_eq!(detail.phones[0].value, "+1234567890");
        assert_eq!(detail.birthday.as_deref(), Some("1990-06-15"));
    }

    #[test]
    fn test_contact_source_info_deserialize() {
        let json = r#"{"name":"iCloud","id":"container-1","type":"container","count":42}"#;
        let info: ContactSourceInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.name, "iCloud");
        assert_eq!(info.source_type, "container");
        assert_eq!(info.count, 42);
        assert!(info.container_name.is_none());
    }

    #[test]
    fn test_contact_source_info_with_container() {
        let json = r#"{"name":"Family","id":"group-1","type":"group","count":5,"container_name":"iCloud"}"#;
        let info: ContactSourceInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.name, "Family");
        assert_eq!(info.source_type, "group");
        assert_eq!(info.container_name.as_deref(), Some("iCloud"));
    }

    #[test]
    fn test_validate_source_access_no_restrictions() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();
        AccessStore::inject_empty_cache();

        // No restrictions => all access allowed
        let args = json!({"container": "iCloud"});
        assert!(validate_source_access(&args).is_ok());

        AccessStore::clear_cache();
    }

    #[test]
    fn test_validate_source_access_with_restrictions() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();

        // Set up restrictions
        AccessStore::inject_empty_cache();
        AccessStore::allow_contact_source("iCloud", "container", None).unwrap();

        // Allowed source
        let args = json!({"container": "iCloud"});
        assert!(validate_source_access(&args).is_ok());

        // Denied source
        let args = json!({"container": "Google"});
        assert!(validate_source_access(&args).is_err());

        AccessStore::clear_cache();
    }

    #[test]
    fn test_validate_source_access_unscoped_rejected() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();

        // Set up restrictions (iCloud allowed)
        AccessStore::inject_empty_cache();
        AccessStore::allow_contact_source("iCloud", "container", None).unwrap();

        // No container/group specified => rejected when restrictions are active
        let args = json!({"query": "John"});
        assert!(validate_source_access(&args).is_err());

        // Empty container also rejected
        let args = json!({"container": ""});
        assert!(validate_source_access(&args).is_err());

        // With valid container => allowed
        let args = json!({"container": "iCloud", "query": "John"});
        assert!(validate_source_access(&args).is_ok());

        AccessStore::clear_cache();
    }

    #[test]
    fn test_validate_source_writable() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();

        AccessStore::inject_empty_cache();
        AccessStore::allow_contact_source("iCloud", "container", None).unwrap();
        // iCloud is read-only by default

        let args = json!({"container": "iCloud"});
        assert!(validate_source_writable(&args).is_err());

        // Make it writable
        AccessStore::set_contact_source_writable("iCloud", true).unwrap();
        assert!(validate_source_writable(&args).is_ok());

        AccessStore::clear_cache();
    }

    #[test]
    fn test_allowed_group_bypasses_container_check() {
        let _lock = crate::access_store::CACHE_TEST_MUTEX.lock().unwrap();

        // Only group "people_w_phones" is allowed (not container "iCloud")
        AccessStore::inject_empty_cache();
        AccessStore::allow_contact_source("people_w_phones", "group", Some("iCloud")).unwrap();
        AccessStore::set_contact_source_writable("people_w_phones", true).unwrap();

        // Group alone => allowed
        let args = json!({"group": "people_w_phones"});
        assert!(validate_source_access(&args).is_ok());
        assert!(validate_source_writable(&args).is_ok());

        // Group + container => allowed (group takes precedence)
        let args = json!({"group": "people_w_phones", "container": "iCloud"});
        assert!(validate_source_access(&args).is_ok());
        assert!(validate_source_writable(&args).is_ok());

        // Container alone => denied (not in allowed list)
        let args = json!({"container": "iCloud"});
        assert!(validate_source_access(&args).is_err());

        AccessStore::clear_cache();
    }
}
