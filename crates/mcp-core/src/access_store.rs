//! Unified Access Control Store
//!
//! Stores all access-control data — allowed Reminder lists, granted file
//! folders, and message recipients. Supports pluggable backends via the
//! `AccessBackend` trait. Default backend is macOS Keychain via `keyring`.
//!
//! A process-level cache eliminates repeat backend hits.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::apple_reminders::AllowedReminderList;
use crate::file_search::GrantedFolder;

// ============================================================================
// Pluggable Backend Trait
// ============================================================================

/// Trait for encrypted backends (e.g., SecretsStore in the paid app).
/// Implement this and call `AccessStore::set_backend()` at startup.
pub trait AccessBackend: Send + Sync {
    fn get_access_json(&self) -> Result<String, String>;
    fn set_access_json(&self, json: &str) -> Result<(), String>;
}

// ============================================================================
// Internal Data Model
// ============================================================================

const SERVICE: &str = "prolog-router";
const ACCOUNT: &str = "allowed-sources";

/// File-based override: when set, read access data from this file instead of Keychain.
static FILE_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Pluggable backend (e.g., encrypted SecretsStore). Set at app startup.
static BACKEND: OnceLock<Arc<dyn AccessBackend>> = OnceLock::new();

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AccessData {
    #[serde(default)]
    reminder_lists: BTreeMap<String, ReminderListEntry>,
    #[serde(default)]
    file_folders: BTreeMap<String, FileFolderEntry>,
    #[serde(default)]
    allowed_recipients: BTreeMap<String, RecipientEntry>,
    #[serde(default)]
    contact_sources: BTreeMap<String, ContactSourceEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    temperature_unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bot_email: Option<String>,
    /// Name of the contact group that serves as the email allowlist.
    /// Only emails from contacts in this group are processed by the IMAP poller.
    /// When None, all emails are accepted (open access).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    email_allowlist_group: Option<String>,
    /// Admin email addresses for notifications about blocked emails (max 3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    admin_emails: Vec<String>,
    /// Whether the iMessage background poller is enabled.
    #[serde(default)]
    imessage_poller_enabled: bool,
    /// Last processed chat.db ROWID (persisted across restarts to catch missed messages).
    #[serde(default)]
    imessage_last_rowid: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReminderListEntry {
    enabled: bool,
    allowed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guidance: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileFolderEntry {
    enabled: bool,
    allowed_at: String,
    display_name: String,
    #[serde(default)]
    writable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecipientEntry {
    enabled: bool,
    allowed_at: String,
    /// Human-readable label (e.g., "Mom", "Boss")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}

/// Public-facing allowed recipient info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowedRecipient {
    /// Phone number or email address
    pub id: String,
    pub enabled: bool,
    pub allowed_at: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ContactSourceEntry {
    enabled: bool,
    allowed_at: String,
    /// "container" or "group"
    source_type: String,
    /// For groups: which container they belong to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    container_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guidance: Option<String>,
    #[serde(default)]
    writable: bool,
}

/// Public-facing allowed contact source info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowedContactSource {
    pub name: String,
    /// "container" or "group"
    pub source_type: String,
    pub container_name: Option<String>,
    pub enabled: bool,
    pub allowed_at: String,
    pub guidance: Option<String>,
    pub writable: bool,
}

// ============================================================================
// Process-Level Cache
// ============================================================================

static CACHE: Mutex<Option<AccessData>> = Mutex::new(None);

/// Mutex to serialize tests that touch the shared static CACHE.
/// Acquire this before calling inject_cache / clear_cache from any module.
#[cfg(test)]
pub(crate) static CACHE_TEST_MUTEX: Mutex<()> = Mutex::new(());

/// Load data from the pluggable backend (if set), falling back to Keychain.
fn load_from_backend() -> AccessData {
    // Try pluggable backend first
    if let Some(backend) = BACKEND.get() {
        match backend.get_access_json() {
            Ok(json) => {
                return serde_json::from_str(&json).unwrap_or_default();
            }
            Err(e) => {
                tracing::debug!(error = %e, "Backend read failed, falling back to Keychain");
            }
        }
    }

    // Fall back to Keychain
    load_from_keychain()
}

/// Load data from Keychain directly (legacy fallback).
fn load_from_keychain() -> AccessData {
    #[cfg(test)]
    if std::env::var("PROLOG_ROUTER_TEST_USE_KEYCHAIN").is_err() {
        return AccessData::default();
    }
    match keyring::Entry::new(SERVICE, ACCOUNT) {
        Ok(entry) => match entry.get_password() {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => AccessData::default(),
        },
        Err(_) => AccessData::default(),
    }
}

/// Load data from a JSON file on disk.
fn load_from_file(path: &Path) -> AccessData {
    match std::fs::read_to_string(path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
            tracing::warn!(error = %e, path = %path.display(), "Failed to parse access store file");
            AccessData::default()
        }),
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "Failed to read access store file");
            AccessData::default()
        }
    }
}

/// Get cached data (loads from file override, SecretsStore, or Keychain on first call).
fn get_cached() -> AccessData {
    let mut cache = CACHE.lock().unwrap();
    if let Some(ref cached) = *cache {
        return cached.clone();
    }
    let data = if let Some(path) = FILE_OVERRIDE.get() {
        tracing::debug!(path = %path.display(), "Loading access store from file override");
        load_from_file(path)
    } else {
        load_from_backend()
    };
    *cache = Some(data.clone());
    data
}

