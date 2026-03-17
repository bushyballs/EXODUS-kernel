/// Terminal I/O settings compatibility (POSIX termios)
///
/// Part of the AIOS compatibility layer.
///
/// Provides POSIX-compatible terminal I/O control: baud rates, character
/// sizes, parity, stop bits, local/input/output flags, control characters,
/// and line discipline modes (raw, cooked, cbreak).
///
/// Design:
///   - Termios struct mirrors the POSIX termios with c_iflag, c_oflag,
///     c_cflag, c_lflag, and c_cc arrays.
///   - Per-TTY settings stored in a global table keyed by fd.
///   - tcgetattr/tcsetattr translate to queries/modifications on the table.
///   - cfmakeraw/cfsetspeed convenience functions.
///   - Global Mutex<Option<Inner>> singleton.
///
/// Inspired by: POSIX termios (termios.h), Linux tty_ioctl. All code is original.

use alloc::vec::Vec;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// Flag constants
// ---------------------------------------------------------------------------

// Input flags (c_iflag)
pub const IGNBRK: u32 = 0x0001;
pub const BRKINT: u32 = 0x0002;
pub const IGNPAR: u32 = 0x0004;
pub const PARMRK: u32 = 0x0008;
pub const INPCK: u32 = 0x0010;
pub const ISTRIP: u32 = 0x0020;
pub const INLCR: u32 = 0x0040;
pub const IGNCR: u32 = 0x0080;
pub const ICRNL: u32 = 0x0100;
pub const IXON: u32 = 0x0400;
pub const IXOFF: u32 = 0x1000;
pub const IXANY: u32 = 0x0800;

// Output flags (c_oflag)
pub const OPOST: u32 = 0x0001;
pub const ONLCR: u32 = 0x0004;
pub const OCRNL: u32 = 0x0008;
pub const ONOCR: u32 = 0x0010;
pub const ONLRET: u32 = 0x0020;

// Control flags (c_cflag)
pub const CSIZE: u32 = 0x0030;
pub const CS5: u32 = 0x0000;
pub const CS6: u32 = 0x0010;
pub const CS7: u32 = 0x0020;
pub const CS8: u32 = 0x0030;
pub const CSTOPB: u32 = 0x0040;
pub const CREAD: u32 = 0x0080;
pub const PARENB: u32 = 0x0100;
pub const PARODD: u32 = 0x0200;
pub const HUPCL: u32 = 0x0400;
pub const CLOCAL: u32 = 0x0800;

// Local flags (c_lflag)
pub const ISIG: u32 = 0x0001;
pub const ICANON: u32 = 0x0002;
pub const ECHO: u32 = 0x0008;
pub const ECHOE: u32 = 0x0010;
pub const ECHOK: u32 = 0x0020;
pub const ECHONL: u32 = 0x0040;
pub const NOFLSH: u32 = 0x0080;
pub const TOSTOP: u32 = 0x0100;
pub const IEXTEN: u32 = 0x8000;

// Control character indices
pub const VEOF: usize = 0;
pub const VEOL: usize = 1;
pub const VERASE: usize = 2;
pub const VINTR: usize = 3;
pub const VKILL: usize = 4;
pub const VMIN: usize = 5;
pub const VQUIT: usize = 6;
pub const VSTART: usize = 7;
pub const VSTOP: usize = 8;
pub const VSUSP: usize = 9;
pub const VTIME: usize = 10;
pub const NCCS: usize = 32;

// Baud rate constants
pub const B0: u32 = 0;
pub const B9600: u32 = 9600;
pub const B19200: u32 = 19200;
pub const B38400: u32 = 38400;
pub const B57600: u32 = 57600;
pub const B115200: u32 = 115200;

// tcsetattr actions
pub const TCSANOW: u32 = 0;
pub const TCSADRAIN: u32 = 1;
pub const TCSAFLUSH: u32 = 2;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// POSIX-compatible terminal I/O settings.
#[derive(Clone)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_cc: [u8; NCCS],
    pub ispeed: u32,
    pub ospeed: u32,
}

