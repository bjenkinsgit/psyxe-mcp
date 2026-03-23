//! Memvid-powered Notes Semantic Search
//!
//! Replaces the slow AppleScript-based Notes search with semantic BERT-based search
//! using memvid-rs. Notes content is indexed into a QR-encoded MP4 video with a
//! SQLite vector index for fast semantic retrieval.
//!
//! This module requires the `memvid` feature to be enabled:
//!   cargo build --features memvid
//!
//! FFmpeg must be installed on the system (brew install ffmpeg).
//!
//! Storage locations:
//! - ~/.cache/prolog-router/apple_notes.mp4       - QR-encoded note content
//! - ~/.cache/prolog-router/apple_notes_index.db  - SQLite vector index
//! - ~/.cache/prolog-router/apple_notes_meta.json - Sync metadata for staleness

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[cfg(feature = "memvid")]
use memvid_rs::{Config, MemvidEncoder, MemvidRetriever};

/// Convert string log level to ffmpeg_next::log::Level
#[cfg(feature = "memvid")]
fn parse_ffmpeg_log_level(level: &str) -> ffmpeg_next::log::Level {
    match level.to_lowercase().as_str() {
        "quiet" => ffmpeg_next::log::Level::Quiet,
        "panic" => ffmpeg_next::log::Level::Panic,
        "fatal" => ffmpeg_next::log::Level::Fatal,
        "error" => ffmpeg_next::log::Level::Error,
        "warning" => ffmpeg_next::log::Level::Warning,
        "info" => ffmpeg_next::log::Level::Info,
        "verbose" => ffmpeg_next::log::Level::Verbose,
        "debug" => ffmpeg_next::log::Level::Debug,
        "trace" => ffmpeg_next::log::Level::Trace,
        _ => ffmpeg_next::log::Level::Error, // Default to error
    }
}

/// Initialize FFmpeg with configurable log verbosity
#[cfg(feature = "memvid")]
fn init_ffmpeg_quiet() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let config = load_config();
        ffmpeg_next::init().ok();
        let level = parse_ffmpeg_log_level(&config.ffmpeg.library_log_level);
        ffmpeg_next::log::set_level(level);
    });
}

/// Get the FFmpeg config (for use by other modules)
pub fn get_ffmpeg_config() -> FfmpegConfig {
    load_config().ffmpeg
}

/// Get the full memvid config (for use by other modules)
pub fn get_full_config() -> MemvidConfig {
    load_config()
}

/// Find the path to memvid_config.toml, checking resource dir, cwd, then workspace root.
fn find_config_path() -> Option<PathBuf> {
    let mut candidates = vec![
        std::env::current_dir()
            .map(|p| p.join("memvid_config.toml"))
            .unwrap_or_else(|_| PathBuf::from("memvid_config.toml")),
        // Two levels up from crates/core — reaches workspace root
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../memvid_config.toml"),
    ];
    // Bundled app: check Contents/Resources/ (set by Tauri setup)
    if let Ok(dir) = std::env::var("SCRIPTS_DIR_OVERRIDE") {
        candidates.insert(0, PathBuf::from(dir).join("memvid_config.toml"));
    }
    // Also check exe-relative for bundled macOS app
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.insert(0, dir.join("../Resources/memvid_config.toml"));
        }
    }
    candidates.into_iter().find(|p| p.exists())
}

/// Find a bundled ffmpeg binary, using the same search locations as Swift helpers.
#[cfg(feature = "memvid")]
fn find_bundled_ffmpeg() -> Option<PathBuf> {
    // 1. SCRIPTS_DIR_OVERRIDE (Tauri app bundle Resources dir)
    if let Some(override_dir) = std::env::var_os("SCRIPTS_DIR_OVERRIDE") {
        let p = PathBuf::from(override_dir).join("ffmpeg");
        if p.is_file() {
            return Some(p);
        }
    }

    // 2. Adjacent to the running executable (Contents/MacOS/../Resources/ffmpeg)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let p = exe_dir.join("../Resources/ffmpeg");
            if p.is_file() {
                return Some(p.canonicalize().unwrap_or(p));
            }
        }
    }

    None
}

/// Apply remote embedding API settings to a memvid_rs Config when provider is "remote".
/// Reads EMBEDDING_API_URL and EMBEDDING_API_MODEL from environment.
#[cfg(feature = "memvid")]
fn apply_remote_embedding_config(config: &mut Config, app_config: &MemvidConfig) {
    if app_config.ml.embedding_provider == "remote" {
        if let Ok(url) = std::env::var("EMBEDDING_API_URL") {
            tracing::info!(url = %url, "Using remote embedding API");
            config.ml.embedding_api_url = Some(url);
        } else {
            tracing::warn!("embedding_provider=remote but EMBEDDING_API_URL not set, falling back to local");
        }
        if let Ok(model) = std::env::var("EMBEDDING_API_MODEL") {
            config.ml.embedding_api_model = Some(model);
        }
    }
}

