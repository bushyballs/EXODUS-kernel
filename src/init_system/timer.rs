/// Timer-based service activation (systemd-timer equivalent)
///
/// Part of the AIOS init_system subsystem.
///
/// Supports both periodic (interval) and oneshot (monotonic deadline)
/// timers. Each timer is associated with a service name and fires by
/// setting a flag that the service manager polls. Uses TSC-based
/// monotonic timestamps for all timing.
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

// ── Timer mode ─────────────────────────────────────────────────────────────

/// Timer behavior mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerMode {
    /// Fires once after a delay.
    Oneshot,
    /// Fires repeatedly at a fixed interval.
    Periodic,
    /// Fires at a specific monotonic timestamp.
    Realtime,
}

// ── Timer state ────────────────────────────────────────────────────────────

/// Runtime state of a timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerState {
    Inactive,
    Waiting,
    Elapsed,
    Failed,
}

// ── Timer entry ────────────────────────────────────────────────────────────

/// A single timer definition.
#[derive(Clone)]
struct TimerEntry {
    /// Name of this timer unit.
    name: String,
    name_hash: u64,
    /// Service to activate when the timer fires.
    service_name: String,
    service_hash: u64,
    /// Timer behavior.
    mode: TimerMode,
    /// Interval in milliseconds (for periodic) or delay (for oneshot).
    interval_ms: u64,
    /// Absolute monotonic deadline (TSC ticks).
    deadline_tsc: u64,
    /// Last time the timer fired (TSC).
    last_fire_tsc: u64,
    /// Number of times this timer has fired.
    fire_count: u64,
    /// Current state.
    state: TimerState,
    /// Whether the timer is persistent (survives reboot for makeup).
    persistent: bool,
    /// Randomized delay range in ms to add jitter.
    randomize_delay_ms: u64,
    /// Whether the timer is enabled.
    enabled: bool,
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

/// Rough conversion: assume ~2GHz TSC -> 1ms = 2_000_000 ticks.
/// This is approximate; real kernel would calibrate against PIT/HPET.
const TSC_PER_MS: u64 = 2_000_000;

fn ms_to_tsc(ms: u64) -> u64 {
    ms.saturating_mul(TSC_PER_MS)
}

// ── Timer manager ──────────────────────────────────────────────────────────

/// Manages all timer units and their tick processing.
struct TimerManagerInner {
    timers: Vec<TimerEntry>,
    /// Services that need to be activated (filled by tick, drained by caller).
    pending_activations: Vec<String>,
}

impl TimerManagerInner {
    fn new() -> Self {
        TimerManagerInner {
            timers: Vec::new(),
            pending_activations: Vec::new(),
        }
    }

    /// Register a new timer.
    fn register(
        &mut self,
        name: &str,
        service: &str,
        mode: TimerMode,
        interval_ms: u64,
    ) -> usize {
        let hash = fnv1a_hash(name.as_bytes());

        // Check for duplicate
        for (i, t) in self.timers.iter().enumerate() {
            if t.name_hash == hash {
                return i;
            }
        }

        let now = read_tsc();
        let deadline = now + ms_to_tsc(interval_ms);

        let idx = self.timers.len();
        self.timers.push(TimerEntry {
            name: String::from(name),
            name_hash: hash,
            service_name: String::from(service),
            service_hash: fnv1a_hash(service.as_bytes()),
            mode,
            interval_ms,
            deadline_tsc: deadline,
            last_fire_tsc: 0,
            fire_count: 0,
            state: TimerState::Inactive,
            persistent: false,
            randomize_delay_ms: 0,
            enabled: false,
        });

        serial_println!(
            "[init_system::timer] registered timer {} -> {} (mode={:?}, interval={}ms)",
            name, service, mode, interval_ms
        );

        idx
    }

    /// Start (enable) a timer.
    fn start(&mut self, name: &str) -> Result<(), ()> {
        let hash = fnv1a_hash(name.as_bytes());
        let timer = self.timers.iter_mut().find(|t| t.name_hash == hash).ok_or(())?;

        let now = read_tsc();
        timer.deadline_tsc = now + ms_to_tsc(timer.interval_ms);
        timer.state = TimerState::Waiting;
        timer.enabled = true;

        serial_println!("[init_system::timer] started timer {}", name);
        Ok(())
    }

    /// Stop (disable) a timer.
    fn stop(&mut self, name: &str) -> Result<(), ()> {
        let hash = fnv1a_hash(name.as_bytes());
        let timer = self.timers.iter_mut().find(|t| t.name_hash == hash).ok_or(())?;

        timer.state = TimerState::Inactive;
        timer.enabled = false;

        serial_println!("[init_system::timer] stopped timer {}", name);
        Ok(())
    }

    /// Process a timer tick. Called periodically by the kernel timer interrupt.
    /// Checks all active timers and fires any that have elapsed.
    fn tick(&mut self) {
        let now = read_tsc();

        for timer in self.timers.iter_mut() {
            if !timer.enabled || timer.state != TimerState::Waiting {
                continue;
            }

            if now >= timer.deadline_tsc {
                // Timer has elapsed
                timer.state = TimerState::Elapsed;
                timer.last_fire_tsc = now;
                timer.fire_count = timer.fire_count.saturating_add(1);

                serial_println!(
                    "[init_system::timer] timer {} elapsed (fire_count={}), activating {}",
                    timer.name, timer.fire_count, timer.service_name
                );

                self.pending_activations.push(timer.service_name.clone());

                // Reset for periodic timers
                match timer.mode {
                    TimerMode::Periodic => {
                        timer.deadline_tsc = now + ms_to_tsc(timer.interval_ms);
                        timer.state = TimerState::Waiting;
                    }
                    TimerMode::Oneshot | TimerMode::Realtime => {
                        timer.state = TimerState::Inactive;
                        timer.enabled = false;
                    }
                }
            }
        }
    }

