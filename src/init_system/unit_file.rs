/// Service unit file parser
///
/// Part of the AIOS init_system subsystem.
///
/// Parses INI-style unit files with [Unit], [Service], and [Install]
/// sections. Extracts service metadata, dependencies, exec commands,
/// restart policies, and resource limits. Uses FNV-1a hashing for
/// fast section/key comparisons.
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

// Well-known section name hashes (precomputed FNV-1a).
const HASH_UNIT: u64 = 0xcbf29ce484232d5d; // placeholder, computed at init
const HASH_SERVICE: u64 = 0xcbf29ce484232d5e;
const HASH_INSTALL: u64 = 0xcbf29ce484232d5f;

/// Compute hashes at usage time instead of relying on const precomputation.
fn hash_eq(a: &str, b: &str) -> bool {
    fnv1a_hash(a.as_bytes()) == fnv1a_hash(b.as_bytes())
}

// ── Restart policy ─────────────────────────────────────────────────────────

/// Restart policy for a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Never restart on failure.
    No,
    /// Restart only on non-zero exit.
    OnFailure,
    /// Always restart regardless of exit status.
    Always,
    /// Restart on abnormal termination (signal, timeout).
    OnAbnormal,
}

impl RestartPolicy {
    fn from_str(s: &str) -> Self {
        let trimmed = s.trim();
        if hash_eq(trimmed, "always") {
            RestartPolicy::Always
        } else if hash_eq(trimmed, "on-failure") {
            RestartPolicy::OnFailure
        } else if hash_eq(trimmed, "on-abnormal") {
            RestartPolicy::OnAbnormal
        } else {
            RestartPolicy::No
        }
    }
}

// ── Service type ───────────────────────────────────────────────────────────

/// Service execution type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceType {
    /// Simple: main process is the service.
    Simple,
    /// Forking: service forks and parent exits.
    Forking,
    /// Oneshot: runs once and exits.
    Oneshot,
    /// Notify: service sends readiness notification.
    Notify,
}

impl ServiceType {
    fn from_str(s: &str) -> Self {
        let trimmed = s.trim();
        if hash_eq(trimmed, "forking") {
            ServiceType::Forking
        } else if hash_eq(trimmed, "oneshot") {
            ServiceType::Oneshot
        } else if hash_eq(trimmed, "notify") {
            ServiceType::Notify
        } else {
            ServiceType::Simple
        }
    }
}

// ── Parse error ────────────────────────────────────────────────────────────

/// Unit file parse errors.
#[derive(Debug)]
pub enum ParseError {
    InvalidSection,
    MissingField,
    SyntaxError,
}

// ── Parsed unit file ───────────────────────────────────────────────────────

/// Parsed service unit file with all extracted metadata.
pub struct UnitFile {
    pub name: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub wants: Vec<String>,
    pub after: Vec<String>,
    pub before: Vec<String>,
    pub exec_start: String,
    pub exec_stop: String,
    pub service_type: ServiceType,
    pub restart_policy: RestartPolicy,
    pub restart_sec: u64,
    pub timeout_start_sec: u64,
    pub timeout_stop_sec: u64,
    pub wanted_by: Vec<String>,
    pub watchdog_sec: u64,
    pub memory_max: u64,
    pub cpu_weight: u32,
}

impl UnitFile {
    /// Parse a unit file from its text contents.
    ///
    /// Format is INI-like:
    /// ```text
    /// [Unit]
    /// Description=My Service
    /// Requires=network.service
    /// After=network.service
    ///
    /// [Service]
    /// Type=simple
    /// ExecStart=/usr/bin/myservice
    /// Restart=on-failure
    ///
    /// [Install]
    /// WantedBy=multi-user.target
    /// ```
    pub fn parse(contents: &str) -> Result<Self, ParseError> {
        let mut unit = UnitFile {
            name: String::new(),
            description: String::new(),
            dependencies: Vec::new(),
            wants: Vec::new(),
            after: Vec::new(),
            before: Vec::new(),
            exec_start: String::new(),
            exec_stop: String::new(),
            service_type: ServiceType::Simple,
            restart_policy: RestartPolicy::No,
            restart_sec: 0,
            timeout_start_sec: 90,
            timeout_stop_sec: 90,
            wanted_by: Vec::new(),
            watchdog_sec: 0,
            memory_max: 0,
            cpu_weight: 100,
        };

        let mut current_section = SectionKind::None;

        for line in contents.lines() {
            let trimmed = line.trim();

            // Skip empty lines and comments
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
                continue;
            }

            // Section header
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                let section_name = &trimmed[1..trimmed.len() - 1];
                current_section = if hash_eq(section_name, "Unit") {
                    SectionKind::Unit
                } else if hash_eq(section_name, "Service") {
                    SectionKind::Service
                } else if hash_eq(section_name, "Install") {
                    SectionKind::Install
                } else {
                    return Err(ParseError::InvalidSection);
                };
                continue;
            }

            // Key=Value parsing
            let eq_pos = match find_char(trimmed, '=') {
                Some(p) => p,
                None => return Err(ParseError::SyntaxError),
            };

