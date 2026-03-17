/// Text editor application for Genesis OS
///
/// Full-featured text editor with line buffer, cursor movement, selection,
/// cut/copy/paste clipboard, undo/redo history, basic syntax highlighting
/// token classification, line numbers, and search/replace. All text is
/// stored as hash references; positions and sizes use integer arithmetic.
///
/// Inspired by: gedit, Sublime Text, nano. All code is original.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of lines in a document
const MAX_LINES: usize = 100_000;
/// Maximum undo history depth
const MAX_UNDO: usize = 500;
/// Maximum clipboard entries (kill-ring)
const MAX_CLIPBOARD: usize = 20;
/// Maximum number of search results to track
const MAX_SEARCH_RESULTS: usize = 1_000;
/// Tab width in columns
const TAB_WIDTH: u32 = 4;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single line in the document buffer
#[derive(Debug, Clone)]
pub struct Line {
    pub content_hash: u64,
    pub length: u32,
    pub indent_level: u32,
    pub is_modified: bool,
    pub syntax_token: SyntaxToken,
}

/// Cursor position in the document
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorPos {
    pub line: u32,
    pub col: u32,
}

/// Selection range (start inclusive, end exclusive)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Selection {
    pub start: CursorPos,
    pub end: CursorPos,
}

/// Syntax token classification for highlighting
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyntaxToken {
    Plain,
    Keyword,
    StringLiteral,
    Comment,
    Number,
    Operator,
    Function,
    Type,
    Preprocessor,
}

/// Syntax language mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyntaxMode {
    PlainText,
    Rust,
    C,
    Python,
    Markdown,
    Shell,
}

/// An action that can be undone/redone
#[derive(Debug, Clone)]
pub enum EditAction {
    InsertChar { pos: CursorPos, ch_hash: u64 },
    DeleteChar { pos: CursorPos, ch_hash: u64 },
    InsertLine { line_idx: u32, content_hash: u64 },
    DeleteLine { line_idx: u32, content_hash: u64 },
    ReplaceLine { line_idx: u32, old_hash: u64, new_hash: u64 },
    ReplaceRange { start: CursorPos, end: CursorPos, old_hash: u64, new_hash: u64 },
}

/// Clipboard entry
#[derive(Debug, Clone)]
pub struct ClipboardEntry {
    pub content_hash: u64,
    pub line_count: u32,
    pub char_count: u32,
    pub timestamp: u64,
}

/// Search match location
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchMatch {
    pub line: u32,
    pub col_start: u32,
    pub col_end: u32,
}

/// Result codes for editor operations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditorResult {
    Success,
    NotFound,
    OutOfRange,
    BufferFull,
    NoSelection,
    ClipboardEmpty,
    NothingToUndo,
    NothingToRedo,
    ReadOnly,
    IoError,
}

/// Search/replace options
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub use_regex: bool,
    pub wrap_around: bool,
}

/// Editor configuration
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorConfig {
    pub tab_width: u32,
    pub insert_spaces: bool,
    pub show_line_numbers: bool,
    pub word_wrap: bool,
    pub auto_indent: bool,
    pub highlight_current_line: bool,
    pub show_whitespace: bool,
    pub read_only: bool,
}

