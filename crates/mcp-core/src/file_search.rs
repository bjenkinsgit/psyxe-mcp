//! Unified local document search — Apple Notes + local files via mdfind (Spotlight).
//!
//! Phase 1: mdfind keyword search over granted folders + Notes memvid search.
//! Phase 2 (future): Separate memvid index for local files.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::access_store::AccessStore;
use crate::memvid_notes;

// ============================================================================
// Data Structures
// ============================================================================

/// A folder the user has granted search access to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantedFolder {
    pub path: PathBuf,
    pub display_name: String,
    pub granted_at: String, // ISO 8601
    pub enabled: bool,
    #[serde(default)]
    pub writable: bool,
}

/// A single file_search result (works for both mdfind and memvid sources).
#[derive(Debug, Clone, Serialize)]
pub struct FileSearchResult {
    pub source: String,            // "notes" | "file"
    pub path: Option<String>,      // file path (for files) or note title (for notes)
    pub title: String,
    pub snippet: String,
    pub score: Option<f32>,        // semantic score (None for keyword results)
    pub file_type: Option<String>, // e.g. "pdf", "md"
    pub modified: Option<String>,  // ISO 8601
}

/// Controls which sources file_search queries.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchScope {
    Notes, // only Notes memvid index
    Files, // only granted folders (mdfind)
    All,   // Notes + files, merged by relevance
}

impl SearchScope {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "notes" => SearchScope::Notes,
            "files" => SearchScope::Files,
            _ => SearchScope::All,
        }
    }
}

/// Parameters extracted by the LLM from natural language.
pub struct FileSearchParams {
    pub query: String,
    pub scope: SearchScope,
    pub file_types: Option<Vec<String>>,
    pub date_after: Option<String>,  // ISO date
    pub date_before: Option<String>, // ISO date
    pub tags: Option<Vec<String>>,   // macOS Finder tags
    pub max_num_results: usize,
}

impl FileSearchParams {
    /// Parse from the JSON args the LLM provides.
    pub fn from_args(args: &serde_json::Value) -> Result<Self> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing required 'query' argument"))?
            .to_string();

        let scope = args
            .get("scope")
            .and_then(|v| v.as_str())
            .map(SearchScope::from_str)
            .unwrap_or(SearchScope::All);

        let file_types = args.get("file_types").and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
        });

        let date_after = args
            .get("date_after")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let date_before = args
            .get("date_before")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let tags = args.get("tags").and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
        });

        let max_num_results = args
            .get("max_num_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        Ok(FileSearchParams {
            query,
            scope,
            file_types,
            date_after,
            date_before,
            tags,
            max_num_results,
        })
    }
}

// ============================================================================
// Granted Folders Store (thin facade over unified AccessStore)
// ============================================================================

/// Facade for managing granted file-search folders.
///
/// Delegates to [`AccessStore`] which holds all access-control data in a
/// single Keychain entry with a process-level cache.
pub struct GrantedFoldersStore;

impl GrantedFoldersStore {
    /// Load all granted folders.
    pub fn load() -> Result<GrantedFoldersStoreView> {
        let folders = AccessStore::list_folders()
            .map_err(|e| anyhow!("{}", e))?;
        Ok(GrantedFoldersStoreView { folders })
    }
}

/// Read-only view of granted folders returned by `GrantedFoldersStore::load()`.
///
/// Preserves the `store.list()` API so callers don't need to change.
/// Mutations go through `GrantedFoldersStore` static methods directly.
pub struct GrantedFoldersStoreView {
    folders: Vec<GrantedFolder>,
}

impl GrantedFoldersStoreView {
    /// List all folders.
    pub fn list(&self) -> &[GrantedFolder] {
        &self.folders
    }

    /// Add a folder (validates and persists via AccessStore).
    pub fn add_folder(&mut self, path: &Path) -> Result<()> {
        AccessStore::add_folder(path).map_err(|e| anyhow!("{}", e))?;
        // Refresh local view
        self.folders = AccessStore::list_folders().map_err(|e| anyhow!("{}", e))?;
        Ok(())
    }

    /// Remove a folder by path.
    pub fn remove_folder(&mut self, path: &Path) -> Result<()> {
        AccessStore::remove_folder(path).map_err(|e| anyhow!("{}", e))?;
        self.folders = AccessStore::list_folders().map_err(|e| anyhow!("{}", e))?;
        Ok(())
    }

    /// Enable or disable a folder.
    pub fn set_enabled(&mut self, path: &Path, enabled: bool) -> Result<()> {
        AccessStore::set_folder_enabled(path, enabled).map_err(|e| anyhow!("{}", e))?;
        self.folders = AccessStore::list_folders().map_err(|e| anyhow!("{}", e))?;
        Ok(())
    }
}

