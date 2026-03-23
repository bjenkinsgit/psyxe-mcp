//! Brave Search API Client
//!
//! Provides web, news, video, and image search via the Brave Search API.
//! Requires a `BRAVE_SEARCH_API_KEY` environment variable.

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;

/// Truncate a string to at most `max_len` characters, appending "…" if truncated.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len { s.to_string() } else { format!("{}…", &s[..max_len]) }
}

/// Check if Brave Search is configured (API key present)
pub fn is_configured() -> bool {
    std::env::var("BRAVE_SEARCH_API_KEY").is_ok()
}

// ============================================================================
// Response Types
// ============================================================================

/// Web search response — results nested under `web.results[]`
#[derive(Debug, Deserialize)]
pub struct BraveSearchResponse {
    pub web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
pub struct BraveWebResults {
    pub results: Vec<BraveWebResult>,
}

#[derive(Debug, Deserialize)]
pub struct BraveWebResult {
    pub title: String,
    pub url: String,
    pub description: String,
}

/// Direct response for news/video/image — results at top level
#[derive(Debug, Deserialize)]
pub struct BraveDirectResponse {
    pub results: Option<Vec<serde_json::Value>>,
}

// ============================================================================
// Shared HTTP helper
// ============================================================================

fn brave_client() -> Result<(Client, String)> {
    let api_key = std::env::var("BRAVE_SEARCH_API_KEY")
        .context("Missing BRAVE_SEARCH_API_KEY environment variable")?;
    tracing::debug!(key_len = api_key.len(), "Brave API key loaded");
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;
    Ok((client, api_key))
}

fn truncate_desc(desc: &str) -> String {
    if desc.len() > 1000 {
        format!("{}...", &desc[..desc.floor_char_boundary(1000)])
    } else {
        desc.to_string()
    }
}

/// Strip query parameters from a URL to save context tokens.
///
/// Removes everything after `?` while preserving fragment identifiers (`#section`).
/// YouTube `watch?v=` URLs are preserved since the video ID lives in the query string.
fn strip_query_params(url: &str) -> String {
    // Preserve YouTube watch URLs where ?v=ID is the resource identifier
    if url.contains("youtube.com/watch?") || url.contains("youtube.com/watch&") {
        return url.to_string();
    }
    let query_start = match url.find('?') {
        Some(pos) => pos,
        None => return url.to_string(),
    };
    // Preserve fragment (#section) if present after query string
    let fragment = url[query_start..].find('#').map(|off| &url[query_start + off..]);
    match fragment {
        Some(frag) => format!("{}{}", &url[..query_start], frag),
        None => url[..query_start].to_string(),
    }
}

/// Build a Brave Goggles string from a list of domain sources.
///
/// Each domain gets a `$boost=10,site=<domain>` rule. Returns `None` if empty.
fn build_goggles_string(sources: &[String]) -> Option<String> {
    let rules: Vec<String> = sources
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| format!("$boost=10,site={}", s))
        .collect();
    if rules.is_empty() {
        None
    } else {
        Some(rules.join("\n"))
    }
}

/// Compute effective result count when sources are specified.
///
/// When sources are provided and no explicit count was given, auto-increases
/// to `sources.len() * 5` (min 5, capped at `max`). Explicit count always wins.
fn effective_count(explicit: Option<u32>, sources: Option<&[String]>, max: u32) -> u32 {
    if let Some(c) = explicit {
        return c.min(max);
    }
    match sources {
        Some(s) if !s.is_empty() => {
            let auto = (s.len() as u32 * 5).max(5);
            auto.min(max)
        }
        _ => 5u32.min(max),
    }
}

// ============================================================================
// Domain matching and filtering helpers
// ============================================================================

/// Extract domain from a URL, stripping scheme, path, port, and "www." prefix.
///
/// `"https://www.cnn.com/politics"` → `"cnn.com"`
fn extract_domain(url: &str) -> Option<String> {
    if url.is_empty() {
        return None;
    }
    // Strip scheme
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    // Take hostname (before first '/' or ':')
    let host = without_scheme
        .split('/')
        .next()?
        .split(':')
        .next()?;
    if host.is_empty() {
        return None;
    }
    // Strip "www." prefix
    let domain = host.strip_prefix("www.").unwrap_or(host);
    Some(domain.to_lowercase())
}