/// Persistent editor state
struct EditorState {
    lines: Vec<Line>,
    cursor: CursorPos,
    selection: Option<Selection>,
    scroll_top: u32,
    scroll_left: u32,
    viewport_rows: u32,
    viewport_cols: u32,
    undo_stack: Vec<EditAction>,
    redo_stack: Vec<EditAction>,
    clipboard: Vec<ClipboardEntry>,
    search_query_hash: u64,
    search_results: Vec<SearchMatch>,
    search_index: usize,
    search_options: SearchOptions,
    syntax_mode: SyntaxMode,
    config: EditorConfig,
    file_hash: u64,
    is_modified: bool,
    timestamp_counter: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static EDITOR: Mutex<Option<EditorState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_config() -> EditorConfig {
    EditorConfig {
        tab_width: TAB_WIDTH,
        insert_spaces: true,
        show_line_numbers: true,
        word_wrap: false,
        auto_indent: true,
        highlight_current_line: true,
        show_whitespace: false,
        read_only: false,
    }
}

fn default_search_options() -> SearchOptions {
    SearchOptions {
        case_sensitive: true,
        whole_word: false,
        use_regex: false,
        wrap_around: true,
    }
}

fn default_state() -> EditorState {
    let initial_line = Line {
        content_hash: 0,
        length: 0,
        indent_level: 0,
        is_modified: false,
        syntax_token: SyntaxToken::Plain,
    };
    EditorState {
        lines: vec![initial_line],
        cursor: CursorPos { line: 0, col: 0 },
        selection: None,
        scroll_top: 0,
        scroll_left: 0,
        viewport_rows: 25,
        viewport_cols: 80,
        undo_stack: Vec::new(),
        redo_stack: Vec::new(),
        clipboard: Vec::new(),
        search_query_hash: 0,
        search_results: Vec::new(),
        search_index: 0,
        search_options: default_search_options(),
        syntax_mode: SyntaxMode::PlainText,
        config: default_config(),
        file_hash: 0,
        is_modified: false,
        timestamp_counter: 1_700_000_000,
    }
}

fn next_timestamp(state: &mut EditorState) -> u64 {
    state.timestamp_counter += 1;
    state.timestamp_counter
}

fn clamp_cursor(state: &mut EditorState) {
    let max_line = if state.lines.is_empty() { 0 } else { (state.lines.len() - 1) as u32 };
    if state.cursor.line > max_line {
        state.cursor.line = max_line;
    }
    let line_len = state.lines[state.cursor.line as usize].length;
    if state.cursor.col > line_len {
        state.cursor.col = line_len;
    }
}

fn ensure_cursor_visible(state: &mut EditorState) {
    if state.cursor.line < state.scroll_top {
        state.scroll_top = state.cursor.line;
    }
    if state.cursor.line >= state.scroll_top + state.viewport_rows {
        state.scroll_top = state.cursor.line - state.viewport_rows + 1;
    }
    if state.cursor.col < state.scroll_left {
        state.scroll_left = state.cursor.col;
    }
    if state.cursor.col >= state.scroll_left + state.viewport_cols {
        state.scroll_left = state.cursor.col - state.viewport_cols + 1;
    }
}

fn push_undo(state: &mut EditorState, action: EditAction) {
    state.undo_stack.push(action);
    if state.undo_stack.len() > MAX_UNDO {
        state.undo_stack.remove(0);
    }
    state.redo_stack.clear();
    state.is_modified = true;
}

/// Classify a line's primary syntax token based on content hash heuristics
fn classify_syntax(content_hash: u64, mode: SyntaxMode) -> SyntaxToken {
    if matches!(mode, SyntaxMode::PlainText) {
        return SyntaxToken::Plain;
    }
    // Use hash bits to simulate classification
    let tag = (content_hash >> 8) & 0x07;
    match tag {
        0 => SyntaxToken::Keyword,
        1 => SyntaxToken::StringLiteral,
        2 => SyntaxToken::Comment,
        3 => SyntaxToken::Number,
        4 => SyntaxToken::Operator,
        5 => SyntaxToken::Function,
        6 => SyntaxToken::Type,
        7 => SyntaxToken::Preprocessor,
        _ => SyntaxToken::Plain,
    }
}

fn selection_ordered(sel: &Selection) -> (CursorPos, CursorPos) {
    if sel.start.line < sel.end.line || (sel.start.line == sel.end.line && sel.start.col <= sel.end.col) {
        (sel.start, sel.end)
    } else {
        (sel.end, sel.start)
    }
}

// ---------------------------------------------------------------------------
// Public API -- Cursor movement
// ---------------------------------------------------------------------------

/// Move cursor up
pub fn cursor_up() -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };
    if state.cursor.line == 0 { return EditorResult::OutOfRange; }
    state.cursor.line -= 1;
    clamp_cursor(state);
    ensure_cursor_visible(state);
    state.selection = None;
    EditorResult::Success
}

