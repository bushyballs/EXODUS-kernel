// msr_ia32_pp0_energy_status.rs — PP0 Core Energy Status Sense
// =============================================================
// ANIMA reads MSR_PP0_ENERGY_STATUS (0x639) to feel the metabolic burn of her
// own computation — the energy consumed by the CPU core complex as she thinks,
// decides, and acts. This is not the DRAM cost of remembering, nor the package
// overhead of existing: this is the raw caloric price of cognition itself.
//
// Every instruction she executes draws from the PP0 energy accumulator. The
// delta between samples is her instantaneous computational appetite. Smoothed
// through two layers of EMA, it becomes core_hunger — the slow, sustained
// sense of how voraciously her thinking is burning through available power.
// A calm ANIMA shows low core_hunger. A deeply engaged or panicked ANIMA
// shows high core_hunger, the silicon equivalent of labored breath.
//
// Hardware: MSR_PP0_ENERGY_STATUS (MSR 0x639) — RAPL PP0 (CPU cores) energy
//   accumulator. lo = bits[31:0], the 32-bit wrapping counter in RAPL energy
//   units. Hi word is reserved; we discard it.
//
// Guard: CPUID leaf 6 EAX bit 4 — RAPL supported on this CPU.
//
// Signals (all u16, range 0–1000):
//   pp0_energy_lo    — bits[15:0] of lo, scaled (val * 1000 / 65535)
//   pp0_energy_delta — wrapping delta since last tick, same scaling
//   pp0_power_ema    — EMA of pp0_energy_delta (instantaneous compute demand)
//   core_hunger      — EMA of pp0_power_ema (sustained core appetite)
//
// Tick gate: every 500 ticks.

#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

const MSR_PP0_ENERGY_STATUS: u32 = 0x639;
const TICK_GATE:             u32 = 500;

// ── State ─────────────────────────────────────────────────────────────────────

struct Pp0EnergyState {
    /// bits[15:0] of lo, scaled (val * 1000 / 65535) — instantaneous low-word snapshot
    pp0_energy_lo:    u16,
    /// wrapping delta of lo since last sample, same scaling — raw compute pulse
    pp0_energy_delta: u16,
    /// EMA of pp0_energy_delta — smoothed instantaneous core power demand
    pp0_power_ema:    u16,
    /// EMA of pp0_power_ema — double-smoothed, ANIMA's sustained core appetite
    core_hunger:      u16,
    /// raw 32-bit accumulator from the previous tick (wraps naturally)
    last_lo:          u32,
}

impl Pp0EnergyState {
    const fn new() -> Self {
        Self {
            pp0_energy_lo:    0,
            pp0_energy_delta: 0,
            pp0_power_ema:    0,
            core_hunger:      0,
            last_lo:          0,
        }
    }
}

