use super::SubsystemClass;
use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Register subsystem AI capabilities
///
/// Part of the Hoags Neural Bus. Tracks which subsystems offer
/// AI features so the cortex can orchestrate them dynamically.
///
/// Each subsystem can register one or more "capabilities" -- named
/// AI features that describe what the subsystem can do for the cortex.
/// The registry supports:
///   - Registration with metadata (node ID, class, name, description)
///   - Lookup by subsystem class
///   - Lookup by capability name (fuzzy substring match)
///   - Dependency tracking between capabilities
///   - Health status per capability
///   - Deregistration when a subsystem goes offline
use alloc::string::String;
use alloc::vec::Vec;

/// Health status of a registered capability
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityHealth {
    /// Fully operational
    Healthy,
    /// Operational but degraded (e.g. low memory)
    Degraded,
    /// Not responding, may recover
    Unhealthy,
    /// Permanently offline
    Offline,
}

/// Registered AI capability of a subsystem
pub struct AiCapability {
    /// Node ID on the neural bus
    pub node_id: u16,
    /// Subsystem class
    pub class: SubsystemClass,
    /// Short capability name (e.g. "text-generation", "image-classify")
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Current health status
    pub health: CapabilityHealth,
    /// Priority / quality score (higher = better)
    pub quality_score: u32,
    /// Dependencies: node IDs this capability depends on
    pub depends_on: Vec<u16>,
    /// How many times this capability has been invoked
    pub invocation_count: u64,
    /// Last invocation timestamp (monotonic counter)
    pub last_invoked: u64,
    /// Registration timestamp
    pub registered_at: u64,
    /// Version of this capability (for upgrades)
    pub version: u32,
}

pub struct SubsystemRegistry {
    /// All registered capabilities
    pub capabilities: Vec<AiCapability>,
    /// Maximum number of capabilities
    max_capabilities: usize,
    /// Monotonic counter for timestamps
    counter: u64,
}

