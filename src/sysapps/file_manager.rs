use crate::sync::Mutex;
use alloc::vec;
/// GUI file browser for Genesis OS
///
/// Provides a full-featured file manager with list, grid, and detail
/// views. Supports copy, move, delete, rename, folder creation,
/// property inspection, sorting, and search. All paths are represented
/// as hashes for kernel-level storage efficiency.
///
/// Inspired by: Nautilus, Dolphin, Windows Explorer. All code is original.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Icon type hint for rendering file entries in the UI
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IconType {
    Folder,
    Document,
    Image,
    Audio,
    Video,
    Archive,
    Executable,
    Code,
    Spreadsheet,
    Presentation,
    Pdf,
    Unknown,
}

/// Determines how files are displayed in the browser pane
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileView {
    List,
    Grid,
    Details,
}

/// Which field to sort the file listing by
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortField {
    Name,
    Size,
    Modified,
    FileType,
}

/// Sort direction
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// Result codes for file operations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileOpResult {
    Success,
    NotFound,
    PermissionDenied,
    AlreadyExists,
    InvalidPath,
    IoError,
    DiskFull,
}

/// A single entry in a directory listing
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name_hash: u64,
    pub path_hash: u64,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
    pub permissions: u32,
    pub icon_type: IconType,
}

/// Properties dialog data for a file or directory
#[derive(Debug, Clone)]
pub struct FileProperties {
    pub name_hash: u64,
    pub path_hash: u64,
    pub is_dir: bool,
    pub size: u64,
    pub created: u64,
    pub modified: u64,
    pub accessed: u64,
    pub permissions: u32,
    pub owner_hash: u64,
    pub group_hash: u64,
    pub child_count: u32,
    pub icon_type: IconType,
}

/// Clipboard operation type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClipboardOp {
    Copy,
    Cut,
}

/// Internal clipboard for copy/cut operations
#[derive(Debug, Clone)]
struct ClipboardEntry {
    path_hash: u64,
    op: ClipboardOp,
}

/// Persistent file manager state
struct FileManagerState {
    current_dir_hash: u64,
    entries: Vec<FileEntry>,
    view_mode: FileView,
    sort_field: SortField,
    sort_direction: SortDirection,
    clipboard: Vec<ClipboardEntry>,
    history: Vec<u64>,
    history_pos: usize,
    show_hidden: bool,
    selected: Vec<u64>,
    search_results: Vec<FileEntry>,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static FILE_MANAGER: Mutex<Option<FileManagerState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_state() -> FileManagerState {
    FileManagerState {
        current_dir_hash: 0x0000_0000_0000_0001, // root
        entries: Vec::new(),
        view_mode: FileView::List,
        sort_field: SortField::Name,
        sort_direction: SortDirection::Ascending,
        clipboard: Vec::new(),
        history: vec![0x0000_0000_0000_0001],
        history_pos: 0,
        show_hidden: false,
        selected: Vec::new(),
        search_results: Vec::new(),
    }
}

fn detect_icon_type(name_hash: u64, is_dir: bool) -> IconType {
    if is_dir {
        return IconType::Folder;
    }
    // Use low byte of name hash as a mock extension discriminator
    match name_hash & 0xFF {
        0x01..=0x0F => IconType::Document,
        0x10..=0x1F => IconType::Image,
        0x20..=0x2F => IconType::Audio,
        0x30..=0x3F => IconType::Video,
        0x40..=0x4F => IconType::Archive,
        0x50..=0x5F => IconType::Executable,
        0x60..=0x6F => IconType::Code,
        0x70..=0x7F => IconType::Spreadsheet,
        0x80..=0x8F => IconType::Presentation,
        0x90..=0x9F => IconType::Pdf,
        _ => IconType::Unknown,
    }
}

fn sort_entries(entries: &mut Vec<FileEntry>, field: SortField, direction: SortDirection) {
    entries.sort_by(|a, b| {
        // Directories always come first
        if a.is_dir && !b.is_dir {
            return core::cmp::Ordering::Less;
        }
        if !a.is_dir && b.is_dir {
            return core::cmp::Ordering::Greater;
        }
        let ord = match field {
            SortField::Name => a.name_hash.cmp(&b.name_hash),
            SortField::Size => a.size.cmp(&b.size),
            SortField::Modified => a.modified.cmp(&b.modified),
            SortField::FileType => {
                let at = a.icon_type as u8;
                let bt = b.icon_type as u8;
                at.cmp(&bt)
            }
        };
        match direction {
            SortDirection::Ascending => ord,
            SortDirection::Descending => ord.reverse(),
        }
    });
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// List the contents of the current directory
pub fn list_dir() -> Vec<FileEntry> {
    let guard = FILE_MANAGER.lock();
    match guard.as_ref() {
        Some(state) => state.entries.clone(),
        None => Vec::new(),
    }
}

/// Navigate to a directory by its path hash
pub fn navigate_to(path_hash: u64) -> FileOpResult {
    let mut guard = FILE_MANAGER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FileOpResult::IoError,
    };

    // Simulate reading directory entries (kernel would populate from VFS)
    state.current_dir_hash = path_hash;
    state.entries.clear();
    state.selected.clear();

    // Push to navigation history
    if state.history_pos + 1 < state.history.len() {
        state.history.truncate(state.history_pos + 1);
    }
    state.history.push(path_hash);
    state.history_pos = state.history.len() - 1;

    // Populate stub entries for the new directory
    for i in 0u64..5 {
        let hash = path_hash.wrapping_add(i.wrapping_mul(0xDEAD));
        let is_dir = i < 2;
        state.entries.push(FileEntry {
            name_hash: hash,
            path_hash: path_hash.wrapping_add(hash),
            is_dir,
            size: if is_dir { 0 } else { (i + 1) * 1024 },
            modified: 1_700_000_000 + i * 3600,
            permissions: 0o755,
            icon_type: detect_icon_type(hash, is_dir),
        });
    }

    sort_entries(&mut state.entries, state.sort_field, state.sort_direction);
    FileOpResult::Success
}

/// Navigate back in history
pub fn navigate_back() -> FileOpResult {
    let mut guard = FILE_MANAGER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FileOpResult::IoError,
    };
    if state.history_pos == 0 {
        return FileOpResult::InvalidPath;
    }
    state.history_pos -= 1;
    let target = state.history[state.history_pos];
    state.current_dir_hash = target;
    state.entries.clear();
    state.selected.clear();
    FileOpResult::Success
}

