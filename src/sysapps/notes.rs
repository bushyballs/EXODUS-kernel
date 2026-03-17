use crate::sync::Mutex;
/// Notes application for Genesis OS
///
/// Full-featured note-taking app with folders, tags, pinning, search,
/// and export. Notes are stored with hash-based content references
/// for kernel-level storage. Supports rich-text style tracking via
/// format flags, folder organization, and recent-notes view.
///
/// Inspired by: GNOME Notes, Apple Notes, Obsidian. All code is original.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of notes
const MAX_NOTES: usize = 10_000;
/// Maximum number of folders
const MAX_FOLDERS: usize = 500;
/// Maximum tags per note
const MAX_TAGS_PER_NOTE: usize = 20;
/// Maximum recent notes to track
const MAX_RECENT: usize = 50;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Note format type (for rendering hints)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoteFormat {
    PlainText,
    Markdown,
    Checklist,
    RichText,
}

/// Sort criteria for notes
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoteSort {
    ModifiedNewest,
    ModifiedOldest,
    CreatedNewest,
    CreatedOldest,
    TitleAsc,
    TitleDesc,
}

/// Export format
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExportFormat {
    PlainText,
    Markdown,
    Html,
    Pdf,
}

/// Result codes for notes operations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NotesResult {
    Success,
    NotFound,
    AlreadyExists,
    LimitReached,
    InvalidInput,
    IoError,
}

/// A single note
#[derive(Debug, Clone)]
pub struct Note {
    pub id: u64,
    pub title_hash: u64,
    pub content_hash: u64,
    pub created: u64,
    pub modified: u64,
    pub pinned: bool,
    pub folder_hash: u64,
    pub tags: Vec<u64>,
    pub format: NoteFormat,
    pub word_count: u32,
    pub char_count: u32,
    pub locked: bool,
    pub color: u32,
}

/// A folder for organizing notes
#[derive(Debug, Clone)]
pub struct NoteFolder {
    pub id: u64,
    pub name_hash: u64,
    pub note_count: u32,
    pub parent_hash: u64,
    pub color: u32,
    pub sort_order: u32,
}

/// A tag for categorizing notes
#[derive(Debug, Clone)]
pub struct NoteTag {
    pub hash: u64,
    pub usage_count: u32,
}

/// Search result for a note
#[derive(Debug, Clone)]
pub struct NoteSearchResult {
    pub note_id: u64,
    pub title_hash: u64,
    pub snippet_hash: u64,
    pub relevance: u32,
}

/// Export result
#[derive(Debug, Clone)]
pub struct ExportResult {
    pub note_id: u64,
    pub format: ExportFormat,
    pub output_hash: u64,
    pub size_bytes: u32,
}

/// Persistent notes state
struct NotesState {
    notes: Vec<Note>,
    folders: Vec<NoteFolder>,
    tags: Vec<NoteTag>,
    next_note_id: u64,
    next_folder_id: u64,
    sort_mode: NoteSort,
    current_folder: Option<u64>,
    recent_ids: Vec<u64>,
    trash: Vec<Note>,
    timestamp_counter: u64,
    default_format: NoteFormat,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static NOTES: Mutex<Option<NotesState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_state() -> NotesState {
    NotesState {
        notes: Vec::new(),
        folders: Vec::new(),
        tags: Vec::new(),
        next_note_id: 1,
        next_folder_id: 1,
        sort_mode: NoteSort::ModifiedNewest,
        current_folder: None,
        recent_ids: Vec::new(),
        trash: Vec::new(),
        timestamp_counter: 1_700_000_000,
        default_format: NoteFormat::PlainText,
    }
}

fn next_timestamp(state: &mut NotesState) -> u64 {
    state.timestamp_counter += 1;
    state.timestamp_counter
}

fn add_to_recent(state: &mut NotesState, note_id: u64) {
    state.recent_ids.retain(|&id| id != note_id);
    state.recent_ids.insert(0, note_id);
    if state.recent_ids.len() > MAX_RECENT {
        state.recent_ids.truncate(MAX_RECENT);
    }
}

fn sort_notes(notes: &mut Vec<Note>, mode: NoteSort) {
    // Pinned notes always come first
    notes.sort_by(|a, b| {
        if a.pinned && !b.pinned {
            return core::cmp::Ordering::Less;
        }
        if !a.pinned && b.pinned {
            return core::cmp::Ordering::Greater;
        }
        match mode {
            NoteSort::ModifiedNewest => b.modified.cmp(&a.modified),
            NoteSort::ModifiedOldest => a.modified.cmp(&b.modified),
            NoteSort::CreatedNewest => b.created.cmp(&a.created),
            NoteSort::CreatedOldest => a.created.cmp(&b.created),
            NoteSort::TitleAsc => a.title_hash.cmp(&b.title_hash),
            NoteSort::TitleDesc => b.title_hash.cmp(&a.title_hash),
        }
    });
}

fn update_folder_counts(state: &mut NotesState) {
    for folder in state.folders.iter_mut() {
        folder.note_count = state
            .notes
            .iter()
            .filter(|n| n.folder_hash == folder.name_hash)
            .count() as u32;
    }
}

fn update_tag_counts(state: &mut NotesState) {
    for tag in state.tags.iter_mut() {
        tag.usage_count = state
            .notes
            .iter()
            .filter(|n| n.tags.contains(&tag.hash))
            .count() as u32;
    }
    // Remove unused tags
    state.tags.retain(|t| t.usage_count > 0);
}

// ---------------------------------------------------------------------------
// Public API — Notes CRUD
// ---------------------------------------------------------------------------

/// Create a new note
pub fn create_note(
    title_hash: u64,
    content_hash: u64,
    folder_hash: u64,
) -> Result<u64, NotesResult> {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(NotesResult::IoError),
    };
    if state.notes.len() >= MAX_NOTES {
        return Err(NotesResult::LimitReached);
    }

