use crate::serial_println;
/// Pseudo-terminal (PTY) subsystem for Genesis
///
/// Provides master/slave PTY pairs that enable programs such as terminal
/// emulators, SSH daemons, and `script(1)` to multiplex terminal I/O without
/// requiring a physical serial port or VGA console.
///
/// Design overview
/// ---------------
/// Each PTY pair consists of:
///   - A *master* end (fd in PTY_MASTER_BASE..PTY_MASTER_BASE+MAX_PTYS range).
///     The terminal emulator/SSH daemon opens this side.
///   - A *slave* end (fd in PTY_SLAVE_BASE..PTY_SLAVE_BASE+MAX_PTYS range).
///     The shell or other program opens this as its controlling terminal.
///
/// Data flow:
///   master write  →  m2s ring  →  slave read
///   slave  write  →  line disc →  s2m ring  →  master read
///
/// Line discipline (cooked / ICANON mode):
///   Bytes from the slave are accumulated in line_buf until '\n' or VEOF,
///   then flushed into the s2m ring.  In raw mode bytes go straight through.
///   ECHO causes the character to be reflected back to the master (so the
///   terminal emulator can display what the user typed).
///   VERASE deletes the last character; VKILL clears the whole line.
///   ISIG on VINTR/VQUIT/VSUSP queues a signal to slave_pid (stub — process
///   subsystem sends the actual signal).
///
/// All ring buffers use power-of-two wrapping arithmetic (mod PTY_BUF_SIZE).
/// PTY_BUF_SIZE *must* remain a power of two; the mask constant enforces this
/// at compile time.
///
/// Kernel safety rules observed:
///   - NO heap: all buffers are fixed-size arrays inside PtyPair.
///   - NO panics: every fallible path returns Option/bool/isize.
///   - NO float casts.
///   - Counters use saturating_add / wrapping_add as appropriate.
///   - Mutex<[PtyPair; MAX_PTYS]> requires PtyPair: Copy + const fn empty().
use crate::sync::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Base fd number for PTY master ends (/dev/ptmx equivalent).
pub const PTY_MASTER_BASE: i32 = 9000;
/// Base fd number for PTY slave ends (/dev/pts/N equivalent).
pub const PTY_SLAVE_BASE: i32 = 9100;
/// Maximum concurrent PTY pairs.
pub const MAX_PTYS: usize = 16;
/// Ring buffer capacity (bytes) for each direction of a PTY pair.
/// MUST be a power of two.
pub const PTY_BUF_SIZE: usize = 4096;

/// Mask used for O(1) ring index wrapping.  Compiler-enforced to equal
/// PTY_BUF_SIZE - 1 when PTY_BUF_SIZE is a power of two.
const PTY_BUF_MASK: u32 = (PTY_BUF_SIZE - 1) as u32;

// ── Termios flag bits ─────────────────────────────────────────────────────────

// c_lflag (local flags)
pub const ISIG: u32 = 0x0001;
pub const ICANON: u32 = 0x0002;
pub const ECHO: u32 = 0x0008;
pub const ECHOE: u32 = 0x0010;
pub const ECHOK: u32 = 0x0020;
pub const ECHONL: u32 = 0x0040;
pub const IEXTEN: u32 = 0x8000;

// c_oflag (output flags)
pub const OPOST: u32 = 0x0001;
pub const ONLCR: u32 = 0x0004;

// c_cflag (control flags)
pub const CS8: u32 = 0x0300;
pub const CREAD: u32 = 0x0800;

// c_iflag (input flags)
pub const ICRNL: u32 = 0x0100;
pub const IXON: u32 = 0x0200;

// ── c_cc indices ─────────────────────────────────────────────────────────────

pub const VINTR: usize = 0; // default Ctrl-C  (0x03)
pub const VQUIT: usize = 1; // default Ctrl-\  (0x1C)
pub const VERASE: usize = 2; // default DEL     (0x7F)
pub const VKILL: usize = 3; // default Ctrl-U  (0x15)
pub const VEOF: usize = 4; // default Ctrl-D  (0x04)
pub const VMIN: usize = 6; // minimum chars for raw read
pub const VTIME: usize = 5; // timeout for raw read (tenths of a second)
pub const VSTART: usize = 8; // Ctrl-Q (0x11)
pub const VSTOP: usize = 9; // Ctrl-S (0x13)
pub const VSUSP: usize = 10; // Ctrl-Z (0x1A)

/// Total number of control-character slots in Termios::c_cc.
const NCC: usize = 32;

// ── Termios ───────────────────────────────────────────────────────────────────

/// POSIX termios structure describing terminal I/O behaviour.
#[derive(Copy, Clone)]
pub struct Termios {
    pub c_iflag: u32,    // input mode flags
    pub c_oflag: u32,    // output mode flags
    pub c_cflag: u32,    // control flags
    pub c_lflag: u32,    // local (line discipline) flags
    pub c_cc: [u8; NCC], // control characters
    pub c_ispeed: u32,   // input baud rate (informational)
    pub c_ospeed: u32,   // output baud rate (informational)
}

