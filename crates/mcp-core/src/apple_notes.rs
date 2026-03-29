//! Apple Notes Integration via AppleScript
//!
//! Provides search, list, and retrieval of Apple Notes using external AppleScript files.
//! Uses a delimiter-based parsing protocol for reliable cross-language communication.
//!
//! Includes a tag indexing system that caches note metadata and extracted hashtags
//! for fast tag-based queries without rescanning all notes.

use anyhow::{anyhow, Result};
use flate2::read::GzDecoder;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Read as _;
use std::path::PathBuf;

// Re-export set_scripts_dir from applescript_utils for backward compatibility
pub use crate::applescript_utils::set_scripts_dir;
use crate::applescript_utils::{find_scripts_dir, run_script};

// ============================================================================
// Minimal Protobuf Parser for Apple Notes TTArchive
// ============================================================================

/// Minimal protobuf wire-format parser — just enough to extract plaintext from
/// Apple Notes' gzipped TTArchive (ZICNOTEDATA.ZDATA).
///
/// Path: root → field 2 (Document) → field 3 (Note) → field 2 (string).
/// Stable since iOS 9 / macOS 10.11.
mod proto {
    /// Read a varint starting at `pos`. Returns (value, bytes_consumed).
    pub fn read_varint(data: &[u8], pos: usize) -> Option<(u64, usize)> {
        let mut value: u64 = 0;
        let mut shift = 0;
        let mut i = pos;
        loop {
            if i >= data.len() {
                return None;
            }
            let byte = data[i];
            value |= ((byte & 0x7F) as u64) << shift;
            i += 1;
            if byte & 0x80 == 0 {
                return Some((value, i - pos));
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
    }

    /// Scan `data` for the first length-delimited (wire type 2) field with the
    /// given field number. Returns the field's payload as a byte slice.
    pub fn find_field<'a>(data: &'a [u8], target_field: u32) -> Option<&'a [u8]> {
        let mut pos = 0;
        while pos < data.len() {
            let (tag, tag_len) = read_varint(data, pos)?;
            pos += tag_len;
            let field_number = (tag >> 3) as u32;
            let wire_type = (tag & 0x07) as u8;

            match wire_type {
                0 => {
                    // Varint — skip
                    let (_, vlen) = read_varint(data, pos)?;
                    pos += vlen;
                }
                1 => {
                    // 64-bit fixed — skip 8 bytes
                    pos += 8;
                }
                2 => {
                    // Length-delimited
                    let (len, len_bytes) = read_varint(data, pos)?;
                    pos += len_bytes;
                    let len = len as usize;
                    if pos + len > data.len() {
                        return None;
                    }
                    if field_number == target_field {
                        return Some(&data[pos..pos + len]);
                    }
                    pos += len;
                }
                5 => {
                    // 32-bit fixed — skip 4 bytes
                    pos += 4;
                }
                _ => return None, // Unknown wire type
            }
        }
        None
    }

    /// Navigate field 2 → field 3 → field 2 to extract the plaintext string.
    pub fn extract_plaintext(data: &[u8]) -> Option<String> {
        let doc = find_field(data, 2)?;
        let note = find_field(doc, 3)?;
        let text_bytes = find_field(note, 2)?;
        String::from_utf8(text_bytes.to_vec()).ok()
    }
}

// ============================================================================
// Shared SQLite Helpers
// ============================================================================

/// Open the NoteStore SQLite database in read-only mode.
fn open_notestore_db() -> Result<Connection> {
    let db_path = notestore_db_path()
        .ok_or_else(|| anyhow!("NoteStore.sqlite not found"))?;
    Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| anyhow!("Failed to open NoteStore.sqlite: {}", e))
}

/// Extract the integer Z_PK from an x-coredata:// note ID.
///
/// Format: `x-coredata://UUID/ICNote/p{Z_PK}`
pub fn parse_zpk_from_id(note_id: &str) -> Result<i64> {
    note_id
        .rsplit('/')
        .next()
        .and_then(|s| s.strip_prefix('p'))
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or_else(|| anyhow!("Invalid note ID format (expected x-coredata://…/ICNote/p<N>): {}", note_id))
}

/// Gzip-decompress `zdata`, then extract plaintext via the protobuf parser.
/// Strips U+FFFC (object replacement character used for attachments).
pub fn decompress_and_extract(zdata: &[u8]) -> Result<String> {
    let mut decoder = GzDecoder::new(zdata);
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| anyhow!("Gzip decompression failed: {}", e))?;

    let text = proto::extract_plaintext(&decompressed)
        .ok_or_else(|| anyhow!("Failed to extract plaintext from protobuf"))?;

    // Strip U+FFFC (object replacement character used for inline attachments)
    Ok(text.replace('\u{FFFC}', ""))
}


/// Default path for the notes index cache file
fn default_index_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("apple_notes_index.json")
}

// ============================================================================
// Data Structures
// ============================================================================

/// A note record parsed from AppleScript output
#[derive(Debug, Serialize)]
pub struct NoteRecord {
    pub id: String,
    pub title: String,
    pub folder: String,
    pub modified: String,
    pub snippet: String,
    /// Command to open this note in Notes.app
    pub open_cmd: String,
}

/// Full note content (includes body)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteContent {
    pub id: String,
    pub title: String,
    pub folder: String,
    pub modified: String,
    pub body: String,
    /// Command to open this note in Notes.app
    pub open_cmd: String,
}

// ============================================================================
// Tag Index Data Structures
// ============================================================================

/// Indexed note metadata (stored in cache)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedNote {
    pub id: String,
    pub title: String,
    pub folder: String,
    pub modified: String,
    pub tags: Vec<String>,
}

