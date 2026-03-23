//! psyXe MCP Core — Apple ecosystem tools library
//!
//! Open-source library providing direct access to macOS-native APIs:
//! - Apple Notes (AppleScript + optional BERT semantic search via memvid)
//! - Apple Reminders (Swift EventKit helper + AppleScript fallback)
//! - Apple Contacts (Swift Contacts framework helper)
//! - Apple Messages (AppleScript send + chat.db polling)
//! - Apple Weather (WeatherKit JWT + Open-Meteo fallback)
//! - Apple Maps (MapKit JS geocoding + Nominatim fallback)
//! - File search and operations (with access-controlled granted folders)
//! - Web tools (Brave Search, URL fetch, YouTube transcripts)
//! - Image generation and analysis (Gemini API)
//! - PDF generation (Markdown/HTML to PDF)
//!
//! All tools execute directly against macOS-native APIs with no LLM intermediary.
//! Access control is enforced via a pluggable `AccessStore` backed by macOS Keychain
//! or a JSON/TOML config file.

// Shared utilities
pub mod applescript_utils;
pub mod tool_semaphore;

// Tool definitions and execution
pub mod tools;
pub mod tool_dispatch;

// Access control (Keychain + file fallback; SecretsStore backend optional via `secrets-store` feature on consumers)
pub mod access_store;

// Apple integrations
pub mod apple_notes;
pub mod apple_reminders;
pub mod apple_contacts;
pub mod apple_messages;
pub mod apple_weather;
pub mod apple_maps;
#[cfg(target_os = "macos")]
pub mod eventkit_ffi;

// Weather fallbacks
pub mod open_meteo;
pub mod nominatim;

// Semantic search (optional, feature-gated)
pub mod memvid_notes;

// File operations
pub mod file_search;
pub mod file_ops;

// Web and media tools
pub mod fetch_url;
pub mod brave_search;
pub mod youtube_transcript;
pub mod code_interpreter;
pub mod gemini_image;

// Image security and PDF
pub mod image_security;
pub mod pdf_generator;
