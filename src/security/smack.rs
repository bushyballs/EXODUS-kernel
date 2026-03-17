/// Simplified Mandatory Access Control Kernel (SMACK) for Genesis
///
/// Label-based mandatory access control:
///   - Every subject (process) and object (file/socket/etc.) has a SMACK label
///   - Access decisions based on (subject_label, object_label, access_type) rules
///   - 7 predefined special labels with built-in rules
///   - Access types: read (r), write (w), execute (x), append (a), transmute (t), lock (l)
///   - Rule format: "subject_label object_label rwxatl"
///   - Default deny unless explicit rule exists or special label applies
///   - Supports onlycap (restrict CAP_MAC_ADMIN to specific labels)
///   - Full audit trail of all access decisions
///
/// Reference: Linux SMACK LSM (Documentation/admin-guide/LSM/Smack.rst).
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

static SMACK: Mutex<Option<SmackInner>> = Mutex::new(None);

/// Maximum rules
const MAX_RULES: usize = 4096;

/// Maximum labels
const MAX_LABELS: usize = 1024;

/// Maximum label length
const MAX_LABEL_LEN: usize = 255;

/// Access permission bits
pub const ACCESS_READ: u8 = 1 << 0; // r
pub const ACCESS_WRITE: u8 = 1 << 1; // w
pub const ACCESS_EXEC: u8 = 1 << 2; // x
pub const ACCESS_APPEND: u8 = 1 << 3; // a
pub const ACCESS_TRANSMUTE: u8 = 1 << 4; // t
pub const ACCESS_LOCK: u8 = 1 << 5; // l

/// All access types combined
pub const ACCESS_ALL: u8 =
    ACCESS_READ | ACCESS_WRITE | ACCESS_EXEC | ACCESS_APPEND | ACCESS_TRANSMUTE | ACCESS_LOCK;

/// Predefined special labels
pub const LABEL_FLOOR: &str = "_"; // Minimum privilege (floor)
pub const LABEL_STAR: &str = "*"; // Wildcard (any label can read)
pub const LABEL_HAT: &str = "^"; // Internet label
pub const LABEL_WEB: &str = "@"; // Web label (untrusted content)

/// SMACK label (short string tag)
#[derive(Clone, PartialEq, Eq)]
pub struct SmackLabel(pub String);

impl SmackLabel {
    pub fn new(s: &str) -> Self {
        let truncated = if s.len() > MAX_LABEL_LEN {
            &s[..MAX_LABEL_LEN]
        } else {
            s
        };
        SmackLabel(String::from(truncated))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Check if this is one of the special predefined labels
    fn is_special(&self) -> bool {
        matches!(self.0.as_str(), "_" | "*" | "^" | "@")
    }
}

impl core::fmt::Debug for SmackLabel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SmackLabel(\"{}\")", self.0)
    }
}

/// SMACK access rule
#[derive(Clone)]
pub struct SmackRule {
    pub subject: SmackLabel,
    pub object: SmackLabel,
    pub access: u8,
}

impl SmackRule {
    pub fn new(subject: &str, object: &str, access: u8) -> Self {
        SmackRule {
            subject: SmackLabel::new(subject),
            object: SmackLabel::new(object),
            access,
        }
    }
}

/// Parse an access string like "rwxa" into a bitmask
pub fn parse_access_string(s: &str) -> u8 {
    let mut access = 0u8;
    for ch in s.chars() {
        match ch {
            'r' | 'R' => access |= ACCESS_READ,
            'w' | 'W' => access |= ACCESS_WRITE,
            'x' | 'X' => access |= ACCESS_EXEC,
            'a' | 'A' => access |= ACCESS_APPEND,
            't' | 'T' => access |= ACCESS_TRANSMUTE,
            'l' | 'L' => access |= ACCESS_LOCK,
            '-' => {} // Placeholder, no permission
            _ => {}
        }
    }
    access
}

/// Format an access bitmask as a human-readable string
fn format_access(access: u8) -> String {
    let mut s = String::with_capacity(6);
    s.push(if access & ACCESS_READ != 0 { 'r' } else { '-' });
    s.push(if access & ACCESS_WRITE != 0 { 'w' } else { '-' });
    s.push(if access & ACCESS_EXEC != 0 { 'x' } else { '-' });
    s.push(if access & ACCESS_APPEND != 0 {
        'a'
    } else {
        '-'
    });
    s.push(if access & ACCESS_TRANSMUTE != 0 {
        't'
    } else {
        '-'
    });
    s.push(if access & ACCESS_LOCK != 0 { 'l' } else { '-' });
    s
}

/// Subject (process) SMACK state
struct SubjectEntry {
    pid: u32,
    label: SmackLabel,
}

/// Object label mapping
struct ObjectEntry {
    path: String,
    label: SmackLabel,
}

/// Audit log entry
struct SmackAuditEntry {
    subject: String,
    object: String,
    requested: u8,
    granted: bool,
    pid: u32,
}

