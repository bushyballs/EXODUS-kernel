use crate::sync::Mutex;
/// TTY subsystem — terminal line discipline
///
/// This module is the public face of the TTY driver.  It:
///   - Defines all shared types (Termios, Tty, TtyMode, EscState, CellAttr, …)
///   - Owns the global TTY and PTY tables
///   - Re-exports the public API of the three sub-modules
///
/// Sub-module layout:
///   mod emulator  — VT100/ANSI escape sequence state machine (output side)
///   mod input     — line discipline: canonical mode, echo, signal generation
///   mod output    — output buffering, post-processing flags, scrollback, drain
///
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

// Sub-modules
pub mod emulator;
pub mod input;
pub mod output;

// ── Capacity constants (shared across all sub-modules) ────────────────────────

pub(super) const INPUT_BUF_CAPACITY: usize = 4096;
pub(super) const QUEUE_CAPACITY: usize = 8192;
pub(super) const SCROLLBACK_MAX_LINES: usize = 1000;
pub(super) const ESC_PARAM_MAX: usize = 16;

const MAX_TTYS: usize = 16;
const MAX_PTYS: usize = 64;

// ── Termios input flags ───────────────────────────────────────────────────────

pub(super) const IGNBRK: u32 = 0x001;
pub(super) const BRKINT: u32 = 0x002;
pub(super) const IGNPAR: u32 = 0x004;
pub(super) const PARMRK: u32 = 0x008;
pub(super) const INPCK: u32 = 0x010;
pub(super) const ISTRIP: u32 = 0x020;
pub(super) const INLCR: u32 = 0x040;
pub(super) const IGNCR: u32 = 0x080;
pub(super) const ICRNL: u32 = 0x100;
pub(super) const IXON: u32 = 0x200;
pub(super) const IXOFF: u32 = 0x400;
pub(super) const IXANY: u32 = 0x800;

// Output flags
pub(super) const OPOST: u32 = 0x001;
pub(super) const ONLCR: u32 = 0x004;
pub(super) const OCRNL: u32 = 0x008;
pub(super) const ONOCR: u32 = 0x010;
pub(super) const ONLRET: u32 = 0x020;
pub(super) const TABDLY: u32 = 0x1800;

// Control flags
pub(super) const CS5: u32 = 0x000;
pub(super) const CS6: u32 = 0x100;
pub(super) const CS7: u32 = 0x200;
pub(super) const CS8: u32 = 0x300;
pub(super) const CSTOPB: u32 = 0x400;
pub(super) const CREAD: u32 = 0x800;
pub(super) const PARENB: u32 = 0x1000;
pub(super) const PARODD: u32 = 0x2000;
pub(super) const HUPCL: u32 = 0x4000;
pub(super) const CLOCAL: u32 = 0x8000;

// Local flags
pub(super) const ISIG: u32 = 0x001;
pub(super) const ICANON: u32 = 0x002;
pub(super) const ECHO: u32 = 0x008;
pub(super) const ECHOE: u32 = 0x010;
pub(super) const ECHOK: u32 = 0x020;
pub(super) const ECHONL: u32 = 0x040;
pub(super) const NOFLSH: u32 = 0x080;
pub(super) const TOSTOP: u32 = 0x100;
pub(super) const ECHOCTL: u32 = 0x200;
pub(super) const IEXTEN: u32 = 0x8000;

// ── TtyMode ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtyMode {
    Cooked,
    Raw,
    CBreak,
}

// ── ControlChars ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ControlChars {
    pub vintr: u8,
    pub vquit: u8,
    pub verase: u8,
    pub vkill: u8,
    pub veof: u8,
    pub vtime: u8,
    pub vmin: u8,
    pub vsusp: u8,
    pub vstart: u8,
    pub vstop: u8,
    pub vwerase: u8,
    pub vlnext: u8,
    pub vreprint: u8,
}

impl ControlChars {
    pub const fn default() -> Self {
        ControlChars {
            vintr: 0x03,  // Ctrl+C
            vquit: 0x1C,  // Ctrl+backslash
            verase: 0x7F, // DEL
            vkill: 0x15,  // Ctrl+U
            veof: 0x04,   // Ctrl+D
            vtime: 0,
            vmin: 1,
            vsusp: 0x1A,    // Ctrl+Z
            vstart: 0x11,   // Ctrl+Q
            vstop: 0x13,    // Ctrl+S
            vwerase: 0x17,  // Ctrl+W
            vlnext: 0x16,   // Ctrl+V
            vreprint: 0x12, // Ctrl+R
        }
    }
}

