//! External Tool Configuration and Execution
//!
//! Loads tool definitions from JSON config files compatible with OpenAI/Anthropic formats.
//! Executes tools via HTTP endpoints with template substitution.

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::time::Duration;

// ============================================================================
// Tool Configuration Structs
// ============================================================================

/// Root configuration containing all tool definitions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsConfig {
    pub tools: Vec<ToolDef>,
}

/// A single tool definition (compatible with OpenAI/Anthropic format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    /// Tool name (e.g., "get_weather")
    pub name: String,

    /// Human-readable description
    pub description: String,

    /// JSON Schema for parameters (OpenAI function calling format)
    pub parameters: Value,

    /// Optional endpoint configuration for HTTP execution
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<EndpointConfig>,
}

/// HTTP endpoint configuration for tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointConfig {
    /// URL template (supports {{arg}} substitution)
    pub url: String,

    /// HTTP method (GET, POST, PUT, DELETE)
    #[serde(default = "default_method")]
    pub method: String,

    /// Query parameters (for GET requests)
    #[serde(default)]
    pub query: HashMap<String, String>,

    /// HTTP headers
    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// Request body template (for POST/PUT)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,

    /// JSONPath to extract from response (e.g., "$.weather[0].description")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_path: Option<String>,

    /// Request timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_method() -> String {
    "GET".to_string()
}

fn default_timeout() -> u64 {
    30
}

// ============================================================================
// Template Substitution
// ============================================================================

/// Substitute {{arg}} placeholders from tool arguments
fn substitute_args(template: &str, args: &Value) -> Result<String> {
    let mut result = template.to_string();

    // Find all {{name}} patterns
    let re = regex::Regex::new(r"\{\{(\w+)\}\}").expect("Invalid regex");

    for cap in re.captures_iter(template) {
        let full_match = &cap[0];
        let arg_name = &cap[1];

        let value = args
            .get(arg_name)
            .ok_or_else(|| anyhow!("Missing required argument: {}", arg_name))?;

        let replacement = match value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => value.to_string(),
        };

        result = result.replace(full_match, &replacement);
    }

    Ok(result)
}

/// Substitute ${ENV_VAR} placeholders from environment
fn substitute_env(template: &str) -> Result<String> {
    let mut result = template.to_string();

    // Find all ${NAME} patterns
    let re = regex::Regex::new(r"\$\{(\w+)\}").expect("Invalid regex");

    for cap in re.captures_iter(template) {
        let full_match = &cap[0];
        let var_name = &cap[1];

        let value = env::var(var_name)
            .with_context(|| format!("Missing environment variable: {}", var_name))?;

        result = result.replace(full_match, &value);
    }

    Ok(result)
}

/// Full template substitution: first args, then env
fn substitute_template(template: &str, args: &Value) -> Result<String> {
    let after_args = substitute_args(template, args)?;
    substitute_env(&after_args)
}

/// Substitute templates in a HashMap of strings
fn substitute_map(map: &HashMap<String, String>, args: &Value) -> Result<HashMap<String, String>> {
    map.iter()
        .map(|(k, v)| {
            let substituted = substitute_template(v, args)?;
            Ok((k.clone(), substituted))
        })
        .collect()
}

/// Substitute templates in a JSON Value recursively
fn substitute_value(value: &Value, args: &Value) -> Result<Value> {
    match value {
        Value::String(s) => {
            let substituted = substitute_template(s, args)?;
            Ok(Value::String(substituted))
        }
        Value::Array(arr) => {
            let substituted: Result<Vec<Value>> =
                arr.iter().map(|v| substitute_value(v, args)).collect();
            Ok(Value::Array(substituted?))
        }
        Value::Object(obj) => {
            let substituted: Result<serde_json::Map<String, Value>> = obj
                .iter()
                .map(|(k, v)| {
                    let sub_v = substitute_value(v, args)?;
                    Ok((k.clone(), sub_v))
                })
                .collect();
            Ok(Value::Object(substituted?))
        }
        // Numbers, bools, null pass through unchanged
        other => Ok(other.clone()),
    }
}

// ============================================================================
// Simple JSONPath Extraction
// ============================================================================