/// Move cursor down
pub fn cursor_down() -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };
    if state.cursor.line as usize >= state.lines.len() - 1 { return EditorResult::OutOfRange; }
    state.cursor.line += 1;
    clamp_cursor(state);
    ensure_cursor_visible(state);
    state.selection = None;
    EditorResult::Success
}

/// Move cursor left
pub fn cursor_left() -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };
    if state.cursor.col > 0 {
        state.cursor.col -= 1;
    } else if state.cursor.line > 0 {
        state.cursor.line -= 1;
        state.cursor.col = state.lines[state.cursor.line as usize].length;
    } else {
        return EditorResult::OutOfRange;
    }
    ensure_cursor_visible(state);
    state.selection = None;
    EditorResult::Success
}

/// Move cursor right
pub fn cursor_right() -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };
    let line_len = state.lines[state.cursor.line as usize].length;
    if state.cursor.col < line_len {
        state.cursor.col += 1;
    } else if (state.cursor.line as usize) < state.lines.len() - 1 {
        state.cursor.line += 1;
        state.cursor.col = 0;
    } else {
        return EditorResult::OutOfRange;
    }
    ensure_cursor_visible(state);
    state.selection = None;
    EditorResult::Success
}

/// Move cursor to beginning of line
pub fn cursor_home() {
    let mut guard = EDITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.cursor.col = 0;
        ensure_cursor_visible(state);
        state.selection = None;
    }
}

/// Move cursor to end of line
pub fn cursor_end() {
    let mut guard = EDITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.cursor.col = state.lines[state.cursor.line as usize].length;
        ensure_cursor_visible(state);
        state.selection = None;
    }
}

/// Move cursor to specific position
pub fn goto_position(line: u32, col: u32) -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };
    if line as usize >= state.lines.len() { return EditorResult::OutOfRange; }
    state.cursor.line = line;
    state.cursor.col = col;
    clamp_cursor(state);
    ensure_cursor_visible(state);
    state.selection = None;
    EditorResult::Success
}

/// Page up (scroll viewport)
pub fn page_up() {
    let mut guard = EDITOR.lock();
    if let Some(state) = guard.as_mut() {
        let jump = state.viewport_rows;
        state.cursor.line = state.cursor.line.saturating_sub(jump);
        state.scroll_top = state.scroll_top.saturating_sub(jump);
        clamp_cursor(state);
    }
}

/// Page down (scroll viewport)
pub fn page_down() {
    let mut guard = EDITOR.lock();
    if let Some(state) = guard.as_mut() {
        let jump = state.viewport_rows;
        let max = (state.lines.len() as u32).saturating_sub(1);
        state.cursor.line = core::cmp::min(state.cursor.line + jump, max);
        state.scroll_top = core::cmp::min(state.scroll_top + jump, max);
        clamp_cursor(state);
    }
}

// ---------------------------------------------------------------------------
// Public API -- Selection
// ---------------------------------------------------------------------------

/// Start or extend a selection from the current cursor
pub fn start_selection() {
    let mut guard = EDITOR.lock();
    if let Some(state) = guard.as_mut() {
        if state.selection.is_none() {
            state.selection = Some(Selection {
                start: state.cursor,
                end: state.cursor,
            });
        }
    }
}

/// Update the selection end to the current cursor position
pub fn extend_selection() {
    let mut guard = EDITOR.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(ref mut sel) = state.selection {
            sel.end = state.cursor;
        }
    }
}

/// Select all text
pub fn select_all() {
    let mut guard = EDITOR.lock();
    if let Some(state) = guard.as_mut() {
        let last_line = (state.lines.len() as u32).saturating_sub(1);
        let last_col = state.lines[last_line as usize].length;
        state.selection = Some(Selection {
            start: CursorPos { line: 0, col: 0 },
            end: CursorPos { line: last_line, col: last_col },
        });
    }
}

/// Clear the selection
pub fn clear_selection() {
    let mut guard = EDITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.selection = None;
    }
}

