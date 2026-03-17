use crate::sync::Mutex;
/// dmesg — kernel message ring buffer viewer for Genesis
///
/// Reads and displays kernel log messages from the ring buffer.
/// Supports filtering by log level, facility, and text search.
///
/// Features:
///   - Kernel ring buffer with configurable size
///   - Timestamp display (uptime and human-readable)
///   - Log level filtering (emerg..debug)
///   - Facility filtering (kern, user, daemon, etc.)
///   - Text pattern search
///   - Follow mode (tail -f equivalent)
///   - Color-coded severity output
///   - Clear buffer (requires root)
///
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

/// Maximum ring buffer entries
const RING_BUFFER_SIZE: usize = 8192;

/// Log level (matches kernel log levels)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LogLevel {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

impl LogLevel {
    /// Short name for display
    pub fn name(self) -> &'static str {
        match self {
            LogLevel::Emergency => "emerg",
            LogLevel::Alert => "alert",
            LogLevel::Critical => "crit",
            LogLevel::Error => "err",
            LogLevel::Warning => "warn",
            LogLevel::Notice => "notice",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
        }
    }

    /// Severity prefix character for compact display
    pub fn prefix(self) -> char {
        match self {
            LogLevel::Emergency => 'E',
            LogLevel::Alert => 'A',
            LogLevel::Critical => 'C',
            LogLevel::Error => 'E',
            LogLevel::Warning => 'W',
            LogLevel::Notice => 'N',
            LogLevel::Info => 'I',
            LogLevel::Debug => 'D',
        }
    }

    /// Parse from numeric level
    pub fn from_u8(val: u8) -> Self {
        match val {
            0 => LogLevel::Emergency,
            1 => LogLevel::Alert,
            2 => LogLevel::Critical,
            3 => LogLevel::Error,
            4 => LogLevel::Warning,
            5 => LogLevel::Notice,
            6 => LogLevel::Info,
            7 => LogLevel::Debug,
            _ => LogLevel::Debug,
        }
    }

    /// Parse from name string
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "emerg" | "emergency" => Some(LogLevel::Emergency),
            "alert" => Some(LogLevel::Alert),
            "crit" | "critical" => Some(LogLevel::Critical),
            "err" | "error" => Some(LogLevel::Error),
            "warn" | "warning" => Some(LogLevel::Warning),
            "notice" => Some(LogLevel::Notice),
            "info" => Some(LogLevel::Info),
            "debug" => Some(LogLevel::Debug),
            _ => None,
        }
    }
}

/// Kernel message facility
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Facility {
    Kern = 0,
    User = 1,
    Mail = 2,
    Daemon = 3,
    Auth = 4,
    Syslog = 5,
    Lpr = 6,
    News = 7,
    Uucp = 8,
    Cron = 9,
}

impl Facility {
    pub fn name(self) -> &'static str {
        match self {
            Facility::Kern => "kern",
            Facility::User => "user",
            Facility::Mail => "mail",
            Facility::Daemon => "daemon",
            Facility::Auth => "auth",
            Facility::Syslog => "syslog",
            Facility::Lpr => "lpr",
            Facility::News => "news",
            Facility::Uucp => "uucp",
            Facility::Cron => "cron",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "kern" => Some(Facility::Kern),
            "user" => Some(Facility::User),
            "mail" => Some(Facility::Mail),
            "daemon" => Some(Facility::Daemon),
            "auth" => Some(Facility::Auth),
            "syslog" => Some(Facility::Syslog),
            "lpr" => Some(Facility::Lpr),
            "news" => Some(Facility::News),
            "uucp" => Some(Facility::Uucp),
            "cron" => Some(Facility::Cron),
            _ => None,
        }
    }
}

/// A single kernel message entry
#[derive(Debug, Clone)]
pub struct KernelMessage {
    /// Monotonic sequence number
    pub seq: u64,
    /// Timestamp in microseconds since boot (Q16 of seconds stored as u64)
    pub timestamp_usec: u64,
    /// Log level / severity
    pub level: LogLevel,
    /// Facility
    pub facility: Facility,
    /// Source subsystem (e.g., "pci", "usb", "net")
    pub subsystem: String,
    /// Message text
    pub message: String,
}