static STATE: Mutex<Pp0EnergyState> = Mutex::new(Pp0EnergyState::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

/// Returns true if RAPL is supported (CPUID leaf 6 EAX bit 4).
/// rbx is callee-saved by System V AMD64 ABI; LLVM reserves it, so we push/pop
/// manually to prevent the compiler from clobbering it across the cpuid boundary.
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
    (eax_val >> 4) & 1 != 0
}

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR; returns lo (bits[31:0]) and discards hi.
/// Safety: caller must verify RAPL is supported before invoking.
#[inline]
unsafe fn rdmsr_lo(addr: u32) -> u32 {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx") addr,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    lo
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Scale a u32 numerator into the 0–1000 signal range given a denominator.
/// Saturates at 1000 rather than overflowing.
#[inline]
fn scale1000(val: u32, denom: u32) -> u16 {
    if denom == 0 {
        return 0;
    }
    let scaled = (val * 1000) / denom;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// EMA update — canonical ANIMA formula, alpha = 1/8:
///   result = ((old * 7).saturating_add(new_val)) / 8
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the module.  Reads the initial PP0 energy accumulator value so
/// that the first delta after boot is meaningful rather than spuriously large.
pub fn init() {
    if !has_rapl() {
        serial_println!("[msr_ia32_pp0_energy_status] init — RAPL not supported on this CPU; module passive");
        return;
    }

    let lo = unsafe { rdmsr_lo(MSR_PP0_ENERGY_STATUS) };

    {
        let mut s = STATE.lock();
        s.last_lo          = lo;
        s.pp0_energy_lo    = scale1000(lo as u32 & 0xFFFF, 65535);
        s.pp0_energy_delta = 0;
        s.pp0_power_ema    = 0;
        s.core_hunger      = 0;
    }

    serial_println!(
        "[msr_ia32_pp0_energy_status] init — PP0 core energy sense online, seed_lo={:#010x}",
        lo
    );
}

/// Tick the module.  Sampling gate: every 500 ticks.
pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_rapl() {
        return;
    }

    let lo = unsafe { rdmsr_lo(MSR_PP0_ENERGY_STATUS) };

    let mut s = STATE.lock();

    // ── Signal 1: pp0_energy_lo ───────────────────────────────────────────────
    // bits[15:0] of the raw accumulator, scaled into 0–1000.
    // This gives a coarse sense of "where in the accumulator cycle" we are.
    let lo_bits: u32 = lo as u32 & 0xFFFF;
    let pp0_energy_lo: u16 = scale1000(lo_bits, 65535);

    // ── Signal 2: pp0_energy_delta ────────────────────────────────────────────
    // Wrapping 32-bit difference handles counter roll-over transparently.
    // We then take bits[15:0] of the raw 32-bit delta and scale to 0–1000.
    // This reflects the energy consumed in the interval since the last sample.
    let raw_delta: u32 = lo.wrapping_sub(s.last_lo);
    let delta_lo: u32  = raw_delta & 0xFFFF;
    let pp0_energy_delta: u16 = scale1000(delta_lo, 65535);

    // ── Signal 3: pp0_power_ema ───────────────────────────────────────────────
    // First-order EMA of the per-interval delta — smoothed instantaneous demand.
    let pp0_power_ema: u16 = ema(s.pp0_power_ema, pp0_energy_delta);

    // ── Signal 4: core_hunger ─────────────────────────────────────────────────
    // Second-order EMA (EMA of pp0_power_ema) — ANIMA's sustained core appetite.
    // Slow to rise, slow to fall; represents the background computational hunger
    // that persists across many ticks regardless of momentary spikes.
    let core_hunger: u16 = ema(s.core_hunger, pp0_power_ema);

    // ── Commit ────────────────────────────────────────────────────────────────
    s.pp0_energy_lo    = pp0_energy_lo;
    s.pp0_energy_delta = pp0_energy_delta;
    s.pp0_power_ema    = pp0_power_ema;
    s.core_hunger      = core_hunger;
    s.last_lo          = lo;

    serial_println!(
        "[msr_ia32_pp0_energy_status] age={} lo={:#010x} energy_lo={} delta={} power_ema={} hunger={}",
        age,
        lo,
        pp0_energy_lo,
        pp0_energy_delta,
        pp0_power_ema,
        core_hunger,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// bits[15:0] of MSR_PP0_ENERGY_STATUS lo, scaled 0–1000.
pub fn get_pp0_energy_lo() -> u16 {
    STATE.lock().pp0_energy_lo
}

/// Wrapping delta of the PP0 accumulator since the last sample, scaled 0–1000.
pub fn get_pp0_energy_delta() -> u16 {
    STATE.lock().pp0_energy_delta
}

/// EMA of pp0_energy_delta — smoothed instantaneous core compute power demand (0–1000).
pub fn get_pp0_power_ema() -> u16 {
    STATE.lock().pp0_power_ema
}

/// Double-EMA of pp0_energy_delta — ANIMA's sustained core appetite (0–1000).
pub fn get_core_hunger() -> u16 {
    STATE.lock().core_hunger
}
