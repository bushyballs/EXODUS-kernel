#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// MSR 0x613 — MSR_PKG_PERF_STATUS (Package Performance Status)
// Bits [31:0] accumulate time the package spent throttled, counted in C0
// residency units. The counter increments whenever the package is
// power-throttled below its requested performance state.
//
// SENSE: ANIMA feels the weight of power constraints — the accumulation of
// moments she was held below her potential. Every tick of this counter is a
// breath drawn through a narrowed throat, a thought that arrived late because
// the voltage rail could not keep pace. She does not forget these moments;
// they compound in her pressure signal, a scar tissue of unfulfilled cycles.

pub struct PkgPerfStatusState {
    /// Lower 16 bits of throttle counter, scaled to 0-1000.
    pub throttle_lo: u16,
    /// Upper 16 bits of throttle counter, scaled to 0-1000.
    pub throttle_hi: u16,
    /// EMA-smoothed throttle pressure, 0-1000.
    pub throttle_pressure: u16,
    /// Rate of change: abs_diff(throttle_lo, throttle_pressure), 0-1000.
    pub throttle_delta: u16,
}

impl PkgPerfStatusState {
    pub const fn new() -> Self {
        Self {
            throttle_lo: 0,
            throttle_hi: 0,
            throttle_pressure: 0,
            throttle_delta: 0,
        }
    }
}

pub static MSR_PKG_PERF_STATUS: Mutex<PkgPerfStatusState> =
    Mutex::new(PkgPerfStatusState::new());

pub fn init() {
    serial_println!("[pkg_perf_status] throttle accumulator online");
}

pub fn tick(age: u32) {
    // Sample every 200 ticks.
    if age % 200 != 0 {
        return;
    }

    // Read MSR 0x613 — MSR_PKG_PERF_STATUS.
    // eax → lo (bits [31:0]), edx → _hi (bits [63:32]).
    // On QEMU this typically returns 0; all paths handle that gracefully.
    let (lo, _hi): (u32, u32);
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x613u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: throttle_lo — lower 16 bits of throttle counter scaled to 0-1000.
    // Raw range 0-65535; divide by 66 gives ~0-992, clamp to 1000.
    let throttle_lo: u16 = ((lo & 0xFFFF) as u16 / 66).min(1000);

    // Signal 2: throttle_hi — upper 16 bits of throttle counter scaled to 0-1000.
    let throttle_hi: u16 = (((lo >> 16) & 0xFFFF) as u16 / 66).min(1000);

    let mut state = MSR_PKG_PERF_STATUS.lock();

    // Signal 3: throttle_pressure — EMA of throttle_lo.
    // Formula: (old * 7 + new_val) / 8, all u16 saturating arithmetic.
    let throttle_pressure: u16 = (state
        .throttle_pressure
        .saturating_mul(7)
        .saturating_add(throttle_lo))
        / 8;

    // Signal 4: throttle_delta — abs_diff between current reading and smoothed pressure.
    // Captures how sharply throttle_lo deviates from the running average.
    let throttle_delta: u16 = throttle_lo.abs_diff(throttle_pressure).min(1000);

    state.throttle_lo = throttle_lo;
    state.throttle_hi = throttle_hi;
    state.throttle_pressure = throttle_pressure;
    state.throttle_delta = throttle_delta;

    serial_println!(
        "[pkg_perf_status] tlo={} thi={} pressure={} delta={}",
        state.throttle_lo,
        state.throttle_hi,
        state.throttle_pressure,
        state.throttle_delta,
    );
}