/// Navigate forward in history
pub fn navigate_forward() -> FileOpResult {
    let mut guard = FILE_MANAGER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FileOpResult::IoError,
    };
    if state.history_pos + 1 >= state.history.len() {
        return FileOpResult::InvalidPath;
    }
    state.history_pos += 1;
    let target = state.history[state.history_pos];
    state.current_dir_hash = target;
    state.entries.clear();
    state.selected.clear();
    FileOpResult::Success
}

/// Copy a file or directory to the clipboard
pub fn copy(path_hash: u64) -> FileOpResult {
    let mut guard = FILE_MANAGER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FileOpResult::IoError,
    };
    state.clipboard.push(ClipboardEntry {
        path_hash,
        op: ClipboardOp::Copy,
    });
    FileOpResult::Success
}

/// Cut (move) a file or directory to the clipboard
pub fn cut(path_hash: u64) -> FileOpResult {
    let mut guard = FILE_MANAGER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FileOpResult::IoError,
    };
    state.clipboard.push(ClipboardEntry {
        path_hash,
        op: ClipboardOp::Cut,
    });
    FileOpResult::Success
}

/// Paste clipboard contents into the current directory
pub fn paste() -> FileOpResult {
    let mut guard = FILE_MANAGER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FileOpResult::IoError,
    };
    if state.clipboard.is_empty() {
        return FileOpResult::NotFound;
    }
    // In a real kernel, this would invoke VFS copy/move operations
    let dest = state.current_dir_hash;
    for entry in state.clipboard.iter() {
        let new_path = dest.wrapping_add(entry.path_hash);
        let is_dir = entry.path_hash & 1 == 0;
        state.entries.push(FileEntry {
            name_hash: entry.path_hash,
            path_hash: new_path,
            is_dir,
            size: 0,
            modified: 1_700_100_000,
            permissions: 0o644,
            icon_type: detect_icon_type(entry.path_hash, is_dir),
        });
    }
    state.clipboard.clear();
    FileOpResult::Success
}

/// Move a file from source to destination path hash
pub fn move_file(src_hash: u64, dest_hash: u64) -> FileOpResult {
    let mut guard = FILE_MANAGER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FileOpResult::IoError,
    };
    if let Some(pos) = state.entries.iter().position(|e| e.path_hash == src_hash) {
        state.entries[pos].path_hash = dest_hash;
        FileOpResult::Success
    } else {
        FileOpResult::NotFound
    }
}

/// Delete a file or directory
pub fn delete(path_hash: u64) -> FileOpResult {
    let mut guard = FILE_MANAGER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FileOpResult::IoError,
    };
    let before = state.entries.len();
    state.entries.retain(|e| e.path_hash != path_hash);
    if state.entries.len() < before {
        FileOpResult::Success
    } else {
        FileOpResult::NotFound
    }
}