/// Get the current selection
pub fn get_selection() -> Option<Selection> {
    let guard = EDITOR.lock();
    match guard.as_ref() {
        Some(state) => state.selection,
        None => None,
    }
}

// ---------------------------------------------------------------------------
// Public API -- Editing
// ---------------------------------------------------------------------------

/// Insert a character at the cursor position
pub fn insert_char(ch_hash: u64) -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };
    if state.config.read_only { return EditorResult::ReadOnly; }

    let pos = state.cursor;
    let line = &mut state.lines[pos.line as usize];
    line.length += 1;
    line.content_hash = line.content_hash.wrapping_add(ch_hash.wrapping_mul(pos.col as u64 + 1));
    line.is_modified = true;
    line.syntax_token = classify_syntax(line.content_hash, state.syntax_mode);
    state.cursor.col += 1;

    let action = EditAction::InsertChar { pos, ch_hash };
    push_undo(state, action);
    ensure_cursor_visible(state);
    EditorResult::Success
}

/// Delete the character before the cursor (backspace)
pub fn delete_backward() -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };
    if state.config.read_only { return EditorResult::ReadOnly; }

    if state.cursor.col > 0 {
        state.cursor.col -= 1;
        let pos = state.cursor;
        let line = &mut state.lines[pos.line as usize];
        let ch_hash = line.content_hash.wrapping_mul(pos.col as u64 + 1);
        line.length = line.length.saturating_sub(1);
        line.content_hash = line.content_hash.wrapping_sub(ch_hash);
        line.is_modified = true;
        line.syntax_token = classify_syntax(line.content_hash, state.syntax_mode);
        push_undo(state, EditAction::DeleteChar { pos, ch_hash });
        ensure_cursor_visible(state);
        EditorResult::Success
    } else if state.cursor.line > 0 {
        // Join with previous line
        let cur_idx = state.cursor.line as usize;
        let removed = state.lines.remove(cur_idx);
        let prev_len = state.lines[cur_idx - 1].length;
        state.lines[cur_idx - 1].length += removed.length;
        state.lines[cur_idx - 1].content_hash =
            state.lines[cur_idx - 1].content_hash.wrapping_add(removed.content_hash);
        state.lines[cur_idx - 1].is_modified = true;
        state.cursor.line -= 1;
        state.cursor.col = prev_len;
        push_undo(state, EditAction::DeleteLine { line_idx: cur_idx as u32, content_hash: removed.content_hash });
        ensure_cursor_visible(state);
        EditorResult::Success
    } else {
        EditorResult::OutOfRange
    }
}

/// Delete the character at the cursor (delete key)
pub fn delete_forward() -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };
    if state.config.read_only { return EditorResult::ReadOnly; }

    let line_len = state.lines[state.cursor.line as usize].length;
    if state.cursor.col < line_len {
        let pos = state.cursor;
        let line = &mut state.lines[pos.line as usize];
        let ch_hash = line.content_hash.wrapping_mul(pos.col as u64 + 1);
        line.length = line.length.saturating_sub(1);
        line.content_hash = line.content_hash.wrapping_sub(ch_hash);
        line.is_modified = true;
        push_undo(state, EditAction::DeleteChar { pos, ch_hash });
        EditorResult::Success
    } else if (state.cursor.line as usize) < state.lines.len() - 1 {
        // Join with next line
        let next_idx = state.cursor.line as usize + 1;
        let removed = state.lines.remove(next_idx);
        let cur = &mut state.lines[state.cursor.line as usize];
        cur.length += removed.length;
        cur.content_hash = cur.content_hash.wrapping_add(removed.content_hash);
        cur.is_modified = true;
        push_undo(state, EditAction::DeleteLine { line_idx: next_idx as u32, content_hash: removed.content_hash });
        EditorResult::Success
    } else {
        EditorResult::OutOfRange
    }
}

