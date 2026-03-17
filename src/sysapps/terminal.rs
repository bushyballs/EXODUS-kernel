/// Terminal emulator application for Genesis OS
///
/// VT100/ANSI-compatible terminal emulator with scrollback buffer,
/// 16-color and 256-color support, cursor modes, alternate screen
/// buffer, and escape sequence parsing. Manages a character cell grid
/// with attribute tracking per cell.
///
/// Inspired by: xterm, GNOME Terminal, Alacritty. All code is original.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default terminal columns
const DEFAULT_COLS: u32 = 80;
/// Default terminal rows
const DEFAULT_ROWS: u32 = 25;
/// Maximum scrollback lines
const MAX_SCROLLBACK: usize = 10_000;
/// Maximum escape sequence parameter count
const MAX_ESC_PARAMS: usize = 16;
/// Maximum input buffer size
const MAX_INPUT_BUFFER: usize = 4096;
/// Tab stop interval
const TAB_STOP: u32 = 8;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Terminal cell color (standard 16 + 256 palette)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TermColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    Palette(u8),
    Default,
}

/// Text attributes for a cell
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellAttrs {
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub inverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
}

/// A single character cell in the terminal grid
#[derive(Debug, Clone, Copy)]
pub struct Cell {
    pub ch: u32,
    pub attrs: CellAttrs,
    pub dirty: bool,
}

/// Cursor shape
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

/// Cursor state
#[derive(Debug, Clone, Copy)]
pub struct CursorState {
    pub row: u32,
    pub col: u32,
    pub visible: bool,
    pub shape: CursorShape,
    pub blinking: bool,
}

/// Parser state for escape sequences
#[derive(Debug, Clone, Copy, PartialEq)]
enum ParseState {
    Normal,
    Escape,
    Csi,
    Osc,
}

/// Terminal mode flags
#[derive(Debug, Clone, Copy)]
pub struct TermModes {
    pub auto_wrap: bool,
    pub cursor_keys_app: bool,
    pub insert_mode: bool,
    pub line_feed_new_line: bool,
    pub origin_mode: bool,
    pub alt_screen: bool,
    pub mouse_tracking: bool,
    pub bracketed_paste: bool,
}

/// Result codes for terminal operations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TermResult {
    Success,
    InvalidSequence,
    BufferFull,
    OutOfRange,
    IoError,
}

/// Scrollback line (stored when lines scroll off the top)
#[derive(Debug, Clone)]
struct ScrollbackLine {
    cells: Vec<Cell>,
    timestamp: u64,
}

