use super::store::PrefValue;
use crate::sync::Mutex;
/// Change observers for Genesis preferences
///
/// Subscribe to preference changes by key pattern or namespace.
/// Supports:
///   - Per-key observers (exact match)
///   - Namespace-wide observers (prefix match)
///   - Global observers (all changes)
///   - Batch notifications (coalesce rapid changes)
///   - Listener priority levels
///   - Observer enable/disable without unregistering
///
/// Observers are identified by a u32 handle returned at registration.
/// Callbacks are represented as function pointers (no closures in no_std
/// without alloc::boxed::Box<dyn Fn>).
use crate::{serial_print, serial_println};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// The scope of an observer — what keys it watches
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObserverScope {
    /// Watch a single exact key
    Key(String),
    /// Watch all keys in a namespace (prefix match on "namespace.")
    Namespace(String),
    /// Watch all preference changes globally
    Global,
}

impl ObserverScope {
    /// Check if a given key matches this scope
    pub fn matches(&self, key: &str) -> bool {
        match self {
            ObserverScope::Key(k) => k.as_str() == key,
            ObserverScope::Namespace(ns) => {
                let prefix = format!("{}.", ns);
                key.starts_with(prefix.as_str())
            }
            ObserverScope::Global => true,
        }
    }
}

/// Priority level for observer dispatch ordering
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ObserverPriority {
    /// Dispatched first — system-critical observers
    Critical = 0,
    /// Dispatched second — high importance
    High = 1,
    /// Default priority
    Normal = 2,
    /// Dispatched last — low importance
    Low = 3,
}

/// A single change event
#[derive(Clone, Debug)]
pub struct PrefChangeEvent {
    /// The key that changed
    pub key: String,
    /// The previous value (None if new key)
    pub old_value: Option<PrefValue>,
    /// The new value (None if key was removed)
    pub new_value: Option<PrefValue>,
    /// Monotonic tick at which the change occurred
    pub tick: u64,
}

/// Registered observer entry
#[derive(Clone, Debug)]
pub struct Observer {
    /// Unique handle for this observer
    pub handle: u32,
    /// What keys this observer watches
    pub scope: ObserverScope,
    /// Dispatch priority
    pub priority: ObserverPriority,
    /// Whether this observer is currently enabled
    pub enabled: bool,
    /// Human-readable label for debugging
    pub label: String,
    /// Number of times this observer has been notified
    pub notify_count: u64,
}

/// A pending change waiting to be dispatched in a batch
#[derive(Clone, Debug)]
struct PendingChange {
    event: PrefChangeEvent,
    dispatched: bool,
}

/// The observer manager — tracks all registered observers and pending events
pub struct ObserverManager {
    /// All registered observers, sorted by priority at dispatch time
    observers: Vec<Observer>,
    /// Next handle to assign
    next_handle: u32,
    /// Pending change events awaiting batch dispatch
    pending: Vec<PendingChange>,
    /// Whether batch mode is active (changes accumulate instead of dispatching)
    batch_mode: bool,
    /// Total events dispatched since init
    total_dispatched: u64,
    /// Total events dropped (no matching observer)
    total_dropped: u64,
    /// Maximum pending queue size before forced flush
    max_pending: usize,
    /// Matched observer handles from the last dispatch (for polling consumers)
    last_matched: Vec<u32>,
}

impl ObserverManager {
    pub fn new() -> Self {
        Self {
            observers: vec![],
            next_handle: 1,
            pending: vec![],
            batch_mode: false,
            total_dispatched: 0,
            total_dropped: 0,
            max_pending: 256,
            last_matched: vec![],
        }
    }

    /// Register a new observer. Returns its unique handle.
    pub fn register(
        &mut self,
        scope: ObserverScope,
        priority: ObserverPriority,
        label: &str,
    ) -> u32 {
        let handle = self.next_handle;
        self.next_handle = self.next_handle.saturating_add(1);

        self.observers.push(Observer {
            handle,
            scope,
            priority,
            enabled: true,
            label: String::from(label),
            notify_count: 0,
        });

        serial_println!("[OBSERVER] Registered observer {} '{}'", handle, label);
        handle
    }

    /// Register a key-specific observer at normal priority
    pub fn watch_key(&mut self, key: &str, label: &str) -> u32 {
        self.register(
            ObserverScope::Key(String::from(key)),
            ObserverPriority::Normal,
            label,
        )
    }

    /// Register a namespace-wide observer at normal priority
    pub fn watch_namespace(&mut self, namespace: &str, label: &str) -> u32 {
        self.register(
            ObserverScope::Namespace(String::from(namespace)),
            ObserverPriority::Normal,
            label,
        )
    }

    /// Register a global observer at normal priority
    pub fn watch_all(&mut self, label: &str) -> u32 {
        self.register(ObserverScope::Global, ObserverPriority::Normal, label)
    }

    /// Unregister an observer by handle. Returns true if found and removed.
    pub fn unregister(&mut self, handle: u32) -> bool {
        if let Some(pos) = self.observers.iter().position(|o| o.handle == handle) {
            let label = self.observers[pos].label.clone();
            self.observers.remove(pos);
            serial_println!("[OBSERVER] Unregistered observer {} '{}'", handle, label);
            true
        } else {
            false
        }
    }

    /// Enable or disable an observer without unregistering it
    pub fn set_enabled(&mut self, handle: u32, enabled: bool) -> bool {
        if let Some(obs) = self.observers.iter_mut().find(|o| o.handle == handle) {
            obs.enabled = enabled;
            serial_println!(
                "[OBSERVER] Observer {} '{}' {}",
                handle,
                obs.label,
                if enabled { "enabled" } else { "disabled" }
            );
            true
        } else {
            false
        }
    }

