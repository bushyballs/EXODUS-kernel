use crate::sync::Mutex;
/// Virtual Terminal Multiplexer for Genesis
///
/// Provides multiple virtual terminals within a single window,
/// with the ability to switch between them. Similar to tmux/screen.
///
/// Features:
///   - Multiple independent terminal sessions
///   - Split panes (horizontal and vertical)
///   - Terminal switching with keyboard shortcuts
///   - Scrollback buffer per terminal
///   - Status bar showing active sessions
use crate::{serial_print, serial_println};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

/// Maximum terminals
const MAX_TERMINALS: usize = 16;
/// Scrollback buffer lines
const SCROLLBACK_LINES: usize = 1000;
/// Maximum columns per terminal
const MAX_COLS: usize = 200;

/// A single virtual terminal
pub struct VirtualTerminal {
    pub id: u32,
    pub name: String,
    /// Screen buffer (lines of characters)
    pub screen: VecDeque<Vec<TermCell>>,
    /// Scrollback buffer (past lines)
    pub scrollback: VecDeque<Vec<TermCell>>,
    /// Cursor position
    pub cursor_row: usize,
    pub cursor_col: usize,
    /// Terminal dimensions
    pub cols: usize,
    pub rows: usize,
    /// Current foreground/background colors
    pub fg: u32,
    pub bg: u32,
    /// Process group running in this terminal
    pub pgid: u32,
    /// Whether this terminal is active
    pub active: bool,
    /// Scroll offset for viewing scrollback
    pub scroll_offset: usize,
}

/// A character cell in the terminal
#[derive(Clone, Copy)]
pub struct TermCell {
    pub ch: char,
    pub fg: u32,
    pub bg: u32,
}

impl TermCell {
    pub const fn blank() -> Self {
        TermCell {
            ch: ' ',
            fg: 0xFFCCCCCC,
            bg: 0xFF121218,
        }
    }
}

impl VirtualTerminal {
    pub fn new(id: u32, name: &str, cols: usize, rows: usize) -> Self {
        let mut screen = VecDeque::new();
        for _ in 0..rows {
            screen.push_back(alloc::vec![TermCell::blank(); cols]);
        }

        VirtualTerminal {
            id,
            name: String::from(name),
            screen,
            scrollback: VecDeque::new(),
            cursor_row: 0,
            cursor_col: 0,
            cols,
            rows,
            fg: 0xFFCCCCCC,
            bg: 0xFF121218,
            pgid: 0,
            active: false,
            scroll_offset: 0,
        }
    }

    /// Write a character to the terminal
    pub fn putchar(&mut self, ch: char) {
        match ch {
            '\n' => {
                self.cursor_col = 0;
                self.cursor_row += 1;
                if self.cursor_row >= self.rows {
                    self.scroll_up();
                }
            }
            '\r' => {
                self.cursor_col = 0;
            }
            '\x08' => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                    if let Some(row) = self.screen.get_mut(self.cursor_row) {
                        if self.cursor_col < row.len() {
                            row[self.cursor_col] = TermCell::blank();
                        }
                    }
                }
            }
            '\t' => {
                self.cursor_col = ((self.cursor_col + 8) & !7).min(self.cols - 1);
            }
            c => {
                if self.cursor_col >= self.cols {
                    self.cursor_col = 0;
                    self.cursor_row += 1;
                    if self.cursor_row >= self.rows {
                        self.scroll_up();
                    }
                }
                if let Some(row) = self.screen.get_mut(self.cursor_row) {
                    if self.cursor_col < row.len() {
                        row[self.cursor_col] = TermCell {
                            ch: c,
                            fg: self.fg,
                            bg: self.bg,
                        };
                    }
                }
                self.cursor_col += 1;
            }
        }
    }

    /// Write a string
    pub fn write_str(&mut self, s: &str) {
        for ch in s.chars() {
            self.putchar(ch);
        }
    }

    /// Scroll up by one line (move top line to scrollback)
    fn scroll_up(&mut self) {
        if let Some(top_line) = self.screen.pop_front() {
            self.scrollback.push_back(top_line);
            // Trim scrollback if too long
            while self.scrollback.len() > SCROLLBACK_LINES {
                self.scrollback.pop_front();
            }
        }
        self.screen
            .push_back(alloc::vec![TermCell::blank(); self.cols]);
        self.cursor_row = self.rows - 1;
    }

    /// Clear the terminal
    pub fn clear(&mut self) {
        for row in self.screen.iter_mut() {
            for cell in row.iter_mut() {
                *cell = TermCell::blank();
            }
        }
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    /// Scroll view up (into scrollback)
    pub fn scroll_view_up(&mut self, lines: usize) {
        self.scroll_offset = (self.scroll_offset + lines).min(self.scrollback.len());
    }

    /// Scroll view down (towards current)
    pub fn scroll_view_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }
}