/// Extract a value from JSON using a simple JSONPath-like expression
/// Supports: $.field, $.field.subfield, $.array[0], $.array[0].field
fn extract_json_path(value: &Value, path: &str) -> Result<String> {
    let path = path.strip_prefix("$.").unwrap_or(path);

    if path.is_empty() {
        return value_to_string(value);
    }

    let mut current = value;

    for segment in path.split('.') {
        // Check for array index: field[0]
        if let Some(bracket_pos) = segment.find('[') {
            let field = &segment[..bracket_pos];
            let index_str = segment[bracket_pos + 1..]
                .strip_suffix(']')
                .ok_or_else(|| anyhow!("Invalid array index syntax: {}", segment))?;
            let index: usize = index_str
                .parse()
                .with_context(|| format!("Invalid array index: {}", index_str))?;

            // Navigate to field if present
            if !field.is_empty() {
                current = current
                    .get(field)
                    .ok_or_else(|| anyhow!("Field not found: {}", field))?;
            }

            // Navigate to array index
            current = current
                .get(index)
                .ok_or_else(|| anyhow!("Array index out of bounds: {}", index))?;
        } else {
            // Simple field access
            current = current
                .get(segment)
                .ok_or_else(|| anyhow!("Field not found: {}", segment))?;
        }
    }

    value_to_string(current)
}

fn value_to_string(value: &Value) -> Result<String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null => Ok("null".to_string()),
        _ => Ok(value.to_string()),
    }
}

// ============================================================================
// Tool Executor
// ============================================================================

/// Executes tools based on configuration
#[derive(Clone)]
pub struct ToolExecutor {
    client: Client,
    tools: HashMap<String, ToolDef>,
}

