#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ─── State ────────────────────────────────────────────────────────────────────

struct RdtState {
    rdt_rmid_range: u16,
    rdt_l3_cap:     u16,
    rdt_bw_cap:     u16,
    rdt_ema:        u16,
}

static RDT: Mutex<RdtState> = Mutex::new(RdtState {
    rdt_rmid_range: 0,
    rdt_l3_cap:     0,
    rdt_bw_cap:     0,
    rdt_ema:        0,
});

// ─── CPUID helpers ────────────────────────────────────────────────────────────

fn has_rdt_monitoring() -> bool {
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
    if max_leaf < 0x0F {
        return false;
    }
    let edx_0f: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x0Fu32 => _,
            in("ecx") 0u32,
            lateout("ecx") _,
            lateout("edx") edx_0f,
            options(nostack, nomem)
        );
    }
    (edx_0f >> 1) & 1 != 0
}

/// Read CPUID leaf 0x0F, sub-leaf 0.
/// Returns (eax, ebx, ecx, edx).
fn cpuid_0f_sub0() -> (u32, u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let ecx_out: u32;
    let edx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            inout("eax") 0x0Fu32 => eax_out,
            in("ecx") 0u32,
            ecx_out = lateout(reg) ecx_out,
            edx_out = lateout(reg) edx_out,
            ebx_out = out(reg) ebx_out,
            options(nostack, nomem)
        );
    }
    (eax_out, ebx_out, ecx_out, edx_out)
}

/// Read CPUID leaf 0x0F, sub-leaf 1 (L3 cache monitoring details).
/// Returns (eax, ebx, ecx, edx).
fn cpuid_0f_sub1() -> (u32, u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let ecx_out: u32;
    let edx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            inout("eax") 0x0Fu32 => eax_out,
            in("ecx") 1u32,
            ecx_out = lateout(reg) ecx_out,
            edx_out = lateout(reg) edx_out,
            ebx_out = out(reg) ebx_out,
            options(nostack, nomem)
        );
    }
    (eax_out, ebx_out, ecx_out, edx_out)
}

// ─── Signal computation ───────────────────────────────────────────────────────

/// Map sub-leaf 0 EBX (max RMID range) to 0–1000.
/// Up to 65536 RMIDs → divide by 64, clamp to 1000.
fn map_rmid_range(ebx0: u32) -> u16 {
    let mapped = ebx0 / 64;
    if mapped > 1000 { 1000u16 } else { mapped as u16 }
}

/// EDX bit 0 of sub-leaf 1 → LLC occupancy supported → 0 or 1000.
fn map_l3_cap(edx1: u32) -> u16 {
    if edx1 & 1 != 0 { 1000 } else { 0 }
}

/// EDX bit 1 of sub-leaf 1 → total MBM supported → 0 or 1000.
fn map_bw_cap(edx1: u32) -> u16 {
    if (edx1 >> 1) & 1 != 0 { 1000 } else { 0 }
}

/// Composite: rmid_range/2 + l3_cap/4 + bw_cap/4, clamped to 0–1000.
fn composite(rmid: u16, l3: u16, bw: u16) -> u16 {
    let v = (rmid as u32) / 2 + (l3 as u32) / 4 + (bw as u32) / 4;
    if v > 1000 { 1000u16 } else { v as u16 }
}

/// EMA: (old * 7 + new_val) / 8, computed in u32, cast to u16.
fn ema(old: u16, new_val: u16) -> u16 {
    let v = ((old as u32) * 7 + (new_val as u32)) / 8;
    v as u16
}

// ─── Public API ───────────────────────────────────────────────────────────────

pub fn init() {
    if !has_rdt_monitoring() {
        crate::serial_println!(
            "[cpuid_rdt_monitoring] RDT monitoring not supported — signals zeroed"
        );
        return;
    }

    let (_eax0, ebx0, _ecx0, _edx0) = cpuid_0f_sub0();
    let (_eax1, _ebx1, _ecx1, edx1) = cpuid_0f_sub1();

    let rmid = map_rmid_range(ebx0);
    let l3   = map_l3_cap(edx1);
    let bw   = map_bw_cap(edx1);
    let comp = composite(rmid, l3, bw);

    let mut state = RDT.lock();
    state.rdt_rmid_range = rmid;
    state.rdt_l3_cap     = l3;
    state.rdt_bw_cap     = bw;
    state.rdt_ema        = comp; // seed EMA with first reading

    crate::serial_println!(
        "[cpuid_rdt_monitoring] init: rmid={} l3={} bw={} ema={}",
        rmid, l3, bw, comp
    );
}

pub fn tick(age: u32) {
    // Sample every 10000 ticks.
    if age % 10000 != 0 {
        return;
    }

    if !has_rdt_monitoring() {
        return;
    }

    let (_eax0, ebx0, _ecx0, _edx0) = cpuid_0f_sub0();
    let (_eax1, _ebx1, _ecx1, edx1) = cpuid_0f_sub1();

    let rmid = map_rmid_range(ebx0);
    let l3   = map_l3_cap(edx1);
    let bw   = map_bw_cap(edx1);
    let comp = composite(rmid, l3, bw);

    let mut state = RDT.lock();
    state.rdt_rmid_range = rmid;
    state.rdt_l3_cap     = l3;
    state.rdt_bw_cap     = bw;
    state.rdt_ema        = ema(state.rdt_ema, comp);

    crate::serial_println!(
        "[cpuid_rdt_monitoring] age={} rmid={} l3={} bw={} ema={}",
        age,
        state.rdt_rmid_range,
        state.rdt_l3_cap,
        state.rdt_bw_cap,
        state.rdt_ema
    );
}

// ─── Getters ──────────────────────────────────────────────────────────────────

pub fn get_rdt_rmid_range() -> u16 {
    RDT.lock().rdt_rmid_range
}

pub fn get_rdt_l3_cap() -> u16 {
    RDT.lock().rdt_l3_cap
}

pub fn get_rdt_bw_cap() -> u16 {
    RDT.lock().rdt_bw_cap
}

pub fn get_rdt_ema() -> u16 {
    RDT.lock().rdt_ema
}