/// Build a memvid_rs::Config from memvid_config.toml, applying ffmpeg and
/// remote embedding overrides. The TOML maps directly to memvid_rs::Config
/// for [chunking], [ml], and [qr] sections; [ffmpeg] fields are remapped
/// to Config.video since the section names differ.
#[cfg(feature = "memvid")]
pub fn build_memvid_rs_config() -> Config {
    let app_config = load_config();
    let mut config = match find_config_path() {
        Some(path) => Config::from_toml_file(&path).unwrap_or_else(|e| {
            tracing::warn!(?path, error = %e, "Failed to parse memvid_config.toml as memvid_rs::Config, using defaults");
            Config::default()
        }),
        None => Config::default(),
    };

    // [ffmpeg]/[video] section maps to config.video (different section names)
    config.video.prores_profile = app_config.ffmpeg.prores_profile.clone();
    config.video.x265_log_level = app_config.ffmpeg.x265_log_level.clone();
    config.video.ffmpeg_cli_log_level = app_config.ffmpeg.cli_log_level.clone();
    config.video.ffmpeg_hide_banner = app_config.ffmpeg.hide_banner;
    config.video.apply_x265_log_level();

    // max_chunk_size should match chunk_size for QR capacity alignment
    config.chunking.max_chunk_size = config.chunking.chunk_size;

    // Auto-detect bundled ffmpeg binary when running from app bundle
    if config.video.ffmpeg_path.is_none() {
        if let Some(path) = find_bundled_ffmpeg() {
            tracing::info!(path = %path.display(), "Using bundled ffmpeg binary");
            config.video.ffmpeg_path = Some(path.to_string_lossy().into_owned());
        }
    }

    // Remote embedding API (when embedding_provider = "remote")
    apply_remote_embedding_config(&mut config, &app_config);
    config
}

#[allow(unused_imports)]
use crate::apple_notes::{self, NoteContent};

#[cfg(feature = "memvid")]
use crate::apple_notes::IndexedNote;

// ============================================================================
// Runtime Configuration
// ============================================================================

/// Runtime configuration loaded from memvid_config.toml
#[derive(Debug, Clone, Deserialize)]
pub struct MemvidConfig {
    #[serde(default)]
    pub chunking: ChunkingConfig,
    #[serde(default)]
    pub ml: MlConfig,
    #[serde(default)]
    pub qr: QrConfig,
    #[serde(default)]
    pub metadata: MetadataConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub ffmpeg: FfmpegConfig,
    #[serde(default)]
    pub search: SearchConfig,
}

/// FFmpeg/video configuration
#[derive(Debug, Clone, Deserialize)]
pub struct FfmpegConfig {
    /// FFmpeg library log level (swscaler, etc.)
    #[serde(default = "default_library_log_level")]
    pub library_log_level: String,
    /// FFmpeg CLI log level
    #[serde(default = "default_cli_log_level")]
    pub cli_log_level: String,
    /// Hide FFmpeg CLI banner
    #[serde(default = "default_hide_banner")]
    pub hide_banner: bool,
    /// ProRes profile: proxy, lt, standard, hq, 4444, xq
    #[serde(default = "default_prores_profile")]
    pub prores_profile: String,
    /// x265 encoder log level (legacy, kept for backwards compatibility)
    #[serde(default = "default_x265_log_level")]
    pub x265_log_level: String,
}

fn default_library_log_level() -> String { "error".to_string() }
fn default_cli_log_level() -> String { "error".to_string() }
fn default_hide_banner() -> bool { true }
fn default_prores_profile() -> String { "proxy".to_string() }
fn default_x265_log_level() -> String { "error".to_string() }