/// The full notes index (persisted to disk)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotesIndex {
    /// Number of notes when index was built (for staleness check)
    pub note_count: usize,
    /// ISO 8601 timestamp when index was last updated
    pub last_updated: String,
    /// Map from tag -> list of note IDs
    pub tags: HashMap<String, Vec<String>>,
    /// Map from note ID -> indexed note metadata
    pub notes: HashMap<String, IndexedNote>,
}

// ============================================================================
// Availability Check
// ============================================================================

/// Check if Apple Notes scripts are available (macOS only)
pub fn is_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        let scripts_dir = find_scripts_dir();
        scripts_dir.join("notes_search.applescript").exists()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Check if a note exists by ID (lightweight SQLite check).
/// Returns true if the note exists, false if it was deleted or not found.
pub fn note_exists(note_id: &str) -> bool {
    let zpk = match parse_zpk_from_id(note_id) {
        Ok(pk) => pk,
        Err(_) => return false,
    };
    let conn = match open_notestore_db() {
        Ok(c) => c,
        Err(_) => return false,
    };
    conn.query_row(
        "SELECT 1 FROM ZICCLOUDSYNCINGOBJECT WHERE Z_PK = ?1 AND ZTITLE1 IS NOT NULL",
        [zpk],
        |_| Ok(()),
    )
    .is_ok()
}

// ============================================================================
// Output Parsing
// ============================================================================

/// Parse delimiter-based output into NoteRecords
///
/// Expected format:
/// ```text
/// RECORD_START
/// id: x-coredata://...
/// title: Note Title
/// folder: Folder Name
/// modified: 2026-01-27T10:30:00Z
/// snippet: First 200 characters...
/// RECORD_END
/// ```
fn parse_records(output: &str) -> Result<Vec<NoteRecord>> {
    let mut records = Vec::new();
    let mut current: Option<NoteRecord> = None;

    for line in output.lines() {
        let line = line.trim();

        if line == "RECORD_START" {
            current = Some(NoteRecord {
                id: String::new(),
                title: String::new(),
                folder: String::new(),
                modified: String::new(),
                snippet: String::new(),
                open_cmd: String::new(),
            });
        } else if line == "RECORD_END" {
            if let Some(mut record) = current.take() {
                // Generate command to open this note
                if !record.id.is_empty() {
                    record.open_cmd =
                        format!("osascript scripts/notes_open.applescript \"{}\"", record.id);
                }
                records.push(record);
            }
        } else if line.starts_with("ERROR:") {
            return Err(anyhow!("{}", line));
        } else if let Some(ref mut record) = current {
            if let Some((key, value)) = line.split_once(": ") {
                match key {
                    "id" => record.id = value.to_string(),
                    "title" => record.title = value.to_string(),
                    "folder" => record.folder = value.to_string(),
                    "modified" => record.modified = value.to_string(),
                    "snippet" => record.snippet = value.to_string(),
                    _ => {}
                }
            }
        }
    }

    Ok(records)
}

/// Parse full note content from AppleScript output
fn parse_note_content(output: &str) -> Result<NoteContent> {
    let mut note = NoteContent {
        id: String::new(),
        title: String::new(),
        folder: String::new(),
        modified: String::new(),
        body: String::new(),
        open_cmd: String::new(),
    };

    let mut in_body = false;
    let mut body_lines = Vec::new();

    for line in output.lines() {
        let line_trimmed = line.trim();

        if line_trimmed.starts_with("ERROR:") {
            return Err(anyhow!("{}", line_trimmed));
        }

        if in_body {
            if line_trimmed == "BODY_END" {
                in_body = false;
                note.body = body_lines.join("\n");
            } else {
                body_lines.push(line.to_string());
            }
        } else if line_trimmed == "BODY_START" {
            in_body = true;
        } else if let Some((key, value)) = line_trimmed.split_once(": ") {
            match key {
                "id" => note.id = value.to_string(),
                "title" => note.title = value.to_string(),
                "folder" => note.folder = value.to_string(),
                "modified" => note.modified = value.to_string(),
                _ => {}
            }
        }
    }

    if note.id.is_empty() {
        return Err(anyhow!("Failed to parse note content"));
    }

    // Generate command to open this note
    note.open_cmd = format!("osascript scripts/notes_open.applescript \"{}\"", note.id);

    Ok(note)
}

// ============================================================================
// Public API
// ============================================================================

/// Search notes by query string.
/// Returns compact JSON (title + snippet) to minimize LLM context token usage.
pub fn search_notes(query: &str, folder: Option<&str>) -> Result<String> {
    let args: Vec<&str> = match folder {
        Some(f) => vec![query, f],
        None => vec![query],
    };

    let output = run_script("notes_search.applescript", &args)?;
    let records = parse_records(&output)?;

    Ok(serde_json::to_string(&json!({
        "count": records.len(),
        "results": records.iter().map(|r| json!({
            "note_id": r.id,
            "title": r.title,
            "folder": r.folder,
            "snippet": r.snippet,
        })).collect::<Vec<_>>()
    }))?)
}

/// List notes, optionally filtered by folder
pub fn list_notes(folder: Option<&str>) -> Result<String> {
    let args: Vec<&str> = folder.map(|f| vec![f]).unwrap_or_default();

    let output = run_script("notes_list.applescript", &args)?;
    let records = parse_records(&output)?;

    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "count": records.len(),
        "notes": records
    }))?)
}

/// Get full note content by ID (SQLite fast path with AppleScript fallback).
pub fn get_note(note_id: &str) -> Result<String> {
    let note = get_note_content(note_id)?;
    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "note": note
    }))?)
}

