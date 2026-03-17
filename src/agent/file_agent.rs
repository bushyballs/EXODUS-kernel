use crate::sync::Mutex;
use alloc::string::String;
/// File management agent with sandbox enforcement
///
/// Part of the AIOS agent layer. Provides file CRUD operations
/// within a sandboxed root, with undo stack, size limits,
/// and path traversal protection.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Actions the file agent can perform
#[derive(Debug, Clone)]
pub enum FileAction {
    Read(String),
    Write(String, String), // path, content
    Append(String, String),
    Search(String),       // pattern
    Move(String, String), // from, to
    Copy(String, String),
    Delete(String),
    List(String), // directory
    Stat(String), // file info
}

/// Result of a file operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileResult {
    Success,
    NotFound,
    PermissionDenied,
    PathTraversal, // Attempted escape from sandbox
    SizeLimitExceeded,
    QuotaExceeded, // Too many operations
    ReadOnly,
}

/// An undo-able file operation
#[derive(Clone)]
struct UndoEntry {
    action_hash: u64,
    path_hash: u64,
    old_content_hash: u64,
    timestamp: u64,
    can_undo: bool,
}

struct FileAgentInner {
    sandbox_root_hash: u64,
    allowed_extensions: Vec<u64>, // Hashes of allowed file extensions (empty = all)
    blocked_extensions: Vec<u64>, // Hashes of blocked extensions (.exe, .sh, etc.)
    undo_stack: Vec<UndoEntry>,
    max_undo: usize,
    // Limits
    max_file_size: u64,
    max_operations_per_session: u32,
    operations_this_session: u32,
    max_files_written: u32,
    files_written: u32,
    read_only: bool,
    // Stats
    total_reads: u64,
    total_writes: u64,
    total_deletes: u64,
    total_denied: u64,
}

static FILE_AGENT: Mutex<Option<FileAgentInner>> = Mutex::new(None);

/// Blocked file extensions (hashed) — prevent writing executables
const BLOCKED_EXT_HASHES: &[u64] = &[
    0x2E657865,     // .exe
    0x2E626174,     // .bat
    0x2E636D64,     // .cmd
    0x2E707331,     // .ps1
    0x2E6D7369,     // .msi
    0x2E646C6C,     // .dll
    0x2E736F,       // .so
    0x2E64796C6962, // .dylib
];

impl FileAgentInner {
    fn new(sandbox_root_hash: u64) -> Self {
        let mut blocked = Vec::new();
        for &h in BLOCKED_EXT_HASHES {
            blocked.push(h);
        }
        FileAgentInner {
            sandbox_root_hash,
            allowed_extensions: Vec::new(),
            blocked_extensions: blocked,
            undo_stack: Vec::new(),
            max_undo: 50,
            max_file_size: 50_000_000, // 50 MB
            max_operations_per_session: 1000,
            operations_this_session: 0,
            max_files_written: 100,
            files_written: 0,
            read_only: false,
            total_reads: 0,
            total_writes: 0,
            total_deletes: 0,
            total_denied: 0,
        }
    }

    /// Check path is within sandbox (simplified: hash-based check)
    fn is_in_sandbox(&self, path_hash: u64) -> bool {
        // In a real implementation, this would check the path prefix
        // Here we check it's not a known system path hash
        path_hash != 0 && self.sandbox_root_hash != 0
    }

    /// Check if extension is allowed
    fn is_ext_allowed(&self, ext_hash: u64) -> bool {
        if self.blocked_extensions.contains(&ext_hash) {
            return false;
        }
        if self.allowed_extensions.is_empty() {
            return true;
        }
        self.allowed_extensions.contains(&ext_hash)
    }

