//! Runtime access filtering for MCP tool calls.
//!
//! Intercepts tool calls to enforce the access policy from `access.toml`.
//! Two strategies:
//!   - Pre-call check: reject calls that target restricted resources
//!   - Post-call filter: strip results that reference restricted resources

use crate::access_config::AccessConfig;
use serde_json::Value;

/// Write operations that require writable access.
const REMINDER_WRITE_TOOLS: &[&str] = &[
    "create_reminder",
    "create_reminders_batch",
    "edit_reminder",
    "edit_reminders_batch",
    "complete_reminder",
    "delete_reminder",
    "create_reminder_list",
    "delete_reminder_list",
];

const CONTACT_WRITE_TOOLS: &[&str] = &[
    "create_contact",
    "edit_contact",
    "delete_contact",
];

const FILE_WRITE_TOOLS: &[&str] = &["write_file"];

/// Result of an access check.
pub enum AccessCheck {
    /// Tool call is allowed, proceed normally.
    Allowed,
    /// Tool call is denied. Contains a user-facing error message.
    Denied(String),
    /// Tool call is allowed but results should be filtered.
    /// Contains the access config for post-call filtering.
    FilterResults,
}

/// Check whether a tool call is allowed before execution.
pub fn check_access(config: &AccessConfig, tool_name: &str, args: &Value) -> AccessCheck {
    // Reminder tools
    if is_reminder_tool(tool_name) {
        return check_reminder_access(config, tool_name, args);
    }

    // Contact tools
    if is_contact_tool(tool_name) {
        return check_contact_access(config, tool_name, args);
    }

    // Note tools
    if is_note_tool(tool_name) {
        return check_note_access(config, tool_name, args);
    }

    // File tools
    if is_file_tool(tool_name) {
        return check_file_access(config, tool_name, args);
    }

    // Unknown tool — allow (shouldn't happen with the allowlist filter)
    AccessCheck::Allowed
}