/// Persistent terminal state
struct TerminalState {
    cols: u32,
    rows: u32,
    grid: Vec<Vec<Cell>>,
    alt_grid: Vec<Vec<Cell>>,
    cursor: CursorState,
    saved_cursor: CursorState,
    current_attrs: CellAttrs,
    scrollback: Vec<ScrollbackLine>,
    scroll_offset: u32,
    scroll_top: u32,
    scroll_bottom: u32,
    modes: TermModes,
    parse_state: ParseState,
    esc_params: Vec<u32>,
    esc_intermediate: u8,
    input_buffer: Vec<u8>,
    tab_stops: Vec<u32>,
    title_hash: u64,
    timestamp_counter: u64,
    total_bytes_processed: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static TERMINAL: Mutex<Option<TerminalState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_attrs() -> CellAttrs {
    CellAttrs {
        fg: TermColor::Default,
        bg: TermColor::Default,
        bold: false,
        italic: false,
        underline: false,
        blink: false,
        inverse: false,
        hidden: false,
        strikethrough: false,
    }
}

fn empty_cell() -> Cell {
    Cell {
        ch: b' ' as u32,
        attrs: default_attrs(),
        dirty: false,
    }
}

fn make_row(cols: u32) -> Vec<Cell> {
    vec![empty_cell(); cols as usize]
}

fn make_grid(rows: u32, cols: u32) -> Vec<Vec<Cell>> {
    let mut grid = Vec::with_capacity(rows as usize);
    for _ in 0..rows {
        grid.push(make_row(cols));
    }
    grid
}

fn default_tab_stops(cols: u32) -> Vec<u32> {
    let mut stops = Vec::new();
    let mut col = TAB_STOP;
    while col < cols {
        stops.push(col);
        col += TAB_STOP;
    }
    stops
}

fn default_modes() -> TermModes {
    TermModes {
        auto_wrap: true,
        cursor_keys_app: false,
        insert_mode: false,
        line_feed_new_line: false,
        origin_mode: false,
        alt_screen: false,
        mouse_tracking: false,
        bracketed_paste: false,
    }
}

fn default_cursor() -> CursorState {
    CursorState {
        row: 0,
        col: 0,
        visible: true,
        shape: CursorShape::Block,
        blinking: true,
    }
}

fn default_state() -> TerminalState {
    let cols = DEFAULT_COLS;
    let rows = DEFAULT_ROWS;
    TerminalState {
        cols,
        rows,
        grid: make_grid(rows, cols),
        alt_grid: make_grid(rows, cols),
        cursor: default_cursor(),
        saved_cursor: default_cursor(),
        current_attrs: default_attrs(),
        scrollback: Vec::new(),
        scroll_offset: 0,
        scroll_top: 0,
        scroll_bottom: rows - 1,
        modes: default_modes(),
        parse_state: ParseState::Normal,
        esc_params: Vec::new(),
        esc_intermediate: 0,
        input_buffer: Vec::new(),
        tab_stops: default_tab_stops(cols),
        title_hash: 0,
        timestamp_counter: 1_700_000_000,
        total_bytes_processed: 0,
    }
}

fn next_timestamp(state: &mut TerminalState) -> u64 {
    state.timestamp_counter += 1;
    state.timestamp_counter
}

fn scroll_up(state: &mut TerminalState) {
    let top = state.scroll_top as usize;
    let bottom = state.scroll_bottom as usize;
    if top >= state.grid.len() || bottom >= state.grid.len() || top >= bottom {
        return;
    }
    // Save top line to scrollback
    let ts = next_timestamp(state);
    let saved = state.grid[top].clone();
    state.scrollback.push(ScrollbackLine { cells: saved, timestamp: ts });
    if state.scrollback.len() > MAX_SCROLLBACK {
        state.scrollback.remove(0);
    }
    // Shift lines up
    for r in top..bottom {
        state.grid[r] = state.grid[r + 1].clone();
    }
    state.grid[bottom] = make_row(state.cols);
}

fn scroll_down(state: &mut TerminalState) {
    let top = state.scroll_top as usize;
    let bottom = state.scroll_bottom as usize;
    if top >= state.grid.len() || bottom >= state.grid.len() || top >= bottom {
        return;
    }
    // Shift lines down
    for r in (top + 1..=bottom).rev() {
        state.grid[r] = state.grid[r - 1].clone();
    }
    state.grid[top] = make_row(state.cols);
}

fn put_char(state: &mut TerminalState, ch: u32) {
    let row = state.cursor.row as usize;
    let col = state.cursor.col as usize;

    if row < state.grid.len() && col < state.grid[row].len() {
        if state.modes.insert_mode {
            // Shift characters right
            let last = (state.cols as usize).saturating_sub(1);
            for c in (col + 1..=last).rev() {
                state.grid[row][c] = state.grid[row][c - 1];
            }
        }
        state.grid[row][col] = Cell {
            ch,
            attrs: state.current_attrs,
            dirty: true,
        };
    }

    state.cursor.col += 1;
    if state.cursor.col >= state.cols {
        if state.modes.auto_wrap {
            state.cursor.col = 0;
            if state.cursor.row >= state.scroll_bottom {
                scroll_up(state);
            } else {
                state.cursor.row += 1;
            }
        } else {
            state.cursor.col = state.cols - 1;
        }
    }
}

fn get_param(params: &[u32], idx: usize, default: u32) -> u32 {
    if idx < params.len() && params[idx] > 0 { params[idx] } else { default }
}

fn handle_sgr(state: &mut TerminalState) {
    let params = state.esc_params.clone();
    if params.is_empty() {
        state.current_attrs = default_attrs();
        return;
    }
    let mut i = 0;
    while i < params.len() {
        match params[i] {
            0 => state.current_attrs = default_attrs(),
            1 => state.current_attrs.bold = true,
            3 => state.current_attrs.italic = true,
            4 => state.current_attrs.underline = true,
            5 => state.current_attrs.blink = true,
            7 => state.current_attrs.inverse = true,
            8 => state.current_attrs.hidden = true,
            9 => state.current_attrs.strikethrough = true,
            22 => state.current_attrs.bold = false,
            23 => state.current_attrs.italic = false,
            24 => state.current_attrs.underline = false,
            25 => state.current_attrs.blink = false,
            27 => state.current_attrs.inverse = false,
            28 => state.current_attrs.hidden = false,
            29 => state.current_attrs.strikethrough = false,
            30 => state.current_attrs.fg = TermColor::Black,
            31 => state.current_attrs.fg = TermColor::Red,
            32 => state.current_attrs.fg = TermColor::Green,
            33 => state.current_attrs.fg = TermColor::Yellow,
            34 => state.current_attrs.fg = TermColor::Blue,
            35 => state.current_attrs.fg = TermColor::Magenta,
            36 => state.current_attrs.fg = TermColor::Cyan,
            37 => state.current_attrs.fg = TermColor::White,
            38 => {
                // 256-color foreground: ESC[38;5;Nm
                if i + 2 < params.len() && params[i + 1] == 5 {
                    state.current_attrs.fg = TermColor::Palette(params[i + 2] as u8);
                    i += 2;
                }
            }
            39 => state.current_attrs.fg = TermColor::Default,
            40 => state.current_attrs.bg = TermColor::Black,
            41 => state.current_attrs.bg = TermColor::Red,
            42 => state.current_attrs.bg = TermColor::Green,
            43 => state.current_attrs.bg = TermColor::Yellow,
            44 => state.current_attrs.bg = TermColor::Blue,
            45 => state.current_attrs.bg = TermColor::Magenta,
            46 => state.current_attrs.bg = TermColor::Cyan,
            47 => state.current_attrs.bg = TermColor::White,
            48 => {
                // 256-color background: ESC[48;5;Nm
                if i + 2 < params.len() && params[i + 1] == 5 {
                    state.current_attrs.bg = TermColor::Palette(params[i + 2] as u8);
                    i += 2;
                }
            }
            49 => state.current_attrs.bg = TermColor::Default,
            90 => state.current_attrs.fg = TermColor::BrightBlack,
            91 => state.current_attrs.fg = TermColor::BrightRed,
            92 => state.current_attrs.fg = TermColor::BrightGreen,
            93 => state.current_attrs.fg = TermColor::BrightYellow,
            94 => state.current_attrs.fg = TermColor::BrightBlue,
            95 => state.current_attrs.fg = TermColor::BrightMagenta,
            96 => state.current_attrs.fg = TermColor::BrightCyan,
            97 => state.current_attrs.fg = TermColor::BrightWhite,
            100 => state.current_attrs.bg = TermColor::BrightBlack,
            101 => state.current_attrs.bg = TermColor::BrightRed,
            102 => state.current_attrs.bg = TermColor::BrightGreen,
            103 => state.current_attrs.bg = TermColor::BrightYellow,
            104 => state.current_attrs.bg = TermColor::BrightBlue,
            105 => state.current_attrs.bg = TermColor::BrightMagenta,
            106 => state.current_attrs.bg = TermColor::BrightCyan,
            107 => state.current_attrs.bg = TermColor::BrightWhite,
            _ => {}
        }
        i += 1;
    }
}

fn handle_csi(state: &mut TerminalState, final_byte: u8) {
    let params = state.esc_params.clone();
    match final_byte {
        // CUU - Cursor Up
        b'A' => {
            let n = get_param(&params, 0, 1);
            state.cursor.row = state.cursor.row.saturating_sub(n);
            if state.cursor.row < state.scroll_top {
                state.cursor.row = state.scroll_top;
            }
        }
        // CUD - Cursor Down
        b'B' => {
            let n = get_param(&params, 0, 1);
            state.cursor.row = core::cmp::min(state.cursor.row + n, state.scroll_bottom);
        }
        // CUF - Cursor Forward
        b'C' => {
            let n = get_param(&params, 0, 1);
            state.cursor.col = core::cmp::min(state.cursor.col + n, state.cols - 1);
        }
        // CUB - Cursor Back
        b'D' => {
            let n = get_param(&params, 0, 1);
            state.cursor.col = state.cursor.col.saturating_sub(n);
        }
        // CUP / HVP - Cursor Position
        b'H' | b'f' => {
            let row = get_param(&params, 0, 1).saturating_sub(1);
            let col = get_param(&params, 1, 1).saturating_sub(1);
            state.cursor.row = core::cmp::min(row, state.rows - 1);
            state.cursor.col = core::cmp::min(col, state.cols - 1);
        }
        // ED - Erase in Display
        b'J' => {
            let mode = get_param(&params, 0, 0);
            match mode {
                0 => {
                    // Clear from cursor to end
                    let r = state.cursor.row as usize;
                    let c = state.cursor.col as usize;
                    for col in c..state.cols as usize {
                        if r < state.grid.len() && col < state.grid[r].len() {
                            state.grid[r][col] = empty_cell();
                        }
                    }
                    for row in (r + 1)..state.rows as usize {
                        if row < state.grid.len() {
                            state.grid[row] = make_row(state.cols);
                        }
                    }
                }
                1 => {
                    // Clear from start to cursor
                    let r = state.cursor.row as usize;
                    let c = state.cursor.col as usize;
                    for row in 0..r {
                        if row < state.grid.len() {
                            state.grid[row] = make_row(state.cols);
                        }
                    }
                    for col in 0..=c {
                        if r < state.grid.len() && col < state.grid[r].len() {
                            state.grid[r][col] = empty_cell();
                        }
                    }
                }
                2 | 3 => {
                    // Clear entire screen
                    state.grid = make_grid(state.rows, state.cols);
                    if mode == 3 {
                        state.scrollback.clear();
                    }
                }
                _ => {}
            }
        }
        // EL - Erase in Line
        b'K' => {
            let mode = get_param(&params, 0, 0);
            let r = state.cursor.row as usize;
            if r < state.grid.len() {
                match mode {
                    0 => {
                        for c in state.cursor.col as usize..state.cols as usize {
                            if c < state.grid[r].len() {
                                state.grid[r][c] = empty_cell();
                            }
                        }
                    }
                    1 => {
                        for c in 0..=state.cursor.col as usize {
                            if c < state.grid[r].len() {
                                state.grid[r][c] = empty_cell();
                            }
                        }
                    }
                    2 => {
                        state.grid[r] = make_row(state.cols);
                    }
                    _ => {}
                }
            }
        }
        // SU - Scroll Up
        b'S' => {
            let n = get_param(&params, 0, 1);
            for _ in 0..n { scroll_up(state); }
        }
        // SD - Scroll Down
        b'T' => {
            let n = get_param(&params, 0, 1);
            for _ in 0..n { scroll_down(state); }
        }
        // SGR - Select Graphic Rendition
        b'm' => {
            handle_sgr(state);
        }
        // DECSTBM - Set Scrolling Region
        b'r' => {
            let top = get_param(&params, 0, 1).saturating_sub(1);
            let bottom = get_param(&params, 1, state.rows).saturating_sub(1);
            state.scroll_top = core::cmp::min(top, state.rows - 1);
            state.scroll_bottom = core::cmp::min(bottom, state.rows - 1);
            if state.scroll_top >= state.scroll_bottom {
                state.scroll_top = 0;
                state.scroll_bottom = state.rows - 1;
            }
            state.cursor.row = 0;
            state.cursor.col = 0;
        }
        // IL - Insert Lines
        b'L' => {
            let n = get_param(&params, 0, 1);
            for _ in 0..n { scroll_down(state); }
        }
        // DL - Delete Lines
        b'M' => {
            let n = get_param(&params, 0, 1);
            for _ in 0..n { scroll_up(state); }
        }
        // DCH - Delete Characters
        b'P' => {
            let n = get_param(&params, 0, 1) as usize;
            let r = state.cursor.row as usize;
            let c = state.cursor.col as usize;
            if r < state.grid.len() {
                for _ in 0..n {
                    if c < state.grid[r].len() {
                        state.grid[r].remove(c);
                        state.grid[r].push(empty_cell());
                    }
                }
            }
        }
        // ICH - Insert Characters
        b'@' => {
            let n = get_param(&params, 0, 1) as usize;
            let r = state.cursor.row as usize;
            let c = state.cursor.col as usize;
            if r < state.grid.len() {
                for _ in 0..n {
                    if c < state.grid[r].len() {
                        state.grid[r].insert(c, empty_cell());
                        state.grid[r].truncate(state.cols as usize);
                    }
                }
            }
        }
        // DECSET/DECRST (h/l with ? prefix handled via intermediate)
        b'h' | b'l' => {
            let enable = final_byte == b'h';
            if state.esc_intermediate == b'?' {
                for &p in params.iter() {
                    match p {
                        1 => state.modes.cursor_keys_app = enable,
                        6 => state.modes.origin_mode = enable,
                        7 => state.modes.auto_wrap = enable,
                        25 => state.cursor.visible = enable,
                        1000 => state.modes.mouse_tracking = enable,
                        1049 => {
                            if enable {
                                state.saved_cursor = state.cursor;
                                core::mem::swap(&mut state.grid, &mut state.alt_grid);
                                state.grid = make_grid(state.rows, state.cols);
                                state.modes.alt_screen = true;
                            } else {
                                core::mem::swap(&mut state.grid, &mut state.alt_grid);
                                state.cursor = state.saved_cursor;
                                state.modes.alt_screen = false;
                            }
                        }
                        2004 => state.modes.bracketed_paste = enable,
                        _ => {}
                    }
                }
            } else if final_byte == b'h' {
                for &p in params.iter() {
                    if p == 4 { state.modes.insert_mode = true; }
                    if p == 20 { state.modes.line_feed_new_line = true; }
                }
            } else {
                for &p in params.iter() {
                    if p == 4 { state.modes.insert_mode = false; }
                    if p == 20 { state.modes.line_feed_new_line = false; }
                }
            }
        }
        // Save cursor
        b's' => {
            state.saved_cursor = state.cursor;
        }
        // Restore cursor
        b'u' => {
            state.cursor = state.saved_cursor;
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Public API -- Input processing
// ---------------------------------------------------------------------------

/// Process a single byte of terminal input
pub fn process_byte(byte: u8) -> TermResult {
    let mut guard = TERMINAL.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return TermResult::IoError };
    state.total_bytes_processed += 1;

    match state.parse_state {
        ParseState::Normal => {
            match byte {
                0x1B => {
                    state.parse_state = ParseState::Escape;
                    state.esc_params.clear();
                    state.esc_intermediate = 0;
                }
                0x07 => {} // BEL - bell
                0x08 => {  // BS - backspace
                    state.cursor.col = state.cursor.col.saturating_sub(1);
                }
                0x09 => {  // HT - horizontal tab
                    let next_tab = state.tab_stops.iter().find(|&&s| s > state.cursor.col);
                    state.cursor.col = match next_tab {
                        Some(&t) => core::cmp::min(t, state.cols - 1),
                        None => state.cols - 1,
                    };
                }
                0x0A | 0x0B | 0x0C => {  // LF, VT, FF
                    if state.cursor.row >= state.scroll_bottom {
                        scroll_up(state);
                    } else {
                        state.cursor.row += 1;
                    }
                    if state.modes.line_feed_new_line {
                        state.cursor.col = 0;
                    }
                }
                0x0D => {  // CR
                    state.cursor.col = 0;
                }
                0x20..=0x7E => {
                    put_char(state, byte as u32);
                }
                _ => {} // Ignore other control characters
            }
        }
        ParseState::Escape => {
            match byte {
                b'[' => {
                    state.parse_state = ParseState::Csi;
                    state.esc_params.clear();
                    state.esc_params.push(0);
                }
                b']' => {
                    state.parse_state = ParseState::Osc;
                }
                b'7' => {
                    state.saved_cursor = state.cursor;
                    state.parse_state = ParseState::Normal;
                }
                b'8' => {
                    state.cursor = state.saved_cursor;
                    state.parse_state = ParseState::Normal;
                }
                b'D' => {
                    // Index - move down, scroll if needed
                    if state.cursor.row >= state.scroll_bottom {
                        scroll_up(state);
                    } else {
                        state.cursor.row += 1;
                    }
                    state.parse_state = ParseState::Normal;
                }
                b'M' => {
                    // Reverse Index - move up, scroll if needed
                    if state.cursor.row <= state.scroll_top {
                        scroll_down(state);
                    } else {
                        state.cursor.row -= 1;
                    }
                    state.parse_state = ParseState::Normal;
                }
                b'c' => {
                    // Full reset (RIS)
                    *state = default_state();
                }
                _ => {
                    state.parse_state = ParseState::Normal;
                }
            }
        }
        ParseState::Csi => {
            match byte {
                b'0'..=b'9' => {
                    let last = state.esc_params.len().saturating_sub(1);
                    if let Some(p) = state.esc_params.get_mut(last) {
                        *p = p.saturating_mul(10).saturating_add((byte - b'0') as u32);
                    }
                }
                b';' => {
                    if state.esc_params.len() < MAX_ESC_PARAMS {
                        state.esc_params.push(0);
                    }
                }
                b'?' | b'>' | b'!' => {
                    state.esc_intermediate = byte;
                }
                0x40..=0x7E => {
                    handle_csi(state, byte);
                    state.parse_state = ParseState::Normal;
                }
                _ => {
                    state.parse_state = ParseState::Normal;
                }
            }
        }
        ParseState::Osc => {
            // OSC sequences end with BEL (0x07) or ST (ESC \)
            if byte == 0x07 {
                state.parse_state = ParseState::Normal;
            } else if byte == 0x1B {
                // Could be start of ST (\), simplified: just reset
                state.parse_state = ParseState::Normal;
            }
            // Otherwise accumulate (ignored for now, title handling would go here)
        }
    }
    TermResult::Success
}

/// Process a sequence of bytes
pub fn process_bytes(data: &[u8]) -> u32 {
    let mut count = 0;
    for &byte in data {
        if process_byte(byte) == TermResult::Success {
            count += 1;
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Public API -- Keyboard input
// ---------------------------------------------------------------------------

/// Queue keyboard input (data to send to the shell)
pub fn queue_input(data: &[u8]) -> TermResult {
    let mut guard = TERMINAL.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return TermResult::IoError };
    if state.input_buffer.len() + data.len() > MAX_INPUT_BUFFER {
        return TermResult::BufferFull;
    }
    state.input_buffer.extend_from_slice(data);
    TermResult::Success
}

/// Drain the pending input buffer
pub fn drain_input() -> Vec<u8> {
    let mut guard = TERMINAL.lock();
    match guard.as_mut() {
        Some(state) => {
            let data = state.input_buffer.clone();
            state.input_buffer.clear();
            data
        }
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Public API -- Display / Query
// ---------------------------------------------------------------------------

/// Get a cell at a specific position
pub fn get_cell(row: u32, col: u32) -> Option<Cell> {
    let guard = TERMINAL.lock();
    let state = guard.as_ref()?;
    let r = row as usize;
    let c = col as usize;
    if r < state.grid.len() && c < state.grid[r].len() {
        Some(state.grid[r][c])
    } else {
        None
    }
}

/// Get the cursor state
pub fn get_cursor() -> CursorState {
    let guard = TERMINAL.lock();
    match guard.as_ref() {
        Some(state) => state.cursor,
        None => default_cursor(),
    }
}

/// Get terminal dimensions
pub fn get_dimensions() -> (u32, u32) {
    let guard = TERMINAL.lock();
    match guard.as_ref() {
        Some(state) => (state.cols, state.rows),
        None => (DEFAULT_COLS, DEFAULT_ROWS),
    }
}

/// Resize the terminal
pub fn resize(new_cols: u32, new_rows: u32) -> TermResult {
    let mut guard = TERMINAL.lock();
    let state = match guard.as_mut() { Some(s) => s, None => return TermResult::IoError };
    if new_cols == 0 || new_rows == 0 { return TermResult::OutOfRange; }

    state.grid = make_grid(new_rows, new_cols);
    state.alt_grid = make_grid(new_rows, new_cols);
    state.cols = new_cols;
    state.rows = new_rows;
    state.scroll_top = 0;
    state.scroll_bottom = new_rows - 1;
    state.tab_stops = default_tab_stops(new_cols);
    state.cursor.row = core::cmp::min(state.cursor.row, new_rows - 1);
    state.cursor.col = core::cmp::min(state.cursor.col, new_cols - 1);
    TermResult::Success
}

/// Get scrollback line count
pub fn scrollback_len() -> usize {
    let guard = TERMINAL.lock();
    match guard.as_ref() {
        Some(state) => state.scrollback.len(),
        None => 0,
    }
}

/// Scroll the view into scrollback buffer
pub fn scroll_view(offset: u32) {
    let mut guard = TERMINAL.lock();
    if let Some(state) = guard.as_mut() {
        state.scroll_offset = core::cmp::min(offset, state.scrollback.len() as u32);
    }
}

/// Get total bytes processed
pub fn total_bytes_processed() -> u64 {
    let guard = TERMINAL.lock();
    match guard.as_ref() {
        Some(state) => state.total_bytes_processed,
        None => 0,
    }
}

/// Set cursor shape
pub fn set_cursor_shape(shape: CursorShape) {
    let mut guard = TERMINAL.lock();
    if let Some(state) = guard.as_mut() {
        state.cursor.shape = shape;
    }
}

/// Set cursor blinking
pub fn set_cursor_blinking(blinking: bool) {
    let mut guard = TERMINAL.lock();
    if let Some(state) = guard.as_mut() {
        state.cursor.blinking = blinking;
    }
}

/// Reset the terminal to initial state
pub fn reset() {
    let mut guard = TERMINAL.lock();
    *guard = Some(default_state());
}

/// Get the current terminal modes
pub fn get_modes() -> TermModes {
    let guard = TERMINAL.lock();
    match guard.as_ref() {
        Some(state) => state.modes,
        None => default_modes(),
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the terminal emulator subsystem
pub fn init() {
    let mut guard = TERMINAL.lock();
    *guard = Some(default_state());
    serial_println!("    Terminal emulator ready ({}x{})", DEFAULT_COLS, DEFAULT_ROWS);
}
