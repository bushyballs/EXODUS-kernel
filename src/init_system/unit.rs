/// Unit state machine and description management
///
/// Part of the AIOS init_system subsystem.
///
/// Each "unit" is a managed entity (service, socket, timer, target, etc.)
/// with a well-defined state machine. This module tracks every loaded unit,
/// its current state, and provides the state transition logic that all
/// unit types share.
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

// ── Unit types ─────────────────────────────────────────────────────────────

/// The kind of entity a unit represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    Service,
    Socket,
    Timer,
    Target,
    Mount,
    Device,
}

// ── Unit states (state machine) ────────────────────────────────────────────

/// Lifecycle states for a unit.
///
/// Transition diagram:
///   Inactive -> Activating -> Active -> Deactivating -> Inactive
///                  |                        |
///                  +-------> Failed <-------+
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitState {
    Inactive,
    Activating,
    Active,
    Deactivating,
    Failed,
    Reloading,
    Maintenance,
}

impl UnitState {
    /// Whether this state is considered "alive" (running or transitioning to run).
    pub fn is_alive(self) -> bool {
        matches!(self, UnitState::Activating | UnitState::Active | UnitState::Reloading)
    }

    fn label(self) -> &'static str {
        match self {
            UnitState::Inactive      => "inactive",
            UnitState::Activating    => "activating",
            UnitState::Active        => "active",
            UnitState::Deactivating  => "deactivating",
            UnitState::Failed        => "failed",
            UnitState::Reloading     => "reloading",
            UnitState::Maintenance   => "maintenance",
        }
    }
}

// ── Unit descriptor ────────────────────────────────────────────────────────

/// Describes a loaded unit and its current runtime state.
#[derive(Clone)]
pub struct UnitDescriptor {
    pub name: String,
    pub name_hash: u64,
    pub kind: UnitKind,
    pub state: UnitState,
    pub description: String,
    /// Number of times this unit has been activated.
    pub activation_count: u32,
    /// Number of times this unit has failed.
    pub failure_count: u32,
    /// TSC timestamp of last state change.
    pub last_state_change: u64,
    /// Whether the unit is enabled (will be started at boot).
    pub enabled: bool,
    /// Whether a stop has been requested.
    pub stop_pending: bool,
}

// ── State transition logic ─────────────────────────────────────────────────

/// Result of a state transition attempt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransitionResult {
    Ok,
    AlreadyInState,
    InvalidTransition,
}

/// Validate and perform a state transition.
fn try_transition(current: UnitState, target: UnitState) -> TransitionResult {
    if current == target {
        return TransitionResult::AlreadyInState;
    }

    let valid = match (current, target) {
        (UnitState::Inactive, UnitState::Activating)       => true,
        (UnitState::Activating, UnitState::Active)          => true,
        (UnitState::Activating, UnitState::Failed)          => true,
        (UnitState::Active, UnitState::Deactivating)        => true,
        (UnitState::Active, UnitState::Reloading)           => true,
        (UnitState::Active, UnitState::Failed)              => true,
        (UnitState::Reloading, UnitState::Active)           => true,
        (UnitState::Reloading, UnitState::Failed)           => true,
        (UnitState::Deactivating, UnitState::Inactive)      => true,
        (UnitState::Deactivating, UnitState::Failed)        => true,
        (UnitState::Failed, UnitState::Inactive)            => true, // reset
        (UnitState::Failed, UnitState::Activating)          => true, // restart
        (UnitState::Maintenance, UnitState::Inactive)       => true,
        (_, UnitState::Maintenance)                         => true, // any -> maintenance
        _ => false,
    };

    if valid {
        TransitionResult::Ok
    } else {
        TransitionResult::InvalidTransition
    }
}

/// Read TSC as a monotonic timestamp.
fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
        ((hi as u64) << 32) | (lo as u64)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

// ── Unit manager ───────────────────────────────────────────────────────────

struct UnitManager {
    units: Vec<UnitDescriptor>,
}

impl UnitManager {
    fn new() -> Self {
        UnitManager {
            units: Vec::new(),
        }
    }

    /// Register a new unit. Returns its index.
    fn register(&mut self, name: &str, kind: UnitKind, description: &str) -> usize {
        let hash = fnv1a_hash(name.as_bytes());

        // Check for duplicate
        for (i, u) in self.units.iter().enumerate() {
            if u.name_hash == hash {
                return i;
            }
        }

        let idx = self.units.len();
        self.units.push(UnitDescriptor {
            name: String::from(name),
            name_hash: hash,
            kind,
            state: UnitState::Inactive,
            description: String::from(description),
            activation_count: 0,
            failure_count: 0,
            last_state_change: read_tsc(),
            enabled: false,
            stop_pending: false,
        });
        idx
    }

