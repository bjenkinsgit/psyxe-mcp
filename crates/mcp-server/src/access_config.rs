//! Access control configuration for the MCP server.
//!
//! Stored as TOML at `~/.psyxe/access.toml`. When a section is absent,
//! full access is granted (backward compatible). When `allowed_*` is an
//! empty list, access is denied for that category.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// Default config file location: `~/.psyxe/access.toml`
pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".psyxe")
        .join("access.toml")
}

/// Top-level access configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccessConfig {
    pub reminders: Option<ReminderAccess>,
    pub contacts: Option<ContactAccess>,
    pub notes: Option<NoteAccess>,
    pub files: Option<FileAccess>,
}

/// Reminder list access control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReminderAccess {
    /// Lists the MCP server can read. Empty = no access.
    #[serde(default)]
    pub allowed_lists: Vec<String>,
    /// Lists the MCP server can write to. Must be a subset of `allowed_lists`.
    #[serde(default)]
    pub writable_lists: Vec<String>,
}

/// Contact group/container access control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactAccess {
    /// Groups or containers the MCP server can read. Empty = no access.
    #[serde(default)]
    pub allowed_groups: Vec<String>,
    /// Groups or containers the MCP server can write to.
    #[serde(default)]
    pub writable_groups: Vec<String>,
}

/// Notes folder access control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteAccess {
    /// Folders the MCP server can read. Empty = no access.
    #[serde(default)]
    pub allowed_folders: Vec<String>,
    /// Folders the MCP server can write to.
    #[serde(default)]
    pub writable_folders: Vec<String>,
}

/// File/folder access control (absolute paths on disk).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAccess {
    /// Folders the MCP server can read files from. Empty = no access.
    /// Uses absolute paths (e.g., "/Users/alice/Documents/project").
    #[serde(default)]
    pub allowed_folders: Vec<String>,
    /// Folders the MCP server can write files to.
    #[serde(default)]
    pub writable_folders: Vec<String>,
}

#[allow(dead_code)] // Individual check methods reserved for future use (e.g., biometric v2)
impl AccessConfig {
    /// Load from disk. Returns default (full access) if file doesn't exist.
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let config: Self = toml::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        Ok(config)
    }

    /// Save to disk, creating parent directories if needed.
    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        let content = toml::to_string_pretty(self)
            .context("Failed to serialize access config")?;
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    /// Check if a reminder list is readable.
    pub fn can_read_reminder_list(&self, list: &str) -> bool {
        match &self.reminders {
            None => true, // No restrictions configured
            Some(access) => access.allowed_lists.iter().any(|l| l.eq_ignore_ascii_case(list)),
        }
    }

    /// Check if a reminder list is writable.
    pub fn can_write_reminder_list(&self, list: &str) -> bool {
        match &self.reminders {
            None => true,
            Some(access) => access.writable_lists.iter().any(|l| l.eq_ignore_ascii_case(list)),
        }
    }

    /// Check if a contact group/container is readable.
    pub fn can_read_contact_group(&self, group: &str) -> bool {
        match &self.contacts {
            None => true,
            Some(access) => access.allowed_groups.iter().any(|g| g.eq_ignore_ascii_case(group)),
        }
    }

    /// Check if a contact group/container is writable.
    pub fn can_write_contact_group(&self, group: &str) -> bool {
        match &self.contacts {
            None => true,
            Some(access) => access.writable_groups.iter().any(|g| g.eq_ignore_ascii_case(group)),
        }
    }

    /// Check if a notes folder is readable.
    pub fn can_read_notes_folder(&self, folder: &str) -> bool {
        match &self.notes {
            None => true,
            Some(access) => access.allowed_folders.iter().any(|f| f.eq_ignore_ascii_case(folder)),
        }
    }

    /// Check if a notes folder is writable.
    pub fn can_write_notes_folder(&self, folder: &str) -> bool {
        match &self.notes {
            None => true,
            Some(access) => access.writable_folders.iter().any(|f| f.eq_ignore_ascii_case(folder)),
        }
    }

    /// Get all allowed reminder lists as a set (for filtering).
    pub fn allowed_reminder_lists(&self) -> Option<HashSet<String>> {
        self.reminders.as_ref().map(|r| {
            r.allowed_lists.iter().map(|s| s.to_lowercase()).collect()
        })
    }

    /// Get all allowed contact groups as a set (for filtering).
    pub fn allowed_contact_groups(&self) -> Option<HashSet<String>> {
        self.contacts.as_ref().map(|c| {
            c.allowed_groups.iter().map(|s| s.to_lowercase()).collect()
        })
    }

    /// Get all allowed notes folders as a set (for filtering).
    pub fn allowed_notes_folders(&self) -> Option<HashSet<String>> {
        self.notes.as_ref().map(|n| {
            n.allowed_folders.iter().map(|s| s.to_lowercase()).collect()
        })
    }

    /// Check if a file path is within any allowed folder (read access).
    pub fn can_read_file(&self, path: &str) -> bool {
        match &self.files {
            None => true, // No restrictions configured
            Some(access) => access
                .allowed_folders
                .iter()
                .any(|folder| is_path_under(path, folder)),
        }
    }

    /// Check if a file path is within any writable folder.
    pub fn can_write_file(&self, path: &str) -> bool {
        match &self.files {
            None => true,
            Some(access) => access
                .writable_folders
                .iter()
                .any(|folder| is_path_under(path, folder)),
        }
    }

    /// Get all allowed file folders (for pre-call info).
    pub fn allowed_file_folders(&self) -> Option<&[String]> {
        self.files.as_ref().map(|f| f.allowed_folders.as_slice())
    }

    /// Check if file access restrictions are active.
    pub fn has_file_restrictions(&self) -> bool {
        self.files.is_some()
    }
}

/// Check if `path` is under `folder` (prefix match on canonicalized paths).
pub fn is_path_under(path: &str, folder: &str) -> bool {
    // Normalize: ensure folder ends with / for prefix matching
    let folder_prefix = if folder.ends_with('/') {
        folder.to_string()
    } else {
        format!("{}/", folder)
    };
    // Path is under folder if it starts with folder/ or equals folder exactly
    path.starts_with(&folder_prefix) || path == folder
}
