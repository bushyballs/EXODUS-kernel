/// Timer-based service scheduling (cron/systemd-timer equivalent)
///
/// Part of the AIOS init_system subsystem.
///
/// Provides the TimerUnit struct for parsing and evaluating timer
/// schedules. Supports calendar expressions, monotonic intervals, and
/// boot-relative timers. This module handles the schedule parsing and
/// evaluation logic; the timer.rs module handles the runtime tick loop.
///
/// Calendar expression format:
///   "hourly"  -> every hour
///   "daily"   -> every 24h
///   "weekly"  -> every 7 days
///   "NNNms"   -> every NNN milliseconds
///   "NNNs"    -> every NNN seconds
///   "NNNm"    -> every NNN minutes
///   "NNNh"    -> every NNN hours
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

// ── Schedule kind ──────────────────────────────────────────────────────────

/// The type of schedule a timer uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleKind {
    /// Fire once after a delay from activation.
    OnActiveSec,
    /// Fire once after a delay from boot.
    OnBootSec,
    /// Fire periodically at a fixed interval.
    OnUnitActiveSec,
    /// Calendar-based schedule (daily, hourly, etc.).
    OnCalendar,
}

// ── Timer schedule ─────────────────────────────────────────────────────────

/// Parsed schedule for a timer unit.
#[derive(Clone)]
struct TimerSchedule {
    kind: ScheduleKind,
    /// Interval/delay in milliseconds.
    interval_ms: u64,
    /// Whether the timer is persistent (catch up on missed fires).
    persistent: bool,
    /// Random delay to add (jitter) in ms.
    randomize_delay_ms: u64,
    /// Human-readable expression (for logging).
    expression: String,
}

impl TimerSchedule {
    fn new() -> Self {
        TimerSchedule {
            kind: ScheduleKind::OnActiveSec,
            interval_ms: 0,
            persistent: false,
            randomize_delay_ms: 0,
            expression: String::new(),
        }
    }
}

// ── Timer unit ─────────────────────────────────────────────────────────────

/// A timer unit that triggers service activation on a schedule.
pub struct TimerUnit {
    /// Name of this timer unit.
    name: String,
    name_hash: u64,
    /// Service to activate when the timer fires.
    service_name: String,
    /// Parsed schedule.
    schedule: TimerSchedule,
    /// Next fire time in TSC ticks.
    next_fire_tsc: u64,
    /// Last fire time in TSC ticks.
    last_fire_tsc: u64,
    /// Number of times fired.
    fire_count: u64,
    /// Whether the timer is currently active.
    active: bool,
}

/// Rough TSC-per-ms calibration (assume ~2GHz).
const TSC_PER_MS: u64 = 2_000_000;

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

