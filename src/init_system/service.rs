/// Service definition and lifecycle
///
/// Part of the AIOS init_system subsystem.
///
/// Defines the ServiceEntry structure and provides the low-level lifecycle
/// operations (start, stop, restart) for individual services. State
/// transitions are validated to prevent illegal moves. Each service
/// tracks its PID, exit code, restart count, and timestamps.
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

// ── Service lifecycle states ───────────────────────────────────────────────

/// Service lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Inactive,
    Starting,
    Running,
    Stopping,
    Failed,
    Restarting,
}

impl ServiceState {
    fn label(self) -> &'static str {
        match self {
            ServiceState::Inactive   => "inactive",
            ServiceState::Starting   => "starting",
            ServiceState::Running    => "running",
            ServiceState::Stopping   => "stopping",
            ServiceState::Failed     => "failed",
            ServiceState::Restarting => "restarting",
        }
    }
}

// ── Restart policy ─────────────────────────────────────────────────────────

/// Restart behavior on service exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    No,
    OnFailure,
    Always,
    OnAbnormal,
}

// ── Service entry ──────────────────────────────────────────────────────────

/// Full description of a managed service.
#[derive(Clone)]
pub struct ServiceEntry {
    pub name: String,
    pub name_hash: u64,
    pub state: ServiceState,
    pub pid: u64,
    pub exit_code: i32,
    pub restart_policy: RestartPolicy,
    pub restart_count: u32,
    pub max_restarts: u32,
    pub restart_delay_ms: u64,
    /// TSC timestamp when the service entered its current state.
    pub state_change_ts: u64,
    /// TSC timestamp when the service was last started.
    pub start_ts: u64,
    /// Whether the service is enabled for auto-start.
    pub enabled: bool,
    /// Dependencies (name hashes of services this one requires).
    pub dependencies: Vec<u64>,
}

impl ServiceEntry {
    /// Create a new inactive service entry.
    pub fn new(name: &str) -> Self {
        ServiceEntry {
            name: String::from(name),
            name_hash: fnv1a_hash(name.as_bytes()),
            state: ServiceState::Inactive,
            pid: 0,
            exit_code: 0,
            restart_policy: RestartPolicy::No,
            restart_count: 0,
            max_restarts: 5,
            restart_delay_ms: 1000,
            state_change_ts: read_tsc(),
            start_ts: 0,
            enabled: false,
            dependencies: Vec::new(),
        }
    }

    /// Validate and perform a state transition.
    fn set_state(&mut self, new_state: ServiceState) -> Result<(), ()> {
        let valid = match (self.state, new_state) {
            (ServiceState::Inactive, ServiceState::Starting)     => true,
            (ServiceState::Starting, ServiceState::Running)      => true,
            (ServiceState::Starting, ServiceState::Failed)       => true,
            (ServiceState::Running, ServiceState::Stopping)      => true,
            (ServiceState::Running, ServiceState::Failed)        => true,
            (ServiceState::Stopping, ServiceState::Inactive)     => true,
            (ServiceState::Stopping, ServiceState::Failed)       => true,
            (ServiceState::Failed, ServiceState::Starting)       => true,
            (ServiceState::Failed, ServiceState::Inactive)       => true,
            (ServiceState::Running, ServiceState::Restarting)    => true,
            (ServiceState::Restarting, ServiceState::Starting)   => true,
            (ServiceState::Restarting, ServiceState::Failed)     => true,
            _ => false,
        };

        if !valid {
            serial_println!(
                "[init_system::service] invalid transition for {}: {} -> {}",
                self.name, self.state.label(), new_state.label()
            );
            return Err(());
        }

        self.state = new_state;
        self.state_change_ts = read_tsc();
        Ok(())
    }

    /// Begin starting the service.
    pub fn start(&mut self) -> Result<(), ()> {
        if self.state == ServiceState::Running {
            return Ok(()); // already running
        }

        self.set_state(ServiceState::Starting)?;
        self.start_ts = read_tsc();
        serial_println!("[init_system::service] starting {}", self.name);
        Ok(())
    }

    /// Mark the service as successfully started with the given PID.
    pub fn mark_running(&mut self, pid: u64) -> Result<(), ()> {
        self.set_state(ServiceState::Running)?;
        self.pid = pid;
        self.exit_code = 0;
        serial_println!("[init_system::service] {} running (pid={})", self.name, pid);
        Ok(())
    }

    /// Begin stopping the service.
    pub fn stop(&mut self) -> Result<(), ()> {
        if self.state == ServiceState::Inactive {
            return Ok(()); // already stopped
        }

        self.set_state(ServiceState::Stopping)?;
        serial_println!("[init_system::service] stopping {}", self.name);
        Ok(())
    }

    /// Mark the service as fully stopped.
    pub fn mark_stopped(&mut self, exit_code: i32) -> Result<(), ()> {
        self.exit_code = exit_code;
        self.pid = 0;

        if exit_code != 0 && self.state != ServiceState::Stopping {
            // Abnormal exit: enter failed state
            self.set_state(ServiceState::Failed)?;
            self.handle_restart_policy();
        } else if self.state == ServiceState::Stopping {
            self.set_state(ServiceState::Inactive)?;
        } else {
            // Unexpected exit while running
            self.set_state(ServiceState::Failed)?;
            self.handle_restart_policy();
        }

        Ok(())
    }