/// Save data to the backend (pluggable preferred, Keychain fallback) and update cache.
fn save_to_backend(data: &AccessData) -> Result<(), String> {
    // Try pluggable backend first
    if let Some(backend) = BACKEND.get() {
        let json = serde_json::to_string(data)
            .map_err(|e| format!("serialize error: {}", e))?;
        backend.set_access_json(&json)?;
        // Update cache inline
        let mut cache = CACHE.lock().unwrap();
        *cache = Some(data.clone());
        return Ok(());
    }

    // Fall back to Keychain
    save_to_keychain(data)
}

/// Save data directly to Keychain (legacy path).
fn save_to_keychain(data: &AccessData) -> Result<(), String> {
    #[cfg(test)]
    if std::env::var("PROLOG_ROUTER_TEST_USE_KEYCHAIN").is_err() {
        // Update cache only, skip Keychain
        let mut cache = CACHE.lock().unwrap();
        *cache = Some(data.clone());
        return Ok(());
    }
    let json = serde_json::to_string(data)
        .map_err(|e| format!("serialize error: {}", e))?;
    let entry = keyring::Entry::new(SERVICE, ACCOUNT)
        .map_err(|e| format!("keyring entry error: {}", e))?;
    entry
        .set_password(&json)
        .map_err(|e| format!("keyring write error: {}", e))?;
    // Update cache inline
    let mut cache = CACHE.lock().unwrap();
    *cache = Some(data.clone());
    Ok(())
}

// ============================================================================
// Public API
// ============================================================================

/// Unified access control store backed by a single Keychain entry.
///
/// All methods are static. Reads are served from a process-level cache;
/// writes go to Keychain and update the cache inline.
pub struct AccessStore;

impl AccessStore {
    /// Set a pluggable backend (e.g., encrypted SecretsStore) as the primary backend.
    /// Must be called at startup before any other AccessStore method.
    pub fn set_backend(backend: Arc<dyn AccessBackend>) {
        if BACKEND.set(backend).is_err() {
            tracing::warn!("AccessStore backend already set, ignoring");
        } else {
            // Clear cache so next access reads from the new backend
            let mut cache = CACHE.lock().unwrap();
            *cache = None;
            tracing::info!("AccessStore: using pluggable backend");
        }
    }

    /// Use a JSON file on disk instead of Keychain for access data.
    /// Must be called before any other AccessStore method (typically at startup).
    pub fn set_file_override(path: PathBuf) {
        if FILE_OVERRIDE.set(path.clone()).is_err() {
            tracing::warn!("AccessStore file override already set, ignoring");
        } else {
            // Clear cache so next access reads from the file
            let mut cache = CACHE.lock().unwrap();
            *cache = None;
            tracing::info!(path = %path.display(), "AccessStore: using file override (Keychain bypassed)");
        }
    }

    // ---- Reminder Lists ----

    /// List all allowed reminder list entries.
    pub fn list_reminder_lists() -> Result<Vec<AllowedReminderList>, String> {
        let data = get_cached();
        Ok(data
            .reminder_lists
            .iter()
            .map(|(name, e)| AllowedReminderList {
                name: name.clone(),
                enabled: e.enabled,
                allowed_at: e.allowed_at.clone(),
                guidance: e.guidance.clone(),
            })
            .collect())
    }

    /// Check if a reminder list is allowed for targeted operations.
    /// Returns true when no restrictions are configured (empty map) or
    /// when the list is present and enabled.
    pub fn is_reminder_list_allowed(list_name: &str) -> bool {
        let data = get_cached();
        if data.reminder_lists.is_empty() {
            return true; // no restrictions
        }
        data.reminder_lists
            .get(list_name)
            .map(|e| e.enabled)
            .unwrap_or(false)
    }