// ── Termios ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Termios {
    pub iflag: u32,
    pub oflag: u32,
    pub cflag: u32,
    pub lflag: u32,
    pub mode: TtyMode,
    pub echo: bool,
    pub isig: bool,
    pub icanon: bool,
    pub cc: ControlChars,
}

impl Termios {
    pub const fn default_cooked() -> Self {
        Termios {
            iflag: ICRNL,
            oflag: OPOST | ONLCR,
            cflag: CS8 | CREAD,
            lflag: ECHO | ECHOE | ECHOK | ECHOCTL | ICANON | ISIG | IEXTEN,
            mode: TtyMode::Cooked,
            echo: true,
            isig: true,
            icanon: true,
            cc: ControlChars::default(),
        }
    }

    pub const fn raw() -> Self {
        Termios {
            iflag: 0,
            oflag: 0,
            cflag: CS8 | CREAD,
            lflag: 0,
            mode: TtyMode::Raw,
            echo: false,
            isig: false,
            icanon: false,
            cc: ControlChars {
                vtime: 0,
                vmin: 1,
                ..ControlChars::default()
            },
        }
    }

    /// Re-derive `mode`, `echo`, `isig`, `icanon` from the raw flag bits.
    pub fn update_derived(&mut self) {
        self.icanon = self.lflag & ICANON != 0;
        self.echo = self.lflag & ECHO != 0;
        self.isig = self.lflag & ISIG != 0;
        self.mode = if self.icanon {
            TtyMode::Cooked
        } else if self.isig {
            TtyMode::CBreak
        } else {
            TtyMode::Raw
        };
    }
}

// ── EscState ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscState {
    Normal,
    Escape,
    Csi,
    Osc,
    SingleShift,
}

// ── CellAttr ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct CellAttr {
    pub bold: bool,
    pub dim: bool,
    pub underline: bool,
    pub blink: bool,
    pub inverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
    pub fg_color: u8, // 0-7 standard palette; 8 = default
    pub bg_color: u8,
}

impl CellAttr {
    pub const fn default() -> Self {
        CellAttr {
            bold: false,
            dim: false,
            underline: false,
            blink: false,
            inverse: false,
            hidden: false,
            strikethrough: false,
            fg_color: 8,
            bg_color: 8,
        }
    }

    pub fn reset(&mut self) {
        *self = CellAttr::default();
    }
}

// ── TtyType ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtyType {
    Console,
    PtySlave,
    Serial,
}

// ── Tty struct ────────────────────────────────────────────────────────────────

pub struct Tty {
    pub id: u32,
    pub name: String,
    pub tty_type: TtyType,
    pub termios: Termios,
    pub input_buf: String,
    pub read_queue: VecDeque<u8>,
    pub output_queue: VecDeque<u8>,
    pub fg_pgid: u32,
    pub session_id: u32,
    pub rows: u16,
    pub cols: u16,
    pub controlling: bool,
    pub pty_master_idx: Option<usize>,
    pub output_stopped: bool,
    pub column: u32,
    pub row: u32,
    pub bytes_read: u64,
    pub bytes_written: u64,
    // Escape-sequence parser state (used by emulator sub-module)
    pub esc_state: EscState,
    pub esc_params: [u16; ESC_PARAM_MAX],
    pub esc_param_count: usize,
    pub esc_current_param: u16,
    pub esc_intermediate: u8,
    pub saved_cursor: (u32, u32),
    // Scrollback buffer (used by output sub-module)
    pub scrollback: VecDeque<Vec<u8>>,
    pub current_line: Vec<u8>,
    pub scroll_offset: usize,
    // Text cell attributes (SGR state, set by emulator)
    pub attr: CellAttr,
}