/// Retrieve a single note's content via direct SQLite access.
///
/// Returns a `NoteContent` with body text extracted from the gzipped protobuf
/// stored in ZICNOTEDATA.ZDATA. Falls back to AppleScript if SQLite fails.
pub fn get_note_content(note_id: &str) -> Result<NoteContent> {
    match get_note_content_sqlite(note_id) {
        Ok(note) => {
            tracing::info!(note_id, body_len = note.body.len(), "├─ Note retrieved via SQLite");
            Ok(note)
        }
        Err(e) => {
            tracing::warn!(note_id, error = %e, "SQLite body extraction failed, falling back to AppleScript");
            let output = run_script("notes_get.applescript", &[note_id])?;
            parse_note_content(&output)
        }
    }
}

/// Direct SQLite implementation for retrieving a single note's full content.
fn get_note_content_sqlite(note_id: &str) -> Result<NoteContent> {
    let zpk = parse_zpk_from_id(note_id)?;
    let conn = open_notestore_db()?;

    let row: (String, String, Option<String>, Option<Vec<u8>>, Option<Vec<u8>>) = conn
        .query_row(
            "SELECT c1.ZTITLE1, \
                    datetime(c1.ZMODIFICATIONDATE1 + 978307200, 'unixepoch'), \
                    c2.ZTITLE2, \
                    nd.ZDATA, \
                    c1.ZCRYPTOINITIALIZATIONVECTOR \
             FROM ZICCLOUDSYNCINGOBJECT c1 \
             LEFT JOIN ZICCLOUDSYNCINGOBJECT c2 ON c2.Z_PK = c1.ZFOLDER \
             LEFT JOIN ZICNOTEDATA nd ON nd.ZNOTE = c1.Z_PK \
             WHERE c1.Z_PK = ?1 AND c1.ZTITLE1 IS NOT NULL",
            [zpk],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(|e| anyhow!("Note not found (Z_PK={}): {}", zpk, e))?;

    let (title, modified, folder, zdata, crypto_iv) = row;

    if crypto_iv.is_some() {
        return Err(anyhow!("Note is encrypted and cannot be read directly"));
    }

    let body = match zdata {
        Some(data) => decompress_and_extract(&data)?,
        None => String::new(),
    };

    Ok(NoteContent {
        id: note_id.to_string(),
        title,
        folder: folder.unwrap_or_else(|| "Notes".to_string()),
        modified,
        body,
        open_cmd: format!("osascript scripts/notes_open.applescript \"{}\"", note_id),
    })
}

/// Batch-fetch note contents via direct SQLite access.
///
/// Processes in chunks of 500 (SQLite parameter limit is 999).
/// Silently skips encrypted notes and null ZDATA (with debug logging).
pub fn get_notes_batch_sqlite(note_ids: &[String]) -> Result<HashMap<String, NoteContent>> {
    let conn = open_notestore_db()?;
    let uuid = get_coredata_uuid()?;
    let mut results = HashMap::new();

    // Build a Z_PK → note_id lookup
    let mut zpk_to_id: HashMap<i64, String> = HashMap::new();
    for id in note_ids {
        if let Ok(zpk) = parse_zpk_from_id(id) {
            zpk_to_id.insert(zpk, id.clone());
        }
    }

    // Process in chunks of 500
    let zpks: Vec<i64> = zpk_to_id.keys().copied().collect();
    for chunk in zpks.chunks(500) {
        let placeholders: String = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT c1.Z_PK, c1.ZTITLE1, \
                    datetime(c1.ZMODIFICATIONDATE1 + 978307200, 'unixepoch'), \
                    c2.ZTITLE2, \
                    nd.ZDATA, \
                    c1.ZCRYPTOINITIALIZATIONVECTOR \
             FROM ZICCLOUDSYNCINGOBJECT c1 \
             LEFT JOIN ZICCLOUDSYNCINGOBJECT c2 ON c2.Z_PK = c1.ZFOLDER \
             LEFT JOIN ZICNOTEDATA nd ON nd.ZNOTE = c1.Z_PK \
             WHERE c1.Z_PK IN ({}) AND c1.ZTITLE1 IS NOT NULL",
            placeholders
        );

        let params: Vec<Box<dyn rusqlite::types::ToSql>> =
            chunk.iter().map(|pk| Box::new(*pk) as Box<dyn rusqlite::types::ToSql>).collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn.prepare(&query)
            .map_err(|e| anyhow!("Failed to prepare batch query: {}", e))?;

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                let zpk: i64 = row.get(0)?;
                let title: String = row.get(1)?;
                let modified: String = row.get(2)?;
                let folder: Option<String> = row.get(3)?;
                let zdata: Option<Vec<u8>> = row.get(4)?;
                let crypto_iv: Option<Vec<u8>> = row.get(5)?;
                Ok((zpk, title, modified, folder, zdata, crypto_iv))
            })
            .map_err(|e| anyhow!("Failed to query batch: {}", e))?;

        for row in rows {
            let (zpk, title, modified, folder, zdata, crypto_iv) = match row {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!(error = %e, "Skipping row with error");
                    continue;
                }
            };

            if crypto_iv.is_some() {
                tracing::debug!(zpk, "Skipping encrypted note");
                continue;
            }

            let body = match zdata {
                Some(data) => match decompress_and_extract(&data) {
                    Ok(text) => text,
                    Err(e) => {
                        tracing::debug!(zpk, error = %e, "Failed to extract body, skipping");
                        continue;
                    }
                },
                None => {
                    tracing::debug!(zpk, "No ZDATA, skipping");
                    continue;
                }
            };

            let note_id = zpk_to_id
                .get(&zpk)
                .cloned()
                .unwrap_or_else(|| format!("x-coredata://{}/ICNote/p{}", uuid, zpk));

            results.insert(
                note_id.clone(),
                NoteContent {
                    id: note_id.clone(),
                    title,
                    folder: folder.unwrap_or_else(|| "Notes".to_string()),
                    modified,
                    body,
                    open_cmd: format!("osascript scripts/notes_open.applescript \"{}\"", note_id),
                },
            );
        }
    }

    Ok(results)
}

