#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_LBR_SELECT: u32 = 0x1C8;

pub struct State {
    pub lbr_filter_bits:  u16,
    pub lbr_cpl_mode:     u16,
    pub lbr_branch_types: u16,
    pub lbr_select_ema:   u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    lbr_filter_bits:  0,
    lbr_cpl_mode:     0,
    lbr_branch_types: 0,
    lbr_select_ema:   0,
});

// ── CPUID helpers ────────────────────────────────────────────────────────────

fn has_pdcm() -> bool {
    // CPUID leaf 1, ECX bit 15 → PDCM (Perfmon and Debug Capability MSR)
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            out("eax") _,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 15) & 1 == 1
}

fn perf_version() -> u32 {
    // CPUID leaf 0xA, EAX[7:0] → Architectural Perf Monitoring version
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 0xA",
            "cpuid",
            "pop rbx",
            out("eax") eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    eax & 0xFF
}

// ── MSR read ─────────────────────────────────────────────────────────────────

fn read_msr(addr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

// ── Popcount helper ───────────────────────────────────────────────────────────

fn popcount(mut v: u32) -> u32 {
    v = v - ((v >> 1) & 0x5555_5555);
    v = (v & 0x3333_3333) + ((v >> 2) & 0x3333_3333);
    v = (v + (v >> 4)) & 0x0F0F_0F0F;
    v = v.wrapping_mul(0x0101_0101) >> 24;
    v
}

// ── EMA ──────────────────────────────────────────────────────────────────────

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

// ── Signal derivation ────────────────────────────────────────────────────────

fn derive_lbr_filter_bits(lo: u32) -> u16 {
    let bits = lo & 0x3FF; // 10 filter bits [9:0]
    let count = popcount(bits);
    ((count * 1000 / 10) as u16).min(1000)
}

fn derive_lbr_cpl_mode(lo: u32) -> u16 {
    match lo & 0x3 {
        0 => 0,
        1 => 500, // CPL_EQ_0 only  → kernel-only
        2 => 500, // CPL_NEQ_0 only → user-only
        3 => 1000, // both           → dual-mode
        _ => 0,
    }
}

fn derive_lbr_branch_types(lo: u32) -> u16 {
    // bits [8:2] = 7 branch-type flags
    let bits = (lo >> 2) & 0x7F;
    let count = popcount(bits);
    ((count * 1000 / 7) as u16).min(1000)
}

// ── Public interface ─────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("[msr_ia32_lbr_select] init");
    if !has_pdcm() || perf_version() < 4 {
        serial_println!("[msr_ia32_lbr_select] PDCM or perf version < 4; module inactive");
        return;
    }
    let raw = read_msr(MSR_LBR_SELECT);
    let lo = raw as u32;

    let filter_bits  = derive_lbr_filter_bits(lo);
    let cpl_mode     = derive_lbr_cpl_mode(lo);
    let branch_types = derive_lbr_branch_types(lo);

    let mut s = MODULE.lock();
    s.lbr_filter_bits  = filter_bits;
    s.lbr_cpl_mode     = cpl_mode;
    s.lbr_branch_types = branch_types;
    s.lbr_select_ema   = filter_bits;

    serial_println!(
        "[msr_ia32_lbr_select] init ok: filter_bits={} cpl_mode={} branch_types={} ema={}",
        s.lbr_filter_bits, s.lbr_cpl_mode, s.lbr_branch_types, s.lbr_select_ema
    );
}

pub fn tick(age: u32) {
    if age % 7000 != 0 {
        return;
    }
    if !has_pdcm() || perf_version() < 4 {
        return;
    }

    let raw = read_msr(MSR_LBR_SELECT);
    let lo = raw as u32;

    let filter_bits  = derive_lbr_filter_bits(lo);
    let cpl_mode     = derive_lbr_cpl_mode(lo);
    let branch_types = derive_lbr_branch_types(lo);

    let mut s = MODULE.lock();
    s.lbr_filter_bits  = filter_bits;
    s.lbr_cpl_mode     = cpl_mode;
    s.lbr_branch_types = branch_types;
    s.lbr_select_ema   = ema(s.lbr_select_ema, filter_bits);

    serial_println!(
        "[msr_ia32_lbr_select] tick {}: filter_bits={} cpl_mode={} branch_types={} ema={}",
        age, s.lbr_filter_bits, s.lbr_cpl_mode, s.lbr_branch_types, s.lbr_select_ema
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_lbr_filter_bits() -> u16 {
    MODULE.lock().lbr_filter_bits
}

pub fn get_lbr_cpl_mode() -> u16 {
    MODULE.lock().lbr_cpl_mode
}

pub fn get_lbr_branch_types() -> u16 {
    MODULE.lock().lbr_branch_types
}

pub fn get_lbr_select_ema() -> u16 {
    MODULE.lock().lbr_select_ema
}