impl Tty {
    pub fn new(id: u32, name: &str, rows: u16, cols: u16) -> Self {
        Tty {
            id,
            name: String::from(name),
            tty_type: TtyType::Console,
            termios: Termios::default_cooked(),
            input_buf: String::new(),
            read_queue: VecDeque::new(),
            output_queue: VecDeque::new(),
            fg_pgid: 0,
            session_id: 0,
            rows,
            cols,
            controlling: false,
            pty_master_idx: None,
            output_stopped: false,
            column: 0,
            row: 0,
            bytes_read: 0,
            bytes_written: 0,
            esc_state: EscState::Normal,
            esc_params: [0; ESC_PARAM_MAX],
            esc_param_count: 0,
            esc_current_param: 0,
            esc_intermediate: 0,
            saved_cursor: (0, 0),
            scrollback: VecDeque::new(),
            current_line: Vec::new(),
            scroll_offset: 0,
            attr: CellAttr::default(),
        }
    }

    // ── Input ────────────────────────────────────────────────────────────

    /// Process one keyboard byte through the full line discipline.
    pub fn input_char(&mut self, ch: u8) {
        input::input_char(self, ch);
    }

    /// Read available bytes into `buf`. Returns bytes copied.
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        input::tty_read(self, buf)
    }

    /// Returns `true` when data is available for a `read()` call.
    pub fn can_read(&self) -> bool {
        input::can_read(self)
    }

    // ── Output ───────────────────────────────────────────────────────────

    /// Write raw bytes to the output queue (OPOST applied).
    pub fn write(&mut self, data: &[u8]) {
        output::write(self, data);
    }

    /// Write bytes through the ANSI/VT100 emulator.
    pub fn write_ansi(&mut self, data: &[u8]) {
        output::write_ansi(self, data);
    }

    /// Drain the output queue into a Vec (called by the display driver).
    pub fn drain_output(&mut self) -> Vec<u8> {
        output::drain_output(self)
    }

    /// Return cursor position as (row, col).
    pub fn cursor_pos(&self) -> (u32, u32) {
        (self.row, self.column)
    }

    /// Return current text attributes.
    pub fn current_attr(&self) -> CellAttr {
        self.attr
    }

    // ── Scrollback ───────────────────────────────────────────────────────

    pub fn scroll_view_up(&mut self, n: usize) {
        output::scroll_view_up(self, n);
    }

    pub fn scroll_view_down(&mut self, n: usize) {
        output::scroll_view_down(self, n);
    }

    pub fn scroll_view_reset(&mut self) {
        output::scroll_view_reset(self);
    }

    pub fn visible_scrollback(&self, rows: usize) -> Vec<&[u8]> {
        output::visible_scrollback(self, rows)
    }

    pub fn scrollback_len(&self) -> usize {
        output::scrollback_len(self)
    }

    pub fn clear_scrollback(&mut self) {
        output::clear_scrollback(self);
    }

    // ── Termios / settings ───────────────────────────────────────────────

    pub fn set_raw(&mut self) {
        self.termios = Termios::raw();
    }

    pub fn set_cooked(&mut self) {
        self.termios = Termios::default_cooked();
    }

    pub fn set_termios(&mut self, mut termios: Termios) {
        termios.update_derived();
        self.termios = termios;
    }

    pub fn get_termios(&self) -> Termios {
        self.termios
    }

    pub fn winsize(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    pub fn set_winsize(&mut self, rows: u16, cols: u16) {
        let changed = self.rows != rows || self.cols != cols;
        self.rows = rows;
        self.cols = cols;
        if changed && self.fg_pgid > 0 {
            let _ =
                crate::process::send_signal(self.fg_pgid, crate::process::pcb::signal::SIGWINCH);
        }
    }

    pub fn set_fg_pgid(&mut self, pgid: u32) {
        self.fg_pgid = pgid;
    }

    pub fn flush_input(&mut self) {
        self.input_buf.clear();
        self.read_queue.clear();
    }

    pub fn flush_output(&mut self) {
        output::flush_output(self);
    }

    pub fn flush_all(&mut self) {
        self.flush_input();
        self.flush_output();
    }
}

// ── PTY master ────────────────────────────────────────────────────────────────

pub struct PtyMaster {
    pub id: u32,
    pub to_slave: VecDeque<u8>,
    pub from_slave: VecDeque<u8>,
    pub open: bool,
    pub slave_tty_idx: usize,
}

impl PtyMaster {
    pub fn write_to_slave(&mut self, data: &[u8]) {
        for &b in data {
            if self.to_slave.len() < QUEUE_CAPACITY {
                self.to_slave.push_back(b);
            }
        }
    }