/// Inner SMACK state
struct SmackInner {
    /// Access rules
    rules: Vec<SmackRule>,
    /// Process label assignments
    subjects: Vec<SubjectEntry>,
    /// Object (file/resource) label assignments
    objects: Vec<ObjectEntry>,
    /// Labels that can use CAP_MAC_ADMIN (empty = all)
    onlycap: Vec<SmackLabel>,
    /// Default label for new processes
    default_label: SmackLabel,
    /// Audit log
    audit_log: Vec<SmackAuditEntry>,
    max_audit_entries: usize,
    /// Statistics
    checks_performed: u64,
    denials: u64,
    rules_added: u64,
}

impl SmackInner {
    fn new() -> Self {
        SmackInner {
            rules: Vec::with_capacity(64),
            subjects: Vec::new(),
            objects: Vec::new(),
            onlycap: Vec::new(),
            default_label: SmackLabel::new(LABEL_FLOOR),
            audit_log: Vec::with_capacity(256),
            max_audit_entries: 4096,
            checks_performed: 0,
            denials: 0,
            rules_added: 0,
        }
    }

    /// Load built-in rules for special labels
    fn load_builtin_rules(&mut self) {
        // Star label (*): everyone can read objects labeled *
        // This is implicit in check logic

        // Floor label (_): floor-labeled subjects have minimal access
        // Floor can read floor objects
        self.add_rule_inner(SmackRule::new(
            LABEL_FLOOR,
            LABEL_FLOOR,
            ACCESS_READ | ACCESS_EXEC,
        ));

        // Hat label (^): internet-facing processes
        // Hat can read star
        self.add_rule_inner(SmackRule::new(LABEL_HAT, LABEL_STAR, ACCESS_READ));

        // Web label (@): untrusted web content
        // Web can read star
        self.add_rule_inner(SmackRule::new(LABEL_WEB, LABEL_STAR, ACCESS_READ));

        serial_println!("    [smack] Built-in special label rules loaded");
    }

    /// Add an access rule
    fn add_rule_inner(&mut self, rule: SmackRule) {
        if self.rules.len() >= MAX_RULES {
            serial_println!("    [smack] WARNING: max rules reached");
            return;
        }

        // Check for existing rule with same subject+object and merge
        for existing in &mut self.rules {
            if existing.subject == rule.subject && existing.object == rule.object {
                existing.access |= rule.access;
                return;
            }
        }

        self.rules.push(rule);
        self.rules_added = self.rules_added.saturating_add(1);
    }

    /// Set a process label
    fn set_subject_label(&mut self, pid: u32, label: &str) {
        self.subjects.retain(|s| s.pid != pid);
        self.subjects.push(SubjectEntry {
            pid,
            label: SmackLabel::new(label),
        });
    }

    /// Get a process label
    fn get_subject_label(&self, pid: u32) -> SmackLabel {
        self.subjects
            .iter()
            .find(|s| s.pid == pid)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| self.default_label.clone())
    }

    /// Set an object label
    fn set_object_label(&mut self, path: &str, label: &str) {
        self.objects.retain(|o| o.path != path);
        self.objects.push(ObjectEntry {
            path: String::from(path),
            label: SmackLabel::new(label),
        });
    }

    /// Get an object label (find most specific path match)
    fn get_object_label(&self, path: &str) -> SmackLabel {
        let mut best_match: Option<&ObjectEntry> = None;
        let mut best_len = 0;

        for obj in &self.objects {
            if path.starts_with(obj.path.as_str()) && obj.path.len() >= best_len {
                best_len = obj.path.len();
                best_match = Some(obj);
            }
        }

        best_match
            .map(|o| o.label.clone())
            .unwrap_or_else(|| SmackLabel::new(LABEL_FLOOR))
    }

    /// Core access check: subject_label accessing object_label with requested permissions
    fn check_access(&mut self, subject_label: &str, object_label: &str, requested: u8) -> bool {
        self.checks_performed = self.checks_performed.saturating_add(1);

        // Rule 1: Subject label == Object label -> all access
        if subject_label == object_label {
            return true;
        }

        // Rule 2: Object label is star (*) -> read access to everyone
        if object_label == LABEL_STAR && (requested & !ACCESS_READ) == 0 {
            return true;
        }

        // Rule 3: Subject label is star (*) -> can read/write/exec anything
        // (star is the "privileged" process label)
        if subject_label == LABEL_STAR {
            return true;
        }

        // Rule 4: Subject label is hat (^) -> can read star objects
        if subject_label == LABEL_HAT
            && object_label == LABEL_STAR
            && (requested & !ACCESS_READ) == 0
        {
            return true;
        }

        // Rule 5: Check explicit rules
        for rule in &self.rules {
            if rule.subject.0 == subject_label && rule.object.0 == object_label {
                if (rule.access & requested) == requested {
                    return true;
                }
            }
        }

        // Default deny
        self.denials = self.denials.saturating_add(1);
        false
    }

    /// High-level access check for a process accessing a path
    fn check_process_access(&mut self, pid: u32, path: &str, requested: u8) -> bool {
        let subject_label = self.get_subject_label(pid);
        let object_label = self.get_object_label(path);

        let granted = self.check_access(subject_label.as_str(), object_label.as_str(), requested);

        // Audit logging
        if self.audit_log.len() >= self.max_audit_entries {
            self.audit_log.remove(0);
        }
        self.audit_log.push(SmackAuditEntry {
            subject: subject_label.0.clone(),
            object: object_label.0.clone(),
            requested,
            granted,
            pid,
        });

        if !granted {
            serial_println!(
                "    [smack] DENIED: '{}' -> '{}' {} (pid={})",
                subject_label.0,
                object_label.0,
                format_access(requested),
                pid
            );

            crate::security::audit::log(
                crate::security::audit::AuditEvent::MacDenied,
                crate::security::audit::AuditResult::Deny,
                pid,
                0,
                &format!(
                    "smack: '{}' -> '{}' {}",
                    subject_label.0,
                    object_label.0,
                    format_access(requested)
                ),
            );
        }

        granted
    }

    /// Remove process entries on exit
    fn process_exit(&mut self, pid: u32) {
        self.subjects.retain(|s| s.pid != pid);
    }

    /// Check if a process can administer MAC (onlycap check)
    fn can_admin_mac(&self, pid: u32) -> bool {
        if self.onlycap.is_empty() {
            return true; // No restriction
        }
        let label = self.get_subject_label(pid);
        self.onlycap.iter().any(|l| *l == label)
    }
}

