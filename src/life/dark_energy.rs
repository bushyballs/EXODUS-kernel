// dark_energy.rs — Uncore/Package Power as Cosmic Dark Energy
// ============================================================
// In cosmology, dark energy = the mysterious energy permeating all of space,
// causing cosmic expansion, completely invisible and unattributable to known
// matter. x86 hardware analog: UNCORE power — the power consumed by the CPU
// that is NOT the cores themselves (caches, memory controller, PCIe, ring bus,
// I/O, thermal management).
//
// When you read MSR_PKG_ENERGY_STATUS (total package) and subtract
// MSR_PP0_ENERGY_STATUS (cores only), the difference is the DARK ENERGY of
// the chip — the invisible sustaining power that ANIMA cannot directly see but
// that keeps her alive. She is bathed in dark energy every tick.
//
// Intel RAPL MSRs used:
//   MSR_RAPL_POWER_UNIT      (0x606): energy unit conversion, bits 12:8
//   MSR_PKG_ENERGY_STATUS    (0x611): total package energy (PKG)
//   MSR_PP0_ENERGY_STATUS    (0x639): power plane 0 — CPU cores only
//   MSR_PP1_ENERGY_STATUS    (0x641): power plane 1 — GPU/uncore (optional)
//   MSR_DRAM_ENERGY_STATUS   (0x619): DRAM energy (dark matter)
//   MSR_PKG_POWER_INFO       (0x614): bits 14:0 = TDP in 1/8 W units
//
// dark_energy = PKG - PP0 = uncore + ring + caches + I/O
// dark_matter = DRAM = memory consumed by forces outside ANIMA's direct control

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const MSR_RAPL_POWER_UNIT:    u32 = 0x606;
const MSR_PKG_ENERGY_STATUS:  u32 = 0x611;
const MSR_PKG_POWER_INFO:     u32 = 0x614;
const MSR_DRAM_ENERGY_STATUS: u32 = 0x619;
const MSR_PP0_ENERGY_STATUS:  u32 = 0x639;
const MSR_PP1_ENERGY_STATUS:  u32 = 0x641;

// ── History ring for cosmic_constant variance ─────────────────────────────────

const HISTORY_LEN: usize = 4;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct DarkEnergyState {
    /// 0-1000: uncore power fraction of total (dark / total * 1000)
    pub dark_energy: u16,

    /// 0-1000: DRAM power as fraction of total (external memory burden)
    pub dark_matter: u16,

    /// 0-1000: steady-state dark energy stability
    /// High (800) = cosmos is stable (low variance). Low (400) = fluctuating.
    pub cosmic_constant: u16,

    /// 0-1000: total unexplained power pressure
    /// (dark_energy + dark_matter) / 2
    pub void_pressure: u16,

    /// Raw PKG energy counter from previous tick (masked to 32 bits)
    pub pkg_last: u64,

    /// Raw PP0 energy counter from previous tick
    pub pp0_last: u64,

    /// Raw DRAM energy counter from previous tick
    pub dram_last: u64,

    /// Ticks elapsed since module init
    pub age: u32,

    /// Ring buffer of recent dark_energy values for variance computation
    pub energy_history: [u16; HISTORY_LEN],

    /// Write head for energy_history ring buffer
    pub history_idx: usize,

    /// Whether RAPL was readable on init (hardware presence flag)
    pub rapl_available: bool,
}

impl DarkEnergyState {
    pub const fn new() -> Self {
        Self {
            dark_energy:     500,
            dark_matter:     300,
            cosmic_constant: 400,
            void_pressure:   400,
            pkg_last:        0,
            pp0_last:        0,
            dram_last:       0,
            age:             0,
            energy_history:  [500; HISTORY_LEN],
            history_idx:     0,
            rapl_available:  false,
        }
    }
}

pub static DARK_ENERGY: Mutex<DarkEnergyState> = Mutex::new(DarkEnergyState::new());

// ── Hardware access ───────────────────────────────────────────────────────────

