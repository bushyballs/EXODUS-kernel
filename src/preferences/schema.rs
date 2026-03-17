use super::store::PrefValue;
use crate::sync::Mutex;
/// Preference schemas for Genesis
///
/// Defines the structure, types, constraints, and defaults for every
/// preference key. Schemas enforce:
///   - Type safety (int/q16/string/bool/list must match)
///   - Range validation (min/max for numeric values)
///   - Allowed-value sets (enum-like string constraints)
///   - Default values (applied when no user override exists)
///   - Human-readable descriptions for settings UI
///
/// Schemas are registered per-namespace and validated on every write.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The expected type of a preference
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefType {
    Int,
    Q16,
    Str,
    Bool,
    List,
}

impl PrefType {
    /// Return a display name for the type
    pub fn name(&self) -> &'static str {
        match self {
            PrefType::Int => "int",
            PrefType::Q16 => "q16",
            PrefType::Str => "string",
            PrefType::Bool => "bool",
            PrefType::List => "list",
        }
    }

    /// Check if a PrefValue matches this expected type
    pub fn matches(&self, value: &PrefValue) -> bool {
        matches!(
            (self, value),
            (PrefType::Int, PrefValue::Int(_))
                | (PrefType::Q16, PrefValue::Q16(_))
                | (PrefType::Str, PrefValue::Str(_))
                | (PrefType::Bool, PrefValue::Bool(_))
                | (PrefType::List, PrefValue::List(_))
        )
    }
}

/// Numeric range constraint (applies to Int and Q16 values)
#[derive(Clone, Copy, Debug)]
pub struct NumericRange {
    pub min: i32,
    pub max: i32,
}

impl NumericRange {
    pub fn new(min: i32, max: i32) -> Self {
        Self { min, max }
    }

    /// Check if a value falls within the range (inclusive)
    pub fn contains(&self, value: i32) -> bool {
        value >= self.min && value <= self.max
    }
}

/// A set of allowed string values (enum-like constraint)
#[derive(Clone, Debug)]
pub struct AllowedValues {
    pub values: Vec<String>,
}

impl AllowedValues {
    pub fn new(values: Vec<String>) -> Self {
        Self { values }
    }

    pub fn from_strs(strs: &[&str]) -> Self {
        Self {
            values: strs.iter().map(|s| String::from(*s)).collect(),
        }
    }

    pub fn contains(&self, value: &str) -> bool {
        self.values.iter().any(|v| v.as_str() == value)
    }
}

/// Schema definition for a single preference key
#[derive(Clone, Debug)]
pub struct PrefSchema {
    /// Fully-qualified key: "namespace.key"
    pub key: String,
    /// Expected value type
    pub pref_type: PrefType,
    /// Default value (must match pref_type)
    pub default: PrefValue,
    /// Human-readable description
    pub description: String,
    /// Numeric range constraint (for Int and Q16 types)
    pub range: Option<NumericRange>,
    /// Allowed string values (for Str type)
    pub allowed: Option<AllowedValues>,
    /// Maximum list length (for List type)
    pub max_list_len: Option<u32>,
    /// Whether this preference requires a restart to take effect
    pub requires_restart: bool,
    /// Whether this preference is hidden from normal settings UI
    pub hidden: bool,
    /// Category tag for grouping in settings UI
    pub category: String,
}

impl PrefSchema {
    /// Create a new schema for an integer preference
    pub fn int(key: &str, default: i32, description: &str) -> Self {
        Self {
            key: String::from(key),
            pref_type: PrefType::Int,
            default: PrefValue::Int(default),
            description: String::from(description),
            range: None,
            allowed: None,
            max_list_len: None,
            requires_restart: false,
            hidden: false,
            category: String::from("general"),
        }
    }

    /// Create a new schema for a Q16 fixed-point preference
    pub fn q16(key: &str, default: i32, description: &str) -> Self {
        Self {
            key: String::from(key),
            pref_type: PrefType::Q16,
            default: PrefValue::Q16(default),
            description: String::from(description),
            range: None,
            allowed: None,
            max_list_len: None,
            requires_restart: false,
            hidden: false,
            category: String::from("general"),
        }
    }