    /// Check restart policy and potentially queue a restart.
    fn handle_restart_policy(&mut self) {
        let should_restart = match self.restart_policy {
            RestartPolicy::No => false,
            RestartPolicy::Always => true,
            RestartPolicy::OnFailure => self.exit_code != 0,
            RestartPolicy::OnAbnormal => self.exit_code < 0, // signal death
        };

        if should_restart && self.restart_count < self.max_restarts {
            self.restart_count = self.restart_count.saturating_add(1);
            serial_println!(
                "[init_system::service] {} scheduling restart ({}/{})",
                self.name, self.restart_count, self.max_restarts
            );
            // Transition to restarting (caller will handle the actual restart)
            let _ = self.set_state(ServiceState::Inactive);
        } else if self.restart_count >= self.max_restarts {
            serial_println!(
                "[init_system::service] {} exceeded max restarts ({})",
                self.name, self.max_restarts
            );
        }
    }

    /// Check if all dependencies are satisfied (given a lookup function).
    pub fn dependencies_met<F>(&self, is_running: F) -> bool
    where
        F: Fn(u64) -> bool,
    {
        self.dependencies.iter().all(|dep_hash| is_running(*dep_hash))
    }

    /// Add a dependency by name.
    pub fn add_dependency(&mut self, dep_name: &str) {
        let hash = fnv1a_hash(dep_name.as_bytes());
        if !self.dependencies.contains(&hash) {
            self.dependencies.push(hash);
        }
    }

    /// Reset the service from failed state back to inactive.
    pub fn reset(&mut self) -> Result<(), ()> {
        if self.state != ServiceState::Failed {
            return Err(());
        }
        self.state = ServiceState::Inactive;
        self.restart_count = 0;
        self.exit_code = 0;
        self.pid = 0;
        serial_println!("[init_system::service] {} reset to inactive", self.name);
        Ok(())
    }
}

// ── TSC helper ─────────────────────────────────────────────────────────────

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

// ── ServiceManager (legacy wrapper) ────────────────────────────────────────

/// Manages all system services and their lifecycle.
pub struct ServiceManager {
    services: Vec<ServiceEntry>,
}

impl ServiceManager {
    pub fn new() -> Self {
        ServiceManager {
            services: Vec::new(),
        }
    }

    /// Register a new service. Returns index.
    pub fn register(&mut self, name: &str) -> usize {
        let hash = fnv1a_hash(name.as_bytes());
        for (i, s) in self.services.iter().enumerate() {
            if s.name_hash == hash {
                return i;
            }
        }
        let idx = self.services.len();
        self.services.push(ServiceEntry::new(name));
        idx
    }

    /// Start a service by name.
    pub fn start(&mut self, name: &str) -> Result<(), ()> {
        let hash = fnv1a_hash(name.as_bytes());
        let svc = self.services.iter_mut().find(|s| s.name_hash == hash);
        match svc {
            Some(s) => s.start(),
            None => {
                serial_println!("[init_system::service] unknown service: {}", name);
                Err(())
            }
        }
    }

    /// Stop a service by name.
    pub fn stop(&mut self, name: &str) -> Result<(), ()> {
        let hash = fnv1a_hash(name.as_bytes());
        let svc = self.services.iter_mut().find(|s| s.name_hash == hash);
        match svc {
            Some(s) => s.stop(),
            None => {
                serial_println!("[init_system::service] unknown service: {}", name);
                Err(())
            }
        }
    }

    /// Get state of a service.
    pub fn get_state(&self, name: &str) -> Option<ServiceState> {
        let hash = fnv1a_hash(name.as_bytes());
        self.services.iter().find(|s| s.name_hash == hash).map(|s| s.state)
    }
}

// ── Global state ───────────────────────────────────────────────────────────

static SERVICE_REGISTRY: Mutex<Option<ServiceManager>> = Mutex::new(None);

/// Initialize the service subsystem.
pub fn init() {
    let mut guard = SERVICE_REGISTRY.lock();
    *guard = Some(ServiceManager::new());
    serial_println!("[init_system::service] service registry initialized");
}

/// Register a new service in the global registry.
pub fn register(name: &str) -> usize {
    let mut guard = SERVICE_REGISTRY.lock();
    let mgr = guard.as_mut().expect("service registry not initialized");
    mgr.register(name)
}

/// Start a service by name.
pub fn start(name: &str) -> Result<(), ()> {
    let mut guard = SERVICE_REGISTRY.lock();
    let mgr = guard.as_mut().expect("service registry not initialized");
    mgr.start(name)
}

/// Stop a service by name.
pub fn stop(name: &str) -> Result<(), ()> {
    let mut guard = SERVICE_REGISTRY.lock();
    let mgr = guard.as_mut().expect("service registry not initialized");
    mgr.stop(name)
}

/// Get the state of a service.
pub fn get_state(name: &str) -> Option<ServiceState> {
    let guard = SERVICE_REGISTRY.lock();
    let mgr = guard.as_ref().expect("service registry not initialized");
    mgr.get_state(name)
}