            let key = trimmed[..eq_pos].trim();
            let value = trimmed[eq_pos + 1..].trim();

            match current_section {
                SectionKind::Unit => parse_unit_key(&mut unit, key, value),
                SectionKind::Service => parse_service_key(&mut unit, key, value),
                SectionKind::Install => parse_install_key(&mut unit, key, value),
                SectionKind::None => return Err(ParseError::SyntaxError),
            }
        }

        Ok(unit)
    }

    /// Return the list of hard dependencies (Requires=).
    pub fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    /// Return the list of soft dependencies (Wants=).
    pub fn wants(&self) -> &[String] {
        &self.wants
    }
}

// ── Internal parsing ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum SectionKind {
    None,
    Unit,
    Service,
    Install,
}

fn find_char(s: &str, ch: char) -> Option<usize> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == ch as u8 {
            return Some(i);
        }
    }
    None
}

/// Split a space-or-comma separated value list into individual strings.
fn split_list(value: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut start = 0;
    let bytes = value.as_bytes();
    let len = bytes.len();

    while start < len {
        // Skip whitespace and commas
        while start < len && (bytes[start] == b' ' || bytes[start] == b',' || bytes[start] == b'\t') {
            start += 1;
        }
        if start >= len {
            break;
        }
        let mut end = start;
        while end < len && bytes[end] != b' ' && bytes[end] != b',' && bytes[end] != b'\t' {
            end += 1;
        }
        if end > start {
            result.push(String::from(&value[start..end]));
        }
        start = end;
    }

    result
}

fn parse_u64(s: &str) -> u64 {
    let trimmed = s.trim();
    let mut val: u64 = 0;
    for &b in trimmed.as_bytes() {
        if b >= b'0' && b <= b'9' {
            val = val.wrapping_mul(10).wrapping_add((b - b'0') as u64);
        } else {
            break;
        }
    }
    val
}

fn parse_u32(s: &str) -> u32 {
    parse_u64(s) as u32
}

fn parse_unit_key(unit: &mut UnitFile, key: &str, value: &str) {
    if hash_eq(key, "Description") {
        unit.description = String::from(value);
    } else if hash_eq(key, "Requires") {
        unit.dependencies.extend(split_list(value));
    } else if hash_eq(key, "Wants") {
        unit.wants.extend(split_list(value));
    } else if hash_eq(key, "After") {
        unit.after.extend(split_list(value));
    } else if hash_eq(key, "Before") {
        unit.before.extend(split_list(value));
    }
}

fn parse_service_key(unit: &mut UnitFile, key: &str, value: &str) {
    if hash_eq(key, "Type") {
        unit.service_type = ServiceType::from_str(value);
    } else if hash_eq(key, "ExecStart") {
        unit.exec_start = String::from(value);
    } else if hash_eq(key, "ExecStop") {
        unit.exec_stop = String::from(value);
    } else if hash_eq(key, "Restart") {
        unit.restart_policy = RestartPolicy::from_str(value);
    } else if hash_eq(key, "RestartSec") {
        unit.restart_sec = parse_u64(value);
    } else if hash_eq(key, "TimeoutStartSec") {
        unit.timeout_start_sec = parse_u64(value);
    } else if hash_eq(key, "TimeoutStopSec") {
        unit.timeout_stop_sec = parse_u64(value);
    } else if hash_eq(key, "WatchdogSec") {
        unit.watchdog_sec = parse_u64(value);
    } else if hash_eq(key, "MemoryMax") {
        unit.memory_max = parse_u64(value);
    } else if hash_eq(key, "CPUWeight") {
        unit.cpu_weight = parse_u32(value);
    }
}

fn parse_install_key(unit: &mut UnitFile, key: &str, value: &str) {
    if hash_eq(key, "WantedBy") {
        unit.wanted_by.extend(split_list(value));
    }
}

// ── Global state ───────────────────────────────────────────────────────────

struct UnitFileRegistry {
    files: Vec<UnitFile>,
}

impl UnitFileRegistry {
    fn new() -> Self {
        UnitFileRegistry { files: Vec::new() }
    }
}

static REGISTRY: Mutex<Option<UnitFileRegistry>> = Mutex::new(None);

/// Initialize the unit file parser subsystem.
pub fn init() {
    let mut guard = REGISTRY.lock();
    *guard = Some(UnitFileRegistry::new());
    serial_println!("[init_system::unit_file] unit file parser initialized");
}

/// Parse and register a unit file.
pub fn load_unit(name: &str, contents: &str) -> Result<(), ParseError> {
    let mut unit = UnitFile::parse(contents)?;
    unit.name = String::from(name);

    let mut guard = REGISTRY.lock();
    let reg = guard.as_mut().expect("unit_file registry not initialized");
    reg.files.push(unit);
    serial_println!("[init_system::unit_file] loaded unit: {}", name);
    Ok(())
}

/// Get the number of loaded unit files.
pub fn loaded_count() -> usize {
    let guard = REGISTRY.lock();
    let reg = guard.as_ref().expect("unit_file registry not initialized");
    reg.files.len()
}