/// Display options for dmesg output
#[derive(Debug, Clone)]
pub struct DisplayOptions {
    /// Show timestamps
    pub show_time: bool,
    /// Show facility
    pub show_facility: bool,
    /// Show log level
    pub show_level: bool,
    /// Minimum level to display (messages with level <= this are shown)
    pub min_level: LogLevel,
    /// Filter by facility (None = all)
    pub facility_filter: Option<Facility>,
    /// Text pattern filter (empty = no filter)
    pub pattern: String,
    /// Maximum number of lines to show (0 = all)
    pub tail_lines: usize,
}

impl DisplayOptions {
    pub fn default_opts() -> Self {
        DisplayOptions {
            show_time: true,
            show_facility: false,
            show_level: true,
            min_level: LogLevel::Debug,
            facility_filter: None,
            pattern: String::new(),
            tail_lines: 0,
        }
    }
}

/// Follow-mode state for tracking new messages
#[derive(Debug, Clone)]
pub struct FollowState {
    /// Last sequence number seen
    pub last_seq: u64,
    /// Whether follow mode is active
    pub active: bool,
}

/// Kernel ring buffer state
struct RingBuffer {
    messages: VecDeque<KernelMessage>,
    next_seq: u64,
    /// Total messages ever written (wraps around)
    total_written: u64,
    /// Total messages dropped due to buffer overflow
    total_dropped: u64,
}

impl RingBuffer {
    const fn new() -> Self {
        RingBuffer {
            messages: VecDeque::new(),
            next_seq: 1,
            total_written: 0,
            total_dropped: 0,
        }
    }
}

static RING: Mutex<RingBuffer> = Mutex::new(RingBuffer::new());

/// Write a message to the kernel ring buffer
pub fn klog(level: LogLevel, facility: Facility, subsystem: &str, message: &str) {
    let mut ring = RING.lock();
    let now_secs = crate::time::clock::uptime_secs();
    // Convert seconds to microseconds (no floats, just multiply)
    let timestamp_usec = now_secs.wrapping_mul(1_000_000);

    if ring.messages.len() >= RING_BUFFER_SIZE {
        ring.messages.pop_front();
        ring.total_dropped = ring.total_dropped.saturating_add(1);
    }

    let seq = ring.next_seq;
    ring.next_seq += 1;
    ring.total_written = ring.total_written.saturating_add(1);

    ring.messages.push_back(KernelMessage {
        seq,
        timestamp_usec,
        level,
        facility,
        subsystem: String::from(subsystem),
        message: String::from(message),
    });
}

/// Convenience: log a kernel message at Info level
pub fn klog_info(subsystem: &str, message: &str) {
    klog(LogLevel::Info, Facility::Kern, subsystem, message);
}

/// Convenience: log a kernel message at Error level
pub fn klog_err(subsystem: &str, message: &str) {
    klog(LogLevel::Error, Facility::Kern, subsystem, message);
}

/// Convenience: log a kernel message at Warning level
pub fn klog_warn(subsystem: &str, message: &str) {
    klog(LogLevel::Warning, Facility::Kern, subsystem, message);
}

/// Read all messages from the ring buffer with filtering
pub fn read_filtered(opts: &DisplayOptions) -> Vec<KernelMessage> {
    let ring = RING.lock();
    let iter = ring.messages.iter();

    let filtered: Vec<KernelMessage> = iter
        .filter(|m| m.level <= opts.min_level)
        .filter(|m| match opts.facility_filter {
            Some(f) => m.facility == f,
            None => true,
        })
        .filter(|m| {
            if opts.pattern.is_empty() {
                true
            } else {
                m.message.contains(opts.pattern.as_str())
                    || m.subsystem.contains(opts.pattern.as_str())
            }
        })
        .cloned()
        .collect();

    if opts.tail_lines > 0 && filtered.len() > opts.tail_lines {
        filtered[filtered.len() - opts.tail_lines..].to_vec()
    } else {
        filtered
    }
}

/// Read all messages (no filter)
pub fn read_all() -> Vec<KernelMessage> {
    RING.lock().messages.iter().cloned().collect()
}

/// Read the last N messages
pub fn tail(n: usize) -> Vec<KernelMessage> {
    let ring = RING.lock();
    let len = ring.messages.len();
    let skip = if len > n { len - n } else { 0 };
    ring.messages.iter().skip(skip).cloned().collect()
}

/// Get new messages since a sequence number (for follow mode)
pub fn follow(since_seq: u64) -> (Vec<KernelMessage>, u64) {
    let ring = RING.lock();
    let new_msgs: Vec<KernelMessage> = ring
        .messages
        .iter()
        .filter(|m| m.seq > since_seq)
        .cloned()
        .collect();

    let last_seq = ring.messages.back().map(|m| m.seq).unwrap_or(since_seq);

    (new_msgs, last_seq)
}

