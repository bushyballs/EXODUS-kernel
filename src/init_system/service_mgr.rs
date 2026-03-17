/// Service lifecycle management (high-level orchestrator)
///
/// Part of the AIOS init_system subsystem.
///
/// The ServiceManager is the top-level orchestrator that ties together
/// dependency resolution, unit state tracking, target membership, and
/// the service lifecycle. It handles ordered startup/shutdown sequences,
/// dependency-aware start/stop, and service status queries.
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
}

impl ServiceState {
    fn label(self) -> &'static str {
        match self {
            ServiceState::Inactive  => "inactive",
            ServiceState::Starting  => "starting",
            ServiceState::Running   => "running",
            ServiceState::Stopping  => "stopping",
            ServiceState::Failed    => "failed",
        }
    }
}

// ── Managed service record ─────────────────────────────────────────────────

/// Internal record for a managed service.
#[derive(Clone)]
struct ManagedService {
    name: String,
    name_hash: u64,
    state: ServiceState,
    pid: u64,
    exit_code: i32,
    /// Indices into the services vec for dependencies.
    dep_indices: Vec<usize>,
    /// Whether the service is enabled for auto-start.
    enabled: bool,
    /// Restart count in current lifecycle.
    restart_count: u32,
    /// Maximum allowed restarts before giving up.
    max_restarts: u32,
    /// Auto-restart on failure.
    auto_restart: bool,
    /// TSC timestamp of last state change.
    last_change: u64,
}

// ── Service manager ────────────────────────────────────────────────────────

/// Manages all system services and their lifecycle states.
struct ServiceManagerInner {
    services: Vec<ManagedService>,
    /// Boot-order cache: indices in dependency-resolved order.
    boot_order: Vec<usize>,
    /// Whether boot_order is stale and needs recomputation.
    order_dirty: bool,
}

impl ServiceManagerInner {
    fn new() -> Self {
        ServiceManagerInner {
            services: Vec::new(),
            boot_order: Vec::new(),
            order_dirty: true,
        }
    }

    /// Register a service. Returns its index.
    fn register(&mut self, name: &str) -> usize {
        let hash = fnv1a_hash(name.as_bytes());

        // Check for existing
        for (i, s) in self.services.iter().enumerate() {
            if s.name_hash == hash {
                return i;
            }
        }

        let idx = self.services.len();
        self.services.push(ManagedService {
            name: String::from(name),
            name_hash: hash,
            state: ServiceState::Inactive,
            pid: 0,
            exit_code: 0,
            dep_indices: Vec::new(),
            enabled: false,
            restart_count: 0,
            max_restarts: 5,
            auto_restart: false,
            last_change: read_tsc(),
        });
        self.order_dirty = true;
        idx
    }

    /// Find service index by name.
    fn find_index(&self, name: &str) -> Option<usize> {
        let hash = fnv1a_hash(name.as_bytes());
        self.services.iter().position(|s| s.name_hash == hash)
    }

    /// Add a dependency: `service` requires `dependency`.
    fn add_dependency(&mut self, service: &str, dependency: &str) -> Result<(), ()> {
        let svc_idx = self.find_index(service).ok_or(())?;
        let dep_idx = self.find_index(dependency).ok_or(())?;

        if !self.services[svc_idx].dep_indices.contains(&dep_idx) {
            self.services[svc_idx].dep_indices.push(dep_idx);
            self.order_dirty = true;
        }
        Ok(())
    }

    /// Recompute boot order using topological sort (Kahn's algorithm).
    fn recompute_order(&mut self) {
        let n = self.services.len();
        if n == 0 {
            self.boot_order.clear();
            self.order_dirty = false;
            return;
        }

        // Compute in-degrees
        let mut in_deg = Vec::with_capacity(n);
        for _ in 0..n {
            in_deg.push(0usize);
        }

        // Build reverse adjacency: for each service, its deps point to it
        let mut reverse_adj: Vec<Vec<usize>> = Vec::with_capacity(n);
        for _ in 0..n {
            reverse_adj.push(Vec::new());
        }

        for (i, svc) in self.services.iter().enumerate() {
            in_deg[i] = svc.dep_indices.len();
            for &dep in &svc.dep_indices {
                reverse_adj[dep].push(i);
            }
        }

        // Kahn's: start from zero in-degree nodes
        let mut queue: Vec<usize> = Vec::new();
        for i in 0..n {
            if in_deg[i] == 0 {
                queue.push(i);
            }
        }

        let mut order = Vec::with_capacity(n);
        let mut head = 0;

        while head < queue.len() {
            let node = queue[head];
            head += 1;
            order.push(node);

            for &dependent in &reverse_adj[node] {
                if in_deg[dependent] > 0 {
                    in_deg[dependent] -= 1;
                    if in_deg[dependent] == 0 {
                        queue.push(dependent);
                    }
                }
            }
        }

        if order.len() != n {
            serial_println!("[init_system::service_mgr] WARNING: dependency cycle detected, using registration order");
            order.clear();
            for i in 0..n {
                order.push(i);
            }
        }

        self.boot_order = order;
        self.order_dirty = false;
    }