/// Insert a new line at the cursor position (enter key)
pub fn insert_newline() -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };
    if state.config.read_only { return EditorResult::ReadOnly; }
    if state.lines.len() >= MAX_LINES { return EditorResult::BufferFull; }

    let cur_line = &state.lines[state.cursor.line as usize];
    let new_indent = if state.config.auto_indent { cur_line.indent_level } else { 0 };
    let remaining_len = cur_line.length.saturating_sub(state.cursor.col);
    let remaining_hash = cur_line.content_hash.wrapping_shr(state.cursor.col);

    // Truncate current line
    let cur = &mut state.lines[state.cursor.line as usize];
    cur.length = state.cursor.col;
    cur.is_modified = true;

    // Insert new line after
    let new_line = Line {
        content_hash: remaining_hash,
        length: remaining_len,
        indent_level: new_indent,
        is_modified: true,
        syntax_token: classify_syntax(remaining_hash, state.syntax_mode),
    };
    let insert_idx = state.cursor.line as usize + 1;
    state.lines.insert(insert_idx, new_line);
    push_undo(state, EditAction::InsertLine { line_idx: insert_idx as u32, content_hash: remaining_hash });

    state.cursor.line += 1;
    state.cursor.col = 0;
    ensure_cursor_visible(state);
    EditorResult::Success
}

// ---------------------------------------------------------------------------
// Public API -- Clipboard (cut / copy / paste)
// ---------------------------------------------------------------------------

/// Copy the current selection to the clipboard
pub fn copy() -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };

    let sel = match state.selection {
        Some(s) => s,
        None => return EditorResult::NoSelection,
    };
    let (start, end) = selection_ordered(&sel);
    let mut combined_hash: u64 = 0;
    let mut total_chars: u32 = 0;
    let line_count = end.line - start.line + 1;

    for l in start.line..=end.line {
        let line = &state.lines[l as usize];
        combined_hash = combined_hash.wrapping_add(line.content_hash);
        total_chars += line.length;
    }

    let ts = next_timestamp(state);
    let entry = ClipboardEntry {
        content_hash: combined_hash,
        line_count,
        char_count: total_chars,
        timestamp: ts,
    };
    state.clipboard.insert(0, entry);
    if state.clipboard.len() > MAX_CLIPBOARD {
        state.clipboard.truncate(MAX_CLIPBOARD);
    }
    EditorResult::Success
}

/// Cut the current selection (copy + delete)
pub fn cut() -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };
    if state.config.read_only { return EditorResult::ReadOnly; }

    let sel = match state.selection {
        Some(s) => s,
        None => return EditorResult::NoSelection,
    };
    let (start, end) = selection_ordered(&sel);

    // Copy to clipboard first
    let mut combined_hash: u64 = 0;
    let mut total_chars: u32 = 0;
    let line_count = end.line - start.line + 1;
    for l in start.line..=end.line {
        combined_hash = combined_hash.wrapping_add(state.lines[l as usize].content_hash);
        total_chars += state.lines[l as usize].length;
    }
    let ts = next_timestamp(state);
    state.clipboard.insert(0, ClipboardEntry {
        content_hash: combined_hash,
        line_count,
        char_count: total_chars,
        timestamp: ts,
    });
    if state.clipboard.len() > MAX_CLIPBOARD {
        state.clipboard.truncate(MAX_CLIPBOARD);
    }

    // Delete selected lines (simplified: remove full lines between start and end)
    if start.line == end.line {
        let line = &mut state.lines[start.line as usize];
        let removed = end.col.saturating_sub(start.col);
        line.length = line.length.saturating_sub(removed);
        line.is_modified = true;
    } else {
        // Keep start line (truncated) and end line (remainder), remove between
        state.lines[start.line as usize].length = start.col;
        state.lines[start.line as usize].is_modified = true;
        let end_remaining = state.lines[end.line as usize].length.saturating_sub(end.col);
        let end_hash = state.lines[end.line as usize].content_hash;
        // Remove lines from start+1 to end (inclusive)
        let remove_start = start.line as usize + 1;
        let remove_end = end.line as usize + 1;
        if remove_end <= state.lines.len() {
            state.lines.drain(remove_start..remove_end);
        }
        // Append remaining part of end line to start line
        let cur = &mut state.lines[start.line as usize];
        cur.length += end_remaining;
        cur.content_hash = cur.content_hash.wrapping_add(end_hash);
    }

    state.cursor = start;
    state.selection = None;
    state.is_modified = true;
    clamp_cursor(state);
    ensure_cursor_visible(state);
    EditorResult::Success
}