impl ToolExecutor {
    /// Load tool configuration from a JSON file
    pub fn load(path: &Path) -> Result<Self> {
        let content =
            fs::read_to_string(path).with_context(|| format!("Failed to read: {}", path.display()))?;

        let config: ToolsConfig =
            serde_json::from_str(&content).with_context(|| "Failed to parse tools config JSON")?;

        let tools: HashMap<String, ToolDef> = config
            .tools
            .into_iter()
            .map(|t| (t.name.clone(), t))
            .collect();

        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self { client, tools })
    }

    /// Check if a tool is configured (useful for LLM function calling)
    #[allow(dead_code)]
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Get tool definition (for LLM function calling schema)
    #[allow(dead_code)]
    pub fn get_tool(&self, name: &str) -> Option<&ToolDef> {
        self.tools.get(name)
    }

    /// Get all tool definitions (for LLM function calling)
    pub fn all_tools(&self) -> impl Iterator<Item = &ToolDef> {
        self.tools.values()
    }

    /// Check if a tool has an endpoint configured
    pub fn has_endpoint(&self, name: &str) -> bool {
        self.tools
            .get(name)
            .map(|t| t.endpoint.is_some())
            .unwrap_or(false)
    }

    /// Execute a tool with given arguments
    /// Returns None if the tool exists but has no endpoint (caller should use stub)
    pub fn execute(&self, tool_name: &str, args: &Value) -> Result<Option<String>> {
        let tool = self
            .tools
            .get(tool_name)
            .ok_or_else(|| anyhow!("Tool not found in config: {}", tool_name))?;

        match &tool.endpoint {
            Some(endpoint) => Ok(Some(self.execute_endpoint(endpoint, args)?)),
            None => Ok(None), // No endpoint - caller should use stub
        }
    }

    fn execute_endpoint(&self, endpoint: &EndpointConfig, args: &Value) -> Result<String> {
        // Substitute URL
        let url = substitute_template(&endpoint.url, args)?;

        // Substitute query params
        let query_params = substitute_map(&endpoint.query, args)?;

        // Substitute headers
        let headers = substitute_map(&endpoint.headers, args)?;

        // Build request
        let method = endpoint.method.to_uppercase();
        let mut request = match method.as_str() {
            "GET" => self.client.get(&url),
            "POST" => self.client.post(&url),
            "PUT" => self.client.put(&url),
            "DELETE" => self.client.delete(&url),
            "PATCH" => self.client.patch(&url),
            _ => return Err(anyhow!("Unsupported HTTP method: {}", method)),
        };

        // Add timeout
        request = request.timeout(Duration::from_secs(endpoint.timeout_secs));

        // Add query params
        if !query_params.is_empty() {
            request = request.query(&query_params);
        }

        // Add headers
        for (key, value) in &headers {
            request = request.header(key.as_str(), value.as_str());
        }

        // Add body for POST/PUT
        if let Some(ref body_template) = endpoint.body {
            let body = substitute_value(body_template, args)?;
            request = request.json(&body);
        }

        // Execute
        let response = request.send().context("HTTP request failed")?;

        let status = response.status();
        let body = response.text().context("Failed to read response body")?;

        if !status.is_success() {
            return Err(anyhow!("HTTP {} - {}", status, body));
        }

        // Extract response using JSONPath if specified
        if let Some(ref path) = endpoint.response_path {
            let json: Value =
                serde_json::from_str(&body).context("Response is not valid JSON")?;
            extract_json_path(&json, path)
        } else {
            Ok(body)
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_args() {
        let args = serde_json::json!({
            "location": "NYC",
            "date": "2026-01-26"
        });

        let result = substitute_args("Weather for {{location}} on {{date}}", &args).unwrap();
        assert_eq!(result, "Weather for NYC on 2026-01-26");
    }

    #[test]
    fn test_substitute_args_missing() {
        let args = serde_json::json!({
            "location": "NYC"
        });

        let result = substitute_args("Weather for {{location}} on {{date}}", &args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("date"));
    }

    #[test]
    fn test_substitute_env() {
        env::set_var("TEST_VAR_12345", "test_value");
        let result = substitute_env("Key: ${TEST_VAR_12345}").unwrap();
        assert_eq!(result, "Key: test_value");
        env::remove_var("TEST_VAR_12345");
    }

    #[test]
    fn test_substitute_env_missing() {
        let result = substitute_env("Key: ${NONEXISTENT_VAR_99999}");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("NONEXISTENT_VAR_99999"));
    }

    #[test]
    fn test_substitute_template_combined() {
        env::set_var("API_KEY_TEST", "secret123");
        let args = serde_json::json!({ "city": "Paris" });

        let result =
            substitute_template("https://api.example.com/weather?city={{city}}&key=${API_KEY_TEST}", &args)
                .unwrap();
        assert_eq!(
            result,
            "https://api.example.com/weather?city=Paris&key=secret123"
        );

        env::remove_var("API_KEY_TEST");
    }

    #[test]
    fn test_extract_json_path_simple() {
        let json = serde_json::json!({
            "weather": [
                { "description": "sunny" },
                { "description": "cloudy" }
            ],
            "name": "London"
        });

        assert_eq!(extract_json_path(&json, "$.name").unwrap(), "London");
        assert_eq!(
            extract_json_path(&json, "$.weather[0].description").unwrap(),
            "sunny"
        );
        assert_eq!(
            extract_json_path(&json, "$.weather[1].description").unwrap(),
            "cloudy"
        );
    }

    #[test]
    fn test_extract_json_path_nested() {
        let json = serde_json::json!({
            "data": {
                "results": [
                    { "value": 42 }
                ]
            }
        });

        assert_eq!(
            extract_json_path(&json, "$.data.results[0].value").unwrap(),
            "42"
        );
    }

    #[test]
    fn test_parse_tools_config() {
        let json = r#"{
            "tools": [
                {
                    "name": "get_weather",
                    "description": "Get weather for a location",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "location": { "type": "string" }
                        },
                        "required": ["location"]
                    },
                    "endpoint": {
                        "url": "https://api.example.com/weather",
                        "method": "GET",
                        "query": { "q": "{{location}}" }
                    }
                }
            ]
        }"#;

        let config: ToolsConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.tools.len(), 1);
        assert_eq!(config.tools[0].name, "get_weather");
        assert!(config.tools[0].endpoint.is_some());
    }

    #[test]
    fn test_substitute_value_object() {
        let template = serde_json::json!({
            "query": "{{search_term}}",
            "options": {
                "limit": 10,
                "filter": "{{filter_type}}"
            }
        });

        let args = serde_json::json!({
            "search_term": "rust programming",
            "filter_type": "recent"
        });

        let result = substitute_value(&template, &args).unwrap();
        assert_eq!(result["query"], "rust programming");
        assert_eq!(result["options"]["filter"], "recent");
        assert_eq!(result["options"]["limit"], 10); // Unchanged
    }

    /// Integration test that hits httpbin.org to verify HTTP execution
    /// Run with: cargo test test_httpbin_integration -- --ignored
    #[test]
    #[ignore]
    fn test_httpbin_integration() {
        let config_json = r#"{
            "tools": [{
                "name": "echo",
                "description": "Echo test",
                "parameters": { "type": "object", "properties": { "msg": { "type": "string" } } },
                "endpoint": {
                    "url": "https://httpbin.org/get",
                    "method": "GET",
                    "query": { "message": "{{msg}}" },
                    "response_path": "$.args.message",
                    "timeout_secs": 15
                }
            }]
        }"#;

        // Write config to temp file
        let temp_dir = std::env::temp_dir();
        let config_path = temp_dir.join("test_tools_httpbin.json");
        std::fs::write(&config_path, config_json).unwrap();

        // Load and execute
        let executor = ToolExecutor::load(&config_path).unwrap();
        let args = serde_json::json!({ "msg": "hello_world_test" });
        let result = executor.execute("echo", &args).unwrap();

        assert!(result.is_some());
        assert_eq!(result.unwrap(), "hello_world_test");

        // Cleanup
        std::fs::remove_file(&config_path).ok();
    }
}
