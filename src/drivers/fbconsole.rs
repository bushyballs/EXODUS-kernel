use crate::drivers::framebuffer;
use crate::sync::Mutex;
/// Framebuffer text console for Genesis
///
/// Renders text using the bitmap font onto the linear framebuffer.
/// Supports ANSI escape codes for color, cursor movement, and scrolling.
/// This replaces VGA text mode when in graphics mode.
use crate::{serial_print, serial_println};

/// Console dimensions
const MAX_COLS: usize = 128;
const MAX_ROWS: usize = 64;

/// Character cell
#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    fg: u32,
    bg: u32,
}

impl Cell {
    const fn blank() -> Self {
        Cell {
            ch: ' ',
            fg: 0xFFCCCCCC,
            bg: 0xFF121218,
        }
    }
}

/// Framebuffer console state
pub struct FbConsole {
    cells: [[Cell; MAX_COLS]; MAX_ROWS],
    cursor_row: usize,
    cursor_col: usize,
    cols: usize,
    rows: usize,
    fg_color: u32,
    bg_color: u32,
    /// ANSI escape state
    esc_state: EscState,
    esc_buf: [u8; 32],
    esc_idx: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum EscState {
    Normal,
    Escape,
    Csi,
}

impl FbConsole {
    pub const fn new() -> Self {
        FbConsole {
            cells: [[Cell::blank(); MAX_COLS]; MAX_ROWS],
            cursor_row: 0,
            cursor_col: 0,
            cols: 80,
            rows: 25,
            fg_color: 0xFFCCCCCC,
            bg_color: 0xFF121218,
            esc_state: EscState::Normal,
            esc_buf: [0; 32],
            esc_idx: 0,
        }
    }

    /// Initialize console dimensions from framebuffer info
    pub fn init_from_fb(&mut self) {
        if let Some(info) = framebuffer::info() {
            // 8px wide, 16px tall font
            self.cols = (info.width as usize / 8).min(MAX_COLS);
            self.rows = (info.height as usize / 16).min(MAX_ROWS);
        }
    }

    /// Write a character to the console
    pub fn putchar(&mut self, ch: char) {
        match self.esc_state {
            EscState::Normal => {
                match ch {
                    '\x1B' => {
                        self.esc_state = EscState::Escape;
                        self.esc_idx = 0;
                    }
                    '\n' => {
                        self.cursor_col = 0;
                        self.cursor_row = self.cursor_row.saturating_add(1);
                        if self.cursor_row >= self.rows {
                            self.scroll_up();
                        }
                    }
                    '\r' => {
                        self.cursor_col = 0;
                    }
                    '\x08' => {
                        // Backspace
                        if self.cursor_col > 0 {
                            self.cursor_col = self.cursor_col.saturating_sub(1);
                            self.cells[self.cursor_row][self.cursor_col] = Cell {
                                ch: ' ',
                                fg: self.fg_color,
                                bg: self.bg_color,
                            };
                        }
                    }
                    '\t' => {
                        let next_tab = (self.cursor_col + 8) & !7;
                        self.cursor_col = next_tab.min(self.cols.saturating_sub(1));
                    }
                    c => {
                        if self.cursor_col >= self.cols {
                            self.cursor_col = 0;
                            self.cursor_row = self.cursor_row.saturating_add(1);
                            if self.cursor_row >= self.rows {
                                self.scroll_up();
                            }
                        }
                        self.cells[self.cursor_row][self.cursor_col] = Cell {
                            ch: c,
                            fg: self.fg_color,
                            bg: self.bg_color,
                        };
                        self.cursor_col = self.cursor_col.saturating_add(1);
                    }
                }
            }
            EscState::Escape => {
                if ch == '[' {
                    self.esc_state = EscState::Csi;
                } else {
                    self.esc_state = EscState::Normal;
                }
            }
            EscState::Csi => {
                if ch.is_ascii_digit() || ch == ';' {
                    if self.esc_idx < 31 {
                        self.esc_buf[self.esc_idx] = ch as u8;
                        self.esc_idx = self.esc_idx.saturating_add(1);
                    }
                } else {
                    self.process_csi(ch);
                    self.esc_state = EscState::Normal;
                }
            }
        }
    }