/// SMACK policy engine (public backward-compatible API)
pub struct SmackPolicy;

impl SmackPolicy {
    pub fn new() -> Self {
        SmackPolicy
    }

    pub fn add_rule(&mut self, rule: SmackRule) {
        if let Some(ref mut inner) = *SMACK.lock() {
            inner.add_rule_inner(rule);
        }
    }

    pub fn check(&self, subject: &str, object: &str, access: u8) -> bool {
        if let Some(ref mut inner) = *SMACK.lock() {
            return inner.check_access(subject, object, access);
        }
        false
    }
}

/// Add an access rule
pub fn add_rule(subject: &str, object: &str, access: u8) {
    if let Some(ref mut inner) = *SMACK.lock() {
        inner.add_rule_inner(SmackRule::new(subject, object, access));
    }
}

/// Add a rule from string format: "subject object rwxatl"
pub fn add_rule_str(rule_str: &str) {
    let parts: Vec<&str> = rule_str.split_whitespace().collect();
    if parts.len() >= 3 {
        let access = parse_access_string(parts[2]);
        add_rule(parts[0], parts[1], access);
    }
}

/// Set process label
pub fn set_process_label(pid: u32, label: &str) {
    if let Some(ref mut inner) = *SMACK.lock() {
        inner.set_subject_label(pid, label);
    }
}

/// Get process label
pub fn get_process_label(pid: u32) -> String {
    if let Some(ref inner) = *SMACK.lock() {
        return inner.get_subject_label(pid).0;
    }
    String::from(LABEL_FLOOR)
}

/// Set object (file/path) label
pub fn set_object_label(path: &str, label: &str) {
    if let Some(ref mut inner) = *SMACK.lock() {
        inner.set_object_label(path, label);
    }
}

/// Check access for a process accessing a path
pub fn check_access(pid: u32, path: &str, access: u8) -> bool {
    if let Some(ref mut inner) = *SMACK.lock() {
        return inner.check_process_access(pid, path, access);
    }
    false
}

/// Check raw label-to-label access
pub fn check_labels(subject: &str, object: &str, access: u8) -> bool {
    if let Some(ref mut inner) = *SMACK.lock() {
        return inner.check_access(subject, object, access);
    }
    false
}

/// Handle process exit
pub fn process_exit(pid: u32) {
    if let Some(ref mut inner) = *SMACK.lock() {
        inner.process_exit(pid);
    }
}

/// Check if process can administer SMACK
pub fn can_admin(pid: u32) -> bool {
    if let Some(ref inner) = *SMACK.lock() {
        return inner.can_admin_mac(pid);
    }
    false
}

/// Set onlycap labels (restrict CAP_MAC_ADMIN to these labels)
pub fn set_onlycap(labels: &[&str]) {
    if let Some(ref mut inner) = *SMACK.lock() {
        inner.onlycap.clear();
        for label in labels {
            inner.onlycap.push(SmackLabel::new(label));
        }
        serial_println!("    [smack] onlycap set to {} labels", labels.len());
    }
}

/// Get statistics
pub fn stats() -> (u64, u64, u64) {
    if let Some(ref inner) = *SMACK.lock() {
        return (inner.checks_performed, inner.denials, inner.rules_added);
    }
    (0, 0, 0)
}

/// Initialize the SMACK subsystem
pub fn init() {
    let mut inner = SmackInner::new();
    inner.load_builtin_rules();

    let rule_count = inner.rules.len();
    *SMACK.lock() = Some(inner);

    serial_println!("    [smack] Simplified MAC Kernel initialized");
    serial_println!("    [smack] Special labels: _ (floor), * (star), ^ (hat), @ (web)");
    serial_println!(
        "    [smack] Built-in rules: {}, max rules: {}",
        rule_count,
        MAX_RULES
    );
}