    /// Create a new schema for a string preference
    pub fn string(key: &str, default: &str, description: &str) -> Self {
        Self {
            key: String::from(key),
            pref_type: PrefType::Str,
            default: PrefValue::Str(String::from(default)),
            description: String::from(description),
            range: None,
            allowed: None,
            max_list_len: None,
            requires_restart: false,
            hidden: false,
            category: String::from("general"),
        }
    }

    /// Create a new schema for a boolean preference
    pub fn boolean(key: &str, default: bool, description: &str) -> Self {
        Self {
            key: String::from(key),
            pref_type: PrefType::Bool,
            default: PrefValue::Bool(default),
            description: String::from(description),
            range: None,
            allowed: None,
            max_list_len: None,
            requires_restart: false,
            hidden: false,
            category: String::from("general"),
        }
    }

    /// Create a new schema for a list preference
    pub fn list(key: &str, default: Vec<String>, description: &str) -> Self {
        Self {
            key: String::from(key),
            pref_type: PrefType::List,
            default: PrefValue::List(default),
            description: String::from(description),
            range: None,
            allowed: None,
            max_list_len: None,
            requires_restart: false,
            hidden: false,
            category: String::from("general"),
        }
    }

    /// Builder: set numeric range constraint
    pub fn with_range(mut self, min: i32, max: i32) -> Self {
        self.range = Some(NumericRange::new(min, max));
        self
    }

    /// Builder: set allowed string values
    pub fn with_allowed(mut self, values: &[&str]) -> Self {
        self.allowed = Some(AllowedValues::from_strs(values));
        self
    }

    /// Builder: set max list length
    pub fn with_max_list_len(mut self, max: u32) -> Self {
        self.max_list_len = Some(max);
        self
    }

    /// Builder: mark as requiring restart
    pub fn restart_required(mut self) -> Self {
        self.requires_restart = true;
        self
    }

    /// Builder: mark as hidden
    pub fn hide(mut self) -> Self {
        self.hidden = true;
        self
    }

    /// Builder: set category
    pub fn with_category(mut self, cat: &str) -> Self {
        self.category = String::from(cat);
        self
    }

    /// Validate a value against this schema. Returns Ok(()) or an error string.
    pub fn validate(&self, value: &PrefValue) -> Result<(), String> {
        // Type check
        if !self.pref_type.matches(value) {
            return Err(format!(
                "Type mismatch for '{}': expected {}, got {}",
                self.key,
                self.pref_type.name(),
                value.type_name()
            ));
        }

        // Range check for Int
        if let (Some(range), PrefValue::Int(v)) = (&self.range, value) {
            if !range.contains(*v) {
                return Err(format!(
                    "Value {} out of range [{}, {}] for '{}'",
                    v, range.min, range.max, self.key
                ));
            }
        }

        // Range check for Q16
        if let (Some(range), PrefValue::Q16(v)) = (&self.range, value) {
            if !range.contains(*v) {
                return Err(format!(
                    "Q16 value {} out of range [{}, {}] for '{}'",
                    v, range.min, range.max, self.key
                ));
            }
        }

        // Allowed values check for Str
        if let (Some(allowed), PrefValue::Str(s)) = (&self.allowed, value) {
            if !allowed.contains(s.as_str()) {
                return Err(format!(
                    "Value '{}' not in allowed set for '{}'",
                    s, self.key
                ));
            }
        }

        // Max list length check
        if let (Some(max_len), PrefValue::List(items)) = (self.max_list_len, value) {
            if items.len() > max_len as usize {
                return Err(format!(
                    "List length {} exceeds maximum {} for '{}'",
                    items.len(),
                    max_len,
                    self.key
                ));
            }
        }

        Ok(())
    }
}

