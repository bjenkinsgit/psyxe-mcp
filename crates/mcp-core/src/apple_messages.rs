//! Apple Messages (iMessage) integration.
//!
//! Send messages via AppleScript and receive replies by polling `chat.db`.
//! Requires Automation permission for Messages.app (send) and Full Disk
//! Access (receive — reads `~/Library/Messages/chat.db`).

use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::process::Command;

use crate::access_store::AccessStore;

// ============================================================================
// Allowed Recipients
// ============================================================================

/// Check if a recipient is in the allowed set.
pub fn is_recipient_allowed(recipient: &str) -> bool {
    AccessStore::is_recipient_allowed(recipient)
}

/// List enabled (allowed) recipients.
pub fn enabled_recipients() -> Vec<String> {
    AccessStore::recipient_enabled_ids()
}

// ============================================================================
// Send
// ============================================================================

/// Send an iMessage to a recipient via AppleScript.
///
/// `recipient` is a phone number (e.g., "+15551234567") or email address.
pub fn send_message(recipient: &str, text: &str) -> Result<String> {
    if recipient.trim().is_empty() {
        return Err(anyhow!("Recipient cannot be empty"));
    }
    if text.trim().is_empty() {
        return Err(anyhow!("Message text cannot be empty"));
    }

    if !is_recipient_allowed(recipient) {
        return Err(anyhow!(
            "Recipient '{}' is not in the allowed list. Grant access first.",
            recipient
        ));
    }

    // Find the AppleScript
    let script_path = find_script("messages_send.applescript")?;

    tracing::info!(recipient = recipient, "Sending iMessage via AppleScript");

    let output = Command::new("osascript")
        .arg(&script_path)
        .arg(recipient)
        .arg(text)
        .output()
        .map_err(|e| anyhow!("Failed to execute osascript: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if stdout.starts_with("ERROR:") {
        return Err(anyhow!("{}", stdout));
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("AppleScript error: {}", stderr));
    }

    Ok(stdout)
}

// ============================================================================
// Receive (poll chat.db)
// ============================================================================

/// Path to the Messages SQLite database.
fn chat_db_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    home.join("Library/Messages/chat.db")
}

/// Wait for a reply from a recipient by polling `chat.db`.
///
/// Polls every 3 seconds until a new inbound message is found or `timeout_secs`
/// expires. Returns the message text or an error on timeout.
///
/// On macOS Ventura+ (13.0+), message bodies may be stored as binary data
/// in the `attributedBody` column instead of plain text in `text`. This
/// function handles both formats.
pub fn wait_for_reply(recipient: &str, timeout_secs: u64) -> Result<String> {
    if recipient.trim().is_empty() {
        return Err(anyhow!("Recipient cannot be empty"));
    }

    if !is_recipient_allowed(recipient) {
        return Err(anyhow!(
            "Recipient '{}' is not in the allowed list. Grant access first.",
            recipient
        ));
    }

    let db_path = chat_db_path();
    if !db_path.exists() {
        return Err(anyhow!(
            "Messages database not found at {}. Full Disk Access may be required.",
            db_path.display()
        ));
    }

    // Get the latest message ROWID from this recipient before we start waiting
    let baseline_rowid = get_latest_inbound_rowid(&db_path, recipient)?;

    tracing::info!(
        recipient = recipient,
        timeout_secs = timeout_secs,
        baseline_rowid = baseline_rowid,
        "Waiting for iMessage reply"
    );

    let poll_interval = std::time::Duration::from_secs(3);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    while std::time::Instant::now() < deadline {
        std::thread::sleep(poll_interval);

        match get_new_inbound_message(&db_path, recipient, baseline_rowid) {
            Ok(Some(text)) => {
                tracing::info!(recipient = recipient, "Received iMessage reply");
                return Ok(text);
            }
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(error = %e, "Error polling chat.db, retrying...");
                continue;
            }
        }
    }

    Err(anyhow!(
        "Timeout after {}s waiting for reply from {}",
        timeout_secs,
        recipient
    ))
}

/// Get the ROWID of the latest inbound message from a recipient.
fn get_latest_inbound_rowid(db_path: &PathBuf, recipient: &str) -> Result<i64> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| anyhow!("Cannot open chat.db (Full Disk Access required?): {}", e))?;

    let rowid: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(m.ROWID), 0)
             FROM message m
             JOIN chat_message_join cmj ON cmj.message_id = m.ROWID
             JOIN chat c ON c.ROWID = cmj.chat_id
             WHERE m.is_from_me = 0
               AND c.chat_identifier LIKE ?1",
            [format!("%{}", normalize_recipient(recipient))],
            |row| row.get(0),
        )
        .unwrap_or(0);

    Ok(rowid)
}