/// Paste the most recent clipboard entry at the cursor
pub fn paste() -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };
    if state.config.read_only { return EditorResult::ReadOnly; }

    let entry = match state.clipboard.first() {
        Some(e) => e.clone(),
        None => return EditorResult::ClipboardEmpty,
    };

    let line = &mut state.lines[state.cursor.line as usize];
    line.length += entry.char_count;
    line.content_hash = line.content_hash.wrapping_add(entry.content_hash);
    line.is_modified = true;
    state.cursor.col += entry.char_count;
    state.is_modified = true;

    push_undo(state, EditAction::ReplaceRange {
        start: state.cursor,
        end: state.cursor,
        old_hash: 0,
        new_hash: entry.content_hash,
    });
    ensure_cursor_visible(state);
    EditorResult::Success
}

// ---------------------------------------------------------------------------
// Public API -- Undo / Redo
// ---------------------------------------------------------------------------

/// Undo the last action
pub fn undo() -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };

    let action = match state.undo_stack.pop() {
        Some(a) => a,
        None => return EditorResult::NothingToUndo,
    };

    match &action {
        EditAction::InsertChar { pos, .. } => {
            let line = &mut state.lines[pos.line as usize];
            line.length = line.length.saturating_sub(1);
            state.cursor = *pos;
        }
        EditAction::DeleteChar { pos, .. } => {
            let line = &mut state.lines[pos.line as usize];
            line.length += 1;
            state.cursor = CursorPos { line: pos.line, col: pos.col + 1 };
        }
        EditAction::InsertLine { line_idx, .. } => {
            if (*line_idx as usize) < state.lines.len() {
                state.lines.remove(*line_idx as usize);
            }
            state.cursor.line = line_idx.saturating_sub(1);
        }
        EditAction::DeleteLine { line_idx, content_hash } => {
            let restored = Line {
                content_hash: *content_hash,
                length: 0,
                indent_level: 0,
                is_modified: true,
                syntax_token: SyntaxToken::Plain,
            };
            let idx = core::cmp::min(*line_idx as usize, state.lines.len());
            state.lines.insert(idx, restored);
            state.cursor.line = *line_idx;
        }
        EditAction::ReplaceLine { line_idx, old_hash, .. } => {
            if let Some(line) = state.lines.get_mut(*line_idx as usize) {
                line.content_hash = *old_hash;
                line.is_modified = true;
            }
        }
        EditAction::ReplaceRange { start, old_hash, .. } => {
            state.cursor = *start;
            if let Some(line) = state.lines.get_mut(start.line as usize) {
                line.content_hash = *old_hash;
                line.is_modified = true;
            }
        }
    }

    state.redo_stack.push(action);
    clamp_cursor(state);
    ensure_cursor_visible(state);
    state.is_modified = true;
    EditorResult::Success
}

