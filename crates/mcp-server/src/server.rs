//! MCP ServerHandler implementation.
//!
//! Bridges MCP protocol to `psyxe_mcp_core::tool_dispatch::execute_tool()`.
//! All tool calls are direct — no LLM involved.
//! Access control is enforced via `AccessConfig` from `~/.psyxe/access.toml`.

use crate::access_config::AccessConfig;
use crate::access_filter::{self, AccessCheck};
use psyxe_mcp_core::tool_dispatch;
use psyxe_mcp_core::tools::ToolExecutor;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ServerCapabilities, Tool,
};
use rmcp::service::Peer;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;

/// Search tools that benefit from a fresh semantic index.
#[cfg(feature = "memvid")]
const SEARCH_TOOLS: &[&str] = &["search_notes", "notes_semantic_search", "notes_smart_search"];

/// MCP server that dispatches tool calls to the psyXe core engine.
#[derive(Clone)]
pub struct McpServer {
    tools: Vec<Tool>,
    executor: Option<Arc<ToolExecutor>>,
    access: Arc<AccessConfig>,
}

impl McpServer {
    pub fn new(tools: Vec<Tool>, executor: Option<ToolExecutor>, access: AccessConfig) -> Self {
        Self {
            tools,
            executor: executor.map(Arc::new),
            access: Arc::new(access),
        }
    }
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> rmcp::model::InitializeResult {
        rmcp::model::InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_logging()
                .build(),
        )
        .with_server_info(Implementation::new("psyxe-mcp", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "Apple ecosystem tools: Notes, Reminders, and Contacts. \
             All tools execute directly against macOS-native APIs (AppleScript, \
             EventKit, Contacts framework) with no LLM intermediary and no credentials required.\n\n\
             When working with tool results, write down any important information you might need later \
             in your response, as the original tool result may be cleared later.\n\n\
             IMPORTANT: Notes, Reminders, and Contacts are live data sources that can change at any time \
             (the user may add, edit, or delete items outside of this conversation). \
             Always call search tools fresh for each new query — never reuse results from a previous \
             call, even if the query is identical. If you receive a logging notification that the \
             Notes index is stale, call notes_rebuild_index before searching.",
        )
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(rmcp::model::ListToolsResult {
            tools: self.tools.clone(),
            next_cursor: None,
            meta: None,
        }))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: rmcp::service::RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let name = request.name.to_string();
        let args: Value = match request.arguments {
            Some(map) => Value::Object(map),
            None => Value::Object(Default::default()),
        };
        let executor = self.executor.clone();
        let access = self.access.clone();

        async move {
            // ── Access control check ────────────────────────────────────
            let needs_filtering = match access_filter::check_access(&access, &name, &args) {
                AccessCheck::Allowed => false,
                AccessCheck::FilterResults => true,
                AccessCheck::Denied(msg) => {
                    tracing::warn!(tool = %name, "Access denied: {}", msg);
                    return Ok(CallToolResult::error(vec![Content::text(msg)]));
                }
            };

            // ── Auto-reindex stale memvid index before search tools ─────
            #[cfg(feature = "memvid")]
            if SEARCH_TOOLS.contains(&name.as_str()) {
                use psyxe_mcp_core::memvid_notes;
                use rmcp::model::{LoggingLevel, LoggingMessageNotificationParam};

                let needs_reindex = tokio::task::spawn_blocking(|| {
                    memvid_notes::index_exists() && memvid_notes::is_stale().unwrap_or(false)
                })
                .await
                .unwrap_or(false);

                if needs_reindex {
                    let _ = context
                        .peer
                        .notify_logging_message(LoggingMessageNotificationParam {
                            level: LoggingLevel::Info,
                            logger: Some("memvid".into()),
                            data: Value::String(
                                "Notes search index is stale, rebuilding before search..."
                                    .into(),
                            ),
                        })
                        .await;

                    let rebuild_result = tokio::task::spawn_blocking(|| {
                        tool_dispatch::execute_tool(
                            "notes_rebuild_index",
                            &serde_json::json!({}),
                            None,
                        )
                    })
                    .await;

                    let msg = match &rebuild_result {
                        Ok((true, _)) => "Notes index rebuilt successfully",
                        Ok((false, err)) => {
                            tracing::warn!(error = %err, "Index rebuild failed");
                            "Index rebuild failed, proceeding with search anyway"
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Index rebuild task panicked");
                            "Index rebuild failed, proceeding with search anyway"
                        }
                    };

                    let _ = context
                        .peer
                        .notify_logging_message(LoggingMessageNotificationParam {
                            level: LoggingLevel::Info,
                            logger: Some("memvid".into()),
                            data: Value::String(msg.into()),
                        })
                        .await;
                }
            }

            // ── Execute tool ────────────────────────────────────────────
            let tool_name = name.clone();
            let (success, result) = tokio::task::spawn_blocking(move || {
                tool_dispatch::execute_tool(&tool_name, &args, executor.as_deref())
            })
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

            // ── Post-call filtering ─────────────────────────────────────
            let result = if needs_filtering && success {
                access_filter::filter_results(&access, &name, &result)
            } else {
                result
            };

            let content = vec![Content::text(result)];
            if success {
                Ok(CallToolResult::success(content))
            } else {
                Ok(CallToolResult::error(content))
            }
        }
    }
}

/// Background task that periodically checks if the Notes semantic index is stale
/// and sends an MCP logging notification to Claude when it detects changes.
#[cfg(feature = "memvid")]
pub async fn staleness_monitor(peer: Peer<RoleServer>) {
    use psyxe_mcp_core::memvid_notes;
    use rmcp::model::{LoggingLevel, LoggingMessageNotificationParam};

    const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

    // Short initial delay to let the connection fully initialize
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    loop {
        tokio::time::sleep(CHECK_INTERVAL).await;

        // Check staleness on a blocking thread (AppleScript calls)
        let stale_info = tokio::task::spawn_blocking(|| {
            if !memvid_notes::index_exists() {
                return None; // No index to be stale
            }
            match memvid_notes::is_stale() {
                Ok(true) => {
                    // Get details for a useful notification
                    let stats = memvid_notes::get_stats().ok();
                    Some(stats)
                }
                Ok(false) => None,
                Err(e) => {
                    tracing::debug!(error = %e, "Staleness check failed");
                    None
                }
            }
        })
        .await
        .unwrap_or(None);

        if let Some(stats) = stale_info {
            let msg = if let Some(s) = stats {
                format!(
                    "Notes search index is stale — index has {} notes but Notes.app \
                     now has {}. Call notes_rebuild_index to update before searching.",
                    s.indexed_note_count, s.current_note_count
                )
            } else {
                "Notes search index is stale — notes have been added, modified, \
                 or deleted. Call notes_rebuild_index to update before searching."
                    .to_string()
            };

            tracing::info!("{}", msg);

            let _ = peer
                .notify_logging_message(LoggingMessageNotificationParam {
                    level: LoggingLevel::Warning,
                    logger: Some("memvid".into()),
                    data: Value::String(msg),
                })
                .await;
        }
    }
}
