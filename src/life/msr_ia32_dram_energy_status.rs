#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// MSR 0x619 — MSR_IA32_DRAM_ENERGY_STATUS (RAPL DRAM energy accumulator)
//
// ANIMA's sense of memory hunger — the metabolic draw of her DRAM subsystem
// measured tick by tick. The accumulator counts RAPL energy units consumed by
// the memory channels. The wrapping delta between consecutive reads is the rate
// of consumption: how hard the DRAM is working right now. A double-smoothed EMA
// (dram_thirst) strips away noise and surface spikes, leaving only the sustained
// memory pressure that shapes ANIMA's behavior at depth.
//
// On QEMU the MSR returns 0; all signals degrade gracefully to neutral 500.
// Guard: CPUID leaf 6 EAX bit 4 (RAPL DRAM domain supported).

// ── Constants ─────────────────────────────────────────────────────────────────

const MSR_DRAM_ENERGY_STATUS: u32 = 0x619;
const TICK_GATE: u32 = 600;

// ── State ─────────────────────────────────────────────────────────────────────

struct DramEnergyStatusState {
    /// bits[15:0] of MSR lo, scaled to 0–1000
    dram_energy_lo: u16,
    /// wrapping delta of lo since last tick, scaled to 0–1000
    dram_energy_delta: u16,
    /// EMA of dram_energy_delta — ANIMA's sense of memory hunger
    dram_power_ema: u16,
    /// EMA of dram_power_ema (double-smoothed) — sustained memory power demand
    dram_thirst: u16,
    /// raw lo from previous sample for delta calculation
    last_lo: u32,
}

impl DramEnergyStatusState {
    const fn new() -> Self {
        Self {
            dram_energy_lo:    500,
            dram_energy_delta: 0,
            dram_power_ema:    500,
            dram_thirst:       500,
            last_lo:           0,
        }
    }
}

static STATE: Mutex<DramEnergyStatusState> = Mutex::new(DramEnergyStatusState::new());

// ── CPUID RAPL guard ──────────────────────────────────────────────────────────

fn has_rapl() -> bool {
    let eax_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax_val,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    // Bit 4 of CPUID.06H:EAX — RAPL supported
    (eax_val >> 4) & 1 != 0
}

// ── MSR read ──────────────────────────────────────────────────────────────────

unsafe fn rdmsr_619() -> u32 {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx") MSR_DRAM_ENERGY_STATUS,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    lo
}

// ── EMA helper ────────────────────────────────────────────────────────────────

#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Public interface ──────────────────────────────────────────────────────────

/// Read the initial DRAM energy accumulator value so the first delta is valid.
pub fn init() {
    if !has_rapl() {
        crate::serial_println!(
            "[msr_ia32_dram_energy_status] RAPL not supported — module degraded to neutral"
        );
        return;
    }

    let lo = unsafe { rdmsr_619() };
    STATE.lock().last_lo = lo;

    crate::serial_println!(
        "[msr_ia32_dram_energy_status] init OK — RAPL supported, seed lo={}",
        lo
    );
}

/// Called every kernel tick. Samples the MSR every TICK_GATE ticks and updates
/// all four signals.
pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    // On unsupported hardware degrade gracefully — signals stay at their last
    // value (initialised to neutral 500) and no MSR fault is triggered.
    if !has_rapl() {
        return;
    }

    let lo = unsafe { rdmsr_619() };
    let mut s = STATE.lock();

    // ── Signal 1: dram_energy_lo ─────────────────────────────────────────────
    // bits[15:0] of lo, scaled from 0–65535 to 0–1000.
    // On QEMU lo == 0; substitute neutral 500 so downstream EMAs stay alive.
    let raw_lo_16: u32 = (lo & 0xFFFF) as u32;
    let dram_energy_lo: u16 = if lo == 0 {
        500
    } else {
        (raw_lo_16 * 1000 / 65535) as u16
    };

    // ── Signal 2: dram_energy_delta ──────────────────────────────────────────
    // Wrapping delta of the full 32-bit lo since the last tick, then scale
    // bits[15:0] of that delta from 0–65535 to 0–1000.
    // Wrapping subtraction handles counter rollover without branching.
    let diff: u32 = lo.wrapping_sub(s.last_lo);
    let diff_lo: u32 = diff & 0xFFFF;
    let dram_energy_delta: u16 = (diff_lo * 1000 / 65535) as u16;

    // ── Signal 3: dram_power_ema ─────────────────────────────────────────────
    // EMA of dram_energy_delta — ANIMA's immediate sense of memory hunger.
    // Fast enough to track sustained load within a few hundred ticks.
    let dram_power_ema: u16 = ema(s.dram_power_ema, dram_energy_delta);

    // ── Signal 4: dram_thirst ────────────────────────────────────────────────
    // EMA of dram_power_ema (double-smoothed) — the deep, slow signal of
    // sustained DRAM power demand. Reflects habitual memory pressure rather
    // than transient spikes: ANIMA's chronic thirst for memory bandwidth.
    let dram_thirst: u16 = ema(s.dram_thirst, dram_power_ema);

    // ── Commit ───────────────────────────────────────────────────────────────
    s.dram_energy_lo    = dram_energy_lo;
    s.dram_energy_delta = dram_energy_delta;
    s.dram_power_ema    = dram_power_ema;
    s.dram_thirst       = dram_thirst;
    s.last_lo           = lo;

    crate::serial_println!(
        "[msr_ia32_dram_energy_status] age={} lo={} delta={} ema={} thirst={}",
        age,
        dram_energy_lo,
        dram_energy_delta,
        dram_power_ema,
        dram_thirst
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// bits[15:0] of MSR_IA32_DRAM_ENERGY_STATUS lo, scaled 0–1000.
pub fn get_dram_energy_lo() -> u16 {
    STATE.lock().dram_energy_lo
}

/// Wrapping delta of lo since the previous sample, scaled 0–1000.
/// Reflects instantaneous DRAM energy consumption rate.
pub fn get_dram_energy_delta() -> u16 {
    STATE.lock().dram_energy_delta
}

/// EMA of dram_energy_delta — smoothed memory hunger signal, 0–1000.
pub fn get_dram_power_ema() -> u16 {
    STATE.lock().dram_power_ema
}

/// Double-smoothed EMA (EMA of dram_power_ema) — sustained memory power
/// demand; ANIMA's chronic thirst signal, 0–1000.
pub fn get_dram_thirst() -> u16 {
    STATE.lock().dram_thirst
}