    let id = state.next_note_id;
    state.next_note_id += 1;
    let now = next_timestamp(state);

    let note = Note {
        id,
        title_hash,
        content_hash,
        created: now,
        modified: now,
        pinned: false,
        folder_hash,
        tags: Vec::new(),
        format: state.default_format,
        word_count: 0,
        char_count: 0,
        locked: false,
        color: 0xFFFF_FFFF,
    };
    state.notes.push(note);
    add_to_recent(state, id);
    update_folder_counts(state);
    sort_notes(&mut state.notes, state.sort_mode);
    Ok(id)
}

/// Edit a note's content
pub fn edit_note(
    note_id: u64,
    new_content_hash: u64,
    word_count: u32,
    char_count: u32,
) -> NotesResult {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NotesResult::IoError,
    };
    let note = match state.notes.iter_mut().find(|n| n.id == note_id) {
        Some(n) => n,
        None => return NotesResult::NotFound,
    };
    if note.locked {
        return NotesResult::InvalidInput;
    }
    note.content_hash = new_content_hash;
    note.word_count = word_count;
    note.char_count = char_count;
    let now = next_timestamp(state);
    // Re-borrow after timestamp update
    let note = state.notes.iter_mut().find(|n| n.id == note_id).unwrap();
    note.modified = now;
    add_to_recent(state, note_id);
    sort_notes(&mut state.notes, state.sort_mode);
    NotesResult::Success
}

/// Edit a note's title
pub fn edit_title(note_id: u64, new_title_hash: u64) -> NotesResult {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NotesResult::IoError,
    };
    let now = next_timestamp(state);
    let note = match state.notes.iter_mut().find(|n| n.id == note_id) {
        Some(n) => n,
        None => return NotesResult::NotFound,
    };
    note.title_hash = new_title_hash;
    note.modified = now;
    NotesResult::Success
}

/// Delete a note (moves to trash)
pub fn delete_note(note_id: u64) -> NotesResult {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NotesResult::IoError,
    };
    if let Some(pos) = state.notes.iter().position(|n| n.id == note_id) {
        let note = state.notes.remove(pos);
        state.trash.push(note);
        state.recent_ids.retain(|&id| id != note_id);
        update_folder_counts(state);
        update_tag_counts(state);
        NotesResult::Success
    } else {
        NotesResult::NotFound
    }
}

/// Permanently delete a note from trash
pub fn delete_permanently(note_id: u64) -> NotesResult {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NotesResult::IoError,
    };
    let before = state.trash.len();
    state.trash.retain(|n| n.id != note_id);
    if state.trash.len() < before {
        NotesResult::Success
    } else {
        NotesResult::NotFound
    }
}

/// Restore a note from trash
pub fn restore_note(note_id: u64) -> NotesResult {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NotesResult::IoError,
    };
    if let Some(pos) = state.trash.iter().position(|n| n.id == note_id) {
        let note = state.trash.remove(pos);
        state.notes.push(note);
        update_folder_counts(state);
        sort_notes(&mut state.notes, state.sort_mode);
        NotesResult::Success
    } else {
        NotesResult::NotFound
    }
}

