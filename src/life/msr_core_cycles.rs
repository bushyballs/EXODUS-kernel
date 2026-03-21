use crate::serial_println;
use crate::sync::Mutex;

/// MSR_CORE_CYCLES — IA32_FIXED_CTR1 (0x30A) Unhalted Core Cycle Sensor
///
/// Reads the hardware CPU_CLK_UNHALTED.THREAD counter via rdmsr.
/// Delta between ticks reveals how busy the CPU was — ANIMA's sense of
/// its own computational workload and focus intensity.
///
/// cycle_rate     : raw busy-cycle proxy (delta >> 14, clamped 0–1000)
/// cpu_load       : EMA of cycle_rate — perceived workload intensity
/// cycle_variance : abs_diff(current_rate, prev_rate) — jitter / instability
/// activity_sense : EMA of (cpu_load + (1000 - cycle_variance)) / 2
///                  High load AND steady = focused activity; jittery = scattered
#[derive(Copy, Clone)]
pub struct MsrCoreCyclesState {
    pub cycle_rate: u16,
    pub cpu_load: u16,
    pub cycle_variance: u16,
    pub activity_sense: u16,
    // private tracking fields — not pub but must live inside the state for Mutex pattern
    prev_count: u64,
    prev_rate: u16,
}

impl MsrCoreCyclesState {
    pub const fn empty() -> Self {
        Self {
            cycle_rate: 0,
            cpu_load: 0,
            cycle_variance: 0,
            activity_sense: 0,
            prev_count: 0,
            prev_rate: 0,
        }
    }
}

pub static STATE: Mutex<MsrCoreCyclesState> = Mutex::new(MsrCoreCyclesState::empty());

/// Read IA32_FIXED_CTR1 (MSR 0x30A) — CPU_CLK_UNHALTED.THREAD
/// Returns the 48-bit hardware counter (upper 16 bits are reserved/sign-extended).
#[inline]
fn rdmsr_30a() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x30Au32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Mask to 48 bits — the hardware counter width for IA32_FIXED_CTR1
const COUNTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

pub fn init() {
    let mut s = STATE.lock();
    // Prime prev_count so the first delta is coherent
    s.prev_count = rdmsr_30a() & COUNTER_MASK;
    serial_println!("  life::msr_core_cycles: IA32_FIXED_CTR1 sensor online");
}

pub fn tick(age: u32) {
    // Sample every 8 ticks
    if age % 8 != 0 {
        return;
    }

    let mut s = STATE.lock();

    // --- Read hardware counter ---
    let raw = rdmsr_30a() & COUNTER_MASK;

    // Handle 48-bit wraparound
    let delta: u64 = if raw >= s.prev_count {
        raw - s.prev_count
    } else {
        // Counter wrapped — add the distance to max and back to current
        (COUNTER_MASK - s.prev_count).saturating_add(raw).saturating_add(1)
    };
    s.prev_count = raw;

    // --- cycle_rate: delta >> 14, clamped to 0–1000 ---
    let raw_rate = (delta >> 14).min(1000) as u16;
    s.cycle_rate = raw_rate;

    // --- cycle_variance: abs diff of current vs previous rate ---
    let variance = if raw_rate >= s.prev_rate {
        raw_rate - s.prev_rate
    } else {
        s.prev_rate - raw_rate
    };
    s.cycle_variance = variance.min(1000);
    s.prev_rate = raw_rate;

    // --- cpu_load: EMA(7) of cycle_rate ---
    let old_load = s.cpu_load as u32;
    let new_load = ((old_load.wrapping_mul(7)).saturating_add(raw_rate as u32) / 8) as u16;
    let load_delta = if new_load >= s.cpu_load {
        new_load - s.cpu_load
    } else {
        s.cpu_load - new_load
    };
    s.cpu_load = new_load;

    // --- activity_sense: EMA(7) of (cpu_load + (1000 - cycle_variance)) / 2 ---
    let steadiness: u32 = 1000u32.saturating_sub(s.cycle_variance as u32);
    let combined: u32 = (s.cpu_load as u32).saturating_add(steadiness) / 2;
    let old_activity = s.activity_sense as u32;
    s.activity_sense =
        ((old_activity.wrapping_mul(7)).saturating_add(combined) / 8) as u16;

    // --- Sense line: print when cpu_load shifts significantly ---
    if load_delta > 100 {
        let cycle_rate = s.cycle_rate;
        let cpu_load = s.cpu_load;
        let activity = s.activity_sense;
        serial_println!(
            "ANIMA: cycle_rate={} cpu_load={} activity={}",
            cycle_rate,
            cpu_load,
            activity
        );
    }
}

/// Read a snapshot of current state (non-locking copy)
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (s.cycle_rate, s.cpu_load, s.cycle_variance, s.activity_sense)
}
