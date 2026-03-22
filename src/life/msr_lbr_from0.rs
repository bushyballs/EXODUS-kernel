#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct LbrFrom0State {
    from0_lo_sense: u16,
    to0_lo_sense:   u16,
    branch_distance: u16,
    lbr0_ema:       u16,
}

impl LbrFrom0State {
    const fn new() -> Self {
        Self {
            from0_lo_sense:  0,
            to0_lo_sense:    0,
            branch_distance: 0,
            lbr0_ema:        0,
        }
    }
}

static STATE: Mutex<LbrFrom0State> = Mutex::new(LbrFrom0State::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

fn has_lbr_v2() -> bool {
    // Check PDCM (leaf 1, ECX bit 15) as proxy for LBR availability.
    let ecx1: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") ecx1,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    if (ecx1 >> 15) & 1 == 0 {
        return false;
    }

    // Confirm CPUID max leaf >= 0x1C.
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    if max_leaf < 0x1C {
        return false;
    }

    // Leaf 0x1C EAX != 0 means LBR is enumerated via CPUID.
    let eax_1c: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x1Cu32 => eax_1c,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    eax_1c != 0
}

// ── RDMSR helpers ─────────────────────────────────────────────────────────────

/// Read the low 32 bits of an MSR.  The high 32 bits (EDX) are discarded;
/// we only need the address portion that fits in 32 bits for signal mapping.
#[inline]
unsafe fn rdmsr_lo(msr: u32) -> u32 {
    let lo: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        lateout("eax") lo,
        lateout("edx") _,
        options(nostack, nomem)
    );
    lo
}

// ── Signal helpers ────────────────────────────────────────────────────────────

/// Map bits[31:16] of a 32-bit MSR readout to 0–1000.
/// bits[31:16] span 0–65535; scale: value * 1000 / 65535 (integer, no floats).
#[inline]
fn hi16_to_sense(raw: u32) -> u16 {
    let hi = (raw >> 16) & 0xFFFF;           // 0–65535
    ((hi as u32) * 1000 / 65535) as u16      // 0–1000
}

/// Compute branch_distance from the low 16 bits of from0 and to0.
/// Formula: (((from & 0xFFFF).wrapping_sub(to & 0xFFFF) & 0xFFFF) * 1000 / 65536) as u16
#[inline]
fn branch_distance(from0_lo: u32, to0_lo: u32) -> u16 {
    let from_lo = from0_lo & 0xFFFF;
    let to_lo   = to0_lo   & 0xFFFF;
    let diff    = from_lo.wrapping_sub(to_lo) & 0xFFFF;    // unsigned wrap, 0–65535
    (diff * 1000 / 65536) as u16                            // 0–999 (max safe)
}

/// EMA: (old * 7 + new_val) / 8, computed in u32, cast to u16.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    (((old as u32) * 7 + (new_val as u32)) / 8) as u16
}

// ── Public interface ──────────────────────────────────────────────────────────

pub fn init() {
    if !has_lbr_v2() {
        crate::serial_println!("[msr_lbr_from0] CPUID guard: LBR not available on this CPU — module idle");
        return;
    }
    crate::serial_println!("[msr_lbr_from0] init: LBR from0/to0 (MSR 0x680 / 0x6C0) ready");
}

pub fn tick(age: u32) {
    // Sample every 200 ticks.
    if age % 200 != 0 {
        return;
    }

    if !has_lbr_v2() {
        return;
    }

    // Read MSR_LBR_FROM_0 (0x680) and MSR_LBR_TO_0 (0x6C0).
    let from0_lo = unsafe { rdmsr_lo(0x680) };
    let to0_lo   = unsafe { rdmsr_lo(0x6C0) };

    // Compute signals.
    let from0_lo_sense_val  = hi16_to_sense(from0_lo);
    let to0_lo_sense_val    = hi16_to_sense(to0_lo);
    let branch_distance_val = branch_distance(from0_lo, to0_lo);

    let mut st = STATE.lock();
    let new_ema = ema(st.lbr0_ema, branch_distance_val);

    st.from0_lo_sense  = from0_lo_sense_val;
    st.to0_lo_sense    = to0_lo_sense_val;
    st.branch_distance = branch_distance_val;
    st.lbr0_ema        = new_ema;

    crate::serial_println!(
        "[msr_lbr_from0] age={} from={} to={} dist={} ema={}",
        age,
        from0_lo_sense_val,
        to0_lo_sense_val,
        branch_distance_val,
        new_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_from0_lo_sense() -> u16 {
    STATE.lock().from0_lo_sense
}

pub fn get_to0_lo_sense() -> u16 {
    STATE.lock().to0_lo_sense
}

pub fn get_branch_distance() -> u16 {
    STATE.lock().branch_distance
}

pub fn get_lbr0_ema() -> u16 {
    STATE.lock().lbr0_ema
}
