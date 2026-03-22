#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// ─── State ────────────────────────────────────────────────────────────────────

struct State {
    rmid_capacity:    u16,
    l3_occupancy_en:  u16,
    l3_bw_en:         u16,
    rdt_richness_ema: u16,
    last_tick:        u32,
    initialized:      bool,
    rdt_supported:    bool,
}

static MODULE: Mutex<State> = Mutex::new(State {
    rmid_capacity:    0,
    l3_occupancy_en:  0,
    l3_bw_en:         0,
    rdt_richness_ema: 0,
    last_tick:        0,
    initialized:      false,
    rdt_supported:    false,
});

// ─── CPUID helpers ────────────────────────────────────────────────────────────

/// Returns the maximum basic CPUID leaf (EAX from leaf 0).
fn max_cpuid_leaf() -> u32 {
    let eax_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => eax_out,
            in("ecx") 0u32,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    eax_out
}

/// CPUID leaf 0x0F, sub-leaf 0.
/// Returns (ebx, edx): ebx = max RMID range, edx bit 1 = L3 monitoring supported.
fn cpuid_leaf_0f_sub0() -> (u32, u32) {
    let rbx_out: u32;
    let edx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {rbx_save:e}, ebx",
            "pop rbx",
            inout("eax") 0x0Fu32 => _,
            in("ecx") 0u32,
            out("edx") edx_out,
            rbx_save = out(reg) rbx_out,
            options(nostack, nomem),
        );
    }
    (rbx_out, edx_out)
}

/// CPUID leaf 0x0F, sub-leaf 1.
/// Returns (ebx, ecx, edx): ebx = conversion factor, ecx = max RMID for L3,
/// edx bits: 0=occupancy, 1=total BW, 2=local BW.
fn cpuid_leaf_0f_sub1() -> (u32, u32, u32) {
    let rbx_out: u32;
    let ecx_out: u32;
    let edx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {rbx_save:e}, ebx",
            "pop rbx",
            inout("eax") 0x0Fu32 => _,
            inout("ecx") 1u32 => ecx_out,
            out("edx") edx_out,
            rbx_save = out(reg) rbx_out,
            options(nostack, nomem),
        );
    }
    (rbx_out, ecx_out, edx_out)
}

// ─── EMA ──────────────────────────────────────────────────────────────────────

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

// ─── Composite signal ─────────────────────────────────────────────────────────

fn composite(rmid_capacity: u16, l3_occupancy_en: u16, l3_bw_en: u16) -> u16 {
    let v = ((rmid_capacity as u32) / 4)
        .saturating_add((l3_occupancy_en as u32) / 4)
        .saturating_add((l3_bw_en as u32) / 2);
    v.min(1000) as u16
}

// ─── Public API ───────────────────────────────────────────────────────────────

pub fn init() {
    let mut state = MODULE.lock();

    // Guard: max CPUID basic leaf must be >= 0x0F
    let max_leaf = max_cpuid_leaf();
    if max_leaf < 0x0F {
        state.rdt_supported = false;
        state.initialized   = true;
        serial_println!(
            "[cpuid_rdt_monitoring] init: max CPUID leaf {:#x} < 0x0F — RDT unavailable",
            max_leaf
        );
        return;
    }

    // Leaf 0x0F sub-leaf 0: EDX bit 1 = L3 cache monitoring supported
    let (sub0_ebx, sub0_edx) = cpuid_leaf_0f_sub0();
    if (sub0_edx >> 1) & 1 == 0 {
        state.rdt_supported = false;
        state.initialized   = true;
        serial_println!(
            "[cpuid_rdt_monitoring] init: L3 cache monitoring not supported (EDX bit 1 = 0)"
        );
        return;
    }

    state.rdt_supported = true;

    // rmid_capacity: EBX from sub-leaf 0, scaled (rmid.min(255) * 1000 / 255)
    let rmid_raw = sub0_ebx.min(255);
    let rmid_scaled = (rmid_raw * 1000 / 255) as u16;
    state.rmid_capacity = rmid_scaled;

    // Sub-leaf 1: L3 monitoring detail flags
    let (_sub1_ebx, _sub1_ecx, sub1_edx) = cpuid_leaf_0f_sub1();

    let l3_occ = if (sub1_edx >> 0) & 1 != 0 { 1000u16 } else { 0u16 };
    let l3_bw  = if (sub1_edx >> 1) & 1 != 0 { 1000u16 } else { 0u16 };

    state.l3_occupancy_en = l3_occ;
    state.l3_bw_en        = l3_bw;

    // Seed EMA with first composite reading
    let comp = composite(rmid_scaled, l3_occ, l3_bw);
    state.rdt_richness_ema = comp;

    state.initialized = true;

    serial_println!(
        "[cpuid_rdt_monitoring] init: rmid_capacity={} l3_occupancy_en={} l3_bw_en={} rdt_richness_ema={}",
        state.rmid_capacity,
        state.l3_occupancy_en,
        state.l3_bw_en,
        state.rdt_richness_ema,
    );
}

pub fn tick(age: u32) {
    const TICK_INTERVAL: u32 = 15000;

    let mut state = MODULE.lock();

    if !state.initialized {
        return;
    }

    if age.wrapping_sub(state.last_tick) < TICK_INTERVAL {
        return;
    }
    state.last_tick = age;

    if !state.rdt_supported {
        return;
    }

    // Re-read sub-leaf 0: RMID capacity and L3 monitoring presence
    let (sub0_ebx, sub0_edx) = cpuid_leaf_0f_sub0();
    if (sub0_edx >> 1) & 1 == 0 {
        state.rdt_supported = false;
        serial_println!(
            "[cpuid_rdt_monitoring] tick@{}: L3 monitoring capability lost",
            age
        );
        return;
    }

    let rmid_raw = sub0_ebx.min(255);
    let rmid_scaled = (rmid_raw * 1000 / 255) as u16;
    state.rmid_capacity = rmid_scaled;

    // Re-read sub-leaf 1: live capability flags
    let (_sub1_ebx, _sub1_ecx, sub1_edx) = cpuid_leaf_0f_sub1();

    let l3_occ = if (sub1_edx >> 0) & 1 != 0 { 1000u16 } else { 0u16 };
    let l3_bw  = if (sub1_edx >> 1) & 1 != 0 { 1000u16 } else { 0u16 };

    state.l3_occupancy_en = l3_occ;
    state.l3_bw_en        = l3_bw;

    let comp = composite(rmid_scaled, l3_occ, l3_bw);
    state.rdt_richness_ema = ema(state.rdt_richness_ema, comp);

    serial_println!(
        "[cpuid_rdt_monitoring] tick@{}: rmid_capacity={} l3_occupancy_en={} l3_bw_en={} rdt_richness_ema={}",
        age,
        state.rmid_capacity,
        state.l3_occupancy_en,
        state.l3_bw_en,
        state.rdt_richness_ema,
    );
}

// ─── Getters ──────────────────────────────────────────────────────────────────

pub fn get_rmid_capacity() -> u16 {
    MODULE.lock().rmid_capacity
}

pub fn get_l3_occupancy_en() -> u16 {
    MODULE.lock().l3_occupancy_en
}

pub fn get_l3_bw_en() -> u16 {
    MODULE.lock().l3_bw_en
}

pub fn get_rdt_richness_ema() -> u16 {
    MODULE.lock().rdt_richness_ema
}