    /// Process a CSI escape sequence
    fn process_csi(&mut self, cmd: char) {
        let params = self.parse_params();
        match cmd {
            'm' => {
                // SGR (Select Graphic Rendition)
                if params.is_empty() {
                    // Bare ESC[m is equivalent to ESC[0m
                    self.fg_color = 0xFFCCCCCC;
                    self.bg_color = 0xFF121218;
                }
                for &p in &params {
                    match p {
                        0 => {
                            self.fg_color = 0xFFCCCCCC;
                            self.bg_color = 0xFF121218;
                        }
                        1 => { /* bold -- use bright colors */ }
                        7 => {
                            // Reverse video: swap fg and bg
                            let tmp = self.fg_color;
                            self.fg_color = self.bg_color;
                            self.bg_color = tmp;
                        }
                        // Standard foreground colors (30-37)
                        30 => self.fg_color = 0xFF000000,
                        31 => self.fg_color = 0xFFCC0000,
                        32 => self.fg_color = 0xFF00CC00,
                        33 => self.fg_color = 0xFFCCCC00,
                        34 => self.fg_color = 0xFF0000CC,
                        35 => self.fg_color = 0xFFCC00CC,
                        36 => self.fg_color = 0xFF00CCCC,
                        37 => self.fg_color = 0xFFCCCCCC,
                        39 => self.fg_color = 0xFFCCCCCC, // default fg
                        // Standard background colors (40-47)
                        40 => self.bg_color = 0xFF000000,
                        41 => self.bg_color = 0xFFCC0000,
                        42 => self.bg_color = 0xFF00CC00,
                        43 => self.bg_color = 0xFFCCCC00,
                        44 => self.bg_color = 0xFF0000CC,
                        45 => self.bg_color = 0xFFCC00CC,
                        46 => self.bg_color = 0xFF00CCCC,
                        47 => self.bg_color = 0xFFCCCCCC,
                        49 => self.bg_color = 0xFF121218, // default bg
                        // Bright foreground colors (90-97)
                        90 => self.fg_color = 0xFF555555,
                        91 => self.fg_color = 0xFFFF5555,
                        92 => self.fg_color = 0xFF55FF55,
                        93 => self.fg_color = 0xFFFFFF55,
                        94 => self.fg_color = 0xFF5555FF,
                        95 => self.fg_color = 0xFFFF55FF,
                        96 => self.fg_color = 0xFF55FFFF,
                        97 => self.fg_color = 0xFFFFFFFF,
                        // Bright background colors (100-107)
                        100 => self.bg_color = 0xFF555555,
                        101 => self.bg_color = 0xFFFF5555,
                        102 => self.bg_color = 0xFF55FF55,
                        103 => self.bg_color = 0xFFFFFF55,
                        104 => self.bg_color = 0xFF5555FF,
                        105 => self.bg_color = 0xFFFF55FF,
                        106 => self.bg_color = 0xFF55FFFF,
                        107 => self.bg_color = 0xFFFFFFFF,
                        _ => {}
                    }
                }
            }
            'H' | 'f' => {
                // Cursor position
                let row = params.first().copied().unwrap_or(1).saturating_sub(1);
                let col = params.get(1).copied().unwrap_or(1).saturating_sub(1);
                self.cursor_row = row.min(self.rows.saturating_sub(1));
                self.cursor_col = col.min(self.cols.saturating_sub(1));
            }
            'J' => {
                // Erase in display
                let mode = params.first().copied().unwrap_or(0);
                match mode {
                    0 => {
                        // Erase from cursor to end of screen
                        for col in self.cursor_col..self.cols {
                            self.cells[self.cursor_row][col] = Cell::blank();
                        }
                        for row in (self.cursor_row + 1)..self.rows {
                            for col in 0..self.cols {
                                self.cells[row][col] = Cell::blank();
                            }
                        }
                    }
                    1 => {
                        // Erase from start of screen to cursor
                        for row in 0..self.cursor_row {
                            for col in 0..self.cols {
                                self.cells[row][col] = Cell::blank();
                            }
                        }
                        for col in 0..=self.cursor_col.min(self.cols.saturating_sub(1)) {
                            self.cells[self.cursor_row][col] = Cell::blank();
                        }
                    }
                    2 | 3 => {
                        // Clear entire screen (3 also clears scrollback, but we have none)
                        for row in 0..self.rows {
                            for col in 0..self.cols {
                                self.cells[row][col] = Cell::blank();
                            }
                        }
                        self.cursor_row = 0;
                        self.cursor_col = 0;
                    }
                    _ => {}
                }
            }
            'K' => {
                // Erase in line
                let mode = params.first().copied().unwrap_or(0);
                match mode {
                    0 => {
                        // Erase from cursor to end of line
                        for col in self.cursor_col..self.cols {
                            self.cells[self.cursor_row][col] = Cell::blank();
                        }
                    }
                    1 => {
                        // Erase from start of line to cursor
                        for col in 0..=self.cursor_col.min(self.cols.saturating_sub(1)) {
                            self.cells[self.cursor_row][col] = Cell::blank();
                        }
                    }
                    2 => {
                        // Erase entire line
                        for col in 0..self.cols {
                            self.cells[self.cursor_row][col] = Cell::blank();
                        }
                    }
                    _ => {}
                }
            }
            'L' => {
                // Insert N blank lines at cursor, scrolling lines below down
                let avail = self.rows.saturating_sub(self.cursor_row);
                let n = params.first().copied().unwrap_or(1).min(avail);
                if n > 0 {
                    for row in (self.cursor_row.saturating_add(n)..self.rows).rev() {
                        self.cells[row] = self.cells[row.saturating_sub(n)];
                    }
                    for row in self.cursor_row..(self.cursor_row.saturating_add(n)).min(self.rows) {
                        for col in 0..self.cols {
                            self.cells[row][col] = Cell::blank();
                        }
                    }
                }
            }
            'M' => {
                // Delete N lines at cursor, scrolling lines below up
                let avail = self.rows.saturating_sub(self.cursor_row);
                let n = params.first().copied().unwrap_or(1).min(avail);
                if n > 0 {
                    let end = self.rows.saturating_sub(n);
                    for row in self.cursor_row..end {
                        self.cells[row] =
                            self.cells[row.saturating_add(n).min(self.rows.saturating_sub(1))];
                    }
                    for row in end..self.rows {
                        for col in 0..self.cols {
                            self.cells[row][col] = Cell::blank();
                        }
                    }
                }
            }
            'G' => {
                // Cursor Horizontal Absolute -- move cursor to column N
                let col = params.first().copied().unwrap_or(1).saturating_sub(1);
                self.cursor_col = col.min(self.cols.saturating_sub(1));
            }
            'd' => {
                // Vertical Position Absolute -- move cursor to row N
                let row = params.first().copied().unwrap_or(1).saturating_sub(1);
                self.cursor_row = row.min(self.rows.saturating_sub(1));
            }
            'S' => {
                // Scroll up N lines
                let n = params.first().copied().unwrap_or(1);
                for _ in 0..n {
                    self.scroll_up();
                }
            }
            'T' => {
                // Scroll down N lines
                let n = params.first().copied().unwrap_or(1);
                for _ in 0..n {
                    self.scroll_down();
                }
            }
            'A' => {
                // Cursor up
                let n = params.first().copied().unwrap_or(1);
                self.cursor_row = self.cursor_row.saturating_sub(n);
            }
            'B' => {
                // Cursor down
                let n = params.first().copied().unwrap_or(1);
                self.cursor_row = self
                    .cursor_row
                    .saturating_add(n)
                    .min(self.rows.saturating_sub(1));
            }
            'C' => {
                // Cursor forward
                let n = params.first().copied().unwrap_or(1);
                self.cursor_col = self
                    .cursor_col
                    .saturating_add(n)
                    .min(self.cols.saturating_sub(1));
            }
            'D' => {
                // Cursor back
                let n = params.first().copied().unwrap_or(1);
                self.cursor_col = self.cursor_col.saturating_sub(n);
            }
            _ => {}
        }
    }

