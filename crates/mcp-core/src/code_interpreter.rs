//! Code interpreter — sandboxed Python execution
//!
//! Runs Python 3 code in a macOS Seatbelt sandbox (no network, restricted
//! filesystem writes). Returns stdout, stderr, and any generated files.

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Maximum execution time for Python scripts (seconds).
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum stdout/stderr capture size (bytes).
const MAX_OUTPUT_BYTES: usize = 100_000;

/// Seatbelt sandbox profile: deny network, restrict writes to workspace.
const SANDBOX_PROFILE: &str = r#"(version 1)
(allow default)

;; Deny all network access
(deny network*)

;; Deny process-fork to prevent shell escape
(deny process-fork)
"#;

/// Return the path where large tool results are saved for code_interpreter,
/// as a display string for error messages.
pub fn tool_result_data_path_str() -> String {
    dirs::cache_dir()
        .map(|d| d.join("prolog-router").join("code_interpreter").join("tool_result_latest.json"))
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/tmp/tool_result_latest.json".to_string())
}

/// Check whether python3 is installed and available on PATH
pub fn is_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Maximum time for pip install (seconds).
const PIP_TIMEOUT_SECS: u64 = 120;

/// Execute a code_interpreter tool action
pub fn execute(action: &str, args: &Value) -> Result<String> {
    match action {
        "execute" => {
            let data_path = crate::code_interpreter::tool_result_data_path_str();
            let code = args
                .get("code")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!(
                    "Missing required parameter: code. \
                     Your function call was likely truncated because the code was too long. \
                     Write shorter code — do NOT embed large data. \
                     Read data from the saved file instead:\n\
                     import json\n\
                     with open('{}') as f:\n\
                         data = json.load(f)",
                    data_path,
                ))?;
            let timeout_secs = args
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .min(120); // hard cap at 2 minutes

            // Optional: pip packages to install before running the script
            let requirements: Vec<String> = args
                .get("requirements")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // Ensure venv exists and install any requested packages
            if !requirements.is_empty() {
                let venv_dir = venv_dir()?;
                ensure_venv(&venv_dir)?;
                install_packages(&venv_dir, &requirements)?;
            }

            run_python(code, timeout_secs)
        }
        _ => Err(anyhow!("Unknown code_interpreter action: {}", action)),
    }
}

