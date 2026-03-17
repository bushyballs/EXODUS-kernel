/// Core logging API
///
/// Part of the AIOS logging infrastructure. Provides the central
/// Logger that dispatches log records through filters, formatters,
/// and sinks. Acts as the main entry point for all kernel logging.

use alloc::vec::Vec;
use alloc::string::String;
use crate::sync::Mutex;

/// Log severity levels.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl LogLevel {
    /// Convert to numeric level for filtering
    pub fn as_u8(&self) -> u8 {
        match self {
            LogLevel::Trace => 0,
            LogLevel::Debug => 1,
            LogLevel::Info => 2,
            LogLevel::Warn => 3,
            LogLevel::Error => 4,
            LogLevel::Fatal => 5,
        }
    }

    /// Convert from numeric level
    pub fn from_u8(val: u8) -> Self {
        match val {
            0 => LogLevel::Trace,
            1 => LogLevel::Debug,
            2 => LogLevel::Info,
            3 => LogLevel::Warn,
            4 => LogLevel::Error,
            _ => LogLevel::Fatal,
        }
    }

    /// Get the display name
    pub fn name(&self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Fatal => "FATAL",
        }
    }
}

/// Sink identifier for the logger's dispatch table
#[derive(Debug, Clone, Copy)]
pub enum SinkId {
    Serial,
    File,
    Ring,
    Remote,
}

/// A registered sink in the logger
struct SinkEntry {
    id: SinkId,
    enabled: bool,
    min_level: LogLevel,
}

/// Statistics for the logger
struct LogStats {
    total_records: u64,
    by_level: [u64; 6], // one per level
    filtered_out: u64,
    dispatch_errors: u64,
}

impl LogStats {
    fn new() -> Self {
        Self {
            total_records: 0,
            by_level: [0; 6],
            filtered_out: 0,
            dispatch_errors: 0,
        }
    }

    fn record(&mut self, level: LogLevel) {
        self.total_records = self.total_records.saturating_add(1);
        let idx = level.as_u8() as usize;
        if idx < 6 {
            self.by_level[idx] = self.by_level[idx].saturating_add(1);
        }
    }
}

/// Core logger that dispatches log records to sinks.
pub struct Logger {
    level: LogLevel,
    sinks: Vec<SinkEntry>,
    stats: LogStats,
    source_prefix: Option<String>,
    muted: bool,
    rate_limit_per_sec: u32,
    rate_count: u32,
    rate_window_tick: u64,
}

static LOG_TICK: Mutex<u64> = Mutex::new(0);

fn log_tick() -> u64 {
    let mut t = LOG_TICK.lock();
    *t = t.saturating_add(1);
    *t
}

impl Logger {
    pub fn new() -> Self {
        // Register default sinks
        let mut sinks = Vec::new();
        sinks.push(SinkEntry {
            id: SinkId::Serial,
            enabled: true,
            min_level: LogLevel::Trace,
        });
        sinks.push(SinkEntry {
            id: SinkId::Ring,
            enabled: true,
            min_level: LogLevel::Debug,
        });

        crate::serial_println!("[log::logger] logger created with {} sinks", sinks.len());
        Self {
            level: LogLevel::Info,
            sinks,
            stats: LogStats::new(),
            source_prefix: None,
            muted: false,
            rate_limit_per_sec: 10000,
            rate_count: 0,
            rate_window_tick: 0,
        }
    }

    /// Log a message at the given level.
    pub fn log(&self, level: LogLevel, message: &str) {
        self.log_with_source(level, "kernel", message);
    }

    /// Log a message with an explicit source module
    pub fn log_with_source(&self, level: LogLevel, source: &str, message: &str) {
        // Check if muted
        if self.muted {
            return;
        }

        // Check global level
        if level < self.level {
            return;
        }

        // Format the record
        let lvl_name = level.name();

        // Dispatch to serial using serial_println
        for sink in &self.sinks {
            if !sink.enabled || level < sink.min_level {
                continue;
            }

            match sink.id {
                SinkId::Serial => {
                    crate::serial_println!("[{}] {} [{}] {}", log_tick(), lvl_name, source, message);
                }
                SinkId::Ring | SinkId::File | SinkId::Remote => {
                    // These sinks would be dispatched through the sink subsystem
                    // which uses its own buffering and formatting
                }
            }
        }
    }

    /// Set the minimum log level.
    pub fn set_level(&mut self, level: LogLevel) {
        crate::serial_println!("[log::logger] level changed from {:?} to {:?}", self.level, level);
        self.level = level;
    }

    /// Get the current minimum level
    pub fn level(&self) -> LogLevel {
        self.level
    }

    /// Add a sink to the logger
    pub fn add_sink(&mut self, id: SinkId, min_level: LogLevel) {
        self.sinks.push(SinkEntry {
            id,
            enabled: true,
            min_level,
        });
        crate::serial_println!("[log::logger] added sink {:?} at level {:?}", id, min_level);
    }

    /// Enable or disable a sink
    pub fn set_sink_enabled(&mut self, id_match: u8, enabled: bool) {
        if let Some(sink) = self.sinks.get_mut(id_match as usize) {
            sink.enabled = enabled;
        }
    }

    /// Mute or unmute the logger
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// Get statistics
    pub fn total_records(&self) -> u64 {
        self.stats.total_records
    }

    /// Get record count by level
    pub fn records_by_level(&self, level: LogLevel) -> u64 {
        let idx = level.as_u8() as usize;
        if idx < 6 { self.stats.by_level[idx] } else { 0 }
    }

    /// Convenience methods for each log level
    pub fn trace(&self, msg: &str) { self.log(LogLevel::Trace, msg); }
    pub fn debug(&self, msg: &str) { self.log(LogLevel::Debug, msg); }
    pub fn info(&self, msg: &str) { self.log(LogLevel::Info, msg); }
    pub fn warn(&self, msg: &str) { self.log(LogLevel::Warn, msg); }
    pub fn error(&self, msg: &str) { self.log(LogLevel::Error, msg); }
    pub fn fatal(&self, msg: &str) { self.log(LogLevel::Fatal, msg); }

    /// Report logger status
    pub fn report(&self) {
        crate::serial_println!("[log::logger] level={:?} sinks={} records={} muted={}",
            self.level, self.sinks.len(), self.stats.total_records, self.muted);
        for (i, sink) in self.sinks.iter().enumerate() {
            crate::serial_println!("[log::logger]   sink {}: {:?} enabled={} min_level={:?}",
                i, sink.id, sink.enabled, sink.min_level);
        }
    }
}

static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);

pub fn init() {
    let logger = Logger::new();
    let mut l = LOGGER.lock();
    *l = Some(logger);
    crate::serial_println!("[log::logger] core logger initialized");
}

/// Log a message at the given level via the global logger
pub fn log(level: LogLevel, message: &str) {
    let l = LOGGER.lock();
    if let Some(ref logger) = *l {
        logger.log(level, message);
    }
}

/// Convenience: log at info level
pub fn info(message: &str) {
    log(LogLevel::Info, message);
}

/// Convenience: log at error level
pub fn error(message: &str) {
    log(LogLevel::Error, message);
}