/// Open a note in Notes.app by ID
pub fn open_note(note_id: &str) -> Result<String> {
    let output = run_script("notes_open.applescript", &[note_id])?;

    // Parse the result - expects "OK: Opened note: <title>" or "ERROR: <message>"
    let output = output.trim();
    if output.starts_with("ERROR:") {
        return Err(anyhow!("{}", output));
    }

    // Extract note title from success message
    let title = output
        .strip_prefix("OK: Opened note: ")
        .unwrap_or("Unknown");

    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "message": format!("Opened note '{}' in Notes.app", title),
        "note_id": note_id
    }))?)
}

/// Create a new note in Apple Notes via AppleScript.
pub fn create_note(title: &str, body: &str, folder: Option<&str>) -> Result<String> {
    let mut args = vec![title, body];
    if let Some(f) = folder {
        args.push(f);
    }
    let output = run_script("notes_create.applescript", &args)?;
    let output = output.trim();

    if output.starts_with("ERROR:") {
        return Err(anyhow!("{}", output));
    }

    // Parse RECORD_START/RECORD_END response
    let mut id = String::new();
    let mut result_folder = String::new();
    for line in output.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("id: ") {
            id = val.to_string();
        } else if let Some(val) = line.strip_prefix("folder: ") {
            result_folder = val.to_string();
        }
    }

    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "id": id,
        "title": title,
        "folder": result_folder,
        "message": format!("Created note '{}' in folder '{}'", title, result_folder)
    }))?)
}

// ============================================================================
// Tag Index Functions
// ============================================================================

/// Load the notes index from disk
pub fn load_index() -> Result<NotesIndex> {
    let path = default_index_path();
    if !path.exists() {
        return Err(anyhow!("Index not found. Run 'notes_index' to build it."));
    }

    let content = fs::read_to_string(&path)
        .map_err(|e| anyhow!("Failed to read index file: {}", e))?;

    serde_json::from_str(&content)
        .map_err(|e| anyhow!("Failed to parse index file: {}", e))
}

/// Save the notes index to disk
fn save_index(index: &NotesIndex) -> Result<()> {
    let path = default_index_path();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Failed to create cache directory: {}", e))?;
    }

    let content = serde_json::to_string_pretty(index)
        .map_err(|e| anyhow!("Failed to serialize index: {}", e))?;

    fs::write(&path, content)
        .map_err(|e| anyhow!("Failed to write index file: {}", e))?;

    Ok(())
}

/// Get current note count (SQLite first, AppleScript fallback)
pub fn get_note_count() -> Result<usize> {
    // Try SQLite first (instant)
    if let Ok(count) = get_note_count_sqlite() {
        return Ok(count);
    }

    // Fall back to AppleScript
    let output = run_script("notes_count.applescript", &[])?;

    for line in output.lines() {
        if let Some(count_str) = line.strip_prefix("COUNT: ") {
            return count_str
                .trim()
                .parse()
                .map_err(|e| anyhow!("Failed to parse note count: {}", e));
        }
        if line.starts_with("ERROR:") {
            return Err(anyhow!("{}", line));
        }
    }

    Err(anyhow!("Failed to get note count"))
}

/// Get the most recent modification date across all notes (ISO 8601 string)
pub fn get_latest_modified() -> Result<String> {
    let output = run_script("notes_latest_modified.applescript", &[])?;

    for line in output.lines() {
        if let Some(ts) = line.strip_prefix("LATEST: ") {
            return Ok(ts.trim().to_string());
        }
        if line.starts_with("ERROR:") {
            return Err(anyhow!("{}", line));
        }
    }

    Err(anyhow!("Failed to get latest modification date"))
}

/// Check if a tag is a CSS hex color code (e.g., #fff, #ffffff, #rrggbbaa)
#[cfg(test)]
fn is_css_color_code(tag: &str) -> bool {
    let tag = tag.strip_prefix('#').unwrap_or(tag);
    let len = tag.len();

    // CSS color codes are 3, 6, or 8 hex digits
    if len != 3 && len != 6 && len != 8 {
        return false;
    }

    // All characters must be hex digits
    tag.chars().all(|c| c.is_ascii_hexdigit())
}

/// Parse the output from notes_index_build.applescript (retained for tests)
#[cfg(test)]
fn parse_index_output(output: &str) -> Result<(usize, Vec<IndexedNote>)> {
    let mut note_count = 0;
    let mut notes = Vec::new();
    let mut current: Option<IndexedNote> = None;

    for line in output.lines() {
        let line = line.trim();

        if let Some(count_str) = line.strip_prefix("NOTE_COUNT: ") {
            note_count = count_str.trim().parse().unwrap_or(0);
        } else if line == "RECORD_START" {
            current = Some(IndexedNote {
                id: String::new(),
                title: String::new(),
                folder: String::new(),
                modified: String::new(),
                tags: Vec::new(),
            });
        } else if line == "RECORD_END" {
            if let Some(note) = current.take() {
                notes.push(note);
            }
        } else if line.starts_with("ERROR:") {
            return Err(anyhow!("{}", line));
        } else if let Some(ref mut note) = current {
            if let Some((key, value)) = line.split_once(": ") {
                match key {
                    "id" => note.id = value.to_string(),
                    "title" => note.title = value.to_string(),
                    "folder" => note.folder = value.to_string(),
                    "modified" => note.modified = value.to_string(),
                    "tags" => {
                        note.tags = value
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty() && !is_css_color_code(s))
                            .collect();
                    }
                    _ => {}
                }
            }
        }
    }

    Ok((note_count, notes))
}

/// Get the path to the NoteStore SQLite database.
fn notestore_db_path() -> Option<PathBuf> {
    let path = dirs::home_dir()?
        .join("Library/Group Containers/group.com.apple.notes/NoteStore.sqlite");
    if path.exists() { Some(path) } else { None }
}

