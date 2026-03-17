/// Service health monitoring manager (high-level watchdog orchestrator)
///
/// Part of the AIOS init_system subsystem.
///
/// The WatchdogManager is the top-level orchestrator that integrates
/// watchdog monitoring with the service manager. It periodically checks
/// all monitored services, handles restart decisions with exponential
/// backoff, and provides aggregate health reporting. This module ties
/// together watchdog.rs (low-level heartbeat tracking) with service_mgr.rs
/// (service lifecycle).
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

// ── TSC helpers ────────────────────────────────────────────────────────────

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

const TSC_PER_MS: u64 = 2_000_000;

fn ms_to_tsc(ms: u64) -> u64 {
    ms.saturating_mul(TSC_PER_MS)
}

// ── Health status ──────────────────────────────────────────────────────────

/// Overall health assessment for a monitored service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Service is healthy (heartbeats within timeout).
    Healthy,
    /// Service is degraded (heartbeat is late but not timed out).
    Degraded,
    /// Service has timed out.
    TimedOut,
    /// Service is not being monitored.
    Unknown,
}

// ── Restart policy ─────────────────────────────────────────────────────────

/// Configuration for restart behavior on timeout.
#[derive(Clone)]
struct RestartConfig {
    /// Base delay before restarting (ms).
    base_delay_ms: u64,
    /// Maximum delay after exponential backoff (ms).
    max_delay_ms: u64,
    /// Current backoff multiplier (doubles on each consecutive failure).
    backoff_factor: u32,
    /// Maximum number of restart attempts.
    max_attempts: u32,
    /// Current attempt count.
    attempt_count: u32,
    /// TSC timestamp when the service is eligible for restart.
    next_restart_tsc: u64,
    /// Whether a restart is pending.
    restart_pending: bool,
}

impl RestartConfig {
    fn new(base_delay_ms: u64, max_delay_ms: u64, max_attempts: u32) -> Self {
        RestartConfig {
            base_delay_ms,
            max_delay_ms,
            backoff_factor: 0,
            max_attempts,
            attempt_count: 0,
            next_restart_tsc: 0,
            restart_pending: false,
        }
    }

    /// Calculate the next restart delay with exponential backoff.
    fn next_delay_ms(&self) -> u64 {
        let delay = self.base_delay_ms << self.backoff_factor;
        if delay > self.max_delay_ms {
            self.max_delay_ms
        } else {
            delay
        }
    }

    /// Record a restart attempt and advance backoff.
    fn record_attempt(&mut self) {
        self.attempt_count = self.attempt_count.saturating_add(1);
        if self.backoff_factor < 10 {
            self.backoff_factor = self.backoff_factor.saturating_add(1);
        }
        let delay = self.next_delay_ms();
        self.next_restart_tsc = read_tsc() + ms_to_tsc(delay);
        self.restart_pending = true;
    }

    /// Reset backoff after a successful heartbeat.
    fn reset(&mut self) {
        self.backoff_factor = 0;
        self.attempt_count = 0;
        self.restart_pending = false;
    }

    /// Check if restart attempts are exhausted.
    fn exhausted(&self) -> bool {
        self.attempt_count >= self.max_attempts
    }
}

// ── Monitored service entry ────────────────────────────────────────────────

#[derive(Clone)]
struct MonitoredEntry {
    name: String,
    name_hash: u64,
    /// Timeout in TSC ticks.
    timeout_tsc: u64,
    /// Timeout in milliseconds (for logging/API).
    timeout_ms: u64,
    /// TSC of last heartbeat.
    last_heartbeat: u64,
    /// Whether monitoring is active.
    active: bool,
    /// Current health status.
    health: HealthStatus,
    /// Restart configuration.
    restart_cfg: RestartConfig,
    /// Total number of restarts triggered by this watchdog.
    total_restarts: u32,
}

// ── Watchdog manager ───────────────────────────────────────────────────────

/// Monitors service health and restarts failed services.
struct WatchdogManagerInner {
    monitored: Vec<MonitoredEntry>,
    /// Services that the caller should restart.
    restart_queue: Vec<String>,
    /// Global monitoring enable flag.
    enabled: bool,
    /// Check interval in TSC ticks.
    check_interval_tsc: u64,
    /// TSC of last check.
    last_check_tsc: u64,
}

impl WatchdogManagerInner {
    fn new() -> Self {
        WatchdogManagerInner {
            monitored: Vec::new(),
            restart_queue: Vec::new(),
            enabled: true,
            check_interval_tsc: ms_to_tsc(1000), // check every 1s
            last_check_tsc: read_tsc(),
        }
    }