    /// Get names of enabled reminder lists.
    pub fn reminder_list_enabled_names() -> Vec<String> {
        let data = get_cached();
        data.reminder_lists
            .iter()
            .filter(|(_, e)| e.enabled)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Returns true if any reminder list restrictions are configured.
    pub fn has_reminder_restrictions() -> bool {
        !get_cached().reminder_lists.is_empty()
    }

    /// Add a reminder list to the allowed set.
    pub fn allow_reminder_list(list_name: &str) -> Result<(), String> {
        let mut data = get_cached();
        data.reminder_lists.entry(list_name.to_string()).or_insert(
            ReminderListEntry {
                enabled: true,
                allowed_at: chrono::Utc::now().to_rfc3339(),
                guidance: None,
            },
        );
        save_to_backend(&data)
    }

    /// Remove a reminder list from the allowed set.
    pub fn disallow_reminder_list(list_name: &str) -> Result<(), String> {
        let mut data = get_cached();
        data.reminder_lists.remove(list_name);
        save_to_backend(&data)
    }

    /// Toggle the enabled state for a reminder list.
    pub fn set_reminder_list_enabled(list_name: &str, enabled: bool) -> Result<(), String> {
        let mut data = get_cached();
        match data.reminder_lists.get_mut(list_name) {
            Some(entry) => entry.enabled = enabled,
            None => return Err(format!("List '{}' not in allowed set", list_name)),
        }
        save_to_backend(&data)
    }

    /// Set or clear the guidance prompt for a reminder list.
    pub fn set_reminder_list_guidance(list_name: &str, guidance: Option<String>) -> Result<(), String> {
        let mut data = get_cached();
        match data.reminder_lists.get_mut(list_name) {
            Some(entry) => {
                entry.guidance = guidance.filter(|g| !g.trim().is_empty());
            }
            None => return Err(format!("List '{}' not in allowed set", list_name)),
        }
        save_to_backend(&data)
    }

    // ---- File Folders ----

    /// List all granted folders.
    pub fn list_folders() -> Result<Vec<GrantedFolder>, String> {
        let data = get_cached();
        Ok(data
            .file_folders
            .iter()
            .map(|(path_str, e)| GrantedFolder {
                path: PathBuf::from(path_str),
                display_name: e.display_name.clone(),
                granted_at: e.allowed_at.clone(),
                enabled: e.enabled,
                writable: e.writable,
            })
            .collect())
    }

    /// List only enabled folders.
    pub fn enabled_folders() -> Vec<GrantedFolder> {
        let data = get_cached();
        data.file_folders
            .iter()
            .filter(|(_, e)| e.enabled)
            .map(|(path_str, e)| GrantedFolder {
                path: PathBuf::from(path_str),
                display_name: e.display_name.clone(),
                granted_at: e.allowed_at.clone(),
                enabled: e.enabled,
                writable: e.writable,
            })
            .collect()
    }

    /// Add a folder (validates it exists and is a directory, deduplicates via canonicalize).
    pub fn add_folder(path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Err(format!("Path does not exist: {}", path.display()));
        }
        if !path.is_dir() {
            return Err(format!("Path is not a directory: {}", path.display()));
        }

        let canonical = path
            .canonicalize()
            .map_err(|e| format!("canonicalize error: {}", e))?;
        let key = canonical.to_string_lossy().to_string();

        let mut data = get_cached();
        if data.file_folders.contains_key(&key) {
            return Err(format!("Folder already granted: {}", path.display()));
        }

        let display_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string();

        data.file_folders.insert(
            key,
            FileFolderEntry {
                enabled: true,
                allowed_at: chrono::Utc::now().to_rfc3339(),
                display_name,
                writable: false,
            },
        );
        save_to_backend(&data)
    }

    /// Remove a folder by path.
    pub fn remove_folder(path: &Path) -> Result<(), String> {
        let canonical = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());
        let key = canonical.to_string_lossy().to_string();

