use crate::sync::Mutex;
/// App manifest parsing
///
/// Part of the Genesis app framework. Reads and validates
/// application manifests that declare metadata and permissions.
/// Manifest format is a simple key-value text format.
use alloc::string::String;
use alloc::vec::Vec;

/// Known permission identifiers
const KNOWN_PERMISSIONS: &[&str] = &[
    "network",
    "filesystem",
    "camera",
    "microphone",
    "location",
    "notifications",
    "clipboard",
    "ipc",
    "background",
    "sensors",
    "usb",
    "bluetooth",
    "display",
    "storage",
];

/// Parsed application manifest
pub struct AppManifest {
    pub name: String,
    pub version: String,
    pub permissions: Vec<String>,
    pub entry_point: String,
}

/// Manifest parsing errors
#[derive(Debug)]
pub enum ManifestError {
    EmptyData,
    MissingField(&'static str),
    InvalidVersion,
    UnknownPermission,
    ParseError,
}

impl AppManifest {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            version: String::new(),
            permissions: Vec::new(),
            entry_point: String::new(),
        }
    }

    /// Parse a manifest from raw bytes.
    /// Expected format (simple key=value, one per line):
    /// ```
    /// name=MyApp
    /// version=1.0.0
    /// entry=main
    /// permissions=network,filesystem
    /// ```
    pub fn parse(data: &[u8]) -> Result<Self, ()> {
        if data.is_empty() {
            crate::serial_println!("[app::manifest] error: empty manifest data");
            return Err(());
        }

        let mut manifest = AppManifest::new();
        let mut found_name = false;
        let mut found_version = false;
        let mut found_entry = false;

        // Parse as UTF-8 text lines
        let text = match core::str::from_utf8(data) {
            Ok(s) => s,
            Err(_) => {
                crate::serial_println!("[app::manifest] error: invalid UTF-8");
                return Err(());
            }
        };

        // Split by newlines and process each line
        let mut line_start = 0;
        let bytes = text.as_bytes();
        let len = bytes.len();

        loop {
            // Find end of line
            let mut line_end = line_start;
            while line_end < len && bytes[line_end] != b'\n' && bytes[line_end] != b'\r' {
                line_end += 1;
            }

            if line_start < line_end {
                let line = &text[line_start..line_end];
                let trimmed = trim_str(line);

                // Skip empty lines and comments
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    // Find '=' separator
                    if let Some(eq_pos) = find_char(trimmed, '=') {
                        let key = trim_str(&trimmed[..eq_pos]);
                        let value = trim_str(&trimmed[eq_pos + 1..]);

                        match key {
                            "name" => {
                                manifest.name = str_to_string(value);
                                found_name = true;
                            }
                            "version" => {
                                manifest.version = str_to_string(value);
                                found_version = true;
                            }
                            "entry" | "entry_point" => {
                                manifest.entry_point = str_to_string(value);
                                found_entry = true;
                            }
                            "permissions" | "perms" => {
                                // Parse comma-separated permissions
                                let perms = split_csv(value);
                                for perm in perms {
                                    let p = trim_str(&perm);
                                    if !p.is_empty() {
                                        manifest.permissions.push(str_to_string(p));
                                    }
                                }
                            }
                            _ => {
                                crate::serial_println!(
                                    "[app::manifest] ignoring unknown key: '{}'",
                                    key
                                );
                            }
                        }
                    }
                }
            }

            // Skip past newline characters
            if line_end >= len {
                break;
            }
            line_start = line_end + 1;
            // Skip \r\n pairs
            if line_start < len && bytes[line_start - 1] == b'\r' && bytes[line_start] == b'\n' {
                line_start += 1;
            }
            if line_start >= len {
                break;
            }
        }

        if !found_name {
            crate::serial_println!("[app::manifest] error: missing 'name' field");
            return Err(());
        }
        if !found_version {
            crate::serial_println!("[app::manifest] error: missing 'version' field");
            return Err(());
        }
        if !found_entry {
            crate::serial_println!("[app::manifest] error: missing 'entry_point' field");
            return Err(());
        }

        crate::serial_println!(
            "[app::manifest] parsed: name='{}', version='{}', entry='{}', {} permissions",
            manifest.name,
            manifest.version,
            manifest.entry_point,
            manifest.permissions.len()
        );
        Ok(manifest)
    }

    /// Validate that all required fields are present and permissions are known
    pub fn validate(&self) -> bool {
        // Check required fields
        if self.name.is_empty() {
            crate::serial_println!("[app::manifest] validation failed: empty name");
            return false;
        }
        if self.version.is_empty() {
            crate::serial_println!("[app::manifest] validation failed: empty version");
            return false;
        }
        if self.entry_point.is_empty() {
            crate::serial_println!("[app::manifest] validation failed: empty entry point");
            return false;
        }

        // Validate name (alphanumeric, underscores, hyphens, dots)
        for c in self.name.chars() {
            if !c.is_alphanumeric() && c != '_' && c != '-' && c != '.' && c != ' ' {
                crate::serial_println!(
                    "[app::manifest] validation failed: invalid char '{}' in name",
                    c
                );
                return false;
            }
        }

        // Validate version format (simple check: contains at least one digit)
        let has_digit = self.version.chars().any(|c| c.is_ascii_digit());
        if !has_digit {
            crate::serial_println!("[app::manifest] validation failed: version has no digits");
            return false;
        }

        // Validate permissions against known list
        for perm in &self.permissions {
            let mut known = false;
            for kp in KNOWN_PERMISSIONS {
                if perm.as_str() == *kp {
                    known = true;
                    break;
                }
            }
            if !known {
                crate::serial_println!(
                    "[app::manifest] validation warning: unknown permission '{}'",
                    perm
                );
                // Non-fatal: unknown permissions are just logged
            }
        }

        // Check for duplicate permissions
        for i in 0..self.permissions.len() {
            for j in (i + 1)..self.permissions.len() {
                if self.permissions[i] == self.permissions[j] {
                    crate::serial_println!(
                        "[app::manifest] validation warning: duplicate permission '{}'",
                        self.permissions[i]
                    );
                }
            }
        }

        crate::serial_println!("[app::manifest] validation passed for '{}'", self.name);
        true
    }

    /// Check if the manifest declares a specific permission
    pub fn has_permission(&self, perm: &str) -> bool {
        for p in &self.permissions {
            if p.as_str() == perm {
                return true;
            }
        }
        false
    }

    /// Parse a semantic version string into (major, minor, patch)
    pub fn parse_version(&self) -> Option<(u32, u32, u32)> {
        let parts = split_csv_with_sep(&self.version, '.');
        if parts.len() < 2 {
            return None;
        }
        let major = parse_u32(&parts[0])?;
        let minor = parse_u32(&parts[1])?;
        let patch = if parts.len() > 2 {
            parse_u32(&parts[2]).unwrap_or(0)
        } else {
            0
        };
        Some((major, minor, patch))
    }
}