/// Read native Apple Notes hashtags directly from the NoteStore SQLite database.
///
/// Apple Notes (macOS Ventura+) stores hashtags as ICInlineAttachment entities
/// with type `com.apple.notes.inlinetextattachment.hashtag`. These do NOT appear
/// in the note's `plaintext` property (they render as a Unicode replacement char),
/// so AppleScript-based extraction misses them entirely.
///
/// Returns a map of note Z_PK → Vec<tag_name> (e.g., "#vehicles").
fn read_native_hashtags() -> HashMap<i64, Vec<String>> {
    let mut result: HashMap<i64, Vec<String>> = HashMap::new();

    let conn = match open_notestore_db() {
        Ok(c) => c,
        Err(_) => {
            tracing::debug!("NoteStore.sqlite not found, skipping native hashtag scan");
            return result;
        }
    };

    let query = "SELECT ia.ZALTTEXT, ia.ZNOTE1 \
                 FROM ZICCLOUDSYNCINGOBJECT ia \
                 WHERE ia.Z_ENT = 9 \
                 AND ia.ZTYPEUTI1 = 'com.apple.notes.inlinetextattachment.hashtag' \
                 AND ia.ZALTTEXT IS NOT NULL \
                 AND ia.ZNOTE1 IS NOT NULL";

    let mut stmt = match conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%e, "Failed to prepare native hashtags query");
            return result;
        }
    };

    let rows = match stmt.query_map([], |row| {
        let tag: String = row.get(0)?;
        let pk: i64 = row.get(1)?;
        Ok((tag, pk))
    }) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, "Failed to query native hashtags");
            return result;
        }
    };

    for row in rows {
        if let Ok((tag, pk)) = row {
            let tag = tag.trim().to_string();
            if !tag.is_empty() {
                result.entry(pk).or_default().push(tag);
            }
        }
    }

    tracing::info!(
        native_tags = result.values().map(|v| v.len()).sum::<usize>(),
        notes_with_tags = result.len(),
        "Native hashtags read from NoteStore"
    );

    result
}

/// Get the Core Data store UUID needed for constructing x-coredata:// note IDs.
///
/// Reads directly from the Z_METADATA table in NoteStore.sqlite — no Notes.app launch.
fn get_coredata_uuid() -> Result<String> {
    let conn = open_notestore_db()?;
    let uuid: String = conn
        .query_row("SELECT Z_UUID FROM Z_METADATA LIMIT 1", [], |row| row.get(0))
        .map_err(|e| anyhow!("Failed to read Z_UUID from Z_METADATA: {}", e))?;
    Ok(uuid)
}

/// Read all note metadata directly from the NoteStore SQLite database.
///
/// This replaces the slow AppleScript-based `notes_index_build.applescript` which
/// iterates every note via Apple Events (~17 min for 400+ notes). The SQLite query
/// completes in under a second.
///
/// Returns a Vec of IndexedNote with empty tags (tags are merged separately from
/// `read_native_hashtags()`).
fn read_notes_from_sqlite() -> Result<Vec<IndexedNote>> {
    let conn = open_notestore_db()?;

    let uuid = get_coredata_uuid()?;
    tracing::info!(%uuid, "Core Data store UUID");

    let query = "\
        SELECT c1.Z_PK, c1.ZTITLE1, \
               datetime(c1.ZMODIFICATIONDATE1 + 978307200, 'unixepoch'), \
               c2.ZTITLE2 \
        FROM ZICCLOUDSYNCINGOBJECT c1 \
        LEFT JOIN ZICCLOUDSYNCINGOBJECT c2 ON c2.Z_PK = c1.ZFOLDER \
        WHERE c1.ZTITLE1 IS NOT NULL \
        AND c1.ZMODIFICATIONDATE1 IS NOT NULL \
        AND c1.Z_ENT = (SELECT Z_ENT FROM Z_PRIMARYKEY WHERE Z_NAME = 'ICNote') \
        AND (c2.ZTITLE2 IS NULL OR c2.ZTITLE2 != 'Recently Deleted')";

    let mut stmt = conn.prepare(query)
        .map_err(|e| anyhow!("Failed to prepare notes query: {}", e))?;

    let notes: Vec<IndexedNote> = stmt
        .query_map([], |row| {
            let z_pk: i64 = row.get(0)?;
            let title: String = row.get(1)?;
            let modified: String = row.get(2)?;
            let folder: Option<String> = row.get(3)?;
            Ok((z_pk, title, modified, folder))
        })
        .map_err(|e| anyhow!("Failed to query notes: {}", e))?
        .filter_map(|row| {
            let (z_pk, title, modified, folder) = row.ok()?;
            Some(IndexedNote {
                id: format!("x-coredata://{}/ICNote/p{}", uuid, z_pk),
                title,
                folder: folder.unwrap_or_else(|| "Notes".to_string()),
                modified,
                tags: Vec::new(),
            })
        })
        .collect();

    tracing::info!(count = notes.len(), "Notes read from NoteStore SQLite");
    Ok(notes)
}

/// Get note count directly from NoteStore SQLite (fast alternative to AppleScript).
fn get_note_count_sqlite() -> Result<usize> {
    let conn = open_notestore_db()?;

    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ZICCLOUDSYNCINGOBJECT c1 \
         LEFT JOIN ZICCLOUDSYNCINGOBJECT c2 ON c2.Z_PK = c1.ZFOLDER \
         WHERE c1.Z_ENT = (SELECT Z_ENT FROM Z_PRIMARYKEY WHERE Z_NAME = 'ICNote') \
         AND c1.ZTITLE1 IS NOT NULL \
         AND (c2.ZTITLE2 IS NULL OR c2.ZTITLE2 != 'Recently Deleted')",
        [],
        |row| row.get(0),
    )
    .map_err(|e| anyhow!("SQLite count query failed: {}", e))?;

    Ok(count as usize)
}