    /// Register a service for watchdog monitoring with a timeout.
    fn monitor(&mut self, service: &str, timeout_ms: u64) {
        let hash = fnv1a_hash(service.as_bytes());

        // Update existing
        for entry in self.monitored.iter_mut() {
            if entry.name_hash == hash {
                entry.timeout_tsc = ms_to_tsc(timeout_ms);
                entry.timeout_ms = timeout_ms;
                entry.last_heartbeat = read_tsc();
                entry.active = true;
                entry.health = HealthStatus::Healthy;
                entry.restart_cfg.reset();
                return;
            }
        }

        let now = read_tsc();
        self.monitored.push(MonitoredEntry {
            name: String::from(service),
            name_hash: hash,
            timeout_tsc: ms_to_tsc(timeout_ms),
            timeout_ms,
            last_heartbeat: now,
            active: true,
            health: HealthStatus::Healthy,
            restart_cfg: RestartConfig::new(1000, 60000, 5),
            total_restarts: 0,
        });

        serial_println!(
            "[init_system::watchdog_mgr] monitoring {} (timeout={}ms)",
            service, timeout_ms
        );
    }

    /// Notify the watchdog that a service is still alive.
    fn notify_alive(&mut self, service: &str) {
        let hash = fnv1a_hash(service.as_bytes());
        for entry in self.monitored.iter_mut() {
            if entry.name_hash == hash {
                entry.last_heartbeat = read_tsc();
                entry.health = HealthStatus::Healthy;
                // Reset backoff on successful heartbeat
                entry.restart_cfg.reset();
                return;
            }
        }
    }

    /// Stop monitoring a service.
    fn unmonitor(&mut self, service: &str) {
        let hash = fnv1a_hash(service.as_bytes());
        for entry in self.monitored.iter_mut() {
            if entry.name_hash == hash {
                entry.active = false;
                entry.health = HealthStatus::Unknown;
                return;
            }
        }
    }

    /// Check all monitored services and handle timeouts.
    fn check_all(&mut self) {
        if !self.enabled {
            return;
        }

        let now = read_tsc();

        // Throttle checks to avoid spinning
        if now < self.last_check_tsc + self.check_interval_tsc {
            return;
        }
        self.last_check_tsc = now;

        for entry in self.monitored.iter_mut() {
            if !entry.active {
                continue;
            }

            let elapsed = now.saturating_sub(entry.last_heartbeat);
            let half_timeout = entry.timeout_tsc / 2;

            if elapsed > entry.timeout_tsc {
                // Full timeout
                if entry.health != HealthStatus::TimedOut {
                    entry.health = HealthStatus::TimedOut;
                    serial_println!(
                        "[init_system::watchdog_mgr] {} timed out (elapsed={}ms, timeout={}ms)",
                        entry.name,
                        elapsed / TSC_PER_MS,
                        entry.timeout_ms
                    );
                }

                // Handle restart with backoff
                if !entry.restart_cfg.exhausted() {
                    if !entry.restart_cfg.restart_pending || now >= entry.restart_cfg.next_restart_tsc {
                        entry.restart_cfg.record_attempt();
                        entry.total_restarts = entry.total_restarts.saturating_add(1);
                        entry.last_heartbeat = now; // reset to avoid immediate re-trigger

                        serial_println!(
                            "[init_system::watchdog_mgr] restarting {} (attempt {}/{}, backoff={}ms)",
                            entry.name,
                            entry.restart_cfg.attempt_count,
                            entry.restart_cfg.max_attempts,
                            entry.restart_cfg.next_delay_ms()
                        );

                        self.restart_queue.push(entry.name.clone());
                    }
                } else {
                    serial_println!(
                        "[init_system::watchdog_mgr] {} restart attempts exhausted ({}), giving up",
                        entry.name, entry.restart_cfg.max_attempts
                    );
                    entry.active = false;
                }
            } else if elapsed > half_timeout {
                // Degraded: heartbeat is late but not yet timed out
                if entry.health == HealthStatus::Healthy {
                    entry.health = HealthStatus::Degraded;
                    serial_println!(
                        "[init_system::watchdog_mgr] {} degraded (heartbeat late)",
                        entry.name
                    );
                }
            } else {
                entry.health = HealthStatus::Healthy;
            }
        }
    }

    /// Drain the restart queue.
    fn drain_restarts(&mut self) -> Vec<String> {
        let result = self.restart_queue.clone();
        self.restart_queue.clear();
        result
    }