        let mut data = get_cached();
        if data.file_folders.remove(&key).is_none() {
            // Try matching against existing entries by canonicalizing them
            let found_key = data
                .file_folders
                .keys()
                .find(|k| {
                    Path::new(k.as_str())
                        .canonicalize()
                        .ok()
                        .as_ref()
                        == Some(&canonical)
                })
                .cloned();
            match found_key {
                Some(k) => {
                    data.file_folders.remove(&k);
                }
                None => return Err(format!("Folder not found: {}", path.display())),
            }
        }
        save_to_backend(&data)
    }

    /// Enable or disable a folder.
    pub fn set_folder_enabled(path: &Path, enabled: bool) -> Result<(), String> {
        let canonical = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());
        let key = canonical.to_string_lossy().to_string();

        let mut data = get_cached();
        if let Some(entry) = data.file_folders.get_mut(&key) {
            entry.enabled = enabled;
            return save_to_backend(&data);
        }
        // Try matching by canonicalizing existing keys
        let found_key = data
            .file_folders
            .keys()
            .find(|k| {
                Path::new(k.as_str())
                    .canonicalize()
                    .ok()
                    .as_ref()
                    == Some(&canonical)
            })
            .cloned();
        match found_key {
            Some(k) => {
                data.file_folders.get_mut(&k).unwrap().enabled = enabled;
                save_to_backend(&data)
            }
            None => Err(format!("Folder not found: {}", path.display())),
        }
    }

    /// Set the writable flag for a folder.
    pub fn set_folder_writable(path: &Path, writable: bool) -> Result<(), String> {
        let canonical = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());
        let key = canonical.to_string_lossy().to_string();

        let mut data = get_cached();
        if let Some(entry) = data.file_folders.get_mut(&key) {
            entry.writable = writable;
            return save_to_backend(&data);
        }
        // Try matching by canonicalizing existing keys
        let found_key = data
            .file_folders
            .keys()
            .find(|k| {
                Path::new(k.as_str())
                    .canonicalize()
                    .ok()
                    .as_ref()
                    == Some(&canonical)
            })
            .cloned();
        match found_key {
            Some(k) => {
                data.file_folders.get_mut(&k).unwrap().writable = writable;
                save_to_backend(&data)
            }
            None => Err(format!("Folder not found: {}", path.display())),
        }
    }

    // ---- Message Recipients ----

    /// List all allowed recipients.
    pub fn list_recipients() -> Result<Vec<AllowedRecipient>, String> {
        let data = get_cached();
        Ok(data
            .allowed_recipients
            .iter()
            .map(|(id, e)| AllowedRecipient {
                id: id.clone(),
                enabled: e.enabled,
                allowed_at: e.allowed_at.clone(),
                label: e.label.clone(),
            })
            .collect())
    }

    /// Check if a recipient is allowed for messaging.
    pub fn is_recipient_allowed(recipient: &str) -> bool {
        let data = get_cached();
        if data.allowed_recipients.is_empty() {
            return false; // no recipients means messaging is not configured
        }
        data.allowed_recipients
            .get(recipient)
            .map(|e| e.enabled)
            .unwrap_or(false)
    }

    /// Get IDs of enabled recipients.
    pub fn recipient_enabled_ids() -> Vec<String> {
        let data = get_cached();
        data.allowed_recipients
            .iter()
            .filter(|(_, e)| e.enabled)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Add a recipient to the allowed set.
    pub fn allow_recipient(id: &str, label: Option<&str>) -> Result<(), String> {
        let mut data = get_cached();
        data.allowed_recipients
            .entry(id.to_string())
            .or_insert(RecipientEntry {
                enabled: true,
                allowed_at: chrono::Utc::now().to_rfc3339(),
                label: label.map(|s| s.to_string()),
            });
        save_to_backend(&data)
    }

    /// Remove a recipient from the allowed set.
    pub fn disallow_recipient(id: &str) -> Result<(), String> {
        let mut data = get_cached();
        data.allowed_recipients.remove(id);
        save_to_backend(&data)
    }

    /// Toggle the enabled state for a recipient.
    pub fn set_recipient_enabled(id: &str, enabled: bool) -> Result<(), String> {
        let mut data = get_cached();
        match data.allowed_recipients.get_mut(id) {
            Some(entry) => entry.enabled = enabled,
            None => return Err(format!("Recipient '{}' not in allowed set", id)),
        }
        save_to_backend(&data)
    }

    // ---- Contact Sources ----

    /// List all allowed contact source entries.
    pub fn list_contact_sources() -> Result<Vec<AllowedContactSource>, String> {
        let data = get_cached();
        Ok(data
            .contact_sources
            .iter()
            .map(|(name, e)| AllowedContactSource {
                name: name.clone(),
                source_type: e.source_type.clone(),
                container_name: e.container_name.clone(),
                enabled: e.enabled,
                allowed_at: e.allowed_at.clone(),
                guidance: e.guidance.clone(),
                writable: e.writable,
            })
            .collect())
    }

    /// Check if a contact source is allowed for access.
    /// Returns true when no restrictions are configured (empty map) or
    /// when the source is present and enabled.
    pub fn is_contact_source_allowed(name: &str) -> bool {
        let data = get_cached();
        if data.contact_sources.is_empty() {
            return true; // no restrictions
        }
        data.contact_sources
            .get(name)
            .map(|e| e.enabled)
            .unwrap_or(false)
    }

    /// Get names of enabled contact sources.
    pub fn contact_source_enabled_names() -> Vec<String> {
        let data = get_cached();
        data.contact_sources
            .iter()
            .filter(|(_, e)| e.enabled)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Returns true if any contact source restrictions are configured.
    pub fn has_contact_restrictions() -> bool {
        !get_cached().contact_sources.is_empty()
    }

    /// Check if a contact source is writable.
    /// Returns true when no restrictions are configured or when the source
    /// is present, enabled, and marked writable.
    pub fn is_contact_source_writable(name: &str) -> bool {
        let data = get_cached();
        if data.contact_sources.is_empty() {
            return true; // no restrictions = full access
        }
        data.contact_sources
            .get(name)
            .map(|e| e.enabled && e.writable)
            .unwrap_or(false)
    }

    /// Add a contact source to the allowed set.
    pub fn allow_contact_source(
        name: &str,
        source_type: &str,
        container_name: Option<&str>,
    ) -> Result<(), String> {
        let mut data = get_cached();
        data.contact_sources
            .entry(name.to_string())
            .or_insert(ContactSourceEntry {
                enabled: true,
                allowed_at: chrono::Utc::now().to_rfc3339(),
                source_type: source_type.to_string(),
                container_name: container_name.map(|s| s.to_string()),
                guidance: None,
                writable: false,
            });
        save_to_backend(&data)
    }

    /// Remove a contact source from the allowed set.
    pub fn disallow_contact_source(name: &str) -> Result<(), String> {
        let mut data = get_cached();
        data.contact_sources.remove(name);
        save_to_backend(&data)
    }

    /// Toggle the enabled state for a contact source.
    pub fn set_contact_source_enabled(name: &str, enabled: bool) -> Result<(), String> {
        let mut data = get_cached();
        match data.contact_sources.get_mut(name) {
            Some(entry) => entry.enabled = enabled,
            None => return Err(format!("Contact source '{}' not in allowed set", name)),
        }
        save_to_backend(&data)
    }

    /// Set or clear the guidance prompt for a contact source.
    pub fn set_contact_source_guidance(
        name: &str,
        guidance: Option<String>,
    ) -> Result<(), String> {
        let mut data = get_cached();
        match data.contact_sources.get_mut(name) {
            Some(entry) => {
                entry.guidance = guidance.filter(|g| !g.trim().is_empty());
            }
            None => return Err(format!("Contact source '{}' not in allowed set", name)),
        }
        save_to_backend(&data)
    }

    /// Set the writable flag for a contact source.
    pub fn set_contact_source_writable(name: &str, writable: bool) -> Result<(), String> {
        let mut data = get_cached();
        match data.contact_sources.get_mut(name) {
            Some(entry) => entry.writable = writable,
            None => return Err(format!("Contact source '{}' not in allowed set", name)),
        }
        save_to_backend(&data)
    }

    // ---- User Settings ----

    /// Get the default location (address string for "near me" queries).
    pub fn get_default_location() -> Option<String> {
        get_cached().default_location.clone()
    }

    /// Set or clear the default location.
    pub fn set_default_location(location: Option<String>) -> Result<(), String> {
        let mut data = get_cached();
        data.default_location = location.filter(|s| !s.trim().is_empty());
        save_to_backend(&data)
    }

    /// Get the temperature unit preference ("F" or "C"). Returns None if not set.
    pub fn get_temperature_unit() -> Option<String> {
        get_cached().temperature_unit.clone()
    }

    /// Set the temperature unit preference ("F" or "C"). Also sets TEMPERATURE_UNIT env var.
    pub fn set_temperature_unit(unit: Option<String>) -> Result<(), String> {
        let mut data = get_cached();
        let unit = unit.filter(|s| !s.trim().is_empty());
        if let Some(ref u) = unit {
            std::env::set_var("TEMPERATURE_UNIT", u);
        } else {
            std::env::remove_var("TEMPERATURE_UNIT");
        }
        data.temperature_unit = unit;
        save_to_backend(&data)
    }

    /// Apply stored temperature unit to env var (call at startup).
    pub fn apply_temperature_unit_to_env() {
        if let Some(unit) = get_cached().temperature_unit.as_ref() {
            if std::env::var("TEMPERATURE_UNIT").is_err() {
                std::env::set_var("TEMPERATURE_UNIT", unit);
            }
        }
    }

    // ---- Bot Email ----

    /// Get the bot email address (used in mailto: links for conversation continuity).
    pub fn get_bot_email() -> Option<String> {
        get_cached().bot_email.clone()
    }

    /// Set or clear the bot email address.
    pub fn set_bot_email(email: Option<String>) -> Result<(), String> {
        let mut data = get_cached();
        data.bot_email = email.filter(|s| !s.trim().is_empty());
        save_to_backend(&data)
    }

    // ---- Email Allowlist ----

    /// Get the contact group designated as the email allowlist.
    pub fn get_email_allowlist_group() -> Option<String> {
        get_cached().email_allowlist_group.clone()
    }

    /// Set or clear the email allowlist group.
    pub fn set_email_allowlist_group(group: Option<String>) -> Result<(), String> {
        let mut data = get_cached();
        data.email_allowlist_group = group.filter(|s| !s.trim().is_empty());
        save_to_backend(&data)
    }

    /// Get admin email addresses for email gate notifications.
    pub fn get_admin_emails() -> Vec<String> {
        get_cached().admin_emails.clone()
    }

    /// Set admin email addresses (max 3, deduplicated, lowercased).
    pub fn set_admin_emails(emails: Vec<String>) -> Result<(), String> {
        let mut data = get_cached();
        let mut seen = std::collections::HashSet::new();
        data.admin_emails = emails
            .into_iter()
            .map(|e| e.trim().to_lowercase())
            .filter(|e| !e.is_empty() && seen.insert(e.clone()))
            .take(3)
            .collect();
        save_to_backend(&data)
    }

    // ---- iMessage Poller ----

    /// Get the iMessage trigger phrase. When set, only messages starting with
    /// this phrase (case-insensitive) are processed by the poller.
    /// Get whether the iMessage poller is enabled.
    pub fn get_imessage_poller_enabled() -> bool {
        get_cached().imessage_poller_enabled
    }

    /// Enable or disable the iMessage background poller.
    pub fn set_imessage_poller_enabled(enabled: bool) -> Result<(), String> {
        let mut data = get_cached();
        data.imessage_poller_enabled = enabled;
        save_to_backend(&data)
    }

    /// Get the last processed chat.db ROWID for the iMessage poller.
    pub fn get_imessage_last_rowid() -> i64 {
        get_cached().imessage_last_rowid
    }

    /// Persist the last processed chat.db ROWID.
    pub fn set_imessage_last_rowid(rowid: i64) -> Result<(), String> {
        let mut data = get_cached();
        data.imessage_last_rowid = rowid;
        save_to_backend(&data)
    }

    // ---- Guidance ----

    /// Format a `## Reminder List Guidance` section for the agent system prompt.
    pub fn format_reminders_guidance_section() -> String {
        let data = get_cached();
        let lines: Vec<String> = data
            .reminder_lists
            .iter()
            .filter(|(_, e)| e.enabled)
            .filter_map(|(name, e)| {
                e.guidance
                    .as_ref()
                    .filter(|g| !g.trim().is_empty())
                    .map(|g| format!("- **{}**: {}", name, g))
            })
            .collect();

        if lines.is_empty() {
            return String::new();
        }

        format!(
            "\n## Reminder List Guidance\nUse this guidance to choose the correct Reminder list:\n{}\n",
            lines.join("\n")
        )
    }

    /// Format a `## Allowed Contact Sources` section for the agent system prompt.
    pub fn format_contacts_guidance_section() -> String {
        let data = get_cached();
        if data.contact_sources.is_empty() {
            return String::new();
        }

        let entries: Vec<String> = data
            .contact_sources
            .iter()
            .filter(|(_, e)| e.enabled)
            .map(|(name, e)| {
                let access = if e.writable { "read-write" } else { "read-only" };
                let source_info = if e.source_type == "group" {
                    if let Some(ref container) = e.container_name {
                        format!(" (group in {})", container)
                    } else {
                        " (group)".to_string()
                    }
                } else {
                    " (container)".to_string()
                };
                let guidance_suffix = e
                    .guidance
                    .as_ref()
                    .filter(|g| !g.trim().is_empty())
                    .map(|g| format!(" — {}", g))
                    .unwrap_or_default();
                format!(
                    "- **{}**{}: {}{}", name, source_info, access, guidance_suffix
                )
            })
            .collect();

        if entries.is_empty() {
            return String::new();
        }

        format!(
            "\n## Allowed Contact Sources\nThese are the contact accounts and groups you can access. Respect the access mode (read-only means no create/edit/delete).\nIMPORTANT: When creating/editing contacts in a **group**, always use the `group` parameter (not `container`). The system resolves the correct container automatically from the group.\n{}\n",
            entries.join("\n")
        )
    }

    // ---- Migration ----

    /// Migrate data from legacy stores into the unified Keychain entry.
    ///
    /// Reads:
    /// - Old per-list Keychain entries (index key + individual entries)
    /// - Old disk JSON file (`granted_folders.json`)
    ///
    /// Idempotent: skips if the unified store already has data.
    /// After migration, renames the JSON file to `.migrated` and deletes
    /// old Keychain entries.
    pub fn migrate_legacy_data() {
        let data = get_cached();
        if !data.reminder_lists.is_empty() || !data.file_folders.is_empty() {
            tracing::debug!("AccessStore: unified store already has data, skipping migration");
            return;
        }

        let mut migrated = AccessData::default();
        let mut did_migrate = false;

        // --- Migrate legacy reminder list Keychain entries ---
        let index_key = "allowed-reminder-lists-index";
        let old_names: Vec<String> = match keyring::Entry::new(SERVICE, index_key) {
            Ok(entry) => match entry.get_password() {
                Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
                Err(_) => Vec::new(),
            },
            Err(_) => Vec::new(),
        };

        for name in &old_names {
            let acct = format!("allowed-reminder-list.{}", name);
            if let Ok(entry) = keyring::Entry::new(SERVICE, &acct) {
                if let Ok(json) = entry.get_password() {
                    // Parse the old per-list entry format
                    if let Ok(old) = serde_json::from_str::<serde_json::Value>(&json) {
                        migrated.reminder_lists.insert(
                            name.clone(),
                            ReminderListEntry {
                                enabled: old.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                                allowed_at: old
                                    .get("allowed_at")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                guidance: old
                                    .get("guidance")
                                    .and_then(|v| v.as_str())
                                    .filter(|s| !s.is_empty())
                                    .map(|s| s.to_string()),
                            },
                        );
                        did_migrate = true;
                    }
                }
            }
        }

        // --- Migrate legacy granted_folders.json ---
        let cache_dir = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        let json_path = cache_dir.join("prolog-router").join("granted_folders.json");
        if json_path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&json_path) {
                if let Ok(folders) = serde_json::from_str::<Vec<GrantedFolder>>(&contents) {
                    for folder in folders {
                        let key = folder.path.to_string_lossy().to_string();
                        migrated.file_folders.insert(
                            key,
                            FileFolderEntry {
                                enabled: folder.enabled,
                                allowed_at: folder.granted_at.clone(),
                                display_name: folder.display_name.clone(),
                                writable: folder.writable,
                            },
                        );
                    }
                    did_migrate = true;
                }
            }
        }

        if !did_migrate {
            tracing::debug!("AccessStore: no legacy data found to migrate");
            return;
        }

        // Write unified entry
        if let Err(e) = save_to_keychain(&migrated) {
            tracing::warn!(error = %e, "AccessStore: failed to save migrated data");
            return;
        }

        tracing::info!(
            reminder_lists = migrated.reminder_lists.len(),
            file_folders = migrated.file_folders.len(),
            "AccessStore: migrated legacy data to unified store"
        );

        // Clean up: rename JSON file
        if json_path.exists() {
            let migrated_path = json_path.with_extension("json.migrated");
            if let Err(e) = std::fs::rename(&json_path, &migrated_path) {
                tracing::warn!(error = %e, "AccessStore: failed to rename old granted_folders.json");
            }
        }

        // Clean up: delete old Keychain entries
        for name in &old_names {
            let acct = format!("allowed-reminder-list.{}", name);
            if let Ok(entry) = keyring::Entry::new(SERVICE, &acct) {
                let _ = entry.delete_credential();
            }
        }
        if let Ok(entry) = keyring::Entry::new(SERVICE, index_key) {
            let _ = entry.delete_credential();
        }
    }

    // ---- Cache Seeding (for MCP server) ----

    /// Seed the process-level cache with pre-built access data.
    ///
    /// This populates the cache without writing to Keychain or any backend.
    /// Used by the MCP server to bridge its TOML-based `AccessConfig` into the
    /// core `AccessStore` so that core-level validation (e.g., `validate_source_access`
    /// in `apple_contacts.rs`) sees the same restrictions.
    ///
    /// Must be called at startup before any tool dispatch.
    pub fn seed_cache_from_lists(
        reminder_lists: Vec<(String, bool)>,        // (name, writable)
        contact_sources: Vec<(String, String, bool)>, // (name, source_type, writable)
    ) {
        let now = chrono::Utc::now().to_rfc3339();
        let mut data = AccessData::default();

        for (name, writable) in reminder_lists {
            data.reminder_lists.insert(
                name,
                ReminderListEntry {
                    enabled: true,
                    allowed_at: now.clone(),
                    guidance: None,
                },
            );
            // Writable flag is on the entry — but ReminderListEntry doesn't have one,
            // so writable reminders are enforced at the MCP filter layer only.
            let _ = writable; // suppress unused warning
        }

        for (name, source_type, writable) in contact_sources {
            data.contact_sources.insert(
                name,
                ContactSourceEntry {
                    enabled: true,
                    allowed_at: now.clone(),
                    source_type,
                    container_name: None,
                    guidance: None,
                    writable,
                },
            );
        }

        let mut cache = CACHE.lock().unwrap();
        *cache = Some(data);
        tracing::info!("AccessStore: cache seeded from external config");
    }

    // ---- Test Helpers ----

    /// Inject data directly into the cache (for tests that don't touch Keychain).
    #[cfg(test)]
    pub(crate) fn inject_cache(data: AccessData) {
        let mut cache = CACHE.lock().unwrap();
        *cache = Some(data);
    }

    /// Inject an empty AccessData into the cache (for cross-module tests).
    #[cfg(test)]
    pub(crate) fn inject_empty_cache() {
        Self::inject_cache(AccessData::default());
    }

    /// Clear the cache (for tests).
    #[cfg(test)]
    pub(crate) fn clear_cache() {
        let mut cache = CACHE.lock().unwrap();
        *cache = None;
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an AccessData with some reminder lists for testing.
    fn sample_data() -> AccessData {
        let mut data = AccessData::default();
        data.reminder_lists.insert(
            "Shopping".to_string(),
            ReminderListEntry {
                enabled: true,
                allowed_at: "2026-01-15T10:00:00Z".to_string(),
                guidance: Some("Groceries and household items".to_string()),
            },
        );
        data.reminder_lists.insert(
            "Work".to_string(),
            ReminderListEntry {
                enabled: true,
                allowed_at: "2026-01-16T10:00:00Z".to_string(),
                guidance: None,
            },
        );
        data.reminder_lists.insert(
            "Disabled".to_string(),
            ReminderListEntry {
                enabled: false,
                allowed_at: "2026-01-17T10:00:00Z".to_string(),
                guidance: Some("Should not appear".to_string()),
            },
        );
        data
    }

    // All cache-dependent tests use CACHE_TEST_MUTEX to avoid parallel races
    // on the shared static CACHE.

    #[test]
    fn test_cache_operations() {
        let _lock = CACHE_TEST_MUTEX.lock().unwrap();
        // --- Reminder list operations with restrictions ---
        {
            let data = sample_data();
            AccessStore::inject_cache(data);

            // list_reminder_lists
            let lists = AccessStore::list_reminder_lists().unwrap();
            assert_eq!(lists.len(), 3);
            assert_eq!(lists[0].name, "Disabled"); // BTreeMap sorted
            assert_eq!(lists[1].name, "Shopping");
            assert_eq!(lists[2].name, "Work");

            // is_reminder_list_allowed — with restrictions configured
            assert!(AccessStore::is_reminder_list_allowed("Shopping"));
            assert!(AccessStore::is_reminder_list_allowed("Work"));
            assert!(!AccessStore::is_reminder_list_allowed("Disabled")); // disabled
            assert!(!AccessStore::is_reminder_list_allowed("Unknown")); // not in set

            // has_reminder_restrictions
            assert!(AccessStore::has_reminder_restrictions());

            // enabled_names
            let names = AccessStore::reminder_list_enabled_names();
            assert_eq!(names, vec!["Shopping", "Work"]);

            // format_reminders_guidance_section
            let section = AccessStore::format_reminders_guidance_section();
            assert!(section.contains("## Reminder List Guidance"));
            assert!(section.contains("**Shopping**: Groceries and household items"));
            assert!(!section.contains("Should not appear")); // disabled list
            assert!(!section.contains("**Work**")); // no guidance
        }

        // --- Empty store allows all ---
        {
            AccessStore::inject_cache(AccessData::default());

            assert!(AccessStore::is_reminder_list_allowed("anything"));
            assert!(!AccessStore::has_reminder_restrictions());
            assert!(AccessStore::reminder_list_enabled_names().is_empty());

            let section = AccessStore::format_reminders_guidance_section();
            assert!(section.is_empty());
        }

        // --- Folder operations ---
        {
            let dir = tempfile::tempdir().unwrap();
            let canonical = dir.path().canonicalize().unwrap();
            let key = canonical.to_string_lossy().to_string();

            let mut data = AccessData::default();
            data.file_folders.insert(
                key.clone(),
                FileFolderEntry {
                    enabled: true,
                    allowed_at: "2026-02-01T10:00:00Z".to_string(),
                    display_name: "TestDir".to_string(),
                    writable: false,
                },
            );
            AccessStore::inject_cache(data);

            let folders = AccessStore::list_folders().unwrap();
            assert_eq!(folders.len(), 1);
            assert_eq!(folders[0].display_name, "TestDir");
            assert!(folders[0].enabled);

            let enabled = AccessStore::enabled_folders();
            assert_eq!(enabled.len(), 1);
        }

        // --- Disabled folder not in enabled ---
        {
            let mut data = AccessData::default();
            data.file_folders.insert(
                "/some/path".to_string(),
                FileFolderEntry {
                    enabled: false,
                    allowed_at: "2026-02-01T10:00:00Z".to_string(),
                    display_name: "Disabled".to_string(),
                    writable: false,
                },
            );
            AccessStore::inject_cache(data);

            let all = AccessStore::list_folders().unwrap();
            assert_eq!(all.len(), 1);
            let enabled = AccessStore::enabled_folders();
            assert!(enabled.is_empty());
        }

        // --- Contact source operations with restrictions ---
        {
            let mut data = AccessData::default();
            data.contact_sources.insert(
                "iCloud".to_string(),
                ContactSourceEntry {
                    enabled: true,
                    allowed_at: "2026-03-01T10:00:00Z".to_string(),
                    source_type: "container".to_string(),
                    container_name: None,
                    guidance: Some("Primary contacts".to_string()),
                    writable: true,
                },
            );
            data.contact_sources.insert(
                "Work".to_string(),
                ContactSourceEntry {
                    enabled: true,
                    allowed_at: "2026-03-01T10:00:00Z".to_string(),
                    source_type: "group".to_string(),
                    container_name: Some("Google".to_string()),
                    guidance: None,
                    writable: false,
                },
            );
            data.contact_sources.insert(
                "Disabled".to_string(),
                ContactSourceEntry {
                    enabled: false,
                    allowed_at: "2026-03-01T10:00:00Z".to_string(),
                    source_type: "container".to_string(),
                    container_name: None,
                    guidance: Some("Should not appear".to_string()),
                    writable: false,
                },
            );
            AccessStore::inject_cache(data);

            // list_contact_sources
            let sources = AccessStore::list_contact_sources().unwrap();
            assert_eq!(sources.len(), 3);
            assert_eq!(sources[0].name, "Disabled"); // BTreeMap sorted
            assert_eq!(sources[1].name, "Work");
            assert_eq!(sources[1].source_type, "group");
            assert_eq!(sources[1].container_name.as_deref(), Some("Google"));
            assert_eq!(sources[2].name, "iCloud");

            // is_contact_source_allowed
            assert!(AccessStore::is_contact_source_allowed("iCloud"));
            assert!(AccessStore::is_contact_source_allowed("Work"));
            assert!(!AccessStore::is_contact_source_allowed("Disabled"));
            assert!(!AccessStore::is_contact_source_allowed("Unknown"));

            // has_contact_restrictions
            assert!(AccessStore::has_contact_restrictions());

            // enabled_names
            let names = AccessStore::contact_source_enabled_names();
            assert_eq!(names, vec!["Work", "iCloud"]);

            // is_contact_source_writable
            assert!(AccessStore::is_contact_source_writable("iCloud"));
            assert!(!AccessStore::is_contact_source_writable("Work")); // read-only
            assert!(!AccessStore::is_contact_source_writable("Disabled")); // disabled

            // format_contacts_guidance_section
            let section = AccessStore::format_contacts_guidance_section();
            assert!(section.contains("## Allowed Contact Sources"));
            assert!(section.contains("**iCloud** (container): read-write"));
            assert!(section.contains("Primary contacts"));
            assert!(section.contains("**Work** (group in Google): read-only"));
            assert!(!section.contains("Should not appear")); // disabled
        }

        // --- Empty contact sources allows all ---
        {
            AccessStore::inject_cache(AccessData::default());

            assert!(AccessStore::is_contact_source_allowed("anything"));
            assert!(AccessStore::is_contact_source_writable("anything"));
            assert!(!AccessStore::has_contact_restrictions());
            assert!(AccessStore::contact_source_enabled_names().is_empty());

            let section = AccessStore::format_contacts_guidance_section();
            assert!(section.is_empty());
        }

        AccessStore::clear_cache();
    }

    #[test]
    fn test_serialization_roundtrip() {
        let data = sample_data();
        let json = serde_json::to_string(&data).unwrap();
        let parsed: AccessData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.reminder_lists.len(), 3);
        assert_eq!(
            parsed.reminder_lists["Shopping"].guidance.as_deref(),
            Some("Groceries and household items")
        );
    }

    #[test]
    fn test_contact_sources_serialization_roundtrip() {
        let mut data = AccessData::default();
        data.contact_sources.insert(
            "iCloud".to_string(),
            ContactSourceEntry {
                enabled: true,
                allowed_at: "2026-03-01T10:00:00Z".to_string(),
                source_type: "container".to_string(),
                container_name: None,
                guidance: Some("My contacts".to_string()),
                writable: true,
            },
        );
        data.contact_sources.insert(
            "Family".to_string(),
            ContactSourceEntry {
                enabled: true,
                allowed_at: "2026-03-01T10:00:00Z".to_string(),
                source_type: "group".to_string(),
                container_name: Some("iCloud".to_string()),
                guidance: None,
                writable: false,
            },
        );
        let json = serde_json::to_string(&data).unwrap();
        let parsed: AccessData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.contact_sources.len(), 2);
        assert_eq!(parsed.contact_sources["iCloud"].source_type, "container");
        assert!(parsed.contact_sources["iCloud"].writable);
        assert_eq!(parsed.contact_sources["iCloud"].guidance.as_deref(), Some("My contacts"));
        assert_eq!(parsed.contact_sources["Family"].source_type, "group");
        assert_eq!(parsed.contact_sources["Family"].container_name.as_deref(), Some("iCloud"));
        assert!(!parsed.contact_sources["Family"].writable);
    }

    #[test]
    fn test_deserialize_empty_json() {
        let parsed: AccessData = serde_json::from_str("{}").unwrap();
        assert!(parsed.reminder_lists.is_empty());
        assert!(parsed.file_folders.is_empty());
        assert!(parsed.contact_sources.is_empty());
    }

    #[test]
    fn test_keychain_roundtrip() {
        let _lock = CACHE_TEST_MUTEX.lock().unwrap();
        // Test write→read roundtrip via the cache-only path
        // (real Keychain is bypassed unless PROLOG_ROUTER_TEST_USE_KEYCHAIN=1)
        let mut data = AccessData::default();
        data.reminder_lists.insert(
            "TestList".to_string(),
            ReminderListEntry {
                enabled: true,
                allowed_at: "2026-01-20T10:00:00Z".to_string(),
                guidance: Some("Test guidance".to_string()),
            },
        );

        AccessStore::clear_cache();
        save_to_keychain(&data).expect("save should succeed");

        // save_to_keychain updates the cache; verify via get_cached()
        let loaded = get_cached();
        assert_eq!(loaded.reminder_lists.len(), 1);
        assert_eq!(loaded.reminder_lists["TestList"].guidance.as_deref(), Some("Test guidance"));

        // Clean up
        AccessStore::clear_cache();
    }
}
