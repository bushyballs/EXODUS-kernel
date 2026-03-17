/// Hoags Service Registry — service discovery, registration, and health checks
///
/// Central registry for all OS services (filesystem, network, display, etc.).
/// Services register with capabilities, version info, and health endpoints.
/// Consumers look up services by capability, perform dependency injection,
/// and monitor service health. Supports graceful degradation when services
/// are unavailable.
///
/// Each service has a state machine: Registered -> Starting -> Running ->
/// Stopping -> Stopped. Health checks run periodically and can trigger
/// automatic restarts.
///
/// All numeric values use i32 Q16 fixed-point (65536 = 1.0).
/// No external crates. No f32/f64.
///
/// Inspired by: Kubernetes service discovery, Android ServiceManager,
/// Consul, mDNS/DNS-SD. All code is original.

use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

/// Q16 fixed-point: 65536 = 1.0
type Q16 = i32;
const Q16_ONE: Q16 = 65536;

/// Maximum registered services
const MAX_SERVICES: usize = 256;
/// Maximum capabilities per service
const MAX_CAPABILITIES: usize = 16;
/// Maximum dependencies per service
const MAX_DEPENDENCIES: usize = 16;
/// Maximum health check history entries per service
const MAX_HEALTH_HISTORY: usize = 32;
/// Maximum dependency injection bindings
const MAX_BINDINGS: usize = 512;
/// Health check interval threshold in ticks
const HEALTH_CHECK_INTERVAL: u64 = 1000;

// ---------------------------------------------------------------------------
// ServiceState — lifecycle state machine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// Service is registered but not yet started
    Registered,
    /// Service is in the process of starting
    Starting,
    /// Service is running and healthy
    Running,
    /// Service is degraded (partially functional)
    Degraded,
    /// Service is in the process of stopping
    Stopping,
    /// Service has stopped cleanly
    Stopped,
    /// Service has failed and needs restart
    Failed,
}

impl ServiceState {
    fn is_available(&self) -> bool {
        matches!(self, ServiceState::Running | ServiceState::Degraded)
    }

    fn as_code(&self) -> u8 {
        match self {
            ServiceState::Registered => 0,
            ServiceState::Starting => 1,
            ServiceState::Running => 2,
            ServiceState::Degraded => 3,
            ServiceState::Stopping => 4,
            ServiceState::Stopped => 5,
            ServiceState::Failed => 6,
        }
    }
}

// ---------------------------------------------------------------------------
// HealthStatus — result of a health check
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Service is healthy and responsive
    Healthy,
    /// Service is responding but with degraded performance
    Warning,
    /// Service is unresponsive or returning errors
    Unhealthy,
    /// Health check has not been run yet
    Unknown,
}

impl HealthStatus {
    fn as_q16(&self) -> Q16 {
        match self {
            HealthStatus::Healthy => Q16_ONE,
            HealthStatus::Warning => Q16_ONE / 2,
            HealthStatus::Unhealthy => 0,
            HealthStatus::Unknown => Q16_ONE / 4,
        }
    }
}

// ---------------------------------------------------------------------------
// Capability — what a service can do
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    /// Hash of the capability name (e.g., hash of "filesystem.read")
    pub name_hash: u64,
    /// Version of this capability (Q16)
    pub version: Q16,
}

// ---------------------------------------------------------------------------
// ServiceEntry — a registered service
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ServiceEntry {
    id: u32,
    name_hash: u64,
    version: u32,
    state: ServiceState,
    capabilities: Vec<Capability>,
    dependencies: Vec<u64>,
    health: HealthStatus,
    health_history: Vec<(u64, HealthStatus)>,
    last_health_check: u64,
    start_time: u64,
    restart_count: u32,
    max_restarts: u32,
    auto_restart: bool,
    endpoint_hash: u64,
    priority: Q16,
}

impl ServiceEntry {
    fn new(id: u32, name_hash: u64, version: u32) -> Self {
        ServiceEntry {
            id,
            name_hash,
            version,
            state: ServiceState::Registered,
            capabilities: Vec::new(),
            dependencies: Vec::new(),
            health: HealthStatus::Unknown,
            health_history: Vec::new(),
            last_health_check: 0,
            start_time: 0,
            restart_count: 0,
            max_restarts: 5,
            auto_restart: true,
            endpoint_hash: 0,
            priority: Q16_ONE / 2,
        }
    }
}

// ---------------------------------------------------------------------------
// DependencyBinding — maps a capability to a providing service
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct DependencyBinding {
    /// Capability being requested
    capability_hash: u64,
    /// Service that provides this capability
    provider_id: u32,
    /// Consumer service that needs this capability
    consumer_id: u32,
    /// Whether this binding is satisfied (provider is running)
    satisfied: bool,
}