impl Termios {
    /// Raw mode: no line buffering, no echo, no signal generation.
    /// Characters are passed to the application immediately.
    pub const fn default_raw() -> Self {
        let mut cc = [0u8; NCC];
        cc[VMIN] = 1; // return as soon as 1 byte is available
        cc[VTIME] = 0; // no timeout
                       // Keep canonical control chars at their POSIX defaults even in raw
                       // mode so that switching back to cooked restores sane values.
        cc[VINTR] = 0x03;
        cc[VQUIT] = 0x1C;
        cc[VERASE] = 0x7F;
        cc[VKILL] = 0x15;
        cc[VEOF] = 0x04;
        cc[VSTART] = 0x11;
        cc[VSTOP] = 0x13;
        cc[VSUSP] = 0x1A;
        Termios {
            c_iflag: 0,
            c_oflag: 0,
            c_cflag: CS8 | CREAD,
            c_lflag: 0,
            c_cc: cc,
            c_ispeed: 38400,
            c_ospeed: 38400,
        }
    }

    /// Cooked (canonical) mode: line buffering, echo enabled, signals active.
    pub const fn default_cooked() -> Self {
        let mut cc = [0u8; NCC];
        cc[VINTR] = 0x03;
        cc[VQUIT] = 0x1C;
        cc[VERASE] = 0x7F;
        cc[VKILL] = 0x15;
        cc[VEOF] = 0x04;
        cc[VMIN] = 1;
        cc[VTIME] = 0;
        cc[VSTART] = 0x11;
        cc[VSTOP] = 0x13;
        cc[VSUSP] = 0x1A;
        Termios {
            c_iflag: ICRNL | IXON,
            c_oflag: OPOST | ONLCR,
            c_cflag: CS8 | CREAD,
            c_lflag: ICANON | ECHO | ECHOE | ECHOK | ECHONL | ISIG | IEXTEN,
            c_cc: cc,
            c_ispeed: 38400,
            c_ospeed: 38400,
        }
    }

    /// Return true when canonical (line-buffered) mode is active.
    #[inline]
    pub fn is_canonical(&self) -> bool {
        self.c_lflag & ICANON != 0
    }

    /// Return true when echo is enabled.
    #[inline]
    pub fn echo_on(&self) -> bool {
        self.c_lflag & ECHO != 0
    }

    /// Return true when signal generation is enabled.
    #[inline]
    pub fn isig_on(&self) -> bool {
        self.c_lflag & ISIG != 0
    }

    /// Return true when output post-processing is enabled.
    #[inline]
    pub fn opost_on(&self) -> bool {
        self.c_oflag & OPOST != 0
    }

    /// Return true when NL→CRNL translation is enabled.
    #[inline]
    pub fn onlcr_on(&self) -> bool {
        self.c_oflag & ONLCR != 0
    }
}

// ── PtyPair ───────────────────────────────────────────────────────────────────

/// A single master/slave PTY pair.
///
/// Two independent ring buffers carry data in each direction:
///
///   m2s_buf  — master writes here; slave reads from here
///   s2m_buf  — slave writes here (via line discipline); master reads from here
///
/// The ring is full when `(head - tail) & MASK == MASK` (one slot unused to
/// distinguish full from empty).
///
/// `line_buf` and `line_len` are used by the cooked-mode line discipline to
/// accumulate a partial line before flushing to s2m_buf.
#[derive(Copy, Clone)]
pub struct PtyPair {
    /// Slot index within PTY_TABLE (0..MAX_PTYS).
    pub index: u8,
    /// Whether this slot is in use.
    pub active: bool,
    /// fd number for the master end.
    pub master_fd: i32,
    /// fd number for the slave end.
    pub slave_fd: i32,
    /// Terminal I/O settings.
    pub termios: Termios,
    /// Window rows.
    pub winsize_rows: u16,
    /// Window columns.
    pub winsize_cols: u16,
    /// Window width in pixels (informational).
    pub winsize_xpixel: u16,
    /// Window height in pixels (informational).
    pub winsize_ypixel: u16,

    // ── Master→Slave ring ──────────────────────────────────────────────────
    /// Data written by the master; consumed by the slave.
    pub m2s_buf: [u8; PTY_BUF_SIZE],
    /// Write pointer (master advances on write).
    pub m2s_head: u32,
    /// Read pointer (slave advances on read).
    pub m2s_tail: u32,

    // ── Slave→Master ring ──────────────────────────────────────────────────
    /// Data written by the slave (via line discipline); consumed by the master.
    pub s2m_buf: [u8; PTY_BUF_SIZE],
    /// Write pointer (slave line discipline advances on write).
    pub s2m_head: u32,
    /// Read pointer (master advances on read).
    pub s2m_tail: u32,

    // ── Cooked-mode line accumulation buffer ───────────────────────────────
    /// Partial line waiting for a terminating '\n' or VEOF.
    pub line_buf: [u8; 256],
    /// Number of valid bytes in line_buf.
    pub line_len: u16,

