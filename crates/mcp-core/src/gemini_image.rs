//! Gemini Image Generation API Client
//!
//! Generates images via the Gemini 2.5 Flash model's image generation capability.
//! Requires a `GEMINI_API_KEY` environment variable.

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

/// Check if Gemini image generation is configured (API key present)
pub fn is_configured() -> bool {
    std::env::var("GEMINI_API_KEY").is_ok()
}

// ============================================================================
// Response Types
// ============================================================================

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<Candidate>>,
}

#[derive(Debug, Deserialize)]
struct Candidate {
    content: Option<Content>,
}

#[derive(Debug, Deserialize)]
struct Content {
    parts: Option<Vec<Part>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Part {
    Text {
        text: String,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: InlineData,
    },
}

#[derive(Debug, Deserialize)]
struct InlineData {
    #[serde(rename = "mimeType")]
    mime_type: String,
    data: String,
}

// ============================================================================
// Image Generation
// ============================================================================

/// Generate an image using the Gemini API.
///
/// Returns a JSON string with prompt, image_path, and description.
pub fn generate_image(
    prompt: &str,
    aspect_ratio: Option<&str>,
    image_size: Option<&str>,
) -> Result<String> {
    let api_key = std::env::var("GEMINI_API_KEY")
        .context("Missing GEMINI_API_KEY environment variable")?;

    // Build imageConfig with optional aspect_ratio and imageSize
    let mut image_config = serde_json::Map::new();
    if let Some(ar) = aspect_ratio {
        image_config.insert("aspectRatio".into(), serde_json::json!(ar));
    }
    // Valid sizes: "1K" (default), "2K", "4K"
    if let Some(size) = image_size {
        image_config.insert("imageSize".into(), serde_json::json!(size));
    }

    let mut gen_config = serde_json::json!({
        "responseModalities": ["IMAGE"]
    });
    if !image_config.is_empty() {
        gen_config["imageConfig"] = serde_json::Value::Object(image_config);
    }

    let body = serde_json::json!({
        "contents": [{
            "parts": [{ "text": prompt }]
        }],
        "generationConfig": gen_config
    });

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("Failed to create HTTP client")?;

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-image:generateContent?key={}",
        api_key
    );

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .context("Gemini API request failed")?;

    let status = response.status();
    let response_body = response.text().context("Failed to read response")?;

    if !status.is_success() {
        return Err(anyhow!("Gemini API error {}: {}", status, response_body));
    }

    let parsed: GeminiResponse =
        serde_json::from_str(&response_body).context("Failed to parse Gemini response")?;

    let parts = parsed
        .candidates
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.content)
        .and_then(|c| c.parts)
        .unwrap_or_default();

    let mut description = String::new();
    let mut image_path: Option<PathBuf> = None;

    for part in parts {
        match part {
            Part::Text { text } => {
                if !description.is_empty() {
                    description.push(' ');
                }
                description.push_str(&text);
            }
            Part::InlineData { inline_data } => {
                if image_path.is_none() {
                    let path = save_image(&inline_data.data, &inline_data.mime_type)?;
                    image_path = Some(path);
                }
            }
        }
    }

    let image_path_str = image_path
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let output = serde_json::json!({
        "prompt": prompt,
        "image_path": image_path_str,
        "description": description,
    });

    serde_json::to_string(&output).map_err(|e| anyhow!("Failed to serialize result: {}", e))
}

// ============================================================================
// Image Understanding (Vision)
// ============================================================================

/// Default prompt for `describe_image` — casual, concise description.
const DESCRIBE_PROMPT: &str = "\
Briefly describe what is in this image in 2-4 sentences. \
Be factual and concise. Identify the main subject, setting, and any notable \
details, but do not provide a structured analysis or bullet points.";

/// Default prompt for `analyze_image` — detailed, structured analysis.
const ANALYZE_PROMPT: &str = "\
Provide a detailed, structured analysis of this image. Include:\n\
- **Subject**: Who or what is depicted, their pose, expression, clothing\n\
- **Setting/Background**: Location, décor, lighting, atmosphere\n\
- **Composition**: Framing, angle, depth of field, color palette\n\
- **Context**: What event or situation this likely depicts\n\
Use a table or bullet points for key visual details.";

/// Default prompt for `extract_text` — OCR / text extraction.
const EXTRACT_TEXT_PROMPT: &str = "\
Extract ALL text visible in this image. Reproduce the text exactly as written, \
preserving line breaks, formatting, and layout where possible. \
If there are multiple text regions (signs, labels, captions, handwriting), \
separate them with blank lines. Output ONLY the extracted text, no commentary.";

