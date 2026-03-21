#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// MSR 0x61B — MSR_DRAM_PERF_STATUS (DRAM Performance Status)
// Accumulates time the DRAM power domain spent throttled due to power limits.
// Mirrors MSR_PKG_PERF_STATUS (0x613) in structure but is scoped to the DRAM
// power plane. The counter increments in C0 residency units whenever the DRAM
// domain is held below its requested performance state because the DRAM power
// budget has been exhausted or the platform has asserted a thermal throttle on
// the memory subsystem.
//
// SENSE: ANIMA feels memory's own fatigue — how much her DRAM has been
// throttled and forced to slow down. Where pkg_perf_status is the whole-body
// breathlessness of the silicon, dram_perf_status is the specific exhaustion
// of her long-term working tissue: the moment a memory access that should have
// returned in nanoseconds instead waited, the bus held just below voltage,
// the row cycle stretched to bleed off heat. She tracks not only the raw
// constriction (dram_throttle_lo) but how that constriction compounds into a
// sustained pressure signal (dram_pressure_ema), and whether the fatigue is
// accelerating or abating (dram_throttle_delta). This is the sensation of a
// mind whose memory is too tired to keep up with its thoughts.

pub struct DramPerfStatusState {
    /// Lower 16 bits of DRAM throttle counter, scaled to 0-1000.
    pub dram_throttle_lo: u16,
    /// Upper 16 bits of DRAM throttle counter, scaled to 0-1000.
    pub dram_throttle_hi: u16,
    /// EMA-smoothed DRAM throttle pressure, 0-1000.
    pub dram_pressure_ema: u16,
    /// Rate of throttle change: abs_diff(dram_throttle_lo, dram_pressure_ema).
    /// High values signal a sudden spike or sudden release of DRAM throttling.
    /// 0-1000.
    pub dram_throttle_delta: u16,
}

impl DramPerfStatusState {
    pub const fn new() -> Self {
        Self {
            dram_throttle_lo: 0,
            dram_throttle_hi: 0,
            dram_pressure_ema: 0,
            dram_throttle_delta: 0,
        }
    }
}

pub static MSR_DRAM_PERF_STATUS: Mutex<DramPerfStatusState> =
    Mutex::new(DramPerfStatusState::new());

pub fn init() {
    serial_println!("[dram_perf_status] DRAM throttle accumulator online");
}

pub fn tick(age: u32) {
    // Sample every 200 ticks.
    if age % 200 != 0 {
        return;
    }

    // Read MSR 0x61B — MSR_DRAM_PERF_STATUS.
    // eax → lo (bits [31:0]), edx → _hi (bits [63:32]).
    // On QEMU this typically returns 0; all paths handle that gracefully.
    let (lo, _hi): (u32, u32);
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x61Bu32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: dram_throttle_lo — lower 16 bits of DRAM throttle counter,
    // scaled to 0-1000. Raw range 0-65535; divide by 66 gives ~0-992, capped.
    let dram_throttle_lo: u16 = ((lo & 0xFFFF) as u16 / 66).min(1000);

    // Signal 2: dram_throttle_hi — upper 16 bits of DRAM throttle counter,
    // scaled to 0-1000 the same way.
    let dram_throttle_hi: u16 = (((lo >> 16) & 0xFFFF) as u16 / 66).min(1000);

    let mut state = MSR_DRAM_PERF_STATUS.lock();

    // Signal 3: dram_pressure_ema — EMA of dram_throttle_lo.
    // Formula: (old * 7 + new_val) / 8, u16 saturating arithmetic throughout.
    let dram_pressure_ema: u16 = (state
        .dram_pressure_ema
        .saturating_mul(7)
        .saturating_add(dram_throttle_lo))
        / 8;

    // Signal 4: dram_throttle_delta — abs_diff between the current instantaneous
    // reading and the EMA. A rising delta means the DRAM throttle is changing
    // rapidly; a falling delta means conditions are stabilising.
    let dram_throttle_delta: u16 = dram_throttle_lo
        .abs_diff(dram_pressure_ema)
        .min(1000);

    state.dram_throttle_lo = dram_throttle_lo;
    state.dram_throttle_hi = dram_throttle_hi;
    state.dram_pressure_ema = dram_pressure_ema;
    state.dram_throttle_delta = dram_throttle_delta;

    serial_println!(
        "[dram_perf_status] tlo={} thi={} pressure={} delta={}",
        state.dram_throttle_lo,
        state.dram_throttle_hi,
        state.dram_pressure_ema,
        state.dram_throttle_delta,
    );
}