/// Empty the trash
pub fn empty_trash() -> u32 {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return 0,
    };
    let count = state.trash.len() as u32;
    state.trash.clear();
    count
}

/// Pin or unpin a note
pub fn pin_note(note_id: u64, pinned: bool) -> NotesResult {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NotesResult::IoError,
    };
    if let Some(note) = state.notes.iter_mut().find(|n| n.id == note_id) {
        note.pinned = pinned;
        sort_notes(&mut state.notes, state.sort_mode);
        NotesResult::Success
    } else {
        NotesResult::NotFound
    }
}

/// Lock or unlock a note (locked notes cannot be edited)
pub fn lock_note(note_id: u64, locked: bool) -> NotesResult {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NotesResult::IoError,
    };
    if let Some(note) = state.notes.iter_mut().find(|n| n.id == note_id) {
        note.locked = locked;
        NotesResult::Success
    } else {
        NotesResult::NotFound
    }
}

/// Set note color
pub fn set_color(note_id: u64, color: u32) -> NotesResult {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NotesResult::IoError,
    };
    if let Some(note) = state.notes.iter_mut().find(|n| n.id == note_id) {
        note.color = color;
        NotesResult::Success
    } else {
        NotesResult::NotFound
    }
}

// ---------------------------------------------------------------------------
// Public API — Tags
// ---------------------------------------------------------------------------

/// Add a tag to a note
pub fn add_tag(note_id: u64, tag_hash: u64) -> NotesResult {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NotesResult::IoError,
    };
    let note = match state.notes.iter_mut().find(|n| n.id == note_id) {
        Some(n) => n,
        None => return NotesResult::NotFound,
    };
    if note.tags.len() >= MAX_TAGS_PER_NOTE {
        return NotesResult::LimitReached;
    }
    if note.tags.contains(&tag_hash) {
        return NotesResult::AlreadyExists;
    }
    note.tags.push(tag_hash);
    // Ensure tag exists in global list
    if !state.tags.iter().any(|t| t.hash == tag_hash) {
        state.tags.push(NoteTag {
            hash: tag_hash,
            usage_count: 0,
        });
    }
    update_tag_counts(state);
    NotesResult::Success
}

/// Remove a tag from a note
pub fn remove_tag(note_id: u64, tag_hash: u64) -> NotesResult {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NotesResult::IoError,
    };
    let note = match state.notes.iter_mut().find(|n| n.id == note_id) {
        Some(n) => n,
        None => return NotesResult::NotFound,
    };
    let before = note.tags.len();
    note.tags.retain(|&h| h != tag_hash);
    if note.tags.len() < before {
        update_tag_counts(state);
        NotesResult::Success
    } else {
        NotesResult::NotFound
    }
}

/// Get all tags with usage counts
pub fn get_all_tags() -> Vec<NoteTag> {
    let guard = NOTES.lock();
    match guard.as_ref() {
        Some(state) => state.tags.clone(),
        None => Vec::new(),
    }
}

/// Get notes by tag
pub fn get_notes_by_tag(tag_hash: u64) -> Vec<Note> {
    let guard = NOTES.lock();
    match guard.as_ref() {
        Some(state) => state
            .notes
            .iter()
            .filter(|n| n.tags.contains(&tag_hash))
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Public API — Folders
// ---------------------------------------------------------------------------

/// Create a new folder
pub fn create_folder(name_hash: u64, parent_hash: u64) -> Result<u64, NotesResult> {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(NotesResult::IoError),
    };
    if state.folders.len() >= MAX_FOLDERS {
        return Err(NotesResult::LimitReached);
    }
    if state
        .folders
        .iter()
        .any(|f| f.name_hash == name_hash && f.parent_hash == parent_hash)
    {
        return Err(NotesResult::AlreadyExists);
    }

    let id = state.next_folder_id;
    state.next_folder_id += 1;
    let sort_order = state.folders.len() as u32;
    state.folders.push(NoteFolder {
        id,
        name_hash,
        note_count: 0,
        parent_hash,
        color: 0xFFAA_BB00,
        sort_order,
    });
    Ok(id)
}

/// Delete a folder (moves contained notes to root)
pub fn delete_folder(folder_id: u64) -> NotesResult {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NotesResult::IoError,
    };
    let folder = match state.folders.iter().find(|f| f.id == folder_id) {
        Some(f) => f.name_hash,
        None => return NotesResult::NotFound,
    };
    // Move notes from this folder to root (folder_hash = 0)
    for note in state.notes.iter_mut() {
        if note.folder_hash == folder {
            note.folder_hash = 0;
        }
    }
    state.folders.retain(|f| f.id != folder_id);
    update_folder_counts(state);
    NotesResult::Success
}