// ---------------------------------------------------------------------------
// RegistryState — main state
// ---------------------------------------------------------------------------

struct RegistryState {
    services: Vec<ServiceEntry>,
    bindings: Vec<DependencyBinding>,
    next_id: u32,
    total_health_checks: u64,
    total_restarts: u64,
    initialized: bool,
}

impl RegistryState {
    fn new() -> Self {
        RegistryState {
            services: Vec::new(),
            bindings: Vec::new(),
            next_id: 1,
            total_health_checks: 0,
            total_restarts: 0,
            initialized: false,
        }
    }
}

static REGISTRY: Mutex<Option<RegistryState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Service registration
// ---------------------------------------------------------------------------

/// Register a new service. Returns the service ID.
pub fn register(
    name_hash: u64,
    version: u32,
    capabilities: Vec<Capability>,
    dependencies: Vec<u64>,
    auto_restart: bool,
) -> u32 {
    let mut guard = REGISTRY.lock();
    if let Some(ref mut state) = *guard {
        if state.services.len() >= MAX_SERVICES {
            serial_println!("[service_reg] ERROR: max services ({}) reached", MAX_SERVICES);
            return 0;
        }

        // Check for duplicate
        for svc in &state.services {
            if svc.name_hash == name_hash {
                serial_println!("[service_reg] ERROR: service already registered (name={:#018X})",
                    name_hash);
                return 0;
            }
        }

        let id = state.next_id;
        state.next_id = state.next_id.saturating_add(1);

        let mut entry = ServiceEntry::new(id, name_hash, version);
        entry.auto_restart = auto_restart;

        for cap in capabilities {
            if entry.capabilities.len() < MAX_CAPABILITIES {
                entry.capabilities.push(cap);
            }
        }
        for dep in dependencies {
            if entry.dependencies.len() < MAX_DEPENDENCIES {
                entry.dependencies.push(dep);
            }
        }

        serial_println!("[service_reg] Registered service {} (name={:#018X}, v{}, {} caps, {} deps)",
            id, name_hash, version, entry.capabilities.len(), entry.dependencies.len());

        state.services.push(entry);
        id
    } else {
        0
    }
}