/// Build or rebuild the notes index
pub fn build_index() -> Result<String> {
    tracing::info!("Reading Notes from NoteStore SQLite...");
    let indexed_notes = read_notes_from_sqlite()?;
    let note_count = indexed_notes.len();
    tracing::info!(note_count, "Notes found");

    // Read native hashtags from SQLite (macOS Ventura+ stores tags as metadata,
    // not in plaintext, so the AppleScript scan misses them)
    let native_tags = read_native_hashtags();

    tracing::info!("Building tag index...");
    // Build tag -> note_ids map
    let mut tags: HashMap<String, Vec<String>> = HashMap::new();
    let mut notes_map: HashMap<String, IndexedNote> = HashMap::new();

    for mut note in indexed_notes {
        let note_id = note.id.clone();

        // Merge native hashtags: match Z_PK from the note's x-coredata ID
        // Format: x-coredata://UUID/ICNote/p<Z_PK>
        if let Some(pk_str) = note_id.rsplit('/').next().and_then(|s| s.strip_prefix('p')) {
            if let Ok(pk) = pk_str.parse::<i64>() {
                if let Some(native) = native_tags.get(&pk) {
                    for tag in native {
                        if !note.tags.contains(tag) {
                            note.tags.push(tag.clone());
                        }
                    }
                }
            }
        }

        for tag in &note.tags {
            if !tag.is_empty() {
                let ids = tags.entry(tag.clone()).or_default();
                if !ids.contains(&note_id) {
                    ids.push(note_id.clone());
                }
            }
        }

        notes_map.insert(note_id, note);
    }
    tracing::info!(tag_count = tags.len(), "Tags found");

    let index = NotesIndex {
        note_count,
        last_updated: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        tags,
        notes: notes_map,
    };

    tracing::info!("Saving index...");
    save_index(&index)?;
    tracing::info!("Index saved");

    let tag_count = index.tags.len();
    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "note_count": index.note_count,
        "tag_count": tag_count,
        "last_updated": index.last_updated,
        "index_path": default_index_path().to_string_lossy()
    }))?)
}

/// Check if the index is stale (note count changed)
pub fn check_index() -> Result<String> {
    let current_count = get_note_count()?;

    match load_index() {
        Ok(index) => {
            let is_stale = current_count != index.note_count;
            Ok(serde_json::to_string_pretty(&json!({
                "success": true,
                "index_exists": true,
                "is_stale": is_stale,
                "current_note_count": current_count,
                "indexed_note_count": index.note_count,
                "last_updated": index.last_updated,
                "tag_count": index.tags.len()
            }))?)
        }
        Err(_) => {
            Ok(serde_json::to_string_pretty(&json!({
                "success": true,
                "index_exists": false,
                "is_stale": true,
                "current_note_count": current_count,
                "message": "Index not found. Run notes_index with action 'build' to create it."
            }))?)
        }
    }
}

/// List all tags from the index
pub fn list_tags() -> Result<String> {
    let index = load_index()?;

    // Sort tags by count (descending), then alphabetically
    let mut tag_list: Vec<(&String, usize)> = index
        .tags
        .iter()
        .map(|(tag, ids)| (tag, ids.len()))
        .collect();
    tag_list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));

    let tags: Vec<Value> = tag_list
        .iter()
        .map(|(tag, count)| {
            json!({
                "tag": tag,
                "count": count
            })
        })
        .collect();

    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "tag_count": tags.len(),
        "tags": tags
    }))?)
}

/// Search notes by tag
pub fn search_by_tag(tag: &str) -> Result<String> {
    let index = load_index()?;

    // Normalize tag (ensure it starts with #)
    let normalized_tag = if tag.starts_with('#') {
        tag.to_string()
    } else {
        format!("#{}", tag)
    };

    let note_ids = index.tags.get(&normalized_tag);

    let notes: Vec<Value> = match note_ids {
        Some(ids) => ids
            .iter()
            .filter_map(|id| index.notes.get(id))
            .map(|note| {
                json!({
                    "id": note.id,
                    "title": note.title,
                    "folder": note.folder,
                    "modified": note.modified,
                    "tags": note.tags,
                    "open_cmd": format!("osascript scripts/notes_open.applescript \"{}\"", note.id)
                })
            })
            .collect(),
        None => Vec::new(),
    };

    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "tag": normalized_tag,
        "count": notes.len(),
        "notes": notes
    }))?)
}

// ============================================================================
// Tool Executor Integration
// ============================================================================

