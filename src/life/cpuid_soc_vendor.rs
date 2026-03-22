#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ─── State ────────────────────────────────────────────────────────────────────

struct SocVendorState {
    vendor_id:   u16,
    project_id:  u16,
    stepping_id: u16,
    soc_ema:     u16,
}

impl SocVendorState {
    const fn zero() -> Self {
        Self {
            vendor_id:   0,
            project_id:  0,
            stepping_id: 0,
            soc_ema:     0,
        }
    }
}

static STATE: Mutex<SocVendorState> = Mutex::new(SocVendorState::zero());

// ─── CPUID guard ──────────────────────────────────────────────────────────────

/// Returns true if CPUID leaf 0x17 sub-leaf 0 EAX > 0 (SoC vendor data present).
fn has_soc_vendor() -> bool {
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
    if max_leaf < 0x17 {
        return false;
    }
    let eax_17: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x17u32 => eax_17,
            in("ecx") 0u32,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    eax_17 > 0
}

// ─── Hardware read ────────────────────────────────────────────────────────────

/// Read sub-leaf 0 of CPUID leaf 0x17.
/// EBX is captured via ESI to avoid LLVM register conflicts.
/// Returns (ebx_val, ecx_val, edx_val).
fn read_soc_subleaf0() -> (u32, u32, u32) {
    let ebx_val: u32;
    let ecx_val: u32;
    let edx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x17u32 => _,
            in("ecx") 0u32,
            out("esi") ebx_val,
            lateout("ecx") ecx_val,
            lateout("edx") edx_val,
            options(nostack, nomem)
        );
    }
    (ebx_val, ecx_val, edx_val)
}

// ─── Signal derivation ────────────────────────────────────────────────────────

/// vendor_id: EBX bits[9:0] (bottom 10 of the 16-bit VendorID), value 0–1023, capped at 1000.
fn derive_vendor_id(ebx_val: u32) -> u16 {
    ((ebx_val & 0x3FF) * 1).min(1000) as u16
}

/// project_id: ECX bits[15:0] mapped to 0–1000 by dividing by 65.
fn derive_project_id(ecx_val: u32) -> u16 {
    ((ecx_val & 0xFFFF) / 65).min(1000) as u16
}

/// stepping_id: EDX bits[15:0] × 15, capped at 1000.
fn derive_stepping_id(edx_val: u32) -> u16 {
    ((edx_val & 0xFFFF) * 15).min(1000) as u16
}

/// EMA formula: (old * 7 + new_val) / 8, computed in u32, cast to u16.
fn ema(old: u16, new_val: u16) -> u16 {
    (((old as u32) * 7 + (new_val as u32)) / 8) as u16
}

// ─── Sample (shared by init and tick) ─────────────────────────────────────────

fn sample(state: &mut SocVendorState) {
    if !has_soc_vendor() {
        return;
    }
    let (ebx_val, ecx_val, edx_val) = read_soc_subleaf0();

    let vendor_id   = derive_vendor_id(ebx_val);
    let project_id  = derive_project_id(ecx_val);
    let stepping_id = derive_stepping_id(edx_val);
    let soc_ema     = ema(state.soc_ema, vendor_id);

    state.vendor_id   = vendor_id;
    state.project_id  = project_id;
    state.stepping_id = stepping_id;
    state.soc_ema     = soc_ema;
}

// ─── Public API ───────────────────────────────────────────────────────────────

pub fn init() {
    let mut state = STATE.lock();
    sample(&mut state);
    crate::serial_println!(
        "[cpuid_soc_vendor] age=0 vendor={} project={} stepping={} ema={}",
        state.vendor_id,
        state.project_id,
        state.stepping_id,
        state.soc_ema,
    );
}

pub fn tick(age: u32) {
    if age % 10_000 != 0 {
        return;
    }
    let mut state = STATE.lock();
    sample(&mut state);
    crate::serial_println!(
        "[cpuid_soc_vendor] age={} vendor={} project={} stepping={} ema={}",
        age,
        state.vendor_id,
        state.project_id,
        state.stepping_id,
        state.soc_ema,
    );
}

pub fn get_vendor_id() -> u16 {
    STATE.lock().vendor_id
}

pub fn get_project_id() -> u16 {
    STATE.lock().project_id
}

pub fn get_stepping_id() -> u16 {
    STATE.lock().stepping_id
}

pub fn get_soc_ema() -> u16 {
    STATE.lock().soc_ema
}
