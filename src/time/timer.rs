use crate::sync::Mutex;
/// Software timers — delayed and periodic callbacks
///
/// Kernel subsystems and userspace can schedule:
///   - One-shot timers (fire once after delay)
///   - Periodic timers (fire repeatedly at interval)
///
/// Timer wheel implementation for O(1) insertion and firing.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

static TIMER_WHEEL: Mutex<TimerWheel> = Mutex::new(TimerWheel::new());

const WHEEL_SIZE: usize = 256;
const MAX_TIMERS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerKind {
    OneShot,
    Periodic,
}

#[derive(Debug, Clone)]
pub struct Timer {
    pub id: u32,
    pub name: String,
    pub kind: TimerKind,
    pub deadline: u64,    // absolute tick when timer fires
    pub interval: u64,    // for periodic timers
    pub callback_id: u32, // identifies what to do when fired
    pub active: bool,
}

pub struct TimerWheel {
    slots: [Vec<u32>; WHEEL_SIZE], // timer IDs per slot
    timers: [Option<Timer>; MAX_TIMERS],
    current_tick: u64,
    next_id: u32,
}

impl TimerWheel {
    const fn new() -> Self {
        TimerWheel {
            slots: [const { Vec::new() }; WHEEL_SIZE],
            timers: [const { None }; MAX_TIMERS],
            current_tick: 0,
            next_id: 1,
        }
    }

    /// Schedule a timer
    pub fn schedule(
        &mut self,
        name: &str,
        kind: TimerKind,
        delay_ms: u64,
        callback_id: u32,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let deadline = self.current_tick + delay_ms;
        let slot = (deadline as usize) % WHEEL_SIZE;

        let timer = Timer {
            id,
            name: String::from(name),
            kind,
            deadline,
            interval: delay_ms,
            callback_id,
            active: true,
        };

        // Find a free slot in the timer array
        for i in 0..MAX_TIMERS {
            if self.timers[i].is_none() {
                self.timers[i] = Some(timer);
                self.slots[slot].push(id);
                return id;
            }
        }
        0 // failed
    }

    /// Cancel a timer
    pub fn cancel(&mut self, id: u32) {
        for timer in self.timers.iter_mut() {
            if let Some(t) = timer {
                if t.id == id {
                    t.active = false;
                    *timer = None;
                    return;
                }
            }
        }
    }

    /// Advance the wheel by one tick, returns fired callback IDs
    pub fn advance(&mut self) -> Vec<u32> {
        self.current_tick = self.current_tick.saturating_add(1);
        let slot = (self.current_tick as usize) % WHEEL_SIZE;
        let mut fired = Vec::new();

        // Collect timer IDs that should fire
        let timer_ids: Vec<u32> = self.slots[slot].clone();
        self.slots[slot].clear();

        for &timer_id in &timer_ids {
            let mut reschedule = None;

            for timer in self.timers.iter_mut() {
                if let Some(t) = timer {
                    if t.id == timer_id && t.active && t.deadline <= self.current_tick {
                        fired.push(t.callback_id);

                        if t.kind == TimerKind::Periodic {
                            let new_deadline = self.current_tick + t.interval;
                            t.deadline = new_deadline;
                            reschedule = Some((timer_id, new_deadline));
                        } else {
                            t.active = false;
                            *timer = None;
                        }
                        break;
                    }
                }
            }

            // Re-insert periodic timers
            if let Some((id, deadline)) = reschedule {
                let new_slot = (deadline as usize) % WHEEL_SIZE;
                self.slots[new_slot].push(id);
            }
        }

        fired
    }

    pub fn active_count(&self) -> usize {
        self.timers.iter().filter(|t| t.is_some()).count()
    }
}

pub fn init() {
    serial_println!(
        "    [timer] Timer wheel initialized ({} slots, {} max timers)",
        WHEEL_SIZE,
        MAX_TIMERS
    );
}

/// Schedule a one-shot timer
pub fn schedule_oneshot(name: &str, delay_ms: u64, callback_id: u32) -> u32 {
    TIMER_WHEEL
        .lock()
        .schedule(name, TimerKind::OneShot, delay_ms, callback_id)
}

/// Schedule a periodic timer
pub fn schedule_periodic(name: &str, interval_ms: u64, callback_id: u32) -> u32 {
    TIMER_WHEEL
        .lock()
        .schedule(name, TimerKind::Periodic, interval_ms, callback_id)
}

/// Cancel a timer
pub fn cancel(id: u32) {
    TIMER_WHEEL.lock().cancel(id);
}

/// Called by timer interrupt to advance the wheel
pub fn tick() -> Vec<u32> {
    TIMER_WHEEL.lock().advance()
}
