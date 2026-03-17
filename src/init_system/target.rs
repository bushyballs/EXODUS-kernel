/// Target groups (multi-user, graphical, rescue, etc.)
///
/// Part of the AIOS init_system subsystem.
///
/// Targets are grouping units that represent system run-levels. Each target
/// defines a set of services that should be running. Switching targets
/// starts/stops services to match the desired state. Supports ordered
/// target transitions (e.g., rescue -> multi-user -> graphical).
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

// ── Boot targets ───────────────────────────────────────────────────────────

/// System boot targets defining service groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootTarget {
    /// Emergency/rescue mode: minimal single-user.
    Rescue,
    /// Multi-user mode: all core services, no GUI.
    MultiUser,
    /// Graphical mode: multi-user + display manager.
    Graphical,
    /// System shutdown.
    Shutdown,
    /// System reboot.
    Reboot,
}

impl BootTarget {
    /// Ordering level: targets are ordered so transitions can determine
    /// which services to start/stop.
    fn level(self) -> u8 {
        match self {
            BootTarget::Rescue    => 1,
            BootTarget::MultiUser => 3,
            BootTarget::Graphical => 5,
            BootTarget::Shutdown  => 0,
            BootTarget::Reboot    => 0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            BootTarget::Rescue    => "rescue.target",
            BootTarget::MultiUser => "multi-user.target",
            BootTarget::Graphical => "graphical.target",
            BootTarget::Shutdown  => "shutdown.target",
            BootTarget::Reboot    => "reboot.target",
        }
    }

    /// Parse a target from its string name.
    pub fn from_name(name: &str) -> Option<Self> {
        let hash = fnv1a_hash(name.as_bytes());
        if hash == fnv1a_hash(b"rescue.target") || hash == fnv1a_hash(b"rescue") {
            Some(BootTarget::Rescue)
        } else if hash == fnv1a_hash(b"multi-user.target") || hash == fnv1a_hash(b"multi-user") {
            Some(BootTarget::MultiUser)
        } else if hash == fnv1a_hash(b"graphical.target") || hash == fnv1a_hash(b"graphical") {
            Some(BootTarget::Graphical)
        } else if hash == fnv1a_hash(b"shutdown.target") || hash == fnv1a_hash(b"shutdown") {
            Some(BootTarget::Shutdown)
        } else if hash == fnv1a_hash(b"reboot.target") || hash == fnv1a_hash(b"reboot") {
            Some(BootTarget::Reboot)
        } else {
            None
        }
    }
}

// ── Target membership ──────────────────────────────────────────────────────

/// A service binding to a target.
#[derive(Clone)]
struct TargetBinding {
    service_name_hash: u64,
    service_name: String,
    target: BootTarget,
    /// If true, this service is required (not just wanted).
    required: bool,
}

// ── Target manager ─────────────────────────────────────────────────────────

/// Manages active boot target and transitions between targets.
struct TargetInner {
    current: BootTarget,
    default_target: BootTarget,
    bindings: Vec<TargetBinding>,
    /// History of target transitions (most recent last).
    history: Vec<BootTarget>,
    /// Services that were explicitly started in the current target.
    active_services: Vec<u64>,
}

impl TargetInner {
    fn new() -> Self {
        TargetInner {
            current: BootTarget::Rescue,
            default_target: BootTarget::MultiUser,
            bindings: Vec::new(),
            history: Vec::new(),
            active_services: Vec::new(),
        }
    }

    /// Bind a service to a target.
    fn bind_service(&mut self, service_name: &str, target: BootTarget, required: bool) {
        let hash = fnv1a_hash(service_name.as_bytes());

        // Avoid duplicate bindings
        let exists = self.bindings.iter().any(|b| {
            b.service_name_hash == hash && b.target == target
        });
        if exists {
            return;
        }

        self.bindings.push(TargetBinding {
            service_name_hash: hash,
            service_name: String::from(service_name),
            target,
            required,
        });
    }

    /// Get all services that should be running for a given target.
    /// Includes services from lower-level targets (cascading).
    fn services_for_target(&self, target: BootTarget) -> Vec<&TargetBinding> {
        let level = target.level();
        self.bindings
            .iter()
            .filter(|b| b.target.level() <= level)
            .collect()
    }

    /// Compute services to start when transitioning to a new target.
    fn services_to_start(&self, target: BootTarget) -> Vec<&str> {
        let needed = self.services_for_target(target);
        let mut to_start = Vec::new();

        for binding in &needed {
            let already_active = self.active_services.iter().any(|h| *h == binding.service_name_hash);
            if !already_active {
                to_start.push(binding.service_name.as_str());
            }
        }

        to_start
    }