impl Default for FfmpegConfig {
    fn default() -> Self {
        Self {
            library_log_level: default_library_log_level(),
            cli_log_level: default_cli_log_level(),
            hide_banner: default_hide_banner(),
            prores_profile: default_prores_profile(),
            x265_log_level: default_x265_log_level(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChunkingConfig {
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_overlap")]
    pub overlap: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MlConfig {
    #[serde(default = "default_device")]
    pub device: String,
    /// Optional BERT model name override (HuggingFace model ID).
    /// When None, uses the memvid-rs default.
    #[serde(default)]
    pub model_name: Option<String>,
    /// Batch size for BERT embedding generation.
    /// Smaller values use less GPU memory. Default: 32.
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    /// Embedding provider: "local" (BERT on device) or "remote" (OpenAI-compatible API).
    /// When "remote", reads EMBEDDING_API_URL and EMBEDDING_API_MODEL from environment.
    #[serde(default = "default_embedding_provider")]
    pub embedding_provider: String,
    /// Prefix prepended to search queries for asymmetric/instruction-tuned models.
    /// Empty by default (no prefix for symmetric models).
    #[serde(default)]
    pub embedding_query_prefix: Option<String>,
    /// Prefix prepended to document chunks during indexing.
    /// Empty by default (most models don't need this).
    #[serde(default)]
    pub embedding_document_prefix: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QrConfig {
    #[serde(default = "default_error_correction")]
    pub error_correction: String,
    #[serde(default)]
    pub version: Option<i16>,
    #[serde(default = "default_enable_compression")]
    pub enable_compression: bool,
    #[serde(default = "default_compression_threshold")]
    pub compression_threshold: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetadataConfig {
    #[serde(default = "default_metadata_strategy")]
    pub strategy: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheConfig {
    #[serde(default = "default_enable_notes_cache")]
    pub enable_notes_cache: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchConfig {
    /// Minimum semantic similarity score (0.0–1.0) for results to be included.
    /// Results below this threshold are filtered out. Default: 0.4
    #[serde(default = "default_min_score")]
    pub min_score: f32,
}

fn default_enable_compression() -> bool { true }
fn default_compression_threshold() -> usize { 100 }
fn default_enable_notes_cache() -> bool { true }
fn default_min_score() -> f32 { 0.4 }

fn default_batch_size() -> usize { 32 }
fn default_embedding_provider() -> String { "local".to_string() }
fn default_chunk_size() -> usize { 500 }
fn default_overlap() -> usize { 100 }
fn default_device() -> String { "metal".to_string() }
fn default_error_correction() -> String { "low".to_string() }
fn default_metadata_strategy() -> String { "indexed".to_string() }

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self { chunk_size: default_chunk_size(), overlap: default_overlap() }
    }
}

impl Default for MlConfig {
    fn default() -> Self { Self { device: default_device(), model_name: None, batch_size: default_batch_size(), embedding_provider: default_embedding_provider(), embedding_query_prefix: None, embedding_document_prefix: None } }
}

impl Default for QrConfig {
    fn default() -> Self {
        Self {
            error_correction: default_error_correction(),
            version: None,
            enable_compression: default_enable_compression(),
            compression_threshold: default_compression_threshold(),
        }
    }
}

impl Default for CacheConfig {
    fn default() -> Self { Self { enable_notes_cache: default_enable_notes_cache() } }
}

impl Default for MetadataConfig {
    fn default() -> Self { Self { strategy: default_metadata_strategy() } }
}

impl Default for SearchConfig {
    fn default() -> Self { Self { min_score: default_min_score() } }
}

impl Default for MemvidConfig {
    fn default() -> Self {
        Self {
            chunking: ChunkingConfig::default(),
            ml: MlConfig::default(),
            qr: QrConfig::default(),
            metadata: MetadataConfig::default(),
            cache: CacheConfig::default(),
            ffmpeg: FfmpegConfig::default(),
            search: SearchConfig::default(),
        }
    }
}

/// Load app-level config from memvid_config.toml (or use defaults).
/// This loads the app-specific sections ([metadata], [cache], [memory], [ffmpeg], etc.)
/// that are not part of memvid_rs::Config.
fn load_config() -> MemvidConfig {
    match find_config_path() {
        Some(path) => {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(config) = toml::from_str(&content) {
                    return config;
                }
                tracing::warn!(?path, "Failed to parse memvid_config.toml, using defaults");
            }
        }
        None => {
            tracing::warn!("memvid_config.toml not found, using defaults");
        }
    }
    MemvidConfig::default()
}

/// Note metadata stored in index file for recovery during search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteMetadataEntry {
    pub note_id: String,
    pub title: String,
    pub folder: String,
    pub modified: String,
}

// ============================================================================
// File Paths
// ============================================================================

/// Directory name within cache for memvid files
const CACHE_SUBDIR: &str = "prolog-router";

/// Video file name (QR-encoded note content)
const VIDEO_FILE: &str = "apple_notes.mp4";

/// Index database file name (vector embeddings)
const INDEX_FILE: &str = "apple_notes_index.db";

/// Metadata file for staleness checking
const META_FILE: &str = "apple_notes_meta.json";

/// Note metadata index (maps chunk IDs to note info)
const NOTE_METADATA_FILE: &str = "apple_notes_metadata.json";

/// Cached notes content (for faster iteration when tuning parameters)
const NOTES_CACHE_FILE: &str = "apple_notes_cache.json";

// ============================================================================
// Data Structures
// ============================================================================

/// Result of a semantic search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotesSearchResult {
    /// Note ID (x-coredata://...) for opening in Notes.app
    pub note_id: String,
    /// Note title
    pub title: String,
    /// Folder containing the note
    pub folder: String,
    /// Relevant text snippet from the matched chunk
    pub snippet: String,
    /// Semantic similarity score (0.0 - 1.0)
    pub score: f32,
}

/// Sync metadata for staleness detection
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SyncMetadata {
    /// Number of notes when index was last built
    note_count: usize,
    /// ISO 8601 timestamp of last sync
    last_updated: String,
    /// Set of note IDs that were indexed (for deletion detection)
    #[serde(default)]
    indexed_note_ids: Vec<String>,
}

/// Search result with staleness information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultsWithStaleness {
    /// The search results (with deleted notes filtered out)
    pub results: Vec<NotesSearchResult>,
    /// Number of results that were filtered out due to deleted notes
    pub filtered_deleted_count: usize,
    /// Whether a rebuild is recommended (deleted notes were found)
    pub rebuild_recommended: bool,
}

/// Statistics about the memvid index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    /// Whether the index files exist
    pub exists: bool,
    /// Whether the index is stale (note count changed)
    pub is_stale: bool,
    /// Number of notes in the index
    pub indexed_note_count: usize,
    /// Current note count in Notes.app
    pub current_note_count: usize,
    /// When the index was last updated
    pub last_updated: String,
    /// Size of the video file in bytes
    pub video_size_bytes: u64,
    /// Size of the index database in bytes
    pub index_size_bytes: u64,
}

// ============================================================================
// Path Utilities
// ============================================================================

/// Get the cache directory for memvid files
fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(CACHE_SUBDIR)
}

/// Get the path to the video file
fn video_path() -> PathBuf {
    cache_dir().join(VIDEO_FILE)
}

/// Get the path to the index database
fn index_path() -> PathBuf {
    cache_dir().join(INDEX_FILE)
}

/// Get the path to the metadata file
fn meta_path() -> PathBuf {
    cache_dir().join(META_FILE)
}

/// Get the path to the note metadata index
fn note_metadata_path() -> PathBuf {
    cache_dir().join(NOTE_METADATA_FILE)
}

/// Get the path to the notes content cache
fn notes_cache_path() -> PathBuf {
    cache_dir().join(NOTES_CACHE_FILE)
}

/// Ensure the cache directory exists
#[cfg(feature = "memvid")]
fn ensure_cache_dir() -> Result<()> {
    let dir = cache_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| anyhow!("Failed to create cache directory {:?}: {}", dir, e))?;
    }
    Ok(())
}

// ============================================================================
// Metadata Persistence
// ============================================================================