    /// Drain pending service activations.
    fn drain_activations(&mut self) -> Vec<String> {
        let result = self.pending_activations.clone();
        self.pending_activations.clear();
        result
    }

    /// Get the number of active (waiting) timers.
    fn active_count(&self) -> usize {
        self.timers.iter().filter(|t| t.state == TimerState::Waiting).count()
    }

    /// Get the time until the next timer fires (in ms, approximate).
    fn next_deadline_ms(&self) -> Option<u64> {
        let now = read_tsc();
        self.timers
            .iter()
            .filter(|t| t.state == TimerState::Waiting && t.deadline_tsc > now)
            .map(|t| (t.deadline_tsc - now) / TSC_PER_MS)
            .min()
    }
}

/// Public wrapper matching original stub API.
pub struct TimerUnit {
    service_name: String,
    interval_ms: u64,
    mode: TimerMode,
    deadline_tsc: u64,
    enabled: bool,
}

impl TimerUnit {
    pub fn new(service: &str) -> Self {
        TimerUnit {
            service_name: String::from(service),
            interval_ms: 0,
            mode: TimerMode::Oneshot,
            deadline_tsc: 0,
            enabled: false,
        }
    }

    /// Set a schedule expression. Supports:
    /// - "NNNms" or "NNNs" for interval timers
    /// - "daily", "hourly", "weekly" shortcuts
    pub fn set_schedule(&mut self, expr: &str) {
        let trimmed = expr.trim();
        let bytes = trimmed.as_bytes();
        let len = bytes.len();

        if len > 2 && bytes[len - 2] == b'm' && bytes[len - 1] == b's' {
            // Parse milliseconds
            self.interval_ms = parse_u64(&trimmed[..len - 2]);
            self.mode = TimerMode::Periodic;
        } else if len > 1 && bytes[len - 1] == b's' {
            // Parse seconds
            self.interval_ms = parse_u64(&trimmed[..len - 1]) * 1000;
            self.mode = TimerMode::Periodic;
        } else if fnv1a_hash(bytes) == fnv1a_hash(b"daily") {
            self.interval_ms = 86_400_000;
            self.mode = TimerMode::Periodic;
        } else if fnv1a_hash(bytes) == fnv1a_hash(b"hourly") {
            self.interval_ms = 3_600_000;
            self.mode = TimerMode::Periodic;
        } else if fnv1a_hash(bytes) == fnv1a_hash(b"weekly") {
            self.interval_ms = 604_800_000;
            self.mode = TimerMode::Periodic;
        } else {
            // Fallback: treat as milliseconds
            self.interval_ms = parse_u64(trimmed);
            self.mode = TimerMode::Oneshot;
        }

        let now = read_tsc();
        self.deadline_tsc = now + ms_to_tsc(self.interval_ms);
        self.enabled = true;
    }

    /// Check if the timer should fire at the given TSC timestamp.
    pub fn is_elapsed(&self, now: u64) -> bool {
        self.enabled && now >= self.deadline_tsc
    }
}

fn parse_u64(s: &str) -> u64 {
    let mut val: u64 = 0;
    for &b in s.as_bytes() {
        if b >= b'0' && b <= b'9' {
            val = val.wrapping_mul(10).wrapping_add((b - b'0') as u64);
        } else {
            break;
        }
    }
    val
}

// ── Global state ───────────────────────────────────────────────────────────

static TIMER_MGR: Mutex<Option<TimerManagerInner>> = Mutex::new(None);

/// Initialize the timer subsystem.
pub fn init() {
    let mut guard = TIMER_MGR.lock();
    *guard = Some(TimerManagerInner::new());
    serial_println!("[init_system::timer] timer manager initialized");
}

/// Register a new timer.
pub fn register(name: &str, service: &str, mode: TimerMode, interval_ms: u64) -> usize {
    let mut guard = TIMER_MGR.lock();
    let mgr = guard.as_mut().expect("timer manager not initialized");
    mgr.register(name, service, mode, interval_ms)
}

/// Start a timer.
pub fn start(name: &str) -> Result<(), ()> {
    let mut guard = TIMER_MGR.lock();
    let mgr = guard.as_mut().expect("timer manager not initialized");
    mgr.start(name)
}

/// Stop a timer.
pub fn stop(name: &str) -> Result<(), ()> {
    let mut guard = TIMER_MGR.lock();
    let mgr = guard.as_mut().expect("timer manager not initialized");
    mgr.stop(name)
}

/// Process a timer tick (call from timer interrupt).
pub fn tick() {
    let mut guard = TIMER_MGR.lock();
    let mgr = guard.as_mut().expect("timer manager not initialized");
    mgr.tick();
}

/// Drain pending service activations.
pub fn drain_activations() -> Vec<String> {
    let mut guard = TIMER_MGR.lock();
    let mgr = guard.as_mut().expect("timer manager not initialized");
    mgr.drain_activations()
}

/// Get number of active timers.
pub fn active_count() -> usize {
    let guard = TIMER_MGR.lock();
    let mgr = guard.as_ref().expect("timer manager not initialized");
    mgr.active_count()
}

/// Get approximate ms until next timer fires.
pub fn next_deadline_ms() -> Option<u64> {
    let guard = TIMER_MGR.lock();
    let mgr = guard.as_ref().expect("timer manager not initialized");
    mgr.next_deadline_ms()
}
