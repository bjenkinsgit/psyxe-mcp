//! Tool dispatch — routes tool names to their implementation modules.
//!
//! This module handles dispatch for all tools that live in mcp-core
//! (Apple integrations, file ops, web tools, etc.). The proprietary
//! `prolog-router-core` crate wraps this with additional dispatchers
//! for core-only tools (scheduler, playbooks, brokerage, etc.).

use crate::tools::ToolExecutor;
use serde_json::Value;

/// Execute a tool by name, returning (success, result_string).
///
/// Handles all mcp-core tools: Apple Notes/Reminders/Contacts/Messages/Weather,
/// file operations, web search, image generation, etc.
///
/// If the tool is not recognized, returns `None` so the caller can try
/// additional dispatchers or fall back.
pub fn dispatch_tool(tool: &str, args: &Value, executor: Option<&ToolExecutor>) -> Option<(bool, String)> {
    use crate::apple_contacts;
    use crate::apple_maps;
    use crate::apple_notes;
    use crate::apple_reminders;
    use crate::apple_weather;
    use crate::brave_search;
    use crate::gemini_image;
    use crate::nominatim;
    use crate::open_meteo;

    let _subsystem_guard = crate::tool_semaphore::acquire_tool_lock(tool);

    tracing::info!(tool, "├─ Executing tool");

    // time_now — host-implemented clock tool
    if tool == "time_now" {
        let now = chrono::Local::now();
        let result = serde_json::json!({
            "date": now.format("%Y-%m-%d").to_string(),
            "time": now.format("%H:%M:%S").to_string(),
            "timezone": now.format("%Z").to_string(),
            "utc_offset": now.format("%:z").to_string(),
            "weekday": now.format("%A").to_string(),
            "unix_timestamp": now.timestamp(),
            "iso8601": now.to_rfc3339(),
        });
        return Some((true, serde_json::to_string_pretty(&result).unwrap()));
    }

    // notify_human — send iMessage
    if tool == "notify_human" {
        let recipient = args.get("recipient").and_then(|v| v.as_str()).unwrap_or("");
        let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("");
        return Some(match crate::apple_messages::send_message(recipient, message) {
            Ok(result) => (true, result),
            Err(e) => (false, format!("Failed to send message: {}", e)),
        });
    }

    // wait_for_human_reply — poll chat.db for reply
    if tool == "wait_for_human_reply" {
        let recipient = args.get("recipient").and_then(|v| v.as_str()).unwrap_or("");
        let timeout = args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(120);
        return Some(match crate::apple_messages::wait_for_reply(recipient, timeout) {
            Ok(reply) => (true, serde_json::json!({ "reply": reply }).to_string()),
            Err(e) => (false, format!("Failed to receive reply: {}", e)),
        });
    }

    // Unified file search (Notes + local files via mdfind)
    if tool == "file_search" {
        return Some(match crate::file_search::FileSearchParams::from_args(args) {
            Ok(params) => match crate::file_search::execute_file_search(&params) {
                Ok(result) => (true, result),
                Err(e) => (false, format!("file_search error: {}", e)),
            },
            Err(e) => (false, format!("file_search param error: {}", e)),
        });
    }

    // Read file from workspace or granted folders
    if tool == "read_file" {
        return Some(match crate::file_ops::ReadFileParams::from_args(args) {
            Ok(params) => match crate::file_ops::execute_read_file(&params) {
                Ok(result) => (true, result),
                Err(e) => (false, format!("read_file error: {}", e)),
            },
            Err(e) => (false, format!("read_file param error: {}", e)),
        });
    }

    // Write file to workspace
    if tool == "write_file" {
        return Some(match crate::file_ops::WriteFileParams::from_args(args) {
            Ok(params) => match crate::file_ops::execute_write_file(&params) {
                Ok(result) => (true, result),
                Err(e) => (false, format!("write_file error: {}", e)),
            },
            Err(e) => (false, format!("write_file param error: {}", e)),
        });
    }

    // Create PDF from text or HTML content
    if tool == "create_pdf" {
        if crate::pdf_generator::is_available() {
            let path_str = args["path"]
                .as_str()
                .or_else(|| args["output_path"].as_str())
                .unwrap_or("output.pdf");
            let content = args["content"].as_str().unwrap_or("");
            let title = args.get("title").and_then(|v| v.as_str());
            return Some(match crate::file_ops::validate_write_path(path_str) {
                Ok(validated) => match crate::pdf_generator::execute_create_pdf(
                    &validated.display().to_string(),
                    content,
                    title,
                ) {
                    Ok(result) => (true, result),
                    Err(e) => (false, format!("create_pdf error: {}", e)),
                },
                Err(e) => (false, format!("create_pdf path error: {}", e)),
            });
        } else {
            return Some((false, "create_pdf is not available (macOS with pdf-helper binary required)".to_string()));
        }
    }

    // Apply unified diff to workspace files
    if tool == "apply_patch" {
        return Some(match crate::file_ops::ApplyPatchParams::from_args(args) {
            Ok(params) => match crate::file_ops::execute_apply_patch(&params) {
                Ok(result) => (true, result),
                Err(e) => (false, format!("apply_patch error: {}", e)),
            },
            Err(e) => (false, format!("apply_patch param error: {}", e)),
        });
    }

    // Apple Notes tools
    let notes_tools = [
        "search_notes", "list_notes", "get_note", "open_note",
        "notes_index", "notes_tags", "notes_search_by_tag",
        "notes_semantic_search", "notes_rebuild_index",
        "notes_index_stats", "notes_smart_search",
    ];
    if notes_tools.contains(&tool) {
        if !apple_notes::is_available() {
            return Some((false, "Apple Notes is not available (macOS with AppleScript required)".to_string()));
        }
        let action = match tool {
            "search_notes" => "smart_search",
            "list_notes" => "list",
            "get_note" => "get",
            "open_note" => "open",
            "notes_index" => {
                match args.get("action").and_then(|v| v.as_str()).unwrap_or("check") {
                    "build" => "index_build",
                    _ => "index_check",
                }
            }
            "notes_tags" => "tags",
            "notes_search_by_tag" => "search_by_tag",
            "notes_semantic_search" => "semantic_search",
            "notes_rebuild_index" => "rebuild_memvid_index",
            "notes_index_stats" => "memvid_stats",
            "notes_smart_search" => "smart_search",
            _ => unreachable!(),
        };
        return Some(match apple_notes::execute_apple_notes(action, args) {
            Ok(result) => (true, result),
            Err(e) => (false, format!("Apple Notes error: {}", e)),
        });
    }

    // Apple Reminders tools
    let reminders_tools = [
        "list_reminder_lists", "search_reminders", "list_reminders",
        "get_reminder", "create_reminder", "create_reminders_batch",
        "complete_reminder", "delete_reminder", "edit_reminder",
        "edit_reminders_batch", "open_reminders",
        "create_reminder_list", "delete_reminder_list",
    ];
    if reminders_tools.contains(&tool) {
        if !apple_reminders::is_available() {
            return Some((false, "Apple Reminders is not available. Build the Swift helper: cd swift/reminders-helper && swift build -c release".to_string()));
        }
        let action = match tool {
            "list_reminder_lists" => "list_lists",
            "search_reminders" => "search",
            "list_reminders" => "list",
            "get_reminder" => "get",
            "create_reminder" => "create",
            "create_reminders_batch" => "create_batch",
            "complete_reminder" => "complete",
            "delete_reminder" => "delete",
            "edit_reminder" => "edit",
            "edit_reminders_batch" => "edit_batch",
            "open_reminders" => "open",
            "create_reminder_list" => "create_list",
            "delete_reminder_list" => "delete_list",
            _ => unreachable!(),
        };
        return Some(match apple_reminders::execute_apple_reminders(action, args) {
            Ok(result) => (true, result),
            Err(e) => (false, format!("Apple Reminders error: {}", e)),
        });
    }

    // Apple Contacts tools
    let contacts_tools = [
        "list_contact_groups", "search_contacts", "list_contacts",
        "get_contact", "create_contact", "edit_contact", "delete_contact",
    ];
    if contacts_tools.contains(&tool) {
        if !apple_contacts::is_available() {
            return Some((false, "Apple Contacts is not available. Build the Swift helper: cd swift/contacts-helper && swift build -c release".to_string()));
        }
        let action = match tool {
            "list_contact_groups" => "list-groups",
            "search_contacts"    => "search",
            "list_contacts"      => "list",
            "get_contact"        => "get",
            "create_contact"     => "create",
            "edit_contact"       => "edit",
            "delete_contact"     => "delete",
            _ => unreachable!(),
        };
        return Some(match apple_contacts::execute_apple_contacts(action, args) {
            Ok(result) => (true, result),
            Err(e) => (false, format!("Apple Contacts error: {}", e)),
        });
    }

    // Weather (Apple WeatherKit primary, Open-Meteo fallback)
    if tool == "get_weather" {
        let location = args["location"].as_str().unwrap_or("NYC");
        let date = args.get("date").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
        let date_end = args.get("date_end").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
        let query_type = args
            .get("weather_query")
            .and_then(|v| v.as_str())
            .map(apple_weather::QueryType::from_str)
            .unwrap_or_default();

        if apple_weather::is_configured() {
            match apple_weather::execute_apple_weather(location, date, date_end, query_type) {
                Ok(result) => {
                    tracing::info!("Weather source: Apple WeatherKit");
                    return Some((true, result));
                }
                Err(e) => tracing::warn!(error = %e, "Apple Weather failed, trying Open-Meteo"),
            }
        }
        return Some(match open_meteo::execute_open_meteo_weather(location, date, date_end, query_type) {
            Ok(result) => {
                tracing::info!("Weather source: Open-Meteo");
                (true, result)
            }
            Err(e) => (false, format!("Error: {}", e)),
        });
    }

    // POI search (Apple Maps primary, Nominatim fallback)
    if tool == "search_nearby" {
        let query = args["query"].as_str().unwrap_or("");
        let location_arg = args["location"].as_str().unwrap_or("").to_string();
        let location = if location_arg.is_empty() {
            crate::access_store::AccessStore::get_default_location().unwrap_or_default()
        } else {
            location_arg
        };
        let location = location.as_str();
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5).min(10) as u32;

        if apple_maps::is_configured() {
            match apple_maps::search_nearby(query, location, limit) {
                Ok(result) => {
                    tracing::info!("Nearby search source: Apple Maps");
                    return Some((true, result));
                }
                Err(e) => tracing::warn!(error = %e, "Apple Maps search failed, trying Nominatim"),
            }
        }
        return Some(match nominatim::search_pois(query, location, limit) {
            Ok(result) => {
                tracing::info!("Nearby search source: Nominatim");
                (true, result)
            }
            Err(e) => (false, format!("Error: {}", e)),
        });
    }

    // Web search (Brave Search backend)
    if tool == "web_search" && brave_search::is_configured() {
        let query = args["query"].as_str().unwrap_or("");
        let count = args.get("count").and_then(|v| v.as_u64()).map(|n| n as u32);
        let sources: Option<Vec<String>> = args.get("sources")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());
        return Some(match brave_search::execute_brave_search(query, count, sources.as_deref()) {
            Ok(result) => (true, result),
            Err(e) => { tracing::error!(error = %e, "TOOL FAILED: Brave Search"); (false, format!("Error: {}", e)) }
        });
    }

    // News search (Brave Search backend)
    if tool == "news_search" && brave_search::is_configured() {
        let query = args["query"].as_str().unwrap_or("");
        let count = args.get("count").and_then(|v| v.as_u64()).map(|n| n as u32);
        let freshness = args.get("freshness").and_then(|v| v.as_str());
        let sources: Option<Vec<String>> = args.get("sources")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());
        return Some(match brave_search::execute_news_search(query, count, freshness, sources.as_deref()) {
            Ok(result) => (true, result),
            Err(e) => { tracing::error!(error = %e, "TOOL FAILED: Brave News Search"); (false, format!("Error: {}", e)) }
        });
    }

    // Video search (Brave Search backend)
    if tool == "video_search" && brave_search::is_configured() {
        let query = args["query"].as_str().unwrap_or("");
        let count = args.get("count").and_then(|v| v.as_u64()).map(|n| n as u32);
        let freshness = args.get("freshness").and_then(|v| v.as_str());
        return Some(match brave_search::execute_video_search(query, count, freshness) {
            Ok(result) => (true, result),
            Err(e) => { tracing::error!(error = %e, "TOOL FAILED: Brave Video Search"); (false, format!("Error: {}", e)) }
        });
    }

    // Image search (Brave Search backend)
    if tool == "image_search" && brave_search::is_configured() {
        let query = args["query"].as_str().unwrap_or("");
        let count = args.get("count").and_then(|v| v.as_u64()).map(|n| n as u32);
        return Some(match brave_search::execute_image_search(query, count) {
            Ok(result) => (true, result),
            Err(e) => { tracing::error!(error = %e, "TOOL FAILED: Brave Image Search"); (false, format!("Error: {}", e)) }
        });
    }

    // YouTube transcript
    if tool == "youtube_transcript" {
        return Some(match crate::youtube_transcript::execute("get_transcript", args) {
            Ok(result) => (true, result),
            Err(e) => { tracing::error!(error = %e, "TOOL FAILED: youtube_transcript"); (false, format!("youtube_transcript error: {}", e)) }
        });
    }

    // Code interpreter — sandboxed Python execution
    if tool == "code_interpreter" {
        return Some(match crate::code_interpreter::execute("execute", args) {
            Ok(result) => (true, result),
            Err(e) => { tracing::error!(error = %e, "TOOL FAILED: code_interpreter"); (false, format!("code_interpreter error: {}", e)) }
        });
    }

    // URL fetching
    if tool == "fetch_url" {
        let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let max_bytes = args.get("max_bytes").and_then(|v| v.as_u64()).map(|n| n as usize);
        return Some(match crate::fetch_url::execute_fetch_url(url, max_bytes) {
            Ok(result) => (true, result),
            Err(e) => { tracing::error!(error = %e, "TOOL FAILED: fetch_url"); (false, format!("fetch_url error: {}", e)) }
        });
    }

    // Image generation (Gemini backend)
    if tool == "image_generation" && gemini_image::is_configured() {
        let prompt = args["prompt"].as_str().unwrap_or("");
        let aspect_ratio = args.get("aspect_ratio").and_then(|v| v.as_str());
        let image_size = args.get("image_size").and_then(|v| v.as_str()).map(|s| {
            match s.to_lowercase().as_str() {
                "small" => "1K", "medium" => "2K", "large" => "4K",
                _ => s,
            }
        });
        return Some(match gemini_image::generate_image(prompt, aspect_ratio, image_size.as_deref()) {
            Ok(result) => (true, result),
            Err(e) => { tracing::error!(error = %e, "TOOL FAILED: Gemini image generation"); (false, format!("Error: {}", e)) }
        });
    }

    // input_image — casual image description
    if tool == "input_image" {
        if !gemini_image::is_configured() {
            return Some((false, "input_image requires GEMINI_API_KEY to be configured".to_string()));
        }
        let source = args.get("image_source").or_else(|| args.get("image_url")).or_else(|| args.get("path")).and_then(|v| v.as_str()).unwrap_or("");
        let prompt = args.get("prompt").or_else(|| args.get("text")).and_then(|v| v.as_str());
        return Some(match gemini_image::describe_image(source, prompt) {
            Ok(result) => (true, result),
            Err(e) => (false, format!("input_image error: {}", e)),
        });
    }

    // analyze_image — detailed structured analysis
    if tool == "analyze_image" {
        if !gemini_image::is_configured() { return Some((false, "analyze_image requires GEMINI_API_KEY to be configured".to_string())); }
        let source = args.get("image_source").or_else(|| args.get("image_url")).or_else(|| args.get("path")).and_then(|v| v.as_str()).unwrap_or("");
        let prompt = args.get("prompt").and_then(|v| v.as_str());
        return Some(match gemini_image::analyze_image(source, prompt) {
            Ok(r) => (true, r), Err(e) => (false, format!("analyze_image error: {}", e)),
        });
    }

    // extract_text — OCR
    if tool == "extract_text" {
        if !gemini_image::is_configured() { return Some((false, "extract_text requires GEMINI_API_KEY to be configured".to_string())); }
        let source = args.get("image_source").or_else(|| args.get("image_url")).or_else(|| args.get("path")).and_then(|v| v.as_str()).unwrap_or("");
        let prompt = args.get("prompt").and_then(|v| v.as_str());
        return Some(match gemini_image::extract_text(source, prompt) {
            Ok(r) => (true, r), Err(e) => (false, format!("extract_text error: {}", e)),
        });
    }

    // detect_objects — bounding boxes
    if tool == "detect_objects" {
        if !gemini_image::is_configured() { return Some((false, "detect_objects requires GEMINI_API_KEY to be configured".to_string())); }
        let source = args.get("image_source").or_else(|| args.get("image_url")).or_else(|| args.get("path")).and_then(|v| v.as_str()).unwrap_or("");
        let prompt = args.get("prompt").and_then(|v| v.as_str());
        return Some(match gemini_image::detect_objects(source, prompt) {
            Ok(r) => (true, r), Err(e) => (false, format!("detect_objects error: {}", e)),
        });
    }

    // compare_images — multi-image comparison
    if tool == "compare_images" {
        if !gemini_image::is_configured() { return Some((false, "compare_images requires GEMINI_API_KEY to be configured".to_string())); }
        let sources: Vec<&str> = if let Some(arr) = args.get("image_sources").and_then(|v| v.as_array()) {
            arr.iter().filter_map(|v| v.as_str()).collect()
        } else { vec![] };
        let prompt = args.get("prompt").and_then(|v| v.as_str());
        return Some(match gemini_image::compare_images(&sources, prompt) {
            Ok(r) => (true, r), Err(e) => (false, format!("compare_images error: {}", e)),
        });
    }

    // Try to execute via configured endpoint (tools.json HTTP endpoints)
    if let Some(exec) = executor {
        if exec.has_endpoint(tool) {
            match exec.execute(tool, args) {
                Ok(Some(result)) => return Some((true, result)),
                Ok(None) => {}
                Err(e) => {
                    tracing::error!(error = %e, "TOOL FAILED: endpoint execution");
                    return Some((false, format!("Error: {}", e)));
                }
            }
        }
    }

    // Not recognized by mcp-core — return None so caller can try additional dispatchers
    None
}

/// Convenience wrapper that calls `dispatch_tool` and falls back to a stub response.
pub fn execute_tool(tool: &str, args: &Value, executor: Option<&ToolExecutor>) -> (bool, String) {
    if let Some(result) = dispatch_tool(tool, args, executor) {
        return result;
    }

    // Fall back to stub
    let result = match tool {
        "search_notes" => format!("[stub] searched notes for: {}", args),
        "get_weather" => format!("[stub] weather result for: {}", args),
        "draft_email" => format!("[stub] drafted email with: {}", args),
        "create_todo" => format!("[stub] created todo with: {}", args),
        "read_file" => format!("[stub] read file: {}", args),
        "apply_patch" => format!("[stub] apply patch: {}", args),
        _ => return (false, format!("Unknown tool: '{}'", tool)),
    };

    (true, result)
}
