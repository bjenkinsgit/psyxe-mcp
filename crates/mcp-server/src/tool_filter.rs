//! Tool filtering and conversion from ToolDef to MCP Tool format.

use psyxe_mcp_core::tools::ToolDef;
use rmcp::model::Tool;
use serde_json::json;
use std::collections::HashSet;

/// Tools to expose via MCP — Apple ecosystem tools only.
/// These are macOS-native capabilities that Claude Code cannot access on its own.
/// No API keys required — all use AppleScript, EventKit, or Contacts framework.
const ALLOWED_TOOLS: &[&str] = &[
    // Apple Notes (AppleScript + optional local BERT semantic search)
    "search_notes",
    "list_notes",
    "get_note",
    "open_note",
    "create_note",
    "notes_tags",
    "notes_search_by_tag",
    "notes_index",
    // Apple Reminders (Swift EventKit helper)
    "list_reminder_lists",
    "search_reminders",
    "list_reminders",
    "get_reminder",
    "create_reminder",
    "create_reminders_batch",
    "complete_reminder",
    "delete_reminder",
    "edit_reminder",
    "edit_reminders_batch",
    "open_reminders",
    "create_reminder_list",
    "delete_reminder_list",
    // Apple Contacts (Swift Contacts framework helper)
    "list_contact_groups",
    "search_contacts",
    "list_contacts",
    "get_contact",
    "create_contact",
    "edit_contact",
    "delete_contact",
    // File system (scoped to allowed folders via access.toml)
    "file_search",
    "read_file",
    "write_file",
];

/// Build the allowlist as a HashSet for fast lookup.
pub fn allowed_set() -> HashSet<&'static str> {
    ALLOWED_TOOLS.iter().copied().collect()
}

/// Convert a ToolDef (from tools.json) to an MCP Tool.
pub fn tooldef_to_mcp_tool(td: &ToolDef) -> Tool {
    // The ToolDef.parameters field is already a JSON Schema object with
    // "type": "object", "properties": {...}, "required": [...]
    let schema = td
        .parameters
        .as_object()
        .cloned()
        .unwrap_or_default();
    Tool::new(td.name.clone(), td.description.clone(), schema)
}

/// Build MCP Tool definitions for memvid-powered tools (not in tools.json).
#[cfg(feature = "memvid")]
fn memvid_mcp_tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "notes_semantic_search",
            "Semantic BERT-based search across Apple Notes. Returns notes ranked by cosine similarity. IMPORTANT: Always call this tool for each new query — do not reuse previous results, as Notes content may have changed since the last search.",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query for semantic matching"
                    },
                    "top_k": {
                        "type": "integer",
                        "description": "Number of results to return (default: 5)"
                    }
                },
                "required": ["query"]
            }).as_object().unwrap().clone(),
        ),
        Tool::new(
            "notes_smart_search",
            "Search Apple Notes: uses semantic search if index exists, falls back to AppleScript text search. IMPORTANT: Always call this tool for each new query — do not reuse previous results, as Notes content may have changed since the last search.",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    }
                },
                "required": ["query"]
            }).as_object().unwrap().clone(),
        ),
        Tool::new(
            "notes_rebuild_index",
            "Rebuild the memvid semantic search index for Apple Notes. Encodes all notes with BERT embeddings.",
            json!({
                "type": "object",
                "properties": {}
            }).as_object().unwrap().clone(),
        ),
        Tool::new(
            "notes_index_stats",
            "Get statistics about the memvid semantic search index (note count, staleness, last update).",
            json!({
                "type": "object",
                "properties": {}
            }).as_object().unwrap().clone(),
        ),
    ]
}

/// Filter tools.json definitions to only the allowed set, then convert to MCP Tools.
/// When the `memvid` feature is enabled, also includes semantic search tools.
pub fn build_mcp_tools(tool_defs: &[ToolDef]) -> Vec<Tool> {
    let allowed = allowed_set();
    let mut tools: Vec<Tool> = tool_defs
        .iter()
        .filter(|td| allowed.contains(td.name.as_str()))
        .map(tooldef_to_mcp_tool)
        .collect();

    #[cfg(feature = "memvid")]
    tools.extend(memvid_mcp_tools());

    tools
}