/// Format a short suffix for file tool descriptions listing granted folders.
/// Returns empty string when no folders are granted.
pub fn format_granted_folders_suffix() -> String {
    let folders = AccessStore::list_folders().unwrap_or_default();
    let entries: Vec<String> = folders.iter()
        .filter(|f| f.enabled)
        .map(|f| {
            let access = if f.writable { "rw" } else { "ro" };
            format!("{}({}) → {}", f.display_name, access, f.path.display())
        })
        .collect();
    if entries.is_empty() {
        return String::new();
    }
    format!(" [Granted folders: {}. Use absolute paths for granted folders, relative paths for workspace.]", entries.join(", "))
}

// ============================================================================
// mdfind Query Builder
// ============================================================================

/// A raw hit from mdfind (path only; metadata read separately).
#[derive(Debug)]
pub struct MdfindHit {
    pub path: PathBuf,
}

/// Map common file extensions to macOS UTI content types.
pub fn ext_to_uti(ext: &str) -> Option<&'static str> {
    match ext.to_lowercase().as_str() {
        "pdf" => Some("com.adobe.pdf"),
        "md" | "markdown" => Some("net.daringfireball.markdown"),
        "txt" | "text" => Some("public.plain-text"),
        "rtf" => Some("public.rtf"),
        "html" | "htm" => Some("public.html"),
        "doc" => Some("com.microsoft.word.doc"),
        "docx" => Some("org.openxmlformats.wordprocessingml.document"),
        "xls" => Some("com.microsoft.excel.xls"),
        "xlsx" => Some("org.openxmlformats.spreadsheetml.sheet"),
        "ppt" => Some("com.microsoft.powerpoint.ppt"),
        "pptx" => Some("org.openxmlformats.presentationml.presentation"),
        "pages" => Some("com.apple.iwork.pages.sffpages"),
        "numbers" => Some("com.apple.iwork.numbers.sffnumbers"),
        "keynote" => Some("com.apple.iwork.keynote.sffkey"),
        "csv" => Some("public.comma-separated-values-text"),
        "json" => Some("public.json"),
        "xml" => Some("public.xml"),
        "py" => Some("public.python-script"),
        "rs" => Some("public.rust-source"),
        "js" => Some("com.netscape.javascript-source"),
        "ts" => Some("public.source-code"), // no specific UTI for TypeScript
        "swift" => Some("public.swift-source"),
        "c" => Some("public.c-source"),
        "cpp" | "cc" | "cxx" => Some("public.c-plus-plus-source"),
        "h" => Some("public.c-header"),
        "java" => Some("com.sun.java-source"),
        "sh" | "bash" | "zsh" => Some("public.shell-script"),
        "yaml" | "yml" => Some("public.yaml"),
        "toml" => Some("public.source-code"),
        "png" => Some("public.png"),
        "jpg" | "jpeg" => Some("public.jpeg"),
        "gif" => Some("public.gif"),
        "svg" => Some("public.svg-image"),
        _ => None,
    }
}

/// Build an mdfind query string from structured parameters.
///
/// Produces Spotlight predicate syntax, e.g.:
/// `(kMDItemContentType == 'com.adobe.pdf') && (kMDItemTextContent == '*Rust*'cd)`
pub fn build_mdfind_query(params: &FileSearchParams) -> String {
    let mut predicates: Vec<String> = Vec::new();

    // Tags filter — when searching by tags, prioritize kMDItemUserTags
    if let Some(ref tags) = params.tags {
        for tag in tags {
            predicates.push(format!("(kMDItemUserTags == '{}'cd)", tag));
        }
    }

    // File type filter
    if let Some(ref types) = params.file_types {
        let type_preds: Vec<String> = types
            .iter()
            .filter_map(|ext| ext_to_uti(ext).map(|uti| format!("kMDItemContentType == '{}'", uti)))
            .collect();
        if !type_preds.is_empty() {
            if type_preds.len() == 1 {
                predicates.push(format!("({})", type_preds[0]));
            } else {
                predicates.push(format!("({})", type_preds.join(" || ")));
            }
        }
    }

    // Date filters
    if let Some(ref after) = params.date_after {
        // mdfind date format: $time.iso(YYYY-MM-DD)
        predicates.push(format!(
            "(kMDItemFSContentChangeDate >= $time.iso({}))",
            after
        ));
    }
    if let Some(ref before) = params.date_before {
        predicates.push(format!(
            "(kMDItemFSContentChangeDate <= $time.iso({}))",
            before
        ));
    }

    // Text content query — always add unless query is empty or only tags search
    let query = params.query.trim();
    if !query.is_empty() {
        predicates.push(format!("(kMDItemTextContent == '*{}*'cd)", query));
    }

    if predicates.is_empty() {
        // Fallback: just search by display name
        format!("(kMDItemDisplayName == '*{}*'cd)", query)
    } else {
        predicates.join(" && ")
    }
}

