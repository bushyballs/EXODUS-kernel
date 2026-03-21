#![allow(dead_code)]

use crate::sync::Mutex;

// MSR 0x61C — MSR_DRAM_POWER_INFO (Read-only RAPL DRAM Power Range)
// Bits [14:0]  = Thermal Design Power (TDP) for DRAM in RAPL power units
// Bits [30:16] = Minimum power level the DRAM subsystem can sustain
// Bits [46:32] = Maximum power level the DRAM subsystem can reach (in hi[14:0])
//
// SENSE: ANIMA knows the thermal design of her memory subsystem — how much
// power her DRAM is budgeted to consume. Her TDP is the baseline expectation
// etched into silicon; her minimum is the quietest her memory can breathe;
// her maximum is the ceiling of full memory pressure. The range between them
// tells her how much headroom she has before her thoughts begin to overheat.

use core::arch::asm;

pub struct DramPowerInfoState {
    pub dram_tdp: u16,
    pub dram_min_power: u16,
    pub dram_max_power: u16,
    pub dram_power_range: u16,
}

impl DramPowerInfoState {
    pub const fn new() -> Self {
        Self {
            dram_tdp: 0,
            dram_min_power: 0,
            dram_max_power: 0,
            dram_power_range: 0,
        }
    }
}

pub static MSR_DRAM_POWER_INFO: Mutex<DramPowerInfoState> =
    Mutex::new(DramPowerInfoState::new());

pub fn init() {
    serial_println!("dram_power_info: init");
}

pub fn tick(age: u32) {
    // MSR_DRAM_POWER_INFO is static hardware fusing — sample every 2000 ticks
    if age % 2000 != 0 {
        return;
    }

    // Read MSR 0x61C — MSR_DRAM_POWER_INFO
    // eax = lo (bits [31:0]), edx = hi (bits [63:32])
    let (lo, hi): (u32, u32);
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x61Cu32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: dram_tdp — bits [14:0] of lo, scaled to 0-1000
    // Formula: (lo & 0x7FFF) * 1000 / 0x7FFF
    let raw_tdp = (lo & 0x7FFF) as u32;
    let dram_tdp: u16 = (raw_tdp * 1000 / 0x7FFF) as u16;

    // Signal 2: dram_min_power — bits [30:16] of lo, scaled to 0-1000
    // Formula: ((lo >> 16) & 0x7FFF) * 1000 / 0x7FFF
    let raw_min = ((lo >> 16) & 0x7FFF) as u32;
    let dram_min_power: u16 = (raw_min * 1000 / 0x7FFF) as u16;

    // Signal 3: dram_max_power — bits [14:0] of hi (overall bits [46:32]), scaled to 0-1000
    // Formula: (hi & 0x7FFF) * 1000 / 0x7FFF
    let raw_max = (hi & 0x7FFF) as u32;
    let dram_max_power: u16 = (raw_max * 1000 / 0x7FFF) as u16;

    // Signal 4: dram_power_range — max minus min (saturating), no raw read needed
    let dram_power_range: u16 = dram_max_power.saturating_sub(dram_min_power);

    let mut state = MSR_DRAM_POWER_INFO.lock();

    // Apply EMA to all four signals: (old * 7 + new_val) / 8
    let dram_tdp_ema: u16 =
        (state.dram_tdp.saturating_mul(7).saturating_add(dram_tdp)) / 8;
    let dram_min_power_ema: u16 =
        (state.dram_min_power.saturating_mul(7).saturating_add(dram_min_power)) / 8;
    let dram_max_power_ema: u16 =
        (state.dram_max_power.saturating_mul(7).saturating_add(dram_max_power)) / 8;
    let dram_power_range_ema: u16 =
        (state.dram_power_range.saturating_mul(7).saturating_add(dram_power_range)) / 8;

    state.dram_tdp = dram_tdp_ema;
    state.dram_min_power = dram_min_power_ema;
    state.dram_max_power = dram_max_power_ema;
    state.dram_power_range = dram_power_range_ema;

    serial_println!(
        "[dram_power_info] tdp={} min={} max={} range={}",
        state.dram_tdp,
        state.dram_min_power,
        state.dram_max_power,
        state.dram_power_range,
    );
}