/// Rename a file or directory
pub fn rename(path_hash: u64, new_name_hash: u64) -> FileOpResult {
    let mut guard = FILE_MANAGER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FileOpResult::IoError,
    };
    if let Some(entry) = state.entries.iter_mut().find(|e| e.path_hash == path_hash) {
        entry.name_hash = new_name_hash;
        FileOpResult::Success
    } else {
        FileOpResult::NotFound
    }
}

/// Create a new folder in the current directory
pub fn create_folder(name_hash: u64) -> FileOpResult {
    let mut guard = FILE_MANAGER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FileOpResult::IoError,
    };
    let path_hash = state.current_dir_hash.wrapping_add(name_hash);
    // Check for duplicates
    if state
        .entries
        .iter()
        .any(|e| e.name_hash == name_hash && e.is_dir)
    {
        return FileOpResult::AlreadyExists;
    }
    state.entries.push(FileEntry {
        name_hash,
        path_hash,
        is_dir: true,
        size: 0,
        modified: 1_700_200_000,
        permissions: 0o755,
        icon_type: IconType::Folder,
    });
    sort_entries(&mut state.entries, state.sort_field, state.sort_direction);
    FileOpResult::Success
}

/// Get properties for a file entry
pub fn get_properties(path_hash: u64) -> Option<FileProperties> {
    let guard = FILE_MANAGER.lock();
    let state = guard.as_ref()?;
    let entry = state.entries.iter().find(|e| e.path_hash == path_hash)?;
    Some(FileProperties {
        name_hash: entry.name_hash,
        path_hash: entry.path_hash,
        is_dir: entry.is_dir,
        size: entry.size,
        created: entry.modified.wrapping_sub(86400),
        modified: entry.modified,
        accessed: entry.modified,
        permissions: entry.permissions,
        owner_hash: 0x0000_0000_0000_0001,
        group_hash: 0x0000_0000_0000_0001,
        child_count: if entry.is_dir { 0 } else { 1 },
        icon_type: entry.icon_type,
    })
}

/// Sort the current listing by the given field and direction
pub fn sort_by(field: SortField, direction: SortDirection) {
    let mut guard = FILE_MANAGER.lock();
    if let Some(state) = guard.as_mut() {
        state.sort_field = field;
        state.sort_direction = direction;
        sort_entries(&mut state.entries, field, direction);
    }
}

/// Search for entries matching a name hash pattern in the current listing
pub fn search(query_hash: u64) -> Vec<FileEntry> {
    let mut guard = FILE_MANAGER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let results: Vec<FileEntry> = state
        .entries
        .iter()
        .filter(|e| {
            // Simple hash proximity match (real impl would do substring matching)
            let diff = if e.name_hash > query_hash {
                e.name_hash - query_hash
            } else {
                query_hash - e.name_hash
            };
            diff < 0x0000_0FFF
        })
        .cloned()
        .collect();
    state.search_results = results.clone();
    results
}

/// Set the view mode (List, Grid, Details)
pub fn set_view_mode(mode: FileView) {
    let mut guard = FILE_MANAGER.lock();
    if let Some(state) = guard.as_mut() {
        state.view_mode = mode;
    }
}

/// Get the current view mode
pub fn get_view_mode() -> FileView {
    let guard = FILE_MANAGER.lock();
    match guard.as_ref() {
        Some(state) => state.view_mode,
        None => FileView::List,
    }
}

/// Toggle hidden file visibility
pub fn toggle_hidden() {
    let mut guard = FILE_MANAGER.lock();
    if let Some(state) = guard.as_mut() {
        state.show_hidden = !state.show_hidden;
    }
}

/// Select a file entry
pub fn select(path_hash: u64) {
    let mut guard = FILE_MANAGER.lock();
    if let Some(state) = guard.as_mut() {
        if !state.selected.contains(&path_hash) {
            state.selected.push(path_hash);
        }
    }
}

/// Clear selection
pub fn clear_selection() {
    let mut guard = FILE_MANAGER.lock();
    if let Some(state) = guard.as_mut() {
        state.selected.clear();
    }
}

/// Get selected entry count
pub fn selection_count() -> usize {
    let guard = FILE_MANAGER.lock();
    match guard.as_ref() {
        Some(state) => state.selected.len(),
        None => 0,
    }
}

/// Get the current directory hash
pub fn current_dir() -> u64 {
    let guard = FILE_MANAGER.lock();
    match guard.as_ref() {
        Some(state) => state.current_dir_hash,
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the file manager subsystem
pub fn init() {
    let mut guard = FILE_MANAGER.lock();
    *guard = Some(default_state());
    serial_println!("    File manager ready");
}