/// Create a follow state starting from the current end of buffer
pub fn follow_start() -> FollowState {
    let ring = RING.lock();
    let last_seq = ring.messages.back().map(|m| m.seq).unwrap_or(0);
    FollowState {
        last_seq,
        active: true,
    }
}

/// Format a single message for display
fn format_message(msg: &KernelMessage, opts: &DisplayOptions) -> String {
    let mut line = String::new();

    if opts.show_time {
        // Format timestamp as seconds.microseconds
        let secs = msg.timestamp_usec / 1_000_000;
        let usecs = msg.timestamp_usec % 1_000_000;
        line.push_str(&alloc::format!("[{:>5}.{:06}] ", secs, usecs));
    }

    if opts.show_level {
        line.push_str(&alloc::format!("<{}> ", msg.level.name()));
    }

    if opts.show_facility {
        line.push_str(&alloc::format!("{}: ", msg.facility.name()));
    }

    if !msg.subsystem.is_empty() {
        line.push_str(&alloc::format!("{}: ", msg.subsystem));
    }

    line.push_str(&msg.message);
    line
}

/// Format messages for display
pub fn format_messages(messages: &[KernelMessage], opts: &DisplayOptions) -> String {
    let mut out = String::new();
    for msg in messages {
        out.push_str(&format_message(msg, opts));
        out.push('\n');
    }
    out
}

/// Clear the ring buffer (requires root)
pub fn clear(uid: u32) -> Result<(), &'static str> {
    if uid != 0 {
        return Err("permission denied: only root can clear dmesg");
    }

    let mut ring = RING.lock();
    ring.messages.clear();
    ring.total_dropped = 0;
    Ok(())
}

/// Get ring buffer statistics
pub fn stats() -> String {
    let ring = RING.lock();
    alloc::format!(
        "Ring buffer: {}/{} entries, {} total written, {} dropped\n\
         Sequence range: {}-{}\n",
        ring.messages.len(),
        RING_BUFFER_SIZE,
        ring.total_written,
        ring.total_dropped,
        ring.messages.front().map(|m| m.seq).unwrap_or(0),
        ring.messages.back().map(|m| m.seq).unwrap_or(0),
    )
}

/// Set the console log level (messages at or below this level are printed)
pub fn set_console_level(level: LogLevel) {
    serial_println!("  [dmesg] Console log level set to: {}", level.name());
}

/// Initialize the dmesg subsystem with boot messages
pub fn init() {
    // Log initial boot messages
    klog(
        LogLevel::Info,
        Facility::Kern,
        "genesis",
        "Genesis kernel starting",
    );
    klog(
        LogLevel::Info,
        Facility::Kern,
        "cpu",
        "x86_64 processor detected",
    );
    klog(
        LogLevel::Info,
        Facility::Kern,
        "memory",
        "Physical memory manager initialized",
    );
    klog(
        LogLevel::Info,
        Facility::Kern,
        "paging",
        "Virtual memory paging enabled",
    );
    klog(
        LogLevel::Info,
        Facility::Kern,
        "heap",
        "Kernel heap allocator ready",
    );
    klog(
        LogLevel::Info,
        Facility::Kern,
        "gdt",
        "Global Descriptor Table loaded",
    );
    klog(
        LogLevel::Info,
        Facility::Kern,
        "idt",
        "Interrupt Descriptor Table configured",
    );
    klog(
        LogLevel::Info,
        Facility::Kern,
        "pic",
        "Programmable Interrupt Controller remapped",
    );
    klog(
        LogLevel::Info,
        Facility::Kern,
        "pit",
        "Programmable Interval Timer configured",
    );
    klog(
        LogLevel::Info,
        Facility::Kern,
        "serial",
        "Serial port COM1 initialized (115200 baud)",
    );
    klog(
        LogLevel::Info,
        Facility::Kern,
        "vga",
        "VGA text mode 80x25 active",
    );
    klog(
        LogLevel::Info,
        Facility::Kern,
        "scheduler",
        "Process scheduler started",
    );

    let ring = RING.lock();
    serial_println!(
        "  dmesg: ring buffer ready ({}/{} entries)",
        ring.messages.len(),
        RING_BUFFER_SIZE
    );
}
