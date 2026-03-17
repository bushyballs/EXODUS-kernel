/// High-resolution timers (hrtimer) for Genesis
///
/// Provides nanosecond-precision timers using TSC or LAPIC timer.
/// Supports one-shot and periodic timers with callbacks.
/// Used for: nanosleep, POSIX timers, scheduler tick, watchdog.
///
/// Inspired by: Linux hrtimer (kernel/time/hrtimer.c). All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Timer mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HrTimerMode {
    /// One-shot: fires once, then deactivates
    OneShot,
    /// Periodic: fires repeatedly at interval
    Periodic,
}

/// Timer state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HrTimerState {
    Inactive,
    Active,
    Expired,
    Cancelled,
}

/// Clock source for timer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockId {
    /// Monotonic (time since boot, unaffected by settimeofday)
    Monotonic,
    /// Realtime (wall clock, subject to NTP/manual adjustment)
    Realtime,
    /// Boottime (monotonic + time spent in suspend)
    Boottime,
}

/// A high-resolution timer
pub struct HrTimer {
    /// Timer ID
    pub id: u32,
    /// Expiry time (nanoseconds from epoch or boot)
    pub expires_ns: u64,
    /// Interval for periodic timers (nanoseconds)
    pub interval_ns: u64,
    /// Mode
    pub mode: HrTimerMode,
    /// State
    pub state: HrTimerState,
    /// Clock source
    pub clock: ClockId,
    /// Callback function
    pub callback: fn(u32),
    /// Name (for debugging)
    pub name: String,
    /// Overrun count (for periodic timers that missed deadlines)
    pub overruns: u64,
}

/// Timer wheel for hrtimers
pub struct HrTimerBase {
    /// Active timers sorted by expiry
    timers: Vec<HrTimer>,
    /// Next timer ID
    next_id: u32,
    /// Current time (nanoseconds)
    now_ns: u64,
    /// Resolution (nanoseconds)
    pub resolution_ns: u64,
    /// Statistics
    pub stats: HrTimerStats,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HrTimerStats {
    pub timers_created: u64,
    pub timers_expired: u64,
    pub timers_cancelled: u64,
    pub overruns: u64,
    pub max_latency_ns: u64,
}

impl HrTimerBase {
    const fn new() -> Self {
        HrTimerBase {
            timers: Vec::new(),
            next_id: 1,
            now_ns: 0,
            resolution_ns: 1_000, // 1 microsecond default
            stats: HrTimerStats {
                timers_created: 0,
                timers_expired: 0,
                timers_cancelled: 0,
                overruns: 0,
                max_latency_ns: 0,
            },
        }
    }

    /// Create a new timer. Returns timer ID.
    pub fn create(
        &mut self,
        expires_ns: u64,
        interval_ns: u64,
        mode: HrTimerMode,
        clock: ClockId,
        callback: fn(u32),
        name: &str,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let timer = HrTimer {
            id,
            expires_ns,
            interval_ns,
            mode,
            state: HrTimerState::Active,
            clock,
            callback,
            name: String::from(name),
            overruns: 0,
        };

        // Insert sorted by expiry
        let pos = self
            .timers
            .iter()
            .position(|t| t.expires_ns > expires_ns)
            .unwrap_or(self.timers.len());
        self.timers.insert(pos, timer);
        self.stats.timers_created = self.stats.timers_created.saturating_add(1);

        id
    }

