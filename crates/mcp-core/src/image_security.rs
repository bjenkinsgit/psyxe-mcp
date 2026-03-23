//! Image security validation pipeline.
//!
//! Fetches external images, validates them via magic-byte detection,
//! decodes with size limits, re-encodes to PNG to strip metadata,
//! and optionally submits to VirusTotal.
//!
//! Shared by both the Tauri UI (SecureImage) and the PDF generator.

use base64::Engine;
use image::{GenericImageView, ImageReader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::sync::Mutex;
use std::time::Instant;

/// Maximum download size (10 MB).
const MAX_DOWNLOAD_BYTES: usize = 10 * 1024 * 1024;

/// Maximum image dimension (width or height).
const MAX_DIMENSION: u32 = 8_000;

/// Maximum total pixels (width * height).
const MAX_PIXELS: u64 = 40_000_000;

/// Maximum memory the image decoder may allocate (40 MB).
const MAX_ALLOC_BYTES: u64 = 40 * 1024 * 1024;

/// Tracking pixel threshold — images <= 3x3 are flagged.
const TRACKING_PIXEL_MAX: u32 = 3;

/// Fetch timeout in seconds.
const FETCH_TIMEOUT_SECS: u64 = 15;

/// Maximum number of HTTP redirects to follow.
const MAX_REDIRECTS: usize = 5;

/// Maximum number of retry attempts for rate-limited or transient failures.
const MAX_RETRIES: u32 = 3;

/// Adaptive throttle: tracks the per-second rate limit from X-RateLimit-Limit
/// headers.  Defaults to no throttling; only kicks in once we see the header.
static BRAVE_RATE_LIMIT: Mutex<(Option<Instant>, u64)> = Mutex::new((None, 0));

/// Record the per-second rate limit from response headers and space requests accordingly.
fn update_brave_rate_limit(headers: &reqwest::header::HeaderMap) {
    // Parse X-RateLimit-Limit: "N, M" — first value is per-second burst limit
    if let Some(limit) = headers
        .get("x-ratelimit-limit")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        if let Ok(mut state) = BRAVE_RATE_LIMIT.lock() {
            state.1 = limit; // per-second limit (e.g. 1, 5, 20)
        }
    }
}