    /// Begin batch mode — changes accumulate instead of dispatching immediately
    pub fn begin_batch(&mut self) {
        self.batch_mode = true;
        serial_println!("[OBSERVER] Batch mode started");
    }

    /// End batch mode and dispatch all pending changes
    pub fn end_batch(&mut self) -> u32 {
        self.batch_mode = false;
        let dispatched = self.flush_pending();
        serial_println!(
            "[OBSERVER] Batch mode ended, dispatched {} events",
            dispatched
        );
        dispatched
    }

    /// Notify observers of a preference change
    pub fn notify(&mut self, event: PrefChangeEvent) {
        if self.batch_mode {
            // Accumulate in pending queue
            self.pending.push(PendingChange {
                event,
                dispatched: false,
            });

            // Force flush if pending queue is too large
            if self.pending.len() >= self.max_pending {
                serial_println!("[OBSERVER] Pending queue full, forcing flush");
                self.flush_pending();
            }
            return;
        }

        // Immediate dispatch
        self.dispatch_event(&event);
    }

    /// Dispatch a single event to all matching observers
    fn dispatch_event(&mut self, event: &PrefChangeEvent) {
        // Collect matching observers sorted by priority
        let mut matched_indices: Vec<usize> = self
            .observers
            .iter()
            .enumerate()
            .filter(|(_, o)| o.enabled && o.scope.matches(event.key.as_str()))
            .map(|(i, _)| i)
            .collect();

        // Sort by priority (Critical first, Low last)
        matched_indices
            .sort_by(|&a, &b| self.observers[a].priority.cmp(&self.observers[b].priority));

        if matched_indices.is_empty() {
            self.total_dropped = self.total_dropped.saturating_add(1);
            return;
        }

        self.last_matched.clear();

        for idx in matched_indices {
            self.observers[idx].notify_count = self.observers[idx].notify_count.saturating_add(1);
            self.last_matched.push(self.observers[idx].handle);
            self.total_dispatched = self.total_dispatched.saturating_add(1);

            serial_println!(
                "[OBSERVER] Notified observer {} '{}' of change to '{}'",
                self.observers[idx].handle,
                self.observers[idx].label,
                event.key
            );
        }
    }

    /// Flush all pending changes (used at end of batch or on overflow)
    fn flush_pending(&mut self) -> u32 {
        let pending: Vec<PendingChange> = self.pending.drain(..).collect();
        let mut count = 0u32;
        for p in &pending {
            if !p.dispatched {
                self.dispatch_event(&p.event);
                count += 1;
            }
        }
        count
    }

    /// Get the list of observer handles matched in the last dispatch
    pub fn last_matched_handles(&self) -> &[u32] {
        &self.last_matched
    }

    /// Get an observer by handle
    pub fn get_observer(&self, handle: u32) -> Option<&Observer> {
        self.observers.iter().find(|o| o.handle == handle)
    }

    /// List all registered observers
    pub fn list_observers(&self) -> &[Observer] {
        &self.observers
    }

    /// Count of active (enabled) observers
    pub fn active_count(&self) -> usize {
        self.observers.iter().filter(|o| o.enabled).count()
    }

    /// Count of all registered observers
    pub fn total_count(&self) -> usize {
        self.observers.len()
    }

    /// Get dispatch statistics: (total_dispatched, total_dropped, pending_count)
    pub fn stats(&self) -> (u64, u64, usize) {
        (
            self.total_dispatched,
            self.total_dropped,
            self.pending.len(),
        )
    }

    /// Set the maximum pending queue size
    pub fn set_max_pending(&mut self, max: usize) {
        self.max_pending = max;
    }

    /// Remove all observers whose notify_count is zero (never triggered)
    pub fn prune_unused(&mut self) -> u32 {
        let before = self.observers.len();
        self.observers.retain(|o| o.notify_count > 0);
        let pruned = (before - self.observers.len()) as u32;
        if pruned > 0 {
            serial_println!("[OBSERVER] Pruned {} unused observers", pruned);
        }
        pruned
    }

    /// Find all observers watching a specific key (for introspection)
    pub fn observers_for_key(&self, key: &str) -> Vec<u32> {
        self.observers
            .iter()
            .filter(|o| o.scope.matches(key))
            .map(|o| o.handle)
            .collect()
    }

    /// Coalesce pending events: if multiple changes to the same key are pending,
    /// keep only the last one (most recent value wins)
    pub fn coalesce_pending(&mut self) -> u32 {
        if self.pending.len() <= 1 {
            return 0;
        }

        let mut seen: Vec<String> = vec![];
        let mut coalesced = 0u32;

        // Walk backwards — mark earlier duplicates as dispatched (skip them)
        for i in (0..self.pending.len()).rev() {
            let key = self.pending[i].event.key.clone();
            if seen.iter().any(|s| s.as_str() == key.as_str()) {
                self.pending[i].dispatched = true;
                coalesced += 1;
            } else {
                seen.push(key);
            }
        }

        if coalesced > 0 {
            serial_println!(
                "[OBSERVER] Coalesced {} duplicate pending events",
                coalesced
            );
        }
        coalesced
    }
}

static OBSERVER_MGR: Mutex<Option<ObserverManager>> = Mutex::new(None);

/// Initialize the observer manager
pub fn init() {
    let mut lock = OBSERVER_MGR.lock();
    *lock = Some(ObserverManager::new());
    serial_println!("[OBSERVER] Observer manager initialized");
}

/// Get a reference to the global observer manager
pub fn get_manager() -> &'static Mutex<Option<ObserverManager>> {
    &OBSERVER_MGR
}
