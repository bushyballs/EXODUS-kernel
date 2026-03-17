/// Mandatory Access Control (MAC) — system-wide security policy
///
/// MAC rules are enforced by the kernel, not bypassable by any process.
/// Even root (UID 0) is subject to MAC policy.
///
/// Policy is defined as a set of rules mapping:
///   Subject (process label) -> Object (resource label) -> Allowed actions
///
/// Labels are hierarchical: "system.network.dns" is a sublabel of "system.network"
use crate::serial_println;
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Global MAC policy
static MAC_POLICY: Mutex<Option<MacPolicy>> = Mutex::new(None);

/// A security label
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Label(pub String);

impl Label {
    /// Check if self is a parent of other (e.g., "system" is parent of "system.network")
    pub fn is_parent_of(&self, other: &Label) -> bool {
        other.0.starts_with(&self.0)
            && other.0.len() > self.0.len()
            && other.0.as_bytes().get(self.0.len()) == Some(&b'.')
    }

    /// Check if labels match (exact match or parent relationship)
    pub fn matches(&self, other: &Label) -> bool {
        self.0 == other.0 || self.is_parent_of(other)
    }
}

/// Actions that MAC can control
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Read,
    Write,
    Execute,
    Create,
    Delete,
    Connect,
    Listen,
    Signal,
    Mount,
    Ptrace,
}

/// A MAC rule
#[derive(Debug, Clone)]
pub struct MacRule {
    /// Subject label (the process)
    pub subject: Label,
    /// Object label (the resource)
    pub object: Label,
    /// Allowed actions
    pub allowed: Vec<Action>,
    /// Is this a deny rule? (deny overrides allow)
    pub deny: bool,
}

/// MAC policy — collection of rules
pub struct MacPolicy {
    rules: Vec<MacRule>,
    /// Process label assignments
    process_labels: BTreeMap<u32, Label>,
    /// File label assignments
    file_labels: BTreeMap<String, Label>,
    /// Enforcement mode
    pub enforcing: bool,
}

impl MacPolicy {
    pub fn new() -> Self {
        MacPolicy {
            rules: Vec::new(),
            process_labels: BTreeMap::new(),
            file_labels: BTreeMap::new(),
            enforcing: true,
        }
    }

    /// Add a rule to the policy
    pub fn add_rule(&mut self, rule: MacRule) {
        self.rules.push(rule);
    }

    /// Assign a label to a process
    pub fn label_process(&mut self, pid: u32, label: Label) {
        self.process_labels.insert(pid, label);
    }

    /// Assign a label to a file path
    pub fn label_file(&mut self, path: String, label: Label) {
        self.file_labels.insert(path, label);
    }

    /// Check if an action is allowed by MAC policy
    pub fn check(&self, pid: u32, object_label: &Label, action: Action) -> bool {
        if !self.enforcing {
            return true;
        }

        let subject_label = match self.process_labels.get(&pid) {
            Some(l) => l,
            None => return false, // unlabeled process = deny all
        };

        // Check deny rules first (deny overrides allow)
        for rule in &self.rules {
            if rule.deny
                && rule.subject.matches(subject_label)
                && rule.object.matches(object_label)
                && rule.allowed.contains(&action)
            {
                return false;
            }
        }

        // Check allow rules
        for rule in &self.rules {
            if !rule.deny
                && rule.subject.matches(subject_label)
                && rule.object.matches(object_label)
                && rule.allowed.contains(&action)
            {
                return true;
            }
        }

        false // default deny
    }

    /// Load the default system policy
    fn load_default(&mut self) {
        // Kernel (PID 0) has full access
        self.label_process(0, Label(String::from("kernel")));
        self.label_process(1, Label(String::from("system.init")));

        // Kernel can do everything
        self.add_rule(MacRule {
            subject: Label(String::from("kernel")),
            object: Label(String::from("*")),
            allowed: alloc::vec![
                Action::Read,
                Action::Write,
                Action::Execute,
                Action::Create,
                Action::Delete,
                Action::Connect,
                Action::Listen,
                Action::Signal,
                Action::Mount,
                Action::Ptrace,
            ],
            deny: false,
        });

        // Init can manage services
        self.add_rule(MacRule {
            subject: Label(String::from("system.init")),
            object: Label(String::from("system")),
            allowed: alloc::vec![
                Action::Read,
                Action::Write,
                Action::Execute,
                Action::Create,
                Action::Signal,
            ],
            deny: false,
        });

        // User processes can read /usr, /bin, /lib
        self.add_rule(MacRule {
            subject: Label(String::from("user")),
            object: Label(String::from("system.bin")),
            allowed: alloc::vec![Action::Read, Action::Execute],
            deny: false,
        });

        // User processes can read/write their own home
        self.add_rule(MacRule {
            subject: Label(String::from("user")),
            object: Label(String::from("user.home")),
            allowed: alloc::vec![Action::Read, Action::Write, Action::Create, Action::Delete,],
            deny: false,
        });

        // Nobody can write to /boot (except kernel)
        self.add_rule(MacRule {
            subject: Label(String::from("user")),
            object: Label(String::from("system.boot")),
            allowed: alloc::vec![Action::Write, Action::Delete],
            deny: true,
        });

        // Network access for network-labeled processes only
        self.add_rule(MacRule {
            subject: Label(String::from("system.network")),
            object: Label(String::from("network")),
            allowed: alloc::vec![Action::Connect, Action::Listen, Action::Read, Action::Write],
            deny: false,
        });
    }
}

/// Initialize MAC
pub fn init() {
    let mut policy = MacPolicy::new();
    policy.load_default();
    *MAC_POLICY.lock() = Some(policy);
    serial_println!("    [mac] Mandatory access control loaded (enforcing)");
}

/// Check MAC policy
pub fn check(pid: u32, object_label: &str, action: Action) -> bool {
    MAC_POLICY
        .lock()
        .as_ref()
        .map(|p| p.check(pid, &Label(String::from(object_label)), action))
        .unwrap_or(false)
}