/// Check if a result domain matches a requested source domain.
///
/// Case-insensitive, strips "www.", handles subdomains:
/// `"edition.cnn.com"` matches `"cnn.com"` via `.ends_with(".cnn.com")`.
/// Rejects false positives: `"notcnn.com"` does NOT match `"cnn.com"`.
fn domains_match(result_domain: &str, requested: &str) -> bool {
    if result_domain.is_empty() || requested.is_empty() {
        return false;
    }
    let rd = result_domain
        .strip_prefix("www.")
        .unwrap_or(result_domain)
        .to_lowercase();
    let req = requested
        .strip_prefix("www.")
        .unwrap_or(requested)
        .to_lowercase();
    if rd == req {
        return true;
    }
    // Subdomain match: rd ends with ".req"
    rd.ends_with(&format!(".{}", req))
}

/// Which field to use for domain matching.
enum DomainField {
    /// News results: use the `"source"` field (already a hostname).
    SourceField,
    /// Web results: extract domain from the `"url"` field.
    UrlField,
}

/// Filter results to only those matching requested source domains.
///
/// Returns `(filtered_results, missing_sources)` where `missing_sources` are
/// domains from `sources` that had zero matches.
fn filter_results_by_sources(
    results: &[serde_json::Value],
    sources: &[String],
    domain_field: &DomainField,
) -> (Vec<serde_json::Value>, Vec<String>) {
    let filtered: Vec<serde_json::Value> = results
        .iter()
        .filter(|r| {
            let domain = match domain_field {
                DomainField::SourceField => r
                    .get("source")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                DomainField::UrlField => r
                    .get("url")
                    .and_then(|v| v.as_str())
                    .and_then(|u| extract_domain(u)),
            };
            match domain {
                Some(d) => sources.iter().any(|s| domains_match(&d, s)),
                None => false,
            }
        })
        .cloned()
        .collect();

    let missing: Vec<String> = sources
        .iter()
        .filter(|s| {
            !filtered.iter().any(|r| {
                let domain = match domain_field {
                    DomainField::SourceField => r
                        .get("source")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    DomainField::UrlField => r
                        .get("url")
                        .and_then(|v| v.as_str())
                        .and_then(|u| extract_domain(u)),
                };
                match domain {
                    Some(d) => domains_match(&d, s),
                    None => false,
                }
            })
        })
        .cloned()
        .collect();

    (filtered, missing)
}

/// Serialize search results to JSON.
///
/// `missing_sources` are logged but NOT included in the output — exposing them
/// causes the LLM to retry with different query formulations.
fn serialize_results(
    query: &str,
    results: &[serde_json::Value],
    missing_sources: &[String],
) -> Result<String> {
    if !missing_sources.is_empty() {
        tracing::info!(?missing_sources, "│  ├─ Sources with no results (not exposed to LLM)");
    }
    let output = serde_json::json!({
        "query": query,
        "results": results,
    });
    serde_json::to_string(&output).map_err(|e| anyhow!("Failed to serialize result: {}", e))
}

/// Raw category search — returns parsed results as Vec, no serialization.
///
/// `endpoint` is the path segment, e.g. "news/search".
/// `extract_result` maps each raw JSON result to the output value.
fn execute_brave_category_search_raw(
    endpoint: &str,
    query: &str,
    count: u32,
    freshness: Option<&str>,
    goggles: Option<&str>,
    extract_result: fn(&serde_json::Value) -> Option<serde_json::Value>,
) -> Result<Vec<serde_json::Value>> {
    let query = truncate_query(query);
    let (client, api_key) = brave_client()?;
    let url = format!("https://api.search.brave.com/res/v1/{}", endpoint);

    let mut req = client
        .get(&url)
        .header("X-Subscription-Token", &api_key)
        .header("Accept", "application/json")
        .query(&[("q", query), ("count", &count.to_string())]);

    if let Some(f) = freshness {
        req = req.query(&[("freshness", f)]);
    }

    if let Some(g) = goggles {
        req = req.query(&[("goggles", g)]);
    }

    tracing::info!(endpoint, %query, count, "│  ├─ Brave API request");

    let response = req.send().context("Brave Search API request failed")?;
    let status = response.status();

    let rl_remaining = response.headers().get("x-ratelimit-remaining").and_then(|v| v.to_str().ok()).map(String::from);
    tracing::debug!(endpoint, %status, ratelimit_remaining = ?rl_remaining, "│  ├─ Brave API response");

    let body = response.text().context("Failed to read response")?;

    if !status.is_success() {
        tracing::warn!(body = truncate_str(&body, 300), "│  └─ Brave API error response");
        return Err(anyhow!("Brave Search API error {}: {}", status, body));
    }

    let parsed: BraveDirectResponse =
        serde_json::from_str(&body).context("Failed to parse Brave Search response")?;

    let results: Vec<serde_json::Value> = parsed
        .results
        .unwrap_or_default()
        .iter()
        .take(count as usize)
        .filter_map(extract_result)
        .collect();

    tracing::info!(endpoint, result_count = results.len(), "│  └─ Brave API results");

    Ok(results)
}

