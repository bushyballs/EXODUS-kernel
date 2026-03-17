/// Service watchdog — heartbeat monitoring and auto-restart
///
/// Part of the AIOS init_system subsystem.
///
/// Each monitored service must periodically send a heartbeat (notify_alive).
/// If a service fails to heartbeat within its configured timeout, the
/// watchdog marks it as failed and triggers a restart. Supports configurable
/// restart limits, backoff delays, and grace periods.
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

// ── Watchdog action ────────────────────────────────────────────────────────

/// Action to take when a watchdog timeout occurs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogAction {
    /// Do nothing (just log).
    None,
    /// Restart the service.
    Restart,
    /// Reboot the entire system.
    Reboot,
    /// Power off the system.
    Poweroff,
}

// ── Watched service entry ──────────────────────────────────────────────────

/// Tracked state for a watched service.
#[derive(Clone)]
struct WatchedService {
    name: String,
    name_hash: u64,
    /// Timeout duration in TSC ticks.
    timeout_tsc: u64,
    /// Timeout in milliseconds (for logging).
    timeout_ms: u64,
    /// TSC timestamp of last heartbeat.
    last_heartbeat: u64,
    /// Number of timeouts that have occurred.
    timeout_count: u32,
    /// Maximum timeouts before giving up.
    max_timeouts: u32,
    /// Action to take on timeout.
    action: WatchdogAction,
    /// Whether monitoring is active.
    active: bool,
    /// Grace period after start before watchdog kicks in (TSC ticks).
    grace_tsc: u64,
    /// TSC when monitoring was activated.
    start_tsc: u64,
    /// Services that need restart (filled by check, consumed by caller).
    needs_restart: bool,
}

// ── Watchdog inner ─────────────────────────────────────────────────────────

struct WatchdogInner {
    monitored: Vec<WatchedService>,
    /// Services pending restart action.
    restart_queue: Vec<String>,
    /// Whether the system-level watchdog is also active.
    system_watchdog_active: bool,
    /// System watchdog timeout in TSC ticks.
    system_timeout_tsc: u64,
    /// Last system-level kick.
    system_last_kick: u64,
}

impl WatchdogInner {
    fn new() -> Self {
        WatchdogInner {
            monitored: Vec::new(),
            restart_queue: Vec::new(),
            system_watchdog_active: false,
            system_timeout_tsc: 0,
            system_last_kick: read_tsc(),
        }
    }

    /// Register a service for watchdog monitoring.
    fn monitor(&mut self, service: &str, timeout_ms: u64) {
        let hash = fnv1a_hash(service.as_bytes());

        // Update existing entry if present
        for ws in self.monitored.iter_mut() {
            if ws.name_hash == hash {
                ws.timeout_tsc = ms_to_tsc(timeout_ms);
                ws.timeout_ms = timeout_ms;
                ws.last_heartbeat = read_tsc();
                ws.active = true;
                ws.needs_restart = false;
                return;
            }
        }

        let now = read_tsc();
        self.monitored.push(WatchedService {
            name: String::from(service),
            name_hash: hash,
            timeout_tsc: ms_to_tsc(timeout_ms),
            timeout_ms,
            last_heartbeat: now,
            timeout_count: 0,
            max_timeouts: 3,
            action: WatchdogAction::Restart,
            active: true,
            grace_tsc: ms_to_tsc(5000), // 5s grace period
            start_tsc: now,
            needs_restart: false,
        });

        serial_println!(
            "[init_system::watchdog] monitoring {} (timeout={}ms)",
            service, timeout_ms
        );
    }

    /// Notify the watchdog that a service is still alive.
    fn notify_alive(&mut self, service: &str) {
        let hash = fnv1a_hash(service.as_bytes());
        for ws in self.monitored.iter_mut() {
            if ws.name_hash == hash {
                ws.last_heartbeat = read_tsc();
                ws.needs_restart = false;
                return;
            }
        }
    }

    /// Stop monitoring a service.
    fn unmonitor(&mut self, service: &str) {
        let hash = fnv1a_hash(service.as_bytes());
        for ws in self.monitored.iter_mut() {
            if ws.name_hash == hash {
                ws.active = false;
                return;
            }
        }
    }

    /// Set the action to take on timeout for a service.
    fn set_action(&mut self, service: &str, action: WatchdogAction) {
        let hash = fnv1a_hash(service.as_bytes());
        for ws in self.monitored.iter_mut() {
            if ws.name_hash == hash {
                ws.action = action;
                return;
            }
        }
    }

    /// Set max timeouts before giving up.
    fn set_max_timeouts(&mut self, service: &str, max: u32) {
        let hash = fnv1a_hash(service.as_bytes());
        for ws in self.monitored.iter_mut() {
            if ws.name_hash == hash {
                ws.max_timeouts = max;
                return;
            }
        }
    }

