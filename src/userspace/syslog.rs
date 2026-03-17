use crate::sync::Mutex;
/// Syslog — system logging daemon for Genesis
///
/// Collects log messages from kernel and userspace, categorizes them
/// by facility and severity, and stores them for later retrieval.
///
/// Facilities: kern, user, daemon, auth, syslog, local0-7
/// Severities: emerg, alert, crit, err, warning, notice, info, debug
///
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

/// Maximum syslog entries
const MAX_ENTRIES: usize = 4096;

/// Syslog facility
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Facility {
    Kern = 0,
    User = 1,
    Daemon = 3,
    Auth = 4,
    Syslog = 5,
    Cron = 9,
    Local0 = 16,
    Local1 = 17,
    Local2 = 18,
    Local3 = 19,
    Local4 = 20,
    Local5 = 21,
    Local6 = 22,
    Local7 = 23,
}

impl Facility {
    pub fn name(self) -> &'static str {
        match self {
            Facility::Kern => "kern",
            Facility::User => "user",
            Facility::Daemon => "daemon",
            Facility::Auth => "auth",
            Facility::Syslog => "syslog",
            Facility::Cron => "cron",
            Facility::Local0 => "local0",
            Facility::Local1 => "local1",
            Facility::Local2 => "local2",
            Facility::Local3 => "local3",
            Facility::Local4 => "local4",
            Facility::Local5 => "local5",
            Facility::Local6 => "local6",
            Facility::Local7 => "local7",
        }
    }
}

/// Syslog severity (same as kernel LogLevel but separate type)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Severity {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Emergency => "emerg",
            Severity::Alert => "alert",
            Severity::Critical => "crit",
            Severity::Error => "err",
            Severity::Warning => "warning",
            Severity::Notice => "notice",
            Severity::Info => "info",
            Severity::Debug => "debug",
        }
    }
}

/// A syslog entry
#[derive(Debug, Clone)]
pub struct SyslogEntry {
    pub timestamp: u64,
    pub facility: Facility,
    pub severity: Severity,
    pub hostname: String,
    pub tag: String,
    pub message: String,
    pub pid: u32,
}

/// Syslog daemon state
struct Syslogd {
    entries: VecDeque<SyslogEntry>,
    min_severity: Severity,
}

impl Syslogd {
    const fn new() -> Self {
        Syslogd {
            entries: VecDeque::new(),
            min_severity: Severity::Debug, // log everything by default
        }
    }
}

static SYSLOG: Mutex<Syslogd> = Mutex::new(Syslogd::new());

/// Log a syslog message
pub fn log(facility: Facility, severity: Severity, tag: &str, msg: &str) {
    let mut syslog = SYSLOG.lock();

    if severity > syslog.min_severity {
        return; // filtered out
    }

    if syslog.entries.len() >= MAX_ENTRIES {
        syslog.entries.pop_front();
    }

    syslog.entries.push_back(SyslogEntry {
        timestamp: crate::time::clock::uptime_secs(),
        facility,
        severity,
        hostname: String::from("genesis"),
        tag: String::from(tag),
        message: String::from(msg),
        pid: crate::process::getpid(),
    });
}

/// Convenience: log a kernel message
pub fn kern(severity: Severity, msg: &str) {
    log(Facility::Kern, severity, "kernel", msg);
}

/// Convenience: log an auth message
pub fn auth(severity: Severity, msg: &str) {
    log(Facility::Auth, severity, "auth", msg);
}

/// Convenience: log a daemon message
pub fn daemon(severity: Severity, msg: &str) {
    log(Facility::Daemon, severity, "daemon", msg);
}

/// Convenience: log a cron message
pub fn cron(severity: Severity, msg: &str) {
    log(Facility::Cron, severity, "crond", msg);
}

/// Read all entries
pub fn read_all() -> Vec<SyslogEntry> {
    SYSLOG.lock().entries.iter().cloned().collect()
}

/// Read entries matching a facility
pub fn read_facility(facility: Facility) -> Vec<SyslogEntry> {
    SYSLOG
        .lock()
        .entries
        .iter()
        .filter(|e| e.facility == facility)
        .cloned()
        .collect()
}

/// Read entries at or above a severity level
pub fn read_severity(max_severity: Severity) -> Vec<SyslogEntry> {
    SYSLOG
        .lock()
        .entries
        .iter()
        .filter(|e| e.severity <= max_severity)
        .cloned()
        .collect()
}

/// Get the last N entries
pub fn tail(n: usize) -> Vec<SyslogEntry> {
    let syslog = SYSLOG.lock();
    let len = syslog.entries.len();
    let skip = if len > n { len - n } else { 0 };
    syslog.entries.iter().skip(skip).cloned().collect()
}

/// Format entries for display
pub fn format_entries(entries: &[SyslogEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        out.push_str(&alloc::format!(
            "{} {} {}.{} {}[{}]: {}\n",
            e.timestamp,
            e.hostname,
            e.facility.name(),
            e.severity.name(),
            e.tag,
            e.pid,
            e.message
        ));
    }
    out
}

/// Set minimum severity filter
pub fn set_level(severity: Severity) {
    SYSLOG.lock().min_severity = severity;
}

/// Clear all entries
pub fn clear() {
    SYSLOG.lock().entries.clear();
}

/// Initialize syslog
pub fn init() {
    log(
        Facility::Syslog,
        Severity::Info,
        "syslogd",
        "System log daemon started",
    );
    serial_println!("  Syslog: daemon ready");
}