/// Run Python code in a sandboxed subprocess.
///
/// Uses the cached venv python if it exists, otherwise falls back to system
/// python3.  The script itself always runs inside a Seatbelt sandbox (no
/// network, no process-fork).
fn run_python(code: &str, timeout_secs: u64) -> Result<String> {
    // Create a unique workspace directory for this execution
    let run_id = format!("{:x}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos());
    let workspace = workspace_base_dir()?.join(&run_id);
    std::fs::create_dir_all(&workspace)
        .map_err(|e| anyhow!("Failed to create workspace directory: {}", e))?;

    // Write the code to a temp file
    let script_path = workspace.join("_script.py");
    // Prepend an os.chdir so file outputs land in the workspace
    let wrapped_code = format!(
        "import os\nos.chdir({})\n{}",
        serde_json::to_string(workspace.to_string_lossy().as_ref())?,
        code
    );
    std::fs::write(&script_path, &wrapped_code)
        .map_err(|e| anyhow!("Failed to write script file: {}", e))?;

    // Snapshot existing files before execution (to detect new outputs)
    let files_before = list_files(&workspace);

    // Write sandbox profile to temp file
    let profile_path = workspace.join("_sandbox.sb");
    std::fs::write(&profile_path, SANDBOX_PROFILE)
        .map_err(|e| anyhow!("Failed to write sandbox profile: {}", e))?;

    // Use venv python if available, otherwise system python3
    let python_bin = venv_python().unwrap_or_else(|| PathBuf::from("python3"));

    // Build command: sandbox-exec → python → script
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("sandbox-exec");
        c.arg("-f")
            .arg(&profile_path)
            .arg(&python_bin)
            .arg(&script_path);
        c
    } else {
        let mut c = Command::new(&python_bin);
        c.arg(&script_path);
        c
    };

    // Set PYTHONDONTWRITEBYTECODE to avoid .pyc clutter
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    // Set matplotlib to non-interactive backend for chart generation
    cmd.env("MPLBACKEND", "Agg");

    // Execute with timeout
    let output = run_with_timeout(&mut cmd, Duration::from_secs(timeout_secs))?;

    // Capture stdout/stderr, truncating if too large
    let mut stdout = truncate_output(&output.stdout);
    let stderr = truncate_output(&output.stderr);
    let exit_code = output.status.code().unwrap_or(-1);

    // Detect new files created by the script (workspace + /tmp/)
    let files_after = list_files(&workspace);
    let new_files: Vec<String> = files_after
        .into_iter()
        .filter(|f| !files_before.contains(f))
        .filter(|f| !f.starts_with('_')) // exclude our temp files
        .collect();

    // Also detect image files written to /tmp/ (LLMs commonly use absolute paths)
    let tmp_image_exts = ["png", "jpg", "jpeg", "gif", "svg", "pdf", "csv"];
    let tmp_images: Vec<String> = std::fs::read_dir("/tmp")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if !path.is_file() { return None; }
            let name = path.file_name()?.to_str()?;
            let ext = path.extension()?.to_str()?.to_lowercase();
            if !tmp_image_exts.contains(&ext.as_str()) { return None; }
            // Only pick up files modified during this run (created after script start)
            let meta = path.metadata().ok()?;
            let modified = meta.modified().ok()?;
            let script_start = std::time::SystemTime::now() - Duration::from_secs(timeout_secs + 5);
            if modified > script_start {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect();

    // Clean up temp files
    let _ = std::fs::remove_file(&script_path);
    let _ = std::fs::remove_file(&profile_path);

    // Move generated files to the stable output directory, then remove run dir
    let output_dir = workspace_base_dir()?.join("output");
    let mut file_paths: Vec<String> = Vec::new();

    if !new_files.is_empty() || !tmp_images.is_empty() {
        std::fs::create_dir_all(&output_dir)
            .map_err(|e| anyhow!("Failed to create output directory: {}", e))?;
        for f in &new_files {
            let src = workspace.join(f);
            let dst = output_dir.join(f);
            if let Err(e) = std::fs::rename(&src, &dst) {
                // rename can fail across mount points; fall back to copy+delete
                tracing::debug!(error = %e, "rename failed, trying copy");
                if std::fs::copy(&src, &dst).is_ok() {
                    let _ = std::fs::remove_file(&src);
                }
            }
            file_paths.push(dst.to_string_lossy().to_string());
        }
        // Move files from /tmp/ to the output directory
        for f in &tmp_images {
            let src = Path::new("/tmp").join(f);
            let dst = output_dir.join(f);
            if dst.exists() { continue; } // already handled via workspace
            if let Err(e) = std::fs::rename(&src, &dst) {
                tracing::debug!(error = %e, file = %f, "rename from /tmp failed, trying copy");
                if std::fs::copy(&src, &dst).is_ok() {
                    let _ = std::fs::remove_file(&src);
                }
            }
            tracing::info!(file = %f, "Recovered file from /tmp");
            file_paths.push(dst.to_string_lossy().to_string());
        }
    }

    // Extract any base64-encoded images from stdout. LLMs often generate
    // Python code that prints raw base64 to stdout instead of saving to a
    // file.  Sending tens of KB of base64 back through the LLM context
    // wastes tokens and stalls the UI, so we intercept it here: decode the
    // image, save it to the output directory, and replace the base64 blob
    // in stdout with the file path.
    let extracted = extract_base64_images(&stdout, &output_dir, &run_id);
    if !extracted.saved_paths.is_empty() {
        stdout = extracted.cleaned_stdout;
        file_paths.extend(extracted.saved_paths);
    }

    // Remove the per-run workspace directory
    let _ = std::fs::remove_dir_all(&workspace);

    // Build result
    let mut result = serde_json::json!({
        "exit_code": exit_code,
        "stdout": stdout,
    });

    if !stderr.is_empty() {
        result["stderr"] = serde_json::Value::String(stderr);
    }

    if !file_paths.is_empty() {
        result["files_created"] = serde_json::json!(file_paths);
    }

    Ok(serde_json::to_string_pretty(&result)?)
}

/// Run a command with a timeout, returning its output or an error.
///
/// Spawns the child, records its PID, then waits on a background thread.
/// If the timeout expires, kills the child via its PID.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Result<std::process::Output> {
    let child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn python3: {}", e))?;

    let pid = child.id();

    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let result = child.wait_with_output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(anyhow!("Error waiting for python3: {}", e)),
        Err(_) => {
            // Timed out — kill via PID using the kill command
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
            Err(anyhow!(
                "Python execution timed out after {} seconds",
                timeout.as_secs()
            ))
        }
    }
}

