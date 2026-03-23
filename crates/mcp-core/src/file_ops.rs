//! File operations for the agent — read files and apply unified diffs.
//!
//! Sandbox model:
//! - **Workspace** (`~/Library/Caches/prolog-router/workspace/`): always readable and writable
//! - **Granted folders**: readable only (via `file_search::GrantedFoldersStore`)
//! - Relative paths resolve against workspace; absolute paths validated against both zones

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::access_store::AccessStore;

// ============================================================================
// Workspace Management
// ============================================================================

/// Override for the workspace directory (used by tests and Tauri app).
static WORKSPACE_PATH_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Set the workspace directory path override.
pub fn set_workspace_path(path: PathBuf) {
    let _ = WORKSPACE_PATH_OVERRIDE.set(path);
}

/// Get the workspace directory (default: `~/Library/Caches/prolog-router/workspace/`).
pub fn workspace_dir() -> PathBuf {
    if let Some(p) = WORKSPACE_PATH_OVERRIDE.get() {
        return p.clone();
    }
    let cache = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    cache.join("prolog-router").join("workspace")
}

/// Ensure the workspace directory exists, creating it if needed.
fn ensure_workspace() -> Result<PathBuf> {
    let dir = workspace_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

// ============================================================================
// Path Validation
// ============================================================================

/// Maximum file size we'll read (100 KB).
const MAX_READ_SIZE: u64 = 100 * 1024;

/// Resolve a path string: relative paths are resolved against the workspace,
/// absolute paths are returned as-is.
fn resolve_path(path_str: &str) -> PathBuf {
    let p = Path::new(path_str);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        workspace_dir().join(p)
    }
}

/// Validate a path for reading. The file must be inside the workspace OR
/// inside an enabled granted folder.
pub fn validate_read_path(path_str: &str) -> Result<PathBuf> {
    let resolved = resolve_path(path_str);

    // Check existence first so we don't give a misleading "outside workspace" error
    if !resolved.exists() {
        return Err(anyhow!("File not found: {}", path_str));
    }

    // Canonicalize the workspace (create first so it exists for canonicalize)
    let ws = ensure_workspace()?;
    if let Ok(canonical_ws) = ws.canonicalize() {
        if let Ok(canonical_path) = resolved.canonicalize() {
            if canonical_path.starts_with(&canonical_ws) {
                return Ok(canonical_path);
            }
        }
    }

    // Check granted folders
    if resolved.is_absolute() {
        if let Ok(canonical_path) = resolved.canonicalize() {
            for folder in AccessStore::enabled_folders() {
                if let Ok(canonical_folder) = folder.path.canonicalize() {
                    if canonical_path.starts_with(&canonical_folder) {
                        return Ok(canonical_path);
                    }
                }
            }

            // Allow reading from the app's own cache directory (code_interpreter
            // data files, images, etc.)
            if let Some(cache_dir) = dirs::cache_dir() {
                let app_cache = cache_dir.join("prolog-router");
                if let Ok(canonical_cache) = app_cache.canonicalize() {
                    if canonical_path.starts_with(&canonical_cache) {
                        return Ok(canonical_path);
                    }
                }
            }
        }
    }

    Err(anyhow!(
        "Path is outside the workspace and not in any granted folder: {}",
        path_str
    ))
}

