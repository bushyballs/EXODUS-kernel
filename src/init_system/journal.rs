/// Binary structured logging journal (journald equivalent)
///
/// Part of the AIOS init_system subsystem.
///
/// Ring-buffer backed structured log with severity levels, per-service
/// filtering, and monotonic timestamps. Entries wrap around when the
/// buffer is full, discarding the oldest entries first.
///
/// Original implementation for Hoags OS. No external crates.

use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── FNV-1a helper ──────────────────────────────────────────────────────────

fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ── Constants ──────────────────────────────────────────────────────────────

/// Maximum number of entries in the ring buffer.
const JOURNAL_CAPACITY: usize = 4096;

// ── Severity levels ────────────────────────────────────────────────────────

/// Log severity levels (syslog-compatible numbering).
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
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Severity::Emergency,
            1 => Severity::Alert,
            2 => Severity::Critical,
            3 => Severity::Error,
            4 => Severity::Warning,
            5 => Severity::Notice,
            6 => Severity::Info,
            _ => Severity::Debug,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Severity::Emergency => "EMERG",
            Severity::Alert     => "ALERT",
            Severity::Critical  => "CRIT",
            Severity::Error     => "ERR",
            Severity::Warning   => "WARN",
            Severity::Notice    => "NOTICE",
            Severity::Info      => "INFO",
            Severity::Debug     => "DEBUG",
        }
    }
}

// ── Journal entry ──────────────────────────────────────────────────────────

/// Structured log entry for the journal.
#[derive(Clone)]
pub struct JournalEntry {
    pub timestamp: u64,
    pub service: String,
    pub priority: u8,
    pub message: String,
    /// FNV-1a hash of the service name for fast filtering.
    service_hash: u64,
    /// Monotonic sequence number (never wraps within boot).
    pub seq: u64,
}

// ── Ring-buffer journal ────────────────────────────────────────────────────

/// Persistent structured logging journal backed by a ring buffer.
struct JournalInner {
    /// Fixed-size ring buffer of entries.
    entries: Vec<Option<JournalEntry>>,
    /// Write head (next slot to write).
    head: usize,
    /// Total entries ever written (monotonic).
    total_written: u64,
    /// Current count of valid entries in the buffer.
    count: usize,
    /// Minimum severity to accept (entries below this are dropped).
    min_severity: Severity,
    /// Whether to also echo entries to serial console.
    echo_serial: bool,
}

impl JournalInner {
    fn new() -> Self {
        let mut entries = Vec::with_capacity(JOURNAL_CAPACITY);
        for _ in 0..JOURNAL_CAPACITY {
            entries.push(None);
        }
        JournalInner {
            entries,
            head: 0,
            total_written: 0,
            count: 0,
            min_severity: Severity::Debug,
            echo_serial: true,
        }
    }

    /// Append a structured log entry to the ring buffer.
    fn log(&mut self, service: &str, priority: u8, message: &str) {
        let sev = Severity::from_u8(priority);
        // Drop entries below minimum severity
        if sev > self.min_severity {
            return;
        }

        self.total_written = self.total_written.saturating_add(1);
        let seq = self.total_written;

        let entry = JournalEntry {
            timestamp: read_tsc(),
            service: String::from(service),
            priority,
            message: String::from(message),
            service_hash: fnv1a_hash(service.as_bytes()),
            seq,
        };

        // Echo to serial if enabled
        if self.echo_serial {
            serial_println!(
                "[journal] {} {} {}: {}",
                seq,
                sev.label(),
                service,
                message
            );
        }

        // Write into ring buffer, overwriting oldest if full
        self.entries[self.head] = Some(entry);
        self.head = (self.head + 1) % JOURNAL_CAPACITY;
        if self.count < JOURNAL_CAPACITY {
            self.count = self.count.saturating_add(1);
        }
    }