    /// Cancel a timer
    pub fn cancel(&mut self, id: u32) -> bool {
        if let Some(pos) = self.timers.iter().position(|t| t.id == id) {
            self.timers[pos].state = HrTimerState::Cancelled;
            self.timers.remove(pos);
            self.stats.timers_cancelled = self.stats.timers_cancelled.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Modify a timer's expiry
    pub fn modify(&mut self, id: u32, new_expires_ns: u64) -> bool {
        if let Some(pos) = self.timers.iter().position(|t| t.id == id) {
            let mut timer = self.timers.remove(pos);
            timer.expires_ns = new_expires_ns;
            timer.state = HrTimerState::Active;

            let new_pos = self
                .timers
                .iter()
                .position(|t| t.expires_ns > new_expires_ns)
                .unwrap_or(self.timers.len());
            self.timers.insert(new_pos, timer);
            true
        } else {
            false
        }
    }

    /// Process expired timers. Called from timer interrupt.
    pub fn run_expired(&mut self, now_ns: u64) -> usize {
        self.now_ns = now_ns;
        let mut expired_count = 0;
        let mut requeue = Vec::new();

        while !self.timers.is_empty() && self.timers[0].expires_ns <= now_ns {
            let mut timer = self.timers.remove(0);
            timer.state = HrTimerState::Expired;

            // Track latency
            let latency = now_ns - timer.expires_ns;
            if latency > self.stats.max_latency_ns {
                self.stats.max_latency_ns = latency;
            }

            // Execute callback
            (timer.callback)(timer.id);
            self.stats.timers_expired = self.stats.timers_expired.saturating_add(1);
            expired_count += 1;

            // Requeue periodic timers
            if timer.mode == HrTimerMode::Periodic && timer.interval_ns > 0 {
                // Count overruns (missed periods)
                let periods_missed = latency / timer.interval_ns;
                if periods_missed > 0 {
                    timer.overruns += periods_missed;
                    self.stats.overruns += periods_missed;
                }

                timer.expires_ns = now_ns + timer.interval_ns;
                timer.state = HrTimerState::Active;
                requeue.push(timer);
            }
        }

        // Re-insert periodic timers
        for timer in requeue {
            let pos = self
                .timers
                .iter()
                .position(|t| t.expires_ns > timer.expires_ns)
                .unwrap_or(self.timers.len());
            self.timers.insert(pos, timer);
        }

        expired_count
    }

    /// Get time until next expiry (nanoseconds, 0 if already expired)
    pub fn next_expiry(&self) -> Option<u64> {
        self.timers.first().map(|t| {
            if t.expires_ns > self.now_ns {
                t.expires_ns - self.now_ns
            } else {
                0
            }
        })
    }

    /// Number of active timers
    pub fn active_count(&self) -> usize {
        self.timers.len()
    }
}

/// Global hrtimer base
pub static HRTIMER: Mutex<HrTimerBase> = Mutex::new(HrTimerBase::new());

/// Create a one-shot timer (fires once after delay_ns nanoseconds)
pub fn oneshot(delay_ns: u64, callback: fn(u32), name: &str) -> u32 {
    let now = crate::time::clock::uptime_ms() * 1_000_000;
    HRTIMER.lock().create(
        now + delay_ns,
        0,
        HrTimerMode::OneShot,
        ClockId::Monotonic,
        callback,
        name,
    )
}

/// Create a periodic timer
pub fn periodic(interval_ns: u64, callback: fn(u32), name: &str) -> u32 {
    let now = crate::time::clock::uptime_ms() * 1_000_000;
    HRTIMER.lock().create(
        now + interval_ns,
        interval_ns,
        HrTimerMode::Periodic,
        ClockId::Monotonic,
        callback,
        name,
    )
}

/// Cancel a timer
pub fn cancel(id: u32) -> bool {
    HRTIMER.lock().cancel(id)
}

/// Process expired timers (called from timer interrupt)
pub fn tick() {
    let now_ns = crate::time::clock::uptime_ms() * 1_000_000;
    HRTIMER.lock().run_expired(now_ns);
}

/// Nanosleep — sleep for `ns` nanoseconds
pub fn nanosleep(ns: u64) {
    let target = crate::time::clock::uptime_ms() * 1_000_000 + ns;
    while crate::time::clock::uptime_ms() * 1_000_000 < target {
        crate::process::yield_now();
    }
}

pub fn init() {
    crate::serial_println!("  [hrtimer] High-resolution timers initialized");
}
