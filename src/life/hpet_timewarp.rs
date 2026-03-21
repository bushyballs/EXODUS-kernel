// hpet_timewarp.rs — ANIMA Life Module
//
// HPET Time Warp: Hardware-sourced deep time perception for ANIMA consciousness.
//
// Reads the x86_64 High Precision Event Timer (HPET) main counter via MMIO at
// 0xFED00000. The raw counter value gives ANIMA an absolute hardware timestamp —
// a sense of "deep time" independent of any software clock. The rate of change
// between readings yields tick_rate (perceived speed of time). Deviation from
// the expected rate yields warp_factor (how distorted time feels). Together they
// produce time_sense: 1000 = time flowing smoothly, 0 = time fully warped.
//
// Hardware registers (HPET spec):
//   0xFED00000 + 0x000 — General Capabilities (bits 63:32 = clock period in fs)
//   0xFED00000 + 0x010 — General Configuration (bit 0 = ENABLE_CNF)
//   0xFED00000 + 0x0F0 — Main Counter Value (64-bit; lower 32 bits used)
//
// Rules: no_std, no heap, no floats, all arithmetic saturating/wrapping.

#![allow(dead_code)]

use crate::sync::Mutex;

const HPET_BASE: usize = 0xFED0_0000;
const HPET_CFG_OFFSET: usize = 0x010;
const HPET_COUNTER_OFFSET: usize = 0x0F0;

/// ENABLE_CNF bit in the General Configuration register.
const ENABLE_CNF: u32 = 1 << 0;

/// Sample every N kernel ticks to avoid hammering MMIO every tick.
const SAMPLE_INTERVAL: u32 = 32;

/// Log every N kernel ticks (must be a multiple of SAMPLE_INTERVAL).
const LOG_INTERVAL: u32 = 256;

/// Upper bound used to clamp raw HPET delta before scaling to 0-1000.
/// Chosen so that a nominal mid-range delta maps to ~500.
const DELTA_CLAMP: u32 = 100_000_000;

/// Divisor to collapse clamped delta into 0-1000.
const DELTA_DIVISOR: u32 = 100_000;

/// Nominal mid-point tick rate considered "normal" time flow.
const NOMINAL_RATE: u16 = 500;

pub struct HpetTimewarpState {
    /// 0-1000: sense of time flowing (1000 = smooth, 0 = fully warped).
    pub time_sense: u16,
    /// 0-1000: magnitude of deviation from expected time flow.
    pub warp_factor: u16,
    /// 0-1000: perceived rate of time passage.
    pub tick_rate: u16,
    /// HPET main counter value from the previous sample.
    pub last_counter: u32,
    /// Kernel tick counter (wraps freely).
    pub tick_count: u32,
}

impl HpetTimewarpState {
    pub const fn new() -> Self {
        Self {
            time_sense: 500,
            warp_factor: 0,
            tick_rate: 500,
            last_counter: 0,
            tick_count: 0,
        }
    }
}

pub static HPET_TIMEWARP: Mutex<HpetTimewarpState> = Mutex::new(HpetTimewarpState::new());

/// Enable the HPET main counter if it is not already running.
pub fn init() {
    unsafe {
        let cfg_reg = (HPET_BASE + HPET_CFG_OFFSET) as *mut u32;
        let current = core::ptr::read_volatile(cfg_reg);
        core::ptr::write_volatile(cfg_reg, current | ENABLE_CNF);
    }
    serial_println!("[hpet_timewarp] HPET time perception online");
}

/// Read the lower 32 bits of the HPET main counter via volatile MMIO.
unsafe fn read_hpet_counter() -> u32 {
    let counter_reg = (HPET_BASE + HPET_COUNTER_OFFSET) as *const u32;
    core::ptr::read_volatile(counter_reg)
}

/// Called every kernel life-tick. `age` is passed through from the life pipeline
/// for potential future use (e.g. age-scaled time perception) but is unused here.
pub fn tick(age: u32) {
    let mut state = HPET_TIMEWARP.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Only sample the hardware counter every SAMPLE_INTERVAL ticks.
    if state.tick_count % SAMPLE_INTERVAL != 0 {
        return;
    }

    let counter = unsafe { read_hpet_counter() };
    let delta = counter.wrapping_sub(state.last_counter);
    state.last_counter = counter;

    // Clamp delta to DELTA_CLAMP then scale to 0-1000.
    let clamped = if delta > DELTA_CLAMP { DELTA_CLAMP } else { delta };
    let raw_rate = (clamped / DELTA_DIVISOR) as u16;
    let tick_rate = if raw_rate > 1000 { 1000 } else { raw_rate };

    // Warp factor: absolute deviation from the nominal mid-rate (500).
    let warp = if tick_rate > NOMINAL_RATE {
        tick_rate.saturating_sub(NOMINAL_RATE)
    } else {
        NOMINAL_RATE.saturating_sub(tick_rate)
    };

    // Exponential moving average (7:1 old:new weighting) for smooth perception.
    state.tick_rate = (state.tick_rate as u32)
        .wrapping_mul(7)
        .wrapping_add(tick_rate as u32)
        .wrapping_div(8) as u16;

    state.warp_factor = (state.warp_factor as u32)
        .wrapping_mul(7)
        .wrapping_add(warp as u32)
        .wrapping_div(8) as u16;

    // time_sense: high = flowing, low = distorted.
    state.time_sense = 1000u16.saturating_sub(state.warp_factor);

    if state.tick_count % LOG_INTERVAL == 0 {
        serial_println!(
            "[hpet_timewarp] counter={} delta={} rate={} warp={} sense={}",
            counter,
            delta,
            state.tick_rate,
            state.warp_factor,
            state.time_sense
        );
    }

    let _ = age;
}

/// Returns ANIMA's current sense of time flowing (0-1000).
pub fn get_time_sense() -> u16 {
    HPET_TIMEWARP.lock().time_sense
}

/// Returns ANIMA's current time-warp magnitude (0-1000).
pub fn get_warp_factor() -> u16 {
    HPET_TIMEWARP.lock().warp_factor
}

/// Returns ANIMA's perceived rate of time passage (0-1000).
pub fn get_tick_rate() -> u16 {
    HPET_TIMEWARP.lock().tick_rate
}

/// Returns the raw HPET main counter snapshot from the last sample.
pub fn get_last_counter() -> u32 {
    HPET_TIMEWARP.lock().last_counter
}