    fn parse_params(&self) -> alloc::vec::Vec<usize> {
        let s = core::str::from_utf8(&self.esc_buf[..self.esc_idx]).unwrap_or("");
        s.split(';').filter_map(|p| p.parse().ok()).collect()
    }

    /// Scroll the console up by one line
    fn scroll_up(&mut self) {
        for row in 1..self.rows {
            self.cells[row - 1] = self.cells[row];
        }
        for col in 0..self.cols {
            self.cells[self.rows - 1][col] = Cell::blank();
        }
        self.cursor_row = self.rows - 1;
    }

    /// Scroll the console down by one line
    fn scroll_down(&mut self) {
        for row in (1..self.rows).rev() {
            self.cells[row] = self.cells[row - 1];
        }
        for col in 0..self.cols {
            self.cells[0][col] = Cell::blank();
        }
    }

    /// Write a string to the console
    pub fn write_str(&mut self, s: &str) {
        for ch in s.chars() {
            self.putchar(ch);
        }
    }

    /// Render the entire console to the framebuffer
    pub fn render(&self) {
        if let Some(info) = framebuffer::info() {
            if info.mode != framebuffer::DisplayMode::Graphics {
                return;
            }
            for row in 0..self.rows {
                for col in 0..self.cols {
                    let cell = &self.cells[row][col];
                    self.render_char(col * 8, row * 16, cell.ch, cell.fg, cell.bg, &info);
                }
            }
        }
    }

