//! Shared AppleScript utilities
//!
//! Provides script discovery and execution helpers used by multiple Apple
//! integration modules (Notes, Reminders, etc.).

use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

/// Override for the scripts directory, set by the Tauri app with its resource_dir.
static SCRIPTS_DIR_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Set the scripts directory override (called by the Tauri app at startup).
pub fn set_scripts_dir(dir: PathBuf) {
    let _ = SCRIPTS_DIR_OVERRIDE.set(dir);
}

/// Find the scripts directory, trying multiple locations.
pub fn find_scripts_dir() -> PathBuf {
    // Check override first (set by Tauri app with resource_dir)
    if let Some(dir) = SCRIPTS_DIR_OVERRIDE.get() {
        let scripts = dir.join("scripts");
        if scripts.exists() && scripts.is_dir() {
            return scripts;
        }
    }

    let candidates = [
        PathBuf::from("scripts"),
        PathBuf::from("../../scripts"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts"),
    ];

    for candidate in &candidates {
        if candidate.exists() && candidate.is_dir() {
            return candidate.clone();
        }
    }

    // Fall back to the first option (will fail with informative error)
    PathBuf::from("scripts")
}

/// Maximum time an AppleScript is allowed to run before being killed.
const APPLESCRIPT_TIMEOUT: Duration = Duration::from_secs(30);

/// Execute an AppleScript file and return raw output.
pub fn run_script(script_name: &str, args: &[&str]) -> Result<String> {
    let scripts_dir = find_scripts_dir();
    let script_path = scripts_dir.join(script_name);

    // Verify script exists
    if !script_path.exists() {
        return Err(anyhow!("Script not found: {}", script_path.display()));
    }

    tracing::info!(script = script_name, ?args, "├─ AppleScript executing");

    let child = Command::new("osascript")
        .arg(&script_path)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("Failed to execute osascript: {}", e))?;

    // Wait with timeout — AppleScript can hang when apps are unresponsive
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let result = child.wait_with_output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(APPLESCRIPT_TIMEOUT) {
        Ok(Ok(output)) => {
            let _ = waiter.join();
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::info!(script = script_name, %stderr, "│  └─ AppleScript failed");
                return Err(anyhow!("AppleScript error: {}", stderr));
            }

            tracing::info!(script = script_name, output_bytes = output.stdout.len(), "│  └─ AppleScript completed");
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(Err(e)) => {
            let _ = waiter.join();
            Err(anyhow!("Failed waiting for osascript: {}", e))
        }
        Err(_timeout) => {
            // Kill the hung osascript process by PID, then reap the thread
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
            let _ = waiter.join();
            tracing::error!(script = script_name, timeout_secs = APPLESCRIPT_TIMEOUT.as_secs(),
                "│  └─ AppleScript timed out, killed");
            Err(anyhow!("AppleScript timed out after {}s", APPLESCRIPT_TIMEOUT.as_secs()))
        }
    }
}