/// Unregister a service. Returns true if found and removed.
pub fn unregister(service_id: u32) -> bool {
    let mut guard = REGISTRY.lock();
    if let Some(ref mut state) = *guard {
        // Remove bindings referencing this service
        state.bindings.retain(|b| b.provider_id != service_id && b.consumer_id != service_id);

        let before = state.services.len();
        state.services.retain(|s| s.id != service_id);
        let removed = state.services.len() < before;
        if removed {
            serial_println!("[service_reg] Unregistered service {}", service_id);
        }
        removed
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Service lifecycle
// ---------------------------------------------------------------------------

/// Transition a service to a new state. Validates state transitions.
pub fn set_state(service_id: u32, new_state: ServiceState, timestamp: u64) -> bool {
    let mut guard = REGISTRY.lock();
    if let Some(ref mut state) = *guard {
        for svc in &mut state.services {
            if svc.id == service_id {
                // Validate state transition
                let valid = match (svc.state, new_state) {
                    (ServiceState::Registered, ServiceState::Starting) => true,
                    (ServiceState::Starting, ServiceState::Running) => true,
                    (ServiceState::Starting, ServiceState::Failed) => true,
                    (ServiceState::Running, ServiceState::Degraded) => true,
                    (ServiceState::Running, ServiceState::Stopping) => true,
                    (ServiceState::Running, ServiceState::Failed) => true,
                    (ServiceState::Degraded, ServiceState::Running) => true,
                    (ServiceState::Degraded, ServiceState::Stopping) => true,
                    (ServiceState::Degraded, ServiceState::Failed) => true,
                    (ServiceState::Stopping, ServiceState::Stopped) => true,
                    (ServiceState::Failed, ServiceState::Starting) => true,
                    (ServiceState::Stopped, ServiceState::Starting) => true,
                    _ => false,
                };

                if !valid {
                    serial_println!("[service_reg] ERROR: invalid transition {:?} -> {:?} for service {}",
                        svc.state, new_state, service_id);
                    return false;
                }

                let old_state = svc.state;
                svc.state = new_state;

                if new_state == ServiceState::Running {
                    svc.start_time = timestamp;
                    svc.health = HealthStatus::Healthy;
                }

                serial_println!("[service_reg] Service {} transitioned {:?} -> {:?}",
                    service_id, old_state, new_state);

                // Update binding satisfaction
                update_bindings_internal(state);
                return true;
            }
        }
    }
    false
}

/// Start a service (transitions Registered/Stopped/Failed -> Starting -> Running).
pub fn start_service(service_id: u32, timestamp: u64) -> bool {
    // Check dependencies first
    if !check_dependencies(service_id) {
        serial_println!("[service_reg] ERROR: unmet dependencies for service {}", service_id);
        return false;
    }
    if set_state(service_id, ServiceState::Starting, timestamp) {
        set_state(service_id, ServiceState::Running, timestamp)
    } else {
        false
    }
}

/// Stop a service gracefully.
pub fn stop_service(service_id: u32, timestamp: u64) -> bool {
    if set_state(service_id, ServiceState::Stopping, timestamp) {
        set_state(service_id, ServiceState::Stopped, timestamp)
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Health checks
// ---------------------------------------------------------------------------

/// Run a health check on a specific service.
pub fn health_check(service_id: u32, status: HealthStatus, timestamp: u64) -> bool {
    let mut guard = REGISTRY.lock();
    if let Some(ref mut state) = *guard {
        state.total_health_checks = state.total_health_checks.saturating_add(1);

        for svc in &mut state.services {
            if svc.id == service_id {
                svc.health = status;
                svc.last_health_check = timestamp;

                // Record in history
                if svc.health_history.len() >= MAX_HEALTH_HISTORY {
                    svc.health_history.remove(0);
                }
                svc.health_history.push((timestamp, status));

                // Handle unhealthy service
                if status == HealthStatus::Unhealthy && svc.state == ServiceState::Running {
                    svc.state = ServiceState::Failed;
                    serial_println!("[service_reg] Service {} failed health check, marking as Failed",
                        service_id);

                    // Auto-restart if enabled
                    if svc.auto_restart && svc.restart_count < svc.max_restarts {
                        svc.restart_count = svc.restart_count.saturating_add(1);
                        svc.state = ServiceState::Starting;
                        serial_println!("[service_reg] Auto-restarting service {} (attempt {}/{})",
                            service_id, svc.restart_count, svc.max_restarts);
                        svc.state = ServiceState::Running;
                        svc.start_time = timestamp;
                        svc.health = HealthStatus::Unknown;
                        state.total_restarts = state.total_restarts.saturating_add(1);
                    }
                } else if status == HealthStatus::Warning && svc.state == ServiceState::Running {
                    svc.state = ServiceState::Degraded;
                    serial_println!("[service_reg] Service {} degraded", service_id);
                } else if status == HealthStatus::Healthy && svc.state == ServiceState::Degraded {
                    svc.state = ServiceState::Running;
                    serial_println!("[service_reg] Service {} recovered", service_id);
                }

                return true;
            }
        }
    }
    false
}

/// Run health checks on all running services. Returns (healthy, degraded, failed).
pub fn health_check_all(timestamp: u64) -> (u32, u32, u32) {
    let guard = REGISTRY.lock();
    if let Some(ref state) = *guard {
        let mut healthy = 0u32;
        let mut degraded = 0u32;
        let mut failed = 0u32;

        for svc in &state.services {
            match svc.state {
                ServiceState::Running => healthy += 1,
                ServiceState::Degraded => degraded += 1,
                ServiceState::Failed => failed += 1,
                _ => {}
            }
        }

        (healthy, degraded, failed)
    } else {
        (0, 0, 0)
    }
}

// ---------------------------------------------------------------------------
// Dependency injection
// ---------------------------------------------------------------------------

/// Check if all dependencies for a service are satisfied.
pub fn check_dependencies(service_id: u32) -> bool {
    let guard = REGISTRY.lock();
    if let Some(ref state) = *guard {
        let deps = {
            let mut d = Vec::new();
            for svc in &state.services {
                if svc.id == service_id {
                    d = svc.dependencies.clone();
                    break;
                }
            }
            d
        };

        // Check each dependency capability is provided by a running service
        for dep_hash in &deps {
            let mut found = false;
            for svc in &state.services {
                if svc.id == service_id { continue; }
                if !svc.state.is_available() { continue; }
                for cap in &svc.capabilities {
                    if cap.name_hash == *dep_hash {
                        found = true;
                        break;
                    }
                }
                if found { break; }
            }
            if !found {
                serial_println!("[service_reg] Unmet dependency {:#018X} for service {}",
                    dep_hash, service_id);
                return false;
            }
        }

        true
    } else {
        false
    }
}

/// Create a dependency binding between a consumer and a provider.
pub fn bind(consumer_id: u32, capability_hash: u64) -> bool {
    let mut guard = REGISTRY.lock();
    if let Some(ref mut state) = *guard {
        if state.bindings.len() >= MAX_BINDINGS {
            serial_println!("[service_reg] ERROR: max bindings ({}) reached", MAX_BINDINGS);
            return false;
        }

        // Find a provider for this capability
        let mut provider_id = 0u32;
        let mut found = false;
        for svc in &state.services {
            if svc.id == consumer_id { continue; }
            for cap in &svc.capabilities {
                if cap.name_hash == capability_hash {
                    provider_id = svc.id;
                    found = true;
                    break;
                }
            }
            if found { break; }
        }

        if !found {
            serial_println!("[service_reg] No provider for capability {:#018X}", capability_hash);
            return false;
        }

        let satisfied = state.services.iter()
            .any(|s| s.id == provider_id && s.state.is_available());

        state.bindings.push(DependencyBinding {
            capability_hash,
            provider_id,
            consumer_id,
            satisfied,
        });

        serial_println!("[service_reg] Bound consumer {} -> provider {} (cap={:#018X}, {})",
            consumer_id, provider_id, capability_hash,
            if satisfied { "satisfied" } else { "pending" });
        true
    } else {
        false
    }
}

fn update_bindings_internal(state: &mut RegistryState) {
    for binding in &mut state.bindings {
        binding.satisfied = state.services.iter()
            .any(|s| s.id == binding.provider_id && s.state.is_available());
    }
}

// ---------------------------------------------------------------------------
// Lookup / Discovery
// ---------------------------------------------------------------------------

/// Look up a service by name hash. Returns (id, state_code, version, health_q16).
pub fn lookup_by_name(name_hash: u64) -> Option<(u32, u8, u32, Q16)> {
    let guard = REGISTRY.lock();
    if let Some(ref state) = *guard {
        for svc in &state.services {
            if svc.name_hash == name_hash {
                return Some((svc.id, svc.state.as_code(), svc.version, svc.health.as_q16()));
            }
        }
    }
    None
}

/// Find all services that provide a specific capability.
/// Returns list of (service_id, name_hash, version, state_code).
pub fn find_by_capability(capability_hash: u64) -> Vec<(u32, u64, u32, u8)> {
    let guard = REGISTRY.lock();
    if let Some(ref state) = *guard {
        let mut result = Vec::new();
        for svc in &state.services {
            for cap in &svc.capabilities {
                if cap.name_hash == capability_hash {
                    result.push((svc.id, svc.name_hash, svc.version, svc.state.as_code()));
                    break;
                }
            }
        }
        result
    } else {
        Vec::new()
    }
}

/// List all registered services as (id, name_hash, version, state_code, health_q16).
pub fn list_services() -> Vec<(u32, u64, u32, u8, Q16)> {
    let guard = REGISTRY.lock();
    if let Some(ref state) = *guard {
        let mut result = Vec::new();
        for svc in &state.services {
            result.push((svc.id, svc.name_hash, svc.version, svc.state.as_code(),
                svc.health.as_q16()));
        }
        result
    } else {
        Vec::new()
    }
}

/// Get the health history for a service as (timestamp, health_q16) pairs.
pub fn get_health_history(service_id: u32) -> Vec<(u64, Q16)> {
    let guard = REGISTRY.lock();
    if let Some(ref state) = *guard {
        for svc in &state.services {
            if svc.id == service_id {
                let mut result = Vec::new();
                for (ts, status) in &svc.health_history {
                    result.push((*ts, status.as_q16()));
                }
                return result;
            }
        }
    }
    Vec::new()
}

/// Get registry statistics: (total_services, running, total_checks, total_restarts).
pub fn registry_stats() -> (usize, usize, u64, u64) {
    let guard = REGISTRY.lock();
    if let Some(ref state) = *guard {
        let running = state.services.iter()
            .filter(|s| s.state == ServiceState::Running)
            .count();
        (state.services.len(), running, state.total_health_checks, state.total_restarts)
    } else {
        (0, 0, 0, 0)
    }
}

/// Compute overall system health as a Q16 value (weighted average of service health).
pub fn system_health_q16() -> Q16 {
    let guard = REGISTRY.lock();
    if let Some(ref state) = *guard {
        if state.services.is_empty() {
            return Q16_ONE;
        }
        let mut total: i64 = 0;
        let mut weight_sum: i64 = 0;
        for svc in &state.services {
            if svc.state == ServiceState::Stopped || svc.state == ServiceState::Registered {
                continue;
            }
            let health = svc.health.as_q16() as i64;
            let weight = svc.priority as i64;
            total += (health * weight) >> 16;
            weight_sum += weight as i64;
        }
        if weight_sum == 0 {
            return Q16_ONE;
        }
        (((total << 16) / weight_sum)) as Q16
    } else {
        Q16_ONE
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut guard = REGISTRY.lock();
    *guard = Some(RegistryState::new());
    if let Some(ref mut state) = *guard {
        state.initialized = true;
    }
    serial_println!("    [integration] Service registry initialized (discovery, health, dependency injection)");
}