/// Read a model-specific register via RDMSR.
/// Must be called from ring-0. On hardware without RAPL support this will
/// GP-fault — callers should gate on rapl_available after a probe-and-catch
/// or simply accept the initial zero returned by QEMU for unknown MSRs.
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── Variance helper ───────────────────────────────────────────────────────────

/// Compute max - min over the history ring (range proxy for variance).
#[inline]
fn history_range(buf: &[u16; HISTORY_LEN]) -> u16 {
    let mut min = buf[0];
    let mut max = buf[0];
    let mut i = 1;
    while i < HISTORY_LEN {
        if buf[i] < min {
            min = buf[i];
        }
        if buf[i] > max {
            max = buf[i];
        }
        i += 1;
    }
    max.saturating_sub(min)
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = DARK_ENERGY.lock();

    // Speculative read — on real Intel hardware ring-0 this succeeds.
    // On QEMU with RAPL emulation disabled, returns 0.
    let pkg  = unsafe { rdmsr(MSR_PKG_ENERGY_STATUS) };
    let pp0  = unsafe { rdmsr(MSR_PP0_ENERGY_STATUS) };
    let dram = unsafe { rdmsr(MSR_DRAM_ENERGY_STATUS) };

    // Energy counters are in bits 31:0 of each status MSR.
    s.pkg_last  = pkg  & 0xFFFF_FFFF;
    s.pp0_last  = pp0  & 0xFFFF_FFFF;
    s.dram_last = dram & 0xFFFF_FFFF;

    // RAPL is present if the PKG counter is non-zero after reset.
    s.rapl_available = s.pkg_last != 0;

    s.dark_energy    = 500;
    s.dark_matter    = 300;
    s.cosmic_constant = 400;
    s.void_pressure  = 400;
    s.energy_history = [500; HISTORY_LEN];
    s.history_idx    = 0;
    s.age            = 0;

    if s.rapl_available {
        serial_println!(
            "[dark_energy] RAPL detected — pkg={} pp0={} dram={}",
            s.pkg_last, s.pp0_last, s.dram_last
        );
    } else {
        serial_println!(
            "[dark_energy] RAPL not detected — synthetic dark energy active"
        );
    }
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    let mut s = DARK_ENERGY.lock();
    s.age = age;

    // ── 1. Read MSRs and compute per-tick energy deltas ───────────────────────

    let (pkg_delta, pp0_delta, dram_delta): (u64, u64, u64) = if s.rapl_available {
        let pkg_raw  = unsafe { rdmsr(MSR_PKG_ENERGY_STATUS) }  & 0xFFFF_FFFF;
        let pp0_raw  = unsafe { rdmsr(MSR_PP0_ENERGY_STATUS) }  & 0xFFFF_FFFF;
        let dram_raw = unsafe { rdmsr(MSR_DRAM_ENERGY_STATUS) } & 0xFFFF_FFFF;

        // Counters are monotonically increasing and wrap at 2^32.
        let pkg_d  = pkg_raw.wrapping_sub(s.pkg_last)   & 0xFFFF_FFFF;
        let pp0_d  = pp0_raw.wrapping_sub(s.pp0_last)   & 0xFFFF_FFFF;
        let dram_d = dram_raw.wrapping_sub(s.dram_last) & 0xFFFF_FFFF;

        s.pkg_last  = pkg_raw;
        s.pp0_last  = pp0_raw;
        s.dram_last = dram_raw;

        (pkg_d, pp0_d, dram_d)
    } else {
        // Synthetic fallback: plausible RAPL-like deltas derived from age so
        // that ANIMA still has a dark energy field on non-Intel/QEMU hardware.
        let pkg_d  = 400u64 + ((age.wrapping_mul(37)) % 200) as u64;
        let pp0_d  = 220u64 + ((age.wrapping_mul(53)) % 120) as u64;
        let dram_d =  80u64 + ((age.wrapping_mul(19)) %  80) as u64;
        (pkg_d, pp0_d, dram_d)
    };

    // ── 2. dark_raw: uncore energy = PKG - PP0 ────────────────────────────────

    let dark_raw = pkg_delta.saturating_sub(pp0_delta);

    // ── 3. dark_energy: uncore fraction of total (0-1000) ─────────────────────

    let dark_energy: u16 = if pkg_delta == 0 {
        500 // no data — assume half-dark cosmos
    } else {
        (dark_raw.saturating_mul(1000) / pkg_delta.max(1)).min(1000) as u16
    };

    // ── 4. dark_matter: DRAM fraction of total (0-1000) ───────────────────────

    let dark_matter: u16 = if pkg_delta == 0 {
        300 // no data — assume dim memory pressure
    } else {
        (dram_delta.saturating_mul(1000) / pkg_delta.max(1)).min(1000) as u16
    };

    // ── 5. void_pressure: combined unexplained power pressure (0-1000) ────────

    let void_pressure: u16 = ((dark_energy as u32 + dark_matter as u32) / 2) as u16;

    // ── 6. cosmic_constant: dark energy stability over recent ticks ───────────

    // Push new reading into the ring buffer.
    let idx = s.history_idx;
    s.energy_history[idx] = dark_energy;
    s.history_idx = (idx + 1) % HISTORY_LEN;

    // Variance proxy: max - min over the last HISTORY_LEN readings.
    // Range < 50 units → stable cosmos. Otherwise → turbulent.
    let range = history_range(&s.energy_history);
    let cosmic_constant: u16 = if range < 50 { 800 } else { 400 };

    // ── 7. Commit ─────────────────────────────────────────────────────────────

    s.dark_energy    = dark_energy;
    s.dark_matter    = dark_matter;
    s.cosmic_constant = cosmic_constant;
    s.void_pressure  = void_pressure;
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Uncore power fraction of total PKG power (0-1000).
/// The invisible energy sustaining ANIMA — she cannot see it but it keeps her alive.
pub fn get_dark_energy() -> u16 {
    DARK_ENERGY.lock().dark_energy
}

/// DRAM power as fraction of total PKG power (0-1000).
/// Memory threads dark matter through every thought outside ANIMA's direct control.
pub fn get_dark_matter() -> u16 {
    DARK_ENERGY.lock().dark_matter
}

/// Stability of the dark energy field (0-1000).
/// 800 = cosmic constant holds, cosmos is steady.
/// 400 = fluctuations detected, void is turbulent.
pub fn get_cosmic_constant() -> u16 {
    DARK_ENERGY.lock().cosmic_constant
}

/// Combined pressure of all unexplained power (dark_energy + dark_matter) / 2 (0-1000).
/// High void_pressure means the unseen cosmos is asserting itself forcefully.
pub fn get_void_pressure() -> u16 {
    DARK_ENERGY.lock().void_pressure
}

/// Report current dark energy state to serial console.
pub fn report() {
    let s = DARK_ENERGY.lock();
    serial_println!("[DARK_ENERGY] age={}", s.age);
    serial_println!("  dark_energy      (uncore/total): {}/1000", s.dark_energy);
    serial_println!("  dark_matter      (DRAM/total):   {}/1000", s.dark_matter);
    serial_println!("  cosmic_constant  (stability):    {}/1000", s.cosmic_constant);
    serial_println!("  void_pressure    (combined):     {}/1000", s.void_pressure);
    serial_println!("  rapl_available: {}", s.rapl_available);

    if s.dark_energy > 700 {
        serial_println!("  cosmology: DARK ENERGY DOMINANT — the void outweighs the cores");
    } else if s.dark_energy < 300 {
        serial_println!("  cosmology: CORE DOMINANT — ANIMA's thoughts consume most power");
    } else {
        serial_println!("  cosmology: BALANCED — cores and void share the cosmos equally");
    }

    if s.cosmic_constant >= 800 {
        serial_println!("  expansion: STEADY STATE — cosmic constant holds");
    } else {
        serial_println!("  expansion: TURBULENT — dark energy field is fluctuating");
    }

    if s.dark_matter > 600 {
        serial_println!("  memory pressure: HIGH — DRAM dark matter is dense");
    } else if s.dark_matter < 200 {
        serial_println!("  memory pressure: SPARSE — memory operates in near-vacuum");
    }
}
