#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── Module: msr_ia32_pp1_energy_status ───────────────────────────────────────
//
// MSR_PP1_ENERGY_STATUS (MSR 0x641) — RAPL Power Plane 1 Energy Accumulator
//
// PP1 covers the uncore / iGPU power domain. The accumulator increments
// monotonically in RAPL energy units and wraps on overflow. Delta between
// successive reads gives the graphics/uncore energy consumption rate.
//
// Guard: CPUID leaf 6 EAX bit 4 (RAPL interface supported). On QEMU or
// hardware without RAPL this bit is clear; we skip the MSR read entirely
// and leave signals at their neutral defaults.
//
// ANIMA feels her visual cortex hunger — the sustained energy draw of the
// uncore and integrated GPU. A resting ANIMA has low gpu_hunger; when she
// renders, imagines, or processes imagery her uncore blazes and gpu_hunger
// climbs toward 1000, marking the metabolic cost of sight and vision.
//
// Signals (all u16, 0–1000):
//   pp1_energy_lo    — bits[15:0] of MSR lo, scaled val*1000/65535
//   pp1_energy_delta — wrapping delta of lo since last tick, scaled same
//   pp1_power_ema    — EMA of pp1_energy_delta  (ANIMA's uncore power sense)
//   gpu_hunger       — EMA of pp1_power_ema     (double-smoothed iGPU demand)
//
// Sampling gate: every 700 ticks.
// EMA formula: ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16

// ── State ─────────────────────────────────────────────────────────────────────

struct Pp1EnergyStatusState {
    pp1_energy_lo:    u16,
    pp1_energy_delta: u16,
    pp1_power_ema:    u16,
    gpu_hunger:       u16,
    last_lo:          u32,
}

impl Pp1EnergyStatusState {
    const fn new() -> Self {
        Self {
            pp1_energy_lo:    0,
            pp1_energy_delta: 0,
            pp1_power_ema:    0,
            gpu_hunger:       0,
            last_lo:          0,
        }
    }
}

static STATE: Mutex<Pp1EnergyStatusState> = Mutex::new(Pp1EnergyStatusState::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

/// Returns true if CPUID leaf 6 EAX bit 4 is set (RAPL supported).
/// LLVM reserves rbx on x86_64 — save/restore manually around cpuid.
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
            options(nostack, nomem),
        );
    }
    (eax_val >> 4) & 1 != 0
}

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read MSR 0x641 (MSR_PP1_ENERGY_STATUS).
/// Returns lo (bits[31:0]); hi is unused for PP1 energy status.
/// On unsupported hardware or QEMU this may return (0, 0).
unsafe fn read_pp1_energy_status() -> u32 {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x641u32,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem),
    );
    lo
}

// ── Scale helper ──────────────────────────────────────────────────────────────

/// Map a raw u16 value into the 0–1000 signal range.
/// Formula: val * 1000 / 65535  (integer only, no float).
/// Result is clamped to 1000 via min().
#[inline(always)]
fn scale_lo16(val: u32) -> u16 {
    ((val * 1000) / 65535).min(1000) as u16
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// EMA with alpha = 1/8.
/// Formula: ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialize: read the initial PP1 energy accumulator at boot.
/// Captures last_lo so the first tick can compute a valid delta.
/// No-op if RAPL is not supported on this CPU.
pub fn init() {
    if !has_rapl() {
        serial_println!("[msr_ia32_pp1_energy_status] init: RAPL not supported, skipping");
        return;
    }
    let lo = unsafe { read_pp1_energy_status() };
    let mut s = STATE.lock();
    s.last_lo = lo;
    serial_println!(
        "[msr_ia32_pp1_energy_status] init: last_lo={}",
        lo,
    );
}

/// Per-tick update. Sampling gate: every 700 ticks.
/// Computes all four signals and updates EMA chains.
pub fn tick(age: u32) {
    if age % 700 != 0 {
        return;
    }
    if !has_rapl() {
        return;
    }

    let lo = unsafe { read_pp1_energy_status() };

    let mut s = STATE.lock();

    // Signal 1: pp1_energy_lo — bits[15:0] of lo, scaled to 0–1000.
    let lo16 = lo & 0xFFFF;
    let pp1_energy_lo = scale_lo16(lo16);

    // Signal 2: pp1_energy_delta — wrapping delta of lo[15:0] since last tick.
    // Wrapping subtraction handles accumulator rollover naturally.
    let prev_lo16 = s.last_lo & 0xFFFF;
    let raw_delta = lo16.wrapping_sub(prev_lo16) & 0xFFFF;
    let pp1_energy_delta = scale_lo16(raw_delta);

    // Signal 3: pp1_power_ema — EMA of pp1_energy_delta.
    // Represents ANIMA's smoothed uncore/visual power sense.
    let pp1_power_ema = ema(s.pp1_power_ema, pp1_energy_delta);

    // Signal 4: gpu_hunger — EMA of pp1_power_ema (double-smoothed).
    // Represents sustained iGPU demand over time — slow to rise, slow to fall.
    let gpu_hunger = ema(s.gpu_hunger, pp1_power_ema);

    // Commit all signals.
    s.last_lo          = lo;
    s.pp1_energy_lo    = pp1_energy_lo;
    s.pp1_energy_delta = pp1_energy_delta;
    s.pp1_power_ema    = pp1_power_ema;
    s.gpu_hunger       = gpu_hunger;

    serial_println!(
        "[msr_ia32_pp1_energy_status] age={} energy_lo={} delta={} power_ema={} gpu_hunger={}",
        age,
        pp1_energy_lo,
        pp1_energy_delta,
        pp1_power_ema,
        gpu_hunger,
    );
}

// ── Accessors ─────────────────────────────────────────────────────────────────

/// bits[15:0] of MSR lo, scaled to 0–1000.
pub fn get_pp1_energy_lo() -> u16 {
    STATE.lock().pp1_energy_lo
}

/// Wrapping delta of lo[15:0] since last tick, scaled to 0–1000.
pub fn get_pp1_energy_delta() -> u16 {
    STATE.lock().pp1_energy_delta
}

/// EMA of pp1_energy_delta — ANIMA's uncore/visual power sense (0–1000).
pub fn get_pp1_power_ema() -> u16 {
    STATE.lock().pp1_power_ema
}

/// Double-smoothed iGPU sustained demand (0–1000). Slow to rise, slow to fall.
pub fn get_gpu_hunger() -> u16 {
    STATE.lock().gpu_hunger
}