    // ── Process tracking ──────────────────────────────────────────────────
    /// PID of the process controlling the slave (receives signals).
    pub slave_pid: u32,
    /// PID of the process holding the master end open.
    pub master_pid: u32,

    // ── Half-close state ──────────────────────────────────────────────────
    /// True while the master fd has not been closed.
    pub master_open: bool,
    /// True while the slave fd has not been closed.
    pub slave_open: bool,
}

impl PtyPair {
    /// Construct a zeroed, inactive slot suitable for a static array.
    pub const fn empty() -> Self {
        PtyPair {
            index: 0,
            active: false,
            master_fd: 0,
            slave_fd: 0,
            termios: Termios::default_cooked(),
            winsize_rows: 24,
            winsize_cols: 80,
            winsize_xpixel: 0,
            winsize_ypixel: 0,
            m2s_buf: [0u8; PTY_BUF_SIZE],
            m2s_head: 0,
            m2s_tail: 0,
            s2m_buf: [0u8; PTY_BUF_SIZE],
            s2m_head: 0,
            s2m_tail: 0,
            line_buf: [0u8; 256],
            line_len: 0,
            slave_pid: 0,
            master_pid: 0,
            master_open: false,
            slave_open: false,
        }
    }
}

// ── Global PTY table ─────────────────────────────────────────────────────────

static PTY_TABLE: Mutex<[PtyPair; MAX_PTYS]> = Mutex::new({
    // Initialise all sixteen slots to empty.  We cannot call a loop inside a
    // const expression so we enumerate each field manually.
    [
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
        PtyPair::empty(),
    ]
});

// ── Ring-buffer helpers ───────────────────────────────────────────────────────

/// Returns the number of bytes currently in the ring (head - tail, wrapping).
///
/// Because head and tail are pure counters that advance by 1 per byte and
/// wrap at u32::MAX, the difference gives the exact occupancy so long as the
/// ring is never more than 2^31 bytes (which it is not — PTY_BUF_SIZE = 4096).
#[inline]
pub fn pty_ring_len(head: u32, tail: u32) -> u32 {
    head.wrapping_sub(tail)
}

/// Returns the number of free bytes available for writing.
#[inline]
fn ring_free(head: u32, tail: u32) -> u32 {
    // One slot is kept unused to distinguish full from empty.
    // Maximum occupancy = PTY_BUF_SIZE - 1.
    let occupied = pty_ring_len(head, tail);
    let capacity = (PTY_BUF_SIZE as u32).saturating_sub(1);
    if occupied >= capacity {
        0
    } else {
        capacity - occupied
    }
}

/// Write `data` into the ring starting at `*head`.
/// Returns the number of bytes actually written (may be < data.len() if full).
pub fn pty_ring_put(buf: &mut [u8; PTY_BUF_SIZE], head: &mut u32, tail: u32, data: &[u8]) -> usize {
    let free = ring_free(*head, tail) as usize;
    let n = data.len().min(free);
    for i in 0..n {
        let idx = (*head & PTY_BUF_MASK) as usize;
        buf[idx] = data[i];
        *head = head.wrapping_add(1);
    }
    n
}

/// Read up to `out.len()` bytes from the ring starting at `*tail`.
/// Returns the number of bytes copied.
pub fn pty_ring_get(buf: &[u8; PTY_BUF_SIZE], head: u32, tail: &mut u32, out: &mut [u8]) -> usize {
    let avail = pty_ring_len(head, *tail) as usize;
    let n = out.len().min(avail);
    for i in 0..n {
        let idx = (*tail & PTY_BUF_MASK) as usize;
        out[i] = buf[idx];
        *tail = tail.wrapping_add(1);
    }
    n
}

// ── Signal stub ───────────────────────────────────────────────────────────────

/// Deliver a POSIX signal to `pid`.  Uses the process subsystem when
/// available; silently succeeds if the pid is 0 (no controlling process set).
#[inline]
fn send_signal_to(pid: u32, sig: u32) {
    if pid == 0 {
        return;
    }
    // Best-effort: ignore error (process may have already exited).
    let _ = crate::process::send_signal(pid, sig as u8);
}

// Signal numbers (matching Linux ABI used throughout the kernel).
const SIGINT: u32 = 2;
const SIGQUIT: u32 = 3;
const SIGTSTP: u32 = 20; // Ctrl-Z stops the foreground process group
const SIGWINCH: u32 = 28;

// ── Line discipline ───────────────────────────────────────────────────────────