/// Check for a new inbound message after the baseline ROWID.
fn get_new_inbound_message(
    db_path: &PathBuf,
    recipient: &str,
    after_rowid: i64,
) -> Result<Option<String>> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| anyhow!("Cannot open chat.db: {}", e))?;

    // Try plain text column first, then attributedBody for Ventura+
    let result: Option<(Option<String>, Option<Vec<u8>>)> = conn
        .query_row(
            "SELECT m.text, m.attributedBody
             FROM message m
             JOIN chat_message_join cmj ON cmj.message_id = m.ROWID
             JOIN chat c ON c.ROWID = cmj.chat_id
             WHERE m.is_from_me = 0
               AND m.ROWID > ?1
               AND c.chat_identifier LIKE ?2
             ORDER BY m.ROWID ASC
             LIMIT 1",
            rusqlite::params![after_rowid, format!("%{}", normalize_recipient(recipient))],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();

    match result {
        Some((Some(text), _)) if !text.is_empty() => Ok(Some(text)),
        Some((_, Some(blob))) if !blob.is_empty() => {
            // Ventura+ stores body in attributedBody as NSAttributedString archive.
            // Extract plain text from the binary blob.
            Ok(Some(extract_text_from_attributed_body(&blob)))
        }
        Some(_) => Ok(None),
        None => Ok(None),
    }
}

/// Extract plain text from an NSAttributedString archived blob.
///
/// On macOS Ventura+, Messages stores the body in `attributedBody` as a
/// binary NSKeyedArchiver plist. The plain text is embedded between
/// `NSString` markers. This is a best-effort extraction without depending
/// on Cocoa frameworks.
fn extract_text_from_attributed_body(blob: &[u8]) -> String {
    // The text content is typically between "NSString" and the next control
    // sequence. Look for the streamTypedString pattern.
    // Common marker: bytes [0x01] followed by the actual text, then [0x86]
    // or other control bytes.
    //
    // Heuristic: find the last occurrence of "+NSString" or "NSMutableString"
    // and extract printable text after it.

    // Search for the NSString marker in raw bytes (not decoded string, to avoid
    // position mismatches from multi-byte UTF-8 replacement characters).
    let ns_string_marker = b"NSString";
    let mut search_pos = blob.len();
    // rfind the last occurrence of "NSString" in raw bytes
    while search_pos >= ns_string_marker.len() {
        search_pos -= 1;
        if blob[search_pos..].starts_with(ns_string_marker) {
            let after = &blob[search_pos + ns_string_marker.len()..];
            // Skip non-printable header bytes until we reach the text content.
            // The pattern is: NSString <control bytes> <length-prefix> <actual text> <0x86>
            // The length-prefix is typically 2 bytes (e.g. "+," or "+:").
            // Find the first alphabetic or common text-start character.
            let text_start = after
                .iter()
                .position(|&b| b.is_ascii_alphabetic() || b == b'@' || b == b'/' || b == b'"')
                .unwrap_or(after.len());
            if text_start >= after.len() {
                continue;
            }
            let text_bytes: Vec<u8> = after[text_start..]
                .iter()
                .take_while(|&&b| b >= 0x20 && b != 0x86 && b != 0x84)
                .copied()
                .collect();
            let extracted = String::from_utf8_lossy(&text_bytes).trim().to_string();
            if !extracted.is_empty() {
                return extracted;
            }
        }
    }

    // Fallback: extract longest run of printable ASCII
    let mut best = String::new();
    let mut current = String::new();
    for &b in blob {
        if b >= 0x20 && b < 0x7F {
            current.push(b as char);
        } else {
            if current.len() > best.len() {
                best = current.clone();
            }
            current.clear();
        }
    }
    if current.len() > best.len() {
        best = current;
    }

    best.trim().to_string()
}

/// Normalize a recipient identifier for matching in chat.db.
/// Strips leading "+" for phone numbers to match iMessage chat_identifier format.
fn normalize_recipient(recipient: &str) -> String {
    recipient.trim().replace(' ', "").replace('-', "")
}

// ============================================================================
// Script Finder
// ============================================================================

fn find_script(name: &str) -> Result<PathBuf> {
    let candidates = vec![
        PathBuf::from("scripts").join(name),
        PathBuf::from("../../scripts").join(name),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts")
            .join(name),
    ];

    candidates
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| anyhow!("Script not found: {}", name))
}

// ============================================================================
// Inbound Polling (for background poller)
// ============================================================================

/// An inbound iMessage from chat.db.
#[derive(Debug, Clone)]
pub struct InboundIMessage {
    pub rowid: i64,
    pub chat_identifier: String, // phone number or email
    pub text: String,
    pub date: i64,
    /// "iMessage" or "SMS" — determines which service to use for replies
    pub service: String,
}