    /// Check if all dependencies of a service are running.
    fn deps_satisfied(&self, idx: usize) -> bool {
        for &dep in &self.services[idx].dep_indices {
            if self.services[dep].state != ServiceState::Running {
                return false;
            }
        }
        true
    }

    /// Start a service by name, respecting dependencies.
    fn start(&mut self, name: &str) -> Result<(), ()> {
        let idx = self.find_index(name).ok_or(())?;

        if self.services[idx].state == ServiceState::Running {
            return Ok(());
        }

        // Check dependencies
        if !self.deps_satisfied(idx) {
            serial_println!(
                "[init_system::service_mgr] {} blocked: dependencies not met",
                name
            );
            return Err(());
        }

        self.services[idx].state = ServiceState::Starting;
        self.services[idx].last_change = read_tsc();
        serial_println!("[init_system::service_mgr] starting {}", name);

        // Simulate successful start (in a real kernel, this would spawn a process)
        self.services[idx].state = ServiceState::Running;
        self.services[idx].pid = (idx as u64) + 1000; // synthetic PID
        self.services[idx].last_change = read_tsc();
        serial_println!(
            "[init_system::service_mgr] {} running (pid={})",
            name, self.services[idx].pid
        );

        Ok(())
    }

    /// Stop a service by name. Also stops dependents first.
    fn stop(&mut self, name: &str) -> Result<(), ()> {
        let idx = self.find_index(name).ok_or(())?;

        if self.services[idx].state == ServiceState::Inactive {
            return Ok(());
        }

        // Find and stop dependents first (reverse deps)
        let dependents: Vec<usize> = self.services.iter().enumerate()
            .filter(|(_, s)| s.dep_indices.contains(&idx) && s.state == ServiceState::Running)
            .map(|(i, _)| i)
            .collect();

        for dep_idx in dependents {
            let dep_name = self.services[dep_idx].name.clone();
            serial_println!(
                "[init_system::service_mgr] stopping dependent {} before {}",
                dep_name, name
            );
            self.services[dep_idx].state = ServiceState::Stopping;
            self.services[dep_idx].last_change = read_tsc();
            self.services[dep_idx].state = ServiceState::Inactive;
            self.services[dep_idx].pid = 0;
        }

        self.services[idx].state = ServiceState::Stopping;
        self.services[idx].last_change = read_tsc();
        serial_println!("[init_system::service_mgr] stopping {}", name);

        self.services[idx].state = ServiceState::Inactive;
        self.services[idx].pid = 0;
        self.services[idx].last_change = read_tsc();

        Ok(())
    }

    /// Restart a service (stop then start).
    fn restart(&mut self, name: &str) -> Result<(), ()> {
        self.stop(name)?;
        self.start(name)
    }

    /// Start all enabled services in dependency order.
    fn start_all_enabled(&mut self) {
        if self.order_dirty {
            self.recompute_order();
        }

        let order = self.boot_order.clone();
        for &idx in &order {
            if self.services[idx].enabled && self.services[idx].state == ServiceState::Inactive {
                let name = self.services[idx].name.clone();
                let _ = self.start(&name);
            }
        }
    }

    /// Stop all running services in reverse dependency order.
    fn stop_all(&mut self) {
        if self.order_dirty {
            self.recompute_order();
        }

        // Stop in reverse boot order
        let order: Vec<usize> = self.boot_order.iter().rev().copied().collect();
        for idx in order {
            if self.services[idx].state == ServiceState::Running {
                let name = self.services[idx].name.clone();
                let _ = self.stop(&name);
            }
        }
    }

    /// Handle a service exit notification.
    fn on_service_exit(&mut self, name: &str, exit_code: i32) {
        let idx = match self.find_index(name) {
            Some(i) => i,
            None => return,
        };

        self.services[idx].exit_code = exit_code;
        self.services[idx].pid = 0;

        if exit_code != 0 {
            self.services[idx].state = ServiceState::Failed;
            serial_println!(
                "[init_system::service_mgr] {} failed (exit={})",
                name, exit_code
            );

            // Auto-restart logic
            if self.services[idx].auto_restart
                && self.services[idx].restart_count < self.services[idx].max_restarts
            {
                self.services[idx].restart_count = self.services[idx].restart_count.saturating_add(1);
                serial_println!(
                    "[init_system::service_mgr] auto-restarting {} ({}/{})",
                    name,
                    self.services[idx].restart_count,
                    self.services[idx].max_restarts
                );
                self.services[idx].state = ServiceState::Inactive;
                let svc_name = self.services[idx].name.clone();
                let _ = self.start(&svc_name);
            }
        } else {
            self.services[idx].state = ServiceState::Inactive;
        }

        self.services[idx].last_change = read_tsc();
    }

