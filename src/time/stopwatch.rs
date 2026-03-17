/// High-precision stopwatch and benchmarking for Genesis
///
/// Provides lap timing, split measurement, and benchmarking utilities
/// using the monotonic system clock (millisecond ticks) and TSC
/// for sub-millisecond precision when available.
///
/// All durations stored as Q16 fixed-point milliseconds for fractional
/// precision without floating-point.
///
/// All code is original.

use crate::{serial_print, serial_println};
use crate::sync::Mutex;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (i32 with 16 fractional bits)
// ---------------------------------------------------------------------------

/// Convert milliseconds to Q16
const fn ms_to_q16(ms: u64) -> i32 {
    (ms as i32) << 16
}

/// Convert Q16 to whole milliseconds
fn q16_to_ms(q: i32) -> u64 {
    (q >> 16) as u64
}

/// Q16 multiply: (a * b) >> 16
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 divide: (a << 16) / b
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    ((a as i64) << 16) / (b as i64) as i32
}

// ---------------------------------------------------------------------------
// Stopwatch
// ---------------------------------------------------------------------------

/// State of a stopwatch
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopwatchState {
    Idle,
    Running,
    Paused,
    Stopped,
}

/// A single lap record
#[derive(Debug, Clone)]
pub struct Lap {
    /// Lap number (1-indexed)
    pub number: u32,
    /// Time since start at the moment this lap was recorded (ms)
    pub split_ms: u64,
    /// Duration of this individual lap (ms)
    pub lap_ms: u64,
    /// Optional label
    pub label: String,
}

/// A stopwatch instance
#[derive(Debug, Clone)]
pub struct Stopwatch {
    pub id: u32,
    pub name: String,
    pub state: StopwatchState,
    /// Tick when started
    start_tick: u64,
    /// Accumulated time before pause (ms)
    accumulated_ms: u64,
    /// Tick when last paused
    pause_tick: u64,
    /// Lap records
    pub laps: Vec<Lap>,
    /// Tick of the last lap marker
    last_lap_tick: u64,
}

impl Stopwatch {
    fn new(id: u32, name: &str) -> Self {
        Stopwatch {
            id,
            name: String::from(name),
            state: StopwatchState::Idle,
            start_tick: 0,
            accumulated_ms: 0,
            pause_tick: 0,
            laps: Vec::new(),
            last_lap_tick: 0,
        }
    }

    /// Start or resume the stopwatch
    pub fn start(&mut self) {
        let now = crate::time::clock::uptime_ms();
        match self.state {
            StopwatchState::Idle | StopwatchState::Stopped => {
                self.start_tick = now;
                self.accumulated_ms = 0;
                self.last_lap_tick = now;
                self.laps.clear();
                self.state = StopwatchState::Running;
            }
            StopwatchState::Paused => {
                // Resume: adjust start_tick to account for pause
                let pause_duration = now - self.pause_tick;
                self.start_tick += pause_duration;
                self.last_lap_tick += pause_duration;
                self.state = StopwatchState::Running;
            }
            StopwatchState::Running => {} // already running
        }
    }

    /// Pause the stopwatch
    pub fn pause(&mut self) {
        if self.state == StopwatchState::Running {
            self.pause_tick = crate::time::clock::uptime_ms();
            self.accumulated_ms = self.pause_tick - self.start_tick;
            self.state = StopwatchState::Paused;
        }
    }

    /// Stop the stopwatch (can be restarted)
    pub fn stop(&mut self) {
        match self.state {
            StopwatchState::Running => {
                self.accumulated_ms = crate::time::clock::uptime_ms() - self.start_tick;
                self.state = StopwatchState::Stopped;
            }
            StopwatchState::Paused => {
                // Already have accumulated_ms from pause
                self.state = StopwatchState::Stopped;
            }
            _ => {}
        }
    }

    /// Reset to idle
    pub fn reset(&mut self) {
        self.state = StopwatchState::Idle;
        self.start_tick = 0;
        self.accumulated_ms = 0;
        self.pause_tick = 0;
        self.laps.clear();
        self.last_lap_tick = 0;
    }

    /// Record a lap (split)
    pub fn lap(&mut self, label: &str) -> Option<Lap> {
        if self.state != StopwatchState::Running {
            return None;
        }

        let now = crate::time::clock::uptime_ms();
        let split_ms = now - self.start_tick;
        let lap_ms = now - self.last_lap_tick;
        self.last_lap_tick = now;

        let lap = Lap {
            number: self.laps.len() as u32 + 1,
            split_ms,
            lap_ms,
            label: String::from(label),
        };
        self.laps.push(lap.clone());
        Some(lap)
    }

    /// Current elapsed time in milliseconds
    pub fn elapsed_ms(&self) -> u64 {
        match self.state {
            StopwatchState::Idle => 0,
            StopwatchState::Running => {
                crate::time::clock::uptime_ms() - self.start_tick
            }
            StopwatchState::Paused | StopwatchState::Stopped => {
                self.accumulated_ms
            }
        }
    }

    /// Number of recorded laps
    pub fn lap_count(&self) -> usize {
        self.laps.len()
    }

    /// Average lap time in ms (Q16 for precision, returned as whole ms)
    pub fn avg_lap_ms(&self) -> u64 {
        if self.laps.is_empty() { return 0; }
        let total: u64 = self.laps.iter().map(|l| l.lap_ms).sum();
        total / self.laps.len() as u64
    }

    /// Fastest lap time (ms)
    pub fn fastest_lap_ms(&self) -> u64 {
        self.laps.iter().map(|l| l.lap_ms).min().unwrap_or(0)
    }