impl Termios {
    /// Create default cooked-mode terminal settings.
    pub fn new() -> Self {
        let mut cc = [0u8; NCCS];
        cc[VEOF] = 0x04;   // Ctrl-D
        cc[VEOL] = 0x00;
        cc[VERASE] = 0x7F; // DEL
        cc[VINTR] = 0x03;  // Ctrl-C
        cc[VKILL] = 0x15;  // Ctrl-U
        cc[VMIN] = 1;
        cc[VQUIT] = 0x1C;  // Ctrl-Backslash
        cc[VSTART] = 0x11; // Ctrl-Q
        cc[VSTOP] = 0x13;  // Ctrl-S
        cc[VSUSP] = 0x1A;  // Ctrl-Z
        cc[VTIME] = 0;

        Termios {
            c_iflag: ICRNL | IXON,
            c_oflag: OPOST | ONLCR,
            c_cflag: CS8 | CREAD | CLOCAL,
            c_lflag: ISIG | ICANON | ECHO | ECHOE | ECHOK | IEXTEN,
            c_cc: cc,
            ispeed: B115200,
            ospeed: B115200,
        }
    }

    /// Set raw mode (no processing, byte-at-a-time input).
    pub fn make_raw(&mut self) {
        self.c_iflag &= !(IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | ICRNL | IXON);
        self.c_oflag &= !OPOST;
        self.c_lflag &= !(ECHO | ECHONL | ICANON | ISIG | IEXTEN);
        self.c_cflag &= !(CSIZE | PARENB);
        self.c_cflag |= CS8;
        self.c_cc[VMIN] = 1;
        self.c_cc[VTIME] = 0;
    }

    /// Set baud rate (both input and output).
    pub fn set_speed(&mut self, speed: u32) {
        self.ispeed = speed;
        self.ospeed = speed;
    }

    /// Check if in canonical (cooked) mode.
    pub fn is_canonical(&self) -> bool {
        self.c_lflag & ICANON != 0
    }

    /// Check if echo is enabled.
    pub fn is_echo(&self) -> bool {
        self.c_lflag & ECHO != 0
    }

    /// Check if signals (INTR, QUIT, SUSP) are enabled.
    pub fn is_isig(&self) -> bool {
        self.c_lflag & ISIG != 0
    }
}

/// Per-TTY settings entry.
struct TtyEntry {
    fd: usize,
    settings: Termios,
}

/// Inner state.
struct Inner {
    ttys: Vec<TtyEntry>,
}

// ---------------------------------------------------------------------------
// Inner implementation
// ---------------------------------------------------------------------------

impl Inner {
    fn new() -> Self {
        Inner { ttys: Vec::new() }
    }

    fn get_settings(&self, fd: usize) -> Option<&Termios> {
        self.ttys.iter().find(|t| t.fd == fd).map(|t| &t.settings)
    }

    fn set_settings(&mut self, fd: usize, settings: Termios) {
        for t in self.ttys.iter_mut() {
            if t.fd == fd {
                t.settings = settings;
                return;
            }
        }
        self.ttys.push(TtyEntry { fd, settings });
    }

    fn remove(&mut self, fd: usize) {
        self.ttys.retain(|t| t.fd != fd);
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static TERMIOS: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Get terminal settings for a file descriptor (tcgetattr).
pub fn tcgetattr(fd: usize) -> Option<Termios> {
    let guard = TERMIOS.lock();
    guard.as_ref().and_then(|inner| inner.get_settings(fd).cloned())
}

/// Set terminal settings for a file descriptor (tcsetattr).
pub fn tcsetattr(fd: usize, _action: u32, settings: Termios) -> Result<(), i32> {
    let mut guard = TERMIOS.lock();
    let inner = guard.as_mut().ok_or(-9)?; // EBADF
    inner.set_settings(fd, settings);
    Ok(())
}

/// Create default settings for a new TTY fd.
pub fn register_tty(fd: usize) {
    let mut guard = TERMIOS.lock();
    if let Some(inner) = guard.as_mut() {
        inner.set_settings(fd, Termios::new());
    }
}

/// Remove settings when a TTY fd is closed.
pub fn unregister_tty(fd: usize) {
    let mut guard = TERMIOS.lock();
    if let Some(inner) = guard.as_mut() {
        inner.remove(fd);
    }
}

/// Initialize the termios subsystem.
pub fn init() {
    let mut guard = TERMIOS.lock();
    *guard = Some(Inner::new());
    serial_println!("    termios: initialized (baud rates, flags, line discipline)");
}