/// Split direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    Horizontal,
    Vertical,
}

/// A pane in the multiplexer (may contain a terminal or be split)
pub struct Pane {
    pub terminal_id: Option<u32>,
    pub split: Option<(SplitDir, Box<Pane>, Box<Pane>)>,
    /// Pane bounds (relative to window)
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Terminal multiplexer state
pub struct VtMux {
    pub terminals: Vec<VirtualTerminal>,
    pub active_id: u32,
    pub next_id: u32,
    pub status_visible: bool,
}

impl VtMux {
    pub const fn new() -> Self {
        VtMux {
            terminals: Vec::new(),
            active_id: 0,
            next_id: 1,
            status_visible: true,
        }
    }

    /// Create a new terminal
    pub fn create_terminal(&mut self, name: &str, cols: usize, rows: usize) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let mut term = VirtualTerminal::new(id, name, cols, rows);
        if self.terminals.is_empty() {
            term.active = true;
            self.active_id = id;
        }

        self.terminals.push(term);
        serial_println!("  VtMux: created terminal {} '{}'", id, name);
        id
    }

    /// Switch to a terminal by ID
    pub fn switch_to(&mut self, id: u32) {
        for term in &mut self.terminals {
            term.active = term.id == id;
        }
        self.active_id = id;
    }

    /// Switch to next terminal
    pub fn next_terminal(&mut self) {
        if self.terminals.is_empty() {
            return;
        }
        let idx = self
            .terminals
            .iter()
            .position(|t| t.id == self.active_id)
            .unwrap_or(0);
        let next = (idx + 1) % self.terminals.len();
        let next_id = self.terminals[next].id;
        self.switch_to(next_id);
    }

    /// Switch to previous terminal
    pub fn prev_terminal(&mut self) {
        if self.terminals.is_empty() {
            return;
        }
        let idx = self
            .terminals
            .iter()
            .position(|t| t.id == self.active_id)
            .unwrap_or(0);
        let prev = if idx == 0 {
            self.terminals.len() - 1
        } else {
            idx - 1
        };
        let prev_id = self.terminals[prev].id;
        self.switch_to(prev_id);
    }

    /// Close a terminal
    pub fn close_terminal(&mut self, id: u32) {
        self.terminals.retain(|t| t.id != id);
        if self.active_id == id {
            if let Some(first) = self.terminals.first() {
                self.active_id = first.id;
            }
        }
        for term in &mut self.terminals {
            term.active = term.id == self.active_id;
        }
    }

    /// Write to the active terminal
    pub fn write(&mut self, s: &str) {
        if let Some(term) = self.terminals.iter_mut().find(|t| t.id == self.active_id) {
            term.write_str(s);
        }
    }

    /// Get the active terminal
    pub fn active_terminal(&self) -> Option<&VirtualTerminal> {
        self.terminals.iter().find(|t| t.id == self.active_id)
    }

    /// Get active terminal mutably
    pub fn active_terminal_mut(&mut self) -> Option<&mut VirtualTerminal> {
        let id = self.active_id;
        self.terminals.iter_mut().find(|t| t.id == id)
    }

    /// Get the status bar text
    pub fn status_bar(&self) -> String {
        let mut bar = String::from("[vtmux] ");
        for term in &self.terminals {
            if term.id == self.active_id {
                bar.push_str(&alloc::format!("[*{}: {}] ", term.id, term.name));
            } else {
                bar.push_str(&alloc::format!("[{}: {}] ", term.id, term.name));
            }
        }
        bar
    }
}

/// Global terminal multiplexer
pub static VTMUX: Mutex<VtMux> = Mutex::new(VtMux::new());

/// Initialize the virtual terminal multiplexer
pub fn init() {
    let mut mux = VTMUX.lock();
    mux.create_terminal("main", 80, 24);
    serial_println!("  VtMux: virtual terminal multiplexer ready");
}

/// Create a new terminal session
pub fn create(name: &str) -> u32 {
    VTMUX.lock().create_terminal(name, 80, 24)
}

/// Switch to next terminal
pub fn next() {
    VTMUX.lock().next_terminal();
}

/// Switch to previous terminal
pub fn prev() {
    VTMUX.lock().prev_terminal();
}

/// Write to active terminal
pub fn write(s: &str) {
    VTMUX.lock().write(s);
}