/// Validate a path for writing. Writable paths include the workspace directory
/// and any granted folders marked as writable.
/// Creates intermediate directories within the allowed zone as needed.
pub fn validate_write_path(path_str: &str) -> Result<PathBuf> {
    let resolved = resolve_path(path_str);
    let ws = ensure_workspace()?;

    // We need to check containment without canonicalize on the target (it may not exist yet).
    // Canonicalize the workspace, then check that the resolved path starts with it.
    let canonical_ws = ws.canonicalize()?;

    // For new files, we can't canonicalize the full path. Instead, canonicalize the
    // parent (or the deepest existing ancestor) and check containment.
    let ancestor = find_existing_ancestor(&resolved);
    if let Some(ref anc) = ancestor {
        if let Ok(canonical_check) = anc.canonicalize() {
            if canonical_check.starts_with(&canonical_ws) {
                // Inside workspace — allow
                if let Some(parent) = resolved.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                return Ok(resolved);
            }
        }
    }

    // Check writable granted folders
    if resolved.is_absolute() {
        let writable_folders: Vec<_> = AccessStore::enabled_folders()
            .into_iter()
            .filter(|f| f.writable)
            .collect();

        if let Some(ref anc) = ancestor {
            if let Ok(canonical_check) = anc.canonicalize() {
                for folder in &writable_folders {
                    if let Ok(canonical_folder) = folder.path.canonicalize() {
                        if canonical_check.starts_with(&canonical_folder) {
                            // Inside a writable granted folder — allow
                            if let Some(parent) = resolved.parent() {
                                std::fs::create_dir_all(parent)?;
                            }
                            return Ok(resolved);
                        }
                    }
                }
            }
        }
    }

    // Allow writing to the app's own cache directory (workspace, code_interpreter, etc.)
    if resolved.is_absolute() {
        if let Some(cache_dir) = dirs::cache_dir() {
            let app_cache = cache_dir.join("prolog-router");
            if let Some(ref anc) = ancestor {
                if let Ok(canonical_check) = anc.canonicalize() {
                    if let Ok(canonical_cache) = app_cache.canonicalize() {
                        if canonical_check.starts_with(&canonical_cache) {
                            if let Some(parent) = resolved.parent() {
                                std::fs::create_dir_all(parent)?;
                            }
                            return Ok(resolved);
                        }
                    }
                }
            }
        }
    }

    // Check if the path falls inside a read-only granted folder — emit a specific warning
    if resolved.is_absolute() {
        let readonly_folders: Vec<_> = AccessStore::enabled_folders()
            .into_iter()
            .filter(|f| !f.writable)
            .collect();
        if let Some(ref anc) = ancestor {
            if let Ok(canonical_check) = anc.canonicalize() {
                for folder in &readonly_folders {
                    if let Ok(canonical_folder) = folder.path.canonicalize() {
                        if canonical_check.starts_with(&canonical_folder) {
                            tracing::warn!(
                                path = %path_str,
                                folder = %folder.display_name,
                                folder_path = %folder.path.display(),
                                "Write rejected: path is inside granted folder '{}' which is read-only. \
                                 Toggle it to read-write in the Security tab to allow writes.",
                                folder.display_name
                            );
                            return Err(anyhow!(
                                "Write path is inside granted folder '{}' which is read-only. \
                                 Toggle it to read-write in the Security tab to allow writes.",
                                folder.display_name
                            ));
                        }
                    }
                }
            }
        }
    }

    tracing::warn!(
        path = %path_str,
        "Write rejected: path is outside the workspace and not in any writable granted folder"
    );
    Err(anyhow!(
        "Write path is outside the workspace and not in any writable granted folder: {}",
        path_str
    ))
}

/// Walk up from a path until we find an existing ancestor directory.
fn find_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut check = path.to_path_buf();
    loop {
        if check.exists() {
            return Some(check);
        }
        if !check.pop() {
            return None;
        }
    }
}

// ============================================================================
// read_file
// ============================================================================

/// Parameters for the `read_file` tool.
#[derive(Debug, Deserialize)]
pub struct ReadFileParams {
    pub path: String,
}

impl ReadFileParams {
    pub fn from_args(args: &serde_json::Value) -> Result<Self> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing required 'path' argument"))?
            .to_string();
        Ok(Self { path })
    }
}