    pub fn read_from_slave(&mut self, buf: &mut [u8]) -> usize {
        let mut count = 0;
        while count < buf.len() {
            if let Some(b) = self.from_slave.pop_front() {
                buf[count] = b;
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    pub fn can_read(&self) -> bool {
        !self.from_slave.is_empty()
    }
}

// ── Global TTY / PTY tables ───────────────────────────────────────────────────

static TTYS: Mutex<Vec<Tty>> = Mutex::new(Vec::new());
static ACTIVE_TTY: Mutex<u32> = Mutex::new(0);
static PTY_MASTERS: Mutex<Vec<PtyMaster>> = Mutex::new(Vec::new());
static NEXT_PTY_ID: Mutex<u32> = Mutex::new(0);

// ── Initialization ────────────────────────────────────────────────────────────

pub fn init() {
    let mut ttys = TTYS.lock();
    let mut tty0 = Tty::new(0, "tty0", 25, 80);
    tty0.controlling = true;
    tty0.fg_pgid = 1;
    tty0.tty_type = TtyType::Console;
    ttys.push(tty0);

    for i in 1..8u32 {
        let name = alloc::format!("tty{}", i);
        let mut tty = Tty::new(i, &name, 25, 80);
        tty.tty_type = TtyType::Console;
        ttys.push(tty);
    }

    drop(ttys);
    crate::drivers::register("tty", crate::drivers::DeviceType::Other);
    serial_println!("  TTY: 8 virtual consoles (tty0-tty7), PTY support enabled");
}

// ── Virtual console management ────────────────────────────────────────────────

pub fn active() -> u32 {
    *ACTIVE_TTY.lock()
}

pub fn switch_to(id: u32) {
    let ttys = TTYS.lock();
    if (id as usize) < ttys.len() {
        drop(ttys);
        *ACTIVE_TTY.lock() = id;
        serial_println!("  TTY: switched to tty{}", id);
    }
}

pub fn write(data: &[u8]) {
    let active = *ACTIVE_TTY.lock();
    let mut ttys = TTYS.lock();
    if let Some(tty) = ttys.get_mut(active as usize) {
        tty.write(data);
    }
}

pub fn input(ch: u8) {
    let active = *ACTIVE_TTY.lock();
    let mut ttys = TTYS.lock();
    if let Some(tty) = ttys.get_mut(active as usize) {
        tty.input_char(ch);
    }
}

pub fn input_to(tty_id: u32, ch: u8) {
    let mut ttys = TTYS.lock();
    if let Some(tty) = ttys.get_mut(tty_id as usize) {
        tty.input_char(ch);
    }
}

pub fn read_from(tty_id: u32, buf: &mut [u8]) -> usize {
    let mut ttys = TTYS.lock();
    if let Some(tty) = ttys.get_mut(tty_id as usize) {
        tty.read(buf)
    } else {
        0
    }
}

pub fn can_read(tty_id: u32) -> bool {
    let ttys = TTYS.lock();
    if let Some(tty) = ttys.get(tty_id as usize) {
        tty.can_read()
    } else {
        false
    }
}

pub fn drain_active_output() -> Vec<u8> {
    let active = *ACTIVE_TTY.lock();
    let mut ttys = TTYS.lock();
    if let Some(tty) = ttys.get_mut(active as usize) {
        tty.drain_output()
    } else {
        Vec::new()
    }
}

pub fn winsize(tty_id: u32) -> (u16, u16) {
    let ttys = TTYS.lock();
    if let Some(tty) = ttys.get(tty_id as usize) {
        (tty.rows, tty.cols)
    } else {
        (25, 80)
    }
}

pub fn set_fg_pgid(tty_id: u32, pgid: u32) {
    let mut ttys = TTYS.lock();
    if let Some(tty) = ttys.get_mut(tty_id as usize) {
        tty.set_fg_pgid(pgid);
    }
}

pub fn tty_count() -> usize {
    TTYS.lock().len()
}

pub fn write_ansi(data: &[u8]) {
    let active = *ACTIVE_TTY.lock();
    let mut ttys = TTYS.lock();
    if let Some(tty) = ttys.get_mut(active as usize) {
        tty.write_ansi(data);
    }
}

pub fn write_ansi_to(tty_id: u32, data: &[u8]) {
    let mut ttys = TTYS.lock();
    if let Some(tty) = ttys.get_mut(tty_id as usize) {
        tty.write_ansi(data);
    }
}

pub fn scroll_up(lines: usize) {
    let active = *ACTIVE_TTY.lock();
    let mut ttys = TTYS.lock();
    if let Some(tty) = ttys.get_mut(active as usize) {
        tty.scroll_view_up(lines);
    }
}

pub fn scroll_down(lines: usize) {
    let active = *ACTIVE_TTY.lock();
    let mut ttys = TTYS.lock();
    if let Some(tty) = ttys.get_mut(active as usize) {
        tty.scroll_view_down(lines);
    }
}

pub fn scroll_reset() {
    let active = *ACTIVE_TTY.lock();
    let mut ttys = TTYS.lock();
    if let Some(tty) = ttys.get_mut(active as usize) {
        tty.scroll_view_reset();
    }
}

pub fn scrollback_len() -> usize {
    let active = *ACTIVE_TTY.lock();
    let ttys = TTYS.lock();
    ttys.get(active as usize)
        .map(|t| t.scrollback_len())
        .unwrap_or(0)
}

pub fn cursor_pos() -> (u32, u32) {
    let active = *ACTIVE_TTY.lock();
    let ttys = TTYS.lock();
    ttys.get(active as usize)
        .map(|t| t.cursor_pos())
        .unwrap_or((0, 0))
}

// ── PTY management ────────────────────────────────────────────────────────────

/// Allocate a new PTY master/slave pair.
/// Returns `(master_id, slave_tty_id)`, or `None` if the limit is reached.
pub fn alloc_pty() -> Option<(u32, u32)> {
    let mut pty_id = NEXT_PTY_ID.lock();
    let id = *pty_id;
    if id as usize >= MAX_PTYS {
        return None;
    }
    *pty_id = id.saturating_add(1);
    drop(pty_id);

    let slave_name = alloc::format!("pts/{}", id);
    let mut slave_tty = Tty::new(1000u32.saturating_add(id), &slave_name, 24, 80);
    slave_tty.tty_type = TtyType::PtySlave;

    let mut ttys = TTYS.lock();
    let slave_idx = ttys.len();
    let slave_tty_id = slave_tty.id;
    slave_tty.pty_master_idx = Some(id as usize);
    ttys.push(slave_tty);
    drop(ttys);

    PTY_MASTERS.lock().push(PtyMaster {
        id,
        to_slave: VecDeque::new(),
        from_slave: VecDeque::new(),
        open: true,
        slave_tty_idx: slave_idx,
    });

    serial_println!(
        "  TTY: allocated PTY pair (master={}, slave=pts/{})",
        id,
        id
    );
    Some((id, slave_tty_id))
}

pub fn pty_master_write(master_id: u32, data: &[u8]) {
    let masters = PTY_MASTERS.lock();
    if let Some(master) = masters.iter().find(|m| m.id == master_id) {
        let slave_idx = master.slave_tty_idx;
        drop(masters);
        let mut ttys = TTYS.lock();
        if let Some(tty) = ttys.get_mut(slave_idx) {
            for &b in data {
                tty.input_char(b);
            }
        }
    }
}

pub fn pty_master_read(master_id: u32, buf: &mut [u8]) -> usize {
    let masters = PTY_MASTERS.lock();
    if let Some(master) = masters.iter().find(|m| m.id == master_id) {
        let slave_idx = master.slave_tty_idx;
        drop(masters);
        let mut ttys = TTYS.lock();
        if let Some(tty) = ttys.get_mut(slave_idx) {
            let out = tty.drain_output();
            let len = out.len().min(buf.len());
            buf[..len].copy_from_slice(&out[..len]);
            return len;
        }
    }
    0
}

pub fn pty_close(master_id: u32) {
    let mut masters = PTY_MASTERS.lock();
    if let Some(master) = masters.iter_mut().find(|m| m.id == master_id) {
        master.open = false;
        let slave_idx = master.slave_tty_idx;
        drop(masters);
        let ttys = TTYS.lock();
        if let Some(tty) = ttys.get(slave_idx) {
            if tty.fg_pgid > 0 {
                let _ =
                    crate::process::send_signal(tty.fg_pgid, crate::process::pcb::signal::SIGHUP);
            }
        }
    }
}

pub fn pty_count() -> usize {
    PTY_MASTERS.lock().iter().filter(|m| m.open).count()
}