/// Schema registry — stores all registered schemas
pub struct SchemaRegistry {
    schemas: BTreeMap<String, PrefSchema>,
    total_registered: u32,
    total_validations: u64,
    total_failures: u64,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        Self {
            schemas: BTreeMap::new(),
            total_registered: 0,
            total_validations: 0,
            total_failures: 0,
        }
    }

    /// Register a new schema
    pub fn register(&mut self, schema: PrefSchema) -> bool {
        if self.schemas.contains_key(&schema.key) {
            serial_println!("[SCHEMA] Schema already registered for '{}'", schema.key);
            return false;
        }
        let key = schema.key.clone();
        self.schemas.insert(key.clone(), schema);
        self.total_registered = self.total_registered.saturating_add(1);
        true
    }

    /// Register multiple schemas at once
    pub fn register_batch(&mut self, schemas: Vec<PrefSchema>) -> u32 {
        let mut count = 0u32;
        for schema in schemas {
            if self.register(schema) {
                count += 1;
            }
        }
        serial_println!("[SCHEMA] Batch registered {} schemas", count);
        count
    }

    /// Get a schema by key
    pub fn get(&self, key: &str) -> Option<&PrefSchema> {
        self.schemas.get(key)
    }

    /// Validate a value against its registered schema
    pub fn validate(&mut self, key: &str, value: &PrefValue) -> Result<(), String> {
        self.total_validations = self.total_validations.saturating_add(1);
        match self.schemas.get(key) {
            Some(schema) => {
                let result = schema.validate(value);
                if result.is_err() {
                    self.total_failures = self.total_failures.saturating_add(1);
                }
                result
            }
            None => {
                // No schema registered — allow by default (unschemed prefs)
                Ok(())
            }
        }
    }

    /// Get the default value for a key
    pub fn default_value(&self, key: &str) -> Option<PrefValue> {
        self.schemas.get(key).map(|s| s.default.clone())
    }

    /// List all schemas in a given namespace
    pub fn schemas_in_namespace(&self, namespace: &str) -> Vec<&PrefSchema> {
        let prefix = format!("{}.", namespace);
        self.schemas
            .values()
            .filter(|s| s.key.starts_with(prefix.as_str()))
            .collect()
    }

    /// List all schemas in a given category
    pub fn schemas_in_category(&self, category: &str) -> Vec<&PrefSchema> {
        self.schemas
            .values()
            .filter(|s| s.category.as_str() == category)
            .collect()
    }

    /// List all visible (non-hidden) schemas
    pub fn visible_schemas(&self) -> Vec<&PrefSchema> {
        self.schemas.values().filter(|s| !s.hidden).collect()
    }

    /// Get total count of registered schemas
    pub fn count(&self) -> usize {
        self.schemas.len()
    }

    /// Get validation statistics: (total_validations, total_failures)
    pub fn stats(&self) -> (u64, u64) {
        (self.total_validations, self.total_failures)
    }

    /// Get all schemas that require restart
    pub fn restart_required_schemas(&self) -> Vec<&PrefSchema> {
        self.schemas
            .values()
            .filter(|s| s.requires_restart)
            .collect()
    }

    /// Produce a list of (key, description, type_name, default_serialized)
    pub fn describe_all(&self) -> Vec<(String, String, String, String)> {
        self.schemas
            .values()
            .map(|s| {
                (
                    s.key.clone(),
                    s.description.clone(),
                    String::from(s.pref_type.name()),
                    s.default.serialize(),
                )
            })
            .collect()
    }
}

static REGISTRY: Mutex<Option<SchemaRegistry>> = Mutex::new(None);

/// Initialize the schema registry
pub fn init() {
    let mut lock = REGISTRY.lock();
    *lock = Some(SchemaRegistry::new());
    serial_println!("[SCHEMA] Schema registry initialized");
}

/// Get a reference to the global schema registry
pub fn get_registry() -> &'static Mutex<Option<SchemaRegistry>> {
    &REGISTRY
}