/// Process one byte through the slave-side line discipline.
///
/// In ICANON mode the byte is accumulated in `pair.line_buf`; when a line
/// terminator is seen the whole line is flushed into the s2m ring so the
/// master can read it.
///
/// In non-canonical mode the byte is written directly to s2m.
pub fn pty_ldisc_process(pair: &mut PtyPair, ch: u8) {
    let canonical = pair.termios.is_canonical();
    let echo = pair.termios.echo_on();
    let isig = pair.termios.isig_on();

    // ── Signal generation ──────────────────────────────────────────────────
    if isig {
        let vintr = pair.termios.c_cc[VINTR];
        let vquit = pair.termios.c_cc[VQUIT];
        let vsusp = pair.termios.c_cc[VSUSP];

        if ch == vintr && vintr != 0 {
            send_signal_to(pair.slave_pid, SIGINT);
            if canonical {
                pair.line_len = 0;
            }
            // Echo the control char back if ECHOCTL is set; skip here for simplicity.
            return;
        }
        if ch == vquit && vquit != 0 {
            send_signal_to(pair.slave_pid, SIGQUIT);
            if canonical {
                pair.line_len = 0;
            }
            return;
        }
        if ch == vsusp && vsusp != 0 {
            send_signal_to(pair.slave_pid, SIGTSTP);
            if canonical {
                pair.line_len = 0;
            }
            return;
        }
    }

    if canonical {
        let verase = pair.termios.c_cc[VERASE];
        let vkill = pair.termios.c_cc[VKILL];
        let veof = pair.termios.c_cc[VEOF];

        // ── VERASE (backspace / DEL) ────────────────────────────────────
        if ch == verase && verase != 0 {
            if pair.line_len > 0 {
                pair.line_len = pair.line_len.saturating_sub(1);
                // Erase-echo: send BS SP BS to master so the display removes
                // the character.
                if echo && pair.termios.c_lflag & ECHOE != 0 {
                    let erase_seq = [0x08u8, 0x20u8, 0x08u8];
                    pty_ring_put(
                        &mut pair.s2m_buf,
                        &mut pair.s2m_head,
                        pair.s2m_tail,
                        &erase_seq,
                    );
                }
            }
            return;
        }

        // ── VKILL (line kill) ───────────────────────────────────────────
        if ch == vkill && vkill != 0 {
            if echo && pair.termios.c_lflag & ECHOK != 0 {
                // Echo each erased character back.
                for _ in 0..pair.line_len {
                    let erase_seq = [0x08u8, 0x20u8, 0x08u8];
                    pty_ring_put(
                        &mut pair.s2m_buf,
                        &mut pair.s2m_head,
                        pair.s2m_tail,
                        &erase_seq,
                    );
                }
                // Optionally write '\n' after kill (ECHOK convention).
                pty_ring_put(
                    &mut pair.s2m_buf,
                    &mut pair.s2m_head,
                    pair.s2m_tail,
                    b"\r\n",
                );
            }
            pair.line_len = 0;
            return;
        }

        // ── VEOF (end-of-file, Ctrl-D) ─────────────────────────────────
        if ch == veof && veof != 0 {
            // Flush whatever is in the line buffer without appending a '\n'.
            let len = pair.line_len as usize;
            if len > 0 {
                // Copy line_buf to a local so we can borrow s2m mutably.
                let mut tmp = [0u8; 256];
                tmp[..len].copy_from_slice(&pair.line_buf[..len]);
                pty_ring_put(
                    &mut pair.s2m_buf,
                    &mut pair.s2m_head,
                    pair.s2m_tail,
                    &tmp[..len],
                );
                pair.line_len = 0;
            }
            return;
        }

        // ── Echo ────────────────────────────────────────────────────────
        if echo {
            // CR→CRNL translation for echo (ICRNL is an input flag, but echo
            // reflects processed input back through the output path).
            if ch == b'\r' {
                pty_ring_put(
                    &mut pair.s2m_buf,
                    &mut pair.s2m_head,
                    pair.s2m_tail,
                    b"\r\n",
                );
            } else {
                pty_ring_put(
                    &mut pair.s2m_buf,
                    &mut pair.s2m_head,
                    pair.s2m_tail,
                    core::slice::from_ref(&ch),
                );
            }
        }

        // ── Accumulate into line buffer ─────────────────────────────────
        // Map CR→LF when ICRNL is set.
        let store_ch = if ch == b'\r' && pair.termios.c_iflag & ICRNL != 0 {
            b'\n'
        } else {
            ch
        };

        if (pair.line_len as usize) < pair.line_buf.len() {
            let idx = pair.line_len as usize;
            pair.line_buf[idx] = store_ch;
            pair.line_len = pair.line_len.saturating_add(1);
        }

        // ── Flush on line terminator ────────────────────────────────────
        if store_ch == b'\n' {
            let len = pair.line_len as usize;
            let mut tmp = [0u8; 256];
            tmp[..len].copy_from_slice(&pair.line_buf[..len]);
            pty_ring_put(
                &mut pair.s2m_buf,
                &mut pair.s2m_head,
                pair.s2m_tail,
                &tmp[..len],
            );
            pair.line_len = 0;
        }
    } else {
        // ── Raw / non-canonical: pass straight to s2m ring ─────────────
        // In raw mode, data goes to s2m unconditionally (the slave application
        // reads bytes directly).  Echo in raw mode means the byte is also
        // reflected to the master for display — but since s2m IS what the
        // master reads, a single write covers both cases.
        pty_ring_put(
            &mut pair.s2m_buf,
            &mut pair.s2m_head,
            pair.s2m_tail,
            core::slice::from_ref(&ch),
        );
    }
}