/// Default prompt for `detect_objects` — object detection with bounding boxes.
const DETECT_OBJECTS_PROMPT: &str = "\
Detect all distinct objects in this image. For each object, provide:\n\
- label: what the object is\n\
- bounding_box: [y_min, x_min, y_max, x_max] normalized to 0-1000 scale\n\
- confidence: your confidence (high, medium, low)\n\n\
Return ONLY a JSON array of objects, e.g.:\n\
[{\"label\": \"cat\", \"bounding_box\": [100, 200, 500, 600], \"confidence\": \"high\"}]";

/// Default prompt for `compare_images` — multi-image comparison.
const COMPARE_IMAGES_PROMPT: &str = "\
Compare these images and describe:\n\
- What they have in common\n\
- Key differences between them\n\
- Any changes, additions, or removals if they appear to be related (e.g., before/after)\n\
Be specific and factual.";

/// Describe an image — returns a brief, natural-language description.
///
/// Maps to the OpenAI `input_image` built-in behavior ("what is in this image?").
/// When a custom prompt is provided, the brevity constraint from DESCRIBE_PROMPT
/// is prepended so Gemini doesn't produce a detailed structured analysis.
pub fn describe_image(image_source: &str, prompt: Option<&str>) -> Result<String> {
    let prompt_text = match prompt {
        None => DESCRIBE_PROMPT.to_string(),
        Some(p) => format!(
            "Answer in 2-4 concise sentences. No tables, bullet points, or structured analysis.\n\n{}",
            p
        ),
    };
    send_image_to_gemini(image_source, &prompt_text)
}

/// Analyze an image — returns a detailed, structured analysis.
///
/// Maps to the `analyze_image` tool ("analyze this image").
pub fn analyze_image(image_source: &str, prompt: Option<&str>) -> Result<String> {
    let prompt_text = prompt.unwrap_or(ANALYZE_PROMPT);
    send_image_to_gemini(image_source, prompt_text)
}

/// Extract text (OCR) from an image — returns all visible text.
pub fn extract_text(image_source: &str, prompt: Option<&str>) -> Result<String> {
    let prompt_text = match prompt {
        None => EXTRACT_TEXT_PROMPT.to_string(),
        Some(p) => format!(
            "Extract text from this image. Output ONLY the extracted text, no commentary.\n\n{}",
            p
        ),
    };
    send_image_to_gemini(image_source, &prompt_text)
}

/// Detect objects in an image — returns JSON with labels and bounding boxes.
pub fn detect_objects(image_source: &str, prompt: Option<&str>) -> Result<String> {
    let prompt_text = match prompt {
        None => DETECT_OBJECTS_PROMPT.to_string(),
        Some(p) => format!(
            "Detect objects in this image. Return a JSON array with label, bounding_box \
             [y_min, x_min, y_max, x_max] (0-1000 scale), and confidence for each.\n\n{}",
            p
        ),
    };
    send_image_to_gemini(image_source, &prompt_text)
}

/// Compare multiple images — returns a description of similarities and differences.
pub fn compare_images(image_sources: &[&str], prompt: Option<&str>) -> Result<String> {
    let prompt_text = prompt.unwrap_or(COMPARE_IMAGES_PROMPT);
    send_multi_image_to_gemini(image_sources, prompt_text)
}

/// Footnote appended to all image-to-text results.
const GEMINI_FOOTNOTE: &str = "\n\n_(processed externally via Google Gemini)_";

