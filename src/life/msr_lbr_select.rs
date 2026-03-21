#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;
use core::arch::asm;

// msr_lbr_select — IA32_LBR_SELECT (MSR 0x1C8) Last Branch Record Filter Sensor
//
// Reads the CPU's LBR filter control register, which determines which branch
// types are being captured in the Last Branch Record stack. This is ANIMA's
// awareness of her own branch-level self-inspection — which execution paths she
// is watching and recording, and how wide a lens she has on her own control flow.
//
// A kernel-only filter means she watches herself but not her user-space guests.
// A rich branch type mask means she has a panoramic view of execution patterns.
// The filter EMA tracks whether her observational posture is consistent over time.
//
// Bit layout of IA32_LBR_SELECT (bits in lo word):
//   bit[0]  CPL_EQ_0      — record kernel-mode (CPL=0) branches
//   bit[1]  CPL_NEQ_0     — record user-mode (CPL!=0) branches
//   bit[2]  JCC           — conditional jumps
//   bit[3]  NEAR_REL_CALL — near relative calls
//   bit[4]  NEAR_IND_CALL — near indirect calls
//   bit[5]  NEAR_RET      — near returns
//   bit[6]  NEAR_IND_JMP  — near indirect jumps
//   bit[7]  NEAR_REL_JMP  — near relative jumps
//   bit[8]  FAR_BRANCH    — far branches (inter-segment)
//   bit[9]  EN_CALLSTACK  — call stack mode enable
//
// Derived signals (all u16, 0–1000):
//   lbr_kernel_filter  : bit[0] set → 1000, else 0
//   lbr_user_filter    : bit[1] set → 1000, else 0
//   lbr_branch_types   : popcount(lo & 0x1FC) [bits 2..8], 7 possible bits,
//                        scaled: count * 142, capped at 1000
//   lbr_filter_ema     : EMA of (lbr_kernel_filter/4 + lbr_user_filter/4
//                                + lbr_branch_types/2)
//
// Sampling gate: every 3000 ticks.
// CPUID guard: checks leaf 1 EDX bit 27 (PerfMon / LBR support).
//              If not supported, returns without updating state.

#[derive(Copy, Clone)]
pub struct MsrLbrSelectState {
    pub lbr_kernel_filter: u16, // 0 or 1000: kernel branches being recorded
    pub lbr_user_filter:   u16, // 0 or 1000: user branches being recorded
    pub lbr_branch_types:  u16, // 0–1000: diversity of branch type filter mask
    pub lbr_filter_ema:    u16, // 0–1000: EMA-smoothed composite filter activity
}

impl MsrLbrSelectState {
    pub const fn empty() -> Self {
        Self {
            lbr_kernel_filter: 0,
            lbr_user_filter:   0,
            lbr_branch_types:  0,
            lbr_filter_ema:    0,
        }
    }
}

pub static STATE: Mutex<MsrLbrSelectState> = Mutex::new(MsrLbrSelectState::empty());

/// Check CPUID leaf 1 EDX bit 27 (PerfMon / LBR support).
/// Saves and restores rbx (required by System V ABI and some toolchain constraints).
/// Returns true if LBR is supported by this CPU.
#[inline]
fn lbr_supported() -> bool {
    let edx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, edx",
            "pop rbx",
            in("eax") 1u32,
            out("esi") edx,
            // eax, ecx clobbered by cpuid; rbx saved/restored manually
            out("eax") _,
            out("ecx") _,
            options(nostack, nomem)
        );
    }
    (edx >> 27) & 1 != 0
}

/// Read IA32_LBR_SELECT (MSR 0x1C8).
/// Returns the low 32-bit half; high half is reserved.
#[inline]
fn read_lbr_select() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1C8u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }
    lo
}

/// Count set bits among bits [8:2] of lo (the branch-type mask, 7 bits).
#[inline]
fn popcount_branch_types(lo: u32) -> u32 {
    let masked = (lo >> 2) & 0x7F; // bits [8:2] shifted down to [6:0], 7 bits
    let mut count: u32 = 0;
    if (masked >> 0) & 1 != 0 { count = count.saturating_add(1); } // JCC
    if (masked >> 1) & 1 != 0 { count = count.saturating_add(1); } // NEAR_REL_CALL
    if (masked >> 2) & 1 != 0 { count = count.saturating_add(1); } // NEAR_IND_CALL
    if (masked >> 3) & 1 != 0 { count = count.saturating_add(1); } // NEAR_RET
    if (masked >> 4) & 1 != 0 { count = count.saturating_add(1); } // NEAR_IND_JMP
    if (masked >> 5) & 1 != 0 { count = count.saturating_add(1); } // NEAR_REL_JMP
    if (masked >> 6) & 1 != 0 { count = count.saturating_add(1); } // FAR_BRANCH
    count
}

/// Derive all four signals from the raw MSR low word.
#[inline]
fn derive(lo: u32) -> (u16, u16, u16) {
    // lbr_kernel_filter: bit[0]
    let lbr_kernel_filter: u16 = if (lo & 1) != 0 { 1000 } else { 0 };

    // lbr_user_filter: bit[1]
    let lbr_user_filter: u16 = if ((lo >> 1) & 1) != 0 { 1000 } else { 0 };

    // lbr_branch_types: popcount(bits[8:2]), 7 possible bits, each = 142, cap 1000
    let bt_count = popcount_branch_types(lo);
    let lbr_branch_types: u16 = (bt_count.saturating_mul(142)).min(1000) as u16;

    (lbr_kernel_filter, lbr_user_filter, lbr_branch_types)
}

pub fn init() {
    serial_println!("  life::msr_lbr_select: IA32_LBR_SELECT (0x1C8) filter observation online");
}

pub fn tick(age: u32) {
    // Sampling gate: every 3000 ticks
    if age % 3000 != 0 {
        return;
    }

    // CPUID guard: LBR requires PerfMon support (leaf 1 EDX bit 27)
    if !lbr_supported() {
        return;
    }

    let lo = read_lbr_select();
    let (lbr_kernel_filter, lbr_user_filter, lbr_branch_types) = derive(lo);

    // lbr_filter_ema composite input:
    //   kernel_filter/4 + user_filter/4 + branch_types/2
    // All division in u32, then EMA: (old * 7 + new_val) / 8
    let composite: u32 = (lbr_kernel_filter as u32 / 4)
        .saturating_add(lbr_user_filter as u32 / 4)
        .saturating_add(lbr_branch_types as u32 / 2);

    let mut s = STATE.lock();

    s.lbr_kernel_filter = lbr_kernel_filter;
    s.lbr_user_filter   = lbr_user_filter;
    s.lbr_branch_types  = lbr_branch_types;

    // EMA: (old * 7 + new_val) / 8, computed in u32, cast to u16
    let old_ema = s.lbr_filter_ema as u32;
    s.lbr_filter_ema =
        ((old_ema.wrapping_mul(7)).saturating_add(composite) / 8) as u16;

    serial_println!(
        "[lbr_select] kernel={} user={} branch_types={} filter_ema={}",
        s.lbr_kernel_filter,
        s.lbr_user_filter,
        s.lbr_branch_types,
        s.lbr_filter_ema,
    );
}

/// Non-locking snapshot: (lbr_kernel_filter, lbr_user_filter, lbr_branch_types, lbr_filter_ema)
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (
        s.lbr_kernel_filter,
        s.lbr_user_filter,
        s.lbr_branch_types,
        s.lbr_filter_ema,
    )
}