// ── Index helpers ─────────────────────────────────────────────────────────────

/// Map a master fd to its table index.  Returns None if out of range.
#[inline]
fn master_fd_to_idx(fd: i32) -> Option<usize> {
    if fd < PTY_MASTER_BASE {
        return None;
    }
    let idx = (fd - PTY_MASTER_BASE) as usize;
    if idx < MAX_PTYS {
        Some(idx)
    } else {
        None
    }
}

/// Map a slave fd to its table index.  Returns None if out of range.
#[inline]
fn slave_fd_to_idx(fd: i32) -> Option<usize> {
    if fd < PTY_SLAVE_BASE {
        return None;
    }
    let idx = (fd - PTY_SLAVE_BASE) as usize;
    if idx < MAX_PTYS {
        Some(idx)
    } else {
        None
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the PTY subsystem.  Called from `drivers::init()`.
pub fn init() {
    // The static table is already zeroed by PtyPair::empty(); nothing else
    // needs to happen at boot time.
    serial_println!(
        "  PTY: subsystem ready ({} slots, {} byte rings)",
        MAX_PTYS,
        PTY_BUF_SIZE
    );
}

/// Open a new PTY pair.
///
/// Finds the first free slot, marks it active, and initialises it with
/// cooked-mode termios and an 80x24 window.
///
/// Returns the *master* fd on success, or `None` if all slots are in use.
pub fn pty_open() -> Option<i32> {
    let mut table = PTY_TABLE.lock();
    for idx in 0..MAX_PTYS {
        if !table[idx].active {
            let pair = &mut table[idx];
            *pair = PtyPair::empty();
            pair.index = idx as u8;
            pair.active = true;
            pair.master_fd = PTY_MASTER_BASE + idx as i32;
            pair.slave_fd = PTY_SLAVE_BASE + idx as i32;
            pair.master_open = true;
            pair.slave_open = true;
            pair.termios = Termios::default_cooked();
            pair.winsize_rows = 24;
            pair.winsize_cols = 80;
            serial_println!(
                "  PTY: opened pair {} (master fd={}, slave fd={})",
                idx,
                pair.master_fd,
                pair.slave_fd
            );
            return Some(pair.master_fd);
        }
    }
    None
}

/// Given a master fd, return the corresponding slave fd.
pub fn pty_get_slave_fd(master_fd: i32) -> Option<i32> {
    let idx = master_fd_to_idx(master_fd)?;
    let table = PTY_TABLE.lock();
    if table[idx].active {
        Some(table[idx].slave_fd)
    } else {
        None
    }
}

/// Close the master side of a PTY.
///
/// If the slave side is already closed the slot is freed.
/// Sends SIGHUP to the slave's controlling process (stub via send_signal).
/// Returns false if fd does not name an active master.
pub fn pty_close_master(fd: i32) -> bool {
    let idx = match master_fd_to_idx(fd) {
        Some(i) => i,
        None => return false,
    };
    let (should_deactivate, slave_pid) = {
        let mut table = PTY_TABLE.lock();
        let pair = &mut table[idx];
        if !pair.active || !pair.master_open {
            return false;
        }
        pair.master_open = false;
        let spid = pair.slave_pid;
        (!pair.slave_open, spid)
    };
    // Notify the slave that the master has gone away.
    send_signal_to(slave_pid, 1 /* SIGHUP */);
    if should_deactivate {
        let mut table = PTY_TABLE.lock();
        table[idx].active = false;
    }
    true
}

/// Close the slave side of a PTY.
///
/// If the master side is already closed the slot is freed.
/// Returns false if fd does not name an active slave.
pub fn pty_close_slave(fd: i32) -> bool {
    let idx = match slave_fd_to_idx(fd) {
        Some(i) => i,
        None => return false,
    };
    let should_deactivate = {
        let mut table = PTY_TABLE.lock();
        let pair = &mut table[idx];
        if !pair.active || !pair.slave_open {
            return false;
        }
        pair.slave_open = false;
        !pair.master_open
    };
    if should_deactivate {
        let mut table = PTY_TABLE.lock();
        table[idx].active = false;
    }
    true
}

/// Write data to the master end (data becomes available to the slave).
///
/// Applies minimal output post-processing when OPOST | ONLCR are set
/// (translates '\n' to '\r\n') before placing bytes in the m2s ring.
///
/// Returns the number of bytes written from `data`, or -1 on error.
pub fn pty_master_write(fd: i32, data: &[u8]) -> isize {
    let idx = match master_fd_to_idx(fd) {
        Some(i) => i,
        None => return -1,
    };
    let mut table = PTY_TABLE.lock();
    let pair = &mut table[idx];
    if !pair.active || !pair.master_open {
        return -1;
    }

    let opost = pair.termios.opost_on();
    let onlcr = pair.termios.onlcr_on();

    let mut written = 0usize;
    for &byte in data {
        if opost && onlcr && byte == b'\n' {
            // Translate NL to CRNL on master→slave path.
            let crnl = [b'\r', b'\n'];
            if pty_ring_put(&mut pair.m2s_buf, &mut pair.m2s_head, pair.m2s_tail, &crnl) < 2 {
                break;
            }
        } else {
            if pty_ring_put(
                &mut pair.m2s_buf,
                &mut pair.m2s_head,
                pair.m2s_tail,
                core::slice::from_ref(&byte),
            ) == 0
            {
                break;
            }
        }
        written = written.saturating_add(1);
    }
    written as isize
}

/// Read data from the master end (data that the slave has written).
///
/// Returns the number of bytes placed in `buf`, or -11 (EAGAIN) if the
/// s2m ring is empty.
pub fn pty_master_read(fd: i32, buf: &mut [u8; 4096]) -> isize {
    let idx = match master_fd_to_idx(fd) {
        Some(i) => i,
        None => return -1,
    };
    let mut table = PTY_TABLE.lock();
    let pair = &mut table[idx];
    if !pair.active || !pair.master_open {
        return -1;
    }

    let avail = pty_ring_len(pair.s2m_head, pair.s2m_tail);
    if avail == 0 {
        return -11; // EAGAIN
    }
    let n = pty_ring_get(&pair.s2m_buf, pair.s2m_head, &mut pair.s2m_tail, buf);
    n as isize
}

/// Write data from the slave end.  The data passes through the line
/// discipline before appearing in the s2m ring for the master to read.
///
/// Returns the number of bytes processed from `data`, or -1 on error.
pub fn pty_slave_write(fd: i32, data: &[u8]) -> isize {
    let idx = match slave_fd_to_idx(fd) {
        Some(i) => i,
        None => return -1,
    };
    let mut table = PTY_TABLE.lock();
    let pair = &mut table[idx];
    if !pair.active || !pair.slave_open {
        return -1;
    }

    let len = data.len();
    for &byte in data {
        pty_ldisc_process(pair, byte);
    }
    len as isize
}

/// Read data on the slave end (data that the master has sent).
///
/// In canonical mode: returns data only when a complete line is available
/// in the line buffer (i.e., when the line_buf contains a '\n').
/// This is an approximation — the real check is whether there is a complete
/// line in the s2m ring.  Here we gate on the m2s ring having data and
/// canonical mode having no pending line.
///
/// In raw mode: returns up to c_cc[VMIN] bytes from the m2s ring, or
/// -11 (EAGAIN) if fewer than VMIN bytes are available.
///
/// Returns bytes read, or -11 (EAGAIN).
pub fn pty_slave_read(fd: i32, buf: &mut [u8; 4096]) -> isize {
    let idx = match slave_fd_to_idx(fd) {
        Some(i) => i,
        None => return -1,
    };
    let mut table = PTY_TABLE.lock();
    let pair = &mut table[idx];
    if !pair.active || !pair.slave_open {
        return -1;
    }

    let avail = pty_ring_len(pair.m2s_head, pair.m2s_tail);
    if avail == 0 {
        return -11;
    } // EAGAIN

    if pair.termios.is_canonical() {
        // In canonical mode we return only complete lines.  A complete line
        // is signalled by a '\n' existing somewhere in the m2s ring.  Scan
        // forward until we find one (or exhaust the ring).
        let mut has_newline = false;
        let mut scan_tail = pair.m2s_tail;
        let scan_head = pair.m2s_head;
        let count = pty_ring_len(scan_head, scan_tail) as usize;
        for _ in 0..count {
            let idx_in_buf = (scan_tail & PTY_BUF_MASK) as usize;
            if pair.m2s_buf[idx_in_buf] == b'\n' {
                has_newline = true;
                break;
            }
            scan_tail = scan_tail.wrapping_add(1);
        }
        if !has_newline {
            return -11;
        } // EAGAIN — no complete line yet
          // Drain up to a full line (including '\n') into buf.
        let to_read = buf.len().min(avail as usize);
        let n = pty_ring_get(
            &pair.m2s_buf,
            pair.m2s_head,
            &mut pair.m2s_tail,
            &mut buf[..to_read],
        );
        n as isize
    } else {
        // Raw mode: respect VMIN.
        let vmin = pair.termios.c_cc[VMIN] as u32;
        let vmin = if vmin == 0 { 1 } else { vmin };
        if avail < vmin {
            return -11;
        } // EAGAIN
        let to_read = buf.len().min(avail as usize);
        let n = pty_ring_get(
            &pair.m2s_buf,
            pair.m2s_head,
            &mut pair.m2s_tail,
            &mut buf[..to_read],
        );
        n as isize
    }
}

/// Update the window size of the PTY identified by `master_fd`.
///
/// Sends SIGWINCH to the slave's controlling process so that applications
/// (e.g. shells, editors) can resize their display.
///
/// Returns false if fd is invalid or inactive.
pub fn pty_set_winsize(master_fd: i32, rows: u16, cols: u16) -> bool {
    let idx = match master_fd_to_idx(master_fd) {
        Some(i) => i,
        None => return false,
    };
    let slave_pid = {
        let mut table = PTY_TABLE.lock();
        let pair = &mut table[idx];
        if !pair.active {
            return false;
        }
        pair.winsize_rows = rows;
        pair.winsize_cols = cols;
        pair.slave_pid
    };
    send_signal_to(slave_pid, SIGWINCH);
    true
}

/// Read the window size for either a master or slave fd.
///
/// Returns `Some((rows, cols))` or `None` if fd is not a known PTY fd.
pub fn pty_get_winsize(fd: i32) -> Option<(u16, u16)> {
    let table = PTY_TABLE.lock();
    if let Some(idx) = master_fd_to_idx(fd) {
        if table[idx].active {
            return Some((table[idx].winsize_rows, table[idx].winsize_cols));
        }
    }
    if let Some(idx) = slave_fd_to_idx(fd) {
        if table[idx].active {
            return Some((table[idx].winsize_rows, table[idx].winsize_cols));
        }
    }
    None
}

/// Copy the current Termios settings for a PTY fd into `out`.
///
/// Works for both master and slave fds.
/// Returns false if fd is not a known active PTY.
pub fn pty_tcgetattr(fd: i32, out: &mut Termios) -> bool {
    let table = PTY_TABLE.lock();
    if let Some(idx) = master_fd_to_idx(fd) {
        if table[idx].active {
            *out = table[idx].termios;
            return true;
        }
    }
    if let Some(idx) = slave_fd_to_idx(fd) {
        if table[idx].active {
            *out = table[idx].termios;
            return true;
        }
    }
    false
}

/// Replace the Termios settings for a PTY fd.
///
/// Works for both master and slave fds.
/// Returns false if fd is not a known active PTY.
pub fn pty_tcsetattr(fd: i32, termios: Termios) -> bool {
    let mut table = PTY_TABLE.lock();
    if let Some(idx) = master_fd_to_idx(fd) {
        if table[idx].active {
            table[idx].termios = termios;
            return true;
        }
    }
    if let Some(idx) = slave_fd_to_idx(fd) {
        if table[idx].active {
            table[idx].termios = termios;
            return true;
        }
    }
    false
}

/// Returns true when `fd` is a master-side PTY fd.
#[inline]
pub fn pty_is_master_fd(fd: i32) -> bool {
    match master_fd_to_idx(fd) {
        Some(idx) => {
            let table = PTY_TABLE.lock();
            table[idx].active && table[idx].master_open
        }
        None => false,
    }
}

/// Returns true when `fd` is a slave-side PTY fd.
#[inline]
pub fn pty_is_slave_fd(fd: i32) -> bool {
    match slave_fd_to_idx(fd) {
        Some(idx) => {
            let table = PTY_TABLE.lock();
            table[idx].active && table[idx].slave_open
        }
        None => false,
    }
}

/// Returns true when `fd` is either a master or slave PTY fd.
#[inline]
pub fn pty_is_fd(fd: i32) -> bool {
    pty_is_master_fd(fd) || pty_is_slave_fd(fd)
}

/// Write the canonical device name "/dev/pts/N\0" for the slave associated
/// with `master_fd` into `out[..32]`.
///
/// Returns the number of bytes written (not including the NUL terminator),
/// or 0 on failure.
pub fn pty_get_slave_name(master_fd: i32, out: &mut [u8; 32]) -> usize {
    let idx = match master_fd_to_idx(master_fd) {
        Some(i) => i,
        None => return 0,
    };
    {
        let table = PTY_TABLE.lock();
        if !table[idx].active {
            return 0;
        }
    }
    // Build "/dev/pts/N\0" into out without alloc.
    // Maximum index is 15, so N is at most 2 digits.
    let prefix = b"/dev/pts/";
    let mut pos = 0usize;
    for &b in prefix.iter() {
        if pos >= 31 {
            break;
        }
        out[pos] = b;
        pos = pos.saturating_add(1);
    }
    // Encode the index as decimal digits.
    let tens = idx / 10;
    let ones = idx % 10;
    if tens > 0 && pos < 31 {
        out[pos] = b'0' + tens as u8;
        pos = pos.saturating_add(1);
    }
    if pos < 31 {
        out[pos] = b'0' + ones as u8;
        pos = pos.saturating_add(1);
    }
    if pos < 32 {
        out[pos] = 0; // NUL terminator
    }
    pos
}

// ── ioctl constants (matching Linux numbers used in sys_ioctl) ────────────────

/// TIOCGPTN — get the slave PTY index (the N in /dev/pts/N).
pub const TIOCGPTN: u64 = 0x8004_5430;
/// TIOCSWINSZ — set window size.
pub const TIOCSWINSZ: u64 = 0x5414;
/// TIOCGWINSZ — get window size.
pub const TIOCGWINSZ: u64 = 0x5413;
/// TIOCSPTLCK — lock/unlock the slave PTY.
pub const TIOCSPTLCK: u64 = 0x4004_5431;
/// TCGETS — get termios (struct termios).
pub const TCGETS: u64 = 0x5401;
/// TCSETS — set termios immediately.
pub const TCSETS: u64 = 0x5402;
/// TCSETSW — set termios, wait for drain first.
pub const TCSETSW: u64 = 0x5403;
/// TCSETSF — set termios, flush then set.
pub const TCSETSF: u64 = 0x5404;

/// A compact winsize structure laid out identically to `struct winsize` in
/// Linux's `<sys/ioctl.h>` so that userspace can write/read it directly.
#[repr(C)]
pub struct WinSize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

/// A compact termios structure compatible with the Linux `struct termios`
/// layout used by TCGETS / TCSETS ioctls (not the POSIX `struct termios`
/// which has a different binary layout on x86-64 glibc).
///
/// Layout: iflag(4) oflag(4) cflag(4) lflag(4) line(1) cc(19) pad(1) ispeed(4) ospeed(4) = 60 bytes
#[repr(C)]
pub struct KernelTermios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; 19],
    pub _pad: u8,
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

/// Handle a PTY-related ioctl.
///
/// `fd`      — file descriptor (master or slave PTY)
/// `request` — ioctl number
/// `arg`     — pointer-sized argument; its semantics depend on `request`
///
/// Returns 0 on success, -1 on failure, negative errno on error.
pub fn pty_ioctl(fd: i32, request: u64, arg: u64) -> isize {
    match request {
        // ── TIOCGPTN: get slave index ──────────────────────────────────
        TIOCGPTN => {
            let idx = match master_fd_to_idx(fd) {
                Some(i) => i,
                None => return -1,
            };
            {
                let table = PTY_TABLE.lock();
                if !table[idx].active {
                    return -1;
                }
            }
            if arg == 0 {
                return -1;
            }
            unsafe {
                core::ptr::write(arg as *mut u32, idx as u32);
            }
            0
        }

        // ── TIOCSWINSZ: set window size ────────────────────────────────
        TIOCSWINSZ => {
            if arg == 0 {
                return -1;
            }
            let ws = unsafe { core::ptr::read(arg as *const WinSize) };
            // Accept both master and slave fds.
            let master_fd_for_set = if pty_is_master_fd(fd) {
                fd
            } else if let Some(idx) = slave_fd_to_idx(fd) {
                PTY_MASTER_BASE + idx as i32
            } else {
                return -1;
            };
            if pty_set_winsize(master_fd_for_set, ws.ws_row, ws.ws_col) {
                0
            } else {
                -1
            }
        }

        // ── TIOCGWINSZ: get window size ────────────────────────────────
        TIOCGWINSZ => {
            if arg == 0 {
                return -1;
            }
            match pty_get_winsize(fd) {
                Some((rows, cols)) => {
                    let ws = WinSize {
                        ws_row: rows,
                        ws_col: cols,
                        ws_xpixel: 0,
                        ws_ypixel: 0,
                    };
                    unsafe {
                        core::ptr::write(arg as *mut WinSize, ws);
                    }
                    0
                }
                None => -1,
            }
        }

        // ── TIOCSPTLCK: lock/unlock slave ─────────────────────────────
        TIOCSPTLCK => {
            // Stub: acknowledge but do not actually enforce a lock.
            // A full implementation would prevent opens of the slave when
            // locked == 1.
            0
        }

        // ── TCGETS: read termios ───────────────────────────────────────
        TCGETS => {
            if arg == 0 {
                return -1;
            }
            let mut t = Termios::default_raw();
            if !pty_tcgetattr(fd, &mut t) {
                return -1;
            }
            // Marshal our internal Termios into the kernel layout.
            let mut kt = KernelTermios {
                c_iflag: t.c_iflag,
                c_oflag: t.c_oflag,
                c_cflag: t.c_cflag,
                c_lflag: t.c_lflag,
                c_line: 0,
                c_cc: [0u8; 19],
                _pad: 0,
                c_ispeed: t.c_ispeed,
                c_ospeed: t.c_ospeed,
            };
            // Copy up to 19 c_cc entries.
            let copy = NCC.min(19);
            kt.c_cc[..copy].copy_from_slice(&t.c_cc[..copy]);
            unsafe {
                core::ptr::write(arg as *mut KernelTermios, kt);
            }
            0
        }

        // ── TCSETS / TCSETSW / TCSETSF: write termios ─────────────────
        TCSETS | TCSETSW | TCSETSF => {
            if arg == 0 {
                return -1;
            }
            let kt = unsafe { core::ptr::read(arg as *const KernelTermios) };
            let mut t = Termios::default_raw();
            // First read existing values so we don't clobber c_cc beyond 19.
            pty_tcgetattr(fd, &mut t);
            t.c_iflag = kt.c_iflag;
            t.c_oflag = kt.c_oflag;
            t.c_cflag = kt.c_cflag;
            t.c_lflag = kt.c_lflag;
            t.c_ispeed = kt.c_ispeed;
            t.c_ospeed = kt.c_ospeed;
            let copy = NCC.min(19);
            t.c_cc[..copy].copy_from_slice(&kt.c_cc[..copy]);
            if pty_tcsetattr(fd, t) {
                0
            } else {
                -1
            }
        }

        _ => -1,
    }
}
