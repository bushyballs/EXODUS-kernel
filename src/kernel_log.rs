use crate::sync::Mutex;
/// Kernel log ring buffer (dmesg)
///
/// Captures all kernel messages in a ring buffer for later viewing
/// via the `dmesg` command. Stores timestamps, log levels, and messages.
///
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;

/// Maximum log entries in the ring buffer
const LOG_RING_SIZE: usize = 2048;

/// Log levels (matching Linux syslog)
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
    pub fn prefix(self) -> &'static str {
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
}

/// A single log entry
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp_ms: u64,
    pub level: LogLevel,
    pub message: String,
}

/// Kernel log ring buffer
struct KernelLog {
    entries: VecDeque<LogEntry>,
    sequence: u64,
}

impl KernelLog {
    const fn new() -> Self {
        KernelLog {
            entries: VecDeque::new(),
            sequence: 0,
        }
    }
}

static KLOG: Mutex<KernelLog> = Mutex::new(KernelLog::new());

/// Log a message at the given level
pub fn log(level: LogLevel, msg: &str) {
    let timestamp_ms = crate::time::clock::uptime_ms();
    let mut klog = KLOG.lock();
    if klog.entries.len() >= LOG_RING_SIZE {
        klog.entries.pop_front();
    }
    klog.entries.push_back(LogEntry {
        timestamp_ms,
        level,
        message: String::from(msg),
    });
    klog.sequence += 1;
}

/// Log at info level
pub fn info(msg: &str) {
    log(LogLevel::Info, msg);
}

/// Log at warning level
pub fn warn(msg: &str) {
    log(LogLevel::Warning, msg);
}

/// Log at error level
pub fn error(msg: &str) {
    log(LogLevel::Error, msg);
}

/// Get all log entries (for dmesg)
pub fn read_all() -> alloc::vec::Vec<LogEntry> {
    let klog = KLOG.lock();
    klog.entries.iter().cloned().collect()
}

/// Get log entries since a given sequence number
pub fn read_since(since_seq: u64) -> alloc::vec::Vec<LogEntry> {
    let klog = KLOG.lock();
    let total = klog.sequence;
    let buf_len = klog.entries.len() as u64;

    if since_seq >= total {
        return alloc::vec::Vec::new();
    }

    let skip = if total - since_seq >= buf_len {
        0
    } else {
        (buf_len - (total - since_seq)) as usize
    };

    klog.entries.iter().skip(skip).cloned().collect()
}

/// Get current sequence number
pub fn sequence() -> u64 {
    KLOG.lock().sequence
}

/// Format log entries for display (like dmesg output)
pub fn format_entries(entries: &[LogEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        let secs = entry.timestamp_ms / 1000;
        let ms = entry.timestamp_ms % 1000;
        out.push_str(&format!(
            "[{:5}.{:03}] {}: {}\n",
            secs,
            ms,
            entry.level.prefix(),
            entry.message
        ));
    }
    out
}

/// Initialize the kernel log
pub fn init() {
    info("Genesis kernel log initialized");
}

/// Macro to log a formatted kernel message
#[macro_export]
macro_rules! klog {
    ($level:expr, $($arg:tt)*) => {
        $crate::kernel_log::log($level, &alloc::format!($($arg)*))
    };
}