/// Load sync metadata from disk
fn load_metadata() -> Option<SyncMetadata> {
    let path = meta_path();
    if !path.exists() {
        return None;
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// Save sync metadata to disk
#[cfg(feature = "memvid")]
fn save_metadata(meta: &SyncMetadata) -> Result<()> {
    ensure_cache_dir()?;
    let path = meta_path();
    let content = serde_json::to_string_pretty(meta)?;
    fs::write(&path, content).map_err(|e| anyhow!("Failed to write metadata: {}", e))
}

/// Save note metadata index (maps numeric index to note info)
#[cfg(feature = "memvid")]
fn save_note_metadata(metadata: &HashMap<u32, NoteMetadataEntry>) -> Result<()> {
    ensure_cache_dir()?;
    let path = note_metadata_path();
    let content = serde_json::to_string_pretty(metadata)?;
    fs::write(&path, content).map_err(|e| anyhow!("Failed to write note metadata: {}", e))
}

/// Load note metadata index from disk
fn load_note_metadata() -> Option<HashMap<u32, NoteMetadataEntry>> {
    let path = note_metadata_path();
    if !path.exists() {
        return None;
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// Save notes content cache to disk
#[cfg(feature = "memvid")]
fn save_notes_cache(cache: &HashMap<String, NoteContent>) -> Result<()> {
    ensure_cache_dir()?;
    let path = notes_cache_path();
    let content = serde_json::to_string(cache)?;  // Compact for performance
    fs::write(&path, content).map_err(|e| anyhow!("Failed to write notes cache: {}", e))
}

/// Load notes content cache from disk
#[cfg(feature = "memvid")]
fn load_notes_cache() -> Option<HashMap<String, NoteContent>> {
    let path = notes_cache_path();
    if !path.exists() {
        return None;
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

// ============================================================================
// Note Content Fetching
// ============================================================================

/// Fetch note content via direct SQLite access (with AppleScript fallback).
#[cfg(feature = "memvid")]
#[allow(dead_code)]
fn fetch_note_content(note_id: &str) -> Result<NoteContent> {
    apple_notes::get_note_content(note_id)
}

/// Batch fetch note contents via direct SQLite access.
/// Returns a HashMap of note_id -> NoteContent for successfully fetched notes.
#[cfg(feature = "memvid")]
fn fetch_notes_batch(note_ids: &[String]) -> Result<std::collections::HashMap<String, NoteContent>> {
    apple_notes::get_notes_batch_sqlite(note_ids)
}

// ============================================================================
// Index Building (memvid feature required)
// ============================================================================

/// Build the memvid index from all Apple Notes
///
/// This reads all notes from SQLite (metadata) and AppleScript (content),
/// then encodes them into a searchable video file with semantic embeddings.
#[cfg(feature = "memvid")]
pub async fn build_index() -> Result<IndexStats> {
    // Suppress FFmpeg swscaler warnings
    init_ffmpeg_quiet();

    ensure_cache_dir()?;

    // Load runtime config
    let app_config = load_config();
    tracing::info!(
        chunk_size = app_config.chunking.chunk_size,
        overlap = app_config.chunking.overlap,
        device = %app_config.ml.device,
        error_correction = %app_config.qr.error_correction,
        metadata = %app_config.metadata.strategy,
        "Loaded memvid config"
    );

    tracing::info!("Loading notes from database");
    // Always rebuild the tag index if it's stale or missing, so the
    // semantic index covers all notes — not just those from a cached scan.
    let live_count = apple_notes::get_note_count().unwrap_or(0);
    let tag_index_count = apple_notes::load_index().map(|i| i.note_count).unwrap_or(0);
    if tag_index_count != live_count {
        tracing::info!(tag_index_count, live_count, "Tag index stale, rebuilding");
        apple_notes::build_index()?;
    }
    let index = apple_notes::load_index()?;
    tracing::info!(count = index.note_count, "Notes found");

    // Collect all note IDs for batch fetching (sorted by ID for deterministic ordering —
    // HashMap iteration order is non-deterministic and would cause chunk-to-note mapping errors)
    let mut notes: Vec<&IndexedNote> = index.notes.values().collect();
    notes.sort_by(|a, b| a.id.cmp(&b.id));
    let total = notes.len();
    let note_ids: Vec<String> = notes.iter().map(|n| n.id.clone()).collect();

    // Try to load from cache first if enabled
    let fetched_notes = if app_config.cache.enable_notes_cache {
        let cached = load_notes_cache().filter(|c| {
            if c.len() == total {
                true
            } else {
                tracing::info!(cached = c.len(), expected = total, "Notes cache stale, discarding");
                let _ = std::fs::remove_file(notes_cache_path());
                false
            }
        });
        if let Some(cached) = cached {
            tracing::info!(count = cached.len(), "Using cached notes content");
            cached
        } else {
            // Batch fetch all note contents in a single AppleScript call
            tracing::info!("Fetching all note contents (batch, will cache)");
            let fetched = fetch_notes_batch(&note_ids)?;
            tracing::info!(fetched = fetched.len(), missing = total - fetched.len(), "Batch fetch complete");
            // Save to cache for next time
            if let Err(e) = save_notes_cache(&fetched) {
                tracing::warn!(error = %e, "Failed to save notes cache");
            } else {
                tracing::info!(path = %notes_cache_path().display(), "Notes content cached");
            }
            fetched
        }
    } else {
        // Batch fetch all note contents in a single AppleScript call
        tracing::info!("Fetching all note contents (batch)");
        let fetched = fetch_notes_batch(&note_ids)?;
        tracing::info!(fetched = fetched.len(), missing = total - fetched.len(), "Batch fetch complete");
        fetched
    };

    // Create encoder with config-driven settings
    tracing::info!(
        device = %app_config.ml.device,
        qr_version = app_config.qr.version.unwrap_or(0),
        "Initializing memvid encoder"
    );
    let config = build_memvid_rs_config();
    let mut encoder = MemvidEncoder::new(Some(config))
        .await
        .map_err(|e| anyhow!("Failed to create encoder: {}", e))?;
    tracing::info!("Memvid encoder initialized");

    // Build note metadata index for the "indexed" strategy
    let use_indexed = app_config.metadata.strategy == "indexed";
    let mut note_metadata_map: HashMap<u32, NoteMetadataEntry> = HashMap::new();
    let chunk_size = app_config.chunking.chunk_size;
    let overlap = app_config.chunking.overlap;

    // Pre-chunk all notes manually to have full control over chunk size
    tracing::info!(count = fetched_notes.len(), chunk_size, overlap, "Chunking notes");
    let mut all_chunks: Vec<String> = Vec::new();
    let mut note_idx: u32 = 0;

    for (i, note) in notes.iter().enumerate() {
        if (i + 1) % 50 == 0 || i + 1 == total {
            tracing::debug!(progress = i + 1, total, "Processing notes");
        }

        // Look up content from batch results
        if let Some(content) = fetched_notes.get(&note.id) {
            // Store metadata separately
            if use_indexed {
                let short_id = note.id.strip_prefix("x-coredata://").unwrap_or(&note.id).to_string();
                note_metadata_map.insert(note_idx, NoteMetadataEntry {
                    note_id: short_id,
                    title: note.title.clone(),
                    folder: note.folder.clone(),
                    modified: note.modified.clone(),
                });
            }

            // Manual chunking with short prefix
            let body = &content.body;
            let prefix = format!("N:{}\n", note_idx);
            let prefix_len = prefix.len();
            let effective_chunk_size = chunk_size.saturating_sub(prefix_len);

            if effective_chunk_size == 0 {
                tracing::warn!(chunk_size, "chunk_size too small for prefix");
                continue;
            }

            // Split body into chunks
            let body_chars: Vec<char> = body.chars().collect();
            let mut pos = 0;
            while pos < body_chars.len() {
                let end = (pos + effective_chunk_size).min(body_chars.len());
                let chunk_text: String = body_chars[pos..end].iter().collect();
                all_chunks.push(format!("{}{}", prefix, chunk_text));

                // Move forward with overlap
                pos += effective_chunk_size.saturating_sub(overlap);
                if pos >= end && end < body_chars.len() {
                    pos = end; // Prevent infinite loop
                }
            }

            // Handle empty notes
            if body.is_empty() {
                all_chunks.push(format!("{}(empty note)", prefix));
            }

            note_idx += 1;
        }
    }
    let encoded_count = note_idx as usize;
    tracing::info!(
        notes = encoded_count,
        chunks = all_chunks.len(),
        "Chunking complete"
    );

    // Add pre-chunked text to encoder using add_chunks()
    tracing::info!(count = all_chunks.len(), "Adding chunks to encoder");
    encoder
        .add_chunks(all_chunks)
        .map_err(|e| anyhow!("Failed to add chunks: {}", e))?;
    tracing::info!("Chunks added to encoder");

    // Save note metadata index if using indexed strategy
    if use_indexed {
        tracing::info!("Saving note metadata index");
        save_note_metadata(&note_metadata_map)?;
        tracing::info!(entries = note_metadata_map.len(), "Note metadata index saved");
    }

    // Build video and index files
    let vpath = video_path();
    let ipath = index_path();

    tracing::info!("Building video memory (this may take a while)");
    encoder
        .build_video(
            vpath.to_str().ok_or_else(|| anyhow!("Invalid video path"))?,
            ipath.to_str().ok_or_else(|| anyhow!("Invalid index path"))?,
        )
        .await
        .map_err(|e| anyhow!("Failed to build video: {}", e))?;
    tracing::info!("Video memory built");

    // Save sync metadata with indexed note IDs for deletion detection.
    // index.note_count is reliable here because we rebuilt the tag index
    // above whenever it diverged from the live Notes.app count.
    // Use local time (not UTC) to match AppleScript's modification dates,
    // which are local time — this keeps is_stale() comparisons consistent.
    let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let indexed_note_ids: Vec<String> = note_metadata_map
        .values()
        .map(|entry| {
            // Store full note ID for lookup
            if entry.note_id.starts_with("x-coredata://") {
                entry.note_id.clone()
            } else {
                format!("x-coredata://{}", entry.note_id)
            }
        })
        .collect();
    let meta = SyncMetadata {
        note_count: index.note_count,
        last_updated: now.clone(),
        indexed_note_ids,
    };
    save_metadata(&meta)?;

    // Get file sizes
    let video_size = fs::metadata(&vpath).map(|m| m.len()).unwrap_or(0);
    let index_size = fs::metadata(&ipath).map(|m| m.len()).unwrap_or(0);

    Ok(IndexStats {
        exists: true,
        is_stale: false,
        indexed_note_count: encoded_count,
        current_note_count: index.note_count,
        last_updated: now,
        video_size_bytes: video_size,
        index_size_bytes: index_size,
    })
}

// ============================================================================
// Staleness Detection
// ============================================================================

/// Check if the memvid index exists
#[allow(dead_code)]
pub fn index_exists() -> bool {
    video_path().exists() && index_path().exists()
}

/// Check if the index is stale (note count changed or any note modified since last build)
///
/// Queries Notes.app directly via AppleScript for the current count and
/// latest modification date, rather than relying on the cached tag index.
#[allow(dead_code)]
pub fn is_stale() -> Result<bool> {
    let meta = match load_metadata() {
        Some(m) => m,
        None => return Ok(true), // No metadata means stale
    };

    // Check 1: note count changed (additions/deletions)
    let current_count = apple_notes::get_note_count()?;
    if current_count != meta.note_count {
        return Ok(true);
    }

    // Check 2: any note modified after the last index build
    if let Ok(latest) = apple_notes::get_latest_modified() {
        if latest > meta.last_updated {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Get statistics about the memvid index
pub fn get_stats() -> Result<IndexStats> {
    let vpath = video_path();
    let ipath = index_path();

    let exists = vpath.exists() && ipath.exists();

    if !exists {
        // Try to get current note count
        let current_count = apple_notes::load_index()
            .map(|i| i.note_count)
            .unwrap_or(0);

        return Ok(IndexStats {
            exists: false,
            is_stale: true,
            indexed_note_count: 0,
            current_note_count: current_count,
            last_updated: String::new(),
            video_size_bytes: 0,
            index_size_bytes: 0,
        });
    }

    let meta = load_metadata().unwrap_or(SyncMetadata {
        note_count: 0,
        last_updated: String::new(),
        indexed_note_ids: Vec::new(),
    });

    let current_index = apple_notes::load_index()?;
    let stale = current_index.note_count != meta.note_count;

    let video_size = fs::metadata(&vpath).map(|m| m.len()).unwrap_or(0);
    let index_size = fs::metadata(&ipath).map(|m| m.len()).unwrap_or(0);

    Ok(IndexStats {
        exists: true,
        is_stale: stale,
        indexed_note_count: meta.note_count,
        current_note_count: current_index.note_count,
        last_updated: meta.last_updated,
        video_size_bytes: video_size,
        index_size_bytes: index_size,
    })
}

// ============================================================================
// Semantic Search (memvid feature required)
// ============================================================================

/// Parse note metadata from the beginning of a chunk text
/// Supports both indexed format ("N:123\n...") and inline format ("NOTE_ID: ...\n...")
/// Returns (note_id, title, folder, remaining_text)
#[allow(dead_code)]
fn parse_chunk_metadata(text: &str) -> (String, String, String, String) {
    // Check for indexed format first: "N:123\n..."
    if let Some(rest) = text.strip_prefix("N:") {
        if let Some(newline_pos) = rest.find('\n') {
            let idx_str = &rest[..newline_pos];
            if let Ok(idx) = idx_str.parse::<u32>() {
                // Look up metadata from index
                if let Some(metadata_map) = load_note_metadata() {
                    if let Some(entry) = metadata_map.get(&idx) {
                        let remaining = rest[newline_pos + 1..].to_string();
                        // Restore "x-coredata://" prefix if not already present
                        let full_note_id = if entry.note_id.starts_with("x-coredata://") {
                            entry.note_id.clone()
                        } else {
                            format!("x-coredata://{}", entry.note_id)
                        };
                        return (
                            full_note_id,
                            entry.title.clone(),
                            entry.folder.clone(),
                            remaining,
                        );
                    }
                }
            }
        }
    }

    // Fall back to inline format for backwards compatibility
    let mut note_id = String::new();
    let mut title = String::new();
    let mut folder = String::new();
    let mut remaining = text.to_string();

    for line in text.lines() {
        if let Some(id) = line.strip_prefix("NOTE_ID: ") {
            note_id = id.to_string();
        } else if let Some(t) = line.strip_prefix("TITLE: ") {
            title = t.to_string();
        } else if let Some(f) = line.strip_prefix("FOLDER: ") {
            folder = f.to_string();
        } else if line.strip_prefix("MODIFIED: ").is_some() {
            // Skip but don't break
        } else if !line.is_empty() {
            // Found content, rest is the body
            if let Some(idx) = text.find(line) {
                remaining = text[idx..].to_string();
            }
            break;
        }
    }

    (note_id, title, folder, remaining)
}

/// Create a snippet from text (first N characters with word boundary)
#[allow(dead_code)]
fn create_snippet(text: &str, max_len: usize) -> String {
    let text = text.trim();
    if text.len() <= max_len {
        return text.to_string();
    }

    // Find last space before max_len
    let truncated = &text[..max_len];
    if let Some(last_space) = truncated.rfind(' ') {
        format!("{}...", &text[..last_space])
    } else {
        format!("{}...", truncated)
    }
}

/// Perform semantic search against the memvid index
///
/// Returns notes ranked by semantic similarity to the query.
/// Note: Use `search_with_validation` for results with deleted note filtering.
#[cfg(feature = "memvid")]
pub async fn search(query: &str, top_k: usize) -> Result<Vec<NotesSearchResult>> {
    let result = search_with_validation(query, top_k).await?;
    Ok(result.results)
}

/// Perform semantic search with staleness reporting.
///
/// Returns results directly from the index without per-note AppleScript
/// validation. Deleted notes will be discovered lazily when the caller
/// tries to fetch them via `get_note`.
#[cfg(feature = "memvid")]
pub async fn search_with_validation(query: &str, top_k: usize) -> Result<SearchResultsWithStaleness> {
    // Suppress FFmpeg swscaler warnings
    init_ffmpeg_quiet();

    let vpath = video_path();
    let ipath = index_path();

    if !vpath.exists() || !ipath.exists() {
        return Err(anyhow!(
            "Index not found. Run notes_rebuild_index first to create the semantic index."
        ));
    }

    // Create retriever with model_name override
    let mut retriever = MemvidRetriever::new_with_config(
        vpath.to_str().ok_or_else(|| anyhow!("Invalid video path"))?,
        ipath.to_str().ok_or_else(|| anyhow!("Invalid index path"))?,
        Some(build_memvid_rs_config()),
    )
    .await
    .map_err(|e| anyhow!("Failed to create retriever: {}", e))?;

    // Perform search (returns Vec<(score, text)>)
    let raw_results = retriever
        .search(query, top_k * 2)
        .await
        .map_err(|e| anyhow!("Search failed: {}", e))?;

    // Parse results and extract note metadata
    let min_score = load_config().search.min_score;
    let mut results: Vec<NotesSearchResult> = Vec::new();
    let mut seen_notes: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (score, text) in raw_results {
        if score < min_score {
            continue;
        }

        let (note_id, title, folder, content) = parse_chunk_metadata(&text);

        // Skip duplicates (same note may have multiple matching chunks)
        if note_id.is_empty() || seen_notes.contains(&note_id) {
            continue;
        }
        seen_notes.insert(note_id.clone());

        results.push(NotesSearchResult {
            note_id,
            title,
            folder,
            snippet: create_snippet(&content, 200),
            score,
        });

        if results.len() >= top_k {
            break;
        }
    }

    Ok(SearchResultsWithStaleness {
        results,
        filtered_deleted_count: 0,
        rebuild_recommended: false,
    })
}

// ============================================================================
// Sync Wrappers (for non-async callers)
// ============================================================================

/// Synchronous wrapper for build_index
#[cfg(feature = "memvid")]
pub fn build_index_sync() -> Result<IndexStats> {
    // Try to use existing tokio runtime (when called from MCP server's spawn_blocking).
    // Fall back to creating a new runtime (when called from CLI or standalone).
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(build_index()))
    } else {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow!("Failed to create tokio runtime: {}", e))?;
        rt.block_on(build_index())
    }
}

/// Synchronous wrapper for search
#[cfg(feature = "memvid")]
pub fn search_sync(query: &str, top_k: usize) -> Result<Vec<NotesSearchResult>> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(search(query, top_k)))
    } else {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow!("Failed to create tokio runtime: {}", e))?;
        rt.block_on(search(query, top_k))
    }
}

/// Synchronous wrapper for search_with_validation
#[cfg(feature = "memvid")]
pub fn search_with_validation_sync(query: &str, top_k: usize) -> Result<SearchResultsWithStaleness> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(search_with_validation(query, top_k)))
    } else {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow!("Failed to create tokio runtime: {}", e))?;
        rt.block_on(search_with_validation(query, top_k))
    }
}