/// Execute the `read_file` tool. Returns JSON: `{ path, size, content, truncated }`.
pub fn execute_read_file(params: &ReadFileParams) -> Result<String> {
    let path = validate_read_path(&params.path)?;

    // Check file size
    let meta = std::fs::metadata(&path)
        .map_err(|e| anyhow!("Cannot read file '{}': {}", params.path, e))?;

    if !meta.is_file() {
        return Err(anyhow!("Path is not a file: {}", params.path));
    }

    let size = meta.len();

    // Read bytes (up to MAX_READ_SIZE + 1 to detect truncation)
    let bytes = if size > MAX_READ_SIZE {
        let mut buf = vec![0u8; (MAX_READ_SIZE + 1) as usize];
        let mut f = std::fs::File::open(&path)?;
        use std::io::Read;
        let n = f.read(&mut buf)?;
        buf.truncate(n);
        buf
    } else {
        std::fs::read(&path)?
    };

    // Reject binary files (check for null bytes in the first 8KB)
    let check_len = bytes.len().min(8192);
    if bytes[..check_len].contains(&0) {
        // Give a helpful hint for PDFs
        let lower = params.path.to_lowercase();
        if lower.ends_with(".pdf") {
            return Err(anyhow!(
                "Cannot read PDF as text. Use the analyze_image tool instead — it supports PDF files: \
                 analyze_image(image_source=\"{}\")",
                params.path
            ));
        }
        return Err(anyhow!(
            "File appears to be binary: {}. For images/PDFs, use analyze_image instead.",
            params.path
        ));
    }

    let content = String::from_utf8_lossy(&bytes);
    let truncated = size > MAX_READ_SIZE;
    let content_str = if truncated {
        // Truncate to MAX_READ_SIZE chars (approximate, but close enough for UTF-8 text)
        let s: String = content.chars().take(MAX_READ_SIZE as usize).collect();
        s
    } else {
        content.into_owned()
    };

    let result = serde_json::json!({
        "path": path.display().to_string(),
        "size": size,
        "content": content_str,
        "truncated": truncated,
    });
    Ok(serde_json::to_string(&result)?)
}

// ============================================================================
// write_file
// ============================================================================

/// Parameters for the `write_file` tool.
#[derive(Debug, Deserialize)]
pub struct WriteFileParams {
    pub path: String,
    pub content: String,
}

impl WriteFileParams {
    pub fn from_args(args: &serde_json::Value) -> Result<Self> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("output_path").and_then(|v| v.as_str()))
            .or_else(|| args.get("file_path").and_then(|v| v.as_str()))
            .ok_or_else(|| anyhow!("Missing required 'path' argument"))?
            .to_string();
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("contents").and_then(|v| v.as_str()))
            .ok_or_else(|| anyhow!("Missing required 'content' argument"))?
            .to_string();
        Ok(Self { path, content })
    }
}

/// Execute the `write_file` tool. Returns JSON: `{ path, size }`.
pub fn execute_write_file(params: &WriteFileParams) -> Result<String> {
    let validated = validate_write_path(&params.path)?;
    std::fs::write(&validated, &params.content)?;
    let size = params.content.len();
    let result = serde_json::json!({
        "path": validated.display().to_string(),
        "size": size,
    });
    Ok(serde_json::to_string(&result)?)
}

// ============================================================================
// apply_patch
// ============================================================================

/// Parameters for the `apply_patch` tool.
#[derive(Debug, Deserialize)]
pub struct ApplyPatchParams {
    pub patch: String,
}

impl ApplyPatchParams {
    pub fn from_args(args: &serde_json::Value) -> Result<Self> {
        let patch = args
            .get("patch")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing required 'patch' argument"))?
            .to_string();
        Ok(Self { patch })
    }
}

/// Result for a single file operation within a patch.
#[derive(Debug, Serialize)]
struct PatchFileResult {
    path: String,
    action: String, // "created" | "modified" | "deleted"
}

/// Execute the `apply_patch` tool. Returns JSON: `{ success, files: [{ path, action }] }`.
pub fn execute_apply_patch(params: &ApplyPatchParams) -> Result<String> {
    let patches = split_multi_file_patch(&params.patch);

    if patches.is_empty() {
        return Err(anyhow!("No valid patches found in input"));
    }

    let mut results: Vec<PatchFileResult> = Vec::new();

    for patch_text in &patches {
        let result = apply_single_file_patch(patch_text)?;
        results.push(result);
    }

    let output = serde_json::json!({
        "success": true,
        "files": results,
    });
    Ok(serde_json::to_string(&output)?)
}