    /// Render a single character at pixel position
    fn render_char(
        &self,
        px: usize,
        py: usize,
        ch: char,
        fg: u32,
        bg: u32,
        info: &framebuffer::FramebufferInfo,
    ) {
        // Simple 8x16 bitmap font rendering
        let glyph = crate::display::font::get_glyph(ch);
        for y in 0..16u32 {
            let row_bits = glyph[y as usize];
            for x in 0..8u32 {
                let color = if row_bits & (0x80 >> x) != 0 { fg } else { bg };
                let sx = px as u32 + x;
                let sy = py as u32 + y;
                if sx < info.width && sy < info.height {
                    let offset = (sy as usize)
                        .saturating_mul(info.pitch as usize)
                        .saturating_add((sx as usize).saturating_mul(info.bpp as usize));
                    unsafe {
                        // Framebuffer is MMIO — use write_volatile to prevent
                        // the compiler from eliding or reordering pixel stores.
                        core::ptr::write_volatile((info.addr + offset) as *mut u32, color);
                    }
                }
            }
        }
    }
}

static FB_CONSOLE: Mutex<FbConsole> = Mutex::new(FbConsole::new());

/// Initialize the framebuffer console
pub fn init() {
    FB_CONSOLE.lock().init_from_fb();
    super::register("fb-console", super::DeviceType::Display);
    serial_println!("  FbConsole: framebuffer text console ready");
}

/// Write a string to the framebuffer console
pub fn write(s: &str) {
    let mut con = FB_CONSOLE.lock();
    con.write_str(s);
}

/// Render the console to screen
pub fn render() {
    FB_CONSOLE.lock().render();
}

/// Clear the console
pub fn clear() {
    let mut con = FB_CONSOLE.lock();
    con.cursor_row = 0;
    con.cursor_col = 0;
    for row in 0..con.rows {
        for col in 0..con.cols {
            con.cells[row][col] = Cell::blank();
        }
    }
}
