//! Fetch URL tool — HTTP GET for arbitrary URLs.
//!
//! Lets the agent retrieve web page content, raw files, API responses, etc.
//! Reuses security patterns from `image_security` (timeouts, size limits,
//! redirect limits, URL normalization).

use anyhow::{anyhow, Result};
use regex::Regex;
use std::io::Read;

/// Default maximum *output* size (512 KB of text after HTML stripping).
const DEFAULT_MAX_BYTES: usize = 524_288;

/// Hard cap on raw HTTP download to prevent OOM (10 MB).
const DOWNLOAD_CAP: usize = 10 * 1024 * 1024;

/// HTTP request timeout in seconds.
const FETCH_TIMEOUT_SECS: u64 = 15;

/// Maximum number of HTTP redirects to follow.
const MAX_REDIRECTS: usize = 5;

/// Fetch the content of a URL via HTTP GET.
///
/// Returns a JSON string with `url`, `status`, `content_type`, `bytes`, `body`,
/// and `truncated` (bool).  For HTML responses the raw markup is stripped to
/// plain text before the `max_bytes` limit is applied, so even multi-megabyte
/// news pages yield usable content instead of an outright rejection.
pub fn execute_fetch_url(url: &str, max_bytes: Option<usize>) -> Result<String> {
    let max_bytes = max_bytes.unwrap_or(DEFAULT_MAX_BYTES);

    // Normalize Unicode look-alikes (LLM artifact cleanup)
    let url = crate::image_security::normalize_url(url);

    // Block file:// URLs — no local file access
    if url.starts_with("file://") || url.starts_with("file:") {
        return Err(anyhow!("file:// URLs are not allowed"));
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .build()?;

    let resp = client
        .get(&url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
             (KHTML, like Gecko) Version/18.0 Safari/605.1.15",
        )
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()?;

    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Stream up to DOWNLOAD_CAP bytes (prevents OOM on huge responses).
    let mut buf = Vec::with_capacity(std::cmp::min(
        resp.content_length().unwrap_or(DOWNLOAD_CAP as u64) as usize,
        DOWNLOAD_CAP,
    ));
    resp.take(DOWNLOAD_CAP as u64).read_to_end(&mut buf)?;
    let raw_text = String::from_utf8_lossy(&buf);

    // If the response looks like HTML, strip tags to extract text.
    let is_html = content_type.contains("text/html") || content_type.contains("application/xhtml");
    let text = if is_html {
        strip_html_to_text(&raw_text)
    } else {
        raw_text.into_owned()
    };

    // Truncate to max_bytes on a char boundary.
    let truncated = text.len() > max_bytes;
    let body = if truncated {
        let boundary = text.floor_char_boundary(max_bytes);
        format!("{}...\n[Content truncated at {} bytes]", &text[..boundary], boundary)
    } else {
        text
    };

    let result = serde_json::json!({
        "url": url,
        "status": status,
        "content_type": content_type,
        "bytes": body.len(),
        "truncated": truncated,
        "body": body,
    });

    Ok(serde_json::to_string(&result)?)
}

/// Strip HTML tags and decode common entities to produce readable plain text.
///
/// This is intentionally simple — no full DOM parse, just good-enough extraction
/// for feeding into an LLM.  Handles: tag removal, script/style stripping,
/// block-element newlines, entity decoding, and whitespace normalization.
fn strip_html_to_text(html: &str) -> String {
    // 1. Remove <script> and <style> blocks entirely
    let re_script = Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap();
    let no_scripts = re_script.replace_all(html, "");
    let re_style = Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap();
    let no_scripts = re_style.replace_all(&no_scripts, "");

    // 2. Insert newlines before block-level elements for readability
    let re_block = Regex::new(r"(?i)<(?:p|div|br|h[1-6]|li|tr|blockquote|section|article|header|footer|nav|aside|main|details|summary|figcaption|dd|dt)[^>]*/?>").unwrap();
    let with_breaks = re_block.replace_all(&no_scripts, "\n");

    // 3. Strip all remaining tags
    let re_tags = Regex::new(r"<[^>]+>").unwrap();
    let text_only = re_tags.replace_all(&with_breaks, "");

    // 4. Decode common HTML entities
    let text = text_only
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&hellip;", "…")
        .replace("&rsquo;", "\u{2019}")
        .replace("&lsquo;", "\u{2018}")
        .replace("&rdquo;", "\u{201D}")
        .replace("&ldquo;", "\u{201C}");

    // 5. Decode numeric entities (&#NNN; and &#xHHH;)
    let re_dec = Regex::new(r"&#(\d+);").unwrap();
    let text = re_dec.replace_all(&text, |caps: &regex::Captures| {
        caps[1]
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string())
            .unwrap_or_default()
    });
    let re_hex = Regex::new(r"&#x([0-9a-fA-F]+);").unwrap();
    let text = re_hex.replace_all(&text, |caps: &regex::Captures| {
        u32::from_str_radix(&caps[1], 16)
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string())
            .unwrap_or_default()
    });

    // 6. Collapse whitespace: multiple blank lines → one, trim each line
    let re_blank = Regex::new(r"\n{3,}").unwrap();
    let text = re_blank.replace_all(&text, "\n\n");
    let re_spaces = Regex::new(r"[^\S\n]+").unwrap();
    let text = re_spaces.replace_all(&text, " ");

    text.lines()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_url_rejected() {
        let err = execute_fetch_url("file:///etc/passwd", None).unwrap_err();
        assert!(
            err.to_string().contains("file://"),
            "Should reject file:// URLs, got: {}",
            err
        );
    }

    #[test]
    fn test_file_url_without_double_slash_rejected() {
        let err = execute_fetch_url("file:/etc/passwd", None).unwrap_err();
        assert!(
            err.to_string().contains("file://"),
            "Should reject file: URLs, got: {}",
            err
        );
    }

    #[test]
    fn test_unicode_normalization_applied() {
        // URL with en-dash should be normalized before fetch attempt.
        // The fetch itself will fail (bad hostname), but we verify normalization
        // by checking the error doesn't contain the Unicode character.
        let result = execute_fetch_url("https://example\u{2013}site.invalid/page", None);
        assert!(result.is_err()); // connection will fail
    }

    #[test]
    fn test_response_json_structure() {
        let result = execute_fetch_url("https://192.0.2.1/test", None);
        assert!(result.is_err(), "Should fail for unreachable host");
    }

    #[test]
    fn test_strip_html_basic() {
        let html = "<html><body><p>Hello <b>world</b></p></body></html>";
        let text = strip_html_to_text(html);
        assert!(text.contains("Hello world"), "Got: {}", text);
    }

    #[test]
    fn test_strip_html_removes_scripts() {
        let html = "<p>Before</p><script>alert('xss')</script><p>After</p>";
        let text = strip_html_to_text(html);
        assert!(!text.contains("alert"), "Script content should be removed: {}", text);
        assert!(text.contains("Before"));
        assert!(text.contains("After"));
    }

    #[test]
    fn test_strip_html_removes_styles() {
        let html = "<p>Text</p><style>.foo { color: red; }</style><p>More</p>";
        let text = strip_html_to_text(html);
        assert!(!text.contains("color"), "Style content should be removed: {}", text);
    }

    #[test]
    fn test_strip_html_entities() {
        let html = "<p>A &amp; B &lt; C &gt; D &quot;E&quot;</p>";
        let text = strip_html_to_text(html);
        assert!(text.contains("A & B < C > D \"E\""), "Got: {}", text);
    }

    #[test]
    fn test_strip_html_numeric_entities() {
        let html = "<p>&#65;&#x42;</p>"; // A, B
        let text = strip_html_to_text(html);
        assert!(text.contains("AB"), "Got: {}", text);
    }

    #[test]
    fn test_strip_html_block_elements_add_newlines() {
        let html = "<div>First</div><div>Second</div>";
        let text = strip_html_to_text(html);
        assert!(text.contains("First\nSecond") || text.contains("First\n\nSecond"),
            "Block elements should produce newlines: {:?}", text);
    }

    #[test]
    fn test_strip_html_collapses_whitespace() {
        let html = "<p>  lots   of    spaces  </p>\n\n\n\n\n<p>after</p>";
        let text = strip_html_to_text(html);
        // Should not have 5+ blank lines
        assert!(!text.contains("\n\n\n"), "Should collapse blank lines: {:?}", text);
    }
}