/// If we know the per-second burst limit is 1 (free tier), throttle requests.
fn throttle_brave_proxy(url: &str) {
    if !url.contains("imgs.search.brave.com") {
        return;
    }
    let mut state = BRAVE_RATE_LIMIT.lock().unwrap();
    let burst_limit = state.1;
    // Only throttle if burst limit is 1 (free tier); higher plans don't need it
    if burst_limit <= 1 && burst_limit > 0 {
        if let Some(prev) = state.0 {
            let elapsed = prev.elapsed();
            let min = std::time::Duration::from_millis(1100);
            if elapsed < min {
                let wait = min - elapsed;
                tracing::debug!(target: "image_fetch", wait_ms = wait.as_millis() as u64, "Throttling Brave proxy");
                std::thread::sleep(wait);
            }
        }
    }
    state.0 = Some(Instant::now());
}

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum RejectionReason {
    FetchFailed { message: String },
    TooLarge { bytes: usize },
    NotAnImage,
    SvgBlocked,
    MimeTypeMismatch { magic: String, header: String },
    DecodeFailed { message: String },
    DimensionsTooLarge { width: u32, height: u32 },
    ReencodeFailed { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingPixelInfo {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum VirusTotalStatus {
    Skipped,
    Pending { scan_id: String },
    Clean,
    Malicious { positives: u32, total: u32 },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageValidationResult {
    pub url: String,
    pub safe: bool,
    pub rejection: Option<RejectionReason>,
    /// `data:image/png;base64,...` when safe.
    pub data_uri: Option<String>,
    pub tracking_pixel: Option<TrackingPixelInfo>,
    pub virus_total: VirusTotalStatus,
    pub dimensions: Option<(u32, u32)>,
    pub sha256: Option<String>,
}

// ============================================================================
// Pipeline
// ============================================================================

/// Normalize a URL by replacing Unicode look-alike characters with ASCII equivalents.
///
/// LLMs often replace ASCII hyphens (U+002D) with non-breaking hyphens (U+2011),
/// en-dashes (U+2013), or other Unicode variants, which corrupts URLs.
pub fn normalize_url(url: &str) -> String {
    url.chars()
        .filter_map(|ch| match ch {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' => Some('-'), // various dashes -> hyphen
            '\u{00AD}' | '\u{200B}' | '\u{FEFF}' => None,                   // soft-hyphen, zero-width, BOM -> remove
            '\u{00A0}' | '\u{202F}' => Some(' '),                            // non-breaking spaces -> space
            other => Some(other),
        })
        .collect()
}

/// Run the full validation pipeline for an external image URL.
pub fn validate_image(url: &str) -> ImageValidationResult {
    // Normalize Unicode look-alikes that LLMs inject into URLs
    let url = normalize_url(url);
    let url = url.as_str();

    let mut result = ImageValidationResult {
        url: url.to_string(),
        safe: false,
        rejection: None,
        data_uri: None,
        tracking_pixel: None,
        virus_total: VirusTotalStatus::Skipped,
        dimensions: None,
        sha256: None,
    };

    // Step 1: Fetch
    let bytes = match fetch_image_bytes(url) {
        Ok(b) => b,
        Err(rejection) => {
            result.rejection = Some(rejection);
            return result;
        }
    };

    // Steps 2-8: Validate and re-encode the fetched bytes
    validate_bytes_pipeline(&bytes, &mut result);

    // Step 9: Optional VirusTotal (only for remote URLs)
    if result.safe {
        if let Some(ref hash) = result.sha256 {
            result.virus_total = check_virustotal(url, hash);
        }
    }

    result
}

/// Validate raw image bytes through the security pipeline: SHA-256, magic-byte
/// check, SVG guard, decode with limits, tracking-pixel detection, re-encode to PNG.
///
/// On success, sets `result.safe = true` and populates `result.data_uri`.
fn validate_bytes_pipeline(bytes: &[u8], result: &mut ImageValidationResult) {
    // Step 2: SHA-256
    let hash = sha256_hex(bytes);
    result.sha256 = Some(hash);

    // Step 3: Magic-byte check
    let kind = match infer::get(bytes) {
        Some(k) => k,
        None => {
            result.rejection = Some(RejectionReason::NotAnImage);
            return;
        }
    };

    if kind.matcher_type() != infer::MatcherType::Image {
        result.rejection = Some(RejectionReason::NotAnImage);
        return;
    }

    // Step 4: SVG check
    if kind.mime_type() == "image/svg+xml" || looks_like_svg(bytes) {
        result.rejection = Some(RejectionReason::SvgBlocked);
        return;
    }

    // Step 5: Decode with limits
    let img = match decode_with_limits(bytes) {
        Ok(i) => i,
        Err(rejection) => {
            result.rejection = Some(rejection);
            return;
        }
    };

    let (w, h) = img.dimensions();
    result.dimensions = Some((w, h));

    // Step 6: Dimension check
    if (w as u64) * (h as u64) > MAX_PIXELS {
        result.rejection = Some(RejectionReason::DimensionsTooLarge { width: w, height: h });
        return;
    }

    // Step 7: Tracking pixel detection
    if w <= TRACKING_PIXEL_MAX && h <= TRACKING_PIXEL_MAX {
        result.tracking_pixel = Some(TrackingPixelInfo { width: w, height: h });
    }

    // Step 8: Re-encode to PNG
    match reencode_to_png(&img) {
        Ok(data_uri) => {
            result.data_uri = Some(data_uri);
            result.safe = true;
        }
        Err(rejection) => {
            result.rejection = Some(rejection);
        }
    }
}

/// Allowed directory for local file:// image resolution.
/// Only files under the prolog-router image cache are permitted to prevent
/// local file inclusion attacks (e.g., reading ~/.ssh/id_rsa via crafted HTML).
pub fn allowed_image_dir() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("prolog-router")
        .join("images")
}

/// Validate a local image file through the same security pipeline as remote images.
///
/// Restricted to files under [`allowed_image_dir()`] to prevent local file
/// inclusion attacks. Returns a result with `safe = true` and a PNG data URI
/// on success, or a rejection reason on failure.
pub fn validate_local_image(path: &str) -> ImageValidationResult {
    let mut result = ImageValidationResult {
        url: format!("file://{}", path),
        safe: false,
        rejection: None,
        data_uri: None,
        tracking_pixel: None,
        virus_total: VirusTotalStatus::Skipped,
        dimensions: None,
        sha256: None,
    };

    // Security: only allow reads from the image cache directory
    let canonical = match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(e) => {
            result.rejection = Some(RejectionReason::FetchFailed {
                message: format!("Cannot resolve path: {}", e),
            });
            return result;
        }
    };
    let allowed = allowed_image_dir();
    if !canonical.starts_with(&allowed) {
        tracing::warn!(
            "Blocked local file read outside image cache: {} (allowed: {})",
            canonical.display(),
            allowed.display()
        );
        result.rejection = Some(RejectionReason::FetchFailed {
            message: "Path outside allowed image directory".to_string(),
        });
        return result;
    }

    // Read file bytes
    let bytes = match std::fs::read(&canonical) {
        Ok(b) => b,
        Err(e) => {
            result.rejection = Some(RejectionReason::FetchFailed {
                message: format!("Failed to read file: {}", e),
            });
            return result;
        }
    };

    if bytes.len() > MAX_DOWNLOAD_BYTES {
        result.rejection = Some(RejectionReason::TooLarge { bytes: bytes.len() });
        return result;
    }

    // Run the shared validation pipeline (SHA-256, magic bytes, decode, re-encode)
    validate_bytes_pipeline(&bytes, &mut result);

    result
}

// ============================================================================
// Helpers
// ============================================================================

/// Fetch image bytes with timeout, redirect limit, size cap, and retry logic.
///
/// Retries on 429 (always) and 403 (only when rate-limit headers are present).
/// Reads `X-RateLimit-Reset` when available; otherwise backs off linearly.
fn fetch_image_bytes(url: &str) -> Result<Vec<u8>, RejectionReason> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .build()
        .map_err(|e| RejectionReason::FetchFailed {
            message: format!("Client build error: {}", e),
        })?;

    // Truncate URL for log lines
    let short_url = if url.len() > 80 { &url[..80] } else { url };

    let mut last_status = None;

    for attempt in 0..=MAX_RETRIES {
        // Serialize Brave proxy requests to respect 1 req/s rate limit
        throttle_brave_proxy(url);
        let resp = client
            .get(url)
            .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15")
            .header("Accept", "image/avif,image/webp,image/apng,image/*,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Referer", "https://search.brave.com/")
            .header("Sec-Fetch-Dest", "image")
            .header("Sec-Fetch-Mode", "no-cors")
            .header("Sec-Fetch-Site", "same-origin")
            .send()
            .map_err(|e| RejectionReason::FetchFailed {
                message: format!("{}", e),
            })?;

        let status = resp.status();
        last_status = Some(status);

        // Learn the rate limit from headers (adapts throttle for future requests)
        update_brave_rate_limit(resp.headers());

        // Log response status and rate-limit headers for diagnostics
        let rl_limit = resp.headers().get("x-ratelimit-limit").and_then(|v| v.to_str().ok()).map(String::from);
        let rl_remaining = resp.headers().get("x-ratelimit-remaining").and_then(|v| v.to_str().ok()).map(String::from);
        let rl_reset = resp.headers().get("x-ratelimit-reset").and_then(|v| v.to_str().ok()).map(String::from);
        let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).map(String::from);

        tracing::debug!(
            target: "image_fetch",
            %status,
            attempt,
            max_retries = MAX_RETRIES,
            url = short_url,
            content_type = ?content_type,
            ratelimit_limit = ?rl_limit,
            ratelimit_remaining = ?rl_remaining,
            ratelimit_reset = ?rl_reset,
            "Image fetch response"
        );

        let has_ratelimit_headers = rl_limit.is_some() || rl_remaining.is_some();

        // 429 is always retryable; 403 only if rate-limit headers suggest it
        let is_retryable = status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || (status == reqwest::StatusCode::FORBIDDEN && has_ratelimit_headers);

        if is_retryable && attempt < MAX_RETRIES {
            let wait_secs = rl_reset
                .as_deref()
                .and_then(|v| v.split(',').next())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(attempt as u64 + 1);

            let wait = wait_secs.min(5);
            tracing::debug!(target: "image_fetch", wait_secs = wait, attempt = attempt + 1, max_retries = MAX_RETRIES, "Retrying");
            std::thread::sleep(std::time::Duration::from_secs(wait));
            continue;
        }

        if !status.is_success() {
            // Read the error body for diagnostics
            let body_preview = resp
                .text()
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect::<String>();
            if !body_preview.is_empty() {
                tracing::debug!(target: "image_fetch", body = %body_preview, "Error response");
            }
            return Err(RejectionReason::FetchFailed {
                message: format!("HTTP {}", status),
            });
        }

        // Pre-check Content-Length if available
        if let Some(len) = resp.content_length() {
            if len as usize > MAX_DOWNLOAD_BYTES {
                return Err(RejectionReason::TooLarge {
                    bytes: len as usize,
                });
            }
        }

        let bytes = resp
            .bytes()
            .map_err(|e| RejectionReason::FetchFailed {
                message: format!("Body read error: {}", e),
            })?;

        if bytes.len() > MAX_DOWNLOAD_BYTES {
            return Err(RejectionReason::TooLarge { bytes: bytes.len() });
        }

        tracing::debug!(target: "image_fetch", size = bytes.len(), "Fetch OK");
        return Ok(bytes.to_vec());
    }

    Err(RejectionReason::FetchFailed {
        message: format!(
            "HTTP {} (after {} retries)",
            last_status.map_or("unknown".to_string(), |s| s.to_string()),
            MAX_RETRIES
        ),
    })
}