    /// Query entries matching optional service name and minimum severity.
    fn query(
        &self,
        service_filter: Option<&str>,
        min_sev: Option<Severity>,
        max_results: usize,
    ) -> Vec<&JournalEntry> {
        let svc_hash = service_filter.map(|s| fnv1a_hash(s.as_bytes()));
        let sev_limit = min_sev.unwrap_or(Severity::Debug);

        let mut results = Vec::new();

        // Walk backwards from head to get newest-first
        let start = if self.head == 0 {
            JOURNAL_CAPACITY - 1
        } else {
            self.head - 1
        };

        let mut visited = 0;
        let mut idx = start;
        while visited < self.count && results.len() < max_results {
            if let Some(ref entry) = self.entries[idx] {
                let entry_sev = Severity::from_u8(entry.priority);
                let sev_ok = entry_sev <= sev_limit;
                let svc_ok = match svc_hash {
                    Some(h) => entry.service_hash == h,
                    None => true,
                };

                if sev_ok && svc_ok {
                    results.push(entry);
                }
            }

            if idx == 0 {
                idx = JOURNAL_CAPACITY - 1;
            } else {
                idx -= 1;
            }
            visited += 1;
        }

        results
    }

    /// Count entries by severity level.
    fn count_by_severity(&self, sev: Severity) -> usize {
        let mut n = 0;
        for entry in &self.entries {
            if let Some(ref e) = entry {
                if Severity::from_u8(e.priority) == sev {
                    n += 1;
                }
            }
        }
        n
    }

    /// Clear all journal entries.
    fn clear(&mut self) {
        for slot in self.entries.iter_mut() {
            *slot = None;
        }
        self.head = 0;
        self.count = 0;
    }
}

// ── Timestamp helper ───────────────────────────────────────────────────────

/// Read TSC as a monotonic timestamp (cycles, not wall-clock).
fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
        ((hi as u64) << 32) | (lo as u64)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

// ── Public legacy wrapper ──────────────────────────────────────────────────

/// Public wrapper matching the original stub API.
pub struct Journal {
    inner: JournalInner,
}

impl Journal {
    pub fn new() -> Self {
        Journal {
            inner: JournalInner::new(),
        }
    }

    /// Append a structured log entry.
    pub fn log(&mut self, entry: JournalEntry) {
        self.inner.log(&entry.service, entry.priority, &entry.message);
    }

    /// Query entries matching a service name filter.
    pub fn query(&self, service: Option<&str>) -> Vec<&JournalEntry> {
        self.inner.query(service, None, 256)
    }
}

// ── Global state ───────────────────────────────────────────────────────────

static JOURNAL: Mutex<Option<JournalInner>> = Mutex::new(None);

/// Initialize the journal subsystem.
pub fn init() {
    let mut guard = JOURNAL.lock();
    *guard = Some(JournalInner::new());
    serial_println!("[init_system::journal] structured journal initialized (capacity={})", JOURNAL_CAPACITY);
}

/// Log a message to the global journal.
pub fn log(service: &str, priority: u8, message: &str) {
    let mut guard = JOURNAL.lock();
    let j = guard.as_mut().expect("journal not initialized");
    j.log(service, priority, message);
}

/// Log convenience: info level.
pub fn info(service: &str, message: &str) {
    log(service, Severity::Info as u8, message);
}

/// Log convenience: error level.
pub fn error(service: &str, message: &str) {
    log(service, Severity::Error as u8, message);
}

/// Log convenience: warning level.
pub fn warn(service: &str, message: &str) {
    log(service, Severity::Warning as u8, message);
}

/// Query the global journal.
pub fn query(service: Option<&str>, max_results: usize) -> Vec<JournalEntry> {
    let guard = JOURNAL.lock();
    let j = guard.as_ref().expect("journal not initialized");
    j.query(service, None, max_results)
        .into_iter()
        .cloned()
        .collect()
}

/// Get total number of entries ever written.
pub fn total_written() -> u64 {
    let guard = JOURNAL.lock();
    let j = guard.as_ref().expect("journal not initialized");
    j.total_written
}

/// Set minimum severity filter.
pub fn set_min_severity(sev: Severity) {
    let mut guard = JOURNAL.lock();
    let j = guard.as_mut().expect("journal not initialized");
    j.min_severity = sev;
}

/// Enable or disable serial echo.
pub fn set_echo_serial(enabled: bool) {
    let mut guard = JOURNAL.lock();
    let j = guard.as_mut().expect("journal not initialized");
    j.echo_serial = enabled;
}

/// Clear all entries from the journal.
pub fn clear() {
    let mut guard = JOURNAL.lock();
    let j = guard.as_mut().expect("journal not initialized");
    j.clear();
}

/// Count entries at a given severity level.
pub fn count_at_severity(sev: Severity) -> usize {
    let guard = JOURNAL.lock();
    let j = guard.as_ref().expect("journal not initialized");
    j.count_by_severity(sev)
}
