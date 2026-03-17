use crate::sync::Mutex;
/// Hierarchical timer wheel for scheduling timeouts.
///
/// Part of the AIOS kernel.
use alloc::vec::Vec;

/// A pending timer callback.
pub struct Timer {
    /// Expiry time in ticks.
    pub expires: u64,
    /// Callback ID for cancellation.
    pub id: u64,
    /// Optional callback: invoked with the timer's `data` field when it fires.
    pub callback: Option<fn(u64)>,
    /// Arbitrary data passed to the callback on firing.
    pub data: u64,
}

impl Timer {
    /// Construct a basic timer with no callback.
    pub fn new(id: u64, expires: u64) -> Self {
        Timer {
            expires,
            id,
            callback: None,
            data: 0,
        }
    }

    /// Attach a callback function to this timer (builder style).
    pub fn with_callback(mut self, cb: fn(u64), data: u64) -> Self {
        self.callback = Some(cb);
        self.data = data;
        self
    }

    /// Attach a callback via mutation (post-construction helper).
    pub fn set_callback(&mut self, cb: fn(u64), data: u64) {
        self.callback = Some(cb);
        self.data = data;
    }
}

/// Multi-level timer wheel (cascading buckets).
pub struct TimerWheel {
    pub current_tick: u64,
    pub buckets: Vec<Vec<Timer>>,
    pub wheel_size: usize,
}

impl TimerWheel {
    pub fn new(wheel_size: usize) -> Self {
        let size = if wheel_size == 0 { 256 } else { wheel_size };
        let mut buckets = Vec::with_capacity(size);
        for _ in 0..size {
            buckets.push(Vec::new());
        }
        TimerWheel {
            current_tick: 0,
            buckets,
            wheel_size: size,
        }
    }

    /// Insert a timer into the bucket corresponding to its expiry tick.
    pub fn add_timer(&mut self, timer: Timer) {
        let slot = (timer.expires as usize) % self.wheel_size;
        self.buckets[slot].push(timer);
    }

    /// Advance the wheel by `ticks` ticks, firing any timers that expire.
    ///
    /// For each timer whose `expires <= current_tick` the optional callback is
    /// invoked with `timer.data`.  Timers placed in the wrong bucket due to the
    /// modulo hash (i.e. `expires > current_tick`) are re-inserted into the
    /// correct future slot.
    pub fn advance(&mut self, ticks: u64) {
        for _ in 0..ticks {
            let slot = (self.current_tick as usize) % self.wheel_size;
            // Drain the current slot.
            let due: Vec<Timer> = self.buckets[slot].drain(..).collect();
            for timer in due {
                if timer.expires <= self.current_tick {
                    crate::serial_println!(
                        "timer_wheel: timer {} fired at tick {}",
                        timer.id,
                        self.current_tick
                    );
                    // Invoke callback if one is registered.
                    if let Some(cb) = timer.callback {
                        cb(timer.data);
                    }
                } else {
                    // Timer not yet due — re-insert into its future slot.
                    let future_slot = (timer.expires as usize) % self.wheel_size;
                    self.buckets[future_slot].push(timer);
                }
            }
            self.current_tick = self.current_tick.wrapping_add(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Global timer wheel instance
// ---------------------------------------------------------------------------

/// Kernel-global timer wheel, protected by a spinlock.
pub static GLOBAL_TIMER_WHEEL: Mutex<Option<TimerWheel>> = Mutex::new(None);

/// Heartbeat callback registered during `init()`.
///
/// Fires on every timer tick and emits a serial log line so boot output
/// confirms the timer subsystem is alive.
fn heartbeat_callback(_data: u64) {
    crate::serial_println!("[timer] tick");
}

/// Initialize the timer wheel subsystem.
///
/// Creates a 256-slot global wheel and registers a heartbeat timer that
/// fires at tick 1 to confirm the callback path is wired up.
pub fn init() {
    let mut wheel = TimerWheel::new(256);

    // Register a sample heartbeat timer: fires at tick 1.
    let heartbeat = Timer::new(0, 1).with_callback(heartbeat_callback, 0);
    wheel.add_timer(heartbeat);

    *GLOBAL_TIMER_WHEEL.lock() = Some(wheel);
    crate::serial_println!("  timer_wheel: initialized (256 slots, heartbeat registered)");
}