/// Shared implementation: load image, send to Gemini, return text response.
fn send_image_to_gemini(image_source: &str, prompt_text: &str) -> Result<String> {
    use base64::Engine;

    let api_key = std::env::var("GEMINI_API_KEY")
        .context("Missing GEMINI_API_KEY environment variable")?;

    // Build the image part: inline base64 for local files, URL for remote
    let image_part = if image_source.starts_with("http://") || image_source.starts_with("https://") {
        // Fetch the image from URL
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        let response = client
            .get(image_source)
            .send()
            .context("Failed to fetch image from URL")?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to fetch image: HTTP {}", response.status()));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        let mime = normalize_mime_type(&content_type);

        let bytes = response.bytes().context("Failed to read image bytes")?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

        serde_json::json!({
            "inlineData": {
                "mimeType": mime,
                "data": b64
            }
        })
    } else {
        // Local file
        let path = std::path::Path::new(image_source);
        if !path.exists() {
            return Err(anyhow!("Image file not found: {}", image_source));
        }

        let bytes = fs::read(path)
            .context("Failed to read image file")?;

        let mime = mime_from_extension(path);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

        serde_json::json!({
            "inlineData": {
                "mimeType": mime,
                "data": b64
            }
        })
    };

    let body = serde_json::json!({
        "contents": [{
            "parts": [
                { "text": prompt_text },
                image_part
            ]
        }]
    });

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .context("Failed to create HTTP client")?;

    // Use gemini-2.5-flash for vision (supports image input natively)
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key={}",
        api_key
    );

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .context("Gemini API request failed")?;

    let status = response.status();
    let response_body = response.text().context("Failed to read response")?;

    if !status.is_success() {
        return Err(anyhow!("Gemini API error {}: {}", status, response_body));
    }

    let parsed: GeminiResponse =
        serde_json::from_str(&response_body).context("Failed to parse Gemini response")?;

    let parts = parsed
        .candidates
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.content)
        .and_then(|c| c.parts)
        .unwrap_or_default();

    // Collect all text parts
    let mut analysis = String::new();
    for part in parts {
        if let Part::Text { text } = part {
            if !analysis.is_empty() {
                analysis.push('\n');
            }
            analysis.push_str(&text);
        }
    }

    if analysis.is_empty() {
        return Err(anyhow!("Gemini returned no text response for the image"));
    }

    analysis.push_str(GEMINI_FOOTNOTE);

    let output = serde_json::json!({
        "source": image_source,
        "description": analysis,
    });

    serde_json::to_string_pretty(&output).map_err(|e| anyhow!("Failed to serialize result: {}", e))
}

/// Build a Gemini inline-data part for a single image source (URL or local file).
fn build_image_part(image_source: &str) -> Result<serde_json::Value> {
    use base64::Engine;

    if image_source.starts_with("http://") || image_source.starts_with("https://") {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        let response = client
            .get(image_source)
            .send()
            .context("Failed to fetch image from URL")?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to fetch image: HTTP {}", response.status()));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        let mime = normalize_mime_type(&content_type);
        let bytes = response.bytes().context("Failed to read image bytes")?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

        Ok(serde_json::json!({
            "inlineData": { "mimeType": mime, "data": b64 }
        }))
    } else {
        let path = std::path::Path::new(image_source);
        if !path.exists() {
            return Err(anyhow!("Image file not found: {}", image_source));
        }
        let bytes = fs::read(path).context("Failed to read image file")?;
        let mime = mime_from_extension(path);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

        Ok(serde_json::json!({
            "inlineData": { "mimeType": mime, "data": b64 }
        }))
    }
}

/// Send multiple images to Gemini with a prompt — for compare_images.
fn send_multi_image_to_gemini(image_sources: &[&str], prompt_text: &str) -> Result<String> {
    let api_key = std::env::var("GEMINI_API_KEY")
        .context("Missing GEMINI_API_KEY environment variable")?;

    if image_sources.is_empty() {
        return Err(anyhow!("No image sources provided"));
    }
    if image_sources.len() < 2 {
        return Err(anyhow!("compare_images requires at least 2 images"));
    }

    // Build parts: prompt text + all images
    let mut parts = vec![serde_json::json!({ "text": prompt_text })];
    for (i, source) in image_sources.iter().enumerate() {
        let image_part = build_image_part(source)
            .with_context(|| format!("Failed to load image {} ({})", i + 1, source))?;
        parts.push(image_part);
    }

    let body = serde_json::json!({
        "contents": [{ "parts": parts }]
    });

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .context("Failed to create HTTP client")?;

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key={}",
        api_key
    );

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .context("Gemini API request failed")?;

    let status = response.status();
    let response_body = response.text().context("Failed to read response")?;

    if !status.is_success() {
        return Err(anyhow!("Gemini API error {}: {}", status, response_body));
    }

    let parsed: GeminiResponse =
        serde_json::from_str(&response_body).context("Failed to parse Gemini response")?;

    let resp_parts = parsed
        .candidates
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.content)
        .and_then(|c| c.parts)
        .unwrap_or_default();

    let mut analysis = String::new();
    for part in resp_parts {
        if let Part::Text { text } = part {
            if !analysis.is_empty() {
                analysis.push('\n');
            }
            analysis.push_str(&text);
        }
    }

    if analysis.is_empty() {
        return Err(anyhow!("Gemini returned no text response for the images"));
    }

    analysis.push_str(GEMINI_FOOTNOTE);

    let output = serde_json::json!({
        "sources": image_sources,
        "description": analysis,
    });

    serde_json::to_string_pretty(&output).map_err(|e| anyhow!("Failed to serialize result: {}", e))
}