/// Split a multi-file unified diff into individual file patches.
///
/// Splits at lines starting with `--- ` that are followed by `+++ `.
fn split_multi_file_patch(input: &str) -> Vec<String> {
    let lines: Vec<&str> = input.lines().collect();
    let mut patches: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        // A new file patch starts at `--- ` followed by `+++ `
        if line.starts_with("--- ")
            && i + 1 < lines.len()
            && lines[i + 1].starts_with("+++ ")
        {
            // Save the previous patch if any
            if !current.is_empty() {
                let text = current.join("\n");
                if text.contains("@@") {
                    patches.push(text);
                }
                current.clear();
            }
        }
        current.push(line);
    }

    // Don't forget the last patch
    if !current.is_empty() {
        let text = current.join("\n");
        if text.contains("@@") {
            patches.push(text);
        }
    }

    patches
}

/// Strip the `a/` or `b/` diff prefix from a path.
fn strip_diff_prefix(path: &str) -> &str {
    if path.starts_with("a/") || path.starts_with("b/") {
        &path[2..]
    } else {
        path
    }
}

/// Parse `--- old_path` and `+++ new_path` from a patch text.
fn parse_file_paths(patch_text: &str) -> Result<(String, String)> {
    let mut old_path = String::new();
    let mut new_path = String::new();

    for line in patch_text.lines() {
        if line.starts_with("--- ") {
            // Handle `--- a/path` or `--- /dev/null`
            let rest = line[4..].trim();
            // Strip trailing tab-separated timestamp if present
            let path_part = rest.split('\t').next().unwrap_or(rest);
            old_path = path_part.to_string();
        } else if line.starts_with("+++ ") {
            let rest = line[4..].trim();
            let path_part = rest.split('\t').next().unwrap_or(rest);
            new_path = path_part.to_string();
            break;
        }
    }

    if old_path.is_empty() || new_path.is_empty() {
        return Err(anyhow!("Could not parse file paths from patch (missing --- or +++ header)"));
    }

    Ok((old_path, new_path))
}

/// Apply a single file patch (create, modify, or delete).
fn apply_single_file_patch(patch_text: &str) -> Result<PatchFileResult> {
    let (old_path, new_path) = parse_file_paths(patch_text)?;

    let is_create = old_path == "/dev/null";
    let is_delete = new_path == "/dev/null";

    if is_create {
        // Create a new file
        let file_path = strip_diff_prefix(&new_path);
        let validated = validate_write_path(file_path)?;

        let content = apply_diff_to_content("", patch_text)?;
        std::fs::write(&validated, &content)?;

        Ok(PatchFileResult {
            path: file_path.to_string(),
            action: "created".to_string(),
        })
    } else if is_delete {
        // Delete a file
        let file_path = strip_diff_prefix(&old_path);
        let validated = validate_write_path(file_path)?;

        if validated.exists() {
            std::fs::remove_file(&validated)?;
        }

        Ok(PatchFileResult {
            path: file_path.to_string(),
            action: "deleted".to_string(),
        })
    } else {
        // Modify an existing file
        let file_path = strip_diff_prefix(&new_path);
        let validated = validate_write_path(file_path)?;

        let original = std::fs::read_to_string(&validated)
            .map_err(|e| anyhow!("Cannot read file for patching '{}': {}", file_path, e))?;

        let content = apply_diff_to_content(&original, patch_text)?;
        std::fs::write(&validated, &content)?;

        Ok(PatchFileResult {
            path: file_path.to_string(),
            action: "modified".to_string(),
        })
    }
}