/// Main entry point for agent tool execution
pub fn execute_apple_notes(action: &str, args: &Value) -> Result<String> {
    match action {
        "search" => {
            // Check if searching by tag
            if let Some(tag) = args.get("tag").and_then(|v| v.as_str()) {
                return search_by_tag(tag);
            }
            let query = args["query"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required 'query' argument"))?;
            let folder = args.get("folder").and_then(|v| v.as_str());
            search_notes(query, folder)
        }
        "list" => {
            let folder = args.get("folder").and_then(|v| v.as_str());
            list_notes(folder)
        }
        "get" => {
            let id = args["id"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required 'id' argument"))?;
            get_note(id)
        }
        "open" => {
            let id = args["id"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required 'id' argument"))?;
            open_note(id)
        }
        // Tag index operations
        "index_build" => build_index(),
        "index_check" => check_index(),
        "tags" => list_tags(),
        "search_by_tag" => {
            let tag = args["tag"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required 'tag' argument"))?;
            search_by_tag(tag)
        }
        // Semantic search operations (memvid-powered)
        "semantic_search" => {
            let query = args["query"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required 'query' argument"))?;
            let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            crate::memvid_notes::search_json(query, top_k)
        }
        "rebuild_memvid_index" => crate::memvid_notes::rebuild_index_json(),
        "memvid_stats" => crate::memvid_notes::stats_json(),
        "smart_search" => {
            let query = args["query"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required 'query' argument"))?;
            crate::memvid_notes::smart_search(query)
        }
        "create" => {
            let title = args["title"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required 'title' argument"))?;
            let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("");
            let folder = args.get("folder").and_then(|v| v.as_str());
            create_note(title, body, folder)
        }
        _ => Err(anyhow!("Unknown Apple Notes action: {}", action)),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_records_empty() {
        let output = "";
        let records = parse_records(output).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn test_parse_records_single() {
        let output = r#"RECORD_START
id: x-coredata://123
title: Test Note
folder: Notes
modified: 2026-01-27T10:30:00Z
snippet: This is a test note...
RECORD_END"#;

        let records = parse_records(output).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "x-coredata://123");
        assert_eq!(records[0].title, "Test Note");
        assert_eq!(records[0].folder, "Notes");
        assert_eq!(records[0].snippet, "This is a test note...");
        assert_eq!(
            records[0].open_cmd,
            "osascript scripts/notes_open.applescript \"x-coredata://123\""
        );
    }

    #[test]
    fn test_parse_records_multiple() {
        let output = r#"RECORD_START
id: note-1
title: First Note
folder: Work
modified: 2026-01-27T10:00:00Z
snippet: First note content
RECORD_END
RECORD_START
id: note-2
title: Second Note
folder: Personal
modified: 2026-01-27T11:00:00Z
snippet: Second note content
RECORD_END"#;

        let records = parse_records(output).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].title, "First Note");
        assert_eq!(records[1].title, "Second Note");
    }

    #[test]
    fn test_parse_records_error() {
        let output = "ERROR: Notes application not available";
        let result = parse_records(output);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ERROR:"));
    }

    #[test]
    fn test_parse_note_content() {
        let output = r#"id: x-coredata://123
title: Full Note
folder: Notes
modified: 2026-01-27T10:30:00Z
BODY_START
This is the full body of the note.
It can have multiple lines.

And paragraphs.
BODY_END"#;

        let note = parse_note_content(output).unwrap();
        assert_eq!(note.id, "x-coredata://123");
        assert_eq!(note.title, "Full Note");
        assert!(note.body.contains("multiple lines"));
        assert!(note.body.contains("paragraphs"));
        assert_eq!(
            note.open_cmd,
            "osascript scripts/notes_open.applescript \"x-coredata://123\""
        );
    }

    #[test]
    fn test_is_available_without_scripts() {
        // This test just ensures the function runs without panicking
        // Actual availability depends on whether scripts exist
        let _ = is_available();
    }

    #[test]
    fn test_is_css_color_code() {
        // 3-digit hex colors
        assert!(is_css_color_code("#fff"));
        assert!(is_css_color_code("#FFF"));
        assert!(is_css_color_code("#abc"));
        assert!(is_css_color_code("#123"));

        // 6-digit hex colors
        assert!(is_css_color_code("#ffffff"));
        assert!(is_css_color_code("#FFFFFF"));
        assert!(is_css_color_code("#dee2e6"));
        assert!(is_css_color_code("#e9ecef"));
        assert!(is_css_color_code("#000000"));

        // 8-digit hex colors (with alpha)
        assert!(is_css_color_code("#ffffffff"));
        assert!(is_css_color_code("#00000080"));

        // Not color codes - real tags
        assert!(!is_css_color_code("#project"));
        assert!(!is_css_color_code("#todo"));
        assert!(!is_css_color_code("#work"));
        assert!(!is_css_color_code("#meeting-notes"));

        // Edge cases
        assert!(!is_css_color_code("#ff")); // Too short
        assert!(!is_css_color_code("#ffff")); // 4 digits - not valid
        assert!(!is_css_color_code("#fffff")); // 5 digits - not valid
        assert!(!is_css_color_code("#fffffff")); // 7 digits - not valid
        assert!(!is_css_color_code("#fffffffff")); // 9 digits - too long
        assert!(!is_css_color_code("#ghijkl")); // Not hex
    }

    #[test]
    fn test_parse_index_filters_color_codes() {
        let output = r#"NOTE_COUNT: 1
RECORD_START
id: note-1
title: Test Note
folder: Notes
modified: 2026-01-27T10:00:00Z
tags: #project,#fff,#work,#dee2e6,#todo
RECORD_END"#;

        let (count, notes) = parse_index_output(output).unwrap();
        assert_eq!(count, 1);
        assert_eq!(notes.len(), 1);
        // Should filter out #fff and #dee2e6
        assert_eq!(notes[0].tags, vec!["#project", "#work", "#todo"]);
    }

    // ========================================================================
    // Protobuf parser tests
    // ========================================================================

    /// Build a protobuf varint encoding of `value`.
    fn encode_varint(value: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut v = value;
        loop {
            let mut byte = (v & 0x7F) as u8;
            v >>= 7;
            if v > 0 {
                byte |= 0x80;
            }
            buf.push(byte);
            if v == 0 {
                break;
            }
        }
        buf
    }

    /// Build a protobuf length-delimited field (wire type 2).
    fn encode_field(field_number: u32, payload: &[u8]) -> Vec<u8> {
        let tag = (field_number << 3) | 2; // wire type 2
        let mut buf = encode_varint(tag as u64);
        buf.extend(encode_varint(payload.len() as u64));
        buf.extend_from_slice(payload);
        buf
    }

    #[test]
    fn test_proto_read_varint() {
        // Single-byte varint
        let (val, len) = proto::read_varint(&[0x05], 0).unwrap();
        assert_eq!(val, 5);
        assert_eq!(len, 1);

        // Multi-byte varint: 300 = 0xAC 0x02
        let (val, len) = proto::read_varint(&[0xAC, 0x02], 0).unwrap();
        assert_eq!(val, 300);
        assert_eq!(len, 2);

        // Varint at an offset
        let (val, len) = proto::read_varint(&[0xFF, 0x01], 1).unwrap();
        assert_eq!(val, 1);
        assert_eq!(len, 1);
    }

    #[test]
    fn test_proto_find_field() {
        let payload = b"hello";
        let data = encode_field(3, payload);
        let found = proto::find_field(&data, 3).unwrap();
        assert_eq!(found, b"hello");

        // Should return None for a non-existent field
        assert!(proto::find_field(&data, 99).is_none());
    }

    #[test]
    fn test_proto_extract_plaintext_roundtrip() {
        // Build: root → field 2 (doc) → field 3 (note) → field 2 (text)
        let text = b"Hello, this is a test note body!";
        let inner = encode_field(2, text);       // note.field2 = text
        let note = encode_field(3, &inner);       // doc.field3 = note
        let doc = encode_field(2, &note);         // root.field2 = doc

        let extracted = proto::extract_plaintext(&doc).unwrap();
        assert_eq!(extracted, "Hello, this is a test note body!");
    }

    #[test]
    fn test_proto_extract_plaintext_empty() {
        // Empty text
        let inner = encode_field(2, b"");
        let note = encode_field(3, &inner);
        let doc = encode_field(2, &note);

        let extracted = proto::extract_plaintext(&doc).unwrap();
        assert_eq!(extracted, "");
    }

    #[test]
    fn test_proto_extract_plaintext_missing_field() {
        // Only has field 1 instead of field 2 at the root
        let data = encode_field(1, b"wrong field");
        assert!(proto::extract_plaintext(&data).is_none());
    }

    // ========================================================================
    // parse_zpk_from_id tests
    // ========================================================================

    #[test]
    fn test_parse_zpk_from_id_valid() {
        let id = "x-coredata://ABC-123-DEF/ICNote/p456";
        assert_eq!(parse_zpk_from_id(id).unwrap(), 456);
    }

    #[test]
    fn test_parse_zpk_from_id_large_pk() {
        let id = "x-coredata://UUID/ICNote/p99999";
        assert_eq!(parse_zpk_from_id(id).unwrap(), 99999);
    }

    #[test]
    fn test_parse_zpk_from_id_invalid_no_p_prefix() {
        let id = "x-coredata://UUID/ICNote/456";
        assert!(parse_zpk_from_id(id).is_err());
    }

    #[test]
    fn test_parse_zpk_from_id_invalid_not_a_number() {
        let id = "x-coredata://UUID/ICNote/pabc";
        assert!(parse_zpk_from_id(id).is_err());
    }

    #[test]
    fn test_parse_zpk_from_id_invalid_empty() {
        assert!(parse_zpk_from_id("").is_err());
    }

    // ========================================================================
    // decompress_and_extract tests
    // ========================================================================

    #[test]
    fn test_decompress_and_extract_roundtrip() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        // Build protobuf: root → field 2 → field 3 → field 2 = "Test body with \u{FFFC} attachment"
        let text = "Test body with \u{FFFC} attachment";
        let inner = encode_field(2, text.as_bytes());
        let note = encode_field(3, &inner);
        let doc = encode_field(2, &note);

        // Gzip compress
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&doc).unwrap();
        let compressed = encoder.finish().unwrap();

        let result = decompress_and_extract(&compressed).unwrap();
        assert_eq!(result, "Test body with  attachment"); // U+FFFC stripped
    }

    #[test]
    fn test_decompress_and_extract_invalid_gzip() {
        let result = decompress_and_extract(b"not gzip data");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Gzip"));
    }

    // ========================================================================
    // Integration test (requires macOS with Notes.app)
    // ========================================================================

    /// Read a real note from NoteStore.sqlite and verify non-empty title/body.
    ///
    /// Run with:
    ///   cargo test -p prolog-router-core test_sqlite_read_real_note -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_sqlite_read_real_note() {
        // Get a note ID from the index
        let notes = read_notes_from_sqlite().expect("Failed to read notes from SQLite");
        assert!(!notes.is_empty(), "No notes found in NoteStore");

        let note_id = &notes[0].id;
        eprintln!("Reading note: {} ({})", notes[0].title, note_id);

        let content = get_note_content(note_id).expect("Failed to get note content");
        eprintln!("Title: {}", content.title);
        eprintln!("Body length: {} chars", content.body.len());
        eprintln!("Body preview: {}", &content.body[..content.body.len().min(200)]);

        assert!(!content.title.is_empty(), "Title should not be empty");
        assert!(!content.body.is_empty(), "Body should not be empty");
        assert!(!content.id.is_empty(), "ID should not be empty");
    }

    /// Batch-fetch all notes via SQLite and verify results.
    ///
    /// Run with:
    ///   cargo test -p prolog-router-core test_sqlite_batch_fetch -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_sqlite_batch_fetch() {
        let notes = read_notes_from_sqlite().expect("Failed to read notes from SQLite");
        let note_ids: Vec<String> = notes.iter().map(|n| n.id.clone()).collect();
        eprintln!("Fetching {} notes via batch SQLite...", note_ids.len());

        let start = std::time::Instant::now();
        let results = get_notes_batch_sqlite(&note_ids).expect("Batch fetch failed");
        let elapsed = start.elapsed();

        eprintln!(
            "Fetched {}/{} notes in {:.2}s",
            results.len(),
            note_ids.len(),
            elapsed.as_secs_f64()
        );

        assert!(!results.is_empty(), "Should have fetched at least one note");

        // Verify a sample note has content
        let sample = results.values().next().unwrap();
        eprintln!("Sample: {} — {} chars", sample.title, sample.body.len());
        assert!(!sample.title.is_empty());
    }
}
