use crate::sync::Mutex;
/// Preferences key-value store for Genesis
///
/// Provides typed preference storage with namespaces. Each preference
/// has a fully-qualified key of the form "namespace.key" and a typed
/// value (integer, string, boolean, or list). Namespaces allow
/// logical grouping (e.g. "display.brightness", "audio.volume").
///
/// Storage is backed by a BTreeMap for ordered iteration and
/// predictable serialization. All integer values use Q16 fixed-point
/// (i32 with 16 fractional bits) to avoid floating-point math.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Q16 fixed-point constant: 1 << 16
const Q16_ONE: i32 = 65536;

/// Typed preference value — no floats allowed
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrefValue {
    /// Signed integer (plain i32, not Q16)
    Int(i32),
    /// Q16 fixed-point (16 fractional bits, for sub-integer precision)
    Q16(i32),
    /// UTF-8 string
    Str(String),
    /// Boolean flag
    Bool(bool),
    /// Ordered list of strings
    List(Vec<String>),
}

impl PrefValue {
    /// Return the type name as a static string
    pub fn type_name(&self) -> &'static str {
        match self {
            PrefValue::Int(_) => "int",
            PrefValue::Q16(_) => "q16",
            PrefValue::Str(_) => "string",
            PrefValue::Bool(_) => "bool",
            PrefValue::List(_) => "list",
        }
    }

    /// Try to extract as i32
    pub fn as_int(&self) -> Option<i32> {
        match self {
            PrefValue::Int(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to extract as Q16 fixed-point
    pub fn as_q16(&self) -> Option<i32> {
        match self {
            PrefValue::Q16(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to extract as string reference
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PrefValue::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Try to extract as bool
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            PrefValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Try to extract as list reference
    pub fn as_list(&self) -> Option<&Vec<String>> {
        match self {
            PrefValue::List(l) => Some(l),
            _ => None,
        }
    }

    /// Serialize value to a string representation for export
    pub fn serialize(&self) -> String {
        match self {
            PrefValue::Int(v) => format!("int:{}", v),
            PrefValue::Q16(v) => format!("q16:{}", v),
            PrefValue::Str(s) => format!("str:{}", s),
            PrefValue::Bool(b) => format!("bool:{}", if *b { "true" } else { "false" }),
            PrefValue::List(items) => {
                let mut out = String::from("list:");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(item.as_str());
                }
                out
            }
        }
    }

    /// Deserialize a value from its string representation
    pub fn deserialize(input: &str) -> Option<PrefValue> {
        if let Some(rest) = input.strip_prefix("int:") {
            parse_i32(rest).map(PrefValue::Int)
        } else if let Some(rest) = input.strip_prefix("q16:") {
            parse_i32(rest).map(PrefValue::Q16)
        } else if let Some(rest) = input.strip_prefix("str:") {
            Some(PrefValue::Str(String::from(rest)))
        } else if let Some(rest) = input.strip_prefix("bool:") {
            match rest {
                "true" => Some(PrefValue::Bool(true)),
                "false" => Some(PrefValue::Bool(false)),
                _ => None,
            }
        } else if let Some(rest) = input.strip_prefix("list:") {
            if rest.is_empty() {
                Some(PrefValue::List(vec![]))
            } else {
                let items: Vec<String> = rest.split(',').map(|s| String::from(s.trim())).collect();
                Some(PrefValue::List(items))
            }
        } else {
            None
        }
    }
}

/// Parse an i32 from a decimal string (no stdlib dependency)
fn parse_i32(s: &str) -> Option<i32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (negative, digits) = if s.as_bytes()[0] == b'-' {
        (true, &s[1..])
    } else {
        (false, s)
    };
    let mut result: i64 = 0;
    for &b in digits.as_bytes() {
        if b < b'0' || b > b'9' {
            return None;
        }
        result = result * 10 + ((b - b'0') as i64);
        if result > 0x7FFF_FFFF_i64 + if negative { 1 } else { 0 } {
            return None; // overflow
        }
    }
    if negative {
        Some(-(result as i32))
    } else {
        Some(result as i32)
    }
}

/// A single stored preference entry
#[derive(Clone, Debug)]
pub struct PrefEntry {
    /// Fully-qualified key: "namespace.key"
    pub key: String,
    /// The typed value
    pub value: PrefValue,
    /// Whether this was explicitly set by the user (vs default)
    pub user_set: bool,
    /// Timestamp (monotonic tick count) when last modified
    pub modified_at: u64,
}

/// Namespace metadata
#[derive(Clone, Debug)]
pub struct Namespace {
    pub name: String,
    pub description: String,
    pub key_count: u32,
    pub read_only: bool,
}

/// The preference store — holds all preference entries and namespace metadata
pub struct PreferenceStore {
    /// All preferences indexed by fully-qualified key
    entries: BTreeMap<String, PrefEntry>,
    /// Registered namespaces
    namespaces: BTreeMap<String, Namespace>,
    /// Monotonic counter for modification timestamps
    tick: u64,
    /// Total number of reads since init
    total_reads: u64,
    /// Total number of writes since init
    total_writes: u64,
}

impl PreferenceStore {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            namespaces: BTreeMap::new(),
            tick: 1,
            total_reads: 0,
            total_writes: 0,
        }
    }

    /// Register a new namespace
    pub fn register_namespace(&mut self, name: &str, description: &str, read_only: bool) -> bool {
        if self.namespaces.contains_key(name) {
            serial_println!("[PREFS] Namespace '{}' already registered", name);
            return false;
        }
        self.namespaces.insert(
            String::from(name),
            Namespace {
                name: String::from(name),
                description: String::from(description),
                key_count: 0,
                read_only,
            },
        );
        serial_println!("[PREFS] Registered namespace '{}'", name);
        true
    }

    /// Extract the namespace portion from a fully-qualified key
    fn namespace_of(key: &str) -> Option<&str> {
        key.find('.').map(|pos| &key[..pos])
    }

    /// Check if a key's namespace is read-only
    fn is_read_only(&self, key: &str) -> bool {
        if let Some(ns_name) = Self::namespace_of(key) {
            if let Some(ns) = self.namespaces.get(ns_name) {
                return ns.read_only;
            }
        }
        false
    }

    /// Set a preference value. Returns the previous value if any.
    pub fn set(&mut self, key: &str, value: PrefValue, user_set: bool) -> Option<PrefValue> {
        if self.is_read_only(key) {
            serial_println!("[PREFS] Cannot write to read-only namespace: {}", key);
            return None;
        }

        let old = self.entries.get(key).map(|e| e.value.clone());

        let tick = self.tick;
        self.tick = self.tick.saturating_add(1);
        self.total_writes = self.total_writes.saturating_add(1);

        // Update namespace key count for new entries
        if !self.entries.contains_key(key) {
            if let Some(ns_name) = Self::namespace_of(key) {
                if let Some(ns) = self.namespaces.get_mut(ns_name) {
                    ns.key_count = ns.key_count.saturating_add(1);
                }
            }
        }

        self.entries.insert(
            String::from(key),
            PrefEntry {
                key: String::from(key),
                value,
                user_set,
                modified_at: tick,
            },
        );

        old
    }

    /// Get a preference value by key
    pub fn get(&mut self, key: &str) -> Option<&PrefValue> {
        self.total_reads = self.total_reads.saturating_add(1);
        self.entries.get(key).map(|e| &e.value)
    }

    /// Get a preference value without incrementing read counter
    pub fn peek(&self, key: &str) -> Option<&PrefValue> {
        self.entries.get(key).map(|e| &e.value)
    }

    /// Get the full entry (including metadata)
    pub fn get_entry(&self, key: &str) -> Option<&PrefEntry> {
        self.entries.get(key)
    }

    /// Remove a preference, returning it if it existed
    pub fn remove(&mut self, key: &str) -> Option<PrefEntry> {
        if self.is_read_only(key) {
            serial_println!("[PREFS] Cannot remove from read-only namespace: {}", key);
            return None;
        }
        let removed = self.entries.remove(key);
        if removed.is_some() {
            if let Some(ns_name) = Self::namespace_of(key) {
                if let Some(ns) = self.namespaces.get_mut(ns_name) {
                    if ns.key_count > 0 {
                        ns.key_count -= 1;
                    }
                }
            }
        }
        removed
    }

    /// List all keys in a given namespace
    pub fn keys_in_namespace(&self, namespace: &str) -> Vec<String> {
        let prefix = format!("{}.", namespace);
        self.entries
            .keys()
            .filter(|k| k.starts_with(prefix.as_str()))
            .cloned()
            .collect()
    }

    /// List all registered namespaces
    pub fn list_namespaces(&self) -> Vec<&Namespace> {
        self.namespaces.values().collect()
    }

    /// Get the total number of stored preferences
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Check whether a key exists
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Get all entries modified after a given tick
    pub fn modified_since(&self, since_tick: u64) -> Vec<&PrefEntry> {
        self.entries
            .values()
            .filter(|e| e.modified_at > since_tick)
            .collect()
    }

    /// Get all user-set preferences (excludes defaults)
    pub fn user_overrides(&self) -> Vec<&PrefEntry> {
        self.entries.values().filter(|e| e.user_set).collect()
    }

    /// Clear all user-set preferences in a namespace, reverting to defaults
    pub fn reset_namespace(&mut self, namespace: &str) -> u32 {
        if self.is_read_only(namespace) {
            return 0;
        }
        let prefix = format!("{}.", namespace);
        let keys_to_remove: Vec<String> = self
            .entries
            .iter()
            .filter(|(k, e)| k.starts_with(prefix.as_str()) && e.user_set)
            .map(|(k, _)| k.clone())
            .collect();
        let count = keys_to_remove.len() as u32;
        for key in keys_to_remove {
            self.entries.remove(&key);
        }
        if let Some(ns) = self.namespaces.get_mut(namespace) {
            // Recount remaining keys
            let remaining = self
                .entries
                .keys()
                .filter(|k| k.starts_with(prefix.as_str()))
                .count() as u32;
            ns.key_count = remaining;
        }
        serial_println!(
            "[PREFS] Reset {} user prefs in namespace '{}'",
            count,
            namespace
        );
        count
    }

    /// Export all entries as serialized key=value lines
    pub fn export_all(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .map(|(k, e)| (k.clone(), e.value.serialize()))
            .collect()
    }

    /// Import entries from serialized key=value pairs
    pub fn import_all(&mut self, pairs: &[(String, String)]) -> u32 {
        let mut imported = 0u32;
        for (key, serialized) in pairs {
            if let Some(value) = PrefValue::deserialize(serialized.as_str()) {
                self.set(key.as_str(), value, true);
                imported += 1;
            } else {
                serial_println!("[PREFS] Skipping invalid import entry: {}", key);
            }
        }
        serial_println!("[PREFS] Imported {} preferences", imported);
        imported
    }

    /// Return read/write statistics: (total_reads, total_writes, entry_count)
    pub fn stats(&self) -> (u64, u64, usize) {
        (self.total_reads, self.total_writes, self.entries.len())
    }

    /// Get the current monotonic tick
    pub fn current_tick(&self) -> u64 {
        self.tick
    }
}

/// Convert an integer to Q16 fixed-point
pub fn int_to_q16(n: i32) -> i32 {
    n.wrapping_mul(Q16_ONE)
}

/// Convert Q16 fixed-point to integer (truncates fractional part)
pub fn q16_to_int(q: i32) -> i32 {
    q >> 16
}

/// Multiply two Q16 values: (a * b) >> 16
pub fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> 16) as i32
}

/// Divide two Q16 values: (a << 16) / b
pub fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as i32
}

static STORE: Mutex<Option<PreferenceStore>> = Mutex::new(None);

/// Initialize the preference store
pub fn init() {
    let mut lock = STORE.lock();
    *lock = Some(PreferenceStore::new());
    serial_println!("[PREFS] Preference store initialized");
}

/// Get a reference to the global preference store
pub fn get_store() -> &'static Mutex<Option<PreferenceStore>> {
    &STORE
}