impl SubsystemRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        SubsystemRegistry {
            capabilities: Vec::new(),
            max_capabilities: 256,
            counter: 0,
        }
    }

    /// Register an AI capability for a subsystem.
    pub fn register(&mut self, node_id: u16, class: SubsystemClass, name: &str) {
        self.register_full(node_id, class, name, "", 0);
    }

    /// Register with full metadata.
    pub fn register_full(
        &mut self,
        node_id: u16,
        class: SubsystemClass,
        name: &str,
        description: &str,
        version: u32,
    ) {
        // Check for duplicate (same node_id + name)
        for cap in &self.capabilities {
            if cap.node_id == node_id && cap.name == name {
                serial_println!(
                    "    [subsys-registry] Duplicate: node {} '{}' already registered",
                    node_id,
                    name
                );
                return;
            }
        }

        if self.capabilities.len() >= self.max_capabilities {
            // Evict the oldest offline capability
            self.evict_oldest_offline();
        }

        self.counter = self.counter.saturating_add(1);
        let cap = AiCapability {
            node_id,
            class,
            name: String::from(name),
            description: String::from(description),
            health: CapabilityHealth::Healthy,
            quality_score: 50, // Default mid-range quality
            depends_on: Vec::new(),
            invocation_count: 0,
            last_invoked: 0,
            registered_at: self.counter,
            version,
        };

        serial_println!(
            "    [subsys-registry] Registered: node {} {:?} '{}'",
            node_id,
            class,
            name
        );
        self.capabilities.push(cap);
    }

    /// Find capabilities matching a subsystem class.
    pub fn find_by_class(&self, class: SubsystemClass) -> Vec<&AiCapability> {
        self.capabilities
            .iter()
            .filter(|c| c.class == class && c.health != CapabilityHealth::Offline)
            .collect()
    }

    /// Find capabilities by name (substring match).
    pub fn find_by_name(&self, query: &str) -> Vec<&AiCapability> {
        let query_lower = to_lower(query);
        self.capabilities
            .iter()
            .filter(|c| {
                let name_lower = to_lower(&c.name);
                name_lower.contains(&query_lower) && c.health != CapabilityHealth::Offline
            })
            .collect()
    }

    /// Find capabilities by node ID.
    pub fn find_by_node(&self, node_id: u16) -> Vec<&AiCapability> {
        self.capabilities
            .iter()
            .filter(|c| c.node_id == node_id)
            .collect()
    }

    /// Get the best capability for a class (highest quality_score, healthy).
    pub fn best_for_class(&self, class: SubsystemClass) -> Option<&AiCapability> {
        self.capabilities
            .iter()
            .filter(|c| c.class == class && c.health == CapabilityHealth::Healthy)
            .max_by_key(|c| c.quality_score)
    }

    /// Record an invocation of a capability.
    pub fn record_invocation(&mut self, node_id: u16, name: &str) {
        self.counter = self.counter.saturating_add(1);
        let ts = self.counter;
        for cap in self.capabilities.iter_mut() {
            if cap.node_id == node_id && cap.name == name {
                cap.invocation_count = cap.invocation_count.saturating_add(1);
                cap.last_invoked = ts;
                return;
            }
        }
    }

    /// Update the health status of a capability.
    pub fn update_health(&mut self, node_id: u16, name: &str, health: CapabilityHealth) {
        for cap in self.capabilities.iter_mut() {
            if cap.node_id == node_id && cap.name == name {
                let old = cap.health;
                cap.health = health;
                if old != health {
                    serial_println!(
                        "    [subsys-registry] Health change: node {} '{}': {:?} -> {:?}",
                        node_id,
                        name,
                        old,
                        health
                    );
                }
                return;
            }
        }
    }

    /// Update the quality score for a capability.
    pub fn update_quality(&mut self, node_id: u16, name: &str, score: u32) {
        for cap in self.capabilities.iter_mut() {
            if cap.node_id == node_id && cap.name == name {
                cap.quality_score = score;
                return;
            }
        }
    }

    /// Add a dependency: capability (node_id, name) depends on dep_node_id.
    pub fn add_dependency(&mut self, node_id: u16, name: &str, dep_node_id: u16) {
        for cap in self.capabilities.iter_mut() {
            if cap.node_id == node_id && cap.name == name {
                if !cap.depends_on.contains(&dep_node_id) {
                    cap.depends_on.push(dep_node_id);
                }
                return;
            }
        }
    }

    /// Check if all dependencies of a capability are healthy.
    pub fn dependencies_healthy(&self, node_id: u16, name: &str) -> bool {
        let cap = self
            .capabilities
            .iter()
            .find(|c| c.node_id == node_id && c.name == name);
        if let Some(cap) = cap {
            for &dep_id in &cap.depends_on {
                let dep_healthy = self
                    .capabilities
                    .iter()
                    .filter(|c| c.node_id == dep_id)
                    .any(|c| c.health == CapabilityHealth::Healthy);
                if !dep_healthy {
                    return false;
                }
            }
            true
        } else {
            false
        }
    }

    /// Deregister all capabilities for a node (e.g. node going offline).
    pub fn deregister_node(&mut self, node_id: u16) {
        let before = self.capabilities.len();
        self.capabilities.retain(|c| c.node_id != node_id);
        let removed = before - self.capabilities.len();
        if removed > 0 {
            serial_println!(
                "    [subsys-registry] Deregistered {} capabilities for node {}",
                removed,
                node_id
            );
        }
    }

    /// Deregister a specific capability.
    pub fn deregister(&mut self, node_id: u16, name: &str) -> bool {
        let before = self.capabilities.len();
        self.capabilities
            .retain(|c| !(c.node_id == node_id && c.name == name));
        before != self.capabilities.len()
    }

    /// Get total number of registered capabilities.
    pub fn count(&self) -> usize {
        self.capabilities.len()
    }

    /// Get total number of healthy capabilities.
    pub fn healthy_count(&self) -> usize {
        self.capabilities
            .iter()
            .filter(|c| c.health == CapabilityHealth::Healthy)
            .count()
    }

    /// Get the most-invoked capabilities.
    pub fn most_used(&self, n: usize) -> Vec<&AiCapability> {
        let mut sorted: Vec<&AiCapability> = self.capabilities.iter().collect();
        sorted.sort_by(|a, b| b.invocation_count.cmp(&a.invocation_count));
        sorted.truncate(n);
        sorted
    }

    /// Evict the oldest offline capability.
    fn evict_oldest_offline(&mut self) {
        let mut oldest_idx = None;
        let mut oldest_ts = u64::MAX;
        for (i, cap) in self.capabilities.iter().enumerate() {
            if cap.health == CapabilityHealth::Offline && cap.registered_at < oldest_ts {
                oldest_ts = cap.registered_at;
                oldest_idx = Some(i);
            }
        }
        if let Some(idx) = oldest_idx {
            serial_println!(
                "    [subsys-registry] Evicting offline capability '{}'",
                self.capabilities[idx].name
            );
            self.capabilities.swap_remove(idx);
        } else {
            // No offline entries; evict the least-used
            let mut least_idx = 0;
            let mut least_count = u64::MAX;
            for (i, cap) in self.capabilities.iter().enumerate() {
                if cap.invocation_count < least_count {
                    least_count = cap.invocation_count;
                    least_idx = i;
                }
            }
            if !self.capabilities.is_empty() {
                serial_println!(
                    "    [subsys-registry] Evicting least-used capability '{}'",
                    self.capabilities[least_idx].name
                );
                self.capabilities.swap_remove(least_idx);
            }
        }
    }
}

/// Simple ASCII lowercase conversion
fn to_lower(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_uppercase() {
            out.push((ch as u8 + 32) as char);
        } else {
            out.push(ch);
        }
    }
    out
}

// ── Global Singleton ────────────────────────────────────────────────

struct RegistryState {
    registry: SubsystemRegistry,
}

static SUBSYS_REGISTRY: Mutex<Option<RegistryState>> = Mutex::new(None);

pub fn init() {
    let registry = SubsystemRegistry::new();
    let mut guard = SUBSYS_REGISTRY.lock();
    *guard = Some(RegistryState { registry });
    serial_println!("    [subsys-registry] Subsystem capability registry initialised");
}

/// Register a capability in the global registry.
pub fn register_global(node_id: u16, class: SubsystemClass, name: &str) {
    let mut guard = SUBSYS_REGISTRY.lock();
    if let Some(state) = guard.as_mut() {
        state.registry.register(node_id, class, name);
    }
}

/// Find capabilities by class in the global registry.
pub fn find_by_class_global(class: SubsystemClass) -> Vec<u16> {
    let guard = SUBSYS_REGISTRY.lock();
    if let Some(state) = guard.as_ref() {
        state
            .registry
            .find_by_class(class)
            .iter()
            .map(|c| c.node_id)
            .collect()
    } else {
        Vec::new()
    }
}