    /// Get health status of a specific service.
    fn get_health(&self, service: &str) -> HealthStatus {
        let hash = fnv1a_hash(service.as_bytes());
        for entry in &self.monitored {
            if entry.name_hash == hash {
                return entry.health;
            }
        }
        HealthStatus::Unknown
    }

    /// Get count of healthy services.
    fn healthy_count(&self) -> usize {
        self.monitored.iter()
            .filter(|e| e.active && e.health == HealthStatus::Healthy)
            .count()
    }

    /// Get count of degraded services.
    fn degraded_count(&self) -> usize {
        self.monitored.iter()
            .filter(|e| e.active && e.health == HealthStatus::Degraded)
            .count()
    }

    /// Get count of timed-out services.
    fn timedout_count(&self) -> usize {
        self.monitored.iter()
            .filter(|e| e.active && e.health == HealthStatus::TimedOut)
            .count()
    }

    /// Get total restarts triggered across all services.
    fn total_restarts(&self) -> u32 {
        self.monitored.iter().map(|e| e.total_restarts).sum()
    }
}

/// Public wrapper matching original stub API.
pub struct WatchdogManager;

impl WatchdogManager {
    pub fn new() -> Self {
        WatchdogManager
    }

    pub fn monitor(&mut self, service: &str, timeout_ms: u64) {
        monitor(service, timeout_ms);
    }

    pub fn notify_alive(&mut self, service: &str) {
        notify_alive(service);
    }

    pub fn check_all(&mut self) {
        check_all();
    }
}

// ── Global state ───────────────────────────────────────────────────────────

static WD_MGR: Mutex<Option<WatchdogManagerInner>> = Mutex::new(None);

/// Initialize the watchdog manager.
pub fn init() {
    let mut guard = WD_MGR.lock();
    *guard = Some(WatchdogManagerInner::new());
    serial_println!("[init_system::watchdog_mgr] watchdog manager initialized");
}

/// Register a service for monitoring.
pub fn monitor(service: &str, timeout_ms: u64) {
    let mut guard = WD_MGR.lock();
    let mgr = guard.as_mut().expect("watchdog manager not initialized");
    mgr.monitor(service, timeout_ms);
}

/// Notify that a service is alive.
pub fn notify_alive(service: &str) {
    let mut guard = WD_MGR.lock();
    let mgr = guard.as_mut().expect("watchdog manager not initialized");
    mgr.notify_alive(service);
}

/// Stop monitoring a service.
pub fn unmonitor(service: &str) {
    let mut guard = WD_MGR.lock();
    let mgr = guard.as_mut().expect("watchdog manager not initialized");
    mgr.unmonitor(service);
}

/// Check all monitored services.
pub fn check_all() {
    let mut guard = WD_MGR.lock();
    let mgr = guard.as_mut().expect("watchdog manager not initialized");
    mgr.check_all();
}

/// Drain services needing restart.
pub fn drain_restarts() -> Vec<String> {
    let mut guard = WD_MGR.lock();
    let mgr = guard.as_mut().expect("watchdog manager not initialized");
    mgr.drain_restarts()
}

/// Get health of a specific service.
pub fn get_health(service: &str) -> HealthStatus {
    let guard = WD_MGR.lock();
    let mgr = guard.as_ref().expect("watchdog manager not initialized");
    mgr.get_health(service)
}

/// Get count of healthy monitored services.
pub fn healthy_count() -> usize {
    let guard = WD_MGR.lock();
    let mgr = guard.as_ref().expect("watchdog manager not initialized");
    mgr.healthy_count()
}

/// Get count of degraded monitored services.
pub fn degraded_count() -> usize {
    let guard = WD_MGR.lock();
    let mgr = guard.as_ref().expect("watchdog manager not initialized");
    mgr.degraded_count()
}

/// Get count of timed-out services.
pub fn timedout_count() -> usize {
    let guard = WD_MGR.lock();
    let mgr = guard.as_ref().expect("watchdog manager not initialized");
    mgr.timedout_count()
}

/// Get total restarts across all services.
pub fn total_restarts() -> u32 {
    let guard = WD_MGR.lock();
    let mgr = guard.as_ref().expect("watchdog manager not initialized");
    mgr.total_restarts()
}

/// Enable or disable global monitoring.
pub fn set_enabled(enabled: bool) {
    let mut guard = WD_MGR.lock();
    let mgr = guard.as_mut().expect("watchdog manager not initialized");
    mgr.enabled = enabled;
}
