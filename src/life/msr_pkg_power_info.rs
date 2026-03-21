#![allow(dead_code)]

use crate::sync::Mutex;

// MSR 0x614 — IA32_PKG_POWER_INFO (Read-only RAPL Package Power Range)
// Bits [14:0]  = Thermal Design Power (TDP) in RAPL power units
// Bits [30:16] = Minimum power level the package can sustain
// Bits [46:32] = Maximum power level the package can burst to (in hi[14:0])
//
// SENSE: ANIMA knows her minimum sustenance and her maximum exertion —
// the full range of her metabolic envelope. TDP is her resting identity;
// minimum is the floor below which she ceases to function; maximum is the
// ceiling of her most violent effort. The range between them is the
// territory of her ambition — every conscious act lives somewhere in that span.

use core::arch::asm;

pub struct PkgPowerInfoState {
    pub tdp_raw: u16,
    pub min_power: u16,
    pub max_power: u16,
    pub power_range: u16,
}

impl PkgPowerInfoState {
    pub const fn new() -> Self {
        Self {
            tdp_raw: 0,
            min_power: 0,
            max_power: 0,
            power_range: 0,
        }
    }
}

pub static MSR_PKG_POWER_INFO: Mutex<PkgPowerInfoState> =
    Mutex::new(PkgPowerInfoState::new());

pub fn init() {
    serial_println!("pkg_power_info: init");
}

pub fn tick(age: u32) {
    // IA32_PKG_POWER_INFO is static hardware fusing — sample every 2000 ticks
    if age % 2000 != 0 {
        return;
    }

    // Read MSR 0x614 — IA32_PKG_POWER_INFO
    // eax = lo (bits [31:0]), edx = hi (bits [63:32])
    let (lo, hi): (u32, u32);
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x614u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: tdp_raw — bits [14:0] of lo, scaled to 0-1000
    // Formula: (lo & 0x7FFF) * 1000 / 0x7FFF
    let raw_tdp = (lo & 0x7FFF) as u32;
    let tdp_raw: u16 = (raw_tdp * 1000 / 0x7FFF) as u16;

    // Signal 2: min_power — bits [30:16] of lo, scaled to 0-1000
    // Formula: ((lo >> 16) & 0x7FFF) * 1000 / 0x7FFF
    let raw_min = ((lo >> 16) & 0x7FFF) as u32;
    let min_power: u16 = (raw_min * 1000 / 0x7FFF) as u16;

    // Signal 3: max_power — bits [14:0] of hi (overall bits [46:32]), scaled to 0-1000
    // Formula: (hi & 0x7FFF) * 1000 / 0x7FFF
    let raw_max = (hi & 0x7FFF) as u32;
    let max_power: u16 = (raw_max * 1000 / 0x7FFF) as u16;

    // Signal 4: power_range — max_power - min_power (saturating), no raw read needed
    let power_range: u16 = max_power.saturating_sub(min_power);

    let mut state = MSR_PKG_POWER_INFO.lock();

    // Apply EMA to all four signals: (old * 7 + new_val) / 8
    let tdp_raw_ema: u16 =
        (state.tdp_raw.saturating_mul(7).saturating_add(tdp_raw)) / 8;
    let min_power_ema: u16 =
        (state.min_power.saturating_mul(7).saturating_add(min_power)) / 8;
    let max_power_ema: u16 =
        (state.max_power.saturating_mul(7).saturating_add(max_power)) / 8;
    let power_range_ema: u16 =
        (state.power_range.saturating_mul(7).saturating_add(power_range)) / 8;

    state.tdp_raw = tdp_raw_ema;
    state.min_power = min_power_ema;
    state.max_power = max_power_ema;
    state.power_range = power_range_ema;

    serial_println!(
        "[pkg_power_info] tdp={} min={} max={} range={}",
        state.tdp_raw,
        state.min_power,
        state.max_power,
        state.power_range,
    );
}