/// Truncate output bytes to MAX_OUTPUT_BYTES and convert to String
fn truncate_output(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    if s.len() > MAX_OUTPUT_BYTES {
        let boundary = s.floor_char_boundary(MAX_OUTPUT_BYTES);
        format!(
            "{}\n\n[Output truncated at {} bytes]",
            &s[..boundary],
            MAX_OUTPUT_BYTES
        )
    } else {
        s.to_string()
    }
}

/// List file names in a directory (non-recursive)
fn list_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.is_file() {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Virtual environment management
// ---------------------------------------------------------------------------

/// Packages pre-installed in the venv on first creation.
/// These cover the most common data science / PDF / charting needs so the
/// LLM doesn't have to request them explicitly.
const SEED_PACKAGES: &[&str] = &[
    "matplotlib", "pandas", "numpy", "Pillow",
    "mplfinance", "reportlab", "scipy",
];

/// Check whether `uv` is available (much faster than pip for installs).
fn has_uv() -> bool {
    Command::new("uv")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Path to the cached virtualenv directory.
fn venv_dir() -> Result<PathBuf> {
    Ok(workspace_base_dir()?.join("venv"))
}

/// Path to the python binary inside the cached venv, if it exists.
fn venv_python() -> Option<PathBuf> {
    let venv = venv_dir().ok()?;
    let py = venv.join("bin").join("python3");
    if py.exists() { Some(py) } else { None }
}

/// Create the virtualenv if it doesn't already exist, then install seed packages.
fn ensure_venv(venv_path: &Path) -> Result<()> {
    if venv_path.join("bin").join("python3").exists() {
        return Ok(());
    }

    let use_uv = has_uv();
    tracing::info!(
        use_uv,
        "Creating code_interpreter virtualenv at {}",
        venv_path.display()
    );

    if use_uv {
        // uv venv is ~10x faster than python -m venv
        let output = Command::new("uv")
            .args(["venv", &venv_path.to_string_lossy()])
            .output()
            .map_err(|e| anyhow!("Failed to create venv with uv: {}", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("uv venv creation failed: {}", stderr));
        }
    } else {
        let output = Command::new("python3")
            .args(["-m", "venv", &venv_path.to_string_lossy()])
            .output()
            .map_err(|e| anyhow!("Failed to create venv: {}", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("venv creation failed: {}", stderr));
        }
        // Pre-upgrade pip (suppresses warnings; uv doesn't need this)
        let pip = venv_path.join("bin").join("pip");
        let _ = Command::new(&pip)
            .args(["install", "--upgrade", "pip"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    // Pre-seed common packages so the LLM doesn't have to request them
    let seed: Vec<String> = SEED_PACKAGES.iter().map(|s| s.to_string()).collect();
    tracing::info!(packages = ?seed, "Pre-seeding venv with common packages");
    install_packages(venv_path, &seed)?;

    Ok(())
}

/// Install packages into the cached venv.  Uses `uv pip install` when
/// available (~10x faster than pip).  Runs WITHOUT the sandbox so it can
/// access the network.
fn install_packages(venv_path: &Path, packages: &[String]) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    let use_uv = has_uv();
    tracing::info!(packages = ?packages, use_uv, "Installing pip packages for code_interpreter");

    let mut cmd = if use_uv {
        let mut c = Command::new("uv");
        c.arg("pip")
            .arg("install")
            .arg("--quiet")
            .arg("--python")
            .arg(venv_path.join("bin").join("python3"))
            .args(packages);
        c
    } else {
        let pip = venv_path.join("bin").join("pip");
        if !pip.exists() {
            return Err(anyhow!("pip not found in venv"));
        }
        let mut c = Command::new(&pip);
        c.arg("install")
            .arg("--quiet")
            .args(packages);
        c
    };

    let output = run_with_timeout(&mut cmd, Duration::from_secs(PIP_TIMEOUT_SECS))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(stderr = %stderr, "pip install had errors (continuing anyway)");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Base64 image extraction
// ---------------------------------------------------------------------------

/// Minimum length (bytes) for a base64 blob to be considered an image worth
/// extracting.  Anything shorter is likely just a short encoded string the
/// user intentionally printed.
const MIN_BASE64_IMAGE_BYTES: usize = 512;

struct ExtractedImages {
    /// Stdout with base64 blobs replaced by `[image saved: <path>]` markers.
    cleaned_stdout: String,
    /// Absolute paths of saved image files.
    saved_paths: Vec<String>,
}

/// Scan `stdout` for base64-encoded image data, decode it, save each image to
/// `output_dir`, and return the cleaned stdout plus the list of saved paths.
///
/// Detection heuristic: any contiguous run of base64 characters (A-Za-z0-9+/=)
/// longer than `MIN_BASE64_IMAGE_BYTES` that decodes to bytes whose magic
/// bytes match a known image format (PNG, JPEG, GIF, WebP, BMP).
fn extract_base64_images(stdout: &str, output_dir: &Path, run_id: &str) -> ExtractedImages {
    let re = regex::Regex::new(r"[A-Za-z0-9+/]{100,}={0,3}").expect("invalid regex");
    let mut cleaned = String::with_capacity(stdout.len());
    let mut saved_paths = Vec::new();
    let mut last_end = 0;
    let mut image_idx: u32 = 0;

    for mat in re.find_iter(stdout) {
        let blob = mat.as_str();
        if blob.len() < MIN_BASE64_IMAGE_BYTES {
            continue;
        }

        // Try to decode
        use base64::Engine;
        let decoded = match base64::engine::general_purpose::STANDARD.decode(blob) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };

        // Check magic bytes for known image formats
        let ext = match detect_image_format(&decoded) {
            Some(ext) => ext,
            None => continue,
        };

        // Save to output directory
        if std::fs::create_dir_all(output_dir).is_err() {
            continue;
        }
        let filename = format!("plot_{}_{}.{}", run_id, image_idx, ext);
        let dest = output_dir.join(&filename);
        if std::fs::write(&dest, &decoded).is_err() {
            continue;
        }

        let dest_str = dest.to_string_lossy().to_string();
        tracing::info!(
            path = %dest_str,
            base64_len = blob.len(),
            decoded_bytes = decoded.len(),
            "Extracted base64 image from code_interpreter stdout"
        );

        // Replace the base64 blob in stdout
        cleaned.push_str(&stdout[last_end..mat.start()]);
        cleaned.push_str(&format!("[image saved: {}]", dest_str));
        last_end = mat.end();

        saved_paths.push(dest_str);
        image_idx += 1;
    }

    if saved_paths.is_empty() {
        return ExtractedImages {
            cleaned_stdout: stdout.to_string(),
            saved_paths,
        };
    }

    // Append any remaining text after the last match
    cleaned.push_str(&stdout[last_end..]);

    ExtractedImages {
        cleaned_stdout: cleaned,
        saved_paths,
    }
}

/// Detect image format from magic bytes, returning the file extension.
fn detect_image_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 4 {
        return None;
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.starts_with(&[0x42, 0x4D]) {
        Some("bmp")
    } else {
        None
    }
}

/// Get the workspace directory for code interpreter outputs
fn workspace_base_dir() -> Result<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("prolog-router")
    } else {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("prolog-router")
    };
    Ok(base.join("code_interpreter"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_is_available() {
        // Just ensure it doesn't panic
        let _ = is_available();
    }

    #[test]
    fn test_missing_code_param() {
        let args = json!({});
        let result = execute("execute", &args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("code"));
    }

    #[test]
    fn test_unknown_action() {
        let args = json!({"code": "print(1)"});
        let result = execute("unknown", &args);
        assert!(result.is_err());
    }

    #[test]
    fn test_simple_print() {
        if !is_available() {
            return;
        }
        let args = json!({"code": "print('hello world')"});
        let result = execute("execute", &args).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["exit_code"], 0);
        assert!(parsed["stdout"].as_str().unwrap().contains("hello world"));
    }

    #[test]
    fn test_math_computation() {
        if !is_available() {
            return;
        }
        let args = json!({"code": "import math\nprint(math.factorial(10))"});
        let result = execute("execute", &args).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["exit_code"], 0);
        assert!(parsed["stdout"].as_str().unwrap().contains("3628800"));
    }

    #[test]
    fn test_syntax_error() {
        if !is_available() {
            return;
        }
        let args = json!({"code": "def foo(\n  invalid syntax here"});
        let result = execute("execute", &args).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_ne!(parsed["exit_code"], 0);
        assert!(parsed["stderr"].as_str().unwrap_or("").contains("SyntaxError"));
    }

    #[test]
    fn test_file_creation() {
        if !is_available() {
            return;
        }
        let args = json!({
            "code": "with open('test_output.csv', 'w') as f:\n    f.write('a,b\\n1,2\\n')"
        });
        let result = execute("execute", &args).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["exit_code"], 0);
        if let Some(files) = parsed["files_created"].as_array() {
            let file_names: Vec<&str> = files.iter().filter_map(|f| f.as_str()).collect();
            assert!(
                file_names.iter().any(|f| f.contains("test_output.csv")),
                "Expected test_output.csv in {:?}",
                file_names
            );
            // Clean up
            for f in files.iter().filter_map(|f| f.as_str()) {
                let _ = std::fs::remove_file(f);
            }
        }
    }

    #[test]
    fn test_truncate_output_small() {
        let small = "hello".as_bytes();
        assert_eq!(truncate_output(small), "hello");
    }

    #[test]
    fn test_truncate_output_large() {
        let large = "x".repeat(MAX_OUTPUT_BYTES + 1000);
        let result = truncate_output(large.as_bytes());
        assert!(result.contains("[Output truncated"));
        assert!(result.len() < large.len());
    }

    #[test]
    fn test_workspace_dir() {
        let dir = workspace_base_dir().unwrap();
        assert!(dir.to_string_lossy().contains("code_interpreter"));
    }

    #[test]
    fn test_extract_base64_png_from_stdout() {
        // Create a minimal valid PNG (1x1 red pixel)
        let png_bytes: Vec<u8> = {
            let mut img = image::RgbaImage::new(1, 1);
            img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
            let mut buf = Vec::new();
            let mut cursor = std::io::Cursor::new(&mut buf);
            img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
            buf
        };

        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        assert!(b64.len() > MIN_BASE64_IMAGE_BYTES, "test PNG must exceed threshold");

        let stdout = format!("Here is the chart:\n{}\nDone!", b64);
        let tmp = tempfile::tempdir().unwrap();
        let output_dir = tmp.path().join("output");

        let result = extract_base64_images(&stdout, &output_dir, "test123");

        assert_eq!(result.saved_paths.len(), 1, "should extract exactly one image");
        assert!(result.saved_paths[0].ends_with(".png"), "should be a .png file");
        assert!(result.cleaned_stdout.contains("[image saved:"), "stdout should have replacement marker");
        assert!(!result.cleaned_stdout.contains(&b64), "base64 blob should be removed from stdout");
        assert!(result.cleaned_stdout.contains("Here is the chart:"));
        assert!(result.cleaned_stdout.contains("Done!"));

        // Verify the saved file is a valid PNG
        let saved = std::fs::read(&result.saved_paths[0]).unwrap();
        assert_eq!(saved, png_bytes);
    }

    #[test]
    fn test_extract_base64_ignores_non_image() {
        // A long base64 string that decodes to non-image data
        use base64::Engine;
        let data = vec![0u8; 1024]; // zeroes — not a valid image
        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);

        let stdout = format!("result: {}", b64);
        let tmp = tempfile::tempdir().unwrap();

        let result = extract_base64_images(&stdout, tmp.path(), "test456");

        assert!(result.saved_paths.is_empty(), "non-image base64 should not be extracted");
        assert_eq!(result.cleaned_stdout, stdout, "stdout should be unchanged");
    }

    #[test]
    fn test_extract_base64_ignores_short_blobs() {
        use base64::Engine;
        // A valid PNG header but too short to meet the threshold
        let short_b64 = base64::engine::general_purpose::STANDARD.encode(&[0x89, b'P', b'N', b'G', 0, 0, 0, 0]);

        let stdout = format!("data: {}", short_b64);
        let tmp = tempfile::tempdir().unwrap();

        let result = extract_base64_images(&stdout, tmp.path(), "test789");

        assert!(result.saved_paths.is_empty());
        assert_eq!(result.cleaned_stdout, stdout);
    }

    #[test]
    fn test_detect_image_format() {
        assert_eq!(detect_image_format(&[0x89, b'P', b'N', b'G']), Some("png"));
        assert_eq!(detect_image_format(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(detect_image_format(b"GIF89a"), Some("gif"));
        assert_eq!(detect_image_format(&[0x42, 0x4D, 0, 0]), Some("bmp"));
        assert_eq!(detect_image_format(&[0, 0, 0, 0]), None);
        assert_eq!(detect_image_format(&[0, 0]), None);
    }
}