/// Get the maximum ROWID of inbound messages in chat.db.
///
/// Used to establish a baseline before starting the poll loop.
pub fn get_max_inbound_rowid() -> Result<i64> {
    let db_path = chat_db_path();
    if !db_path.exists() {
        return Err(anyhow!(
            "Messages database not found at {}. Full Disk Access may be required.",
            db_path.display()
        ));
    }

    let conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| anyhow!("Cannot open chat.db (Full Disk Access required?): {}", e))?;

    let rowid: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(m.ROWID), 0)
             FROM message m
             WHERE m.is_from_me = 0",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    Ok(rowid)
}

/// Poll for new inbound messages after the given ROWID.
///
/// Joins through `chat_message_join` and `chat` to get the `chat_identifier`.
/// Filters out group chats (style = 43). Handles both `text` and
/// `attributedBody` columns (Ventura+).
pub fn poll_new_inbound_messages(after_rowid: i64) -> Result<Vec<InboundIMessage>> {
    let db_path = chat_db_path();
    if !db_path.exists() {
        return Err(anyhow!(
            "Messages database not found at {}. Full Disk Access may be required.",
            db_path.display()
        ));
    }

    let conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| anyhow!("Cannot open chat.db: {}", e))?;

    let mut stmt = conn
        .prepare(
            "SELECT m.ROWID, c.chat_identifier, m.text, m.attributedBody, m.date, c.service_name
             FROM message m
             JOIN chat_message_join cmj ON cmj.message_id = m.ROWID
             JOIN chat c ON c.ROWID = cmj.chat_id
             WHERE m.is_from_me = 0
               AND m.ROWID > ?1
               AND c.style != 43
             ORDER BY m.ROWID ASC",
        )
        .map_err(|e| anyhow!("Failed to prepare poll query: {}", e))?;

    let rows = stmt
        .query_map(rusqlite::params![after_rowid], |row| {
            let rowid: i64 = row.get(0)?;
            let chat_identifier: String = row.get(1)?;
            let text: Option<String> = row.get(2)?;
            let attributed_body: Option<Vec<u8>> = row.get(3)?;
            let date: i64 = row.get(4)?;
            let service: String = row.get::<_, String>(5).unwrap_or_else(|_| "iMessage".to_string());
            Ok((rowid, chat_identifier, text, attributed_body, date, service))
        })
        .map_err(|e| anyhow!("Failed to query new messages: {}", e))?;

    let mut messages = Vec::new();
    for row in rows {
        let (rowid, chat_identifier, text, attributed_body, date, service) =
            row.map_err(|e| anyhow!("Row read error: {}", e))?;

        // Extract text from either column
        let body = match text {
            Some(ref t) if !t.is_empty() => t.clone(),
            _ => match attributed_body {
                Some(ref blob) if !blob.is_empty() => {
                    extract_text_from_attributed_body(blob)
                }
                _ => continue, // No text content, skip
            },
        };

        if body.trim().is_empty() {
            continue;
        }

        messages.push(InboundIMessage {
            rowid,
            chat_identifier,
            text: body,
            date,
            service,
        });
    }

    Ok(messages)
}

/// Send an iMessage WITHOUT the allowed-recipient check.
///
/// Used by the background poller to reply to senders who already passed the
/// contact group allowlist. Callers are responsible for their own authorization.
pub fn send_message_internal(recipient: &str, text: &str, service: Option<&str>) -> Result<String> {
    if recipient.trim().is_empty() {
        return Err(anyhow!("Recipient cannot be empty"));
    }
    if text.trim().is_empty() {
        return Err(anyhow!("Message text cannot be empty"));
    }

    let script_path = find_script("messages_send.applescript")?;
    let svc = service.unwrap_or("iMessage");

    tracing::info!(recipient = recipient, service = svc, "Sending message reply (internal)");

    let output = Command::new("osascript")
        .arg(&script_path)
        .arg(recipient)
        .arg(text)
        .arg(svc)
        .output()
        .map_err(|e| anyhow!("Failed to execute osascript: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if stdout.starts_with("ERROR:") {
        return Err(anyhow!("{}", stdout));
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("AppleScript error: {}", stderr));
    }

    Ok(stdout)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_recipient() {
        assert_eq!(normalize_recipient("+1 555-123-4567"), "+15551234567");
        assert_eq!(normalize_recipient("user@example.com"), "user@example.com");
    }

    #[test]
    fn test_extract_text_from_attributed_body_fallback() {
        // Simple test: embedded text in binary blob
        let blob = b"\x00\x01NSString\x05Hello world\x86\x00";
        let text = extract_text_from_attributed_body(blob);
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn test_extract_text_fallback_printable() {
        // No NSString marker — should find longest printable run
        let blob = b"\x00\x01\x02This is the message\x00\x01\x02short\x00";
        let text = extract_text_from_attributed_body(blob);
        assert_eq!(text, "This is the message");
    }

    #[test]
    fn test_send_message_empty_recipient() {
        let result = send_message("", "hello");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_send_message_empty_text() {
        let result = send_message("+15551234567", "");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }
}