// ============================================================================
// JSON Output for CLI/Agent
// ============================================================================

/// Build index and return JSON result
#[cfg(feature = "memvid")]
pub fn rebuild_index_json() -> Result<String> {
    let stats = build_index_sync()?;
    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "action": "rebuild",
        "indexed_note_count": stats.indexed_note_count,
        "video_size_bytes": stats.video_size_bytes,
        "index_size_bytes": stats.index_size_bytes,
        "last_updated": stats.last_updated,
        "video_path": video_path().to_string_lossy(),
        "index_path": index_path().to_string_lossy()
    }))?)
}

/// Stub for rebuild_index_json when memvid is disabled
#[cfg(not(feature = "memvid"))]
pub fn rebuild_index_json() -> Result<String> {
    Err(anyhow!(
        "Semantic search requires the 'memvid' feature. Build with: cargo build --features memvid\n\
         Note: FFmpeg must be installed (brew install ffmpeg)"
    ))
}

/// Pre-download and cache the BERT model without building an index.
/// Called by `psyxe-mcp warmup` to avoid a download delay on first search.
#[cfg(feature = "memvid")]
pub fn warmup_model() -> Result<()> {
    let config = build_memvid_rs_config();
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| anyhow!("Failed to create tokio runtime: {}", e))?;
    rt.block_on(async {
        // Create encoder — this downloads the model if needed
        let mut encoder = MemvidEncoder::new(Some(config))
            .await
            .map_err(|e| anyhow!("Failed to initialize encoder: {}", e))?;

        // Verify the model works by encoding a test string
        encoder
            .add_text("warmup test", 100, 0)
            .await
            .map_err(|e| anyhow!("BERT model verification failed: {}", e))?;

        tracing::info!("BERT model loaded and verified");
        Ok(())
    })
}