/// Filter tool results to remove restricted items.
pub fn filter_results(config: &AccessConfig, tool_name: &str, result: &str) -> String {
    // Parse as JSON; if it fails, return as-is
    let Ok(mut parsed) = serde_json::from_str::<Value>(result) else {
        return result.to_string();
    };

    if tool_name == "list_reminder_lists" {
        if let Some(allowed) = config.allowed_reminder_lists() {
            filter_json_array(&mut parsed, "lists", "name", &allowed);
            update_count(&mut parsed, "lists");
        }
    } else if tool_name == "list_reminders" {
        if let Some(allowed) = config.allowed_reminder_lists() {
            filter_json_array(&mut parsed, "reminders", "list", &allowed);
            update_count(&mut parsed, "reminders");
        }
    } else if tool_name == "search_reminders" {
        // search_reminders returns results in "results", not "reminders"
        if let Some(allowed) = config.allowed_reminder_lists() {
            filter_json_array(&mut parsed, "results", "list", &allowed);
            update_count(&mut parsed, "results");
        }
    } else if tool_name == "get_reminder" {
        // Single reminder retrieval — check list in the response
        if let Some(allowed) = config.allowed_reminder_lists() {
            let list = parsed
                .get("reminder")
                .and_then(|r| r.get("list"))
                .and_then(|l| l.as_str())
                .unwrap_or("");
            if !list.is_empty() && !allowed.contains(&list.to_lowercase()) {
                return serde_json::to_string(&serde_json::json!({
                    "error": format!(
                        "Access denied: reminder is in list \"{}\" which is not in the allowed list.",
                        list
                    )
                }))
                .unwrap_or_else(|_| result.to_string());
            }
        }
    } else if tool_name == "list_contact_groups" {
        if let Some(allowed) = config.allowed_contact_groups() {
            filter_json_array(&mut parsed, "sources", "name", &allowed);
            update_count(&mut parsed, "sources");
        }
    } else if tool_name == "list_contacts" || tool_name == "search_contacts" {
        // Contact results don't include group info, so we can't filter post-hoc.
        // Access is enforced via pre-call check on the group parameter.
    } else if tool_name == "list_notes" || tool_name == "search_notes" {
        if let Some(allowed) = config.allowed_notes_folders() {
            filter_json_array(&mut parsed, "notes", "folder", &allowed);
            update_count(&mut parsed, "notes");
        }
    } else if tool_name == "notes_smart_search" || tool_name == "notes_semantic_search" {
        if let Some(allowed) = config.allowed_notes_folders() {
            filter_json_array(&mut parsed, "results", "folder", &allowed);
            // Semantic search uses "count" at top level
            update_count_field(&mut parsed, "results", "count");
        }
    } else if tool_name == "notes_search_by_tag" {
        if let Some(allowed) = config.allowed_notes_folders() {
            filter_json_array(&mut parsed, "notes", "folder", &allowed);
            update_count(&mut parsed, "notes");
        }
    } else if tool_name == "get_note" {
        // Single note retrieval by ID — check folder in the response
        if let Some(allowed) = config.allowed_notes_folders() {
            let folder = parsed
                .get("note")
                .and_then(|n| n.get("folder"))
                .and_then(|f| f.as_str())
                .unwrap_or("");
            if !folder.is_empty() && !allowed.contains(&folder.to_lowercase()) {
                // Replace with an access-denied error instead of the note content
                return serde_json::to_string(&serde_json::json!({
                    "error": format!(
                        "Access denied: note is in folder \"{}\" which is not in the allowed list.",
                        folder
                    )
                }))
                .unwrap_or_else(|_| result.to_string());
            }
        }
    } else if tool_name == "file_search" {
        if let Some(allowed_folders) = config.allowed_file_folders() {
            // Filter results to only include files under allowed folders
            let new_count = if let Some(arr) = parsed.get_mut("results").and_then(|v| v.as_array_mut()) {
                arr.retain(|item| {
                    item.get("path")
                        .and_then(|v| v.as_str())
                        .map(|path| {
                            allowed_folders
                                .iter()
                                .any(|folder| crate::access_config::is_path_under(path, folder))
                        })
                        .unwrap_or(false)
                });
                Some(arr.len())
            } else {
                None
            };
            if let (Some(count), Some(obj)) = (new_count, parsed.as_object_mut()) {
                obj.insert("count".to_string(), Value::Number(count.into()));
            }
        }
    }

    serde_json::to_string(&parsed).unwrap_or_else(|_| result.to_string())
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn is_reminder_tool(name: &str) -> bool {
    matches!(
        name,
        "list_reminder_lists"
            | "list_reminders"
            | "search_reminders"
            | "get_reminder"
            | "create_reminder"
            | "create_reminders_batch"
            | "edit_reminder"
            | "edit_reminders_batch"
            | "complete_reminder"
            | "delete_reminder"
            | "open_reminders"
            | "create_reminder_list"
            | "delete_reminder_list"
    )
}

fn is_contact_tool(name: &str) -> bool {
    matches!(
        name,
        "list_contact_groups"
            | "list_contacts"
            | "search_contacts"
            | "get_contact"
            | "create_contact"
            | "edit_contact"
            | "delete_contact"
    )
}

fn is_note_tool(name: &str) -> bool {
    matches!(
        name,
        "search_notes"
            | "list_notes"
            | "get_note"
            | "open_note"
            | "notes_tags"
            | "notes_search_by_tag"
            | "notes_index"
            | "notes_semantic_search"
            | "notes_smart_search"
            | "notes_rebuild_index"
            | "notes_index_stats"
    )
}

fn check_reminder_access(config: &AccessConfig, tool_name: &str, args: &Value) -> AccessCheck {
    let restrictions = match &config.reminders {
        None => return AccessCheck::Allowed,
        Some(r) => r,
    };

    // open_reminders is always allowed (just opens the app)
    if tool_name == "open_reminders" {
        return AccessCheck::Allowed;
    }

    // List/search/get tools return filtered results
    if matches!(tool_name, "list_reminder_lists" | "list_reminders" | "search_reminders" | "get_reminder") {
        // If a specific list is requested, check it
        if let Some(list) = args.get("list").and_then(|l| l.as_str()) {
            if !restrictions.allowed_lists.iter().any(|l| l.eq_ignore_ascii_case(list)) {
                return AccessCheck::Denied(format!(
                    "Access denied: reminder list \"{}\" is not in the allowed list. \
                     Allowed lists: {}",
                    list,
                    format_list(&restrictions.allowed_lists),
                ));
            }
        }
        return AccessCheck::FilterResults;
    }

    // Tools that target a specific reminder (via `list` param)
    if let Some(list) = args.get("list").and_then(|l| l.as_str()) {
        if !restrictions.allowed_lists.iter().any(|l| l.eq_ignore_ascii_case(list)) {
            return AccessCheck::Denied(format!(
                "Access denied: reminder list \"{}\" is not in the allowed list.",
                list,
            ));
        }
        if REMINDER_WRITE_TOOLS.contains(&tool_name)
            && !restrictions.writable_lists.iter().any(|l| l.eq_ignore_ascii_case(list))
        {
            return AccessCheck::Denied(format!(
                "Access denied: reminder list \"{}\" is read-only.",
                list,
            ));
        }
    } else if REMINDER_WRITE_TOOLS.contains(&tool_name) {
        // Write tool without specifying a list — check if any writable list exists
        if restrictions.writable_lists.is_empty() {
            return AccessCheck::Denied(
                "Access denied: no writable reminder lists configured.".to_string(),
            );
        }
    }

    AccessCheck::Allowed
}

fn check_contact_access(config: &AccessConfig, tool_name: &str, args: &Value) -> AccessCheck {
    let restrictions = match &config.contacts {
        None => return AccessCheck::Allowed,
        Some(c) => c,
    };

    if restrictions.allowed_groups.is_empty() {
        return AccessCheck::Denied(
            "Access denied: no contact groups are configured for access.".to_string(),
        );
    }

    // List groups returns filtered results
    if tool_name == "list_contact_groups" {
        return AccessCheck::FilterResults;
    }

    // get_contact — requires group or container when restrictions are active,
    // since contact IDs alone can't be verified against the allowed group list.
    // The core layer also enforces this, but we reject early at the MCP layer.
    if tool_name == "get_contact" {
        let has_group = args.get("group").and_then(|g| g.as_str()).is_some_and(|s| !s.is_empty());
        let has_container = args
            .get("container")
            .and_then(|c| c.as_str())
            .is_some_and(|s| !s.is_empty());
        if !has_group && !has_container {
            return AccessCheck::Denied(
                "Access denied: a 'group' or 'container' parameter is required when \
                 contact restrictions are active."
                    .to_string(),
            );
        }
        if let Some(group) = args.get("group").and_then(|g| g.as_str()) {
            if !restrictions.allowed_groups.iter().any(|g| g.eq_ignore_ascii_case(group)) {
                return AccessCheck::Denied(format!(
                    "Access denied: contact group \"{}\" is not in the allowed list.",
                    group,
                ));
            }
        }
        if let Some(container) = args.get("container").and_then(|c| c.as_str()) {
            if !has_group
                && !restrictions
                    .allowed_groups
                    .iter()
                    .any(|g| g.eq_ignore_ascii_case(container))
            {
                return AccessCheck::Denied(format!(
                    "Access denied: contact container \"{}\" is not in the allowed list.",
                    container,
                ));
            }
        }
        return AccessCheck::Allowed;
    }

    // List/search contacts — if a group is specified, check it
    if matches!(tool_name, "list_contacts" | "search_contacts") {
        if let Some(group) = args.get("group").and_then(|g| g.as_str()) {
            if !restrictions.allowed_groups.iter().any(|g| g.eq_ignore_ascii_case(group)) {
                return AccessCheck::Denied(format!(
                    "Access denied: contact group \"{}\" is not in the allowed list.",
                    group,
                ));
            }
        }
        // Without a group param, contacts can't be filtered by group post-hoc.
        // The core layer enforces the group requirement when restrictions are active.
        return AccessCheck::Allowed;
    }

    // Write operations
    if CONTACT_WRITE_TOOLS.contains(&tool_name) {
        // Check group if specified
        if let Some(group) = args.get("group").and_then(|g| g.as_str()) {
            if !restrictions.writable_groups.iter().any(|g| g.eq_ignore_ascii_case(group)) {
                return AccessCheck::Denied(format!(
                    "Access denied: contact group \"{}\" is read-only.",
                    group,
                ));
            }
        } else if let Some(container) = args.get("container").and_then(|c| c.as_str()) {
            if !restrictions.writable_groups.iter().any(|g| g.eq_ignore_ascii_case(container)) {
                return AccessCheck::Denied(format!(
                    "Access denied: contact container \"{}\" is read-only.",
                    container,
                ));
            }
        } else if restrictions.writable_groups.is_empty() {
            return AccessCheck::Denied(
                "Access denied: no writable contact groups configured.".to_string(),
            );
        }
    }

    AccessCheck::Allowed
}

fn is_file_tool(name: &str) -> bool {
    matches!(name, "file_search" | "read_file" | "write_file")
}

fn check_file_access(config: &AccessConfig, tool_name: &str, args: &Value) -> AccessCheck {
    let restrictions = match &config.files {
        None => return AccessCheck::Allowed,
        Some(f) => f,
    };

    if restrictions.allowed_folders.is_empty() {
        return AccessCheck::Denied(
            "Access denied: no file folders are configured for access.".to_string(),
        );
    }

    // file_search returns filtered results
    if tool_name == "file_search" {
        return AccessCheck::FilterResults;
    }

    // read_file / write_file — check the path parameter
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .unwrap_or("");

    if path.is_empty() {
        return AccessCheck::Denied(
            "Access denied: no file path specified.".to_string(),
        );
    }

    // Check read access
    if !config.can_read_file(path) {
        return AccessCheck::Denied(format!(
            "Access denied: path \"{}\" is not under any allowed folder. \
             Allowed folders: {}",
            path,
            format_list(&restrictions.allowed_folders),
        ));
    }

    // Check write access for write tools
    if FILE_WRITE_TOOLS.contains(&tool_name) && !config.can_write_file(path) {
        return AccessCheck::Denied(format!(
            "Access denied: path \"{}\" is read-only. \
             Writable folders: {}",
            path,
            format_list(&restrictions.writable_folders),
        ));
    }

    AccessCheck::Allowed
}

fn check_note_access(config: &AccessConfig, tool_name: &str, args: &Value) -> AccessCheck {
    let restrictions = match &config.notes {
        None => return AccessCheck::Allowed,
        Some(n) => n,
    };

    if restrictions.allowed_folders.is_empty() {
        // Allow index stats and rebuild (they don't expose note content)
        if matches!(tool_name, "notes_index_stats" | "notes_rebuild_index") {
            return AccessCheck::Allowed;
        }
        return AccessCheck::Denied(
            "Access denied: no notes folders are configured for access.".to_string(),
        );
    }

    // Tools that return lists of notes — filter post-hoc
    if matches!(
        tool_name,
        "list_notes"
            | "search_notes"
            | "notes_search_by_tag"
            | "notes_tags"
            | "notes_index"
            | "notes_semantic_search"
            | "notes_smart_search"
    ) {
        // If a specific folder is requested, check it
        if let Some(folder) = args.get("folder").and_then(|f| f.as_str()) {
            if !restrictions.allowed_folders.iter().any(|f| f.eq_ignore_ascii_case(folder)) {
                return AccessCheck::Denied(format!(
                    "Access denied: notes folder \"{}\" is not in the allowed list.",
                    folder,
                ));
            }
        }
        return AccessCheck::FilterResults;
    }

    // create_note — write operation, validate folder access
    if tool_name == "create_note" {
        if let Some(folder) = args.get("folder").and_then(|f| f.as_str()) {
            if !restrictions.allowed_folders.iter().any(|f| f.eq_ignore_ascii_case(folder)) {
                return AccessCheck::Denied(format!(
                    "Access denied: notes folder \"{}\" is not in the allowed list.",
                    folder,
                ));
            }
            if !restrictions.writable_folders.is_empty()
                && !restrictions.writable_folders.iter().any(|f| f.eq_ignore_ascii_case(folder))
            {
                return AccessCheck::Denied(format!(
                    "Access denied: notes folder \"{}\" is read-only. \
                     Writable folders: {}",
                    folder,
                    format_list(&restrictions.writable_folders),
                ));
            }
        } else {
            // No folder specified — deny when restrictions are active
            return AccessCheck::Denied(
                "Access denied: a folder must be specified when notes restrictions are active. \
                 Use one of the allowed folders."
                    .to_string(),
            );
        }
        return AccessCheck::Allowed;
    }

    // get_note / open_note — retrieve by ID. Post-call filtering will verify
    // the note's folder against the allowed list before returning content.
    if matches!(tool_name, "get_note" | "open_note") {
        return AccessCheck::FilterResults;
    }

    // Index management tools
    if matches!(tool_name, "notes_index_stats" | "notes_rebuild_index") {
        return AccessCheck::Allowed;
    }

    AccessCheck::Allowed
}

/// Filter a JSON array field, keeping only items where `key` matches the allowed set.
fn filter_json_array(
    root: &mut Value,
    array_field: &str,
    key: &str,
    allowed: &std::collections::HashSet<String>,
) {
    if let Some(arr) = root.get_mut(array_field).and_then(|v| v.as_array_mut()) {
        arr.retain(|item| {
            item.get(key)
                .and_then(|v| v.as_str())
                .map(|s| allowed.contains(&s.to_lowercase()))
                .unwrap_or(false)
        });
    }
}

/// Update a "count" field to match the length of an array field (same name convention).
fn update_count(root: &mut Value, array_field: &str) {
    if let Some(arr) = root.get(array_field).and_then(|v| v.as_array()) {
        let count = arr.len();
        if let Some(obj) = root.as_object_mut() {
            obj.insert("count".to_string(), Value::Number(count.into()));
        }
    }
}

/// Update a specific count field name.
fn update_count_field(root: &mut Value, array_field: &str, count_field: &str) {
    if let Some(arr) = root.get(array_field).and_then(|v| v.as_array()) {
        let count = arr.len();
        if let Some(obj) = root.as_object_mut() {
            obj.insert(count_field.to_string(), Value::Number(count.into()));
        }
    }
}

fn format_list(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}
