//! PDF Generation via Swift helper (macOS only)
//!
//! Uses a compiled Swift binary (`pdf-helper`) that leverages WKWebView to render
//! HTML to PDF. The helper uses JSON stdin/stdout protocol, same pattern as
//! `reminders-helper`.

use crate::image_security;

use anyhow::{anyhow, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tracing::{info, warn};

// ============================================================================
// Swift Helper Binary
// ============================================================================

/// Cached path to the Swift pdf-helper binary (None = not yet found).
/// Uses Mutex instead of OnceLock so a failed lookup can be retried
/// (e.g. when SCRIPTS_DIR_OVERRIDE is set after the first call).
static HELPER_PATH: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Search for the compiled Swift helper binary in known locations.
fn find_helper_binary() -> Option<PathBuf> {
    // 1. SCRIPTS_DIR_OVERRIDE (used by Tauri bundled resources)
    if let Some(override_dir) = std::env::var_os("SCRIPTS_DIR_OVERRIDE") {
        let p = PathBuf::from(override_dir).join("pdf-helper");
        if p.is_file() {
            return Some(p);
        }
    }

    // 2. Dev paths relative to CWD
    let dev_paths = [
        "swift/pdf-helper/.build/release/pdf-helper",
        "swift/pdf-helper/.build/debug/pdf-helper",
        "../../swift/pdf-helper/.build/release/pdf-helper",
        "../../swift/pdf-helper/.build/debug/pdf-helper",
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
            // Bundled macOS app: Contents/MacOS/../Helpers/pdf-helper (preferred)
            let p = exe_dir.join("../Helpers/pdf-helper");
            if p.is_file() {
                return Some(p);
            }
            // Bundled macOS app: Contents/MacOS/../Resources/pdf-helper (legacy)
            let p = exe_dir.join("../Resources/pdf-helper");
            if p.is_file() {
                return Some(p);
            }
        }
    }

    // 4. target/swift path (built by build.sh)
    let target_paths = [
        "target/swift/pdf-helper",
        "../../target/swift/pdf-helper",
    ];
    for rel in &target_paths {
        let p = PathBuf::from(rel);
        if p.is_file() {
            return Some(p);
        }
    }

    // 4. CARGO_MANIFEST_DIR-relative (for cargo test/run)
    if let Some(manifest_dir) = std::env::var_os("CARGO_MANIFEST_DIR") {
        let base = PathBuf::from(manifest_dir);
        for suffix in &[
            "../../swift/pdf-helper/.build/release/pdf-helper",
            "../../swift/pdf-helper/.build/debug/pdf-helper",
            "../../target/swift/pdf-helper",
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
        let mut guard = HELPER_PATH.lock().unwrap();
        *guard = Some(path.clone());
        Some(path)
    } else {
        None
    }
}

/// Run the Swift helper binary with the given JSON input.
/// Returns the stdout output on success.
fn run_helper(args: &Value) -> Result<String> {
    let binary = helper_binary()
        .ok_or_else(|| anyhow!("Swift pdf-helper binary not found"))?;

    let mut child = Command::new(binary)
        .arg("generate")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn pdf-helper: {}", e))?;

    // Write JSON to stdin
    if let Some(mut stdin) = child.stdin.take() {
        let json_bytes = serde_json::to_vec(args)?;
        stdin.write_all(&json_bytes)?;
        // stdin is dropped here, closing the pipe
    }

    let output = child
        .wait_with_output()
        .map_err(|e| anyhow!("Failed to wait for pdf-helper: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        // Try to extract error message from stderr JSON
        let stderr = String::from_utf8_lossy(&output.stderr);
        if let Ok(err_json) = serde_json::from_str::<Value>(&stderr) {
            if let Some(msg) = err_json.get("message").and_then(|v| v.as_str()) {
                return Err(anyhow!("{}", msg));
            }
        }
        let code = output.status.code().unwrap_or(-1);
        Err(anyhow!(
            "pdf-helper exited with code {}: {}",
            code,
            stderr.trim()
        ))
    }
}

// ============================================================================
// Remote Image Resolution
// ============================================================================

/// Maximum number of remote images to download per PDF.
const MAX_REMOTE_IMAGES: usize = 5;

/// Small SVG placeholder for failed image downloads.
pub const PLACEHOLDER_SVG: &str = r#"data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIyMDAiIGhlaWdodD0iMTAwIj48cmVjdCB3aWR0aD0iMjAwIiBoZWlnaHQ9IjEwMCIgZmlsbD0iI2YwZjBmMCIgc3Ryb2tlPSIjY2NjIi8+PHRleHQgeD0iNTAlIiB5PSI1MCUiIHRleHQtYW5jaG9yPSJtaWRkbGUiIGR5PSIuM2VtIiBmb250LWZhbWlseT0ic2Fucy1zZXJpZiIgZm9udC1zaXplPSIxMiIgZmlsbD0iIzk5OSI+SW1hZ2UgdW5hdmFpbGFibGU8L3RleHQ+PC9zdmc+"#;

/// Validate a URL through the image security pipeline and return a data URI or placeholder.
fn resolve_image_url(url: &str) -> String {
    let result = image_security::validate_image(url);
    if result.safe {
        if let Some(data_uri) = result.data_uri {
            info!("Resolved remote image via security pipeline: {}", url);
            return data_uri;
        }
    }
    if let Some(ref rejection) = result.rejection {
        warn!("Image rejected ({}): {:?}", url, rejection);
    }
    PLACEHOLDER_SVG.to_string()
}

/// Validate a local file through the image security pipeline and return a data URI or placeholder.
fn resolve_local_image(path: &str) -> String {
    let result = image_security::validate_local_image(path);
    if result.safe {
        if let Some(data_uri) = result.data_uri {
            info!("Resolved local image via security pipeline: {}", path);
            return data_uri;
        }
    }
    if let Some(ref rejection) = result.rejection {
        warn!("Local image rejected ({}): {:?}", path, rejection);
    }
    PLACEHOLDER_SVG.to_string()
}

/// Download remote images in HTML and replace with base64 data URIs.
/// Also resolves file:// URLs by reading the local file.
/// Failed downloads are replaced with a small placeholder SVG.
/// All images pass through the full security pipeline (magic bytes, decode,
/// re-encode to PNG, tracking pixel detection).
pub fn resolve_remote_images(html: &str) -> String {
    let mut result = html.to_string();
    let mut resolved_count = 0usize;

    // 1. Handle <img src="..."> tags (http/https remote + file:// local)
    let img_re = Regex::new(r#"<img([^>]*?)src\s*=\s*["']([^"']+)["']"#).unwrap();
    let urls: Vec<(String, String)> = img_re
        .captures_iter(html)
        .filter_map(|cap| {
            let full_match = cap.get(0)?.as_str().to_string();
            let url = cap.get(2)?.as_str().to_string();
            if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file:///") {
                Some((full_match, url))
            } else {
                None
            }
        })
        .collect();

    for (full_match, url) in urls {
        if resolved_count >= MAX_REMOTE_IMAGES {
            info!("Reached max remote image limit ({}), skipping remaining", MAX_REMOTE_IMAGES);
            break;
        }
        let data_uri = if url.starts_with("file:///") {
            let path = &url["file://".len()..]; // strip "file://" to get absolute path
            resolve_local_image(path)
        } else {
            resolve_image_url(&url)
        };
        let replacement = full_match.replace(&url, &data_uri);
        result = result.replacen(&full_match, &replacement, 1);
        resolved_count += 1;
    }

    // 2. Handle <img src="/absolute/path"> (bare local paths, not file:// URLs)
    let local_img_re = Regex::new(r#"<img([^>]*?)src\s*=\s*["'](/[^"']+)["']"#).unwrap();
    let local_paths: Vec<(String, String)> = local_img_re
        .captures_iter(&result.clone())
        .filter_map(|cap| {
            let full_match = cap.get(0)?.as_str().to_string();
            let path = cap.get(2)?.as_str().to_string();
            // Skip if already resolved to data: URI in pass 1
            if full_match.contains("data:image") {
                return None;
            }
            Some((full_match, path))
        })
        .collect();

    for (full_match, path) in local_paths {
        if resolved_count >= MAX_REMOTE_IMAGES {
            info!("Reached max remote image limit ({}), skipping remaining", MAX_REMOTE_IMAGES);
            break;
        }
        let data_uri = resolve_local_image(&path);
        let replacement = full_match.replace(&path, &data_uri);
        result = result.replacen(&full_match, &replacement, 1);
        resolved_count += 1;
    }

    // 3. Handle CSS background-image: url(...)
    let css_re = Regex::new(r#"url\(\s*["']?(https?://[^"')]+)["']?\s*\)"#).unwrap();
    let css_urls: Vec<(String, String)> = css_re
        .captures_iter(&result.clone())
        .filter_map(|cap| {
            let full_match = cap.get(0)?.as_str().to_string();
            let url = cap.get(1)?.as_str().to_string();
            Some((full_match, url))
        })
        .collect();

    for (full_match, url) in css_urls {
        if resolved_count >= MAX_REMOTE_IMAGES {
            info!("Reached max remote image limit ({}), skipping remaining", MAX_REMOTE_IMAGES);
            break;
        }
        let validated = image_security::validate_image(&url);
        if validated.safe {
            if let Some(data_uri) = validated.data_uri {
                let replacement = format!("url(\"{}\")", data_uri);
                result = result.replacen(&full_match, &replacement, 1);
                resolved_count += 1;
            }
        }
        // For CSS background images, keep original URL on failure rather than placeholder
    }

    result
}

// ============================================================================
// Public API
// ============================================================================

/// Check if PDF generation is available (macOS with helper binary).
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

/// Detect whether content looks like markdown (vs plain prose).
/// Checks for common markdown constructs like headings, tables, links, images, code blocks.
fn looks_like_markdown(content: &str) -> bool {
    let lines: Vec<&str> = content.lines().take(50).collect();
    let mut score = 0u32;
    for line in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') { score += 2; }
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") { score += 1; }
        if trimmed.starts_with('|') && trimmed.ends_with('|') { score += 2; }
        if trimmed.starts_with("```") { score += 2; }
        if trimmed.contains("![") || trimmed.contains("](") { score += 2; }
        if trimmed.starts_with("> ") { score += 1; }
        if trimmed.starts_with("---") || trimmed.starts_with("***") { score += 1; }
    }
    score >= 2
}

/// Convert markdown content to an HTML body fragment using pulldown-cmark.
fn markdown_to_html(content: &str) -> String {
    use pulldown_cmark::{Options, Parser, html};
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(content, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

/// Auto-link bare URLs in HTML that aren't already inside an `<a>` tag.
/// Matches `https://...` sequences that appear as plain text (not already in
/// `href="..."` or `src="..."`).  Turns them into clickable `<a>` links.
fn auto_link_urls(html: &str) -> String {
    let url_re = Regex::new(r#"(https?://[^\s<>"'\)]+)"#).unwrap();

    // Two-pass approach: find URLs, check if each is already inside an attribute,
    // replace only bare ones.  We iterate in reverse so byte offsets stay valid.
    let matches: Vec<(usize, usize, &str)> = url_re
        .find_iter(html)
        .map(|m| (m.start(), m.end(), m.as_str()))
        .collect();

    let mut result = html.to_string();
    for &(start, _end, url) in matches.iter().rev() {
        // Look back for the nearest `"` or `'` — if it's an attribute value, skip.
        // Also skip URLs that are already the text content of an <a> tag (the
        // `>` check catches `<a href="...">https://...` produced by pulldown-cmark).
        let before = &html[..start];
        let already_linked = before.ends_with("href=\"")
            || before.ends_with("href='")
            || before.ends_with("src=\"")
            || before.ends_with("src='")
            || before.ends_with("url(\"")
            || before.ends_with("url('")
            || is_inside_a_tag(before);
        if already_linked {
            continue;
        }
        let link = format!(
            r#"<a href="{url}" style="color:#1a73e8;text-decoration:none">{url}</a>"#
        );
        result.replace_range(start.._end, &link);
    }
    result
}

/// Returns true when `before` (the HTML preceding a URL match) indicates the URL
/// sits inside the text content of an `<a>` tag.  We scan backwards for the most
/// recent unmatched `<a` or `</a` — if it's an opening tag, the URL is link text.
fn is_inside_a_tag(before: &str) -> bool {
    // Walk backwards through `before` looking for `<a ` or `</a`.
    let mut depth: i32 = 0;
    let bytes = before.as_bytes();
    let mut i = bytes.len();
    while i >= 2 {
        i -= 1;
        if i >= 3 && &bytes[i - 3..=i] == b"</a>" {
            depth += 1; // closing tag pushes us out
        } else if bytes[i] == b'<' && i + 1 < bytes.len() && (bytes[i + 1] == b'a' || bytes[i + 1] == b'A') {
            if i + 2 < bytes.len() && (bytes[i + 2] == b' ' || bytes[i + 2] == b'>') {
                if depth > 0 {
                    depth -= 1; // matched with a closing tag
                } else {
                    return true; // unmatched opening <a — we're inside
                }
            }
        }
    }
    false
}

/// Check whether a title and a markdown heading are redundant via word overlap.
/// Normalises to lowercase, strips punctuation, and computes Jaccard similarity
/// on the resulting word sets.  Threshold of 0.5 catches near-duplicates like
/// "Daily Weather report for 2026-02-26" vs "Daily Weather Report — 2026-02-26"
/// while allowing genuinely different titles through.
fn title_resembles_heading(title: &str, heading: &str) -> bool {
    fn words(s: &str) -> std::collections::HashSet<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }
    let a = words(title);
    let b = words(heading);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let intersection = a.intersection(&b).count();
    let union = a.union(&b).count();
    (intersection as f64 / union as f64) >= 0.5
}

/// Wrap content in a styled HTML template.
///
/// Detection order:
/// 1. If content starts with `<`, it's treated as raw HTML and passed through.
/// 2. If content looks like markdown (headings, tables, links, code blocks),
///    it's converted to HTML via pulldown-cmark.
/// 3. Otherwise, plain text lines are wrapped in `<p>` tags.
pub fn prepare_html(content: &str, title: Option<&str>) -> String {
    let trimmed = content.trim_start();
    if trimmed.starts_with('<') {
        return content.to_string();
    }

    let is_markdown = looks_like_markdown(content);

    // Skip the injected title when the markdown content already starts with a
    // heading that says roughly the same thing — the LLM often puts the same
    // info in both, producing a redundant duplicate line at the top of the PDF.
    let title_redundant = is_markdown && title.map_or(false, |t| {
        let first_line = content.trim_start().lines().next().unwrap_or("");
        first_line.starts_with("# ") && title_resembles_heading(t, &first_line[2..])
    });
    let title_block = match title {
        Some(t) if !t.is_empty() && !title_redundant => {
            format!("<h1>{}</h1>\n", html_escape(t))
        }
        _ => String::new(),
    };

    let body = if is_markdown {
        markdown_to_html(content)
    } else {
        content
            .lines()
            .map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    String::new()
                } else {
                    format!("<p>{}</p>", html_escape(trimmed))
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Turn bare URLs into clickable links (LLMs often emit `URL: https://...`
    // as plain text rather than markdown links).
    let body = auto_link_urls(&body);

    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8">
<style>
body {{ font-family: -apple-system, Helvetica, sans-serif; margin: 40px; line-height: 1.6; color: #333; }}
h1 {{ font-size: 24px; margin-bottom: 16px; }}
h2 {{ font-size: 20px; margin-top: 24px; }}
h3 {{ font-size: 16px; margin-top: 20px; }}
pre {{ background: #f5f5f5; padding: 12px; border-radius: 4px; overflow-x: auto; }}
code {{ background: #f5f5f5; padding: 2px 6px; border-radius: 3px; font-size: 0.9em; }}
pre code {{ background: none; padding: 0; }}
table {{ border-collapse: collapse; width: 100%; margin: 16px 0; }}
th, td {{ border: 1px solid #ddd; padding: 8px 12px; text-align: left; }}
th {{ background: #f5f5f5; font-weight: 600; }}
tr:nth-child(even) {{ background: #fafafa; }}
blockquote {{ border-left: 4px solid #ddd; margin: 16px 0; padding: 8px 16px; color: #555; }}
img {{ max-width: 100%; height: auto; }}
a {{ color: #1a73e8; text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
</style></head>
<body>
{title_block}{body}
</body></html>"#
    )
}

/// Basic HTML escaping for text content.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Create a PDF from text or HTML content, writing to the given output path.
pub fn execute_create_pdf(
    output_path: &str,
    content: &str,
    title: Option<&str>,
) -> Result<String> {
    let html = prepare_html(content, title);
    let html = resolve_remote_images(&html);
    let input = json!({
        "html": html,
        "output_path": output_path,
    });

    run_helper(&input)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prepare_html_plain_text() {
        let result = prepare_html("Hello world\nSecond line", None);
        assert!(result.contains("<p>Hello world</p>"));
        assert!(result.contains("<p>Second line</p>"));
        assert!(result.contains("<!DOCTYPE html>"));
        assert!(!result.contains("<h1>"));
    }

    #[test]
    fn test_prepare_html_with_title() {
        let result = prepare_html("Body text", Some("My Title"));
        assert!(result.contains("<h1>My Title</h1>"));
        assert!(result.contains("<p>Body text</p>"));
    }

    #[test]
    fn test_prepare_html_passthrough() {
        let html = "<html><body><h1>Already HTML</h1></body></html>";
        let result = prepare_html(html, Some("Ignored Title"));
        assert_eq!(result, html);
    }

    #[test]
    fn test_prepare_html_html_escaping() {
        let result = prepare_html("x < y & z > w", None);
        assert!(result.contains("<p>x &lt; y &amp; z &gt; w</p>"));
    }

    #[test]
    fn test_prepare_html_empty_lines() {
        let result = prepare_html("First\n\nThird", None);
        assert!(result.contains("<p>First</p>"));
        assert!(result.contains("<p>Third</p>"));
    }

    #[test]
    fn test_prepare_html_markdown_heading() {
        let md = "# Weather Report\n\nSunny with a high of 75°F.";
        let result = prepare_html(md, None);
        assert!(result.contains("<h1>Weather Report</h1>"));
        assert!(result.contains("Sunny with a high of 75°F."));
        assert!(result.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn test_prepare_html_markdown_table() {
        let md = "## Forecast\n\n| Day | High | Low |\n|-----|------|-----|\n| Mon | 70 | 55 |\n| Tue | 72 | 58 |";
        let result = prepare_html(md, None);
        assert!(result.contains("<table>"));
        assert!(result.contains("<th>Day</th>"));
        assert!(result.contains("<td>Mon</td>"));
    }

    #[test]
    fn test_prepare_html_markdown_image() {
        let md = "# Report\n\n![weather](https://example.com/img.png)";
        let result = prepare_html(md, None);
        assert!(result.contains("<img"));
        assert!(result.contains("https://example.com/img.png"));
    }

    #[test]
    fn test_prepare_html_markdown_with_title() {
        let md = "## Section\n\nSome content here.";
        let result = prepare_html(md, Some("My Report"));
        // Title from param is an h1, markdown h2 is preserved
        assert!(result.contains("<h1>My Report</h1>"));
        assert!(result.contains("<h2>Section</h2>"));
    }

    #[test]
    fn test_title_resembles_heading() {
        // Near-identical (different case, dash vs em-dash)
        assert!(title_resembles_heading(
            "Daily Weather report for 2026-02-26",
            "Daily Weather Report — 2026-02-26"
        ));
        // Exact match
        assert!(title_resembles_heading("My Report", "My Report"));
        // Genuinely different
        assert!(!title_resembles_heading("Weather Summary", "Detailed Forecast by Region"));
        // Empty
        assert!(!title_resembles_heading("", "Something"));
        assert!(!title_resembles_heading("Something", ""));
    }

    #[test]
    fn test_prepare_html_markdown_with_similar_heading_skips_title() {
        // LLM provides title + markdown heading that say the same thing
        let md = "# Daily Weather Report — 2026-02-26\n\nLocation: Ashburn, VA";
        let result = prepare_html(md, Some("Daily Weather report for 2026-02-26"));
        assert_eq!(result.matches("<h1>").count(), 1);
    }

    #[test]
    fn test_prepare_html_markdown_with_different_heading_keeps_title() {
        // Title and heading are genuinely different — keep both
        let md = "# Detailed Forecast by Region\n\nNortheast: Rain expected.";
        let result = prepare_html(md, Some("Weather Summary"));
        assert_eq!(result.matches("<h1>").count(), 2);
        assert!(result.contains("<h1>Weather Summary</h1>"));
        assert!(result.contains("<h1>Detailed Forecast by Region</h1>"));
    }

    #[test]
    fn test_looks_like_markdown_positive() {
        assert!(looks_like_markdown("# Heading\n\nSome text"));
        assert!(looks_like_markdown("| A | B |\n|---|---|\n| 1 | 2 |"));
        assert!(looks_like_markdown("Check this ![img](url) out"));
    }

    #[test]
    fn test_looks_like_markdown_negative() {
        assert!(!looks_like_markdown("Just plain text here"));
        assert!(!looks_like_markdown("No special formatting at all."));
    }

    #[test]
    fn test_auto_link_bare_url() {
        let html = "<p>URL: https://www.cnn.com/2026/02/25/tech/article</p>";
        let result = auto_link_urls(html);
        assert!(result.contains(r#"<a href="https://www.cnn.com/2026/02/25/tech/article""#));
        assert!(result.contains("</a>"));
    }

    #[test]
    fn test_auto_link_skips_existing_href() {
        let html = r#"<a href="https://example.com">link</a>"#;
        let result = auto_link_urls(html);
        // Should not double-wrap
        assert_eq!(result.matches("<a ").count(), 1);
    }

    #[test]
    fn test_auto_link_skips_img_src() {
        let html = r#"<img src="https://example.com/img.png" alt="photo">"#;
        let result = auto_link_urls(html);
        assert!(!result.contains("<a "), "Should not link img src URLs");
    }

    #[test]
    fn test_auto_link_in_full_pdf() {
        let md = "# News\n\nURL: https://www.cnn.com/2026/02/25/tech/story\n\nMore text.";
        let result = prepare_html(md, None);
        assert!(result.contains(r#"<a href="https://www.cnn.com/2026/02/25/tech/story""#));
    }

    #[test]
    fn test_is_available() {
        // Smoke test -- should not panic
        let _ = is_available();
    }

    // ====================================================================
    // resolve_remote_images tests
    // ====================================================================

    #[test]
    fn test_resolve_no_images() {
        let html = "<html><body><p>No images here</p></body></html>";
        let result = resolve_remote_images(html);
        assert_eq!(result, html);
    }

    #[test]
    fn test_resolve_data_uri_unchanged() {
        let html = r#"<img src="data:image/png;base64,abc123">"#;
        let result = resolve_remote_images(html);
        assert_eq!(result, html);
    }

    #[test]
    fn test_resolve_relative_path_unchanged() {
        let html = r#"<img src="images/photo.png">"#;
        let result = resolve_remote_images(html);
        assert_eq!(result, html);
    }

    #[test]
    fn test_resolve_failed_url_gets_placeholder() {
        // Use a URL that will definitely fail (non-routable address)
        let html = r#"<img src="https://192.0.2.1/nonexistent.png" alt="test">"#;
        let result = resolve_remote_images(html);
        assert!(result.contains(PLACEHOLDER_SVG), "Should contain placeholder SVG");
        assert!(!result.contains("192.0.2.1"), "Should not contain original URL");
    }

    #[test]
    fn test_resolve_multiple_mixed_sources() {
        let html = r#"<img src="data:image/png;base64,abc"><img src="local.png"><img src="https://192.0.2.1/a.png">"#;
        let result = resolve_remote_images(html);
        // data: URI should be unchanged
        assert!(result.contains("data:image/png;base64,abc"));
        // local path should be unchanged
        assert!(result.contains(r#"src="local.png""#));
        // remote URL should be replaced with placeholder (will fail)
        assert!(result.contains(PLACEHOLDER_SVG));
    }

    /// Minimal valid 1x1 PNG (69 bytes) for tests that need real image data.
    const MINIMAL_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
        0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, // 8-bit RGB
        0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, // IDAT chunk
        0x78, 0x9C, 0x63, 0x60, 0x60, 0x60, 0x00, 0x00, // zlib-compressed data
        0x00, 0x04, 0x00, 0x01, 0xF6, 0x17, 0x38, 0x55, // CRC
        0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND chunk
        0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn test_resolve_local_file_image() {
        // Write to the allowed image cache directory
        let img_dir = image_security::allowed_image_dir();
        std::fs::create_dir_all(&img_dir).unwrap();
        let img_path = img_dir.join("test_resolve_local.png");
        std::fs::write(&img_path, MINIMAL_PNG).unwrap();

        let url = format!("file://{}", img_path.display());
        let html = format!(r#"<img src="{}" alt="local">"#, url);
        let result = resolve_remote_images(&html);

        // Should be replaced with a data URI (re-encoded as PNG by security pipeline)
        assert!(result.contains("data:image/png;base64,"), "Should contain data URI, got: {}", result);
        assert!(!result.contains("file://"), "Should not contain file:// URL");

        std::fs::remove_file(&img_path).ok();
    }

    #[test]
    fn test_resolve_local_file_outside_cache_blocked() {
        // file:// pointing outside the image cache should be blocked
        let html = r#"<img src="file:///etc/passwd" alt="sneaky">"#;
        let result = resolve_remote_images(html);
        assert!(result.contains(PLACEHOLDER_SVG), "Should contain placeholder SVG");
        assert!(!result.contains("file://"), "Should not contain file:// URL");
    }

    #[test]
    fn test_resolve_local_file_missing_gets_placeholder() {
        // Nonexistent file under allowed dir still gets placeholder (canonicalize fails)
        let img_dir = image_security::allowed_image_dir();
        let url = format!("file://{}/nonexistent_12345.png", img_dir.display());
        let html = format!(r#"<img src="{}" alt="missing">"#, url);
        let result = resolve_remote_images(&html);
        assert!(result.contains(PLACEHOLDER_SVG), "Should contain placeholder SVG");
        assert!(!result.contains("file://"), "Should not contain file:// URL");
    }

    #[test]
    fn test_resolve_css_background_image_failed() {
        let html = r#"<div style="background-image: url('https://192.0.2.1/bg.png')"></div>"#;
        let result = resolve_remote_images(html);
        // CSS background images keep original URL on failure
        assert!(result.contains("192.0.2.1"));
    }

    #[test]
    #[ignore] // Requires built Swift helper binary
    fn test_generate_pdf() {
        let dir = std::env::temp_dir();
        let output = dir.join("test_pdf_generator.pdf");
        let result = execute_create_pdf(
            output.to_str().unwrap(),
            "Hello from Rust test",
            Some("Test PDF"),
        );
        assert!(result.is_ok(), "Failed: {:?}", result.err());
        assert!(output.exists(), "PDF file was not created");
        let metadata = std::fs::metadata(&output).unwrap();
        assert!(metadata.len() > 0, "PDF file is empty");
        // Cleanup
        let _ = std::fs::remove_file(&output);
    }
}