/// Stub for warmup_model when memvid is disabled
#[cfg(not(feature = "memvid"))]
pub fn warmup_model() -> Result<()> {
    Ok(()) // Nothing to download
}

/// Get index stats as JSON
pub fn stats_json() -> Result<String> {
    let stats = get_stats()?;

    #[cfg(feature = "memvid")]
    let memvid_enabled = true;
    #[cfg(not(feature = "memvid"))]
    let memvid_enabled = false;

    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "memvid_enabled": memvid_enabled,
        "exists": stats.exists,
        "is_stale": stats.is_stale,
        "indexed_note_count": stats.indexed_note_count,
        "current_note_count": stats.current_note_count,
        "last_updated": stats.last_updated,
        "video_size_bytes": stats.video_size_bytes,
        "index_size_bytes": stats.index_size_bytes,
        "video_path": video_path().to_string_lossy(),
        "index_path": index_path().to_string_lossy()
    }))?)
}

/// Semantic search and return compact JSON for LLM consumption.
/// Only includes title, snippet, and similarity score to minimize context token usage.
/// Validates results against Apple Notes and filters out deleted notes.
#[cfg(feature = "memvid")]
pub fn search_json(query: &str, top_k: usize) -> Result<String> {
    let search_result = search_with_validation_sync(query, top_k)?;
    Ok(serde_json::to_string(&json!({
        "count": search_result.results.len(),
        "results": search_result.results.iter().map(|r| json!({
            "note_id": r.note_id,
            "title": r.title,
            "snippet": r.snippet,
            "score": r.score,
        })).collect::<Vec<_>>()
    }))?)
}

