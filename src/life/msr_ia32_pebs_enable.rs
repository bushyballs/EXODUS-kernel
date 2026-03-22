#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct PebsState {
    pebs_pmc0_en:      u16,
    pebs_pmc1_en:      u16,
    pebs_active_count: u16,
    pebs_ema:          u16,
}

impl PebsState {
    const fn new() -> Self {
        Self {
            pebs_pmc0_en:      0,
            pebs_pmc1_en:      0,
            pebs_active_count: 0,
            pebs_ema:          0,
        }
    }
}

static STATE: Mutex<PebsState> = Mutex::new(PebsState::new());

// ---------------------------------------------------------------------------
// CPUID guard — PDCM (Perfmon and Debug Capability MSR): leaf 1, ECX bit 15
// ---------------------------------------------------------------------------

fn has_pdcm() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") ecx_val,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx_val >> 15) & 1 != 0
}

// ---------------------------------------------------------------------------
// MSR helpers
// ---------------------------------------------------------------------------

/// Read MSR 0x3F1 (MSR_PEBS_ENABLE). Returns the low 32 bits.
fn read_pebs_enable() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x3F1u32,
            lateout("eax") lo,
            lateout("edx") _hi,
            options(nostack, nomem)
        );
    }
    lo
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Count set bits in the low 4 bits of v.
fn popcount4(v: u32) -> u32 {
    let mut c = 0u32;
    let mut v = v & 0xF;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

/// EMA: (old * 7 + new_val) / 8, computed in u32, cast to u16.
fn ema(old: u16, new_val: u16) -> u16 {
    let result = ((old as u32) * 7 + (new_val as u32)) / 8;
    result as u16
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    if !has_pdcm() {
        crate::serial_println!(
            "[msr_ia32_pebs_enable] PDCM not supported — module disabled"
        );
        return;
    }
    let mut state = STATE.lock();
    *state = PebsState::new();
    crate::serial_println!("[msr_ia32_pebs_enable] init ok (PDCM present)");
}

pub fn tick(age: u32) {
    // Sample every 2000 ticks.
    if age % 2000 != 0 {
        return;
    }

    if !has_pdcm() {
        return;
    }

    let raw = read_pebs_enable();

    // Bit 0 → PMC0 PEBS enable
    let pmc0: u16 = if (raw >> 0) & 1 != 0 { 1000 } else { 0 };
    // Bit 1 → PMC1 PEBS enable
    let pmc1: u16 = if (raw >> 1) & 1 != 0 { 1000 } else { 0 };

    // popcount of bits[3:0] × 250, capped at 1000
    let pc = popcount4(raw);
    let active_raw: u32 = pc * 250;
    let active: u16 = if active_raw > 1000 { 1000 } else { active_raw as u16 };

    let mut state = STATE.lock();
    let new_ema = ema(state.pebs_ema, active);

    state.pebs_pmc0_en      = pmc0;
    state.pebs_pmc1_en      = pmc1;
    state.pebs_active_count = active;
    state.pebs_ema          = new_ema;

    crate::serial_println!(
        "[msr_ia32_pebs_enable] age={} pmc0={} pmc1={} active={} ema={}",
        age, pmc0, pmc1, active, new_ema
    );
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

pub fn get_pebs_pmc0_en() -> u16 {
    STATE.lock().pebs_pmc0_en
}

pub fn get_pebs_pmc1_en() -> u16 {
    STATE.lock().pebs_pmc1_en
}

pub fn get_pebs_active_count() -> u16 {
    STATE.lock().pebs_active_count
}

pub fn get_pebs_ema() -> u16 {
    STATE.lock().pebs_ema
}
