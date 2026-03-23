//! Per-subsystem tool semaphore layer.
//!
//! Provides `RwLock`-based concurrency control for tool execution so that
//! swarm workers can fan out tool calls safely. Read tools (search, list, get)
//! run concurrently; write tools (create, edit, delete, rebuild) get exclusive
//! access within their subsystem. Different subsystems are fully independent.
//!
//! Single entry point: [`acquire_tool_lock`] — called at the top of
//! `execute_tool_impl()` in `agent.rs`.

use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

// ---------------------------------------------------------------------------
// Subsystem + AccessMode enums
// ---------------------------------------------------------------------------

/// Logical subsystem that a tool belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subsystem {
    AppleNotes,
    AppleReminders,
    AppleMessages,
    Memvid,
    FileSystem,
    PdfGenerator,
}

/// Whether a tool needs shared (read) or exclusive (write) access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    Read,
    Write,
}

// ---------------------------------------------------------------------------
// Tool → (Subsystem, AccessMode) mapping
// ---------------------------------------------------------------------------

/// Map a tool name to its subsystem and access mode.
/// Returns `None` for tools that need no locking (weather, web, LLM, KB, etc.).
pub fn tool_subsystem(tool: &str) -> Option<(Subsystem, AccessMode)> {
    use AccessMode::*;
    use Subsystem::*;

    match tool {
        // Apple Notes — reads
        "search_notes" | "list_notes" | "get_note" | "open_note" | "notes_tags"
        | "notes_search_by_tag" | "notes_semantic_search" | "notes_smart_search"
        | "notes_index_stats" => Some((AppleNotes, Read)),
        // Apple Notes — writes
        "notes_index" | "notes_rebuild_index" => Some((AppleNotes, Write)),

        // Apple Reminders — reads
        "list_reminder_lists" | "search_reminders" | "list_reminders" | "get_reminder"
        | "open_reminders" => Some((AppleReminders, Read)),
        // Apple Reminders — writes
        "create_reminder" | "create_reminders_batch" | "complete_reminder"
        | "delete_reminder" | "edit_reminder" | "edit_reminders_batch"
        | "create_reminder_list" | "delete_reminder_list" => Some((AppleReminders, Write)),

        // Apple Messages — writes only
        "notify_human" | "wait_for_human_reply" => Some((AppleMessages, Write)),

        // Memvid — reads only
        "memory_search" | "memory_stats" => Some((Memvid, Read)),

        // File system — reads
        "read_file" | "file_search" => Some((FileSystem, Read)),
        // File system — writes
        "write_file" | "apply_patch" => Some((FileSystem, Write)),

        // PDF — write only
        "create_pdf" => Some((PdfGenerator, Write)),

        // Everything else: no lock needed
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Static registry of per-subsystem RwLocks
// ---------------------------------------------------------------------------

struct SubsystemRegistry {
    apple_notes: RwLock<()>,
    apple_reminders: RwLock<()>,
    apple_messages: RwLock<()>,
    memvid: RwLock<()>,
    file_system: RwLock<()>,
    pdf_generator: RwLock<()>,
}

impl SubsystemRegistry {
    const fn new() -> Self {
        Self {
            apple_notes: RwLock::new(()),
            apple_reminders: RwLock::new(()),
            apple_messages: RwLock::new(()),
            memvid: RwLock::new(()),
            file_system: RwLock::new(()),
            pdf_generator: RwLock::new(()),
        }
    }

    fn get(&self, subsystem: Subsystem) -> &RwLock<()> {
        match subsystem {
            Subsystem::AppleNotes => &self.apple_notes,
            Subsystem::AppleReminders => &self.apple_reminders,
            Subsystem::AppleMessages => &self.apple_messages,
            Subsystem::Memvid => &self.memvid,
            Subsystem::FileSystem => &self.file_system,
            Subsystem::PdfGenerator => &self.pdf_generator,
        }
    }
}

static SUBSYSTEMS: SubsystemRegistry = SubsystemRegistry::new();

// ---------------------------------------------------------------------------
// RAII guard enum
// ---------------------------------------------------------------------------

/// RAII guard returned by [`acquire_tool_lock`].  Holds either a read lock,
/// write lock, or nothing (for tools that need no synchronization).
pub enum SubsystemGuard<'a> {
    Read(RwLockReadGuard<'a, ()>),
    Write(RwLockWriteGuard<'a, ()>),
    None,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Acquire the appropriate subsystem lock for `tool`.
///
/// Returns an RAII guard that is held for the duration of tool execution.
/// - Read tools get a shared lock (concurrent with other reads).
/// - Write tools get an exclusive lock.
/// - Unrecognized / no-lock tools return `SubsystemGuard::None` instantly.
///
/// Poisoned locks are recovered via `into_inner()` so a panicked thread
/// never permanently blocks a subsystem.
pub fn acquire_tool_lock(tool: &str) -> SubsystemGuard<'static> {
    let Some((subsystem, mode)) = tool_subsystem(tool) else {
        return SubsystemGuard::None;
    };

    let lock = SUBSYSTEMS.get(subsystem);

    match mode {
        AccessMode::Read => {
            let guard = lock.read().unwrap_or_else(|poisoned| {
                tracing::warn!(
                    ?subsystem,
                    "Subsystem RwLock was poisoned (read); recovering"
                );
                poisoned.into_inner()
            });
            SubsystemGuard::Read(guard)
        }
        AccessMode::Write => {
            let guard = lock.write().unwrap_or_else(|poisoned| {
                tracing::warn!(
                    ?subsystem,
                    "Subsystem RwLock was poisoned (write); recovering"
                );
                poisoned.into_inner()
            });
            SubsystemGuard::Write(guard)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn test_tool_subsystem_mapping() {
        // Notes reads
        assert_eq!(
            tool_subsystem("search_notes"),
            Some((Subsystem::AppleNotes, AccessMode::Read))
        );
        assert_eq!(
            tool_subsystem("notes_smart_search"),
            Some((Subsystem::AppleNotes, AccessMode::Read))
        );
        // Notes writes
        assert_eq!(
            tool_subsystem("notes_rebuild_index"),
            Some((Subsystem::AppleNotes, AccessMode::Write))
        );
        // Reminders reads
        assert_eq!(
            tool_subsystem("list_reminders"),
            Some((Subsystem::AppleReminders, AccessMode::Read))
        );
        // Reminders writes
        assert_eq!(
            tool_subsystem("create_reminder"),
            Some((Subsystem::AppleReminders, AccessMode::Write))
        );
        assert_eq!(
            tool_subsystem("edit_reminders_batch"),
            Some((Subsystem::AppleReminders, AccessMode::Write))
        );
        // Messages
        assert_eq!(
            tool_subsystem("notify_human"),
            Some((Subsystem::AppleMessages, AccessMode::Write))
        );
        // Memvid
        assert_eq!(
            tool_subsystem("memory_search"),
            Some((Subsystem::Memvid, AccessMode::Read))
        );
        // FileSystem
        assert_eq!(
            tool_subsystem("read_file"),
            Some((Subsystem::FileSystem, AccessMode::Read))
        );
        assert_eq!(
            tool_subsystem("apply_patch"),
            Some((Subsystem::FileSystem, AccessMode::Write))
        );
        // PDF
        assert_eq!(
            tool_subsystem("create_pdf"),
            Some((Subsystem::PdfGenerator, AccessMode::Write))
        );
        // No-lock tools
        assert_eq!(tool_subsystem("get_weather"), None);
        assert_eq!(tool_subsystem("web_search"), None);
        assert_eq!(tool_subsystem("time_now"), None);
        assert_eq!(tool_subsystem("delegate_task"), None);
        assert_eq!(tool_subsystem("consult_agent"), None);
        assert_eq!(tool_subsystem("completely_unknown_tool"), None);
    }

    #[test]
    fn test_concurrent_reads_allowed() {
        // 3 threads all acquire read locks on the same subsystem simultaneously.
        let barrier = Arc::new(Barrier::new(3));
        let handles: Vec<_> = (0..3)
            .map(|_| {
                let b = Arc::clone(&barrier);
                thread::spawn(move || {
                    let _guard = acquire_tool_lock("search_notes");
                    // All 3 threads reach the barrier while holding read locks.
                    b.wait();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn test_write_blocks_reads() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;

        // Acquire write lock on AppleReminders in a background thread.
        let write_held = Arc::new(AtomicBool::new(false));
        let write_done = Arc::new(AtomicBool::new(false));
        let barrier = Arc::new(Barrier::new(2));

        let wh = Arc::clone(&write_held);
        let wd = Arc::clone(&write_done);
        let b = Arc::clone(&barrier);
        let writer = thread::spawn(move || {
            let lock = SUBSYSTEMS.get(Subsystem::AppleReminders);
            let _guard = lock.write().unwrap();
            wh.store(true, Ordering::SeqCst);
            b.wait(); // signal that write lock is held
            thread::sleep(Duration::from_millis(50));
            wd.store(true, Ordering::SeqCst);
            // guard dropped here
        });

        barrier.wait(); // wait until writer holds the lock
        assert!(write_held.load(Ordering::SeqCst));

        // Now try to read — should block until writer releases.
        let _guard = acquire_tool_lock("list_reminders");
        assert!(write_done.load(Ordering::SeqCst), "read should wait for write to finish");

        writer.join().unwrap();
    }

    #[test]
    fn test_no_lock_tools_are_instant() {
        let guard = acquire_tool_lock("get_weather");
        assert!(matches!(guard, SubsystemGuard::None));

        let guard = acquire_tool_lock("time_now");
        assert!(matches!(guard, SubsystemGuard::None));

        let guard = acquire_tool_lock("some_kb_tool");
        assert!(matches!(guard, SubsystemGuard::None));
    }

    #[test]
    fn test_different_subsystems_independent() {
        // A write lock on AppleNotes does not block a write on AppleReminders.
        let barrier = Arc::new(Barrier::new(2));

        let b1 = Arc::clone(&barrier);
        let t1 = thread::spawn(move || {
            let lock = SUBSYSTEMS.get(Subsystem::AppleNotes);
            let _guard = lock.write().unwrap();
            b1.wait(); // both threads hold write locks simultaneously
        });

        let b2 = Arc::clone(&barrier);
        let t2 = thread::spawn(move || {
            let lock = SUBSYSTEMS.get(Subsystem::AppleReminders);
            let _guard = lock.write().unwrap();
            b2.wait();
        });

        t1.join().unwrap();
        t2.join().unwrap();
    }

    #[test]
    fn test_poison_recovery() {
        // Poison the Memvid lock by panicking while holding it.
        let result = thread::spawn(|| {
            let lock = SUBSYSTEMS.get(Subsystem::Memvid);
            let _guard = lock.write().unwrap();
            panic!("intentional panic to poison lock");
        })
        .join();
        assert!(result.is_err(), "thread should have panicked");

        // Lock is now poisoned — acquire_tool_lock should recover.
        let guard = acquire_tool_lock("memory_search");
        assert!(matches!(guard, SubsystemGuard::Read(_)));
    }
}