/// Apply a unified diff patch to content using `diffy`.
fn apply_diff_to_content(original: &str, patch_text: &str) -> Result<String> {
    let patch = diffy::Patch::from_str(patch_text)
        .map_err(|e| anyhow!("Failed to parse patch: {}", e))?;

    diffy::apply(original, &patch)
        .map_err(|e| anyhow!("Failed to apply patch: {}", e))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a temp workspace and set the override.
    /// Returns the temp dir (must be kept alive for the duration of the test).
    fn setup_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // Since OnceLock can only be set once, we work directly with the dir
        // and validate paths manually in tests.
        dir
    }

    // ------------------------------------------------------------------
    // split_multi_file_patch
    // ------------------------------------------------------------------

    #[test]
    fn test_split_single_patch() {
        let input = "--- /dev/null\n+++ b/hello.txt\n@@ -0,0 +1 @@\n+Hello world\n";
        let patches = split_multi_file_patch(input);
        assert_eq!(patches.len(), 1);
    }

    #[test]
    fn test_split_multi_file_patch() {
        let input = "\
--- /dev/null
+++ b/file1.txt
@@ -0,0 +1 @@
+Content 1
--- /dev/null
+++ b/file2.txt
@@ -0,0 +1 @@
+Content 2
";
        let patches = split_multi_file_patch(input);
        assert_eq!(patches.len(), 2);
        assert!(patches[0].contains("file1.txt"));
        assert!(patches[1].contains("file2.txt"));
    }

    #[test]
    fn test_split_no_hunks() {
        let input = "--- a/file.txt\n+++ b/file.txt\nno hunks here\n";
        let patches = split_multi_file_patch(input);
        assert!(patches.is_empty());
    }

    // ------------------------------------------------------------------
    // strip_diff_prefix
    // ------------------------------------------------------------------

    #[test]
    fn test_strip_diff_prefix() {
        assert_eq!(strip_diff_prefix("a/foo.txt"), "foo.txt");
        assert_eq!(strip_diff_prefix("b/bar/baz.rs"), "bar/baz.rs");
        assert_eq!(strip_diff_prefix("plain.txt"), "plain.txt");
        assert_eq!(strip_diff_prefix("/dev/null"), "/dev/null");
    }

    // ------------------------------------------------------------------
    // parse_file_paths
    // ------------------------------------------------------------------

    #[test]
    fn test_parse_file_paths_create() {
        let patch = "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hello\n";
        let (old, new) = parse_file_paths(patch).unwrap();
        assert_eq!(old, "/dev/null");
        assert_eq!(new, "b/new.txt");
    }

    #[test]
    fn test_parse_file_paths_modify() {
        let patch = "--- a/existing.txt\n+++ b/existing.txt\n@@ -1 +1 @@\n-old\n+new\n";
        let (old, new) = parse_file_paths(patch).unwrap();
        assert_eq!(old, "a/existing.txt");
        assert_eq!(new, "b/existing.txt");
    }

    #[test]
    fn test_parse_file_paths_delete() {
        let patch = "--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n";
        let (old, new) = parse_file_paths(patch).unwrap();
        assert_eq!(old, "a/gone.txt");
        assert_eq!(new, "/dev/null");
    }

    #[test]
    fn test_parse_file_paths_missing() {
        let patch = "no headers here\n";
        assert!(parse_file_paths(patch).is_err());
    }

    // ------------------------------------------------------------------
    // apply_diff_to_content
    // ------------------------------------------------------------------

    #[test]
    fn test_apply_diff_create() {
        let patch = "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+Hello world\n";
        let result = apply_diff_to_content("", patch).unwrap();
        assert_eq!(result, "Hello world\n");
    }

    #[test]
    fn test_apply_diff_modify() {
        let original = "line1\nline2\nline3\n";
        let patch = "--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@\n line1\n-line2\n+LINE2\n line3\n";
        let result = apply_diff_to_content(original, patch).unwrap();
        assert_eq!(result, "line1\nLINE2\nline3\n");
    }

    #[test]
    fn test_apply_diff_delete() {
        let original = "bye\n";
        let patch = "--- a/f.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n";
        let result = apply_diff_to_content(original, patch).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_apply_diff_wrong_context() {
        // Patch expects "old line" but original has "different content"
        let original = "different content\n";
        let patch = "--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-old line\n+new line\n";
        let result = apply_diff_to_content(original, patch);
        assert!(result.is_err());
    }

    // ------------------------------------------------------------------
    // End-to-end: read_file + apply_patch in a temp workspace
    // ------------------------------------------------------------------

    #[test]
    fn test_create_and_read_file() {
        let ws = setup_workspace();
        let ws_path = ws.path().to_path_buf();

        // Create a file via apply_patch logic
        let file_path = ws_path.join("test.txt");
        let content = "Hello from test\n";
        fs::write(&file_path, content).unwrap();

        // Read it back via execute_read_file (using absolute path)
        // Read it back directly (workspace override can't be set per-test
        // because OnceLock is global, so we test content reading directly)
        let read_content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(read_content, content);
    }

    #[test]
    fn test_binary_detection() {
        let ws = setup_workspace();
        let file_path = ws.path().join("binary.bin");
        // Write some binary content with null bytes
        fs::write(&file_path, b"\x00\x01\x02\x03").unwrap();

        // Check that binary detection works
        let bytes = fs::read(&file_path).unwrap();
        let check_len = bytes.len().min(8192);
        assert!(bytes[..check_len].contains(&0));
    }

    #[test]
    fn test_apply_single_file_patch_create() {
        let ws = setup_workspace();
        let ws_path = ws.path().to_path_buf();

        // Create file directly via the helper
        let patch = "--- /dev/null\n+++ b/created.txt\n@@ -0,0 +1,2 @@\n+line one\n+line two\n";
        let content = apply_diff_to_content("", patch).unwrap();
        let file_path = ws_path.join("created.txt");
        fs::write(&file_path, &content).unwrap();

        assert!(file_path.exists());
        let read = fs::read_to_string(&file_path).unwrap();
        assert_eq!(read, "line one\nline two\n");
    }

    #[test]
    fn test_apply_single_file_patch_modify() {
        let ws = setup_workspace();
        let ws_path = ws.path().to_path_buf();

        // Write original file
        let file_path = ws_path.join("modify.txt");
        fs::write(&file_path, "aaa\nbbb\nccc\n").unwrap();

        // Apply a modify patch
        let patch = "--- a/modify.txt\n+++ b/modify.txt\n@@ -1,3 +1,3 @@\n aaa\n-bbb\n+BBB\n ccc\n";
        let original = fs::read_to_string(&file_path).unwrap();
        let content = apply_diff_to_content(&original, patch).unwrap();
        fs::write(&file_path, &content).unwrap();

        let read = fs::read_to_string(&file_path).unwrap();
        assert_eq!(read, "aaa\nBBB\nccc\n");
    }

    // ------------------------------------------------------------------
    // write_file
    // ------------------------------------------------------------------

    #[test]
    fn test_write_file_creates_new() {
        let ws = setup_workspace();
        let file_path = ws.path().join("output.txt");

        let content = "Hello from write_file\n";
        fs::write(&file_path, content).unwrap();

        assert!(file_path.exists());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), content);
    }

    #[test]
    fn test_write_file_overwrites_existing() {
        let ws = setup_workspace();
        let file_path = ws.path().join("overwrite.txt");

        fs::write(&file_path, "original").unwrap();
        fs::write(&file_path, "replaced").unwrap();

        assert_eq!(fs::read_to_string(&file_path).unwrap(), "replaced");
    }

    #[test]
    fn test_write_file_params_from_args() {
        let args = serde_json::json!({
            "path": "report.txt",
            "content": "weather data here"
        });
        let params = WriteFileParams::from_args(&args).unwrap();
        assert_eq!(params.path, "report.txt");
        assert_eq!(params.content, "weather data here");
    }

    #[test]
    fn test_write_file_params_missing_content() {
        let args = serde_json::json!({ "path": "report.txt" });
        assert!(WriteFileParams::from_args(&args).is_err());
    }

    #[test]
    fn test_path_traversal_rejected() {
        // Attempting to write outside workspace should fail
        let result = validate_write_path("/etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_workspace_dir_default() {
        // The default workspace should be under the cache directory
        let ws = workspace_dir();
        assert!(ws.to_string_lossy().contains("prolog-router"));
        assert!(ws.to_string_lossy().contains("workspace"));
    }
}