// String utility functions

fn str_to_string(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        result.push(c);
    }
    result
}

fn trim_str(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut start = 0;
    while start < bytes.len() && (bytes[start] == b' ' || bytes[start] == b'\t') {
        start += 1;
    }
    let mut end = bytes.len();
    while end > start && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t') {
        end -= 1;
    }
    &s[start..end]
}

fn find_char(s: &str, c: char) -> Option<usize> {
    for (i, ch) in s.chars().enumerate() {
        if ch == c {
            return Some(i);
        }
    }
    None
}

fn split_csv(s: &str) -> Vec<String> {
    split_csv_with_sep(s, ',')
}

fn split_csv_with_sep(s: &str, sep: char) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    for c in s.chars() {
        if c == sep {
            result.push(current);
            current = String::new();
        } else {
            current.push(c);
        }
    }
    result.push(current);
    result
}

fn parse_u32(s: &str) -> Option<u32> {
    let trimmed = trim_str(s);
    if trimmed.is_empty() {
        return None;
    }
    let mut result: u32 = 0;
    for c in trimmed.chars() {
        if !c.is_ascii_digit() {
            return None;
        }
        result = result.checked_mul(10)?.checked_add(c as u32 - '0' as u32)?;
    }
    Some(result)
}

static MANIFEST_REGISTRY: Mutex<Option<Vec<AppManifest>>> = Mutex::new(None);

pub fn init() {
    let mut reg = MANIFEST_REGISTRY.lock();
    *reg = Some(Vec::new());
    crate::serial_println!("[app::manifest] manifest subsystem initialized");
}
