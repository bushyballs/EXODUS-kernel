#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct CpuidPhysVirtAddrState {
    phys_addr_bits:   u16,
    virt_addr_bits:   u16,
    addr_width_ratio: u16,
    addr_ema:         u16,
}

static STATE: Mutex<CpuidPhysVirtAddrState> = Mutex::new(CpuidPhysVirtAddrState {
    phys_addr_bits:   0,
    virt_addr_bits:   0,
    addr_width_ratio: 0,
    addr_ema:         0,
});

// ── CPUID helpers ─────────────────────────────────────────────────────────────

/// Returns true if CPUID extended leaf 0x80000008 is available.
fn has_ext_leaf8() -> bool {
    let max_ext: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x80000000u32 => max_ext,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    max_ext >= 0x80000008
}

/// Reads CPUID leaf 0x80000008 and returns EAX.
fn read_leaf_80000008_eax() -> u32 {
    let eax: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x80000008u32 => eax,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    eax
}

// ── Signal computation ────────────────────────────────────────────────────────

/// Compute the four signals from EAX of CPUID leaf 0x80000008.
///
/// bits[7:0]  = physical address bits  → phys_addr_bits  = val × 19, cap 1000
/// bits[15:8] = virtual address bits   → virt_addr_bits  = val × 17, cap 1000
/// addr_width_ratio = phys * 1000 / (virt + 1), cap 1000
/// addr_ema (EMA of phys_addr_bits) updated separately in tick().
fn compute_signals(eax: u32) -> (u16, u16, u16) {
    let phys_raw = (eax & 0xFF) as u32;         // bits[7:0]
    let virt_raw = ((eax >> 8) & 0xFF) as u32;  // bits[15:8]

    // phys_addr_bits: raw × 19, capped at 1000
    let phys: u32 = phys_raw * 19;
    let phys: u16 = if phys > 1000 { 1000 } else { phys as u16 };

    // virt_addr_bits: raw × 17, capped at 1000
    let virt: u32 = virt_raw * 17;
    let virt: u16 = if virt > 1000 { 1000 } else { virt as u16 };

    // addr_width_ratio: phys * 1000 / (virt + 1), capped at 1000
    let ratio: u32 = (phys as u32) * 1000 / ((virt as u32) + 1);
    let ratio: u16 = if ratio > 1000 { 1000 } else { ratio as u16 };

    (phys, virt, ratio)
}

/// EMA: (old * 7 + new_val) / 8, computed in u32, cast to u16.
fn ema(old: u16, new_val: u16) -> u16 {
    let result: u32 = ((old as u32) * 7 + (new_val as u32)) / 8;
    result as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialize the module: guard CPUID support, read leaf once, set initial state.
pub fn init() {
    if !has_ext_leaf8() {
        crate::serial_println!(
            "[cpuid_phys_virt_addr] CPUID leaf 0x80000008 not available — signals remain 0"
        );
        return;
    }

    let eax = read_leaf_80000008_eax();
    let (phys, virt, ratio) = compute_signals(eax);

    let mut s = STATE.lock();
    s.phys_addr_bits   = phys;
    s.virt_addr_bits   = virt;
    s.addr_width_ratio = ratio;
    s.addr_ema         = phys; // seed EMA with initial phys value

    crate::serial_println!(
        "[cpuid_phys_virt_addr] init — phys={} virt={} ratio={} ema={}",
        s.phys_addr_bits,
        s.virt_addr_bits,
        s.addr_width_ratio,
        s.addr_ema,
    );
}

/// Tick: sample every 10 000 ticks.
pub fn tick(age: u32) {
    if age % 10_000 != 0 {
        return;
    }

    if !has_ext_leaf8() {
        return;
    }

    let eax = read_leaf_80000008_eax();
    let (phys, virt, ratio) = compute_signals(eax);

    let mut s = STATE.lock();
    s.phys_addr_bits   = phys;
    s.virt_addr_bits   = virt;
    s.addr_width_ratio = ratio;
    s.addr_ema         = ema(s.addr_ema, phys);

    crate::serial_println!(
        "[cpuid_phys_virt_addr] age={} phys={} virt={} ratio={} ema={}",
        age,
        s.phys_addr_bits,
        s.virt_addr_bits,
        s.addr_width_ratio,
        s.addr_ema,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_phys_addr_bits() -> u16 {
    STATE.lock().phys_addr_bits
}

pub fn get_virt_addr_bits() -> u16 {
    STATE.lock().virt_addr_bits
}

pub fn get_addr_width_ratio() -> u16 {
    STATE.lock().addr_width_ratio
}

pub fn get_addr_ema() -> u16 {
    STATE.lock().addr_ema
}