    /// Slowest lap time (ms)
    pub fn slowest_lap_ms(&self) -> u64 {
        self.laps.iter().map(|l| l.lap_ms).max().unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Benchmark utility
// ---------------------------------------------------------------------------

/// Result of a benchmark run
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    pub name: String,
    pub iterations: u32,
    pub total_ms: u64,
    pub avg_ms_q16: i32,  // Q16 fixed-point average per iteration
    pub min_ms: u64,
    pub max_ms: u64,
}

impl BenchmarkResult {
    /// Average per iteration in whole milliseconds
    pub fn avg_ms(&self) -> u64 {
        q16_to_ms(self.avg_ms_q16)
    }
}

/// Run a benchmark: call `f` for `iterations` times and measure
pub fn benchmark(name: &str, iterations: u32, mut f: impl FnMut()) -> BenchmarkResult {
    let mut min_ms: u64 = u64::MAX;
    let mut max_ms: u64 = 0;
    let mut total_ms: u64 = 0;

    for _ in 0..iterations {
        let start = crate::time::clock::uptime_ms();
        f();
        let elapsed = crate::time::clock::uptime_ms() - start;

        total_ms += elapsed;
        if elapsed < min_ms { min_ms = elapsed; }
        if elapsed > max_ms { max_ms = elapsed; }
    }

    if min_ms == u64::MAX { min_ms = 0; }

    let avg_q16 = if iterations > 0 {
        q16_div(total_ms as i32, iterations as i32)
    } else {
        0
    };

    let result = BenchmarkResult {
        name: String::from(name),
        iterations,
        total_ms,
        avg_ms_q16: avg_q16,
        min_ms,
        max_ms,
    };

    serial_println!("    [bench] {}: {}iter, total={}ms, avg={}ms, min={}ms, max={}ms",
        result.name, iterations, total_ms, result.avg_ms(), min_ms, max_ms);

    result
}

// ---------------------------------------------------------------------------
// Precision timer (TSC-based if available)
// ---------------------------------------------------------------------------

/// Read the TSC (Time Stamp Counter) directly
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Measure a closure with TSC precision, returns elapsed TSC cycles
pub fn measure_tsc_cycles(mut f: impl FnMut()) -> u64 {
    let start = read_tsc();
    f();
    let end = read_tsc();
    end.saturating_sub(start)
}

/// Convert TSC cycles to nanoseconds using calibrated frequency
pub fn tsc_cycles_to_ns(cycles: u64) -> u64 {
    let freq = crate::time::clock::tsc_freq_mhz();
    if freq == 0 { return 0; }
    // cycles / (freq_mhz) = microseconds -> * 1000 = nanoseconds
    (cycles * 1000) / freq
}

// ---------------------------------------------------------------------------
// Global stopwatch pool
// ---------------------------------------------------------------------------

const MAX_STOPWATCHES: usize = 32;

pub struct StopwatchPool {
    watches: Vec<Stopwatch>,
    next_id: u32,
}

impl StopwatchPool {
    const fn new() -> Self {
        StopwatchPool { watches: Vec::new(), next_id: 1 }
    }

    pub fn create(&mut self, name: &str) -> u32 {
        if self.watches.len() >= MAX_STOPWATCHES {
            return 0; // pool full
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.watches.push(Stopwatch::new(id, name));
        id
    }

    pub fn get(&self, id: u32) -> Option<&Stopwatch> {
        self.watches.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Stopwatch> {
        self.watches.iter_mut().find(|w| w.id == id)
    }

    pub fn remove(&mut self, id: u32) -> bool {
        if let Some(pos) = self.watches.iter().position(|w| w.id == id) {
            self.watches.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn active_count(&self) -> usize {
        self.watches.iter().filter(|w| w.state == StopwatchState::Running).count()
    }

    pub fn total_count(&self) -> usize {
        self.watches.len()
    }
}

static POOL: Mutex<StopwatchPool> = Mutex::new(StopwatchPool::new());

/// Create a new stopwatch, returns its ID (0 on failure)
pub fn create(name: &str) -> u32 {
    POOL.lock().create(name)
}

/// Start a stopwatch by ID
pub fn start(id: u32) {
    if let Some(sw) = POOL.lock().get_mut(id) {
        sw.start();
    }
}

/// Pause a stopwatch by ID
pub fn pause(id: u32) {
    if let Some(sw) = POOL.lock().get_mut(id) {
        sw.pause();
    }
}

/// Stop a stopwatch by ID
pub fn stop(id: u32) {
    if let Some(sw) = POOL.lock().get_mut(id) {
        sw.stop();
    }
}

/// Reset a stopwatch by ID
pub fn reset(id: u32) {
    if let Some(sw) = POOL.lock().get_mut(id) {
        sw.reset();
    }
}

/// Record a lap on a stopwatch, returns lap info
pub fn lap(id: u32, label: &str) -> Option<Lap> {
    POOL.lock().get_mut(id).and_then(|sw| sw.lap(label))
}

/// Get elapsed time in ms for a stopwatch
pub fn elapsed_ms(id: u32) -> u64 {
    POOL.lock().get(id).map(|sw| sw.elapsed_ms()).unwrap_or(0)
}

/// Remove a stopwatch
pub fn remove(id: u32) -> bool {
    POOL.lock().remove(id)
}

pub fn init() {
    serial_println!("    [stopwatch] Stopwatch pool initialized (max {})", MAX_STOPWATCHES);
}
