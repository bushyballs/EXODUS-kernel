/// Log formatting (text, JSON, binary)
///
/// Part of the AIOS logging infrastructure. Formats log records
/// into various output formats for consumption by sinks.

use alloc::vec::Vec;
use alloc::string::String;
use crate::sync::Mutex;

/// Output format for log records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    PlainText,
    Json,
    Binary,
}

/// Log level names for formatting
fn level_str(level: u8) -> &'static str {
    match level {
        0 => "TRACE",
        1 => "DEBUG",
        2 => "INFO",
        3 => "WARN",
        4 => "ERROR",
        5 => "FATAL",
        _ => "?????",
    }
}

/// Level tag for binary format
fn level_tag(level: u8) -> u8 {
    level.min(5)
}

/// Simple monotonic timestamp
static FMT_TICK: Mutex<u64> = Mutex::new(0);

fn fmt_tick() -> u64 {
    let mut t = FMT_TICK.lock();
    *t = t.saturating_add(1);
    *t
}

/// Append a u64 as decimal to a byte vector
fn append_u64(buf: &mut Vec<u8>, mut val: u64) {
    if val == 0 {
        buf.push(b'0');
        return;
    }
    let start = buf.len();
    while val > 0 {
        buf.push(b'0' + (val % 10) as u8);
        val /= 10;
    }
    // Reverse the digits
    let end = buf.len();
    let mut i = start;
    let mut j = end - 1;
    while i < j {
        let tmp = buf[i];
        buf[i] = buf[j];
        buf[j] = tmp;
        i += 1;
        j -= 1;
    }
}

/// Append a string to a byte vector
fn append_str(buf: &mut Vec<u8>, s: &str) {
    for b in s.as_bytes() {
        buf.push(*b);
    }
}

/// Escape a string for JSON output (handles quotes and backslashes)
fn append_json_string(buf: &mut Vec<u8>, s: &str) {
    buf.push(b'"');
    for c in s.bytes() {
        match c {
            b'"' => { buf.push(b'\\'); buf.push(b'"'); }
            b'\\' => { buf.push(b'\\'); buf.push(b'\\'); }
            b'\n' => { buf.push(b'\\'); buf.push(b'n'); }
            b'\r' => { buf.push(b'\\'); buf.push(b'r'); }
            b'\t' => { buf.push(b'\\'); buf.push(b't'); }
            _ => { buf.push(c); }
        }
    }
    buf.push(b'"');
}

/// Formats log records into the chosen output format.
pub struct LogFormatter {
    format: LogFormat,
    include_timestamp: bool,
    include_source: bool,
    default_source: &'static str,
    sequence: u64,
}

impl LogFormatter {
    pub fn new(format: LogFormat) -> Self {
        crate::serial_println!("[log::format] formatter created: {:?}", format);
        Self {
            format,
            include_timestamp: true,
            include_source: true,
            default_source: "kernel",
            sequence: 0,
        }
    }

    /// Format a log record into bytes.
    pub fn format(&self, level: u8, message: &str) -> Vec<u8> {
        self.format_with_source(level, self.default_source, message)
    }

    /// Format a log record with an explicit source module.
    pub fn format_with_source(&self, level: u8, source: &str, message: &str) -> Vec<u8> {
        match self.format {
            LogFormat::PlainText => self.format_plain(level, source, message),
            LogFormat::Json => self.format_json(level, source, message),
            LogFormat::Binary => self.format_binary(level, source, message),
        }
    }