    /// Compute services to stop when transitioning to a new target.
    fn services_to_stop(&self, target: BootTarget) -> Vec<u64> {
        let level = target.level();
        let mut to_stop = Vec::new();

        // Any active service whose target level is above the new target
        // should be stopped.
        for &hash in &self.active_services {
            let binding = self.bindings.iter().find(|b| b.service_name_hash == hash);
            if let Some(b) = binding {
                if b.target.level() > level {
                    to_stop.push(hash);
                }
            }
        }

        to_stop
    }

    /// Perform a target switch. Returns lists of (services_to_start, services_to_stop_hashes).
    fn switch_to(&mut self, target: BootTarget) -> (Vec<String>, Vec<u64>) {
        if target == self.current {
            return (Vec::new(), Vec::new());
        }

        serial_println!(
            "[init_system::target] switching {} -> {}",
            self.current.label(),
            target.label()
        );

        let to_start: Vec<String> = self.services_to_start(target)
            .into_iter()
            .map(String::from)
            .collect();
        let to_stop = self.services_to_stop(target);

        // Remove stopped services from active set
        self.active_services.retain(|h| !to_stop.contains(h));

        // Add newly started services to active set
        for name in &to_start {
            let hash = fnv1a_hash(name.as_bytes());
            if !self.active_services.contains(&hash) {
                self.active_services.push(hash);
            }
        }

        self.history.push(self.current);
        self.current = target;

        (to_start, to_stop)
    }

    /// Mark a service as active in the current target.
    fn mark_service_active(&mut self, name: &str) {
        let hash = fnv1a_hash(name.as_bytes());
        if !self.active_services.contains(&hash) {
            self.active_services.push(hash);
        }
    }

    /// Mark a service as stopped.
    fn mark_service_stopped(&mut self, name: &str) {
        let hash = fnv1a_hash(name.as_bytes());
        self.active_services.retain(|h| *h != hash);
    }
}

/// Public wrapper matching original stub API.
pub struct TargetManager {
    inner: TargetInner,
}

impl TargetManager {
    pub fn new() -> Self {
        TargetManager {
            inner: TargetInner::new(),
        }
    }

    /// Switch to a new boot target.
    pub fn switch_to(&mut self, target: BootTarget) {
        self.inner.switch_to(target);
    }

    /// Get the current active boot target.
    pub fn current(&self) -> BootTarget {
        self.inner.current
    }
}

// ── Global state ───────────────────────────────────────────────────────────

static TARGET_MGR: Mutex<Option<TargetInner>> = Mutex::new(None);

/// Initialize the target subsystem, starting in Rescue mode.
pub fn init() {
    let mut guard = TARGET_MGR.lock();
    *guard = Some(TargetInner::new());
    serial_println!("[init_system::target] target manager initialized (default=multi-user)");
}

/// Bind a service to a boot target.
pub fn bind_service(service_name: &str, target: BootTarget, required: bool) {
    let mut guard = TARGET_MGR.lock();
    let mgr = guard.as_mut().expect("target manager not initialized");
    mgr.bind_service(service_name, target, required);
}

/// Switch to a new boot target. Returns the list of service names to start.
pub fn switch_to(target: BootTarget) -> Vec<String> {
    let mut guard = TARGET_MGR.lock();
    let mgr = guard.as_mut().expect("target manager not initialized");
    let (to_start, _to_stop) = mgr.switch_to(target);
    to_start
}

/// Get the current boot target.
pub fn current() -> BootTarget {
    let guard = TARGET_MGR.lock();
    let mgr = guard.as_ref().expect("target manager not initialized");
    mgr.current
}

/// Set the default boot target.
pub fn set_default(target: BootTarget) {
    let mut guard = TARGET_MGR.lock();
    let mgr = guard.as_mut().expect("target manager not initialized");
    mgr.default_target = target;
}

/// Get the default boot target.
pub fn get_default() -> BootTarget {
    let guard = TARGET_MGR.lock();
    let mgr = guard.as_ref().expect("target manager not initialized");
    mgr.default_target
}

/// Mark a service as active in the current target.
pub fn mark_active(service: &str) {
    let mut guard = TARGET_MGR.lock();
    let mgr = guard.as_mut().expect("target manager not initialized");
    mgr.mark_service_active(service);
}

/// Mark a service as stopped.
pub fn mark_stopped(service: &str) {
    let mut guard = TARGET_MGR.lock();
    let mgr = guard.as_mut().expect("target manager not initialized");
    mgr.mark_service_stopped(service);
}

/// Get the number of active services in the current target.
pub fn active_service_count() -> usize {
    let guard = TARGET_MGR.lock();
    let mgr = guard.as_ref().expect("target manager not initialized");
    mgr.active_services.len()
}