/// Generic category search — thin wrapper around `_raw` that serializes.
fn execute_brave_category_search(
    endpoint: &str,
    query: &str,
    count: u32,
    freshness: Option<&str>,
    goggles: Option<&str>,
    extract_result: fn(&serde_json::Value) -> Option<serde_json::Value>,
) -> Result<String> {
    let results =
        execute_brave_category_search_raw(endpoint, query, count, freshness, goggles, extract_result)?;
    serialize_results(truncate_query(query), &results, &[])
}

/// Extract a news result from a raw Brave API JSON item.
fn news_extract_result(item: &serde_json::Value) -> Option<serde_json::Value> {
    let title = item.get("title")?.as_str()?;
    let url = item.get("url")?.as_str()?;
    let description = item
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let age = item.get("age").and_then(|v| v.as_str()).unwrap_or("");
    let source = item
        .get("meta_url")
        .and_then(|m| m.get("hostname"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    Some(serde_json::json!({
        "title": title,
        "url": strip_query_params(url),
        "description": truncate_desc(description),
        "age": age,
        "source": source,
    }))
}

// ============================================================================
// Web Search
// ============================================================================

/// Brave API query limit is 400 characters. Truncate on a word boundary.
fn truncate_query(query: &str) -> &str {
    const MAX_LEN: usize = 400;
    if query.len() <= MAX_LEN {
        return query;
    }
    // Find the last space before the limit to avoid splitting mid-word
    match query[..MAX_LEN].rfind(' ') {
        Some(pos) => &query[..pos],
        None => &query[..MAX_LEN],
    }
}

/// Raw web search — returns parsed results as Vec, no serialization.
fn execute_brave_search_raw(query: &str, count: u32, goggles: Option<&str>) -> Result<Vec<serde_json::Value>> {
    let query = truncate_query(query);
    let (client, api_key) = brave_client()?;

    tracing::info!(endpoint = "web/search", %query, count, "│  ├─ Brave API request");

    let mut req = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", &api_key)
        .header("Accept", "application/json")
        .query(&[("q", query), ("count", &count.to_string())]);

    if let Some(g) = goggles {
        req = req.query(&[("goggles", g)]);
    }

    let response = req
        .send()
        .context("Brave Search API request failed")?;

    let status = response.status();

    let rl_remaining = response.headers().get("x-ratelimit-remaining").and_then(|v| v.to_str().ok()).map(String::from);
    tracing::debug!(endpoint = "web/search", %status, ratelimit_remaining = ?rl_remaining, "│  ├─ Brave API response");

    let body = response.text().context("Failed to read response")?;

    if !status.is_success() {
        tracing::warn!(body = truncate_str(&body, 300), "│  └─ Brave API error response");
        return Err(anyhow!("Brave Search API error {}: {}", status, body));
    }

    let parsed: BraveSearchResponse =
        serde_json::from_str(&body).context("Failed to parse Brave Search response")?;

    let results: Vec<serde_json::Value> = parsed
        .web
        .map(|w| {
            w.results
                .into_iter()
                .take(count as usize)
                .map(|r| {
                    let desc = truncate_desc(&r.description);
                    serde_json::json!({
                        "title": r.title,
                        "url": strip_query_params(&r.url),
                        "description": desc,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    tracing::info!(endpoint = "web/search", result_count = results.len(), "│  └─ Brave API results");

    Ok(results)
}

/// Execute a web search via the Brave Search API.
///
/// Returns a JSON string with query and results array.
/// If `sources` is provided, results are filtered to those domains. Missing sources
/// trigger parallel `site:` fallback queries. Includes `missing_sources` in output
/// for any domains that still had zero results after fallback.
pub fn execute_brave_search(query: &str, count: Option<u32>, sources: Option<&[String]>) -> Result<String> {
    let query = truncate_query(query);
    let goggles = sources.and_then(|s| build_goggles_string(s));
    let count = effective_count(count, sources, 20);

    // Phase 1: broad search with goggles
    let raw_results = execute_brave_search_raw(query, count, goggles.as_deref())?;

    // No sources requested — return unfiltered
    let sources = match sources {
        Some(s) if !s.is_empty() => s,
        _ => return serialize_results(query, &raw_results, &[]),
    };

    // Filter to requested domains
    let (mut filtered, missing) =
        filter_results_by_sources(&raw_results, sources, &DomainField::UrlField);

    tracing::info!(
        phase1_total = raw_results.len(),
        phase1_filtered = filtered.len(),
        missing_count = missing.len(),
        ?missing,
        "│  ├─ Phase 1 web filter"
    );

    if missing.is_empty() {
        return serialize_results(query, &filtered, &[]);
    }

    // Phase 2: parallel site: fallback for missing sources
    let still_missing: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = missing
            .iter()
            .map(|domain| {
                let site_query = format!("site:{} {}", domain, query);
                let domain = domain.clone();
                s.spawn(move || {
                    tracing::info!(domain = %domain, "│  ├─ Phase 2 web fallback");
                    match execute_brave_search_raw(&site_query, 5, None) {
                        Ok(results) => {
                            let (matched, _) =
                                filter_results_by_sources(&results, &[domain.clone()], &DomainField::UrlField);
                            (domain, matched)
                        }
                        Err(e) => {
                            tracing::warn!(domain = %domain, error = %e, "│  └─ Phase 2 web fallback failed");
                            (domain, vec![])
                        }
                    }
                })
            })
            .collect();

        let mut still_missing = Vec::new();
        for handle in handles {
            let (domain, results) = handle.join().unwrap_or_else(|_| (String::new(), vec![]));
            if results.is_empty() && !domain.is_empty() {
                still_missing.push(domain);
            } else {
                filtered.extend(results);
            }
        }
        still_missing
    });

    tracing::info!(
        final_count = filtered.len(),
        still_missing = ?still_missing,
        "│  └─ Web search complete"
    );

    serialize_results(query, &filtered, &still_missing)
}

// ============================================================================
// News Search
// ============================================================================

/// Execute a news search via the Brave Search API.
///
/// Returns JSON with query and results containing title, url, description, age, source.
/// If `sources` is provided, results are filtered to those domains. Missing sources
/// trigger parallel `site:` fallback queries. Includes `missing_sources` in output
/// for any domains that still had zero results after fallback.
pub fn execute_news_search(
    query: &str,
    count: Option<u32>,
    freshness: Option<&str>,
    sources: Option<&[String]>,
) -> Result<String> {
    let query = truncate_query(query);
    let goggles = sources.and_then(|s| build_goggles_string(s));
    let count = effective_count(count, sources, 20);

    // Phase 1: broad search with goggles
    let raw_results = execute_brave_category_search_raw(
        "news/search",
        query,
        count,
        freshness,
        goggles.as_deref(),
        news_extract_result,
    )?;

    // No sources requested — return unfiltered
    let sources = match sources {
        Some(s) if !s.is_empty() => s,
        _ => return serialize_results(query, &raw_results, &[]),
    };

    // Filter to requested domains (news results have a "source" field = hostname)
    let (mut filtered, missing) =
        filter_results_by_sources(&raw_results, sources, &DomainField::SourceField);

    tracing::info!(
        phase1_total = raw_results.len(),
        phase1_filtered = filtered.len(),
        missing_count = missing.len(),
        ?missing,
        "│  ├─ Phase 1 news filter"
    );

    if missing.is_empty() {
        return serialize_results(query, &filtered, &[]);
    }

    // Phase 2: parallel site: fallback for missing sources
    let still_missing: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = missing
            .iter()
            .map(|domain| {
                let site_query = format!("site:{} {}", domain, query);
                let domain = domain.clone();
                s.spawn(move || {
                    tracing::info!(domain = %domain, "│  ├─ Phase 2 news fallback");
                    match execute_brave_category_search_raw(
                        "news/search",
                        &site_query,
                        5,
                        freshness,
                        None,
                        news_extract_result,
                    ) {
                        Ok(results) => {
                            let (matched, _) = filter_results_by_sources(
                                &results,
                                &[domain.clone()],
                                &DomainField::SourceField,
                            );
                            (domain, matched)
                        }
                        Err(e) => {
                            tracing::warn!(domain = %domain, error = %e, "│  └─ Phase 2 news fallback failed");
                            (domain, vec![])
                        }
                    }
                })
            })
            .collect();

        let mut still_missing = Vec::new();
        for handle in handles {
            let (domain, results) = handle.join().unwrap_or_else(|_| (String::new(), vec![]));
            if results.is_empty() && !domain.is_empty() {
                still_missing.push(domain);
            } else {
                filtered.extend(results);
            }
        }
        still_missing
    });

    tracing::info!(
        final_count = filtered.len(),
        still_missing = ?still_missing,
        "│  └─ News search complete"
    );

    serialize_results(query, &filtered, &still_missing)
}

// ============================================================================
// Video Search
// ============================================================================

/// Execute a video search via the Brave Search API.
///
/// Returns JSON with query and results containing title, url, description, age, duration, creator, publisher.
pub fn execute_video_search(query: &str, count: Option<u32>, freshness: Option<&str>) -> Result<String> {
    let count = effective_count(count, None, 20);
    execute_brave_category_search("videos/search", query, count, freshness, None, |item| {
        let title = item.get("title")?.as_str()?;
        let url = item.get("url")?.as_str()?;
        let description = item
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let age = item.get("age").and_then(|v| v.as_str()).unwrap_or("");
        let video = item.get("video");
        let duration = video
            .and_then(|v| v.get("duration"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let creator = video
            .and_then(|v| v.get("creator"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let publisher = video
            .and_then(|v| v.get("publisher"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        Some(serde_json::json!({
            "title": title,
            "url": strip_query_params(url),
            "description": truncate_desc(description),
            "age": age,
            "duration": duration,
            "creator": creator,
            "publisher": publisher,
        }))
    })
}

// ============================================================================
// Image Search
// ============================================================================

/// Execute an image search via the Brave Search API.
///
/// Returns JSON with query and results containing title, url, source, thumbnail, width, height.
pub fn execute_image_search(query: &str, count: Option<u32>) -> Result<String> {
    let count = effective_count(count, None, 50);
    execute_brave_category_search("images/search", query, count, None, None, |item| {
        let title = item.get("title")?.as_str()?;
        let url = item.get("url")?.as_str()?;
        let source = item
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let thumbnail = item
            .get("thumbnail")
            .and_then(|t| t.get("src"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let width = item
            .get("properties")
            .and_then(|p| p.get("width"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let height = item
            .get("properties")
            .and_then(|p| p.get("height"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        Some(serde_json::json!({
            "title": title,
            "url": strip_query_params(url),
            "source": source,
            "thumbnail": thumbnail,
            "width": width,
            "height": height,
        }))
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_configured() {
        // Must be a single test to avoid parallel env var races
        std::env::remove_var("BRAVE_SEARCH_API_KEY");
        assert!(!is_configured());

        std::env::set_var("BRAVE_SEARCH_API_KEY", "test-key");
        assert!(is_configured());

        std::env::remove_var("BRAVE_SEARCH_API_KEY");
    }

    #[test]
    fn test_parse_response_json() {
        let json = r#"{
            "web": {
                "results": [
                    {
                        "title": "Rust Programming Language",
                        "url": "https://www.rust-lang.org/",
                        "description": "A language empowering everyone to build reliable software."
                    },
                    {
                        "title": "Rust (programming language) - Wikipedia",
                        "url": "https://en.wikipedia.org/wiki/Rust_(programming_language)",
                        "description": "Rust is a multi-paradigm programming language."
                    }
                ]
            }
        }"#;

        let parsed: BraveSearchResponse = serde_json::from_str(json).unwrap();
        let web = parsed.web.unwrap();
        assert_eq!(web.results.len(), 2);
        assert_eq!(web.results[0].title, "Rust Programming Language");
        assert_eq!(web.results[1].url, "https://en.wikipedia.org/wiki/Rust_(programming_language)");
    }

    #[test]
    fn test_parse_response_no_web() {
        let json = r#"{}"#;
        let parsed: BraveSearchResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.web.is_none());
    }

    #[test]
    fn test_parse_news_response() {
        let json = r#"{
            "results": [
                {
                    "title": "AI Breakthrough in 2025",
                    "url": "https://reuters.com/tech/ai-breakthrough",
                    "description": "Major advances in artificial intelligence reported today.",
                    "age": "13 hours ago",
                    "page_age": "2025-01-15T10:30:00",
                    "meta_url": { "hostname": "reuters.com" },
                    "thumbnail": { "src": "https://img.reuters.com/thumb.jpg" }
                },
                {
                    "title": "Tech Stocks Rally",
                    "url": "https://cnbc.com/markets/tech-rally",
                    "description": "Technology stocks surged on AI optimism.",
                    "age": "2 hours ago",
                    "meta_url": { "hostname": "cnbc.com" }
                }
            ]
        }"#;

        let parsed: BraveDirectResponse = serde_json::from_str(json).unwrap();
        let results = parsed.results.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["title"].as_str().unwrap(), "AI Breakthrough in 2025");
        assert_eq!(results[0]["age"].as_str().unwrap(), "13 hours ago");
        assert_eq!(
            results[0]["meta_url"]["hostname"].as_str().unwrap(),
            "reuters.com"
        );
    }

    #[test]
    fn test_parse_video_response() {
        let json = r#"{
            "results": [
                {
                    "title": "Learn Rust in 10 Minutes",
                    "url": "https://youtube.com/watch?v=abc123",
                    "description": "A quick introduction to the Rust programming language.",
                    "age": "3 months ago",
                    "video": {
                        "duration": "10:32",
                        "creator": "RustTutorials",
                        "publisher": "YouTube",
                        "tags": ["rust", "programming"]
                    },
                    "thumbnail": { "src": "https://i.ytimg.com/vi/abc123/hq.jpg" }
                },
                {
                    "title": "Rust vs Go Performance",
                    "url": "https://youtube.com/watch?v=def456",
                    "description": "Comparing performance of Rust and Go.",
                    "age": "1 week ago",
                    "video": {
                        "duration": "25:10",
                        "creator": "TechCompare",
                        "publisher": "YouTube"
                    }
                }
            ]
        }"#;

        let parsed: BraveDirectResponse = serde_json::from_str(json).unwrap();
        let results = parsed.results.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["title"].as_str().unwrap(), "Learn Rust in 10 Minutes");
        assert_eq!(
            results[0]["video"]["duration"].as_str().unwrap(),
            "10:32"
        );
        assert_eq!(
            results[0]["video"]["creator"].as_str().unwrap(),
            "RustTutorials"
        );
    }

    #[test]
    fn test_parse_image_response() {
        let json = r#"{
            "results": [
                {
                    "title": "Aurora Borealis over Norway",
                    "url": "https://example.com/aurora.jpg",
                    "source": "example.com",
                    "page_fetched": "2025-01-10T08:00:00",
                    "thumbnail": {
                        "src": "https://example.com/aurora_thumb.jpg",
                        "width": 200,
                        "height": 150
                    },
                    "properties": {
                        "url": "https://example.com/aurora_full.jpg",
                        "width": 1920,
                        "height": 1080
                    },
                    "confidence": 0.95
                },
                {
                    "title": "Northern Lights Iceland",
                    "url": "https://photos.com/northern-lights.jpg",
                    "source": "photos.com",
                    "thumbnail": {
                        "src": "https://photos.com/nl_thumb.jpg"
                    },
                    "properties": {
                        "width": 3840,
                        "height": 2160
                    }
                }
            ]
        }"#;

        let parsed: BraveDirectResponse = serde_json::from_str(json).unwrap();
        let results = parsed.results.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0]["title"].as_str().unwrap(),
            "Aurora Borealis over Norway"
        );
        assert_eq!(results[0]["source"].as_str().unwrap(), "example.com");
        assert_eq!(results[0]["properties"]["width"].as_u64().unwrap(), 1920);
        assert_eq!(
            results[1]["thumbnail"]["src"].as_str().unwrap(),
            "https://photos.com/nl_thumb.jpg"
        );
    }

    #[test]
    fn test_parse_direct_response_no_results() {
        let json = r#"{}"#;
        let parsed: BraveDirectResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.results.is_none());
    }

    // ========================================================================
    // Goggles helper tests
    // ========================================================================

    #[test]
    fn test_build_goggles_empty() {
        assert_eq!(build_goggles_string(&[]), None);
    }

    #[test]
    fn test_build_goggles_single_source() {
        let sources = vec!["cnn.com".to_string()];
        assert_eq!(
            build_goggles_string(&sources),
            Some("$boost=10,site=cnn.com".to_string())
        );
    }

    #[test]
    fn test_build_goggles_multiple_sources() {
        let sources = vec!["cnn.com".to_string(), "axios.com".to_string()];
        assert_eq!(
            build_goggles_string(&sources),
            Some("$boost=10,site=cnn.com\n$boost=10,site=axios.com".to_string())
        );
    }

    #[test]
    fn test_build_goggles_trims_whitespace() {
        let sources = vec!["  cnn.com ".to_string(), "".to_string(), " axios.com".to_string()];
        assert_eq!(
            build_goggles_string(&sources),
            Some("$boost=10,site=cnn.com\n$boost=10,site=axios.com".to_string())
        );
    }

    #[test]
    fn test_effective_count_no_sources() {
        assert_eq!(effective_count(None, None, 20), 5);
    }

    #[test]
    fn test_effective_count_with_sources() {
        let sources = vec!["a.com".to_string(), "b.com".to_string(), "c.com".to_string()];
        // 3 sources * 5 = 15
        assert_eq!(effective_count(None, Some(&sources), 20), 15);
    }

    #[test]
    fn test_effective_count_many_sources_capped() {
        let sources: Vec<String> = (0..10).map(|i| format!("s{}.com", i)).collect();
        // 10 * 5 = 50, capped at 20
        assert_eq!(effective_count(None, Some(&sources), 20), 20);
    }

    #[test]
    fn test_effective_count_explicit_overrides() {
        let sources = vec!["a.com".to_string(), "b.com".to_string()];
        assert_eq!(effective_count(Some(3), Some(&sources), 20), 3);
    }

    // ========================================================================
    // Domain matching tests
    // ========================================================================

    #[test]
    fn test_extract_domain_basic() {
        assert_eq!(extract_domain("https://www.cnn.com/politics"), Some("cnn.com".into()));
        assert_eq!(extract_domain("http://reuters.com/tech"), Some("reuters.com".into()));
        assert_eq!(extract_domain("https://example.com"), Some("example.com".into()));
    }

    #[test]
    fn test_extract_domain_edge_cases() {
        assert_eq!(extract_domain(""), None);
        assert_eq!(extract_domain("cnn.com/path"), Some("cnn.com".into()));
        assert_eq!(extract_domain("https://CNN.COM/path"), Some("cnn.com".into()));
        assert_eq!(extract_domain("https://example.com:8080/path"), Some("example.com".into()));
    }

    #[test]
    fn test_domains_match_exact() {
        assert!(domains_match("cnn.com", "cnn.com"));
        assert!(domains_match("CNN.COM", "cnn.com"));
        assert!(domains_match("cnn.com", "CNN.COM"));
    }

    #[test]
    fn test_domains_match_www_stripped() {
        assert!(domains_match("www.cnn.com", "cnn.com"));
        assert!(domains_match("cnn.com", "www.cnn.com"));
    }

    #[test]
    fn test_domains_match_subdomain() {
        assert!(domains_match("edition.cnn.com", "cnn.com"));
        assert!(domains_match("us.edition.cnn.com", "cnn.com"));
    }

    #[test]
    fn test_domains_match_rejects_suffix() {
        assert!(!domains_match("notcnn.com", "cnn.com"));
        assert!(!domains_match("mycnn.com", "cnn.com"));
    }

    #[test]
    fn test_domains_match_empty() {
        assert!(!domains_match("", "cnn.com"));
        assert!(!domains_match("cnn.com", ""));
        assert!(!domains_match("", ""));
    }

    // ========================================================================
    // Filter results tests
    // ========================================================================

    #[test]
    fn test_filter_news_results_by_source() {
        let results = vec![
            serde_json::json!({"title": "A", "source": "cnn.com"}),
            serde_json::json!({"title": "B", "source": "reuters.com"}),
            serde_json::json!({"title": "C", "source": "bbc.com"}),
        ];
        let sources = vec!["cnn.com".to_string(), "reuters.com".to_string()];
        let (filtered, missing) =
            filter_results_by_sources(&results, &sources, &DomainField::SourceField);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0]["title"], "A");
        assert_eq!(filtered[1]["title"], "B");
        assert!(missing.is_empty());
    }

    #[test]
    fn test_filter_identifies_missing_sources() {
        let results = vec![
            serde_json::json!({"title": "A", "source": "cnn.com"}),
        ];
        let sources = vec!["cnn.com".to_string(), "axios.com".to_string()];
        let (filtered, missing) =
            filter_results_by_sources(&results, &sources, &DomainField::SourceField);
        assert_eq!(filtered.len(), 1);
        assert_eq!(missing, vec!["axios.com"]);
    }

    #[test]
    fn test_filter_web_results_by_url() {
        let results = vec![
            serde_json::json!({"title": "A", "url": "https://www.cnn.com/article"}),
            serde_json::json!({"title": "B", "url": "https://reuters.com/news"}),
            serde_json::json!({"title": "C", "url": "https://bbc.com/world"}),
        ];
        let sources = vec!["cnn.com".to_string()];
        let (filtered, missing) =
            filter_results_by_sources(&results, &sources, &DomainField::UrlField);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0]["title"], "A");
        assert!(missing.is_empty());
    }

    // ========================================================================
    // strip_query_params tests
    // ========================================================================

    #[test]
    fn test_strip_query_params_basic() {
        assert_eq!(
            strip_query_params("https://example.com/article?utm_source=api&campaign=123"),
            "https://example.com/article"
        );
    }

    #[test]
    fn test_strip_query_params_no_params() {
        assert_eq!(
            strip_query_params("https://example.com/article"),
            "https://example.com/article"
        );
    }

    #[test]
    fn test_strip_query_params_preserves_fragment() {
        assert_eq!(
            strip_query_params("https://example.com/page?foo=bar#section"),
            "https://example.com/page#section"
        );
    }

    #[test]
    fn test_strip_query_params_fragment_only() {
        assert_eq!(
            strip_query_params("https://example.com/page#section"),
            "https://example.com/page#section"
        );
    }

    #[test]
    fn test_strip_query_params_empty_query() {
        assert_eq!(
            strip_query_params("https://example.com/page?"),
            "https://example.com/page"
        );
    }

    #[test]
    fn test_strip_query_params_youtube_preserved() {
        let yt = "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42";
        assert_eq!(strip_query_params(yt), yt);
    }

    #[test]
    fn test_strip_query_params_non_url_passthrough() {
        assert_eq!(strip_query_params("not a url"), "not a url");
    }

    #[test]
    fn test_filter_subdomain_matches() {
        let results = vec![
            serde_json::json!({"title": "A", "url": "https://edition.cnn.com/article"}),
        ];
        let sources = vec!["cnn.com".to_string()];
        let (filtered, missing) =
            filter_results_by_sources(&results, &sources, &DomainField::UrlField);
        assert_eq!(filtered.len(), 1);
        assert!(missing.is_empty());
    }

    // ========================================================================
    // Serialize results tests
    // ========================================================================

    #[test]
    fn test_serialize_results_with_missing_not_exposed() {
        let results = vec![serde_json::json!({"title": "A"})];
        let missing = vec!["axios.com".to_string()];
        let json_str = serialize_results("test query", &results, &missing).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["query"], "test query");
        assert_eq!(parsed["results"].as_array().unwrap().len(), 1);
        // missing_sources is logged but NOT included in output
        assert!(parsed.get("missing_sources").is_none());
    }

    #[test]
    fn test_serialize_results_without_missing() {
        let results = vec![serde_json::json!({"title": "A"})];
        let json_str = serialize_results("test query", &results, &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["query"], "test query");
        assert!(parsed.get("missing_sources").is_none());
    }
}