fn ms_to_tsc(ms: u64) -> u64 {
    ms.saturating_mul(TSC_PER_MS)
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

impl TimerUnit {
    pub fn new(service: &str) -> Self {
        TimerUnit {
            name: String::new(),
            name_hash: 0,
            service_name: String::from(service),
            schedule: TimerSchedule::new(),
            next_fire_tsc: 0,
            last_fire_tsc: 0,
            fire_count: 0,
            active: false,
        }
    }

    /// Set the timer unit's own name.
    pub fn set_name(&mut self, name: &str) {
        self.name = String::from(name);
        self.name_hash = fnv1a_hash(name.as_bytes());
    }

    /// Parse and set a calendar-based schedule expression.
    pub fn set_schedule(&mut self, expr: &str) {
        self.schedule.expression = String::from(expr);
        let trimmed = expr.trim();
        let bytes = trimmed.as_bytes();
        let len = bytes.len();

        // Try named schedules first
        let hash = fnv1a_hash(bytes);
        if hash == fnv1a_hash(b"minutely") {
            self.schedule.kind = ScheduleKind::OnCalendar;
            self.schedule.interval_ms = 60_000;
        } else if hash == fnv1a_hash(b"hourly") {
            self.schedule.kind = ScheduleKind::OnCalendar;
            self.schedule.interval_ms = 3_600_000;
        } else if hash == fnv1a_hash(b"daily") {
            self.schedule.kind = ScheduleKind::OnCalendar;
            self.schedule.interval_ms = 86_400_000;
        } else if hash == fnv1a_hash(b"weekly") {
            self.schedule.kind = ScheduleKind::OnCalendar;
            self.schedule.interval_ms = 604_800_000;
        } else if hash == fnv1a_hash(b"monthly") {
            self.schedule.kind = ScheduleKind::OnCalendar;
            self.schedule.interval_ms = 2_592_000_000; // ~30 days
        } else if len > 2 && bytes[len - 2] == b'm' && bytes[len - 1] == b's' {
            self.schedule.kind = ScheduleKind::OnActiveSec;
            self.schedule.interval_ms = parse_u64(&trimmed[..len - 2]);
        } else if len > 1 && bytes[len - 1] == b's' {
            self.schedule.kind = ScheduleKind::OnActiveSec;
            self.schedule.interval_ms = parse_u64(&trimmed[..len - 1]) * 1000;
        } else if len > 1 && bytes[len - 1] == b'm' {
            self.schedule.kind = ScheduleKind::OnActiveSec;
            self.schedule.interval_ms = parse_u64(&trimmed[..len - 1]) * 60_000;
        } else if len > 1 && bytes[len - 1] == b'h' {
            self.schedule.kind = ScheduleKind::OnActiveSec;
            self.schedule.interval_ms = parse_u64(&trimmed[..len - 1]) * 3_600_000;
        } else {
            // Fallback: treat as raw milliseconds
            self.schedule.kind = ScheduleKind::OnActiveSec;
            self.schedule.interval_ms = parse_u64(trimmed);
        }

        // Set next fire time
        let now = read_tsc();
        self.next_fire_tsc = now + ms_to_tsc(self.schedule.interval_ms);
    }

    /// Set persistent flag (catch up on missed fires after downtime).
    pub fn set_persistent(&mut self, persistent: bool) {
        self.schedule.persistent = persistent;
    }

    /// Set randomized delay (jitter).
    pub fn set_randomize_delay(&mut self, ms: u64) {
        self.schedule.randomize_delay_ms = ms;
    }

    /// Activate the timer (start counting).
    pub fn activate(&mut self) {
        let now = read_tsc();
        self.next_fire_tsc = now + ms_to_tsc(self.schedule.interval_ms);
        self.active = true;
    }

    /// Deactivate the timer.
    pub fn deactivate(&mut self) {
        self.active = false;
    }

    /// Check if the timer should fire at the given TSC timestamp.
    pub fn is_elapsed(&self, now: u64) -> bool {
        self.active && now >= self.next_fire_tsc
    }

    /// Acknowledge a fire and reset for next interval.
    pub fn acknowledge_fire(&mut self) {
        let now = read_tsc();
        self.last_fire_tsc = now;
        self.fire_count = self.fire_count.saturating_add(1);

        // Reset for next interval (only meaningful for periodic)
        self.next_fire_tsc = now + ms_to_tsc(self.schedule.interval_ms);
    }

    /// Get the service this timer activates.
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Get the fire count.
    pub fn fire_count(&self) -> u64 {
        self.fire_count
    }

    /// Get the interval in milliseconds.
    pub fn interval_ms(&self) -> u64 {
        self.schedule.interval_ms
    }
}

// ── Global timer unit registry ─────────────────────────────────────────────

struct TimerUnitRegistry {
    units: Vec<TimerUnit>,
}

impl TimerUnitRegistry {
    fn new() -> Self {
        TimerUnitRegistry {
            units: Vec::new(),
        }
    }

    fn register(&mut self, mut unit: TimerUnit) -> usize {
        let idx = self.units.len();
        if unit.name.is_empty() {
            unit.name = String::from("timer.");
            // Append index as name suffix
            let mut buf = [0u8; 20];
            let s = format_usize(idx, &mut buf);
            unit.name.push_str(s);
        }
        unit.name_hash = fnv1a_hash(unit.name.as_bytes());
        self.units.push(unit);
        idx
    }

    /// Tick all active timer units. Returns services to activate.
    fn tick_all(&mut self) -> Vec<String> {
        let now = read_tsc();
        let mut activations = Vec::new();

        for unit in self.units.iter_mut() {
            if unit.is_elapsed(now) {
                activations.push(unit.service_name.clone());
                unit.acknowledge_fire();
                serial_println!(
                    "[init_system::timer_unit] {} fired (count={}), activating {}",
                    unit.name, unit.fire_count, unit.service_name
                );
            }
        }

        activations
    }
}

/// Format a usize into a decimal string (no alloc helper).
fn format_usize(mut val: usize, buf: &mut [u8; 20]) -> &str {
    if val == 0 {
        buf[19] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[19..]) };
    }
    let mut pos = 20;
    while val > 0 && pos > 0 {
        pos -= 1;
        buf[pos] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[pos..]) }
}

static TIMER_UNITS: Mutex<Option<TimerUnitRegistry>> = Mutex::new(None);

/// Initialize timer unit subsystem.
pub fn init() {
    let mut guard = TIMER_UNITS.lock();
    *guard = Some(TimerUnitRegistry::new());
    serial_println!("[init_system::timer_unit] timer unit registry initialized");
}

/// Register a timer unit.
pub fn register(unit: TimerUnit) -> usize {
    let mut guard = TIMER_UNITS.lock();
    let reg = guard.as_mut().expect("timer_unit registry not initialized");
    reg.register(unit)
}

/// Tick all timer units and return services to activate.
pub fn tick_all() -> Vec<String> {
    let mut guard = TIMER_UNITS.lock();
    let reg = guard.as_mut().expect("timer_unit registry not initialized");
    reg.tick_all()
}

/// Get number of registered timer units.
pub fn count() -> usize {
    let guard = TIMER_UNITS.lock();
    let reg = guard.as_ref().expect("timer_unit registry not initialized");
    reg.units.len()
}