    /// Get count of running services.
    fn running_count(&self) -> usize {
        self.services.iter().filter(|s| s.state == ServiceState::Running).count()
    }

    /// Get count of failed services.
    fn failed_count(&self) -> usize {
        self.services.iter().filter(|s| s.state == ServiceState::Failed).count()
    }
}

/// Public wrapper matching the original stub API.
pub struct ServiceManager;

impl ServiceManager {
    pub fn new() -> Self {
        ServiceManager
    }

    pub fn start(&mut self, name: &str) -> Result<(), ()> {
        start(name)
    }

    pub fn stop(&mut self, name: &str) -> Result<(), ()> {
        stop(name)
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

// ── Global state ───────────────────────────────────────────────────────────

static SVC_MGR: Mutex<Option<ServiceManagerInner>> = Mutex::new(None);

/// Initialize the service manager, load unit files, and prepare boot order.
pub fn init() {
    let mut guard = SVC_MGR.lock();
    *guard = Some(ServiceManagerInner::new());
    serial_println!("[init_system::service_mgr] service manager initialized");
}

/// Register a service.
pub fn register(name: &str) -> usize {
    let mut guard = SVC_MGR.lock();
    let mgr = guard.as_mut().expect("service manager not initialized");
    mgr.register(name)
}

/// Add a dependency between two registered services.
pub fn add_dependency(service: &str, dependency: &str) -> Result<(), ()> {
    let mut guard = SVC_MGR.lock();
    let mgr = guard.as_mut().expect("service manager not initialized");
    mgr.add_dependency(service, dependency)
}

/// Enable a service for auto-start.
pub fn enable(name: &str) -> Result<(), ()> {
    let mut guard = SVC_MGR.lock();
    let mgr = guard.as_mut().expect("service manager not initialized");
    let idx = mgr.find_index(name).ok_or(())?;
    mgr.services[idx].enabled = true;
    Ok(())
}

/// Set auto-restart on failure for a service.
pub fn set_auto_restart(name: &str, enabled: bool) -> Result<(), ()> {
    let mut guard = SVC_MGR.lock();
    let mgr = guard.as_mut().expect("service manager not initialized");
    let idx = mgr.find_index(name).ok_or(())?;
    mgr.services[idx].auto_restart = enabled;
    Ok(())
}

/// Start a service by name.
pub fn start(name: &str) -> Result<(), ()> {
    let mut guard = SVC_MGR.lock();
    let mgr = guard.as_mut().expect("service manager not initialized");
    mgr.start(name)
}

/// Stop a service by name.
pub fn stop(name: &str) -> Result<(), ()> {
    let mut guard = SVC_MGR.lock();
    let mgr = guard.as_mut().expect("service manager not initialized");
    mgr.stop(name)
}

/// Restart a service by name.
pub fn restart(name: &str) -> Result<(), ()> {
    let mut guard = SVC_MGR.lock();
    let mgr = guard.as_mut().expect("service manager not initialized");
    mgr.restart(name)
}

/// Start all enabled services in dependency order.
pub fn start_all() {
    let mut guard = SVC_MGR.lock();
    let mgr = guard.as_mut().expect("service manager not initialized");
    mgr.start_all_enabled();
}

/// Stop all running services in reverse dependency order.
pub fn stop_all() {
    let mut guard = SVC_MGR.lock();
    let mgr = guard.as_mut().expect("service manager not initialized");
    mgr.stop_all();
}

/// Notify the manager that a service has exited.
pub fn on_exit(name: &str, exit_code: i32) {
    let mut guard = SVC_MGR.lock();
    let mgr = guard.as_mut().expect("service manager not initialized");
    mgr.on_service_exit(name, exit_code);
}

/// Get number of running services.
pub fn running_count() -> usize {
    let guard = SVC_MGR.lock();
    let mgr = guard.as_ref().expect("service manager not initialized");
    mgr.running_count()
}

/// Get number of failed services.
pub fn failed_count() -> usize {
    let guard = SVC_MGR.lock();
    let mgr = guard.as_ref().expect("service manager not initialized");
    mgr.failed_count()
}

/// Get state of a specific service.
pub fn get_state(name: &str) -> Option<ServiceState> {
    let guard = SVC_MGR.lock();
    let mgr = guard.as_ref().expect("service manager not initialized");
    let idx = mgr.find_index(name)?;
    Some(mgr.services[idx].state)
}

/// Get total number of registered services.
pub fn total_count() -> usize {
    let guard = SVC_MGR.lock();
    let mgr = guard.as_ref().expect("service manager not initialized");
    mgr.services.len()
}
