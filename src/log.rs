// log.rs - Simple logging module for Genesis OS

/// Log levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Simple logger (to be replaced with proper implementation)
pub struct Logger;

impl Logger {
    pub fn log(level: LogLevel, message: &str) {
        // In real implementation, this would:
        // 1. Write to serial port
        // 2. Write to kernel log buffer
        // 3. Optionally write to display (early boot)

        // For now, placeholder
        let _ = (level, message);
    }
}

// Macros for convenient logging
#[macro_export]
macro_rules! log_trace {
    ($($arg:tt)*) => {
        // Logger::log(LogLevel::Trace, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        // Logger::log(LogLevel::Debug, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        // Logger::log(LogLevel::Info, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        // Logger::log(LogLevel::Warn, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        // Logger::log(LogLevel::Error, format_args!($($arg)*))
    };
}

// Re-export for external use as log::
pub mod log {
    pub use super::*;

    pub fn info(_msg: &str) {}
    pub fn debug(_msg: &str) {}
    pub fn warn(_msg: &str) {}
    pub fn error(_msg: &str) {}
    pub fn trace(_msg: &str) {}
}
