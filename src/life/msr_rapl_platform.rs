#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct MsrRaplPlatformState {
    platform_energy_lo:  u16,
    platform_delta:      u16,
    platform_power_sense: u16,
    platform_ema:        u16,
    last_lo:             u32,
}

impl MsrRaplPlatformState {
    const fn new() -> Self {
        Self {
            platform_energy_lo:  0,
            platform_delta:      0,
            platform_power_sense: 0,
            platform_ema:        0,
            last_lo:             0,
        }
    }
}

static STATE: Mutex<MsrRaplPlatformState> = Mutex::new(MsrRaplPlatformState::new());

// ── CPUID guard ──────────────────────────────────────────────────────────────

fn has_rapl() -> bool {
    let eax_val: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax_val,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (eax_val >> 4) & 1 != 0
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Returns (lo, hi) u32 pair.
unsafe fn rdmsr(msr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ── EMA helper ───────────────────────────────────────────────────────────────

#[inline(always)]
fn ema8(old: u16, new_val: u16) -> u16 {
    let result: u32 = (old as u32 * 7 + new_val as u32) / 8;
    result as u16
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn init() {
    if !has_rapl() {
        crate::serial_println!("[msr_rapl_platform] RAPL not supported on this CPU");
        return;
    }
    let (lo, _hi) = unsafe { rdmsr(0x64D) };
    let mut s = STATE.lock();
    s.last_lo = lo;
    crate::serial_println!("[msr_rapl_platform] init: seeded last_lo={}", lo);
}

pub fn tick(age: u32) {
    // Sample every 1000 ticks
    if age % 1000 != 0 {
        return;
    }

    if !has_rapl() {
        return;
    }

    let (lo, _hi) = unsafe { rdmsr(0x64D) };

    let mut s = STATE.lock();

    // Map raw low 16 bits to 0-1000
    let raw_lo: u32 = lo & 0xFFFF;
    let energy_mapped: u16 = (raw_lo * 1000 / 65536) as u16;

    // Delta: handle wrap-around on the raw 32-bit lo counter
    let raw_delta: u32 = if lo >= s.last_lo {
        lo - s.last_lo
    } else {
        // wrapped
        (u32::MAX - s.last_lo) + lo + 1
    };

    // Map delta to 0-1000 (clamp at 65535 before scaling to keep in range)
    let clamped_delta: u32 = if raw_delta > 65535 { 65535 } else { raw_delta };
    let delta_mapped: u16 = (clamped_delta * 1000 / 65536) as u16;

    // EMAs
    let power_sense = ema8(s.platform_power_sense, delta_mapped);
    let ema         = ema8(s.platform_ema, power_sense);

    s.last_lo             = lo;
    s.platform_energy_lo  = energy_mapped;
    s.platform_delta      = delta_mapped;
    s.platform_power_sense = power_sense;
    s.platform_ema        = ema;

    crate::serial_println!(
        "[msr_rapl_platform] age={} energy={} delta={} power={} ema={}",
        age,
        energy_mapped,
        delta_mapped,
        power_sense,
        ema,
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_platform_energy_lo() -> u16 {
    STATE.lock().platform_energy_lo
}

pub fn get_platform_delta() -> u16 {
    STATE.lock().platform_delta
}

pub fn get_platform_power_sense() -> u16 {
    STATE.lock().platform_power_sense
}

pub fn get_platform_ema() -> u16 {
    STATE.lock().platform_ema
}