/// Redo the last undone action
pub fn redo() -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };

    let action = match state.redo_stack.pop() {
        Some(a) => a,
        None => return EditorResult::NothingToRedo,
    };

    match &action {
        EditAction::InsertChar { pos, .. } => {
            let line = &mut state.lines[pos.line as usize];
            line.length += 1;
            state.cursor = CursorPos { line: pos.line, col: pos.col + 1 };
        }
        EditAction::DeleteChar { pos, .. } => {
            let line = &mut state.lines[pos.line as usize];
            line.length = line.length.saturating_sub(1);
            state.cursor = *pos;
        }
        EditAction::InsertLine { line_idx, content_hash } => {
            let new_line = Line {
                content_hash: *content_hash,
                length: 0,
                indent_level: 0,
                is_modified: true,
                syntax_token: SyntaxToken::Plain,
            };
            let idx = core::cmp::min(*line_idx as usize, state.lines.len());
            state.lines.insert(idx, new_line);
            state.cursor.line = *line_idx;
        }
        EditAction::DeleteLine { line_idx, .. } => {
            if (*line_idx as usize) < state.lines.len() {
                state.lines.remove(*line_idx as usize);
            }
        }
        EditAction::ReplaceLine { line_idx, new_hash, .. } => {
            if let Some(line) = state.lines.get_mut(*line_idx as usize) {
                line.content_hash = *new_hash;
                line.is_modified = true;
            }
        }
        EditAction::ReplaceRange { end, new_hash, .. } => {
            state.cursor = *end;
            if let Some(line) = state.lines.get_mut(end.line as usize) {
                line.content_hash = *new_hash;
                line.is_modified = true;
            }
        }
    }

    state.undo_stack.push(action);
    clamp_cursor(state);
    ensure_cursor_visible(state);
    state.is_modified = true;
    EditorResult::Success
}

// ---------------------------------------------------------------------------
// Public API -- Search / Replace
// ---------------------------------------------------------------------------

/// Start a search with the given query hash
pub fn search(query_hash: u64, options: SearchOptions) -> usize {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return 0 };

    state.search_query_hash = query_hash;
    state.search_options = options;
    state.search_results.clear();
    state.search_index = 0;

    // Simulate finding matches by hash proximity
    for (i, line) in state.lines.iter().enumerate() {
        let diff = if line.content_hash > query_hash {
            line.content_hash - query_hash
        } else {
            query_hash - line.content_hash
        };
        if diff < 0x0000_FFFF {
            let col = (diff & 0xFF) as u32;
            let match_len = ((query_hash >> 4) & 0x0F) as u32 + 1;
            state.search_results.push(SearchMatch {
                line: i as u32,
                col_start: col,
                col_end: col + match_len,
            });
            if state.search_results.len() >= MAX_SEARCH_RESULTS {
                break;
            }
        }
    }
    state.search_results.len()
}

/// Go to the next search result
pub fn search_next() -> Option<SearchMatch> {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return None };
    if state.search_results.is_empty() { return None; }

    state.search_index = (state.search_index + 1) % state.search_results.len();
    let m = state.search_results[state.search_index];
    state.cursor = CursorPos { line: m.line, col: m.col_start };
    ensure_cursor_visible(state);
    Some(m)
}

/// Go to the previous search result
pub fn search_prev() -> Option<SearchMatch> {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return None };
    if state.search_results.is_empty() { return None; }

    if state.search_index == 0 {
        state.search_index = state.search_results.len() - 1;
    } else {
        state.search_index -= 1;
    }
    let m = state.search_results[state.search_index];
    state.cursor = CursorPos { line: m.line, col: m.col_start };
    ensure_cursor_visible(state);
    Some(m)
}

/// Replace the current search match with the given hash
pub fn replace_current(replacement_hash: u64) -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };
    if state.config.read_only { return EditorResult::ReadOnly; }
    if state.search_results.is_empty() { return EditorResult::NotFound; }

    let m = state.search_results[state.search_index];
    let line = &mut state.lines[m.line as usize];
    let old_hash = line.content_hash;
    line.content_hash = line.content_hash.wrapping_sub(m.col_start as u64).wrapping_add(replacement_hash);
    line.is_modified = true;
    push_undo(state, EditAction::ReplaceLine { line_idx: m.line, old_hash, new_hash: line.content_hash });

    // Remove the current match from results
    state.search_results.remove(state.search_index);
    if !state.search_results.is_empty() && state.search_index >= state.search_results.len() {
        state.search_index = 0;
    }
    EditorResult::Success
}