/// Check if raw bytes look like SVG (text scan of first 512 bytes).
fn looks_like_svg(bytes: &[u8]) -> bool {
    let check_len = bytes.len().min(512);
    let snippet = String::from_utf8_lossy(&bytes[..check_len]).to_lowercase();
    snippet.contains("<svg") || snippet.contains("<!doctype svg")
}

/// Decode image with safety limits.
fn decode_with_limits(bytes: &[u8]) -> Result<image::DynamicImage, RejectionReason> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| RejectionReason::DecodeFailed {
            message: format!("Format guess failed: {}", e),
        })?;

    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOC_BYTES);

    let mut reader = reader;
    reader.limits(limits);

    reader.decode().map_err(|e| RejectionReason::DecodeFailed {
        message: format!("{}", e),
    })
}

/// Re-encode a decoded image to PNG and return a data URI.
fn reencode_to_png(img: &image::DynamicImage) -> Result<String, RejectionReason> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| RejectionReason::ReencodeFailed {
            message: format!("{}", e),
        })?;

    let b64 = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());
    Ok(format!("data:image/png;base64,{}", b64))
}

/// Compute the SHA-256 hex digest of a byte slice.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Optional VirusTotal check (only if VIRUSTOTAL_API_KEY is set).
fn check_virustotal(url: &str, _sha256: &str) -> VirusTotalStatus {
    let api_key = match std::env::var("VIRUSTOTAL_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => return VirusTotalStatus::Skipped,
    };

    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return VirusTotalStatus::Error {
                message: format!("Client error: {}", e),
            }
        }
    };

    let resp = client
        .post("https://www.virustotal.com/api/v3/urls")
        .header("x-apikey", &api_key)
        .form(&[("url", url)])
        .send();

    match resp {
        Ok(r) => {
            if let Ok(body) = r.json::<serde_json::Value>() {
                if let Some(id) = body
                    .get("data")
                    .and_then(|d| d.get("id"))
                    .and_then(|i| i.as_str())
                {
                    return VirusTotalStatus::Pending {
                        scan_id: id.to_string(),
                    };
                }
            }
            VirusTotalStatus::Error {
                message: "Unexpected VT response".to_string(),
            }
        }
        Err(e) => VirusTotalStatus::Error {
            message: format!("{}", e),
        },
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_validate_bytes_pipeline_png() {
        let mut result = ImageValidationResult {
            url: "test".to_string(),
            safe: false,
            rejection: None,
            data_uri: None,
            tracking_pixel: None,
            virus_total: VirusTotalStatus::Skipped,
            dimensions: None,
            sha256: None,
        };
        validate_bytes_pipeline(MINIMAL_PNG, &mut result);
        assert!(result.safe, "Should be safe: {:?}", result.rejection);
        assert!(result.data_uri.is_some());
        assert!(result.sha256.is_some());
        assert_eq!(result.dimensions, Some((1, 1)));
        // 1x1 is a tracking pixel
        assert!(result.tracking_pixel.is_some());
    }

    #[test]
    fn test_validate_bytes_pipeline_not_image() {
        let mut result = ImageValidationResult {
            url: "test".to_string(),
            safe: false,
            rejection: None,
            data_uri: None,
            tracking_pixel: None,
            virus_total: VirusTotalStatus::Skipped,
            dimensions: None,
            sha256: None,
        };
        validate_bytes_pipeline(b"not an image at all", &mut result);
        assert!(!result.safe);
        assert!(matches!(result.rejection, Some(RejectionReason::NotAnImage)));
    }

    #[test]
    fn test_validate_local_image_success() {
        let img_dir = allowed_image_dir();
        std::fs::create_dir_all(&img_dir).unwrap();
        let img_path = img_dir.join("test_security_local.png");
        std::fs::write(&img_path, MINIMAL_PNG).unwrap();

        let result = validate_local_image(img_path.to_str().unwrap());
        assert!(result.safe, "Should be safe: {:?}", result.rejection);
        assert!(result.data_uri.as_ref().unwrap().starts_with("data:image/png;base64,"));
        assert_eq!(result.dimensions, Some((1, 1)));

        std::fs::remove_file(&img_path).ok();
    }

    #[test]
    fn test_validate_local_image_outside_cache_blocked() {
        let result = validate_local_image("/etc/passwd");
        assert!(!result.safe);
        assert!(matches!(result.rejection, Some(RejectionReason::FetchFailed { .. })));
    }

    #[test]
    fn test_validate_local_image_missing_file() {
        let img_dir = allowed_image_dir();
        let missing = img_dir.join("does_not_exist_99999.png");
        let result = validate_local_image(missing.to_str().unwrap());
        assert!(!result.safe);
        assert!(matches!(result.rejection, Some(RejectionReason::FetchFailed { .. })));
    }

    #[test]
    fn test_looks_like_svg() {
        assert!(looks_like_svg(b"<svg xmlns='http://www.w3.org/2000/svg'>"));
        assert!(looks_like_svg(b"<!DOCTYPE svg"));
        assert!(!looks_like_svg(b"just some random bytes"));
        assert!(!looks_like_svg(MINIMAL_PNG));
    }

    #[test]
    fn test_normalize_url() {
        // Unicode en-dash should become ASCII hyphen
        assert_eq!(normalize_url("https://example\u{2013}site.com"), "https://example-site.com");
        // Zero-width space removed
        assert_eq!(normalize_url("https://example\u{200B}.com"), "https://example.com");
        // Non-breaking space becomes regular space
        assert_eq!(normalize_url("https://example\u{00A0}site.com"), "https://example site.com");
        // Normal URL unchanged
        assert_eq!(normalize_url("https://example.com/image.png"), "https://example.com/image.png");
    }

    #[test]
    fn test_sha256_hex_known_value() {
        let hash = sha256_hex(b"hello");
        assert_eq!(hash, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }
}