/// Stub for search_json when memvid is disabled
#[cfg(not(feature = "memvid"))]
pub fn search_json(_query: &str, _top_k: usize) -> Result<String> {
    Err(anyhow!(
        "Semantic search requires the 'memvid' feature. Build with: cargo build --features memvid\n\
         Note: FFmpeg must be installed (brew install ffmpeg)"
    ))
}

// ============================================================================
// Smart Search (auto-select best method)
// ============================================================================

/// Smart search: uses semantic search if available and index exists, falls back to AppleScript
#[cfg(feature = "memvid")]
pub fn smart_search(query: &str) -> Result<String> {
    if index_exists() {
        search_json(query, 10)
    } else {
        // Fall back to AppleScript search
        apple_notes::search_notes(query, None)
    }
}

/// Smart search fallback when memvid is disabled
#[cfg(not(feature = "memvid"))]
pub fn smart_search(query: &str) -> Result<String> {
    // Always use AppleScript when memvid is not available
    apple_notes::search_notes(query, None)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_chunk_metadata() {
        let chunk = "NOTE_ID: x-coredata://123/ICNote/p456\nTITLE: Test Note\nFOLDER: Notes\nMODIFIED: 2026-01-27\n\nThis is the actual content of the note.";
        let (id, title, folder, content) = parse_chunk_metadata(chunk);
        assert_eq!(id, "x-coredata://123/ICNote/p456");
        assert_eq!(title, "Test Note");
        assert_eq!(folder, "Notes");
        assert!(content.contains("actual content"));
    }

    #[test]
    fn test_parse_chunk_metadata_no_metadata() {
        let chunk = "Just some plain text without metadata headers.";
        let (id, title, folder, content) = parse_chunk_metadata(chunk);
        assert!(id.is_empty());
        assert!(title.is_empty());
        assert!(folder.is_empty());
        // Content should be the original text
        assert!(content.contains("plain text"));
    }

    #[test]
    fn test_create_snippet() {
        let text = "This is a short text.";
        assert_eq!(create_snippet(text, 100), "This is a short text.");

        let long_text = "This is a much longer text that should be truncated at a word boundary.";
        let snippet = create_snippet(long_text, 30);
        assert!(snippet.len() <= 33); // 30 + "..."
        assert!(snippet.ends_with("..."));
    }

    #[test]
    fn test_cache_paths() {
        let vpath = video_path();
        let ipath = index_path();
        let mpath = meta_path();

        assert!(vpath.to_string_lossy().contains("apple_notes.mp4"));
        assert!(ipath.to_string_lossy().contains("apple_notes_index.db"));
        assert!(mpath.to_string_lossy().contains("apple_notes_meta.json"));
    }

    /// Integration test: after a full index rebuild, is_stale() must return false.
    ///
    /// This test requires:
    /// - macOS with Apple Notes.app
    /// - The `memvid` feature enabled
    /// - FFmpeg 7 installed
    ///
    /// Run manually with:
    ///   cargo test --features memvid test_index_not_stale_after_rebuild -- --ignored --nocapture
    #[test]
    #[ignore]
    #[cfg(feature = "memvid")]
    fn test_index_not_stale_after_rebuild() {
        // Step 1: Get the live note count from Notes.app
        let live_count = crate::apple_notes::get_note_count()
            .expect("Failed to get note count from Notes.app");
        eprintln!("Live note count from Notes.app: {}", live_count);

        // Step 2: Rebuild the semantic index
        eprintln!("Rebuilding memvid index (this may take several minutes)...");
        let stats = build_index_sync().expect("Index rebuild failed");
        eprintln!(
            "Rebuild complete: indexed={}, current={}, stale={}",
            stats.indexed_note_count, stats.current_note_count, stats.is_stale
        );

        // Step 3: Verify metadata was saved with the live count
        let meta = load_metadata().expect("Metadata should exist after rebuild");
        eprintln!(
            "Metadata note_count={}, live_count={}",
            meta.note_count, live_count
        );
        assert_eq!(
            meta.note_count, live_count,
            "Metadata note_count ({}) should match live Notes.app count ({})",
            meta.note_count, live_count
        );

        // Step 4: is_stale() must return false immediately after rebuild
        let stale = is_stale().expect("is_stale() failed");
        assert!(
            !stale,
            "Index should NOT be stale immediately after rebuild \
             (metadata count={}, live count={})",
            meta.note_count, live_count
        );
        eprintln!("PASS: is_stale() returned false after rebuild");

        // Step 5: Call is_stale() a second time (simulates the second query)
        let stale_again = is_stale().expect("is_stale() second call failed");
        assert!(
            !stale_again,
            "Index should still NOT be stale on second check"
        );
        eprintln!("PASS: is_stale() returned false on second check");
    }

    /// Quick sanity check: is_stale() and get_note_count() agree after rebuild
    /// without actually rebuilding — just checks the metadata vs live count.
    ///
    /// Run with:
    ///   cargo test --features memvid test_staleness_count_agreement -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_staleness_count_agreement() {
        if !index_exists() {
            eprintln!("SKIP: No memvid index exists — run a rebuild first");
            return;
        }

        let live_count = crate::apple_notes::get_note_count()
            .expect("Failed to get note count from Notes.app");
        let meta = load_metadata().expect("Metadata should exist when index exists");
        let stale = is_stale().expect("is_stale() failed");

        eprintln!("Live count: {}", live_count);
        eprintln!("Metadata count: {}", meta.note_count);
        eprintln!("is_stale: {}", stale);

        assert_eq!(
            stale,
            live_count != meta.note_count,
            "is_stale() should be true iff live count ({}) != metadata count ({})",
            live_count, meta.note_count
        );
    }
}