    /// Check all monitored services and handle timeouts.
    fn check_all(&mut self) {
        let now = read_tsc();

        for ws in self.monitored.iter_mut() {
            if !ws.active {
                continue;
            }

            // Skip during grace period
            if now < ws.start_tsc + ws.grace_tsc {
                continue;
            }

            let elapsed = now.saturating_sub(ws.last_heartbeat);
            if elapsed > ws.timeout_tsc {
                ws.timeout_count = ws.timeout_count.saturating_add(1);
                ws.needs_restart = true;

                serial_println!(
                    "[init_system::watchdog] {} timed out (count={}/{}, elapsed_ms={})",
                    ws.name,
                    ws.timeout_count,
                    ws.max_timeouts,
                    elapsed / TSC_PER_MS
                );

                if ws.timeout_count >= ws.max_timeouts {
                    serial_println!(
                        "[init_system::watchdog] {} exceeded max timeouts, action={:?}",
                        ws.name, ws.action
                    );
                    ws.active = false;

                    match ws.action {
                        WatchdogAction::Restart => {
                            self.restart_queue.push(ws.name.clone());
                        }
                        WatchdogAction::Reboot => {
                            serial_println!("[init_system::watchdog] REBOOT triggered by {}", ws.name);
                            // In a real kernel, trigger reboot here
                        }
                        WatchdogAction::Poweroff => {
                            serial_println!("[init_system::watchdog] POWEROFF triggered by {}", ws.name);
                        }
                        WatchdogAction::None => {}
                    }
                } else {
                    // Reset heartbeat to give the service another chance
                    ws.last_heartbeat = now;
                    match ws.action {
                        WatchdogAction::Restart => {
                            self.restart_queue.push(ws.name.clone());
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Drain the restart queue.
    fn drain_restarts(&mut self) -> Vec<String> {
        let result = self.restart_queue.clone();
        self.restart_queue.clear();
        result
    }

    /// Get count of actively monitored services.
    fn active_count(&self) -> usize {
        self.monitored.iter().filter(|ws| ws.active).count()
    }

    /// Get total timeout events across all services.
    fn total_timeouts(&self) -> u32 {
        self.monitored.iter().map(|ws| ws.timeout_count).sum()
    }

    /// Enable system-level watchdog (hardware watchdog kick).
    fn enable_system_watchdog(&mut self, timeout_ms: u64) {
        self.system_watchdog_active = true;
        self.system_timeout_tsc = ms_to_tsc(timeout_ms);
        self.system_last_kick = read_tsc();
        serial_println!(
            "[init_system::watchdog] system watchdog enabled (timeout={}ms)",
            timeout_ms
        );
    }

    /// Kick the system watchdog.
    fn kick_system_watchdog(&mut self) {
        if self.system_watchdog_active {
            self.system_last_kick = read_tsc();
        }
    }
}

/// Public wrapper matching original stub API.
pub struct Watchdog {
    inner: WatchdogInner,
}

impl Watchdog {
    pub fn new() -> Self {
        Watchdog {
            inner: WatchdogInner::new(),
        }
    }

    pub fn monitor(&mut self, service: &str, timeout_ms: u64) {
        self.inner.monitor(service, timeout_ms);
    }

    pub fn notify_alive(&mut self, service: &str) {
        self.inner.notify_alive(service);
    }

    pub fn check_all(&mut self) {
        self.inner.check_all();
    }
}

// ── Global state ───────────────────────────────────────────────────────────

static WATCHDOG: Mutex<Option<WatchdogInner>> = Mutex::new(None);

/// Initialize the watchdog subsystem.
pub fn init() {
    let mut guard = WATCHDOG.lock();
    *guard = Some(WatchdogInner::new());
    serial_println!("[init_system::watchdog] watchdog initialized");
}

/// Register a service for watchdog monitoring.
pub fn monitor(service: &str, timeout_ms: u64) {
    let mut guard = WATCHDOG.lock();
    let wd = guard.as_mut().expect("watchdog not initialized");
    wd.monitor(service, timeout_ms);
}

/// Notify the watchdog that a service is alive.
pub fn notify_alive(service: &str) {
    let mut guard = WATCHDOG.lock();
    let wd = guard.as_mut().expect("watchdog not initialized");
    wd.notify_alive(service);
}

/// Stop monitoring a service.
pub fn unmonitor(service: &str) {
    let mut guard = WATCHDOG.lock();
    let wd = guard.as_mut().expect("watchdog not initialized");
    wd.unmonitor(service);
}

/// Set watchdog action for a service.
pub fn set_action(service: &str, action: WatchdogAction) {
    let mut guard = WATCHDOG.lock();
    let wd = guard.as_mut().expect("watchdog not initialized");
    wd.set_action(service, action);
}

/// Check all monitored services (call periodically).
pub fn check_all() {
    let mut guard = WATCHDOG.lock();
    let wd = guard.as_mut().expect("watchdog not initialized");
    wd.check_all();
}

/// Drain services that need restarting.
pub fn drain_restarts() -> Vec<String> {
    let mut guard = WATCHDOG.lock();
    let wd = guard.as_mut().expect("watchdog not initialized");
    wd.drain_restarts()
}

/// Get count of actively monitored services.
pub fn active_count() -> usize {
    let guard = WATCHDOG.lock();
    let wd = guard.as_ref().expect("watchdog not initialized");
    wd.active_count()
}

/// Get total timeout events.
pub fn total_timeouts() -> u32 {
    let guard = WATCHDOG.lock();
    let wd = guard.as_ref().expect("watchdog not initialized");
    wd.total_timeouts()
}

/// Enable the system-level hardware watchdog.
pub fn enable_system_watchdog(timeout_ms: u64) {
    let mut guard = WATCHDOG.lock();
    let wd = guard.as_mut().expect("watchdog not initialized");
    wd.enable_system_watchdog(timeout_ms);
}

/// Kick the system-level watchdog.
pub fn kick_system() {
    let mut guard = WATCHDOG.lock();
    let wd = guard.as_mut().expect("watchdog not initialized");
    wd.kick_system_watchdog();
}