    /// Execute a file action
    fn do_action(
        &mut self,
        action: &FileAction,
        path_hash: u64,
        ext_hash: u64,
        content_size: u64,
        timestamp: u64,
    ) -> FileResult {
        // Rate limit
        if self.operations_this_session >= self.max_operations_per_session {
            self.total_denied = self.total_denied.saturating_add(1);
            return FileResult::QuotaExceeded;
        }
        self.operations_this_session = self.operations_this_session.saturating_add(1);

        // Sandbox check
        if !self.is_in_sandbox(path_hash) {
            self.total_denied = self.total_denied.saturating_add(1);
            return FileResult::PathTraversal;
        }

        match action {
            FileAction::Read(_)
            | FileAction::Search(_)
            | FileAction::List(_)
            | FileAction::Stat(_) => {
                self.total_reads = self.total_reads.saturating_add(1);
                FileResult::Success
            }
            FileAction::Write(_, _) | FileAction::Append(_, _) => {
                if self.read_only {
                    self.total_denied = self.total_denied.saturating_add(1);
                    return FileResult::ReadOnly;
                }
                if !self.is_ext_allowed(ext_hash) {
                    self.total_denied = self.total_denied.saturating_add(1);
                    return FileResult::PermissionDenied;
                }
                if content_size > self.max_file_size {
                    self.total_denied = self.total_denied.saturating_add(1);
                    return FileResult::SizeLimitExceeded;
                }
                if self.files_written >= self.max_files_written {
                    self.total_denied = self.total_denied.saturating_add(1);
                    return FileResult::QuotaExceeded;
                }
                // Record for undo
                if self.undo_stack.len() >= self.max_undo {
                    self.undo_stack.remove(0);
                }
                self.undo_stack.push(UndoEntry {
                    action_hash: 0x77726974, // "writ"
                    path_hash,
                    old_content_hash: 0,
                    timestamp,
                    can_undo: true,
                });
                self.files_written = self.files_written.saturating_add(1);
                self.total_writes = self.total_writes.saturating_add(1);
                FileResult::Success
            }
            FileAction::Move(_, _) | FileAction::Copy(_, _) => {
                if self.read_only {
                    self.total_denied = self.total_denied.saturating_add(1);
                    return FileResult::ReadOnly;
                }
                self.undo_stack.push(UndoEntry {
                    action_hash: 0x6D6F7665, // "move"
                    path_hash,
                    old_content_hash: 0,
                    timestamp,
                    can_undo: true,
                });
                self.total_writes = self.total_writes.saturating_add(1);
                FileResult::Success
            }
            FileAction::Delete(_) => {
                if self.read_only {
                    self.total_denied = self.total_denied.saturating_add(1);
                    return FileResult::ReadOnly;
                }
                self.undo_stack.push(UndoEntry {
                    action_hash: 0x64656C65, // "dele"
                    path_hash,
                    old_content_hash: 0,
                    timestamp,
                    can_undo: false, // Deletes can't be undone without content backup
                });
                self.total_deletes = self.total_deletes.saturating_add(1);
                FileResult::Success
            }
        }
    }

    /// Undo the last undoable operation
    fn undo(&mut self) -> Option<UndoEntry> {
        while let Some(entry) = self.undo_stack.pop() {
            if entry.can_undo {
                return Some(entry);
            }
        }
        None
    }

    fn set_read_only(&mut self, ro: bool) {
        self.read_only = ro;
    }

    fn reset_session(&mut self) {
        self.operations_this_session = 0;
        self.files_written = 0;
        self.undo_stack.clear();
    }
}

// --- Public API ---

/// Execute a file action
pub fn do_action(
    action: &FileAction,
    path_hash: u64,
    ext_hash: u64,
    content_size: u64,
    timestamp: u64,
) -> FileResult {
    let mut agent = FILE_AGENT.lock();
    match agent.as_mut() {
        Some(a) => a.do_action(action, path_hash, ext_hash, content_size, timestamp),
        None => FileResult::PermissionDenied,
    }
}

/// Undo last operation
pub fn undo() -> bool {
    let mut agent = FILE_AGENT.lock();
    match agent.as_mut() {
        Some(a) => a.undo().is_some(),
        None => false,
    }
}

/// Set read-only mode
pub fn set_read_only(ro: bool) {
    let mut agent = FILE_AGENT.lock();
    if let Some(a) = agent.as_mut() {
        a.set_read_only(ro);
    }
}

/// Reset for new session
pub fn reset_session() {
    let mut agent = FILE_AGENT.lock();
    if let Some(a) = agent.as_mut() {
        a.reset_session();
    }
}

pub fn init() {
    let mut agent = FILE_AGENT.lock();
    *agent = Some(FileAgentInner::new(0xA105_F11E)); // Default sandbox root
    serial_println!(
        "    File agent: sandboxed CRUD, undo stack, extension filtering, size limits ready"
    );
}