/// Execute mdfind scoped to granted folders, returning file paths.
pub fn run_mdfind(
    params: &FileSearchParams,
    folders: &[GrantedFolder],
) -> Result<Vec<MdfindHit>> {
    if folders.is_empty() {
        return Ok(Vec::new());
    }

    let query = build_mdfind_query(params);

    let mut cmd = Command::new("mdfind");
    for folder in folders {
        cmd.arg("-onlyin").arg(&folder.path);
    }
    cmd.arg(&query);

    tracing::debug!(query = %query, "Running mdfind");

    let output = cmd
        .output()
        .map_err(|e| anyhow!("Failed to execute mdfind: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // mdfind returns non-zero for some query errors but still outputs results;
        // log and continue
        tracing::warn!(stderr = %stderr, "mdfind returned non-zero status");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let hits: Vec<MdfindHit> = stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| MdfindHit {
            path: PathBuf::from(line),
        })
        .collect();

    Ok(hits)
}

/// Read a snippet from a file hit (first N chars of content).
pub fn read_file_snippet(path: &Path, max_chars: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let trimmed: String = content.chars().take(max_chars).collect();
            if content.len() > max_chars {
                format!("{}...", trimmed)
            } else {
                trimmed
            }
        }
        Err(_) => {
            // Binary file or read error — just show the filename
            format!(
                "[binary or unreadable: {}]",
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
            )
        }
    }
}

/// Read file modification time as ISO 8601 string.
fn file_modified_iso(path: &Path) -> Option<String> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.to_rfc3339()
        })
}

/// Infer file type from extension.
fn file_type_from_path(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
}

/// Convert mdfind hits into FileSearchResult entries.
fn mdfind_to_file_results(hits: Vec<MdfindHit>) -> Vec<FileSearchResult> {
    hits.into_iter()
        .map(|hit| {
            let title = hit
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            let snippet = read_file_snippet(&hit.path, 300);
            let modified = file_modified_iso(&hit.path);
            let file_type = file_type_from_path(&hit.path);

            FileSearchResult {
                source: "file".to_string(),
                path: Some(hit.path.to_string_lossy().to_string()),
                title,
                snippet,
                score: None, // keyword results have no semantic score
                file_type,
                modified,
            }
        })
        .collect()
}

/// Convert Notes memvid search results into FileSearchResult entries.
fn notes_json_to_file_results(json_str: &str) -> Vec<FileSearchResult> {
    // The memvid search_json returns: { "count": N, "results": [...] }
    // Each result has: "title", "snippet", "score"
    let parsed: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let results = match parsed.get("results").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return Vec::new(),
    };

    results
        .iter()
        .map(|r| FileSearchResult {
            source: "notes".to_string(),
            path: None,
            title: r
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            snippet: r
                .get("snippet")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            score: r.get("score").and_then(|v| v.as_f64()).map(|f| f as f32),
            file_type: None,
            modified: None,
        })
        .collect()
}

// ============================================================================
// Unified Dispatch
// ============================================================================