/// Move a note to a different folder
pub fn move_to_folder(note_id: u64, folder_hash: u64) -> NotesResult {
    let mut guard = NOTES.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return NotesResult::IoError,
    };
    let now = next_timestamp(state);
    let note = match state.notes.iter_mut().find(|n| n.id == note_id) {
        Some(n) => n,
        None => return NotesResult::NotFound,
    };
    note.folder_hash = folder_hash;
    note.modified = now;
    update_folder_counts(state);
    NotesResult::Success
}

/// Get all folders
pub fn get_folders() -> Vec<NoteFolder> {
    let guard = NOTES.lock();
    match guard.as_ref() {
        Some(state) => state.folders.clone(),
        None => Vec::new(),
    }
}

/// Get notes in a specific folder
pub fn get_folder_notes(folder_hash: u64) -> Vec<Note> {
    let guard = NOTES.lock();
    match guard.as_ref() {
        Some(state) => state
            .notes
            .iter()
            .filter(|n| n.folder_hash == folder_hash)
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Public API — Search and navigation
// ---------------------------------------------------------------------------

/// Search notes by title/content hash proximity
pub fn search_notes(query_hash: u64) -> Vec<NoteSearchResult> {
    let guard = NOTES.lock();
    let state = match guard.as_ref() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let mut results: Vec<NoteSearchResult> = Vec::new();
    for note in state.notes.iter() {
        let title_diff = if note.title_hash > query_hash {
            note.title_hash - query_hash
        } else {
            query_hash - note.title_hash
        };
        let content_diff = if note.content_hash > query_hash {
            note.content_hash - query_hash
        } else {
            query_hash - note.content_hash
        };
        let min_diff = if title_diff < content_diff {
            title_diff
        } else {
            content_diff
        };
        if min_diff < 0x0000_FFFF_FFFF {
            let relevance = (0x0000_FFFF_FFFF - min_diff) as u32;
            results.push(NoteSearchResult {
                note_id: note.id,
                title_hash: note.title_hash,
                snippet_hash: note.content_hash,
                relevance,
            });
        }
    }
    // Sort by relevance descending
    results.sort_by(|a, b| b.relevance.cmp(&a.relevance));
    results
}

/// Export a note to a given format
pub fn export_note(note_id: u64, format: ExportFormat) -> Result<ExportResult, NotesResult> {
    let guard = NOTES.lock();
    let state = match guard.as_ref() {
        Some(s) => s,
        None => return Err(NotesResult::IoError),
    };
    let note = match state.notes.iter().find(|n| n.id == note_id) {
        Some(n) => n,
        None => return Err(NotesResult::NotFound),
    };
    // Simulate export by generating an output hash
    let output_hash = note.content_hash.wrapping_mul(format as u64 + 1);
    let size_bytes = note.char_count
        * match format {
            ExportFormat::PlainText => 1,
            ExportFormat::Markdown => 2,
            ExportFormat::Html => 4,
            ExportFormat::Pdf => 3,
        };
    Ok(ExportResult {
        note_id,
        format,
        output_hash,
        size_bytes,
    })
}

/// Get recently accessed notes
pub fn get_recent() -> Vec<Note> {
    let guard = NOTES.lock();
    let state = match guard.as_ref() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let mut results = Vec::new();
    for &id in state.recent_ids.iter() {
        if let Some(note) = state.notes.iter().find(|n| n.id == id) {
            results.push(note.clone());
        }
    }
    results
}

/// Get total note count
pub fn note_count() -> usize {
    let guard = NOTES.lock();
    match guard.as_ref() {
        Some(state) => state.notes.len(),
        None => 0,
    }
}

/// Get trash count
pub fn trash_count() -> usize {
    let guard = NOTES.lock();
    match guard.as_ref() {
        Some(state) => state.trash.len(),
        None => 0,
    }
}

/// Set the default note format for new notes
pub fn set_default_format(format: NoteFormat) {
    let mut guard = NOTES.lock();
    if let Some(state) = guard.as_mut() {
        state.default_format = format;
    }
}

/// Set the sort mode
pub fn set_sort(mode: NoteSort) {
    let mut guard = NOTES.lock();
    if let Some(state) = guard.as_mut() {
        state.sort_mode = mode;
        sort_notes(&mut state.notes, mode);
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the notes subsystem
pub fn init() {
    let mut guard = NOTES.lock();
    *guard = Some(default_state());
    serial_println!("    Notes ready");
}