    /// Format as plain text: "[TIMESTAMP] LEVEL [SOURCE] MESSAGE\n"
    fn format_plain(&self, level: u8, source: &str, message: &str) -> Vec<u8> {
        let mut buf = Vec::with_capacity(message.len() + 64);

        if self.include_timestamp {
            buf.push(b'[');
            append_u64(&mut buf, fmt_tick());
            buf.push(b']');
            buf.push(b' ');
        }

        // Level with fixed-width padding
        let lvl = level_str(level);
        append_str(&mut buf, lvl);
        // Pad to 5 chars
        for _ in lvl.len()..5 {
            buf.push(b' ');
        }
        buf.push(b' ');

        if self.include_source && !source.is_empty() {
            buf.push(b'[');
            append_str(&mut buf, source);
            buf.push(b']');
            buf.push(b' ');
        }

        append_str(&mut buf, message);
        buf.push(b'\n');
        buf
    }

    /// Format as JSON: {"ts":123,"level":"INFO","src":"mod","msg":"..."}
    fn format_json(&self, level: u8, source: &str, message: &str) -> Vec<u8> {
        let mut buf = Vec::with_capacity(message.len() + 128);
        buf.push(b'{');

        if self.include_timestamp {
            append_str(&mut buf, "\"ts\":");
            append_u64(&mut buf, fmt_tick());
            buf.push(b',');
        }

        append_str(&mut buf, "\"level\":");
        append_json_string(&mut buf, level_str(level));
        buf.push(b',');

        append_str(&mut buf, "\"level_num\":");
        append_u64(&mut buf, level as u64);
        buf.push(b',');

        if self.include_source {
            append_str(&mut buf, "\"src\":");
            append_json_string(&mut buf, source);
            buf.push(b',');
        }

        append_str(&mut buf, "\"msg\":");
        append_json_string(&mut buf, message);

        buf.push(b'}');
        buf.push(b'\n');
        buf
    }

    /// Format as compact binary:
    /// [magic:1][level:1][timestamp:8][source_len:2][source][msg_len:4][msg]
    fn format_binary(&self, level: u8, source: &str, message: &str) -> Vec<u8> {
        let source_bytes = source.as_bytes();
        let msg_bytes = message.as_bytes();
        let total = 1 + 1 + 8 + 2 + source_bytes.len() + 4 + msg_bytes.len();
        let mut buf = Vec::with_capacity(total);

        // Magic byte
        buf.push(0xAE);
        // Level
        buf.push(level_tag(level));
        // Timestamp (8 bytes LE)
        let ts = fmt_tick();
        for b in &ts.to_le_bytes() { buf.push(*b); }
        // Source length (2 bytes LE) + source
        let src_len = source_bytes.len() as u16;
        buf.push((src_len & 0xFF) as u8);
        buf.push(((src_len >> 8) & 0xFF) as u8);
        for b in source_bytes { buf.push(*b); }
        // Message length (4 bytes LE) + message
        let msg_len = msg_bytes.len() as u32;
        for b in &msg_len.to_le_bytes() { buf.push(*b); }
        for b in msg_bytes { buf.push(*b); }

        buf
    }

    /// Set whether to include timestamps
    pub fn set_include_timestamp(&mut self, include: bool) {
        self.include_timestamp = include;
    }

    /// Set whether to include source module
    pub fn set_include_source(&mut self, include: bool) {
        self.include_source = include;
    }

    /// Set the output format
    pub fn set_format(&mut self, format: LogFormat) {
        self.format = format;
        crate::serial_println!("[log::format] format changed to {:?}", format);
    }

    /// Get the current format
    pub fn current_format(&self) -> LogFormat {
        self.format
    }
}

static FORMATTER: Mutex<Option<LogFormatter>> = Mutex::new(None);

pub fn init() {
    let formatter = LogFormatter::new(LogFormat::PlainText);
    let mut f = FORMATTER.lock();
    *f = Some(formatter);
    crate::serial_println!("[log::format] format subsystem initialized");
}

/// Format a log record using the global formatter
pub fn format_record(level: u8, message: &str) -> Vec<u8> {
    let f = FORMATTER.lock();
    match f.as_ref() {
        Some(formatter) => formatter.format(level, message),
        None => {
            let mut buf = Vec::new();
            append_str(&mut buf, message);
            buf.push(b'\n');
            buf
        }
    }
}