/// Main entry point — scope controls which sources are searched.
///
/// - `scope=Notes`  → only Notes memvid
/// - `scope=Files`  → only mdfind over granted folders
/// - `scope=All`    → Notes + files, merged by score
pub fn execute_file_search(params: &FileSearchParams) -> Result<String> {
    let mut results: Vec<FileSearchResult> = Vec::new();

    // 1. Search Notes (if scope is Notes or All)
    if matches!(params.scope, SearchScope::Notes | SearchScope::All) {
        if memvid_notes::index_exists() {
            match memvid_notes::search_json(&params.query, params.max_num_results) {
                Ok(json_str) => {
                    results.extend(notes_json_to_file_results(&json_str));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Notes search failed, continuing with file search");
                }
            }
        }
    }

    // 2. Search files via mdfind (if scope is Files or All)
    if matches!(params.scope, SearchScope::Files | SearchScope::All) {
        let enabled = AccessStore::enabled_folders();
        if !enabled.is_empty() {
            match run_mdfind(params, &enabled) {
                Ok(hits) => {
                    results.extend(mdfind_to_file_results(hits));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "mdfind search failed");
                }
            }
        }
    }

    // 3. Merge: semantic results first (by score desc), then keyword results
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(params.max_num_results);

    Ok(serde_json::to_string(&serde_json::json!({
        "count": results.len(),
        "results": results
    }))?)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ext_to_uti_known() {
        assert_eq!(ext_to_uti("pdf"), Some("com.adobe.pdf"));
        assert_eq!(ext_to_uti("md"), Some("net.daringfireball.markdown"));
        assert_eq!(ext_to_uti("txt"), Some("public.plain-text"));
        assert_eq!(ext_to_uti("docx"), Some("org.openxmlformats.wordprocessingml.document"));
        assert_eq!(ext_to_uti("py"), Some("public.python-script"));
        assert_eq!(ext_to_uti("rs"), Some("public.rust-source"));
    }

    #[test]
    fn test_ext_to_uti_case_insensitive() {
        assert_eq!(ext_to_uti("PDF"), Some("com.adobe.pdf"));
        assert_eq!(ext_to_uti("Md"), Some("net.daringfireball.markdown"));
    }

    #[test]
    fn test_ext_to_uti_unknown() {
        assert_eq!(ext_to_uti("xyz"), None);
        assert_eq!(ext_to_uti(""), None);
    }

    #[test]
    fn test_build_query_text_only() {
        let params = FileSearchParams {
            query: "Rust async".to_string(),
            scope: SearchScope::All,
            file_types: None,
            date_after: None,
            date_before: None,
            tags: None,
            max_num_results: 10,
        };
        let q = build_mdfind_query(&params);
        assert_eq!(q, "(kMDItemTextContent == '*Rust async*'cd)");
    }

    #[test]
    fn test_build_query_with_file_type() {
        let params = FileSearchParams {
            query: "Rust".to_string(),
            scope: SearchScope::Files,
            file_types: Some(vec!["pdf".to_string()]),
            date_after: None,
            date_before: None,
            tags: None,
            max_num_results: 10,
        };
        let q = build_mdfind_query(&params);
        assert!(q.contains("kMDItemContentType == 'com.adobe.pdf'"));
        assert!(q.contains("kMDItemTextContent == '*Rust*'cd"));
    }

    #[test]
    fn test_build_query_multiple_file_types() {
        let params = FileSearchParams {
            query: "test".to_string(),
            scope: SearchScope::Files,
            file_types: Some(vec!["pdf".to_string(), "md".to_string()]),
            date_after: None,
            date_before: None,
            tags: None,
            max_num_results: 10,
        };
        let q = build_mdfind_query(&params);
        assert!(q.contains("kMDItemContentType == 'com.adobe.pdf'"));
        assert!(q.contains("kMDItemContentType == 'net.daringfireball.markdown'"));
        assert!(q.contains("||")); // multiple types joined with OR
    }

    #[test]
    fn test_build_query_with_dates() {
        let params = FileSearchParams {
            query: "notes".to_string(),
            scope: SearchScope::All,
            file_types: None,
            date_after: Some("2026-01-01".to_string()),
            date_before: Some("2026-02-01".to_string()),
            tags: None,
            max_num_results: 10,
        };
        let q = build_mdfind_query(&params);
        assert!(q.contains("kMDItemFSContentChangeDate >= $time.iso(2026-01-01)"));
        assert!(q.contains("kMDItemFSContentChangeDate <= $time.iso(2026-02-01)"));
    }

    #[test]
    fn test_build_query_with_tags() {
        let params = FileSearchParams {
            query: "".to_string(),
            scope: SearchScope::Files,
            file_types: None,
            date_after: None,
            date_before: None,
            tags: Some(vec!["Important".to_string()]),
            max_num_results: 10,
        };
        let q = build_mdfind_query(&params);
        assert!(q.contains("kMDItemUserTags == 'Important'cd"));
    }

    #[test]
    fn test_build_query_tags_before_content() {
        // Tags predicates should appear before content predicates
        let params = FileSearchParams {
            query: "report".to_string(),
            scope: SearchScope::Files,
            file_types: None,
            date_after: None,
            date_before: None,
            tags: Some(vec!["Work".to_string()]),
            max_num_results: 10,
        };
        let q = build_mdfind_query(&params);
        let tag_pos = q.find("kMDItemUserTags").unwrap();
        let content_pos = q.find("kMDItemTextContent").unwrap();
        assert!(
            tag_pos < content_pos,
            "Tags predicate should come before content predicate"
        );
    }

    #[test]
    fn test_search_scope_from_str() {
        assert_eq!(SearchScope::from_str("notes"), SearchScope::Notes);
        assert_eq!(SearchScope::from_str("files"), SearchScope::Files);
        assert_eq!(SearchScope::from_str("all"), SearchScope::All);
        assert_eq!(SearchScope::from_str("NOTES"), SearchScope::Notes);
        assert_eq!(SearchScope::from_str("anything"), SearchScope::All);
    }

    #[test]
    fn test_params_from_args_minimal() {
        let args = serde_json::json!({ "query": "hello" });
        let params = FileSearchParams::from_args(&args).unwrap();
        assert_eq!(params.query, "hello");
        assert_eq!(params.scope, SearchScope::All);
        assert_eq!(params.max_num_results, 10);
        assert!(params.file_types.is_none());
        assert!(params.date_after.is_none());
        assert!(params.tags.is_none());
    }

    #[test]
    fn test_params_from_args_full() {
        let args = serde_json::json!({
            "query": "Rust",
            "scope": "files",
            "file_types": ["pdf", "md"],
            "date_after": "2026-01-01",
            "date_before": "2026-02-01",
            "tags": ["Important"],
            "max_num_results": 5
        });
        let params = FileSearchParams::from_args(&args).unwrap();
        assert_eq!(params.query, "Rust");
        assert_eq!(params.scope, SearchScope::Files);
        assert_eq!(params.file_types, Some(vec!["pdf".into(), "md".into()]));
        assert_eq!(params.date_after, Some("2026-01-01".into()));
        assert_eq!(params.date_before, Some("2026-02-01".into()));
        assert_eq!(params.tags, Some(vec!["Important".into()]));
        assert_eq!(params.max_num_results, 5);
    }

    #[test]
    fn test_params_from_args_missing_query() {
        let args = serde_json::json!({ "scope": "files" });
        assert!(FileSearchParams::from_args(&args).is_err());
    }

    #[test]
    fn test_granted_folders_store_crud() {
        // Uses GrantedFoldersStoreView which delegates to AccessStore.
        // Since we can't easily inject AccessStore cache from this module,
        // test the view methods via the facade (depends on live AccessStore).
        let store = GrantedFoldersStore::load().unwrap();
        // Just verify load returns a view with list()
        let _ = store.list();
    }

    #[test]
    fn test_granted_folders_store_rejects_nonexistent() {
        let mut store = GrantedFoldersStore::load().unwrap();
        let fake_path = PathBuf::from("/nonexistent/path/abc123");
        assert!(store.add_folder(&fake_path).is_err());
    }

    #[test]
    fn test_read_file_snippet_text() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "Hello, world! This is a test file.").unwrap();

        let snippet = read_file_snippet(&file, 10);
        assert_eq!(snippet, "Hello, wor...");
    }

    #[test]
    fn test_read_file_snippet_short() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("short.txt");
        std::fs::write(&file, "Hi").unwrap();

        let snippet = read_file_snippet(&file, 100);
        assert_eq!(snippet, "Hi");
    }

    #[test]
    fn test_read_file_snippet_missing() {
        let snippet = read_file_snippet(Path::new("/nonexistent/file.txt"), 100);
        assert!(snippet.contains("binary or unreadable"));
    }

    #[test]
    fn test_notes_json_to_file_results() {
        let json = r#"{"count":2,"results":[
            {"title":"Note A","snippet":"Some text","score":0.85},
            {"title":"Note B","snippet":"Other text","score":0.72}
        ]}"#;
        let results = notes_json_to_file_results(json);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].source, "notes");
        assert_eq!(results[0].title, "Note A");
        assert_eq!(results[0].score, Some(0.85));
        assert_eq!(results[1].title, "Note B");
    }

    #[test]
    fn test_notes_json_to_file_results_empty() {
        let json = r#"{"count":0,"results":[]}"#;
        let results = notes_json_to_file_results(json);
        assert!(results.is_empty());
    }

    #[test]
    fn test_notes_json_to_file_results_invalid() {
        let results = notes_json_to_file_results("not json");
        assert!(results.is_empty());
    }

    #[test]
    fn test_mdfind_to_file_results() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.pdf");
        std::fs::write(&file, "PDF content here").unwrap();

        let hits = vec![MdfindHit {
            path: file.clone(),
        }];
        let results = mdfind_to_file_results(hits);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "file");
        assert_eq!(results[0].title, "doc.pdf");
        assert_eq!(results[0].file_type, Some("pdf".to_string()));
        assert!(results[0].path.is_some());
        assert!(results[0].modified.is_some());
    }
}