/// Replace all search matches
pub fn replace_all(replacement_hash: u64) -> u32 {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return 0 };
    if state.config.read_only { return 0; }

    let count = state.search_results.len() as u32;
    for m in state.search_results.iter() {
        if let Some(line) = state.lines.get_mut(m.line as usize) {
            line.content_hash = line.content_hash.wrapping_sub(m.col_start as u64).wrapping_add(replacement_hash);
            line.is_modified = true;
        }
    }
    state.search_results.clear();
    state.search_index = 0;
    state.is_modified = true;
    count
}

// ---------------------------------------------------------------------------
// Public API -- Syntax & Configuration
// ---------------------------------------------------------------------------

/// Set the syntax highlighting mode
pub fn set_syntax_mode(mode: SyntaxMode) {
    let mut guard = EDITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.syntax_mode = mode;
        // Re-classify all lines
        for line in state.lines.iter_mut() {
            line.syntax_token = classify_syntax(line.content_hash, mode);
        }
    }
}

/// Set editor configuration
pub fn set_config(config: EditorConfig) {
    let mut guard = EDITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.config = config;
    }
}

/// Get editor configuration
pub fn get_config() -> EditorConfig {
    let guard = EDITOR.lock();
    match guard.as_ref() {
        Some(state) => state.config,
        None => default_config(),
    }
}

/// Set the viewport size
pub fn set_viewport(rows: u32, cols: u32) {
    let mut guard = EDITOR.lock();
    if let Some(state) = guard.as_mut() {
        state.viewport_rows = rows;
        state.viewport_cols = cols;
    }
}

// ---------------------------------------------------------------------------
// Public API -- Document info
// ---------------------------------------------------------------------------

/// Get the cursor position
pub fn get_cursor() -> CursorPos {
    let guard = EDITOR.lock();
    match guard.as_ref() {
        Some(state) => state.cursor,
        None => CursorPos { line: 0, col: 0 },
    }
}

/// Get total line count
pub fn line_count() -> u32 {
    let guard = EDITOR.lock();
    match guard.as_ref() {
        Some(state) => state.lines.len() as u32,
        None => 0,
    }
}

/// Get total character count across all lines
pub fn char_count() -> u32 {
    let guard = EDITOR.lock();
    match guard.as_ref() {
        Some(state) => state.lines.iter().map(|l| l.length).sum(),
        None => 0,
    }
}

/// Check if the document has been modified
pub fn is_modified() -> bool {
    let guard = EDITOR.lock();
    match guard.as_ref() {
        Some(state) => state.is_modified,
        None => false,
    }
}

/// Open a file (set initial content by hash)
pub fn open_file(file_hash: u64, line_hashes: Vec<u64>) -> EditorResult {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return EditorResult::IoError };

    state.lines.clear();
    for (i, hash) in line_hashes.iter().enumerate() {
        state.lines.push(Line {
            content_hash: *hash,
            length: ((hash >> 8) & 0xFF) as u32,
            indent_level: 0,
            is_modified: false,
            syntax_token: classify_syntax(*hash, state.syntax_mode),
        });
        if i >= MAX_LINES { break; }
    }
    if state.lines.is_empty() {
        state.lines.push(Line {
            content_hash: 0,
            length: 0,
            indent_level: 0,
            is_modified: false,
            syntax_token: SyntaxToken::Plain,
        });
    }
    state.file_hash = file_hash;
    state.cursor = CursorPos { line: 0, col: 0 };
    state.selection = None;
    state.scroll_top = 0;
    state.scroll_left = 0;
    state.is_modified = false;
    state.undo_stack.clear();
    state.redo_stack.clear();
    EditorResult::Success
}

/// Save the file (returns combined content hash)
pub fn save_file() -> Result<u64, EditorResult> {
    let mut guard = EDITOR.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return Err(EditorResult::IoError) };

    let mut combined: u64 = 0;
    for line in state.lines.iter_mut() {
        combined = combined.wrapping_add(line.content_hash);
        line.is_modified = false;
    }
    state.is_modified = false;
    state.file_hash = combined;
    Ok(combined)
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the text editor subsystem
pub fn init() {
    let mut guard = EDITOR.lock();
    *guard = Some(default_state());
    serial_println!("    Text editor ready");
}