/// Normalize a content-type header to a clean MIME type
fn normalize_mime_type(content_type: &str) -> String {
    // Strip charset and other parameters: "image/jpeg; charset=utf-8" → "image/jpeg"
    content_type
        .split(';')
        .next()
        .unwrap_or("image/jpeg")
        .trim()
        .to_string()
}

/// Infer MIME type from file extension
fn mime_from_extension(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        Some("heic") | Some("heif") => "image/heic",
        Some("pdf") => "application/pdf",
        _ => "image/jpeg", // safe default
    }
}

/// Save base64-encoded image data to the images cache directory.
fn save_image(base64_data: &str, mime_type: &str) -> Result<PathBuf> {
    use base64::Engine;

    let ext = match mime_type {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    };

    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("prolog-router")
        .join("images");

    fs::create_dir_all(&cache_dir)
        .context("Failed to create images cache directory")?;

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let filename = format!("{}.{}", timestamp, ext);
    let path = cache_dir.join(&filename);

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data)
        .context("Failed to decode base64 image data")?;

    fs::write(&path, &bytes)
        .context("Failed to write image file")?;

    tracing::debug!(path = ?path, size = bytes.len(), "Saved generated image");

    Ok(path)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_configured() {
        std::env::remove_var("GEMINI_API_KEY");
        assert!(!is_configured());

        std::env::set_var("GEMINI_API_KEY", "test-key");
        assert!(is_configured());

        std::env::remove_var("GEMINI_API_KEY");
    }

    #[test]
    fn test_parse_response_with_text_and_image() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [
                        { "text": "Here is a cat with a top hat." },
                        { "inlineData": { "mimeType": "image/png", "data": "iVBORw0KGgo=" } }
                    ]
                }
            }]
        }"#;

        let parsed: GeminiResponse = serde_json::from_str(json).unwrap();
        let candidates = parsed.candidates.unwrap();
        let content = candidates[0].content.as_ref().unwrap();
        let parts = content.parts.as_ref().unwrap();
        assert_eq!(parts.len(), 2);

        match &parts[0] {
            Part::Text { text } => assert_eq!(text, "Here is a cat with a top hat."),
            _ => panic!("Expected text part"),
        }

        match &parts[1] {
            Part::InlineData { inline_data } => {
                assert_eq!(inline_data.mime_type, "image/png");
                assert!(!inline_data.data.is_empty());
            }
            _ => panic!("Expected inline data part"),
        }
    }

    #[test]
    fn test_parse_response_no_candidates() {
        let json = r#"{}"#;
        let parsed: GeminiResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.candidates.is_none());
    }

    #[test]
    fn test_normalize_mime_type() {
        assert_eq!(normalize_mime_type("image/jpeg"), "image/jpeg");
        assert_eq!(normalize_mime_type("image/png; charset=utf-8"), "image/png");
        assert_eq!(normalize_mime_type("image/webp; boundary=x"), "image/webp");
    }

    #[test]
    fn test_mime_from_extension() {
        assert_eq!(mime_from_extension(std::path::Path::new("photo.jpg")), "image/jpeg");
        assert_eq!(mime_from_extension(std::path::Path::new("photo.jpeg")), "image/jpeg");
        assert_eq!(mime_from_extension(std::path::Path::new("icon.png")), "image/png");
        assert_eq!(mime_from_extension(std::path::Path::new("pic.webp")), "image/webp");
        assert_eq!(mime_from_extension(std::path::Path::new("photo.heic")), "image/heic");
        assert_eq!(mime_from_extension(std::path::Path::new("noext")), "image/jpeg");
    }

    #[test]
    fn test_analyze_image_missing_file() {
        std::env::set_var("GEMINI_API_KEY", "test-key");
        let result = analyze_image("/nonexistent/path/image.png", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
        std::env::remove_var("GEMINI_API_KEY");
    }

    #[test]
    fn test_analyze_image_no_api_key() {
        std::env::remove_var("GEMINI_API_KEY");
        let result = analyze_image("/some/image.png", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("GEMINI_API_KEY"));
    }
}