    /// Find a unit by name hash.
    fn find_mut(&mut self, name: &str) -> Option<&mut UnitDescriptor> {
        let hash = fnv1a_hash(name.as_bytes());
        self.units.iter_mut().find(|u| u.name_hash == hash)
    }

    fn find(&self, name: &str) -> Option<&UnitDescriptor> {
        let hash = fnv1a_hash(name.as_bytes());
        self.units.iter().find(|u| u.name_hash == hash)
    }

    /// Transition a unit to a new state.
    fn transition(&mut self, name: &str, target: UnitState) -> TransitionResult {
        let hash = fnv1a_hash(name.as_bytes());
        let unit = match self.units.iter_mut().find(|u| u.name_hash == hash) {
            Some(u) => u,
            None => return TransitionResult::InvalidTransition,
        };

        let result = try_transition(unit.state, target);
        if result == TransitionResult::Ok {
            let old = unit.state;
            unit.state = target;
            unit.last_state_change = read_tsc();

            if target == UnitState::Active {
                unit.activation_count = unit.activation_count.saturating_add(1);
            }
            if target == UnitState::Failed {
                unit.failure_count = unit.failure_count.saturating_add(1);
            }

            serial_println!(
                "[init_system::unit] {} {} -> {}",
                name,
                old.label(),
                target.label()
            );
        }

        result
    }

    /// Get all units in a given state.
    fn units_in_state(&self, state: UnitState) -> Vec<&UnitDescriptor> {
        self.units.iter().filter(|u| u.state == state).collect()
    }

    /// Count units by state.
    fn count_in_state(&self, state: UnitState) -> usize {
        self.units.iter().filter(|u| u.state == state).count()
    }
}

// ── Global state ───────────────────────────────────────────────────────────

static UNIT_MGR: Mutex<Option<UnitManager>> = Mutex::new(None);

/// Initialize the unit subsystem.
pub fn init() {
    let mut guard = UNIT_MGR.lock();
    *guard = Some(UnitManager::new());
    serial_println!("[init_system::unit] unit manager initialized");
}

/// Register a new unit.
pub fn register(name: &str, kind: UnitKind, description: &str) -> usize {
    let mut guard = UNIT_MGR.lock();
    let mgr = guard.as_mut().expect("unit manager not initialized");
    mgr.register(name, kind, description)
}

/// Transition a unit to a new state.
pub fn transition(name: &str, target: UnitState) -> TransitionResult {
    let mut guard = UNIT_MGR.lock();
    let mgr = guard.as_mut().expect("unit manager not initialized");
    mgr.transition(name, target)
}

/// Get the current state of a unit.
pub fn get_state(name: &str) -> Option<UnitState> {
    let guard = UNIT_MGR.lock();
    let mgr = guard.as_ref().expect("unit manager not initialized");
    mgr.find(name).map(|u| u.state)
}

/// Enable a unit for auto-start at boot.
pub fn enable(name: &str) -> bool {
    let mut guard = UNIT_MGR.lock();
    let mgr = guard.as_mut().expect("unit manager not initialized");
    if let Some(u) = mgr.find_mut(name) {
        u.enabled = true;
        true
    } else {
        false
    }
}

/// Disable a unit from auto-start.
pub fn disable(name: &str) -> bool {
    let mut guard = UNIT_MGR.lock();
    let mgr = guard.as_mut().expect("unit manager not initialized");
    if let Some(u) = mgr.find_mut(name) {
        u.enabled = false;
        true
    } else {
        false
    }
}

/// Get count of loaded units.
pub fn loaded_count() -> usize {
    let guard = UNIT_MGR.lock();
    let mgr = guard.as_ref().expect("unit manager not initialized");
    mgr.units.len()
}

/// Get count of active units.
pub fn active_count() -> usize {
    let guard = UNIT_MGR.lock();
    let mgr = guard.as_ref().expect("unit manager not initialized");
    mgr.count_in_state(UnitState::Active)
}

/// Get count of failed units.
pub fn failed_count() -> usize {
    let guard = UNIT_MGR.lock();
    let mgr = guard.as_ref().expect("unit manager not initialized");
    mgr.count_in_state(UnitState::Failed)
}

/// Reset a failed unit back to inactive so it can be restarted.
pub fn reset_failed(name: &str) -> bool {
    let mut guard = UNIT_MGR.lock();
    let mgr = guard.as_mut().expect("unit manager not initialized");
    let result = mgr.transition(name, UnitState::Inactive);
    result == TransitionResult::Ok
}
