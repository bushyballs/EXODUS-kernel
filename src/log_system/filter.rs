/// Log level and source filtering
///
/// Part of the AIOS logging infrastructure. Filters log records
/// based on severity level and source module name. Supports
/// both allowlists and blocklists for source-based filtering.

use alloc::vec::Vec;
use crate::sync::Mutex;

/// Log level numeric values (lower = more verbose)
pub const LEVEL_TRACE: u8 = 0;
pub const LEVEL_DEBUG: u8 = 1;
pub const LEVEL_INFO: u8 = 2;
pub const LEVEL_WARN: u8 = 3;
pub const LEVEL_ERROR: u8 = 4;
pub const LEVEL_FATAL: u8 = 5;

/// Get a human-readable name for a log level
pub fn level_name(level: u8) -> &'static str {
    match level {
        0 => "TRACE",
        1 => "DEBUG",
        2 => "INFO",
        3 => "WARN",
        4 => "ERROR",
        5 => "FATAL",
        _ => "UNKNOWN",
    }
}

/// Filter mode for source matching
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// Only allow sources in the allowed list
    Allowlist,
    /// Allow all sources except those in the blocked list
    Blocklist,
    /// Allow all sources (no source filtering)
    PassAll,
}

/// Filters log records by level and source module.
pub struct LogFilter {
    min_level: u8,
    allowed_sources: Vec<&'static str>,
    blocked_sources: Vec<&'static str>,
    mode: FilterMode,
    /// Per-source level overrides: (source, min_level)
    source_levels: Vec<(&'static str, u8)>,
    total_checked: u64,
    total_passed: u64,
    total_filtered: u64,
}

impl LogFilter {
    pub fn new() -> Self {
        crate::serial_println!("[log::filter] filter created, min_level=INFO, mode=PassAll");
        Self {
            min_level: LEVEL_INFO,
            allowed_sources: Vec::new(),
            blocked_sources: Vec::new(),
            mode: FilterMode::PassAll,
            source_levels: Vec::new(),
            total_checked: 0,
            total_passed: 0,
            total_filtered: 0,
        }
    }

    /// Create a filter with a specific minimum level
    pub fn with_level(level: u8) -> Self {
        let mut f = Self::new();
        f.min_level = level;
        f
    }

    /// Check if a log record passes this filter.
    pub fn matches(&self, level: u8, source: &str) -> bool {
        // Check per-source level override first
        for &(src, src_level) in &self.source_levels {
            if source_matches(source, src) {
                return level >= src_level;
            }
        }

        // Check global level threshold
        if level < self.min_level {
            return false;
        }

        // Check source filter
        match self.mode {
            FilterMode::PassAll => true,
            FilterMode::Allowlist => {
                if self.allowed_sources.is_empty() {
                    return true; // empty allowlist = allow all
                }
                for allowed in &self.allowed_sources {
                    if source_matches(source, allowed) {
                        return true;
                    }
                }
                false
            }
            FilterMode::Blocklist => {
                for blocked in &self.blocked_sources {
                    if source_matches(source, blocked) {
                        return false;
                    }
                }
                true
            }
        }
    }

    /// Mutable version of matches that tracks statistics
    pub fn check(&mut self, level: u8, source: &str) -> bool {
        self.total_checked = self.total_checked.saturating_add(1);
        let result = self.matches(level, source);
        if result {
            self.total_passed = self.total_passed.saturating_add(1);
        } else {
            self.total_filtered = self.total_filtered.saturating_add(1);
        }
        result
    }

    /// Add an allowed source module.
    pub fn allow_source(&mut self, source: &'static str) {
        // Avoid duplicates
        for s in &self.allowed_sources {
            if *s == source { return; }
        }
        self.allowed_sources.push(source);
        if self.mode == FilterMode::PassAll {
            self.mode = FilterMode::Allowlist;
        }
        crate::serial_println!("[log::filter] allowed source: '{}'", source);
    }

    /// Block a source module
    pub fn block_source(&mut self, source: &'static str) {
        for s in &self.blocked_sources {
            if *s == source { return; }
        }
        self.blocked_sources.push(source);
        if self.mode == FilterMode::PassAll {
            self.mode = FilterMode::Blocklist;
        }
    }

    /// Set the minimum log level
    pub fn set_level(&mut self, level: u8) {
        self.min_level = level;
        crate::serial_println!("[log::filter] min_level set to {} ({})", level, level_name(level));
    }

    /// Set the filter mode
    pub fn set_mode(&mut self, mode: FilterMode) {
        self.mode = mode;
    }

    /// Set a per-source level override
    pub fn set_source_level(&mut self, source: &'static str, level: u8) {
        // Replace existing override
        for entry in self.source_levels.iter_mut() {
            if entry.0 == source {
                entry.1 = level;
                return;
            }
        }
        self.source_levels.push((source, level));
        crate::serial_println!("[log::filter] source '{}' level set to {}", source, level_name(level));
    }

    /// Remove a per-source level override
    pub fn remove_source_level(&mut self, source: &'static str) {
        let mut i = 0;
        while i < self.source_levels.len() {
            if self.source_levels[i].0 == source {
                self.source_levels.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Get filter statistics
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.total_checked, self.total_passed, self.total_filtered)
    }

    /// Get the current minimum level
    pub fn min_level(&self) -> u8 {
        self.min_level
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.total_checked = 0;
        self.total_passed = 0;
        self.total_filtered = 0;
    }
}

/// Check if a source name matches a pattern.
/// Supports prefix matching: "app" matches "app::runtime", "app::data", etc.
fn source_matches(source: &str, pattern: &str) -> bool {
    if source == pattern {
        return true;
    }
    // Prefix match: "app" matches "app::anything"
    if source.len() > pattern.len() {
        let prefix = &source[..pattern.len()];
        if prefix == pattern {
            let next_bytes = source.as_bytes();
            if pattern.len() < next_bytes.len() && next_bytes[pattern.len()] == b':' {
                return true;
            }
        }
    }
    false
}

static FILTER: Mutex<Option<LogFilter>> = Mutex::new(None);

pub fn init() {
    let filter = LogFilter::new();
    let mut f = FILTER.lock();
    *f = Some(filter);
    crate::serial_println!("[log::filter] filter subsystem initialized");
}

/// Check a log record against the global filter
pub fn should_log(level: u8, source: &str) -> bool {
    let f = FILTER.lock();
    match f.as_ref() {
        Some(filter) => filter.matches(level, source),
        None => true, // no filter = allow all
    }
}
